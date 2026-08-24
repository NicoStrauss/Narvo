// The write-back: radiance becomes emission, so the next frame marches against
// what this frame lit.
//
// **M8.6's capability, and it is deliberately two entry points rather than one.**
// The recurrence a surface cache runs is
//
//     bounce_{n+1} = albedo * R(direct + bounce_n)
//
// and writing it as one kernel would put `direct + albedo * R` in a single
// expression - one inexact float product feeding a float add, which is exactly
// the shape ADR-0051 forbids and exactly the shape M8.5a measured splitting the
// eight adapter/backend pairs into two fields.
//
// So the multiply and the add are two *dispatches*:
//
//   - `reflect` computes `albedo * R` and **stores** it. A product that is stored
//     cannot be contracted with anything, because there is no add for it to fuse
//     into.
//   - `combine` computes `direct + bounce` and stores that. It contains no float
//     multiply at all.
//
// That is a structural guarantee rather than a stylistic one: a texel store
// between the two forces the intermediate to be rounded to `f32` and written to
// memory, so no compiler on any backend can see the product and the sum as one
// expression. The two are separated by a memory round trip, not by a line break.
//
// **The cost is one extra full-field pass per frame**, and it is paid here rather
// than by everybody: the alternative that keeps one dispatch is to give the
// cascade kernels a second emission binding, which would make every existing
// caller allocate a field of zeros it never reads. Neither `cascade.wgsl` nor
// `cascade_directional.wgsl` is touched by M8.6, and that is the reason.
//
// # Which probe a texel takes its radiance from
//
// The nearest one, found in integer arithmetic. ADR-0050 decided that a
// comparison over coordinates is written in `i32` because `f32` names the wrong
// seed once a squared distance passes 2^24 and does so *differently per
// rasteriser family*; an index over coordinates is the same kind of quantity and
// is written the same way. `SurfaceCache` refuses a cascade whose level-zero
// origin or spacing is not a whole number of texels, so the division below is
// exact by construction rather than by rounding.
//
// This is the surface cache's approximation and it is named rather than hidden:
// a wall texel takes the radiance of the probe nearest it, which is not the
// radiance arriving *at that texel*. It is what makes the closed-chamber fixed
// point exact - a uniform field reads the same however it is sampled - and it is
// what a finer probe spacing improves.
//
// # Why only occluder texels matter, without this kernel knowing which they are
//
// `cascade.wgsl` reads emission **at the seed** a direction stopped on, so a
// texel that is not an occluder is never read as an emitter. Writing a bounce
// into one is therefore inert rather than wrong, and this kernel needs no
// occluder test - which would cost a distance-field fetch per texel to skip work
// that already costs nothing. `Emission`'s own header records the same property
// for the map an author writes.

struct Bounce {
    // The field's extent, so an invocation past the edge returns.
    width: u32,
    height: u32,
    // Level zero's probe grid: where probe (0, 0) sits, how far apart they are,
    // and how many there are. Whole texels, checked by `SurfaceCache::new`.
    origin_x: i32,
    origin_y: i32,
    spacing: i32,
    probes_x: u32,
    probes_y: u32,
    pad: u32,
}

@group(0) @binding(0) var source_a: texture_2d<f32>;
@group(0) @binding(1) var source_b: texture_2d<f32>;
@group(0) @binding(2) var written: texture_storage_2d<rgba32float, write>;
@group(0) @binding(3) var<uniform> params: Bounce;

// The index of the probe nearest `d` texels from the grid origin, clamped into
// the grid. Integer throughout: `spacing` is a positive whole number, so
// `spacing / 2` is the tie-break and `(d + spacing / 2) / spacing` is the round.
// A texel before the origin takes probe zero rather than a negative index, which
// also keeps the division away from truncation-toward-zero.
fn nearest_probe(d: i32, spacing: i32, last: i32) -> i32 {
    if (d <= 0) {
        return 0;
    }
    let half = spacing / 2;
    let index = (d + half) / spacing;
    return clamp(index, 0, last);
}

// Eight by eight, two dimensions: a field is a field, exactly as `jump_flood.wgsl`
// walks one. `surface::BOUNCE_WORKGROUP` divides the dispatch by the same number
// and a test holds the two together.
//
// `source_a` is the albedo map, `source_b` is level zero's radiance, and `written`
// is the bounced field.
@compute @workgroup_size(8, 8)
fn reflect(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let texel = vec2<i32>(i32(id.x), i32(id.y));

    let px = nearest_probe(texel.x - params.origin_x, params.spacing, i32(params.probes_x) - 1);
    let py = nearest_probe(texel.y - params.origin_y, params.spacing, i32(params.probes_y) - 1);
    let radiance = textureLoad(source_b, vec2<i32>(px, py), 0);
    let albedo = textureLoad(source_a, texel, 0);

    // **The three float multiplies of this file, and none of them feeds an add.**
    // The channels are written out one at a time rather than as a vector product
    // so that the source guard can read them: a kernel that coupled red into
    // green would be a plausible picture and a wrong field, and the only place
    // that is visible is here. The fourth channel is not a colour and is stored
    // as zero rather than multiplied.
    let r = albedo.x * radiance.x;
    let g = albedo.y * radiance.y;
    let b = albedo.z * radiance.z;
    textureStore(written, texel, vec4<f32>(r, g, b, 0.0));
}

// `source_a` is the author's direct emission, `source_b` is the bounced field,
// and `written` is what the next cascade marches against. No multiplication of
// any kind, so there is nothing for an add to be fused with.
@compute @workgroup_size(8, 8)
fn combine(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }
    let texel = vec2<i32>(i32(id.x), i32(id.y));
    let direct = textureLoad(source_a, texel, 0);
    let bounced = textureLoad(source_b, texel, 0);
    textureStore(
        written,
        texel,
        vec4<f32>(
            direct.x + bounced.x,
            direct.y + bounced.y,
            direct.z + bounced.z,
            0.0,
        ),
    );
}
