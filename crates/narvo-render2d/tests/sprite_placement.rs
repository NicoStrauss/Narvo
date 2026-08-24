//! Pixel probes for the sprite path: does a placed sprite land where the
//! convention says it lands?
//!
//! Not a golden-image test, deliberately. A reference image is blessed by a
//! human who has looked at it, and this task has no blessed reference for a
//! sprite; inventing one would assert only that the renderer agrees with
//! whatever it produced the first time. Four probes with expectations derived
//! from ADR-0004 before anything rendered assert something a self-blessed image
//! cannot: that the output matches a *prediction*.
//!
//! # How the expectations were derived
//!
//! Target 64 x 64, sprite scaled to 32 x 32 world units at the origin. One
//! world unit is one target pixel and the origin is the target's centre
//! (`Projection`), so the sprite covers world `-16 ..= 16` on both axes.
//!
//! For a framebuffer pixel `(px, py)` — `py` counted downward from the top row,
//! ADR-0004's `Pixels` row — the GPU's viewport transform gives
//! `ndc_x = (px + 0.5) / 32 - 1` and `ndc_y = 1 - (py + 0.5) / 32`, and the
//! projection gives `world = ndc * 32`. The texture coordinate follows from the
//! sprite's own extent: `u = world_x / 32 + 0.5`, and `v = 0.5 - world_y / 32`
//! because texture rows run down while world y runs up.
//!
//! | pixel | world | u, v | quadrant | expected |
//! | --- | --- | --- | --- | --- |
//! | (20, 20) | (-11.5, +11.5) | 0.141, 0.141 | top left | red |
//! | (44, 20) | (+12.5, +11.5) | 0.891, 0.141 | top right | green |
//! | (20, 44) | (-11.5, -12.5) | 0.141, 0.891 | bottom left | blue |
//! | (44, 44) | (+12.5, -12.5) | 0.891, 0.891 | bottom right | brown |
//! | (4, 4) | (-27.5, +27.5) | outside | none | black |
//!
//! All five expectations are pairwise distinct, which is what makes a mirroring
//! in either axis fail rather than swap two identical values.

use std::path::{Path, PathBuf};

use narvo_render2d::{
    MAX_SPRITES_PER_BATCH, OffscreenTarget, RenderError, SpriteFilter, SpriteInstance,
    SpritePlacement,
};

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The four quadrant colours of the fixture texture, in image order.
///
/// Asymmetric on both axes on purpose, and that property is asserted rather
/// than assumed by `the_fixture_is_asymmetric_on_both_axes` below. A texture
/// symmetric in either axis would let the corresponding flip pass unnoticed,
/// which is the whole failure this file exists to catch.
const TOP_LEFT: [u8; 4] = narvo_testkit::QUADRANT_TOP_LEFT;
const TOP_RIGHT: [u8; 4] = narvo_testkit::QUADRANT_TOP_RIGHT;
const BOTTOM_LEFT: [u8; 4] = narvo_testkit::QUADRANT_BOTTOM_LEFT;
const BOTTOM_RIGHT: [u8; 4] = narvo_testkit::QUADRANT_BOTTOM_RIGHT;

/// What the target is cleared to, and therefore what "no sprite here" looks
/// like.
const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

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

/// The probes, as `(pixel x, pixel y, expected colour, why)`.
///
/// The expectations are literals because they were worked out from the
/// convention before the first render, not read off an image afterwards. The
/// table in this file's documentation is that derivation.
const PROBES: [(u32, u32, [u8; 4], &str); 5] = [
    (20, 20, TOP_LEFT, "inside, upper left of the sprite"),
    (44, 20, TOP_RIGHT, "inside, upper right"),
    (20, 44, BOTTOM_LEFT, "inside, lower left"),
    (44, 44, BOTTOM_RIGHT, "inside, lower right"),
    (4, 4, BACKGROUND, "outside the sprite entirely"),
];

