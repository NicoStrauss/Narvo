// One cascade level, keeping what `cascade.wgsl` throws away: the direction.
//
// The aggregate form composes a level with the one above it by applying **one**
// upper value to every direction that escaped. That is what makes it cheap - one
// texel per probe instead of one per probe per direction - and it is also
// exactly what makes it wrong in a way M8.5b measures: an upper level's interval
// *starts* beyond the lower level's, so an upper probe can see a source that the
// lower probe's line of sight is walled off from, and the aggregate form spreads
// that light over every escaping direction including the ones pointing at the
// wall.
//
// This kernel keeps a radiance per direction. A direction that escapes takes the
// radiance of the **matching** directions above it: level `n+1` has four times
// the directions of level `n`, so direction `k` here corresponds to directions
// `4k`, `4k+1`, `4k+2` and `4k+3` there, which span the same arc.
//
// # The arithmetic, and why ADR-0051 is untouched
//
// **There is no `f32` multiplication in this file either.** Two averages are
// needed and both are divisions by four:
//
//   - over the four upper directions that share this direction's arc;
//   - over the four upper probes that surround this probe.
//
// The second is the bilinear interpolation of an aligned two-to-one grid, whose
// weights are 1, 1/2 and 1/4. Summing four samples - the same sample twice where
// a weight is 1 - and dividing by four *is* that weighting, written without a
// multiply. Both sums are pairwise, so four equal samples come to exactly four
// times one, and the even/even case returns its upper probe exactly.
//
// The `*` that do appear are all integer index arithmetic, and `cascade.rs`'s
// source guard holds the exact list.
//
// One invocation per probe, walking its directions in a written loop: no atomic,
// no barrier, no workgroup storage. ADR-0049's rule, obeyed by construction.

struct Params {
    probes: u32,
    directions: u32,
    width: u32,
    height: u32,
    grid_w: u32,
    upper_w: u32,
    upper_h: u32,
    has_upper: u32,
    far_r: f32,
    far_g: f32,
    far_b: f32,
    pad_d: f32,
}

struct Ray {
    from_x: i32,
    from_y: i32,
    dir_x: i32,
    dir_y: i32,
    length: i32,
    // Set by `CascadeStage::rays` when this direction's interval began beyond
    // the field's edge, so it met nothing and must carry the level above.
    escaping: i32,
    pad1: i32,
    pad2: i32,
}

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
// The level above, one entry per (probe, direction). Four floats an entry, the
// fourth unused, so the layout matches the field format's sixteen bytes.
@group(0) @binding(6) var<storage, read> upper: array<vec4<f32>>;
// This level's own per-direction radiance, for the level below to read.
@group(0) @binding(7) var<storage, read_write> outgoing: array<vec4<f32>>;

const FIXED_SHIFT: u32 = 8u;
const FIXED: i32 = 256;
const BLOCKED: u32 = 0u;
const VISIBLE: u32 = 1u;

// The four upper directions covering one of this level's directions, averaged.
//
// `at` is the index of the first of them. Pairwise, and the division is by four.
fn upper_arc(at: u32) -> vec3<f32> {
    let a = upper[at].xyz;
    let b = upper[at + 1u].xyz;
    let c = upper[at + 2u].xyz;
    let d = upper[at + 3u].xyz;
    // Halved at every step rather than summed and quartered, so the weights
    // stay exact at every step. It is **not** a fix for the cross-backend
    // divergence M8.5b measured in the spatial interpolation - `cascade.wgsl`
    // carries that measurement, and the halved and flat forms were shown to
    // produce bit-identical values.
    let first = (a + b) / 2.0;
    let second = (c + d) / 2.0;
    return (first + second) / 2.0;
}

