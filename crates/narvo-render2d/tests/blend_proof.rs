//! The first scene that composites, and the measurement that says how.
//!
//! Eight 32 x 32 panels over two solid grounds: four alpha steps over opaque
//! red, the same four over opaque black, drawn `Nearest` at four pixels per
//! texel from `narvo_testkit::blend`. Every edge is on a whole pixel and every
//! panel interior is fully covered, so multisampling contributes nothing and the
//! only arithmetic in the picture is the blend itself.
//!
//! # Why this scene exists in the form it does
//!
//! `src + dst * (1 - a)` does not say **where** the arithmetic happens, and on
//! this project's target it is not where the equation looks like it is. The
//! render target is `Rgba8UnormSrgb`. Two readings were available:
//!
//! - **stored bytes** — the blend works on the bytes as they sit in the target,
//!   so a step of 85 over a channel of 255 gives `85 + 255 * 170 / 255 = 255`;
//! - **linear light** — the hardware decodes the target to linear, blends there
//!   and re-encodes on write, so the same case gives
//!   `E(D(85/255) + 1 - 85/255)`, which is not 255.
//!
//! The fixture's steps were chosen so that **both readings are whole numbers**
//! over both grounds ( `255 - 85 = 170` and `255 - 170 = 85` divide 255 without
//! a remainder). Nothing about the comparison therefore depends on how any
//! rasteriser rounds, and
//! [`the_two_readings_of_the_blend_are_told_apart`] reports which one happened
//! before this image becomes anything.
//!
//! # The half of the picture that needs no model at all
//!
//! The black row. Every channel of the ground is zero, so `dst * (1 - a)` is
//! zero in any space and the result is the source term alone: a step of `a` must
//! read back exactly `[a, a, a, 255]`. Both readings agree there, and so does
//! any third one. If that row is wrong, the source term is not arriving and no
//! colour-space argument explains it.
//!
//! The same holds at both ends of the red row: a step of 0 is `[0, 0, 0, 0]` and
//! must leave the ground untouched, a step of 255 must replace it outright.
//! **Six of the eight panels are predicted with no transfer function in the
//! prediction**; two are the discriminator.

use narvo_render2d::{
    Golden, OffscreenTarget, Pixels, RenderError, SpriteFilter, SpriteInstance, SpritePlacement,
    TextureRegion, golden_artifact_dir,
};
use narvo_testkit::blend::{self, ALPHA_STEPS, Ground};
use std::path::{Path, PathBuf};

/// Printed instead of failing when this machine has no GPU adapter at all.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set in CI, where a missing adapter is a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// The scene's name, and the stem of its reference file.
const SCENE: &str = "blend_proof_steps_128x128";

/// The canvas, in pixels. Square, and a multiple of the panel grid.
const SIZE: u32 = 128;

/// One panel's edge, in pixels. Four times the fixture's cell, so a panel
/// magnifies its texel by exactly four.
const PANEL: u32 = 32;

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

/// A sprite at a pixel rectangle, expressed the way the world would express it.
///
/// World units are pixels and the camera is the identity, so a centre and a size
/// in world units *is* a rectangle in the image — with y running the other way
/// (ADR-0004), which is why the rows below are computed from the centre rather
/// than assumed.
fn sprite(x: f32, y: f32, width: f32, height: f32, region: TextureRegion) -> SpriteInstance {
    SpriteInstance::new(
        SpritePlacement {
            x,
            y,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: width,
            scale_y: height,
        },
        region,
    )
    .sampled(SpriteFilter::Nearest)
}

/// World x of the centre of panel column `column`, counted from the left.
fn column_centre(column: u32) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "four columns of 32 pixels on a 128 pixel canvas are exact in f32"
    )]
    let centre = (column * PANEL + PANEL / 2) as f32 - (SIZE / 2) as f32;
    centre
}

/// The image column a panel's centre falls in.
const fn column_probe(column: u32) -> u32 {
    column * PANEL + PANEL / 2
}

/// World y of the red row's centre: image rows 16..48.
const RED_ROW_CENTRE: f32 = 32.0;
/// World y of the black row's centre: image rows 80..112.
const BLACK_ROW_CENTRE: f32 = -32.0;