/// The fixture has to be able to fail for a flip in either axis.
///
/// Runs without a GPU: it is a statement about the texture, not about the
/// renderer. If it ever stops holding, every probe below becomes weaker without
/// any of them going red — the quiet kind of failure this project keeps meeting.
#[test]
fn the_fixture_is_asymmetric_on_both_axes() {
    assert_ne!(
        TOP_LEFT, BOTTOM_LEFT,
        "a y-flip swaps the top and bottom rows; identical colours there would \
         make it invisible"
    );
    assert_ne!(
        TOP_LEFT, TOP_RIGHT,
        "an x-flip swaps the left and right columns"
    );
    assert_ne!(TOP_RIGHT, BOTTOM_RIGHT);
    assert_ne!(BOTTOM_LEFT, BOTTOM_RIGHT);

    let mut colours = [TOP_LEFT, TOP_RIGHT, BOTTOM_LEFT, BOTTOM_RIGHT, BACKGROUND];
    colours.sort_unstable();
    let distinct = colours.windows(2).filter(|pair| pair[0] != pair[1]).count() + 1;
    assert_eq!(
        distinct, 5,
        "all five expected colours must be pairwise distinct, or a probe could \
         pass against the wrong quadrant"
    );
}

#[test]
fn a_placed_sprite_lands_where_the_convention_says_it_does() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let rendered = target
        .render_sprite(
            &narvo_testkit::quadrant_texture(8),
            SpritePlacement::new(32.0, 32.0),
        )
        .expect("drawing one sprite must succeed once a device exists");

    for (x, y, expected, why) in PROBES {
        let actual = rendered
            .pixel(x, y)
            .unwrap_or_else(|| panic!("({x}, {y}) is outside the 64 x 64 target"));

        println!("({x}, {y}) {why}: {actual:?}");
        assert_eq!(
            actual, expected,
            "pixel ({x}, {y}), {why}: expected {expected:?}, rendered {actual:?}. \
             The expectation comes from ADR-0004 and was written before this ran. \
             A mismatch on one probe means the sprite is displaced; a mismatch on \
             a diagonal pair means it is mirrored."
        );
    }
}

/// The probes have to depend on the placement, or they assert nothing about it.
///
/// Without this, a `render_sprite` that ignored its `SpritePlacement` and always
/// drew the screen-filling quad would satisfy every probe above by accident,
/// because the full-screen quad puts the same quadrants in the same corners.
#[test]
fn moving_the_sprite_moves_what_the_probes_see() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let texture = narvo_testkit::quadrant_texture(8);
    let centred = target
        .render_sprite(&texture, SpritePlacement::new(32.0, 32.0))
        .expect("drawing must succeed");

    // Sixteen world units to the right is half the sprite's width, so the probe
    // at (20, 20) - which sat inside the sprite's left half - is now background.
    let moved = target
        .render_sprite(
            &texture,
            SpritePlacement {
                x: 16.0,
                ..SpritePlacement::new(32.0, 32.0)
            },
        )
        .expect("drawing must succeed");

    assert_eq!(centred.pixel(20, 20), Some(TOP_LEFT));
    assert_eq!(
        moved.pixel(20, 20),
        Some(BACKGROUND),
        "moving the sprite 16 world units right must uncover the probe at \
         (20, 20); if it did not, the placement is being ignored and every other \
         probe in this file is passing for the wrong reason"
    );
}