@compute @workgroup_size(64)
fn integrate_directional(@builtin(global_invocation_id) id: vec3<u32>) {
    let probe = id.x;
    if (probe >= params.probes) {
        return;
    }

    let last_x = i32(params.width) - 1;
    let last_y = i32(params.height) - 1;

    // The four upper probes this one sits between. The aligned two-to-one cut:
    // an even index lands on an upper probe, an odd index halfway between two,
    // and sampling the same probe twice is how a weight of 1 or 1/2 is written
    // without a multiply.
    let out_x = i32(probe % params.grid_w);
    let out_y = i32(probe / params.grid_w);
    let last_ux = i32(params.upper_w) - 1;
    let last_uy = i32(params.upper_h) - 1;
    let x0 = clamp(out_x >> 1, 0, last_ux);
    let y0 = clamp(out_y >> 1, 0, last_uy);
    let x1 = clamp((out_x >> 1) + (out_x & 1), 0, last_ux);
    let y1 = clamp((out_y >> 1) + (out_y & 1), 0, last_uy);

    let upper_directions = params.directions * 4u;
    let row0 = u32(y0) * params.upper_w;
    let row1 = u32(y1) * params.upper_w;
    let base00 = (row0 + u32(x0)) * upper_directions;
    let base10 = (row0 + u32(x1)) * upper_directions;
    let base01 = (row1 + u32(x0)) * upper_directions;
    let base11 = (row1 + u32(x1)) * upper_directions;

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
        let ray = rays[base + k];

        var carried = vec3<f32>(0.0, 0.0, 0.0);

        if (ray.escaping != 0 || hit.verdict == VISIBLE) {
            if (params.has_upper != 0u) {
                // The arc this direction covers, at each of the four upper
                // probes, then averaged over the four probes. Pairwise again.
                let offset = k * 4u;
                let a = upper_arc(base00 + offset);
                let b = upper_arc(base10 + offset);
                let c = upper_arc(base01 + offset);
                let d = upper_arc(base11 + offset);
                let first = (a + b) / 2.0;
                let second = (c + d) / 2.0;
                carried = (first + second) / 2.0;
            } else {
                carried = vec3<f32>(params.far_r, params.far_g, params.far_b);
            }
            escaped = escaped + 1.0;
        } else if (hit.verdict == BLOCKED) {
            // The emission is read at the seed, not at the stopping point, for
            // the reason `cascade.wgsl` derives: a march stops up to a whole
            // texel short of what stopped it.
            let advance_x = (ray.dir_x * hit.distance) / FIXED;
            let advance_y = (ray.dir_y * hit.distance) / FIXED;
            let px = ray.from_x + advance_x;
            let py = ray.from_y + advance_y;
            let tx = clamp(px >> FIXED_SHIFT, 0, last_x);
            let ty = clamp(py >> FIXED_SHIFT, 0, last_y);
            let seed = textureLoad(field, vec2<i32>(tx, ty), 0);
            if (seed.z != 0.0) {
                let sx = clamp(i32(seed.x), 0, last_x);
                let sy = clamp(i32(seed.y), 0, last_y);
                let emitted = textureLoad(emission, vec2<i32>(sx, sy), 0);
                carried = emitted.xyz;
            }
        }
        // EXHAUSTED falls through carrying nothing, and is counted as neither
        // answered nor owed - `cascade.wgsl`'s reading of the same verdict.

        outgoing[base + k] = vec4<f32>(carried, 0.0);
        sum_r = sum_r + carried.x;
        sum_g = sum_g + carried.y;
        sum_b = sum_b + carried.z;
        k = k + 1u;
    }

    // The mean, for a consumer that wants one number per probe - and for the
    // comparison against the aggregate form, which is the whole of M8.5b's §2.
    let scale = f32(params.directions);
    let mean_r = sum_r / scale;
    let mean_g = sum_g / scale;
    let mean_b = sum_b / scale;
    let mean_v = escaped / scale;
    textureStore(radiance, vec2<i32>(out_x, out_y), vec4<f32>(mean_r, mean_g, mean_b, mean_v));
}
