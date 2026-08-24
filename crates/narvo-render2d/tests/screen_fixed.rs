//! A layer that does not move when the camera does, measured in both
//! directions.
//!
//! # Why no single image can be the oracle here
//!
//! "The HUD stands still" is a statement about **two** camera positions. One
//! rendering cannot carry it: whatever the picture shows, it shows it once, and
//! a HUD that rides the camera would have produced a perfectly plausible
//! picture too. So the load-bearing test below renders the same scene and the
//! same overlay twice, at two different scene cameras, and asserts a pair at the
//! *same* pixels:
//!
//! - the HUD rectangles are **byte-identical**, and
//! - the frames as a whole **differ**.
//!
//! Neither half alone says anything. The first is satisfied by a HUD that is
//! never drawn at all; the second by a HUD that moves with the world. Together
//! they say the overlay held still while the world went past it, which is the
//! capability.
//!
//! # And the mirror, because even the pair can be passed by a fake
//! implementation
//!
//! Both halves above still pass against a renderer that **hard-codes** the
//! identity for the second batch and never reads
//! [`SpriteBatch::camera`](narvo_render2d::SpriteBatch::camera). That
//! implementation would be screen-fixed and nothing else — no world-fixed
//! overlay, no zoomed HUD, no second view of any kind. So the symmetric test
//! holds the scene camera still and moves the **overlay's** camera, and asserts
//! the mirror pair: the scene rectangles are byte-identical and the frames
//! differ.
//!
//! The two tests together pin the field to the batch it belongs to. A third
//! hands the overlay the *scene's* camera and asserts the HUD moves with the
//! world — which is both the pre-M6b.4 behaviour reproduced on purpose and the
//! escape hatch for a world-fixed overlay, the case a single camera per batch
//! would otherwise have no answer for.
//!
//! # The fixture, and why nothing in it overlaps
//!
//! One 20 x 20 padded atlas (D13's shape) serves both batches: the world draws
//! from the red and green cells, the HUD from the blue and yellow ones. Two
//! batches from one texture is deliberate — this file is about the coordinate
//! space, and `two_textures.rs` already owns the question of which texture a
//! batch binds. What the shared atlas buys is that a wrongly bound batch would
//! still sample *something*, so a colour assertion here is about geometry rather
//! than about binding.
//!
//! **The four rectangles are laid out so that none of them overlaps any other,
//! at either camera position.** That is what makes "the HUD rectangles are
//! byte-identical" a clean statement: no world pixel can wander into a HUD
//! rectangle and be mistaken for the HUD holding still. Composition where the
//! two do overlap is ADR-0023's and `two_textures.rs`'s subject, not this
//! file's.
//!
//! Whole-pixel positions, integer scales that are exact multiples of the 8-texel
//! region edge, and `Nearest`: every source texel maps to a whole block of
//! target pixels, so the frame is exactly predictable and carries no
//! rasteriser-dependent edge.

use std::path::{Path, PathBuf};

use narvo_render2d::{
    CameraView, Golden, OffscreenTarget, Pixels, RenderError, SpriteBatch, SpriteInstance,
    SpritePlacement, TextureRegion, golden_artifact_dir,
};
use narvo_testkit::AtlasLayout;

/// Printed instead of failing when this machine has no GPU adapter at all.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set in CI, where a missing adapter is a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// The name this scene's candidate reference is blessed under.
const SCENE: &str = "hud_over_moving_scene_128x128";

/// The canvas edge, in pixels.
const EDGE: u32 = 128;

/// The scene camera the blessed reference is rendered through.
///
/// **Deliberately not the identity.** A screen-fixed layer is indistinguishable
/// from a world-fixed one under the identity camera, so a reference blessed at
/// the identity would be a picture of nothing in particular. At this camera the
/// two spaces are 24 px apart horizontally and 16 px vertically, and the
/// separation is readable off the image: the world sprite authored at the origin
/// does not sit at the centre of the frame, and the HUD authored at the origin
/// would.
const SCENE_CAMERA: CameraView = CameraView::new(24.0, -16.0, 1.0);

/// A second scene camera, for the pair of renders no single image can express.
///
/// Chosen so both world sprites stay wholly inside the frame at both positions —
/// a sprite that leaves the frame would make "the frames differ" true for a
/// reason that is not the one under test.
const OTHER_SCENE_CAMERA: CameraView = CameraView::new(8.0, -8.0, 1.0);