/// Three sprites, no two of them overlapping, in draw order.
///
/// | sprite | centre | scale | covers px | covers py | quadrant split |
/// | --- | --- | --- | --- | --- | --- |
/// | A | (-32, +32) | 32 x 32 (uniform) | 16..47 | 16..47 | px 32, py 32 |
/// | B | (+24, +32) | 48 x 24 | 64..111 | 20..43 | px 88, py 32 |
/// | C | (-16, -40) | 24 x 40 | 36..59 | 84..123 | px 48, py 104 |
///
/// Every edge lands on an integer pixel boundary, for the reason M3.5 gives: no
/// rasteriser then has a coverage decision to make, and the scene stays inside
/// the regime `BASELINE.md` measured as byte-identical across three of them.
///
/// **Overlap-free on purpose.** Draw order is the order of this array and
/// nothing sorts, so overlapping sprites would make the image depend on a
/// decision this milestone has not taken — depth ordering is its own capability
/// in `ProjektPlan.md` §6/M3. Disjoint sprites make the image independent of it.
fn three_sprites() -> [SpritePlacement; 3] {
    [
        SpritePlacement {
            x: -32.0,
            y: 32.0,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: 32.0,
            scale_y: 32.0,
        },
        SpritePlacement {
            x: 24.0,
            y: 32.0,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: 48.0,
            scale_y: 24.0,
        },
        SpritePlacement {
            x: -16.0,
            y: -40.0,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: 24.0,
            scale_y: 40.0,
        },
    ]
}

/// The probes for [`three_sprites`], derived before anything rendered.
///
/// For a pixel `(px, py)` on a 128 x 128 target the centre is at world
/// `(px - 63.5, 63.5 - py)`; within a sprite of centre `c` and scale `s` that
/// gives `u = (world_x - c_x) / s_x + 0.5` and `v = 0.5 - (world_y - c_y) / s_y`.
///
/// | probe | pixel | world | u, v | expected |
/// | --- | --- | --- | --- | --- |
/// | A upper left | (20, 20) | (-43.5, +43.5) | 0.141, 0.141 | red |
/// | A lower right | (44, 44) | (-19.5, +19.5) | 0.891, 0.891 | brown |
/// | B upper right | (100, 24) | (+36.5, +39.5) | 0.760, 0.188 | green |
/// | B lower left | (70, 40) | (+6.5, +23.5) | 0.135, 0.854 | blue |
/// | C upper left | (40, 90) | (-23.5, -26.5) | 0.188, 0.163 | red |
/// | C lower right | (56, 118) | (-7.5, -54.5) | 0.854, 0.863 | brown |
/// | between A and B | (56, 30) | — | outside all three | black |
/// | between A and C | (30, 70) | — | outside all three | black |
/// | above B | (88, 12) | — | outside all three | black |
const BATCH_PROBES: [(u32, u32, [u8; 4], &str); 9] = [
    (20, 20, TOP_LEFT, "sprite A, upper left"),
    (44, 44, BOTTOM_RIGHT, "sprite A, lower right"),
    (100, 24, TOP_RIGHT, "sprite B, upper right"),
    (70, 40, BOTTOM_LEFT, "sprite B, lower left"),
    (40, 90, TOP_LEFT, "sprite C, upper left"),
    (56, 118, BOTTOM_RIGHT, "sprite C, lower right"),
    (56, 30, BACKGROUND, "background between A and B"),
    (30, 70, BACKGROUND, "background between A and C"),
    (88, 12, BACKGROUND, "background just above B"),
];

#[test]
fn a_batch_of_three_sprites_puts_each_one_where_its_placement_says() {
    let Some(target) = target_or_skip(128, 128) else {
        return;
    };

    let rendered = target
        .render_sprites(
            &narvo_testkit::quadrant_texture(8),
            &three_sprites().map(SpriteInstance::whole_texture),
        )
        .expect("three sprites are far inside the batch limit");

    for (x, y, expected, why) in BATCH_PROBES {
        let actual = rendered
            .pixel(x, y)
            .unwrap_or_else(|| panic!("({x}, {y}) is outside the 128 x 128 target"));

        println!("({x}, {y}) {why}: {actual:?}");
        assert_eq!(
            actual, expected,
            "pixel ({x}, {y}), {why}: expected {expected:?}, rendered {actual:?}. \
             The expectation was derived from the projection before this ran. A \
             sprite-probe gone background means that sprite was not drawn or was \
             drawn elsewhere; a background probe gone coloured means a sprite is \
             larger than its placement asks."
        );
    }
}

