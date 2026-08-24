//! Two textures in one frame, which is the whole of M6.6c's seam.
//!
//! # Why this file exists
//!
//! A draw call binds one texture. M6.6c measured what that means for an
//! overlay: glyph sprites appended to the scene's own buffer draw *cutouts of
//! the scene atlas* at glyph positions, and a second `draw` call is no way out
//! because every pass carries `LoadOp::Clear` and would wipe the scene. What was
//! missing is a second batch inside the *same* pass — `encode_runs` already
//! binds per run, so a run pointing at another texture's bind group was
//! structurally provided for and simply had no caller.
//!
//! `render_sprites_over` is that caller, and this file is the coverage `lib.rs`
//! asks for in its own words:
//!
//! > "A toggle would be two render paths where the golden images pin one and
//! > nothing checks the other."
//!
//! # What is asserted, and why it needs no blessing
//!
//! **No new reference.** The scene is two flat, opaque, axis-aligned rectangles
//! of known colour, drawn `Nearest` at whole pixels, one from each texture — so
//! every pixel is *predicted arithmetic* rather than a picture somebody had to
//! look at. That is the same standing this crate gives
//! `the_scene_is_exactly_what_the_model_predicts` in `text_lines.rs`, which
//! computes its expectation and compares, beside the golden rather than through
//! it. Nothing here writes to `tests/golden/`.
//!
//! The two colours are chosen so that **binding the wrong texture cannot look
//! right**: the first batch samples a red quadrant of `quadrant_texture`, the
//! second a solid cyan image that shares no channel pattern with it. A second
//! batch drawn from the first texture reads red where cyan is asserted.

use narvo_render2d::{
    CameraView, OffscreenTarget, Pixels, RenderError, SpriteBatch, SpriteFilter, SpriteInstance,
    SpritePlacement, TextureRegion,
};

/// Printed instead of failing when this machine has no GPU adapter at all.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set in CI, where a missing adapter is a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// The canvas edge. Small: every assertion below names a pixel.
const EDGE: u32 = 64;

/// What the first batch draws, from `quadrant_texture`'s top-left quadrant.
const RED: [u8; 4] = narvo_testkit::QUADRANT_TOP_LEFT;

/// What the second batch draws, from its own solid image.
///
/// Cyan, and deliberately not one of the four quadrant colours: red, green,
/// blue and the brown of the fourth quadrant all differ from it in at least two
/// channels, so *whichever* part of the first texture a mis-bound second batch
/// sampled, it could not read as this.
const CYAN: [u8; 4] = [0, 255, 255, 255];

/// The background the pass clears to, which is what an undrawn pixel reads.
const CLEARED: [u8; 4] = [0, 0, 0, 255];

#[test]
fn a_second_batch_draws_from_its_own_texture() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let (scene_texture, scene) = scene_batch();
    let overlay_texture = solid(CYAN);
    let overlay = [quad(16.0, 0.0, 16.0)];

    let frame = target
        .render_sprites_over(
            &scene_texture,
            &scene,
            SpriteBatch {
                image: &overlay_texture,
                sprites: &overlay,
                camera: CameraView::IDENTITY,
            },
            CameraView::IDENTITY,
        )
        .expect("two small batches are far inside the batch limit");

    // The left rectangle comes from the first texture, the right one from the
    // second. If the second batch bound the first texture, the right sample
    // reads red — which is the failure M6.6c described and this seam exists to
    // make impossible.
    assert_eq!(
        pixel(&frame, 16, 32),
        RED,
        "the first batch did not draw its own texture"
    );
    assert_eq!(
        pixel(&frame, 48, 32),
        CYAN,
        "the second batch drew something other than its own texture"
    );
    // And the frame is a frame, not a wall of one sprite: a corner no rectangle
    // reaches still reads the clear.
    assert_eq!(pixel(&frame, 2, 2), CLEARED, "something covered the corner");
}

/// The second batch is drawn **after** the first, so it wins where they overlap.
///
/// Draw order is composition since ADR-0023, and this seam adds an ordering rule
/// rather than a blending one: both batches go into one pass, the second batch's
/// runs after the first's. Both rectangles here are fully opaque, so the
/// overlap is decided by order alone and nothing about alpha enters it.
#[test]
fn the_second_batch_draws_over_the_first() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let (scene_texture, _) = scene_batch();
    let overlay_texture = solid(CYAN);
    // Same place, same size: whatever is on top is the whole answer.
    let scene = [quad(0.0, 0.0, 24.0)];
    let overlay = [quad(0.0, 0.0, 24.0)];

    let frame = target
        .render_sprites_over(
            &scene_texture,
            &scene,
            SpriteBatch {
                image: &overlay_texture,
                sprites: &overlay,
                camera: CameraView::IDENTITY,
            },
            CameraView::IDENTITY,
        )
        .expect("two small batches are far inside the batch limit");

    assert_eq!(
        pixel(&frame, 32, 32),
        CYAN,
        "the first batch drew over the second, so the order is reversed"
    );
}

