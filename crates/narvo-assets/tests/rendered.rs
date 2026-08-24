//! The consumer proof: a packed atlas drawn, and its colours read back.
//!
//! # What this is for, and what it deliberately is not
//!
//! Every other test of the contract compares the packer against itself or
//! against a guard in the same repository. This one takes the chain the contract
//! actually exists to serve — **region table → texture coordinates → screen** —
//! and walks it through the real renderer on a real adapter. If the table said
//! one rectangle and the sampler read another, nothing else in this crate would
//! notice.
//!
//! **It is not a golden image and it asks for no blessing.** There is no
//! reference file, nothing is written to `target/golden`, and no existing
//! scenario changes. What it asserts is a handful of colours that were put into
//! the atlas by name a few lines above, sampled at the centres of large areas
//! where the result is unambiguous under the sampler in use. Edges are exactly
//! where a bilinear filter is entitled to blend, so no probe goes near one.
//!
//! # Why it lives here
//!
//! The claim is the *contract's* — that what a consumer is handed can be drawn —
//! so the crate that makes the claim carries its proof. `narvo-render2d` is a
//! dev-dependency for this and for the padding guard; it is never a normal one,
//! and `Cargo.toml` records why.

use narvo_assets::{Atlas, Placement, SourceRegion, pack};
use narvo_render2d::{
    OffscreenTarget, Pixels, RenderError, SpriteInstance, SpritePlacement, TextureRegion,
};

/// Set this to make a missing adapter a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// What a skipped run prints, so a CI log can be grepped for it.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The target is square and generous, so each sprite covers many pixels and a
/// centre probe is far from every edge.
const TARGET: u32 = 128;

/// Three regions of three sizes and three unmistakable colours.
///
/// Fully saturated primaries: after the sRGB round trip through the render
/// target they come back as themselves, and any two of them differ in every
/// channel — so a probe that read the wrong region cannot accidentally match.
fn regions() -> Vec<SourceRegion> {
    vec![
        SourceRegion::solid("red", 16, 8, [255, 0, 0, 255]).expect("valid"),
        SourceRegion::solid("green", 8, 16, [0, 255, 0, 255]).expect("valid"),
        SourceRegion::solid("blue", 12, 12, [0, 0, 255, 255]).expect("valid"),
    ]
}

/// Builds a target, or reports that this machine cannot host one.
///
/// The idiom the renderer's own tests use, copied rather than shared because
/// sharing it would mean a dependency on `narvo-testkit` for four lines.
fn target_or_skip() -> Option<OffscreenTarget> {
    match OffscreenTarget::new(TARGET, TARGET) {
        Ok(target) => {
            println!("adapter in use: {}", target.adapter_summary());
            Some(target)
        }
        Err(error @ RenderError::NoAdapter { .. }) => {
            assert!(
                std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a failure rather \
                 than a skip: {error}"
            );
            println!("{SKIP_MARKER}: {error}");
            None
        }
        Err(other) => panic!("the offscreen target failed for an unrelated reason: {other}"),
    }
}

/// The atlas as a texture.
fn texture(atlas: &Atlas) -> Pixels {
    Pixels::from_rgba8(atlas.width(), atlas.height(), atlas.rgba().to_vec())
        .expect("the packer produces a well-formed image")
}

/// A sprite drawing `place` of `atlas` at `(x, y)`, `size` world units across.
///
/// World units are pixels here: the projection for a `TARGET`-wide target has a
/// half-extent of `TARGET / 2`, so world x maps to pixel `x + TARGET / 2` and a
/// sprite of `size` covers `size` pixels.
///
/// **This is the step under test.** Everything the sprite knows about where its
/// pixels are comes from the region table: the four numbers go in as texels and
/// `TextureRegion::from_texels` turns them into the normalised coordinates the
/// shader samples with. A table that pointed one texel off would show up as a
/// colour from the neighbouring region, or from the background.
fn sprite_of(atlas: &Pixels, place: Placement, x: f32, y: f32, size: f32) -> SpriteInstance {
    let region = TextureRegion::from_texels(
        place.left(),
        place.top(),
        place.width(),
        place.height(),
        atlas,
    );

    SpriteInstance::new(
        SpritePlacement {
            x,
            y,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: size,
            scale_y: size,
        },
        region,
    )
}

