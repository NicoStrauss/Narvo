// One cascade stage: integrate what a probe's directions found.
//
// A probe has a position, a distance interval and a set of directions. M8.4's
// march has already taken every one of those directions over its own interval
// and written a verdict and a distance into the hit buffer; this kernel turns
// that list into one number per probe.
//
// One invocation per probe. It walks its own directions in a fixed loop order,
// so the summation order is the shader's and not the scheduler's. No shared
// memory, no barrier, no atomic - ADR-0049's rule obeyed by construction rather
// than by care. The parallelism comes from the probe grid, which is where a
// cascade has it: a level with few probes has many directions and vice versa,
// and the *product* is what stays constant.
//
// # There is no `f32` multiplication here, and that is the decision
//
// M8.0 measured DX12 contracting `a*b + c` into a fused multiply-add in 928 of
// 4096 expressions inside one run. A contraction rounds once where the written
// expression rounds twice, so it produces a *plausible* number - which is why no
// comparison of outputs can report it, and why the guard for this is a source
// read in `cascade.rs`.
//
// M8.5a measured what that does here, on eight adapter/backend pairs, in both
// profiles, twice each:
//
//   - written as a sum with **no float multiply** (this kernel), the radiance
//     field is **one field in 32 of 32 cells**, byte-identical, and equal to an
//     unfused CPU computing the same thing;
//   - written as `sum + r * 0.7` - one inexact product feeding an add - it is
//     **two** fields: the five AMD driver paths fuse it, and WARP and lavapipe
//     do not. One input, two fields, split by rasteriser family. That is
//     M8.3a's finding arriving in float arithmetic instead of in a comparison.
//
// So the rule this file is written under is sharper than "avoid `fma`": **no
// float multiply may feed a float add**, and the way this kernel guarantees it
// is by containing no float multiply at all. The normalisation is a *division*
// by the direction count, and there is no fused divide-add to fear.
//
// `params.directions` is required to be a power of two (`CascadeStage::new`),
// which makes that division exact - it only decrements an exponent. The three
// `*` below are all integer.
//
// A fourth measurement is recorded because it corrects the obvious guess:
// `sum + r * (1/D)` with a power-of-two `D` is contractible **and** returns the
// identical field on all eight pairs, because scaling by a power of two commutes
// with rounding. Being contractible is not enough; the product has to be
// inexact. And a fifth: `fma(r, 0.7, sum)` - the fusion asked for out loud - is
// computed *unfused* by WARP and by lavapipe, so WGSL's `fma` is not a way to
// force one either.

struct Params {
    // How many probes this dispatch covers. An invocation past this returns.
    probes: u32,
    // Directions per probe. A power of two, so the normalisation is exact.
    directions: u32,
    // The occluder field's extent, so a hit position can be clamped into it.
    width: u32,
    height: u32,
    // Probes per row of the output, to turn a probe index into a texel.
    grid_w: u32,
    // WGSL rounds a uniform struct to 16 bytes. Named rather than implicit, so
    // the Rust side and this side agree in writing - `transport.wgsl`'s form.
    pad_a: u32,
    pad_b: u32,
    pad_c: u32,
    // What a direction that left the interval without meeting anything
    // contributes. For a single stage this is the caller's far field; for a
    // cascade it is what M8.5b replaces with the level above.
    far_r: f32,
    far_g: f32,
    far_b: f32,
    pad_d: f32,
}

// M8.4's ray, unchanged. Eight words, built by `Ray::new` on the CPU.
struct Ray {
    from_x: i32,
    from_y: i32,
    dir_x: i32,
    dir_y: i32,
    length: i32,
    pad0: i32,
    pad1: i32,
    pad2: i32,
}

// M8.4's hit, unchanged. `verdict` is 0 blocked, 1 visible, 2 exhausted.
struct Hit {
    verdict: u32,
    distance: i32,
    steps: u32,
    pad: u32,
}

@group(0) @binding(0) var field: texture_2d<f32>;
@group(0) @binding(1) var emission: texture_2d<f32>;
@group(0) @binding(2) var<storage, read> rays: array<Ray>;
@group(0) @binding(3) var<storage, read> hits: array<Hit>;
@group(0) @binding(4) var radiance: texture_storage_2d<rgba32float, write>;
@group(0) @binding(5) var<uniform> params: Params;