/// A second overlay camera, for the mirror test.
///
/// A horizontal shift only, and small enough that both HUD elements stay wholly
/// inside the frame after it.
const OTHER_OVERLAY_CAMERA: CameraView = CameraView::new(-8.0, 0.0, 1.0);

/// The colour the pass clears to, which is what an undrawn pixel reads.
const CLEARED: [u8; 4] = [0, 0, 0, 255];

/// A rectangle of the frame, in pixels: `(left, top, width, height)`.
///
/// Half-open on the right and the bottom, as every loop below reads it.
type Rect = (u32, u32, u32, u32);

/// Where the HUD bar lands, at **every** camera position. See the module
/// documentation for the derivation; it is repeated in the assertions as a
/// prediction rather than trusted from here.
const BAR_RECT: Rect = (16, 104, 96, 16);

/// Where the HUD badge lands, at every camera position.
const BADGE_RECT: Rect = (8, 8, 16, 16);

/// Where the red world sprite lands at [`SCENE_CAMERA`].
const WORLD_RED_RECT: Rect = (24, 32, 32, 32);

/// Where the green world sprite lands at [`SCENE_CAMERA`].
const WORLD_GREEN_RECT: Rect = (72, 32, 32, 32);

/// Where blessed reference images live. Read-only for anything automated.
fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Builds a target, or reports that this machine cannot host one.
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

/// The shared atlas: D13's padded shape, four cells, eight colours.
fn atlas() -> Pixels {
    AtlasLayout::PADDED.atlas()
}

/// A sprite at whole-pixel `(x, y)` with scale `(w, h)`, sampling one atlas cell.
fn sprite(x: f32, y: f32, w: f32, h: f32, cell: (u32, u32), texture: &Pixels) -> SpriteInstance {
    SpriteInstance::new(
        SpritePlacement {
            x,
            y,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: w,
            scale_y: h,
        },
        AtlasLayout::PADDED.region(cell.0, cell.1, texture),
    )
}

/// The world batch: two 32 x 32 sprites, 48 world units apart.
///
/// The first is authored at the **origin**, which is what makes the camera
/// legible in the reference: at [`SCENE_CAMERA`] it does not sit at the centre
/// of the frame, and a HUD element authored at the origin does.
fn world_sprites(texture: &Pixels) -> Vec<SpriteInstance> {
    vec![
        sprite(0.0, 0.0, 32.0, 32.0, (0, 0), texture),
        sprite(48.0, 0.0, 32.0, 32.0, (1, 0), texture),
    ]
}

/// The HUD batch: a bar low in the frame and a badge in the upper left.
///
/// Both scales are exact multiples of the 8-texel region edge — 96 = 12 x 8,
/// 16 = 2 x 8 — so `Nearest` maps every source texel onto a whole block of
/// target pixels and no edge is decided by rounding.
fn hud_sprites(texture: &Pixels) -> Vec<SpriteInstance> {
    vec![
        sprite(0.0, -48.0, 96.0, 16.0, (0, 1), texture),
        sprite(-48.0, 48.0, 16.0, 16.0, (1, 1), texture),
    ]
}

/// Renders the fixture at a named pair of cameras.
fn render(
    target: &OffscreenTarget,
    texture: &Pixels,
    scene_camera: CameraView,
    overlay_camera: CameraView,
) -> Pixels {
    let world = world_sprites(texture);
    let hud = hud_sprites(texture);

    target
        .render_sprites_over(
            texture,
            &world,
            SpriteBatch {
                image: texture,
                sprites: &hud,
                camera: overlay_camera,
            },
            scene_camera,
        )
        .expect("four sprites are far inside the batch limit")
}

/// Every pixel of `rect`, in row-major order.
fn region_bytes(frame: &Pixels, rect: Rect) -> Vec<[u8; 4]> {
    let (left, top, width, height) = rect;
    (top..top + height)
        .flat_map(|y| (left..left + width).map(move |x| (x, y)))
        .map(|(x, y)| {
            frame
                .pixel(x, y)
                .unwrap_or_else(|| panic!("({x}, {y}) is outside a {EDGE} x {EDGE} frame"))
        })
        .collect()
}

/// Asserts `rect` holds something drawn, and that it came from the cell it
/// should have.
///
/// **This is what stops the byte-equality half of every test below from being
/// vacuous.** Two empty rectangles are byte-identical, and so are two rectangles
/// full of the clear colour; without this, "the HUD did not move" would be
/// satisfied by a HUD that was never drawn. The channel pattern is asserted
/// rather than the exact colour, because each cell carries a bright and a dark
/// half and this says only which cell was sampled.
fn assert_drawn_from(frame: &Pixels, rect: Rect, what: &str, channels: fn([u8; 4]) -> bool) {
    let pixels = region_bytes(frame, rect);
    assert!(
        pixels.iter().all(|pixel| *pixel != CLEARED),
        "{what} at {rect:?} contains cleared pixels, so it was not fully drawn"
    );
    assert!(
        pixels.iter().copied().all(channels),
        "{what} at {rect:?} was drawn from the wrong atlas cell"
    );
}

