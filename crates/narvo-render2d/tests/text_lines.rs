//! The first text scene, and the first golden image of drawn glyphs.
//!
//! Two lines of ASCII from the M3.34 glyph atlas — one at 16 px, one at 32 px —
//! laid out by `narvo_testkit::text` and drawn at 1:1 with `Nearest`.
//!
//! # Why `Nearest`, decided rather than defaulted
//!
//! D13 chose `Linear` with padding for sprite atlases, and M3.34 padded this
//! atlas to the border D17's arithmetic demands. This scene still draws
//! `Nearest`, for two reasons that both point the same way:
//!
//! - **UI text at 1:1 wants the texel, not a blend of it.** A glyph rasterised
//!   at 16 px and drawn at 16 px has already been antialiased by `ab_glyph`;
//!   filtering it again softens edges that were computed to be exactly that
//!   shape.
//! - **At 1:1 on whole pixels the two filters agree, measured rather than
//!   argued.** Rendering this scene with every sprite flipped to `Linear` gives
//!   an image differing from the `Nearest` one in **0 of 15 360 pixels**, and
//!   both match the model exactly. The mechanism is M3.34's border derivation:
//!   on-grid, every sample lands on a texel centre, so a bilinear fetch returns
//!   that texel's own value.
//!
//!   So the choice does not change *this* image at all, and saying otherwise
//!   would be the stronger claim the measurement refuses. What it changes is
//!   everything off the grid — a scaled, rotated or sub-pixel-positioned line —
//!   and `Nearest` is the honest declaration of what this scene actually does.
//!
//! So the D13 regime sentence does **not** apply to this image: no blend value
//! is predicted, because no blend happens. The atlas keeps its padding anyway,
//! since the padding belongs to the atlas rather than to this scene, and the
//! next scene that scales or rotates text will need it.
//!
//! # What that makes the prediction
//!
//! **A copy.** Under `Nearest`, at 1:1, on whole-pixel positions, each drawn
//! pixel is one atlas texel and nothing else — so the expected image is
//! computable on the CPU from the atlas and the layout alone, byte for byte,
//! and `the_scene_is_exactly_what_the_model_predicts` asserts precisely that
//! against the render before any reference exists — with the limit that test's
//! own documentation states.
//!
//! That is a different *form* of prediction from the ones before it, and the
//! difference is worth naming rather than ranking. M3.24 predicted a **delta**
//! — that 323 of 16 384 pixels would change — against a reference that already
//! existed, and hit that count; it did not predict which pixels, and its own
//! report records where the hand count and the measurement diverged. This one
//! has no reference to
//! predict a delta against, so it predicts the **whole image from first
//! principles**: 15 360 pixels computed from the atlas and the layout, with no
//! tolerance and no budget. Neither is stronger than the other in general; this
//! one is available only because a `Nearest` copy is exactly reproducible on a
//! CPU, and a scene with a blend would not offer it.

use narvo_render2d::{
    Golden, OffscreenTarget, Pixels, SpriteFilter, SpriteInstance, golden_artifact_dir,
};
use narvo_testkit::glyph_atlas::{self, GlyphAtlas};
use narvo_testkit::text::{self, PlacedGlyph};
use std::path::PathBuf;

/// The scene's name, and the stem of its reference file.
const SCENE: &str = "text_lines_ascii_192x80";

/// The canvas, in pixels.
const WIDTH: u32 = 192;
const HEIGHT: u32 = 80;