const FIXED_SHIFT: u32 = 8u;
const FIXED: i32 = 256;
const BLOCKED: u32 = 0u;
const VISIBLE: u32 = 1u;

// Sixty-four, one dimension: a probe list is a list, exactly as a ray list is in
// `march.wgsl`. `cascade::CASCADE_WORKGROUP` divides the dispatch by the same
// number and a test holds the two together.
@compute @workgroup_size(64)
fn integrate(@builtin(global_invocation_id) id: vec3<u32>) {
    let probe = id.x;
    if (probe >= params.probes) {
        return;
    }

    let last_x = i32(params.width) - 1;
    let last_y = i32(params.height) - 1;

    var sum_r: f32 = 0.0;
    var sum_g: f32 = 0.0;
    var sum_b: f32 = 0.0;
    var escaped: f32 = 0.0;

    let base = probe * params.directions;
    var k: u32 = 0u;
    loop {
        if (k >= params.directions) {
            break;
        }
        let hit = hits[base + k];

        var r: f32 = 0.0;
        var g: f32 = 0.0;
        var b: f32 = 0.0;

        if (hit.verdict == VISIBLE) {
            // The direction left its interval without meeting anything, so what
            // it carries is whatever lies beyond - and beyond is the level
            // above, which this stage does not have. `escaped` records how much
            // of the answer is therefore still owed upward.
            r = params.far_r;
            g = params.far_g;
            b = params.far_b;
            escaped = escaped + 1.0;
        } else if (hit.verdict == BLOCKED) {
            // Where the march stopped, in the same fixed point it marched in.
            // Split across two statements so that no line multiplies and adds -
            // the arithmetic is integer either way, and the shape is what the
            // source guard reads.
            let ray = rays[base + k];
            let advance_x = (ray.dir_x * hit.distance) / FIXED;
            let advance_y = (ray.dir_y * hit.distance) / FIXED;
            let px = ray.from_x + advance_x;
            let py = ray.from_y + advance_y;
            let tx = clamp(px >> FIXED_SHIFT, 0, last_x);
            let ty = clamp(py >> FIXED_SHIFT, 0, last_y);

            // **The emission is read at the seed, not at the stopping point.**
            // A march stops up to one whole texel short of what stopped it
            // (`march.wgsl`'s MARGIN), so the texel under the stopping point is
            // often *not* the occluder - it is the empty texel in front of it,
            // and sampling there would read an emission of zero from a lamp.
            // The distance field already holds the coordinate of the nearest
            // seed at every texel, so the thing that blocked the ray names
            // itself, exactly and in integers.
            let seed = textureLoad(field, vec2<i32>(tx, ty), 0);
            if (seed.z != 0.0) {
                let sx = clamp(i32(seed.x), 0, last_x);
                let sy = clamp(i32(seed.y), 0, last_y);
                let emitted = textureLoad(emission, vec2<i32>(sx, sy), 0);
                r = emitted.x;
                g = emitted.y;
                b = emitted.z;
            }
        }
        // EXHAUSTED falls through with zero, and does so by saying nothing
        // rather than by a branch: a march that ran out of steps established
        // nothing, so it contributes nothing and is not counted as escaped
        // either. `MarchHit::is_visible` reads exhaustion the same way.

        sum_r = sum_r + r;
        sum_g = sum_g + g;
        sum_b = sum_b + b;
        k = k + 1u;
    }

    // The only float division in the kernel, and the reason there is no float
    // multiply anywhere above it. `params.directions` is a power of two, so this
    // is exact.
    let scale = f32(params.directions);
    let mean_r = sum_r / scale;
    let mean_g = sum_g / scale;
    let mean_b = sum_b / scale;
    let mean_v = escaped / scale;

    let out_x = i32(probe % params.grid_w);
    let out_y = i32(probe / params.grid_w);
    textureStore(radiance, vec2<i32>(out_x, out_y), vec4<f32>(mean_r, mean_g, mean_b, mean_v));
}