/// Blue cell: blue only.
fn is_blue(pixel: [u8; 4]) -> bool {
    pixel[0] == 0 && pixel[1] == 0 && pixel[2] > 0
}

/// Yellow cell: red and green, no blue.
fn is_yellow(pixel: [u8; 4]) -> bool {
    pixel[0] > 0 && pixel[1] > 0 && pixel[2] == 0
}

/// Red cell: red only.
fn is_red(pixel: [u8; 4]) -> bool {
    pixel[0] > 0 && pixel[1] == 0 && pixel[2] == 0
}

/// Green cell: green only.
fn is_green(pixel: [u8; 4]) -> bool {
    pixel[0] == 0 && pixel[1] > 0 && pixel[2] == 0
}

/// **The load-bearing test: the HUD stands still while the world goes past it.**
///
/// Two renders of one fixture, differing only in the scene camera, with the
/// overlay screen-fixed in both. Both halves of the pair are asserted at the
/// same pixels, and the rectangles are checked to hold drawn content first — see
/// the module documentation for why each half alone proves nothing.
#[test]
fn the_hud_stands_still_while_the_scene_camera_moves() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = atlas();
    let here = render(&target, &texture, SCENE_CAMERA, CameraView::IDENTITY);
    let there = render(&target, &texture, OTHER_SCENE_CAMERA, CameraView::IDENTITY);

    // Non-vacuity, before either comparison: both HUD elements are drawn, from
    // their own cells, in both frames.
    for (frame, which) in [(&here, "here"), (&there, "there")] {
        assert_drawn_from(frame, BAR_RECT, &format!("the bar ({which})"), is_blue);
        assert_drawn_from(
            frame,
            BADGE_RECT,
            &format!("the badge ({which})"),
            is_yellow,
        );
    }

    // Half one: the HUD did not move, byte for byte.
    assert_eq!(
        region_bytes(&here, BAR_RECT),
        region_bytes(&there, BAR_RECT),
        "the HUD bar moved when the scene camera did"
    );
    assert_eq!(
        region_bytes(&here, BADGE_RECT),
        region_bytes(&there, BADGE_RECT),
        "the HUD badge moved when the scene camera did"
    );

    // Half two: the world did. Without this the test is passed by a renderer
    // that ignores the scene camera as well.
    assert_ne!(
        here.rgba(),
        there.rgba(),
        "the two scene cameras produced the same frame, so nothing moved at all"
    );
    assert_drawn_from(&here, WORLD_RED_RECT, "the red world sprite", is_red);
    assert_ne!(
        region_bytes(&here, WORLD_RED_RECT),
        region_bytes(&there, WORLD_RED_RECT),
        "the world sprite did not move when its camera did"
    );
}

/// **The mirror: the overlay's own camera is what moves the overlay.**
///
/// The scene camera is held; the overlay's is moved. This is the half the first
/// test cannot give — an implementation that hard-codes the identity for the
/// second batch passes that one and fails this one, which is exactly the
/// difference between "there is a screen-fixed layer" and "the batch has a
/// camera".
#[test]
fn the_overlays_own_camera_is_what_moves_it() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = atlas();
    let fixed = render(&target, &texture, SCENE_CAMERA, CameraView::IDENTITY);
    let shifted = render(&target, &texture, SCENE_CAMERA, OTHER_OVERLAY_CAMERA);

    // The world is drawn in both, at the same place, from the same cells.
    for (frame, which) in [(&fixed, "fixed"), (&shifted, "shifted")] {
        assert_drawn_from(
            frame,
            WORLD_RED_RECT,
            &format!("the red sprite ({which})"),
            is_red,
        );
        assert_drawn_from(
            frame,
            WORLD_GREEN_RECT,
            &format!("the green sprite ({which})"),
            is_green,
        );
    }

    assert_eq!(
        region_bytes(&fixed, WORLD_RED_RECT),
        region_bytes(&shifted, WORLD_RED_RECT),
        "the world moved when only the overlay's camera changed"
    );
    assert_eq!(
        region_bytes(&fixed, WORLD_GREEN_RECT),
        region_bytes(&shifted, WORLD_GREEN_RECT),
        "the world moved when only the overlay's camera changed"
    );

    assert_ne!(
        fixed.rgba(),
        shifted.rgba(),
        "moving the overlay's camera changed nothing, so the field is not read"
    );
    assert_ne!(
        region_bytes(&fixed, BADGE_RECT),
        region_bytes(&shifted, BADGE_RECT),
        "the badge stayed put when its own camera moved"
    );
}