/// The two lines, their sizes, and where their baselines sit.
///
/// The 16 px line carries descenders on purpose: `gjpq` hang below the
/// baseline, so a layout that ignored `bearing_y` would put them level with the
/// rest and the picture would say so.
///
/// The `!` is here for a different reason and not that one — its `bearing_y` is
/// -11, the same as `A`, so it says nothing about vertical placement. It is the
/// last glyph of the line, which is what makes it the probe that catches an
/// accumulated advance error, and it is a two-part shape (a stem and a dot with
/// a gap between) which makes it easy to recognise in the magnifier.
///
/// # The word is "Amboss" and stays that way (U1a)
///
/// This is rendered *content*, not a name: `text_lines_ascii_192x80.png` is a
/// blessed reference and it holds these glyphs as pixels. U1a renamed the
/// engine to Narvo and changed this string with everything else; the lit-pixel
/// count moved from 1588 to 1819 and the reference stopped matching. A blessed
/// reference moving is a HALT and never an occasion to re-bless (ADR-0008), so
/// the string was put back. Changing it means re-blessing this reference and
/// the two margin files that rebuild the same scene, deliberately and as its
/// own task.
const LINE_16: &str = "Amboss 16 gjpq!";
const LINE_32: &str = "Amboss 32";
const BASELINE_16: f32 = 24.0;
const BASELINE_32: f32 = 64.0;
const MARGIN: f32 = 8.0;

/// Where blessed references live for this crate.
fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

/// Where a failing run leaves its artifacts.
fn output_dir() -> PathBuf {
    golden_artifact_dir()
}

/// A target, or `None` on a machine with no usable adapter.
fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
    match OffscreenTarget::new(width, height) {
        Ok(target) => Some(target),
        Err(error) => {
            assert!(
                std::env::var_os("NARVO_REQUIRE_GPU").is_none(),
                "NARVO_REQUIRE_GPU is set and no adapter was usable: {error}"
            );
            eprintln!("skipping: no usable adapter ({error})");
            None
        }
    }
}

/// The two atlases stacked into one image, and the row the second starts at.
///
/// **A draw call binds exactly one texture** (`render_sprites` takes one
/// `image`), so a scene showing two sizes has to put both in one. Stacking is
/// done here, in the scene, rather than in `narvo-testkit`: M3.34 anchors one
/// atlas per size and this must not disturb that. The atlases are copied
/// unchanged and the 32 px regions are shifted down by the offset.
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

/// Everything the scene needs: the texture, the placed glyphs, the sprites.
fn scene() -> (Pixels, Vec<PlacedGlyph>, Vec<SpriteInstance>) {
    let small = glyph_atlas::rasterize(16.0);
    let large = glyph_atlas::rasterize(32.0);
    let (texture, offset) = stacked_atlases(&small, &large);

    let mut placed = text::layout_line(LINE_16, &small, MARGIN, BASELINE_16);
    for glyph in text::layout_line(LINE_32, &large, MARGIN, BASELINE_32) {
        // The 32 px atlas sits below the 16 px one in the stacked texture, so
        // its regions move down by exactly that many rows.
        let mut glyph = glyph;
        glyph.region.top += offset;
        placed.push(glyph);
    }

    let sprites = text::sprites_for(&placed, &texture, WIDTH, HEIGHT);
    (texture, placed, sprites)
}

/// The image the scene must produce, computed without a GPU.
fn model() -> Vec<u8> {
    let (texture, placed, _) = scene();
    text::model_image(&placed, &texture, WIDTH, HEIGHT)
}

/// The rendered scene equals the model, byte for byte.
///
/// **What it proves: the render path is a copy.** At 1:1 with `Nearest` on
/// whole-pixel positions, every drawn byte should be an atlas texel, and this
/// compares all 15 360 of them against a CPU model with no tolerance and no
/// budget. Sampling, filtering, the sRGB round trip, the non-blending pipeline,
/// the uv arithmetic and the world-space conversion are all inside what it
/// catches.
///
/// **What it structurally cannot prove: that the layout is right.** `scene()`
/// produces one list of placed glyphs and both sides consume it — the model
/// blits from it and the sprites are built from it — so an advance, a bearing,
/// a rounding rule or the stacking offset being wrong moves both together and
/// this test stays green. Demonstration (a) of M3.35 showed exactly that.
///
/// Layout is pinned instead by the hand-computed positions in
/// `narvo_testkit::text`'s own tests and by the probes below, and saying so
/// here is what keeps this test from being read as more than it is.
#[test]
fn the_scene_is_exactly_what_the_model_predicts() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, _, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("the line is far inside the batch limit");
    let expected = model();

    assert_eq!(rendered.rgba().len(), expected.len());

    let mut differing = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let offset = ((y * WIDTH + x) * 4) as usize;
            let got = &rendered.rgba()[offset..offset + 4];
            let want = &expected[offset..offset + 4];
            if got != want {
                differing.push((x, y, got.to_vec(), want.to_vec()));
            }
        }
    }

    assert!(
        differing.is_empty(),
        "{} of {} pixels differ from the model. First five: {:?}",
        differing.len(),
        WIDTH * HEIGHT,
        &differing[..differing.len().min(5)]
    );
}