/// An empty second batch renders exactly what no second batch renders.
///
/// The GPU-side statement of the property `batch_plan`'s unit tests prove
/// without a device: an empty batch must produce **nothing**, not merely
/// nothing visible. Byte-for-byte against the frame `render_sprites_viewed_by`
/// produces, which is the call every existing caller makes.
#[test]
fn an_empty_second_batch_renders_the_same_bytes_as_no_second_batch() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let (scene_texture, scene) = scene_batch();
    let overlay_texture = solid(CYAN);

    let without = target
        .render_sprites_viewed_by(&scene_texture, &scene, CameraView::IDENTITY)
        .expect("inside the batch limit");
    let with_empty = target
        .render_sprites_over(
            &scene_texture,
            &scene,
            SpriteBatch {
                image: &overlay_texture,
                sprites: &[],
                camera: CameraView::IDENTITY,
            },
            CameraView::IDENTITY,
        )
        .expect("inside the batch limit");

    assert_eq!(
        with_empty.rgba(),
        without.rgba(),
        "an empty second batch changed the frame"
    );
}

/// The batch limit is on the **sum**, because both batches share one pass.
///
/// The wording is the existing [`RenderError::BatchTooLarge`]; what is new is
/// which number reaches it. Neither batch alone exceeds the limit here.
#[test]
fn the_batch_limit_counts_both_batches_together() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let (scene_texture, _) = scene_batch();
    let overlay_texture = solid(CYAN);

    let half = narvo_render2d::MAX_SPRITES_PER_BATCH / 2 + 1;
    let scene = vec![quad(0.0, 0.0, 4.0); half];
    let overlay = vec![quad(0.0, 0.0, 4.0); half];

    let error = target
        .render_sprites_over(
            &scene_texture,
            &scene,
            SpriteBatch {
                image: &overlay_texture,
                sprites: &overlay,
                camera: CameraView::IDENTITY,
            },
            CameraView::IDENTITY,
        )
        .expect_err("two half-limit batches exceed the limit together");

    match error {
        RenderError::BatchTooLarge { requested, limit } => {
            assert_eq!(requested, half * 2, "the sum is what is reported");
            assert_eq!(limit, narvo_render2d::MAX_SPRITES_PER_BATCH);
        }
        other => panic!("expected BatchTooLarge, got {other}"),
    }
}

/// The four-quadrant fixture and one sprite showing its red quadrant.
fn scene_batch() -> (Pixels, [SpriteInstance; 1]) {
    let texture = narvo_testkit::quadrant_texture(16);
    // The top-left quadrant is `QUADRANT_TOP_LEFT`, which is `RED`.
    let region = TextureRegion::from_texels(0, 0, 8, 8, &texture);
    let sprite = SpriteInstance::new(
        SpritePlacement {
            x: -16.0,
            y: 0.0,
            rot_cos: SpritePlacement::UNTURNED.0,
            rot_sin: SpritePlacement::UNTURNED.1,
            scale_x: 16.0,
            scale_y: 16.0,
        },
        region,
    )
    .sampled(SpriteFilter::Nearest);

    (texture, [sprite])
}

/// A `size`-square sprite centred on (`x`, `y`), showing its whole texture.
fn quad(x: f32, y: f32, size: f32) -> SpriteInstance {
    SpriteInstance::new(
        SpritePlacement {
            x,
            y,
            rot_cos: SpritePlacement::UNTURNED.0,
            rot_sin: SpritePlacement::UNTURNED.1,
            scale_x: size,
            scale_y: size,
        },
        TextureRegion::WHOLE_TEXTURE,
    )
    .sampled(SpriteFilter::Nearest)
}

/// An 8x8 image of one colour.
fn solid(colour: [u8; 4]) -> Pixels {
    let rgba = colour.iter().copied().cycle().take(8 * 8 * 4).collect();
    Pixels::from_rgba8(8, 8, rgba).expect("8x8 is a legal size")
}

/// The pixel at (`x`, `y`), which must be inside the canvas.
fn pixel(frame: &Pixels, x: u32, y: u32) -> [u8; 4] {
    frame.pixel(x, y).expect("inside the canvas")
}

/// An offscreen target, or `None` on a machine with no adapter at all.
fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
    match OffscreenTarget::new(width, height) {
        Ok(target) => {
            println!("adapter in use: {}", target.adapter_summary());
            Some(target)
        }
        Err(error @ RenderError::NoAdapter { .. }) => {
            assert!(
                std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a failure \
                 rather than a skip: {error}"
            );

            println!("{SKIP_MARKER}: {error}");
            None
        }
        Err(other) => panic!(
            "creating the offscreen target failed for a reason other than a \
             missing adapter: {other}"
        ),
    }
}
