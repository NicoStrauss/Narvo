//! Golden-image test for the textured quad of M1.2.
//!
//! This does not replace the per-pixel assertions in the crate's unit tests. Those
//! pin the handful of positions that carry the orientation convention; this pins
//! the whole frame, so a change nobody thought to sample still shows up.

use std::path::{Path, PathBuf};

use narvo_render2d::{
    Golden, OffscreenTarget, RenderError, SpriteFilter, SpriteInstance, SpritePlacement,
    golden_artifact_dir,
};

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set this, to anything, and a missing adapter fails instead of skipping. CI
/// sets it; a developer machine normally does not.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Where blessed reference images live. Read-only for anything automated - see
/// the README in that directory.
fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Where a failing comparison writes what it rendered and how it differs.
///
/// [`golden_artifact_dir`] rather than this file's own `CARGO_TARGET_TMPDIR`
/// path, since M3.11: both landed under the Cargo target directory, which the
/// repository already ignores, but they landed in *different* directories, and
/// the one rule a maintainer needs is where to look. The unit test in
/// `narvo-app` cannot use `CARGO_TARGET_TMPDIR` at all — cargo sets it only for
/// integration tests and benches — so the shared derivation is the one with no
/// exception.
fn output_dir() -> PathBuf {
    golden_artifact_dir()
}

/// Builds a target, or reports that this machine cannot host one.
///
/// Only a missing adapter skips, and only when [`REQUIRE_GPU_VAR`] is unset.
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

#[test]
fn the_textured_quad_matches_its_golden_reference() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let rendered = target
        .render_textured_quad(&narvo_testkit::quadrant_texture(8))
        .expect("drawing a textured quad must succeed once a device exists");

    let references = reference_dir();
    let output = output_dir();
    let golden = Golden::new(&references, &output);

    match golden.verify("textured_quad_quadrants_64x64", &rendered) {
        // A passing comparison prints its full measurement, not just the fact
        // that it passed. What the margin actually is on this rasteriser is the
        // question M3 has to answer once per reference image, and answering it
        // from a green log costs nothing; reconstructing it later costs a run
        // per platform.
        Ok(report) => println!(
            "golden match for \"textured_quad_quadrants_64x64\": {}",
            report.measured_against(golden.tolerance)
        ),
        // The error carries the paths, the metrics and the instruction. Printing
        // it as the panic message is the whole point: the failure has to be
        // actionable from the test output alone.
        Err(error) => panic!("{error}"),
    }
}

/// The scene the placed sprite of M3.4 is blessed on, and what M3.6 draws again.
///
/// # Why these numbers
///
/// A 128 x 128 target, one sprite 48 x 32 world units, centred at
/// `(+16.0, -8.0)`. Three properties, each of them the reason for one of the
/// numbers:
///
/// - **Off-centre on both axes, and non-uniformly scaled.** Batching in M3.6
///   rebuilds per-sprite vertices and merges them into one buffer. A sprite at
///   the origin at uniform scale would let a batch that dropped the translation,
///   swapped the two scale axes, or emitted its corners in another order
///   reproduce this image exactly. None of those survives here.
/// - **Every edge on an integer pixel boundary.** One world unit is one target
///   pixel, so the sprite covers x in `56..104` and y in `56..88` exactly. No
///   edge passes through a pixel centre, and no rasteriser has a coverage
///   decision to get right differently from another.
/// - **Rotation is deliberately absent**, and that is a judgement worth stating
///   rather than hiding. A turned sprite would stress batching harder, but its
///   edges would cross pixel centres — precisely the case `BASELINE.md` records
///   as *not* measured across rasterisers. Making the project's first blessed
///   sprite reference depend on an untested rasterisation property would buy
///   coverage with a reference nobody can trust. Rotation stays covered by the
///   derived pixel probes in `sprite_placement.rs` and in
///   `narvo-app/tests/transform_to_sprite.rs`, which sample well inside
///   quadrants and do not depend on where an edge falls.
///
/// # `Linear` since M3.27
///
/// The second of the four blessed scenes converted (D13, order in
/// `ProjektPlan.md` §12). **One place:** the wish is set on the `SpriteInstance` the
/// test below hands to `render_sprites`, and that is the only place in the
/// workspace where this scene asks for anything but the default.
///
/// Three other files transcribe the same placement by hand —
/// `msaa_blessed_margin.rs:234`, `linear_blessed_margin.rs:218` and
/// `draw_order_margin.rs:183` — and none of them moves, for two different
/// reasons worth keeping apart. `linear_blessed_margin.rs` names a wish on every
/// sprite it renders (`.sampled(wish)`, :775) and so is immune by construction.
/// The other two build theirs with `SpriteInstance::new` (`msaa_blessed_margin.rs:743`,
/// `draw_order_margin.rs:411`), which takes the `Nearest` **default** — they are
/// deliberately-`Nearest` replicas in the M3.24 sense, and what protects them is
/// that this change does not touch that default. Change `SpriteInstance::new` instead of
/// the call site and both would move silently.
///
/// **Whole texture, so no padding — `ClampToEdge` is the whole guard.**
/// `TextureRegion::WHOLE_TEXTURE` cannot be padded; its border would lie
/// outside the texture. What M3.21's one-texel border does at a region boundary,
/// `AddressMode::ClampToEdge` does at the texture rim: the blend partner one
/// texel out is a copy of the rim texel itself, so the rim stays pure. That step
/// is recorded in `src/shaders/quad.wgsl:55-56`, and the sampler that makes it
/// true is `src/quad.rs:422-424`. **Nothing in this file asserts it** — the
/// conversion's own check was M3.27's, off to the side of the test: (56, 56)
/// still reads exactly `[255, 0, 0, 255]` and (103, 87) `[128, 64, 32, 255]`.
///
/// **No silhouette moves.** Every edge lands on an integer pixel boundary (the
/// bullet above), so no pixel is partly covered and the sampler cannot reach the
/// outline. Checked in M3.27 by recomputing the difference from the two PNGs,
/// not asserted here: all 360 changed pixels lie inside the footprint
/// x 56–103, y 56–87.
///
/// **What changes is two interior bands, and they are the whole difference.**
/// Eight texels over 48 px is 6 px per texel across, over 32 px is 4 down, so
/// the blend spans the six columns and four rows whose sample point falls
/// between texel centres 3.5 and 4.5:
///
/// - **columns 77–82**, all 32 rows — 192 pixels, the vertical quadrant boundary
/// - **rows 70–73**, all 48 columns — 192 pixels, the horizontal one
/// - they share 24, so 192 + 192 − 24 = **360 of 16384 (2.1973 %)**, which is
///   what the failing comparison reports. Worst channel **173**; the failure
///   message names (79, 56), and recomputing the difference in M3.27 put the
///   same 173 at all 28 pixels of columns 79–80, rows 56–69.
///
/// **The bands cannot move with the interpolation qualifier**, which is why this
/// scene needed no `centroid`/`center` reasoning of its own: 6 and 4 px per texel
/// are even, so u = 3.5 falls at x = 77.0 and v = 3.5 at y = 70.0 — exactly on
/// pixel boundaries. A sample point stays inside its own pixel under either
/// qualifier, so it can neither enter a band nor leave one. `BASELINE.md`'s
/// M3.26 note records the measured half of the same fact: this row is one of the
/// two D17 left "unchanged to the pixel".
const PLACED_SPRITE: &str = "placed_sprite_quadrants_128x128";