/// The numbers the prediction is stated as, so the report can quote them.
///
/// Printed rather than only asserted: the model's totals are what
/// `target/reports/M3.35.md` records as the prediction, and a number in a report
/// that no run prints is a number nobody checked.
#[test]
fn the_models_numbers_are_the_predicted_ones() {
    let expected = model();

    let lit = expected
        .chunks_exact(4)
        .filter(|texel| texel[3] != 0 && texel[..3] != [0, 0, 0])
        .count();
    let fully = expected
        .chunks_exact(4)
        .filter(|texel| texel[3] == 255 && texel[0] == 255)
        .count();
    let transparent = expected
        .chunks_exact(4)
        .filter(|texel| texel[3] == 0)
        .count();

    println!("model: {lit} lit pixels, {fully} fully covered, {transparent} transparent");

    let at = |x: u32, y: u32| {
        let offset = ((y * WIDTH + x) * 4) as usize;
        [
            expected[offset],
            expected[offset + 1],
            expected[offset + 2],
            expected[offset + 3],
        ]
    };

    // Three named probes, fixed here so a change to any of them is a change to
    // this file rather than to a number in a report.
    println!("probe 16 px apex  (12, 14): {:?}", at(12, 14));
    println!("probe 32 px apex  (15, 44): {:?}", at(15, 44));
    println!("probe inner edge  (11, 18): {:?}", at(11, 18));
    println!("probe last 16 px  (127, 17): {:?}", at(127, 17));
    println!("probe last 32 px  (142, 47): {:?}", at(142, 47));

    assert_eq!(lit, LIT_PIXELS, "the model's lit-pixel count moved");
    assert_eq!(fully, FULLY_COVERED, "the fully-covered count moved");
    assert_eq!(
        transparent, TRANSPARENT,
        "the transparent-inside-a-box count moved"
    );
    assert_eq!(at(12, 14), PROBE_16, "the 16 px apex probe moved");
    assert_eq!(at(15, 44), PROBE_32, "the 32 px apex probe moved");
    assert_eq!(at(11, 18), PROBE_EDGE, "the inner-edge probe moved");
    assert_eq!(
        at(127, 17),
        PROBE_LAST_16,
        "the 16 px last-glyph probe moved"
    );
    assert_eq!(
        at(142, 47),
        PROBE_LAST_32,
        "the 32 px last-glyph probe moved"
    );
}

/// Lit pixels in the model: any pixel with a non-zero colour.
///
/// The three counts are not a partition, and reading them as one would be
/// wrong: of the 15 360 pixels in the image, **1 819 are lit** and **445 of
/// those same 1 819** are fully covered. Both numbers are M3.35's and **neither
/// moved in M4.7** — the colour picture is what it was.
const LIT_PIXELS: usize = 1_819;

/// Pixels at full coverage — a subset of [`LIT_PIXELS`], not a separate class.
const FULLY_COVERED: usize = 445;

