// The circle march: how far a ray gets before it meets an occluder.
//
// Sphere tracing with one dimension fewer than Lumen runs it in 3D. At a point
// p the field says how far the nearest seeded texel is; the march steps that
// far, because nothing can be in the way; and it repeats until it either
// arrives or runs out of room.
//
// One invocation per ray. No shared memory, no barrier, no atomic: each ray
// reads the field and writes its own slot, so nothing here depends on the order
// invocations run in — ADR-0049's rule, obeyed by construction.
//
// # Everything is an integer, and that is the whole design
//
// **No `sqrt` appears here.** M8.3a promised that no square root would enter
// WGSL in this crate, and M8.4 keeps the promise rather than amending it. Two
// things made that possible:
//
//   - the field's distances are `isqrt` of an exact integer, refined by an
//     integer division (`distance_below`), which is a *lower* bound by
//     construction rather than a rounded value;
//   - the ray's unit direction is normalised once on the CPU, in `f64`, and
//     arrives here as fixed-point integers. A direction is a parameter of the
//     query, not something the march computes.
//
// Positions are fixed point, `FIXED` units to the texel. `>> FIXED_SHIFT` is an
// arithmetic shift, which floors for negative values too — that is the texel a
// position sits in, and it is deliberately not a truncation towards zero.
//
// # Why the step is shortened, and by exactly one texel
//
// **The field over-estimates.** It stores the distance to *a* seeded texel,
// which may not be the nearest one — M8.3a measured jump flooding keeping a seed
// up to 0.242480 texels too far. A sphere trace that stepped by an over-estimate
// would step past a thin occluder and report a visibility it never checked. So
// the step is the field's value minus a margin, and the margin is derived:
//
//   margin >= q + eps
//     q   = 0.707107  the most a position can be from its own texel's centre,
//                     which is sqrt(0.5) and is exact
//     eps = 0.242480  the most jump flooding was measured to over-estimate
//   q + eps = 0.949587, so one whole texel is enough, with 0.050413 to spare.
//
// M8.4 measured the alternative M8.3a handed over — two extra flooding passes
// drive `eps` to **0.000000** on every arrangement — and declined it: `q` cannot
// be reduced by any number of passes, so the best reachable margin is 0.707107
// against this 1.0, and the gain would rest on `eps = 0` being measured on six
// arrangements rather than proven. The report carries the numbers.

struct Params {
    // The field's extent in texels, so a position can be clamped into it.
    width: u32,
    height: u32,
    // How many rays the buffers hold. An invocation past this returns.
    ray_count: u32,
    // The most steps any one ray may take before it gives up. `march` derives a
    // provably sufficient value; `march_within` lets a caller ask for less.
    max_steps: u32,
}

// A ray, in fixed point. Built by `Ray::new` on the CPU, which is where the
// direction is normalised.
struct Ray {
    from_x: i32,
    from_y: i32,
    // The unit direction, times FIXED. Its length is one to within a unit.
    dir_x: i32,
    dir_y: i32,
    // How far it is to the far end, in fixed units.
    length: i32,
    pad0: i32,
    pad1: i32,
    pad2: i32,
}

// What one ray came back with. `verdict` is 0 blocked, 1 visible, 2 exhausted —
// and the three are separate on purpose: a march that ran out of steps has not
// established visibility, so calling it visible would be a claim it never
// checked.
struct Hit {
    verdict: u32,
    distance: i32,
    steps: u32,
    pad: u32,
}

@group(0) @binding(0) var field: texture_2d<f32>;
@group(0) @binding(1) var<storage, read> rays: array<Ray>;
@group(0) @binding(2) var<storage, read_write> hits: array<Hit>;
@group(0) @binding(3) var<uniform> params: Params;

const FIXED_SHIFT: u32 = 8u;
const FIXED: i32 = 256;
// One texel. The derivation is in this file's header.
const MARGIN: i32 = 256;

const BLOCKED: u32 = 0u;
const VISIBLE: u32 = 1u;
const EXHAUSTED: u32 = 2u;

// floor(sqrt(value)), by binary search over the sixteen bits a `u32` root can
// have. No `sqrt`, no float, and the same answer on every machine because every
// operation is an integer one.
fn isqrt(value: u32) -> u32 {
    var root: u32 = 0u;
    var bit: u32 = 1u << 30u;
    var rest: u32 = value;
    loop {
        if (bit == 0u) {
            break;
        }
        if (rest >= root + bit) {
            rest = rest - (root + bit);
            root = (root >> 1u) + bit;
        } else {
            root = root >> 1u;
        }
        bit = bit >> 2u;
    }
    return root;
}

// A **lower** bound on `sqrt(squared)`, in fixed point.
//
// `isqrt` gives the whole part; the fraction is refined by `r / (2w + 1)`, which
// under-estimates because `(w + r/(2w+1))^2 < w^2 + r` for every positive `w` —
// the denominator is one larger than the tangent's would be, and that one is
// what turns an over-estimate into an under-estimate. Both terms floor, so the
// result is never above the true root and the march's step stays conservative.
fn distance_below(squared: u32) -> i32 {
    let whole = isqrt(squared);
    let rest = squared - whole * whole;
    let fraction = (rest << FIXED_SHIFT) / (2u * whole + 1u);
    return i32(whole << FIXED_SHIFT) + i32(fraction);
}

@compute @workgroup_size(64)
fn march(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= params.ray_count) {
        return;
    }
    let ray = rays[index];

    let last_x = i32(params.width) - 1;
    let last_y = i32(params.height) - 1;

    var travelled: i32 = 0;
    var steps: u32 = 0u;
    var verdict: u32 = EXHAUSTED;

    loop {
        if (steps >= params.max_steps) {
            // Out of steps, and therefore out of anything to say. The verdict
            // stays EXHAUSTED, which is not VISIBLE.
            break;
        }
        steps = steps + 1u;

        // Recomputed from `travelled` rather than accumulated, so the division's
        // truncation happens once instead of compounding over the march.
        let px = ray.from_x + (ray.dir_x * travelled) / FIXED;
        let py = ray.from_y + (ray.dir_y * travelled) / FIXED;

        // The texel this position sits in. The clamp is defence against a
        // position exactly on the far edge; `Ray::new` refuses an endpoint
        // outside the field, and that refusal is the guard — M8.3b measured that
        // a clamp can mask an off-by-one, so it is not asked to be one.
        let tx = clamp(px >> FIXED_SHIFT, 0, last_x);
        let ty = clamp(py >> FIXED_SHIFT, 0, last_y);

        let texel = textureLoad(field, vec2<i32>(tx, ty), 0);
        if (texel.z == 0.0) {
            // No seed reached this texel, so the field holds no occluder at all
            // and nothing can be in the way.
            verdict = VISIBLE;
            travelled = ray.length;
            break;
        }

        let dx = i32(texel.x) - tx;
        let dy = i32(texel.y) - ty;
        let squared = u32(dx * dx + dy * dy);
        let step = distance_below(squared) - MARGIN;

        if (step <= 0) {
            // Within a texel of a seed: stop here rather than step into it.
            verdict = BLOCKED;
            break;
        }

        travelled = travelled + step;
        if (travelled >= ray.length) {
            verdict = VISIBLE;
            travelled = ray.length;
            break;
        }
    }

    hits[index] = Hit(verdict, travelled, steps, 0u);
}
