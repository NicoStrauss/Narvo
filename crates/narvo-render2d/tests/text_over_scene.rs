//! The consumer: glyph lines over a coloured ground, which is what blending is
//! for.
//!
//! `text_lines_ascii_192x80` draws the same two lines over the cleared canvas
//! and could not have been drawn over anything: with `blend: None` each glyph's
//! quad replaced its whole rectangle, so a line over a scene would have carried
//! a transparent box around every character. This scene puts an opaque red
//! ground under exactly that text, and the box is what must not be there.
//!
//! # Conditions, and why they are the text scene's conditions
//!
//! `Nearest`, 1:1, whole-pixel positions, the M3.34 glyph atlas, the M3.35
//! layout. Everything `text_lines.rs`'s module documentation argues for applies
//! here unchanged and is not repeated; what this file adds is one sprite
//! underneath, from `narvo_testkit::blend`'s red ground, drawn first.
//!
//! # What is predicted, and how much of it needs a model
//!
//! For a glyph texel of coverage `c` over the opaque red ground `[255, 0, 0]`:
//!
//! - **green and blue are exactly `c`.** The ground contributes nothing to them,
//!   so the result is the source term alone and the sRGB round trip returns the
//!   byte it started from.
//! - **alpha is exactly 255.** `c + 255 * (1 - c/255) = 255`, in every reading.
//! - **red needs the transfer function**, because the ground has 255 there. It
//!   is the one channel of this scene that does.
//!
//! So two of four channels and the whole alpha plane are predicted with no model
//! at all, and the probes below say which is which rather than lumping them
//! together.

use narvo_render2d::{
    Golden, OffscreenTarget, Pixels, RenderError, SpriteFilter, SpriteInstance, SpritePlacement,
    golden_artifact_dir,
};
use narvo_testkit::blend::{self, Ground};
use narvo_testkit::glyph_atlas::{self, GlyphAtlas};
use narvo_testkit::text::{self, PlacedGlyph};
use std::path::{Path, PathBuf};

/// Printed instead of failing when this machine has no GPU adapter at all.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set in CI, where a missing adapter is a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// The scene's name, and the stem of its reference file.
const SCENE: &str = "text_over_scene_192x80";

/// The canvas, in pixels. The text scene's, so the layout is the same layout.
const WIDTH: u32 = 192;
const HEIGHT: u32 = 80;

/// The two lines and where their baselines sit.
///
/// Deliberately **not** the text scene's strings. That scene's `gjpq!` earns its
/// place by testing descenders and accumulated advance; this one is about what
/// is *under* the glyphs, and says so.
const LINE_16: &str = "over a scene 16";
const LINE_32: &str = "over 32";
const BASELINE_16: f32 = 24.0;
const BASELINE_32: f32 = 64.0;
const MARGIN: f32 = 8.0;