/// Pixels the model leaves transparent. **Zero since M4.7, and 1 217 before it.**
///
/// This constant is the one number in this file that the blend change moved, and
/// it moved to the floor. Under `blend: None` a glyph's quad replaced its whole
/// rectangle, so every zero-coverage texel inside a box wrote `[0, 0, 0, 0]`
/// over the opaque clear — the "boxes" M3.35 recorded as by design, 1 217 of
/// them. Premultiplied `OVER` (ADR-0023) makes such a source contribute nothing,
/// so the clear stands and nothing in the image is transparent.
///
/// **Two independent instruments agree on the 1 217.** This model counted them
/// before the change, and a channel-wise comparison of the pre-blend reference
/// against the post-blend render counted 1 217 pixels going from alpha 0 to
/// alpha 255 — out of 2 591 that moved in alpha at all, the rest having been
/// partially covered. The M4.7 report carries the distribution.
///
/// Kept as a named constant at zero rather than deleted: a count that is
/// asserted to be zero is what catches the boxes coming back.
const TRANSPARENT: usize = 0;

/// The apex of the 16 px `A`, fully covered.
const PROBE_16: [u8; 4] = [255, 255, 255, 255];

/// The apex of the 32 px `A`, fully covered.
const PROBE_32: [u8; 4] = [255, 255, 255, 255];

/// **The inner edge of the 16 px `A`'s counter** — the triangular hole.
///
/// Partial coverage, so neither 0 nor 255: this is the probe that would move if
/// anything resampled the glyph instead of copying it, and the one a `Linear`
/// draw or an off-grid position would disturb first.
///
/// **Its colour is M3.35's 53 and did not move in M4.7; its alpha did**, from 53
/// to 255. Under `blend: None` the quad wrote the premultiplied texel whole,
/// alpha included; under premultiplied `OVER` the colour is the same 53 over a
/// black ground and the alpha is `53 + 255 * (1 - 53/255) = 255`. The same holds
/// for both late probes below. That the *colour* of all three is untouched is
/// the single clearest statement of what this change did and did not do.
const PROBE_EDGE: [u8; 4] = [53, 53, 53, 255];

/// **The stem of the `!` that ends the 16 px line** — fourteen advances out.
///
/// The first three probes all sit in the *first* glyph of a line, and a first
/// glyph is the one thing an advance error cannot move: demonstration (a)
/// injected a one-pixel advance error and this test stayed green until this
/// probe existed.
///
/// The lit-pixel count does not separate it either — 1 819 either way — and
/// **not because a shift preserves it.** Glyph boxes here do overlap: eight
/// pixels are covered twice, at `x = 16`, `y = 16..=23`, where `A`'s box meets
/// `m`'s. Until M4.7 the later glyph *replaced* the earlier across its whole
/// box; since M4.7 it composites over it. **The eight pixels come out the same
/// either way**, because `A`'s own coverage is zero at all eight — a replacement
/// wrote transparent black over `m`'s ink and a composite contributes nothing to
/// it, and in this scene `m` is the later of the two. The count survives for the
/// same reason it did before: the overlap hides nothing lit and reveals nothing
/// lit when a shift separates them. Move the glyphs the other way and pixels do
/// vanish — under the old semantics they vanished from the *image*, under the
/// new ones they cannot, which is a difference this probe does not measure and
/// `text_over_scene.rs` does. A count is a weak instrument for position, which
/// is the point of a late probe.
const PROBE_LAST_16: [u8; 4] = [137, 137, 137, 255];

/// **The `2` that ends the 32 px line** — eight advances out, partial coverage.
///
/// A second late probe, because one was not enough and the gap was measured
/// rather than reasoned about: with `PROBE_LAST_16` in place but no equivalent
/// on the other line, an advance error applied to the 32 px line alone moves
/// **1 972 of 15 360 pixels** while every assertion in this test stays green —
/// all four other probes lie in the 16 px line or in the 32 px line's first
/// glyph, and the three counts are shift-invariant. M3.12a's lesson, met twice
/// in one task: a probe set is only as good as the errors it can separate.
const PROBE_LAST_32: [u8; 4] = [97, 97, 97, 255];