/// The scene has to stay overlap-free, or a probe stops naming one sprite.
///
/// Runs without a GPU: it is arithmetic about the placements, not about what
/// was drawn.
///
/// **The reason changed in M3.10 and the test did not.** It was written because
/// draw order was undecided, so overlapping sprites would have made the image
/// depend on something §6/M3 had not settled. Draw order is settled now — by
/// depth, then by entity id, in `narvo-app`'s `placements_of` — so that reason
/// has expired. What keeps the test is the other property it always had: every
/// probe in this file identifies a sprite *by position*, and in an overlap
/// region a pixel belongs to two of them. Without this, a probe that started
/// failing would no longer say which sprite moved.
#[test]
fn no_two_sprites_in_the_batch_scene_overlap() {
    let extents: Vec<(f32, f32, f32, f32)> = three_sprites()
        .iter()
        .map(|s| {
            (
                s.x - s.scale_x / 2.0,
                s.x + s.scale_x / 2.0,
                s.y - s.scale_y / 2.0,
                s.y + s.scale_y / 2.0,
            )
        })
        .collect();

    for (i, a) in extents.iter().enumerate() {
        for (j, b) in extents.iter().enumerate().skip(i + 1) {
            let apart = a.1 <= b.0 || b.1 <= a.0 || a.3 <= b.2 || b.3 <= a.2;
            assert!(
                apart,
                "sprites {i} and {j} overlap: {a:?} against {b:?}. Overlapping \
                 sprites make the image depend on draw order, which §6/M3 has not \
                 decided, so the probes would be asserting something this task is \
                 not entitled to assert."
            );
        }
    }
}

/// A world with nothing in it renders a cleared target, not a panic.
///
/// Found while breaking the batch on purpose for M3.6's regression
/// demonstration: an empty batch bound a zero-length vertex buffer, and
/// `Buffer::slice` panics on one inside wgpu. A world whose entities have no
/// `Transform` yet is an ordinary state, not a fault, so it has to render.
#[test]
fn an_empty_batch_clears_the_target_instead_of_panicking() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let rendered = target
        .render_sprites(&narvo_testkit::quadrant_texture(8), &[])
        .expect("an empty batch is a legal batch");

    for (x, y) in [(0_u32, 0_u32), (32, 32), (63, 63)] {
        assert_eq!(
            rendered.pixel(x, y),
            Some(BACKGROUND),
            "an empty batch must leave the target at its clear colour"
        );
    }
}

/// A batch larger than the cap is refused, not truncated.
#[test]
fn an_oversized_batch_is_an_error_rather_than_a_silent_truncation() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let too_many = vec![
        SpriteInstance::whole_texture(SpritePlacement::new(4.0, 4.0));
        MAX_SPRITES_PER_BATCH + 1
    ];
    let error = target
        .render_sprites(&narvo_testkit::quadrant_texture(8), &too_many)
        .expect_err("one past the cap must fail");

    let message = error.to_string();
    println!("oversized batch: {message}");
    assert!(
        message.contains("exceeds the limit"),
        "the message must name the limit, got: {message}"
    );
}

/// Both filters draw, and a batch may mix them.
///
/// The positive counterpart of M3.20's `a_filter_the_renderer_cannot_honour_is_refused_rather_than_ignored`,
/// which this replaces. That test existed because the renderer honoured
/// `Nearest` only and refused the rest; M3.23 honours both, so there is no
/// unsupported filter left to refuse and `RenderError::UnsupportedFilter` is
/// gone with it. What has to be exercised now is the opposite: that a `Linear`
/// wish reaches a picture instead of an error.
///
/// It asserts nothing about *what* the two pictures look like — that is
/// `linear_production.rs`'s subject. Here: a mixed batch renders, and the two
/// filters do not render the same thing, which is the cheapest possible proof
/// that the wish is not being dropped on the way through.
#[test]
fn both_filters_draw_and_a_batch_may_mix_them() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let texture = narvo_testkit::quadrant_texture(8);
    let placement = SpritePlacement::new(4.0, 4.0);

    // A mixed batch: two runs, two draw calls, no error.
    target
        .render_sprites(
            &texture,
            &[
                SpriteInstance::whole_texture(placement),
                SpriteInstance::whole_texture(placement).sampled(SpriteFilter::Linear),
            ],
        )
        .expect("both filters are honoured since M3.23");

    // And the two are not the same picture. A 48 x 48 sprite over an 8 x 8
    // texture magnifies six pixels to the texel, so `Linear` has something to
    // blend across each quadrant boundary and `Nearest` does not.
    let big = SpritePlacement {
        x: 0.0,
        y: 0.0,
        rot_cos: 1.0,
        rot_sin: 0.0,
        scale_x: 48.0,
        scale_y: 48.0,
    };
    let nearest = target
        .render_sprites(&texture, &[SpriteInstance::whole_texture(big)])
        .expect("a single sprite is inside the batch limit");
    let linear = target
        .render_sprites(
            &texture,
            &[SpriteInstance::whole_texture(big).sampled(SpriteFilter::Linear)],
        )
        .expect("a single sprite is inside the batch limit");

    assert_ne!(
        nearest.rgba(),
        linear.rgba(),
        "the two samplers drew the same image, so the wish reached no sampler"
    );
}

