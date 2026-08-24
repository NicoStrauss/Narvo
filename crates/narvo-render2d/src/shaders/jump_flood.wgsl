// The jump-flooding kernel: one pass of the chain that turns a set of seed
// texels into, for every texel, the coordinate of the nearest seed.
//
// A texel is `vec4<f32>(seed_x, seed_y, has_seed, 0)`. `has_seed` is 0 or 1 and
// nothing else. An all-zero texel therefore reads as "no seed", which is what a
// field nobody wrote looks like — chosen over a negative sentinel for exactly
// that reason: a kernel reading an unwritten field produces a field with no
// seeds in it, which is obviously wrong, rather than a field in which every
// texel claims the seed at the origin, which is a plausible picture.
//
// # The arithmetic, which is the decision this file exists to carry
//
// **Every comparison is in `i32`.** Seed coordinates are integers, squared
// distances between integers are integers, and integer comparison has no
// rounding and no contraction to have. The alternative — `dx*dx + dy*dy` in
// `f32`, defended by the magnitude argument M8.2 used — was built as a control
// and measured against this one on eight adapter/backend pairs, in both
// profiles, twice each. Two results decided it, and both are in
// `target/reports/M8.3a-Distanzfeld.md`:
//
//   - The `f32` form gives an answer that disagrees with brute force from a
//     squared distance of 2^24 upward. The measured witness is a field 8192
//     texels wide — which is exactly `OffscreenTarget::MAX_DIMENSION` — with two
//     seeds one row apart: the first wrong texel is at x = 4096 on all eight
//     pairs, and 3248 or 4096 of the 8192 texels in that row keep the farther
//     seed. The `i32` form is wrong in none of them.
//   - Worse, the `f32` form is wrong *differently on different machines*: the
//     three AMD paths (Vulkan, DX12, GL) report 3248 and the three software
//     rasterisers (WARP on DX12, llvmpipe on Vulkan and GL) report 4096. One
//     input, two fields. The `i32` form returned **one** field in 16 of 16 cells.
//
// So the second half is the sharper one: the `f32` form does not merely round,
// it rounds differently per translator, which is M8.0's contraction finding
// arriving in this kernel. `i32` has no such freedom to exercise.
//
// The bound up to which the `f32` form is unobservable is therefore
// **a squared distance below 2^24**, and it is measured rather than chosen: at
// 2^24 exactly, `x*x` and `x*x + 1` land on one `f32`. Nothing here relies on
// it.
//
// `*` appears, and that is not a contradiction of the transport kernel's rule.
// M8.0 measured contraction of `a*b + c` in **`f32`**; there is no fused
// multiply-add over integers and no rounding to fuse. The one guard that could
// not be written as a behaviour — an output comparison cannot see a contraction,
// because a contracted expression produces a plausible number — is written as a
// source read in `sdf.rs`.

struct Params {
    // The jump distance for this pass. Descends by halves from the largest
    // power of two below the longer side, down to one.
    step: u32,
    // The field's extent, so an invocation past the edge can return and a probe
    // past the edge can be skipped. The dispatch is rounded up to whole
    // workgroups, so the last one runs partly outside on any size that is not a
    // multiple of the workgroup.
    width: u32,
    height: u32,
    // WGSL rounds a uniform struct up to 16 bytes anyway. Named rather than
    // implicit so the Rust side and this side agree in writing, exactly as
    // `transport.wgsl` does.
    padding: u32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var destination: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> params: Params;

// Eight by eight, the same 64 invocations `transport.wgsl` uses and for the same
// measured reason: `max_compute_invocations_per_workgroup` came back as 256 on
// all eight adapter/backend pairs, so this is a quarter of the smallest limit
// seen. `compute::WORKGROUP_SIDE` divides the dispatch by the same number and a
// test in `sdf.rs` holds the two together.
//
// No `workgroupBarrier`, no `var<workgroup>` and no atomic: every invocation
// writes the one texel it owns and reads only texels the *previous* pass wrote,
// which is what the ping-pong guarantees. That is the rule `compute.rs`'s header
// states — a merge may not be written order-dependently — obeyed by
// construction rather than by care.
@compute @workgroup_size(8, 8)
fn jump_flood(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let width = i32(params.width);
    let height = i32(params.height);
    let x = i32(id.x);
    let y = i32(id.y);
    let step = i32(params.step);

    var best_x: i32 = 0;
    var best_y: i32 = 0;
    var best_d: i32 = 0;
    var found: bool = false;

    for (var oy: i32 = -1; oy <= 1; oy = oy + 1) {
        for (var ox: i32 = -1; ox <= 1; ox = ox + 1) {
            let nx = x + ox * step;
            let ny = y + oy * step;
            // **This test is defence against unspecified behaviour, and not a
            // correctness requirement of the algorithm — measured, because the
            // first version of this comment claimed the opposite.**
            //
            // Two injections say so. Deleting the test outright moved no guard,
            // and replacing it with a `clamp` moved no guard either, on Vulkan
            // and on DX12 alike. The clamp result is the interesting one and its
            // reason is a property of jump flooding rather than of a backend:
            // every value a probe can return is a coordinate some seed really
            // occupies, so an extra read can only offer a *valid* candidate, and
            // a valid candidate can only improve the answer or leave it alone.
            // There is no coordinate a clamped read can invent.
            //
            // What is not benign is losing a probe that should have happened:
            // an off-by-one here (`nx >= width - 1`) drops the last column's
            // seeds and fells five of the oracles in `sdf.rs`. That is the
            // border defect this test guards against.
            //
            // It stays because an out-of-range `textureLoad` returning zero is
            // something wgpu 30.0.0 was *measured* doing on Vulkan and DX12 on
            // one machine, not something this kernel is entitled to. Skipping
            // makes the answer independent of it.
            if (nx < 0 || nx >= width || ny < 0 || ny >= height) {
                continue;
            }
            let probe = textureLoad(source, vec2<i32>(nx, ny), 0);
            if (probe.z == 0.0) {
                continue;
            }
            let sx = i32(probe.x);
            let sy = i32(probe.y);
            let dx = sx - x;
            let dy = sy - y;
            let d = dx * dx + dy * dy;
            // The order is total: squared distance, then the seed's row, then
            // its column. A total order is what makes the answer independent of
            // the order the nine probes happen to be visited in, and it is what
            // lets a brute force on the CPU name the same seed rather than
            // merely an equally distant one.
            var better: bool = !found;
            if (found) {
                if (d < best_d) {
                    better = true;
                } else if (d == best_d) {
                    if (sy < best_y) {
                        better = true;
                    } else if (sy == best_y && sx < best_x) {
                        better = true;
                    }
                }
            }
            if (better) {
                found = true;
                best_d = d;
                best_x = sx;
                best_y = sy;
            }
        }
    }

    var out = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if (found) {
        // The coordinate, never the distance. A field that propagated distances
        // would have thrown away what the next pass needs; the distance is
        // derived once, at the end, on the CPU (`SeedMap::distance_squared`).
        out = vec4<f32>(f32(best_x), f32(best_y), 1.0, 0.0);
    }
    textureStore(destination, vec2<i32>(x, y), out);
}