/// Where blessed reference images live. Read-only for anything automated.
fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Where a failing comparison writes what it rendered and how it differs.
fn output_dir() -> PathBuf {
    golden_artifact_dir()
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

/// The two glyph atlases stacked, and the row the second starts at.
///
/// Transcribed from `text_lines.rs` rather than shared with it: one draw call
/// binds one texture, so a scene showing two sizes has to stack them, and M3.34
/// anchors one atlas per size. Both files stack the same way and neither reads
/// the other's copy.
fn stacked_atlases(small: &GlyphAtlas, large: &GlyphAtlas) -> (Pixels, u32) {
    let (width, small_h) = (small.pixels().width(), small.pixels().height());
    assert_eq!(
        width,
        large.pixels().width(),
        "the two atlases must share a width to stack without re-packing"
    );

    let height = small_h + large.pixels().height();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    rgba.extend_from_slice(small.pixels().rgba());
    rgba.extend_from_slice(large.pixels().rgba());

    (
        Pixels::from_rgba8(width, height, rgba).expect("the stacked buffer matches its dimensions"),
        small_h,
    )
}

/// The glyph texture with the blend fixture's ground appended, and the row the
/// 32 px atlas starts at.
///
/// The appending itself is `narvo_testkit::blend::ground_beside`, so that the
/// two margin files can call it rather than transcribe it a third time — a
/// scoped exception documented at that function.
fn combined(
    small: &GlyphAtlas,
    large: &GlyphAtlas,
) -> (Pixels, u32, narvo_render2d::TextureRegion) {
    let (glyphs, offset) = stacked_atlases(small, large);
    let (combined, ground) = blend::ground_beside(&glyphs);
    (combined, offset, ground)
}

/// Everything the scene needs: the texture, the placed glyphs, the sprites.
///
/// **The ground is drawn first.** Order is composition (ADR-0023): the same list
/// in the other order would put the ground over the text and the picture would
/// be a red rectangle.
fn scene() -> (Pixels, Vec<PlacedGlyph>, Vec<SpriteInstance>) {
    let small = glyph_atlas::rasterize(16.0);
    let large = glyph_atlas::rasterize(32.0);
    let (texture, offset, ground_region) = combined(&small, &large);

    let mut placed = text::layout_line(LINE_16, &small, MARGIN, BASELINE_16);
    for glyph in text::layout_line(LINE_32, &large, MARGIN, BASELINE_32) {
        let mut glyph = glyph;
        glyph.region.top += offset;
        placed.push(glyph);
    }

    #[expect(clippy::cast_precision_loss, reason = "192 and 80 are exact in f32")]
    let (canvas_w, canvas_h) = (WIDTH as f32, HEIGHT as f32);

    let mut sprites = vec![
        SpriteInstance::new(
            SpritePlacement {
                x: 0.0,
                y: 0.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: canvas_w,
                scale_y: canvas_h,
            },
            ground_region,
        )
        .sampled(SpriteFilter::Nearest),
    ];
    sprites.extend(text::sprites_for(&placed, &texture, WIDTH, HEIGHT));

    (texture, placed, sprites)
}

/// The three kinds of point this scene exists to probe.
///
/// One pixel inside a glyph's rectangle whose coverage is zero, one whose
/// coverage is strictly between, and one at full coverage — each with the
/// coverage that put it in its class. Found by reading the atlas rather than by
/// naming coordinates: hand-named coordinates would pin the probes to a font
/// version, and coverage pins them to the thing the probe is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Probe {
    x: u32,
    y: u32,
    coverage: u8,
}

/// The three probes, in the order the flank asserts them.
fn probe_points(
    placed: &[PlacedGlyph],
    texture: &Pixels,
) -> (Option<Probe>, Option<Probe>, Option<Probe>) {
    let (mut ground, mut edge, mut interior) = (None, None, None);

    for glyph in placed {
        for dy in 0..glyph.region.height {
            for dx in 0..glyph.region.width {
                let texel = texture
                    .pixel(glyph.region.left + dx, glyph.region.top + dy)
                    .expect("the region lies inside the texture");

                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "glyph extents are small and non-negative"
                )]
                let (x, y) = (glyph.left + dx as i32, glyph.top + dy as i32);
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "canvas dimensions are small and non-negative"
                )]
                let inside = x >= 0 && y >= 0 && x < WIDTH as i32 && y < HEIGHT as i32;
                if !inside {
                    continue;
                }

                #[expect(
                    clippy::cast_sign_loss,
                    reason = "guarded as non-negative directly above"
                )]
                let probe = Probe {
                    x: x as u32,
                    y: y as u32,
                    coverage: texel[3],
                };

                match texel[3] {
                    0 if ground.is_none() => ground = Some(probe),
                    255 if interior.is_none() => interior = Some(probe),
                    coverage if coverage > 0 && coverage < 255 && edge.is_none() => {
                        edge = Some(probe);
                    }
                    _ => {}
                }
            }
        }
    }

    (ground, edge, interior)
}