/// Golden image for the placed sprite, against which M3.6's batching runs.
///
/// The pixel probes in `sprite_placement.rs` are not replaced by this and do
/// not replace it. They bind four points to an expectation derived from
/// ADR-0004 before anything rendered; this binds every other pixel to a state a
/// human looked at. A probe cannot notice a change it does not sample, and a
/// reference image cannot tell anyone what the right answer was supposed to be.
///
/// **Expected to fail until the maintainer blesses the reference.** Nothing
/// here writes one: the run leaves a `.actual.png` under the target directory
/// and says where. That is the designed outcome of M3.5, not a defect.
#[test]
fn the_placed_sprite_matches_its_golden_reference() {
    let Some(target) = target_or_skip(128, 128) else {
        return;
    };

    // `render_sprites` with a slice of one, not `render_sprite`: the wish is a
    // field of `SpriteInstance`, and `render_sprite` builds its own through
    // `SpriteInstance::whole_texture`, which is `Nearest` by construction. This is the
    // one place the scene asks for `Linear`; see `PLACED_SPRITE` above.
    let rendered = target
        .render_sprites(
            &narvo_testkit::quadrant_texture(8),
            &[SpriteInstance::whole_texture(SpritePlacement {
                x: 16.0,
                y: -8.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 48.0,
                scale_y: 32.0,
            })
            .sampled(SpriteFilter::Linear)],
        )
        .expect("drawing one sprite must succeed once a device exists");

    let references = reference_dir();
    let output = output_dir();
    let golden = Golden::new(&references, &output);

    match golden.verify(PLACED_SPRITE, &rendered) {
        Ok(report) => println!(
            "golden match for \"{PLACED_SPRITE}\": {}",
            report.measured_against(golden.tolerance)
        ),
        Err(error) => panic!("{error}"),
    }
}

/// What [`narvo_render2d::SAMPLE_COUNT`] costs in memory, and that the
/// attachment is really the size the count implies.
///
/// The number a budget is written against, so it is measured rather than
/// remembered. It is also the only assertion in the repository that the
/// multisample attachment exists at all at the size it claims: everything else
/// checks pixels, and a target that had quietly fallen back to one sample would
/// still produce pixels.
#[test]
fn the_multisample_attachment_costs_what_the_sample_count_implies() {
    let Some(target) = target_or_skip(64, 64) else {
        return;
    };

    let single = u64::from(target.width()) * u64::from(target.height()) * 4;
    assert_eq!(
        target.multisample_bytes(),
        single * u64::from(narvo_render2d::SAMPLE_COUNT),
        "a {} x {} Rgba8UnormSrgb target is {single} bytes at one sample",
        target.width(),
        target.height()
    );

    // The figure `docs/perf/BASELINE.md` records, at the size the throughput
    // table already uses. Printed rather than asserted: a budget belongs in the
    // baseline document, and a hard limit here would be a threshold this task
    // was told not to introduce.
    if let Some(full) = target_or_skip(1920, 1080) {
        println!(
            "multisample attachment at 1920 x 1080: {} bytes ({:.1} MiB), \
             on top of {} bytes of single-sample target",
            full.multisample_bytes(),
            full.multisample_bytes() as f64 / (1024.0 * 1024.0),
            u64::from(full.width()) * u64::from(full.height()) * 4
        );
    }
}