/// An overlay handed the **scene's** camera moves with the scene.
///
/// Two things at once, and both are worth having. It is the pre-M6b.4 behaviour
/// reproduced deliberately — every caller that passes the scene camera gets
/// exactly what it got before the field existed. And it is the answer for a
/// world-fixed overlay: damage numbers over enemies do not need a second overlay
/// batch, they need this camera.
#[test]
fn an_overlay_given_the_scenes_camera_moves_with_the_scene() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = atlas();
    let here = render(&target, &texture, SCENE_CAMERA, SCENE_CAMERA);
    let there = render(&target, &texture, OTHER_SCENE_CAMERA, OTHER_SCENE_CAMERA);

    assert_ne!(
        here.rgba(),
        there.rgba(),
        "a world-fixed overlay did not move with its scene"
    );

    // And it is *the overlay* that moved, not only the world: at the screen-fixed
    // position the bar is no longer there, because the overlay went with the
    // camera instead of staying.
    let screen_fixed = render(&target, &texture, SCENE_CAMERA, CameraView::IDENTITY);
    assert_drawn_from(&screen_fixed, BAR_RECT, "the screen-fixed bar", is_blue);
    assert_ne!(
        region_bytes(&screen_fixed, BAR_RECT),
        region_bytes(&here, BAR_RECT),
        "the bar landed in the same place through two different cameras"
    );
}

/// The blessed frame reads the ten values a human is asked to look for.
///
/// **The reference cannot say this and this cannot say what the reference
/// says.** A picture is what a human blesses; a picture is also what nobody can
/// read a number out of. The inspection package hands over a table of named
/// points and expected colours, and this is what makes that table a
/// *prediction* checked by machine rather than a transcript of whatever came
/// out.
///
/// The last assertion is the strongest and the cheapest: **every pixel of the
/// frame is one of the atlas's eight colours or the clear.** Nothing blends,
/// nothing interpolates, no edge is half-covered — which is why this reference
/// is byte-identical across rasterisers where M6b.3's is not.
#[test]
fn the_blessed_frame_reads_the_values_the_inspection_table_names() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = atlas();
    let frame = render(&target, &texture, SCENE_CAMERA, CameraView::IDENTITY);

    // Each cell is bright in its upper half and dark in its lower one, so a
    // pair of points per element also says the texture was sampled the right way
    // up (ADR-0004).
    let expected = [
        ((16, 12), [255, 255, 0, 255], "badge, upper half"),
        ((16, 20), [128, 128, 0, 255], "badge, lower half"),
        ((40, 40), [255, 0, 0, 255], "red world sprite, upper half"),
        ((40, 56), [128, 0, 0, 255], "red world sprite, lower half"),
        ((88, 40), [0, 255, 0, 255], "green world sprite, upper half"),
        ((88, 56), [0, 128, 0, 255], "green world sprite, lower half"),
        ((64, 108), [0, 0, 255, 255], "bar, upper half"),
        ((64, 116), [0, 0, 128, 255], "bar, lower half"),
        ((2, 2), CLEARED, "the cleared background"),
        // **The camera, read straight off the frame.** A world sprite is
        // authored at the origin; the centre of the frame is where the identity
        // camera would have put it. It is empty, and that emptiness is the
        // 24-by-16 offset between the two spaces made visible.
        (
            (64, 64),
            CLEARED,
            "the frame centre, which the camera moved off",
        ),
    ];

    for ((x, y), colour, what) in expected {
        assert_eq!(frame.pixel(x, y), Some(colour), "{what} at ({x}, {y})");
    }

    // Nine colours and no tenth: the eight the atlas carries, plus the clear.
    let palette: Vec<[u8; 4]> = (0..EDGE)
        .flat_map(|y| (0..EDGE).map(move |x| (x, y)))
        .map(|(x, y)| frame.pixel(x, y).expect("inside the frame"))
        .fold(Vec::new(), |mut seen, pixel| {
            if !seen.contains(&pixel) {
                seen.push(pixel);
            }
            seen
        });
    assert_eq!(
        palette.len(),
        9,
        "the frame holds {} distinct colours rather than the eight atlas \
         colours plus the clear, so something blended or interpolated: {palette:?}",
        palette.len()
    );
}