/// The image row a world y falls in.
///
/// World y grows upward and image rows grow downward (ADR-0004), so the two run
/// against each other about the canvas's middle. Derived rather than written
/// down, because the first version of this file wrote down the black row's *top*
/// edge and called it a centre — and the probe still landed inside the panel, so
/// only a test that walked the whole panel caught it.
const fn row_of(world_y: i32) -> u32 {
    #[expect(
        clippy::cast_sign_loss,
        reason = "both call sites are inside the canvas by construction"
    )]
    let row = (SIZE as i32 / 2 - world_y) as u32;
    row
}

/// The image row the red row's panel centres fall in: 32.
const RED_ROW_PROBE: u32 = row_of(32);
/// The image row the black row's panel centres fall in: 96.
const BLACK_ROW_PROBE: u32 = row_of(-32);

/// The scene: the fixture, and the ten sprites in draw order.
///
/// Order is composition (ADR-0023): the red ground covers the canvas, the black
/// band covers the lower row's rectangle, and the eight steps go over them. A
/// scene rendered in any other order is a different picture, which is what
/// blending makes true and what `blend: None` hid.
fn scene() -> (Pixels, Vec<SpriteInstance>) {
    let texture = blend::atlas();

    #[expect(clippy::cast_precision_loss, reason = "128 and 32 are exact in f32")]
    let (canvas, panel) = (SIZE as f32, PANEL as f32);

    let mut sprites = vec![
        // The red ground, over the whole canvas.
        sprite(
            0.0,
            0.0,
            canvas,
            canvas,
            blend::ground_region(Ground::RED, &texture),
        ),
        // The black band, exactly under the lower row of panels.
        sprite(
            0.0,
            BLACK_ROW_CENTRE,
            canvas,
            panel,
            blend::ground_region(Ground::BLACK, &texture),
        ),
    ];

    for (column, alpha) in ALPHA_STEPS.iter().enumerate() {
        let x = column_centre(u32::try_from(column).expect("four columns fit in a u32"));
        let region = blend::step_region(*alpha, &texture);
        sprites.push(sprite(x, RED_ROW_CENTRE, panel, panel, region));
        sprites.push(sprite(x, BLACK_ROW_CENTRE, panel, panel, region));
    }

    (texture, sprites)
}

/// IEC 61966-2-1's transfer function, stored byte to linear light.
///
/// Written out rather than taken from a crate, the same way `camera_motion.rs`
/// and `camera_pan_steps.rs` write it out, and for the same reason: a reference
/// image outlives a dependency's rounding.
fn decode(byte: u8) -> f64 {
    let c = f64::from(byte) / 255.0;
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The other direction, linear light to a stored byte.
fn encode(linear: f64) -> u8 {
    let encoded = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        linear.powf(1.0 / 2.4).mul_add(1.055, -0.055)
    };

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "encoded is in 0.0..=1.0, so the product is in 0..=255"
    )]
    let byte = (encoded * 255.0).round() as u8;
    byte
}

/// What a step of `alpha` over `ground` produces if the blend happens in
/// **linear light**.
fn predicted_in_linear_light(alpha: u8, ground: [u8; 4]) -> [u8; 4] {
    let coverage = f64::from(alpha) / 255.0;
    let source = decode(alpha);

    let mut out = [0_u8; 4];
    for channel in 0..3 {
        out[channel] = encode(decode(ground[channel]).mul_add(1.0 - coverage, source));
    }

    // Alpha is not sRGB-encoded, so its arithmetic is the plain one in both
    // readings: `a + a_dst * (1 - a)`, which over an opaque ground is 255.
    let alpha_out = f64::from(alpha) + f64::from(ground[3]) * (1.0 - coverage);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the sum of two bytes weighted to one is in 0..=255"
    )]
    let rounded = alpha_out.round() as u8;
    out[3] = rounded;
    out
}

/// What the same step produces if the blend happens on the **stored bytes**.
///
/// The reading the fixture's steps were chosen to make exact: over a channel of
/// 0 or 255 this divides without a remainder, so the value below is a whole
/// number rather than a rounding of one.
fn predicted_in_stored_bytes(alpha: u8, ground: [u8; 4]) -> [u8; 4] {
    let complement = u32::from(255 - alpha);
    let mut out = [0_u8; 4];
    for channel in 0..4 {
        let blended = u32::from(alpha) + u32::from(ground[channel]) * complement / 255;
        out[channel] = u8::try_from(blended.min(255)).expect("clamped to a byte");
    }
    out
}

