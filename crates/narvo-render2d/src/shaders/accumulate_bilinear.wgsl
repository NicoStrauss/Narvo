// The other arm of the reprojection: the four surrounding probes, weighted by
// where between them the source landed.
//
// **This file is inexact on purpose, and it is the only file in this crate of
// which that is true.** ADR-0051 forbids an `f32` multiply feeding an `f32` add,
// having measured that shape returning two radiance fields across eight
// adapter/backend pairs where the unfused form returns one. A bilinear resample
// is four such products summed, and unlike the temporal blend in
// `accumulate.wgsl` there is no way to write it otherwise: the weights are the
// fractional part of a continuous camera motion, so they are arbitrary dyadic
// fractions rather than powers of two, and `v * 0.3` is not exact however it is
// spelled.
//
// **It exists so that a measurement has two arms.** M8.7's §2 asks how steppy a
// nearest-neighbour reprojection is, and that is not answerable without something
// to be steppy against; M8.5b and M8.6 kept `MergeForm::Aggregate` for the same
// reason, and their reports say what a measurement with one arm deleted is worth.
// The default is `Resample::Nearest` and this arm is the alternative, not the
// other way round.
//
// The index arithmetic is `accumulate.wgsl`'s and integer for ADR-0050's reason;
// only the weighting is float. The disocclusion rule is the same one, with one
// difference named where it happens.

struct Accumulate {
    probes_x: u32,
    probes_y: u32,
    offset_x: i32,
    offset_y: i32,
    divisor: u32,
    has_history: u32,
    pad0: u32,
    pad1: u32,
}

const SHIFT: u32 = 16u;
const UNIT: i32 = 65536;

@group(0) @binding(0) var source_a: texture_2d<f32>;
@group(0) @binding(1) var source_b: texture_2d<f32>;
@group(0) @binding(2) var written: texture_storage_2d<rgba32float, write>;
@group(0) @binding(3) var<uniform> params: Accumulate;

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

    let sx = (out.x << SHIFT) - params.offset_x;
    let sy = (out.y << SHIFT) - params.offset_y;

    // Floor, and the fraction that is left over. `>> SHIFT` on a signed value
    // floors on both sides of zero, so the fraction is never negative.
    let x0 = sx >> SHIFT;
    let y0 = sy >> SHIFT;
    let fx = sx - (x0 << SHIFT);
    let fy = sy - (y0 << SHIFT);

    // **The second tap is the first one again where the fraction is zero.** That
    // is what makes a motion of whole probes — and a motion of none — collapse
    // onto a single probe and read exactly, rather than reading one probe and its
    // neighbour at weights one and zero. `cascade.wgsl` uses the same trick on its
    // own upsample and for the same reason.
    let x1 = x0 + select(0, 1, fx != 0);
    let y1 = y0 + select(0, 1, fy != 0);

    if (x0 < 0 || y0 < 0 || x1 >= i32(params.probes_x) || y1 >= i32(params.probes_y)) {
        // No history, as `accumulate.wgsl` explains. **The band is one probe
        // wider on the trailing edge than the nearest arm's**, because a
        // footprint needs both of its taps and a round needs only the one it
        // lands on. That is a real difference between the two arms and it is
        // named here rather than left to be found in a picture.
        textureStore(written, out, fresh);
        return;
    }

    // Exact: `fx` is below 65536 so `f32(fx)` is exact, and `UNIT` is a power of
    // two so the division is. `1 - w` is exact too — the result needs at most
    // sixteen significant bits below one and an `f32` carries twenty-four.
    let wx = f32(fx) / f32(UNIT);
    let wy = f32(fy) / f32(UNIT);

    let v00 = textureLoad(source_a, vec2<i32>(x0, y0), 0);
    let v10 = textureLoad(source_a, vec2<i32>(x1, y0), 0);
    let v01 = textureLoad(source_a, vec2<i32>(x0, y1), 0);
    let v11 = textureLoad(source_a, vec2<i32>(x1, y1), 0);

    // **The six float multiplies of this crate's lighting path, and every one of
    // them feeds an addition.** That is the property ADR-0051 forbids, written
    // deliberately, in the one file whose reason for existing is to be measured
    // against the file that does not.
    let top = v00 * (1.0 - wx) + v10 * wx;
    let bottom = v01 * (1.0 - wx) + v11 * wx;
    let value = top * (1.0 - wy) + bottom * wy;

    textureStore(written, out, value);
}
