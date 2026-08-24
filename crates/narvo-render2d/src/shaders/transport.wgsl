// The transport kernel: the one compute entry point M8.2 ships.
//
// It exists so that the multi-pass machinery has something to carry, and it is
// written to be an *oracle* rather than an effect. Three channels answer three
// different questions about a chain, and a fourth answers none on purpose.
//
//   x : value.x + value.x + step   — order-sensitive
//   y : value.y + value.y          — count-sensitive
//   z : value.z                    — must not move
//   w : value.w                    — must not move
//
// After n passes with steps s1..sn:
//
//   x = x0 * 2^n + s1 * 2^(n-1) + s2 * 2^(n-2) + … + sn
//   y = y0 * 2^n
//   z, w unchanged
//
// The x channel separates two orderings of the same steps, which no sum can; the
// y channel separates n passes from one, which no per-pass value can; and z and w
// fail the moment a pass invents a texel rather than moving one.
//
// # Arithmetic (§3 of the M8.2 brief)
//
// Only `+` appears here, on f32, and `+` is one of the three operations M8.0
// measured as reproducible. There is no `*`, no `/`, no `sqrt` and nothing
// transcendental, so there is no `a*b+c` for a translator to contract — M8.0
// measured DX12 contracting 928 of 4096 such expressions inside a single run,
// and a comparison against a contractable expression measures the translator
// rather than the pipeline.
//
// The doubling is written `value + value` rather than `value * 2.0` for exactly
// that reason. A translator is still free to strength-reduce it to a multiply and
// then fuse — nothing in WGSL can forbid that — so the second half of the defence
// is magnitude: every value the tests put through this shader is an exact integer
// far below 2^24, where a multiply by two and a following add are exact in either
// form. Contraction is therefore unobservable here by construction rather than by
// hope.
//
// `f32(params.step)` is a conversion and not an arithmetic operation; it is exact
// for every u32 below 2^24.

struct Params {
    // What this pass adds to the x channel. M8.3's jump flooding needs exactly
    // this shape: one integer per pass, halving each time.
    step: u32,
    // The field's extent, so an invocation outside it can return. The dispatch
    // is rounded up to whole workgroups, so the last one runs partly past the
    // edge on any size that is not a multiple of the workgroup.
    width: u32,
    height: u32,
    // WGSL rounds a uniform struct up to 16 bytes anyway. Named rather than
    // implicit so the Rust side and this side agree in writing.
    padding: u32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var destination: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> params: Params;

// Eight by eight, which is 64 invocations — a quarter of the 256 that
// `max_compute_invocations_per_workgroup` reported on all eight adapter/backend
// pairs the M8.2 probe measured, so no configuration is near its limit.
//
// No `workgroupBarrier`, no `var<workgroup>` and no atomic anywhere: every
// invocation writes exactly the one texel it owns and reads no texel another
// invocation writes. That is what makes the result independent of the order the
// invocations run in, which §4 measured to be the difference between a value that
// reproduces and one that does not.
@compute @workgroup_size(8, 8)
fn transport(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let coord = vec2<i32>(i32(id.x), i32(id.y));
    let value = textureLoad(source, coord, 0);
    let step = f32(params.step);

    let moved = vec4<f32>(
        value.x + value.x + step,
        value.y + value.y,
        value.z,
        value.w,
    );
    textureStore(destination, coord, moved);
}