/// Records how long an offscreen round trip takes with N sprites in the batch.
///
/// **This is not the 60 fps figure of `ProjektPlan.md` §6/M3, and it cannot
/// become it.** What is timed here is one full `render_sprites` call: building
/// the vertex and index buffers, uploading the texture, encoding the pass,
/// submitting, and then blocking until the result has been copied back to the
/// CPU. A frame in a window does none of the last part and does not stall on
/// the GPU at all — it hands a swapchain image to the compositor and moves on.
/// The read-back is what makes the number measurable without a frame loop, and
/// it is also what makes it an upper bound rather than a frame time.
///
/// Recorded rather than asserted. There is no threshold here: a wall-clock
/// budget for GPU work on whatever adapter happens to be present is the kind of
/// gate that gets raised after the third red build. The batch preparation, which
/// is CPU-side and where a regression is a change of shape rather than a change
/// of machine, is gated in `sprite.rs` instead.
#[test]
fn the_cost_of_an_offscreen_round_trip_is_recorded() {
    let Some(target) = target_or_skip(1920, 1080) else {
        return;
    };

    let texture = narvo_testkit::quadrant_texture(8);
    println!("sprites | offscreen round trip ms | adapter");

    for count in [1_usize, 1_000, 10_000, 50_000] {
        let placements: Vec<SpriteInstance> = (0..count)
            .map(|index| {
                let step = (index % 512) as f32;
                SpriteInstance::whole_texture(SpritePlacement {
                    x: step * 3.0 - 768.0,
                    y: step * 2.0 - 512.0,
                    rot_cos: 1.0,
                    rot_sin: 0.0,
                    scale_x: 16.0,
                    scale_y: 16.0,
                })
            })
            .collect();

        // Best of three, as `docs/perf/BASELINE.md` takes every other figure.
        let mut best = f64::MAX;
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let rendered = target
                .render_sprites(&texture, &placements)
                .expect("the batch is inside the limit");
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(rendered.width(), 1920, "the render must have happened");
            best = best.min(elapsed);
        }

        assert!(
            best > 0.0,
            "an offscreen round trip that took no measurable time did not happen"
        );
        println!("{count:7} | {best:23.2} | {}", target.adapter_summary());
    }
}

/// Writes a human-sized render under the target directory for visual review.
///
/// Not an assertion and not a reference. `tests/golden/` is untouched: blessing
/// an image is a human step, and this file only makes the image easy to look at.
#[test]
fn a_preview_is_written_for_the_maintainer_to_look_at() {
    let Some(target) = target_or_skip(256, 256) else {
        return;
    };

    let rendered = target
        .render_sprite(
            &narvo_testkit::quadrant_texture(8),
            SpritePlacement::new(160.0, 128.0),
        )
        .expect("drawing must succeed");

    let path: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sprite-preview.png");
    rendered
        .save_png(&path)
        .expect("the preview must be writable");

    println!("preview written to {}", path.display());
}