/// The blessed reference for the text scene.
///
/// **Expected to fail until the maintainer blesses the reference.** Nothing here
/// writes one: the run leaves a `.actual.png` under the target directory and
/// says where, and `GoldenError::ReferenceMissing` carries the instruction. That
/// is the designed outcome of M3.9, not a defect.
///
/// # Two things the blessing has to carry with it
///
/// Both were found by M3.35's audit rather than by planning, and neither is this
/// task's to do — it stops at the red commit. They are written here because the
/// blessing is the next step and would otherwise hit them cold.
///
/// 1. **Blessing this reference turns two currently-green tests red.**
///    `msaa_blessed_margin.rs` and `linear_blessed_margin.rs` each define
///    `every_blessed_reference_has_a_scene_here`, which reads both golden
///    directories and asserts set equality against that file's own `scenes()`
///    list. Both list exactly the five references on disk today, so a sixth
///    file breaks both until each gains a scene for it. That guard is working
///    as designed — it exists so a blessed image cannot sit unmeasured — and
///    the blessing commit owes it two entries rather than an exemption.
/// 2. **The golden tolerance was written with this image in mind and has never
///    been exercised.** `Tolerance::default`'s own doc names the trigger:
///    "Three things M3 adds would give them something: filtered sampling at
///    non-integer texture coordinates, a camera transform that puts vertices
///    between pixel centres, and **glyph coverage**. The first reference image
///    containing one of them is when these values are worth arguing about
///    again." This is that image. It is also the one image least likely to need
///    the tolerance, since it is a copy — but the decision is the maintainer's
///    and the pointer belongs here.
#[test]
fn the_text_scene_matches_its_golden_reference() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, _, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("the line is far inside the batch limit");

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

/// Every glyph region this scene draws carries the border it claims.
///
/// The scene's own copy of M3.34's check, against the *stacked* texture rather
/// than either atlas: stacking shifts every 32 px region and a shift that lost a
/// border row would still draw a plausible line.
#[test]
fn every_region_the_scene_draws_is_padded_in_the_stacked_texture() {
    let (texture, placed, _) = scene();

    for glyph in &placed {
        narvo_render2d::check_region_padding(
            glyph.region.left,
            glyph.region.top,
            glyph.region.width,
            glyph.region.height,
            glyph_atlas::GLYPH_PADDING_TEXELS,
            &texture,
        )
        .unwrap_or_else(|defect| {
            let ch = glyph.ch;
            panic!("{ch:?} is not padded in the stacked texture: {defect}")
        });
    }

    assert!(!placed.is_empty(), "the scene drew nothing");
}