/// **The consumer flank.** Glyph edges over the ground, with no rectangle.
///
/// Three probes, each asserting a different thing, and the first is the one the
/// whole task is for: a pixel that lies *inside* a glyph's quad but carries no
/// coverage must read the ground exactly. Under the old pipeline it read
/// `[0, 0, 0, 0]` — a hole punched through the scene in the shape of a
/// rectangle.
#[test]
fn glyph_rectangles_leave_no_mark_on_the_ground() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, placed, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("the lines are far inside the batch limit");

    let (ground, edge, interior) = probe_points(&placed, &texture);

    let ground = ground.expect("some glyph rectangle contains a zero-coverage texel");
    let (gx, gy) = (ground.x, ground.y);
    println!(
        "  ground inside a rectangle at ({gx}, {gy}): {:?}",
        rendered.pixel(gx, gy).expect("inside the canvas")
    );
    assert_eq!(
        rendered.pixel(gx, gy).expect("inside the canvas"),
        Ground::RED,
        "({gx}, {gy}) lies inside a glyph's rectangle with no coverage and does \
         not read the ground — the rectangle is still there"
    );

    let edge = edge.expect("some glyph has a partially covered texel");
    let (ex, ey, coverage) = (edge.x, edge.y, edge.coverage);
    let measured = rendered.pixel(ex, ey).expect("inside the canvas");
    println!("  edge at ({ex}, {ey}), coverage {coverage}: {measured:?}");
    assert_eq!(
        [measured[1], measured[2], measured[3]],
        [coverage, coverage, 255],
        "({ex}, {ey}) has coverage {coverage}, so green and blue must be exactly \
         that and alpha exactly 255 — those three need no colour-space model"
    );
    assert!(
        measured[0] > coverage,
        "({ex}, {ey}) is over a red ground, so red must exceed the coverage the \
         glyph contributes; it reads {measured:?}"
    );

    let interior = interior.expect("some glyph has a fully covered texel");
    let (ix, iy) = (interior.x, interior.y);
    println!(
        "  glyph interior at ({ix}, {iy}): {:?}",
        rendered.pixel(ix, iy).expect("inside the canvas")
    );
    assert_eq!(
        rendered.pixel(ix, iy).expect("inside the canvas"),
        [255, 255, 255, 255],
        "({ix}, {iy}) is fully covered white ink and must have replaced the ground"
    );
}