/// The eight panel centres, as `(alpha, ground, x, y)`.
fn probes() -> Vec<(u8, [u8; 4], u32, u32)> {
    let mut probes = Vec::new();
    for (column, alpha) in ALPHA_STEPS.iter().enumerate() {
        let x = column_probe(u32::try_from(column).expect("four columns fit in a u32"));
        probes.push((*alpha, Ground::RED, x, RED_ROW_PROBE));
        probes.push((*alpha, Ground::BLACK, x, BLACK_ROW_PROBE));
    }
    probes
}

/// **The measurement.** Which reading of the blend the hardware follows.
///
/// Open-ended by construction: it computes both predictions, compares each
/// against the render, and asserts only that **one of them is exactly right
/// everywhere**. Whichever it turns out to be is the finding, and the test
/// prints the whole table either way.
///
/// The assertion is deliberately not "linear light wins". A test that named the
/// answer in advance would be a test of the answer, and M4.7's question was
/// which one is true here.
#[test]
fn the_two_readings_of_the_blend_are_told_apart() {
    let Some(target) = target_or_skip(SIZE, SIZE) else {
        return;
    };

    let (texture, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("ten sprites are far inside the batch limit");

    let mut linear_matches = 0_usize;
    let mut stored_matches = 0_usize;
    let mut disagreements = Vec::new();

    for (alpha, ground, x, y) in probes() {
        let measured = rendered
            .pixel(x, y)
            .expect("a probe lies inside the canvas");
        let linear = predicted_in_linear_light(alpha, ground);
        let stored = predicted_in_stored_bytes(alpha, ground);

        let ground_name = if ground == Ground::RED {
            "red"
        } else {
            "black"
        };
        println!(
            "  step {alpha:>3} over {ground_name:<5} at ({x:>3}, {y:>3}): \
             measured {measured:?}  linear-light {linear:?}  stored-bytes {stored:?}"
        );

        if measured == linear {
            linear_matches += 1;
        }
        if measured == stored {
            stored_matches += 1;
        }
        if measured != linear && measured != stored {
            disagreements.push(format!(
                "({x}, {y}) step {alpha} over {ground_name}: measured {measured:?}, \
                 neither linear-light {linear:?} nor stored-bytes {stored:?}"
            ));
        }
    }

    let total = probes().len();
    println!(
        "readings: linear-light matches {linear_matches}/{total}, \
         stored-bytes matches {stored_matches}/{total}"
    );

    assert!(
        disagreements.is_empty(),
        "some panels match neither reading of the blend, so the model in \
         `quad.rs` is wrong about more than the colour space:\n  {}",
        disagreements.join("\n  ")
    );

    assert!(
        linear_matches == total || stored_matches == total,
        "neither reading is right everywhere — linear-light {linear_matches}/{total}, \
         stored-bytes {stored_matches}/{total}. A mixture means the two readings \
         happen to agree on the six model-free panels and disagree on the two \
         that discriminate, which is the case this scene exists to report."
    );
}

/// The six panels whose value needs no transfer function, asserted without one.
///
/// This is the half of the picture that survives any answer to the colour-space
/// question: both ends of the red row, and the whole black row. If a future
/// change to the blend state breaks compositing at all, it breaks here, and the
/// failure message is arithmetic a reader can check on paper.
#[test]
fn the_model_free_panels_are_exactly_what_premultiplied_over_requires() {
    let Some(target) = target_or_skip(SIZE, SIZE) else {
        return;
    };

    let (texture, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("ten sprites are far inside the batch limit");

    // The black row: `dst * (1 - a)` is zero in every space, so the result is
    // the source term alone and a step of `a` reads back as `[a, a, a, 255]`.
    for (column, alpha) in ALPHA_STEPS.iter().enumerate() {
        let x = column_probe(u32::try_from(column).expect("four columns fit in a u32"));
        let measured = rendered
            .pixel(x, BLACK_ROW_PROBE)
            .expect("a probe lies inside the canvas");
        assert_eq!(
            measured,
            [*alpha, *alpha, *alpha, 255],
            "step {alpha} over black at ({x}, {BLACK_ROW_PROBE}) is not the source term alone"
        );
    }

    // The red row's ends: fully transparent leaves the ground, fully opaque
    // replaces it.
    let transparent = column_probe(0);
    assert_eq!(
        rendered
            .pixel(transparent, RED_ROW_PROBE)
            .expect("a probe lies inside the canvas"),
        Ground::RED,
        "a fully transparent premultiplied source changed the ground"
    );

    let opaque = column_probe(3);
    assert_eq!(
        rendered
            .pixel(opaque, RED_ROW_PROBE)
            .expect("a probe lies inside the canvas"),
        [255, 255, 255, 255],
        "a fully opaque source did not replace the ground"
    );
}

/// Every pixel of the image is opaque, which is the alpha formula's own claim.
///
/// `a_out = a_src + a_dst * (1 - a_src)` over an opaque ground is 255 for every
/// source alpha, exactly and in every colour space. So a single transparent
/// pixel anywhere means the alpha component of the blend state is not `OVER` —
/// the failure `One, Zero` would produce, and the one a picture does not show
/// because the colour channels would look right.
#[test]
fn compositing_over_an_opaque_ground_leaves_nothing_transparent() {
    let Some(target) = target_or_skip(SIZE, SIZE) else {
        return;
    };

    let (texture, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("ten sprites are far inside the batch limit");

    let mut transparent = Vec::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let pixel = rendered.pixel(x, y).expect("inside the canvas");
            if pixel[3] != 255 {
                transparent.push(format!("({x}, {y}) = {pixel:?}"));
            }
        }
    }

    assert!(
        transparent.is_empty(),
        "{} pixels are not opaque over an opaque ground, first few: {}",
        transparent.len(),
        transparent
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
}

/// A fully opaque sprite through the blend pipeline is the unblended write.
///
/// The named single test M4.7's opaque-identity flank asks for. `src * 1 + dst *
/// 0` is an identity on paper; whether a rasteriser reproduces it bit for bit is
/// a hardware question, and this answers it in the small: the opaque panel's
/// every pixel is the texel it sampled, with no dependence on what was under it.
///
/// The measurement in the large is the six blessed references, five of which
/// stayed byte-identical when the pipeline changed — that is in the M4.7 report
/// rather than here, because it is a comparison against artefacts this test
/// cannot see.
#[test]
fn an_opaque_sprite_writes_exactly_its_texel_over_anything() {
    let Some(target) = target_or_skip(SIZE, SIZE) else {
        return;
    };

    let (texture, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("ten sprites are far inside the batch limit");

    // The opaque panel is column 3, and it sits over red in the upper row and
    // over black in the lower one. Two different grounds, one result.
    let x = column_probe(3);
    for (row, ground) in [(RED_ROW_PROBE, "red"), (BLACK_ROW_PROBE, "black")] {
        // Every pixel of the panel, not only its centre: an opaque write that
        // was right in the middle and wrong at an edge would be a partial
        // coverage defect, and this scene has no partial coverage.
        for probe_y in row - PANEL / 2 + 1..row + PANEL / 2 - 1 {
            for probe_x in x - PANEL / 2 + 1..x + PANEL / 2 - 1 {
                assert_eq!(
                    rendered.pixel(probe_x, probe_y).expect("inside the canvas"),
                    [255, 255, 255, 255],
                    "the opaque panel over {ground} is not its own texel at \
                     ({probe_x}, {probe_y})"
                );
            }
        }
    }
}

/// Compares the render against the blessed reference.
///
/// **A first-blessing candidate.** Until a human has looked at the image and
/// committed it, this test is the thing that says whether the candidate on disk
/// is what the code produces.
#[test]
fn the_blend_scene_matches_its_golden_reference() {
    let Some(target) = target_or_skip(SIZE, SIZE) else {
        return;
    };

    let (texture, sprites) = scene();
    let rendered = target
        .render_sprites(&texture, &sprites)
        .expect("ten sprites are far inside the batch limit");

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