/// The colour at the middle of the target, and at the middle of each half.
fn probe(image: &Pixels, x: u32, y: u32) -> [u8; 4] {
    image.pixel(x, y).expect("the probe is inside the image")
}

/// Two channels agreeing within a small tolerance.
///
/// Not an exact comparison: the sprite goes through a projection, a rasteriser
/// and an sRGB target, and the last of those is entitled to move a fully
/// saturated channel by a count. The tolerance is far below the distance
/// between any two of the three colours, so it cannot let a wrong region pass.
fn assert_colour(found: [u8; 4], expected: [u8; 4], what: &str) {
    for channel in 0..4 {
        let difference = i32::from(found[channel]) - i32::from(expected[channel]);
        assert!(
            difference.abs() <= 2,
            "{what}: expected {expected:?}, found {found:?}"
        );
    }
}

/// **The chain, end to end.** Three regions, three sprites, three probes.
#[test]
fn a_packed_atlas_draws_the_colours_its_table_points_at() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let atlas = pack(regions()).expect("three small regions fit");
    let texture = texture(&atlas);

    // Three sprites side by side, each large enough that its centre is many
    // pixels from its own edge.
    let sprites = [
        sprite_of(
            &texture,
            atlas.region("red").expect("packed"),
            -40.0,
            0.0,
            32.0,
        ),
        sprite_of(
            &texture,
            atlas.region("green").expect("packed"),
            0.0,
            0.0,
            32.0,
        ),
        sprite_of(
            &texture,
            atlas.region("blue").expect("packed"),
            40.0,
            0.0,
            32.0,
        ),
    ];

    let image = target
        .render_sprites(&texture, &sprites)
        .expect("three sprites is well inside every limit");

    // The centres of the three sprites, in pixels: world x + 64. Each sprite is
    // 32 pixels across, so a probe at its centre is sixteen pixels from its own
    // edge and eight from the nearest gap - nowhere a filter can reach across.
    assert_colour(probe(&image, 24, 64), [255, 0, 0, 255], "the red region");
    assert_colour(probe(&image, 64, 64), [0, 255, 0, 255], "the green region");
    assert_colour(probe(&image, 104, 64), [0, 0, 255, 255], "the blue region");
}

/// **The probe's own flank: a shifted table row is seen.**
///
/// Without this, the test above could be passing because the renderer draws
/// *something* rather than because it draws the right thing. Here the green
/// sprite is given the red region's rectangle instead of its own — the one
/// mistake a wrong table would make — and the probe at green's centre has to
/// come back red.
#[test]
fn a_sprite_pointed_at_the_wrong_table_row_shows_the_wrong_colour() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let atlas = pack(regions()).expect("fits");
    let texture = texture(&atlas);
    let red = atlas.region("red").expect("packed");

    // Same position as the green sprite in the test above, pointed at red.
    let sprites = [sprite_of(&texture, red, 0.0, 0.0, 32.0)];
    let image = target
        .render_sprites(&texture, &sprites)
        .expect("one sprite renders");

    assert_colour(
        probe(&image, 64, 64),
        [255, 0, 0, 255],
        "a sprite given red's rectangle has to show red, or the probe is not reading \
         the region the table pointed at",
    );
}

/// The padding around a region is its own edge colour, seen through the sampler.
///
/// The rendered counterpart of `check_region_padding`: that function reads the
/// texture directly, this one draws a region magnified so its border texels
/// participate, and asserts the result is still the region's colour rather than
/// a neighbour's. It is why padding exists at all (D13).
#[test]
fn a_magnified_region_stays_its_own_colour_to_its_edges() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let atlas = pack(regions()).expect("fits");
    let texture = texture(&atlas);
    let blue = atlas.region("blue").expect("packed");

    // One sprite larger than the target, at Linear so the border texels are what
    // the blend reaches for at the rim.
    let sprite =
        sprite_of(&texture, blue, 0.0, 0.0, 200.0).sampled(narvo_render2d::SpriteFilter::Linear);
    let image = target
        .render_sprites(&texture, &[sprite])
        .expect("one sprite renders");

    // The sprite is 200 pixels across on a 128-pixel target, so every one of
    // these is well inside it.
    for (x, y) in [(16, 16), (112, 16), (16, 112), (112, 112), (64, 64)] {
        assert_colour(
            probe(&image, x, y),
            [0, 0, 255, 255],
            &format!("the magnified blue region at ({x}, {y})"),
        );
    }
}