/// The whole image against the CPU model — exactly where the model is exact,
/// and measured where it is not.
///
/// # The result this test is shaped by, rather than the one it was written for
///
/// It began as `text_lines.rs`'s instrument: model the image on the CPU, compare
/// all 15 360 pixels, no tolerance and no budget. Over the black clear that is
/// legitimate, because `dst * (1 - a)` vanishes and the arithmetic collapses to
/// the sRGB round trip, which is the identity. Over a **red** ground it is not:
/// the red channel goes through `E(D(src) + D(dst) * (1 - a))`, and
/// `docs/perf/BASELINE.md` already records that a CPU's sRGB conversion cannot
/// be assumed bit-identical to a GPU's.
///
/// Measured, first run, RX 9070 XT: **83 of 15 360 pixels differ, every one of
/// them by 1 count in red alone.** Green, blue and alpha are byte-exact
/// everywhere — which is what the module documentation predicts, since the
/// ground contributes nothing to them.
///
/// So the test asserts the two things separately: **exactness where the
/// prediction is model-free, and the repository's own noise floor where it is
/// not.** The floor is `Tolerance::default().channel`, taken from the golden
/// comparison rather than invented here, so this test cannot drift away from
/// what CI already accepts.
///
/// The limit `text_lines.rs` names applies here too: `scene()` produces one list
/// of placed glyphs and both sides consume it, so a layout error moves both
/// together.
#[test]
fn the_scene_is_what_the_model_predicts_exactly_where_the_model_is_exact() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, placed, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("the lines are far inside the batch limit");

    // The model starts from the ground rather than from the clear, so it is
    // built by compositing the glyphs over a red canvas by hand.
    let mut expected = vec![0_u8; (WIDTH * HEIGHT * 4) as usize];
    for texel in expected.chunks_exact_mut(4) {
        texel.copy_from_slice(&Ground::RED);
    }
    for glyph in &placed {
        for dy in 0..glyph.region.height {
            for dx in 0..glyph.region.width {
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "glyph extents are small and non-negative"
                )]
                let (x, y) = (glyph.left + dx as i32, glyph.top + dy as i32);
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "canvas dimensions are small and non-negative"
                )]
                let inside = x >= 0 && y >= 0 && x < WIDTH as i32 && y < HEIGHT as i32;
                if !inside {
                    continue;
                }

                let texel = texture
                    .pixel(glyph.region.left + dx, glyph.region.top + dy)
                    .expect("the region lies inside the texture");

                #[expect(
                    clippy::cast_sign_loss,
                    reason = "guarded as non-negative directly above"
                )]
                let offset = ((y as u32 * WIDTH + x as u32) * 4) as usize;
                let under: [u8; 4] = expected[offset..offset + 4]
                    .try_into()
                    .expect("four bytes make a texel");
                expected[offset..offset + 4].copy_from_slice(&text::over(texel, under));
            }
        }
    }

    let mut model_free_mismatches = Vec::new();
    let mut red_differing = 0_u64;
    let mut worst_red = 0_u8;
    let mut worst_red_at = None;

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let offset = ((y * WIDTH + x) * 4) as usize;
            let predicted: [u8; 4] = expected[offset..offset + 4]
                .try_into()
                .expect("four bytes make a texel");
            let measured = rendered.pixel(x, y).expect("inside the canvas");

            // Green, blue and alpha take nothing from the ground, so they are
            // the source term alone and must be exact.
            if measured[1..4] != predicted[1..4] {
                model_free_mismatches.push(format!(
                    "({x}, {y}) model {predicted:?} render {measured:?}"
                ));
            }

            let deviation = measured[0].abs_diff(predicted[0]);
            if deviation > 0 {
                red_differing += 1;
                if deviation > worst_red {
                    worst_red = deviation;
                    worst_red_at = Some((x, y, predicted, measured));
                }
            }
        }
    }

    assert!(
        model_free_mismatches.is_empty(),
        "{} pixels differ in green, blue or alpha, where the ground contributes \
         nothing and the prediction needs no colour-space model at all. First \
         few:\n  {}",
        model_free_mismatches.len(),
        model_free_mismatches
            .iter()
            .take(6)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    let floor = narvo_render2d::Tolerance::default().channel;
    println!(
        "red channel against the CPU model: {red_differing} of {} pixels differ, \
         worst {worst_red} counts, floor {floor}",
        WIDTH * HEIGHT
    );
    if let Some((x, y, model, render)) = worst_red_at {
        println!("  worst at ({x}, {y}): model {model:?} render {render:?}");
    }

    assert!(
        worst_red <= floor,
        "the red channel is off the CPU model by {worst_red} counts, past the \
         {floor}-count floor the golden comparison uses. That is no longer a \
         transfer-function rounding difference and wants explaining."
    );
}

/// Nothing in the image is transparent, because the ground is opaque.
#[test]
fn the_composited_scene_is_opaque_everywhere() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, _, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("the lines are far inside the batch limit");

    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let pixel = rendered.pixel(x, y).expect("inside the canvas");
            assert_eq!(
                pixel[3], 255,
                "({x}, {y}) is {pixel:?} over an opaque ground"
            );
        }
    }
}

/// Compares the render against the blessed reference.
///
/// **A first-blessing candidate**, like `blend_proof`'s.
#[test]
fn the_text_over_scene_matches_its_golden_reference() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, _, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("the lines are far inside the batch limit");

    let (references, output) = (reference_dir(), output_dir());
    let golden = Golden::new(&references, &output);

    match golden.verify(SCENE, &rendered) {
        Ok(report) => println!(
            "golden match for \"{SCENE}\": {}",
            report.measured_against(golden.tolerance)
        ),
        Err(error) => panic!("{error}"),
    }
}
