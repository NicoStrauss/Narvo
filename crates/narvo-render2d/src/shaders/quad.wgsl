// Textured quad: samples one texture across a screen-filling quad and
// multiplies the sample by a per-sprite tint.
//
// Deliberately does nothing else. Transforms and batching are the caller's;
// this shader exists so the offscreen path has real geometry and a real texture
// fetch to verify, rather than a clear colour. The tint arrived in M6b.3 and is
// the first thing this shader does beyond passing a sample through.

struct VertexInput {
    // Normalised device coordinates. y points up here; the framebuffer's y
    // points down, and the viewport transform flips between them.
    @location(0) position: vec2<f32>,
    // Texture coordinates, origin at the top left, as textures are addressed.
    @location(1) uv: vec2<f32>,
    // The sprite's tint, already premultiplied by its own alpha
    // (`SpriteTint::premultiplied`). The same value sits on all four corners of
    // a quad; see the qualifier on the varying below for what that buys.
    @location(2) tint: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // `center`, written out rather than left to the default, because it is a
    // decision: D17 in `ProjektPlan.md` §11, decided during M3.24's blessing
    // window and carried out in M3.26. It replaced `centroid`, which M3.15 had
    // introduced.
    //
    // Under MSAA the fragment shader runs once per *fragment* - once per covered
    // pixel per primitive - and not once per sample; only
    // @interpolate(..., sample) makes it per sample, which naga documents as
    // "invoke the fragment shader once per sample"
    // (naga-30.0.0/src/ir/mod.rs:640-641). One invocation therefore reads the
    // varying at exactly one point, and this qualifier names which point.
    //
    // **That is the anchor argument, and D17's.** naga's `Center` is "the
    // center of the pixel" (:632) - one point, fixed by the specification. Its
    // `Centroid` is "a point that lies within all samples covered by the
    // fragment within the current primitive" (:635-637) - which names a *region*
    // and leaves the point inside it to the driver. A verification strategy that
    // rests on rasterisers agreeing cannot anchor on a place no specification
    // pins; D17 records that the three rasterisers which agreed under `centroid`
    // were not guaranteed to.
    //
    // The hazard `centroid` was introduced against is real, and content now
    // answers it. At the pixel centre a partly covered pixel reads its uv
    // *outside* the sprite, so the uv is extrapolated past the sprite's own
    // atlas region and the sampler fetches whatever lies beyond it. M3.15
    // measured that on an **unpadded** atlas: sprite B of the camera scene has
    // its left edge at pixel 52.75, so pixel 52 is a quarter covered, and drawn
    // alone against black at four samples it came back [137, 0, 0] - a quarter
    // of *red*, the texel column left of its region - where its own region is
    // green.
    //
    // **That measurement can no longer be reproduced from the file it names**,
    // and the property is the reason: `tests/camera_scene.rs` has been padded
    // since M3.21, and the texel beyond a padded region is a duplicate of the
    // region's own edge texel. Re-measured on padded content in M3.24a, on a
    // probe fixture of the same shape - one region placed alone against black
    // with its edge on a quarter pixel - that class of pixel reads [0, 225, 0]:
    // the coverage blend of its own green against black, zero red, and
    // byte-identical under `Nearest` and `Linear`. At a whole-texture rim, where
    // there is no border to duplicate, `AddressMode::ClampToEdge` does the same
    // job. `tests/camera_motion.rs` keeps these three sprites on an unpadded
    // atlas on purpose, and documents what happens there.
    //
    // So the protection moved rather than vanished. D17 states it as product
    // character: **sampling anchors at the specified pixel centre, and
    // protecting unpadded surfaces is the content pipeline's job** - which binds
    // D10's glyph atlases to padding every cell.
    //
    // What `centroid` cost, and why removing it changes a reference image: a
    // quad is two triangles sharing its top-left-to-bottom-right diagonal
    // (`INDICES` in `quad.rs`), so a pixel on that diagonal is fully covered by
    // the quad and only partly by each triangle. `centroid` handed the two
    // triangles two sample points, they could land in two texels, and the
    // resolve averaged two colours into a faint seam. Under `Nearest` the
    // displacement never crossed a texel edge and the seam was invisible; under
    // `Linear` it was, and it carried sprite_atlas_regions_128x128's worst
    // deviation. `center` removes it.
    @location(0) @interpolate(perspective, center) uv: vec2<f32>,
    // `flat`, and it is a decision rather than a shortcut.
    //
    // A tint is a property of a *sprite*, not of a corner: all four vertices of
    // a quad carry the identical value, so there is nothing for an interpolator
    // to interpolate. Asking for one anyway would make the value the fragment
    // shader reads the output of a weighted sum whose exactness for a constant
    // input is the hardware's business and not this project's — and V1 of this
    // task is that a tint of one moves *zero* pixels of ten blessed references.
    // Under `flat` no arithmetic runs at all: naga documents `Sampling::First`
    // as "use the value provided by the first vertex of the current primitive"
    // (naga-30.0.0/src/ir/mod.rs:644-645), so the fragment shader reads the
    // float the vertex buffer holds, bit for bit.
    //
    // `first` is written out for the reason `center` is above it (D17): naga's
    // other admissible sampling for `flat` is `Either`, "the exact choice is
    // implementation-dependent" (:647-648), and a verification strategy that
    // rests on rasterisers agreeing cannot anchor on a choice no specification
    // pins. That all four vertices carry the same value makes the two
    // indistinguishable here, which is precisely why the qualifier has to say
    // which one it means rather than leaving the next reader to work out that
    // it does not matter yet.
    @location(1) @interpolate(flat, first) tint: vec4<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    output.uv = input.uv;
    output.tint = input.tint;
    return output;
}

@group(0) @binding(0) var quad_texture: texture_2d<f32>;
@group(0) @binding(1) var quad_sampler: sampler;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // The texture is Rgba8UnormSrgb and so is the render target, so the sample
    // is decoded to linear here and re-encoded on write. A byte that goes in
    // comes back out unchanged *under an untinted draw*, which is what makes
    // the quadrant test an assertion about orientation rather than about colour
    // management.
    //
    // That round trip is also what decides the space this multiply happens in,
    // and it decides it rather than leaving a choice: what `textureSample`
    // returns is linear light, so the product below is a product of light.
    // `SpriteTint` carries the measurement and the citation.
    //
    // Both operands are premultiplied — the texel by ADR-0024's load-time
    // arithmetic, the tint by `SpriteTint::premultiplied` on the CPU — so this
    // one product is the whole of tinting, and its result is premultiplied too.
    // A tint of (1, 1, 1, 1) makes it a multiplication by 1.0 on every channel,
    // which IEEE-754 defines as the identity.
    let texel = textureSample(quad_texture, quad_sampler, input.uv);
    return texel * input.tint;
}