/// The composed frame, against what a human blessed.
///
/// A capability with no visual oracle is not verified, and the pair of tests
/// above are pixel comparisons between two renders of the same code — they would
/// agree with each other just as happily if the whole fixture drew in the wrong
/// place. This is the test that says the picture is the intended one.
#[test]
fn the_screen_fixed_scene_matches_its_golden_reference() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = atlas();
    let rendered = render(&target, &texture, SCENE_CAMERA, CameraView::IDENTITY);

    let references = reference_dir();
    let output = golden_artifact_dir();
    let golden = Golden::new(&references, &output);

    match golden.verify(SCENE, &rendered) {
        Ok(report) => println!(
            "golden match for {SCENE:?}: {}",
            report.measured_against(golden.tolerance)
        ),
        Err(error) => panic!("{error}"),
    }
}

/// The four rectangles this file names are where the projection puts them.
///
/// Runs with a GPU but asserts no image: it checks the *addresses* the other
/// tests compare at, computed here from `world_to_ndc` rather than from the
/// constants. A wrong constant would otherwise make every comparison above
/// compare the wrong pixels — quietly, and in agreement with itself.
#[test]
fn the_named_rectangles_are_where_the_projection_puts_them() {
    // Half-extents of a 128 x 128 target.
    let half = f64::from(EDGE) / 2.0;

    // The same mapping `Projection::world_to_ndc` applies, then NDC to pixels:
    // x right, y **up**, so the row is subtracted (ADR-0004).
    let rect_of = |x: f64, y: f64, w: f64, h: f64, camera: CameraView| -> Rect {
        let centre_x = half + (x - f64::from(camera.x)) * f64::from(camera.zoom);
        let centre_y = half - (y - f64::from(camera.y)) * f64::from(camera.zoom);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "every value here is a small non-negative whole number by construction"
        )]
        (
            (centre_x - w / 2.0) as u32,
            (centre_y - h / 2.0) as u32,
            w as u32,
            h as u32,
        )
    };

    assert_eq!(
        rect_of(0.0, -48.0, 96.0, 16.0, CameraView::IDENTITY),
        BAR_RECT
    );
    assert_eq!(
        rect_of(-48.0, 48.0, 16.0, 16.0, CameraView::IDENTITY),
        BADGE_RECT
    );
    assert_eq!(rect_of(0.0, 0.0, 32.0, 32.0, SCENE_CAMERA), WORLD_RED_RECT);
    assert_eq!(
        rect_of(48.0, 0.0, 32.0, 32.0, SCENE_CAMERA),
        WORLD_GREEN_RECT
    );

    // And they are pairwise disjoint, which is what the byte comparisons rest
    // on: no world pixel can land inside a HUD rectangle and be mistaken for the
    // HUD holding still. Checked at **both** scene cameras.
    let overlaps =
        |a: Rect, b: Rect| a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3;
    let world_at = |camera| {
        [
            rect_of(0.0, 0.0, 32.0, 32.0, camera),
            rect_of(48.0, 0.0, 32.0, 32.0, camera),
        ]
    };

    for camera in [SCENE_CAMERA, OTHER_SCENE_CAMERA] {
        for world in world_at(camera) {
            for hud in [BAR_RECT, BADGE_RECT] {
                assert!(
                    !overlaps(world, hud),
                    "{world:?} overlaps {hud:?} at camera {camera:?}, so a byte \
                     comparison of the HUD rectangle would include world pixels"
                );
            }
        }
    }
}

/// The regions this file samples are the padded atlas's, not coordinates copied
/// by hand.
///
/// Runs without a GPU. `AtlasLayout::PADDED` puts content at a one-texel offset
/// inside a ten-texel cell; the margin instruments transcribe those numbers as
/// literals, so this is the one place the literal and the layout are held
/// together.
#[test]
fn the_cells_this_file_samples_are_the_padded_layouts() {
    let texture = atlas();
    assert_eq!((texture.width(), texture.height()), (20, 20));

    for (cell_x, cell_y, left, top) in [(0, 0, 1, 1), (1, 0, 11, 1), (0, 1, 1, 11), (1, 1, 11, 11)]
    {
        assert_eq!(
            AtlasLayout::PADDED.region(cell_x, cell_y, &texture),
            TextureRegion::from_texels(left, top, 8, 8, &texture),
            "cell ({cell_x}, {cell_y}) is not at ({left}, {top})"
        );
    }
}
