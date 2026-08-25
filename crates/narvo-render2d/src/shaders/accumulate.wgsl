// Reprojection and the temporal blend: what one frame lit, carried into the
// next frame's grid.
//
// **M8.7's capability, and it is two entry points for M8.6's reason** — a texel
// store between them, so neither pipeline is ever handed a texture it also
// writes. The recurrence is
//
//     accumulated_{n+1} = reproject(accumulated_n) + (fresh - reproject(accumulated_n)) / divisor
//
// and the two halves are `reproject` and `blend`.
//
// # There is no float multiply in this file, and that is the whole design
//
// ADR-0051 forbids an `f32` multiply feeding an `f32` add, having measured that
// exactly that shape returns **two** radiance fields across eight adapter/backend
// pairs where the unfused form returns one. A temporal blend is `h + (f - h) * a`,
// which is that shape written out.
//
// Three things remove it rather than tolerate it:
//
//   - **`a` is a negative power of two**, so the product is exact. ADR-0051
//     measured this directly: a power-of-two product returns the identical field,
//     because scaling by a power of two commutes with rounding.
//   - **It is written as a division rather than a multiplication.** There is no
//     fused divide-add in WGSL, SPIR-V or DXIL, so a backend has no contraction
//     to perform even if it wanted one. `cascade.wgsl` reaches the same place by
//     the same route, and its comment says so in as many words.
//   - **The reprojection performs no arithmetic on a radiance value at all.** It
//     is a gather: an integer index in, a texel out. Nothing is weighted, so
//     nothing rounds.
//
// The consequence is that the exact reference regime survives the accumulation,
// which is a stronger claim than M8.6 could make for the feedback and is the one
// M8.7's §2 asked to be measured rather than assumed.
//
// `accumulate_bilinear.wgsl` is the other arm, and it is inexact on purpose: it
// exists so that "how steppy is nearest" has something to be steppy against.
//
// # Why the index arithmetic is integer
//
// ADR-0050. A comparison over coordinates is written in `i32` because `f32` names
// the wrong seed once a squared distance passes 2^24 and does so *differently per
// rasteriser family*. A reprojection offset is the same kind of quantity, and the
// camera motion that produces it is continuous — so it arrives here already
// converted to fixed point, `UNIT` steps to the probe, rounded once on the CPU in
// `f64`. **No shader in this crate ever sees a float motion.**
//
// # Disocclusion is not a branch in the blend
//
// What moves in from outside the previous grid has no history. Giving it one is
// ghosting: a plausible picture over a wrong field. So `reproject` writes the
// **fresh** value at every probe whose source lies outside the grid, and `blend`
// then computes `f + (f - f) / d`, which is `f` exactly. The first frame is the
// same case with `has_history` at zero, so it needs no second code path either.
//
// **The limit that names itself:** the only disocclusion this sees is the grid's
// own edge. A probe that was behind an occluder last frame and is not now has a
// history that is wrong rather than absent, and nothing here detects it. That is
// a screen-space reprojection's classical gap and it is named rather than hidden.

struct Accumulate {
    // The probe grid's extent, so an invocation past the edge returns.
    probes_x: u32,
    probes_y: u32,
    // How far the content moved, in probes, as fixed point with UNIT to the
    // probe. Rounded once on the CPU; see the header.
    offset_x: i32,
    offset_y: i32,
    // A power of two. The fresh frame's share of the answer is `1 / divisor`.
    divisor: u32,
    // Zero on the first frame, when there is nothing to reproject.
    has_history: u32,
    pad0: u32,
    pad1: u32,
}

// Fixed-point units to one probe. Sixteen bits of fraction, which is finer than
// any camera speed a frame can carry and leaves fifteen bits of whole probes
// before an `i32` product could overflow. `MAX_REPROJECT_OFFSET` in
// `accumulate.rs` is what holds that, and a test compares the two.
const SHIFT: u32 = 16u;
const HALF: i32 = 32768;

@group(0) @binding(0) var source_a: texture_2d<f32>;
@group(0) @binding(1) var source_b: texture_2d<f32>;
@group(0) @binding(2) var written: texture_storage_2d<rgba32float, write>;
@group(0) @binding(3) var<uniform> params: Accumulate;

// Eight by eight, two dimensions: a probe grid is a field, exactly as
// `bounce.wgsl` and `jump_flood.wgsl` walk one. `accumulate::ACCUMULATE_WORKGROUP`
// divides the dispatch by the same number and a test holds the two together.
//
// `source_a` is the accumulated field of the previous frame, `source_b` is this
// frame's fresh radiance, and `written` is the previous field resampled into this
// frame's grid.
@compute @workgroup_size(8, 8)
fn reproject(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.probes_x || id.y >= params.probes_y) {
        return;
    }
    let out = vec2<i32>(i32(id.x), i32(id.y));
    let fresh = textureLoad(source_b, out, 0);

    if (params.has_history == 0u) {
        textureStore(written, out, fresh);
        return;
    }

    // Where this probe's content sat in the previous grid. Integer throughout:
    // `<< SHIFT` scales a whole probe index into fixed point and the offset is
    // already there.
    let sx = (out.x << SHIFT) - params.offset_x;
    let sy = (out.y << SHIFT) - params.offset_y;

    // The nearest probe. `>> SHIFT` on a signed value floors, so adding half a
    // probe first is a round rather than a truncation, and it rounds the same way
    // on both sides of zero.
    let ix = (sx + HALF) >> SHIFT;
    let iy = (sy + HALF) >> SHIFT;

    if (ix < 0 || iy < 0 || ix >= i32(params.probes_x) || iy >= i32(params.probes_y)) {
        // No history. The header says why this is the fresh value and not a
        // clamped neighbour.
        textureStore(written, out, fresh);
        return;
    }

    textureStore(written, out, textureLoad(source_a, vec2<i32>(ix, iy), 0));
}

// `source_a` is the reprojected history, `source_b` is this frame's fresh
// radiance, and `written` is what the next frame will reproject.
//
// **The only float arithmetic in this file**, and it is one subtraction, one
// division by a power of two and one addition per channel — written out per
// channel rather than as a vector so that the source guard can read them. The
// fourth channel is the share of directions that escaped, and it is carried
// through the same blend as the three colours: it is a value the accumulation
// owns, not a flag.
@compute @workgroup_size(8, 8)
fn blend(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.probes_x || id.y >= params.probes_y) {
        return;
    }
    let texel = vec2<i32>(i32(id.x), i32(id.y));
    let history = textureLoad(source_a, texel, 0);
    let fresh = textureLoad(source_b, texel, 0);
    let divisor = f32(params.divisor);
    textureStore(
        written,
        texel,
        vec4<f32>(
            history.x + (fresh.x - history.x) / divisor,
            history.y + (fresh.y - history.y) / divisor,
            history.z + (fresh.z - history.z) / divisor,
            history.w + (fresh.w - history.w) / divisor,
        ),
    );
}