/// Writes the blessing material: the candidate image and a 12x magnifier.
///
/// Not an assertion and not a reference — `tests/golden/` is untouched. It
/// exists because a first blessing needs something a human can actually look
/// at, and a 192 x 80 image of 16 px text is not that.
///
/// **The captions are drawn into the image**, with this crate's own layout and
/// the 32 px atlas. M3.26 recorded the failure that makes this the rule: panel
/// meaning that lives only in a filename or a report is meaning that can drift
/// away from the picture, and the M3.29 crop labelling caught a real
/// mislabelling on its first use. Here it also does something second: the
/// caption is itself drawn text, so a caption that reads correctly is one more
/// witness that the layout works.
#[test]
fn the_blessing_material_is_written() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, _, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("the line is far inside the batch limit");

    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("reports")
        .join("M3.35-crops");
    std::fs::create_dir_all(&directory).expect("the crop directory");

    rendered
        .save_png(directory.join("candidate-full-192x80.png"))
        .expect("the candidate is written");

    // One word per size, magnified twelve times. The crops are named by the
    // rectangle they take, and the same numbers go into the caption.
    let crops = [
        ("16 px  Amb  x6-36 y11-27  12x", 6, 11, 30, 16),
        ("32 px  Amb  x6-62 y40-72  12x", 6, 40, 56, 32),
    ];

    const ZOOM: u32 = 12;
    let caption_atlas = glyph_atlas::rasterize(32.0);
    let caption_height = 44;

    let panels: Vec<(String, Vec<u8>, u32, u32)> = crops
        .iter()
        .map(|(caption, left, top, width, height)| {
            let (zoom_w, zoom_h) = (width * ZOOM, height * ZOOM);
            let mut panel = vec![0_u8; (zoom_w * zoom_h * 4) as usize];

            for y in 0..zoom_h {
                for x in 0..zoom_w {
                    let source = rendered
                        .pixel(left + x / ZOOM, top + y / ZOOM)
                        .expect("the crop lies inside the candidate");
                    let offset = ((y * zoom_w + x) * 4) as usize;
                    panel[offset..offset + 4].copy_from_slice(&source);
                }
            }

            ((*caption).to_owned(), panel, zoom_w, zoom_h)
        })
        .collect();

    let sheet_width = panels
        .iter()
        .map(|(_, _, w, _)| *w)
        .max()
        .expect("two panels");
    let sheet_height: u32 = panels.iter().map(|(_, _, _, h)| h + caption_height).sum();

    let mut sheet = vec![0_u8; (sheet_width * sheet_height * 4) as usize];
    for texel in sheet.chunks_exact_mut(4) {
        texel.copy_from_slice(&[0, 0, 0, 255]);
    }

    let mut pen_y = 0;
    for (caption, panel, panel_w, panel_h) in &panels {
        // The caption, laid out by the same code the scene uses.
        let placed = text::layout_line(caption, &caption_atlas, 8.0, (pen_y + 30) as f32);
        for glyph in &placed {
            for dy in 0..glyph.region.height {
                for dx in 0..glyph.region.width {
                    let texel = caption_atlas
                        .pixels()
                        .pixel(glyph.region.left + dx, glyph.region.top + dy)
                        .expect("inside the atlas");
                    let (x, y) = (glyph.left as u32 + dx, glyph.top as u32 + dy);
                    if x < sheet_width && y < sheet_height {
                        let offset = ((y * sheet_width + x) * 4) as usize;
                        sheet[offset..offset + 4].copy_from_slice(&texel);
                    }
                }
            }
        }
        pen_y += caption_height;

        for y in 0..*panel_h {
            for x in 0..*panel_w {
                let from = ((y * panel_w + x) * 4) as usize;
                let to = (((pen_y + y) * sheet_width + x) * 4) as usize;
                sheet[to..to + 4].copy_from_slice(&panel[from..from + 4]);
            }
        }
        pen_y += panel_h;
    }

    Pixels::from_rgba8(sheet_width, sheet_height, sheet)
        .expect("the sheet buffer matches its dimensions")
        .save_png(directory.join("magnified-12x-labelled.png"))
        .expect("the magnifier sheet is written");

    println!("blessing material in {}", directory.display());
}

/// `Linear` draws this scene identically to `Nearest`, byte for byte.
///
/// The module doc says the filter choice does not change *this* image and gives
/// a number; this is that number. It is a test rather than a sentence because a
/// claim about the renderer that nothing runs is a claim nobody checked — and
/// because it is the evidence that choosing `Nearest` costs the scene nothing,
/// which is what makes the choice about off-grid behaviour rather than about
/// this picture.
///
/// It would go red the moment a glyph landed off the pixel grid, which makes it
/// a second, independent guard on the layout's snapping.
#[test]
fn linear_draws_this_scene_identically_to_nearest() {
    let Some(target) = target_or_skip(WIDTH, HEIGHT) else {
        return;
    };

    let (texture, _, sprites) = scene();
    let nearest = target
        .render_sprites(&texture, &sprites)
        .expect("the line is far inside the batch limit");

    let relit: Vec<SpriteInstance> = sprites
        .iter()
        .map(|sprite| {
            SpriteInstance::new(sprite.placement, sprite.region).sampled(SpriteFilter::Linear)
        })
        .collect();
    let linear = target
        .render_sprites(&texture, &relit)
        .expect("the line is far inside the batch limit");

    let differing = nearest
        .rgba()
        .iter()
        .zip(linear.rgba())
        .filter(|(a, b)| a != b)
        .count();

    assert_eq!(
        differing, 0,
        "{differing} bytes differ between the two filters, so this scene is not \
         on the pixel grid after all"
    );
}
