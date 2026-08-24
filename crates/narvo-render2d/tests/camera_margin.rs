//! What a camera costs in rasteriser agreement, measured before any reference
//! image depends on it.
//!
//! # The question
//!
//! Every blessed reference image in this repository has its sprite edges on
//! integer pixel boundaries. `docs/perf/BASELINE.md` records three rasterisers
//! — llvmpipe, WARP and a discrete AMD GPU — rendering the M1 quad
//! byte-identically, and says in the same breath what that does *not* cover:
//! "filtered sampling at non-integer positions, a camera transform that puts
//! vertices between pixel centres, or glyph coverage". M3.12 adds the second of
//! those, so the margin has to be measured in that regime before a fifth
//! reference is blessed in it. A reference CI cannot reproduce is worse than no
//! reference.
//!
//! # What this file does
//!
//! It renders one scene through six cameras and writes each result as a PNG
//! under the golden artifact directory. It asserts almost nothing: it is a
//! measuring instrument, in the shape `the_cost_of_preparing_a_batch_is_recorded`
//! already uses, and the numbers it feeds live in `BASELINE.md`.
//!
//! Two of the six cameras keep every edge on a pixel boundary and are controls.
//! `zoom_2` is the interesting one of those: a zoom that is a whole number does
//! **not** by itself take the geometry off the grid, and saying so needs a
//! measured case rather than an argument.
//!
//! # Comparing two rasterisers
//!
//! Set `NARVO_RASTER_BASELINE` to a directory holding another rasteriser's
//! PNGs, named as this test names them, and every case is compared against it
//! through `Golden` — the same comparison, and the same printed three numbers,
//! that the golden-image tests use. Unset, the test only writes.
//!
//! That is deliberately not a golden reference: nothing here reads or writes
//! `tests/golden/`, and the directory being compared against is one this test
//! produced on another machine, not one a human blessed.

use std::path::{Path, PathBuf};

use narvo_render2d::{
    CameraView, Golden, OffscreenTarget, Pixels, RenderError, SpriteInstance, SpritePlacement,
    TextureRegion, golden_artifact_dir,
};

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// A directory of another rasteriser's PNGs to compare against.
const BASELINE_VAR: &str = "NARVO_RASTER_BASELINE";

/// The render target's edge. Square, so one number.
const TARGET: u32 = 128;

/// The 16 x 16 atlas: four 8 x 8 quadrants, each split into an upper and a
/// lower half of four rows. The fixture shape M3.9 and M3.11 used.
///
/// **Deliberately unpadded.** M3.21 gave the originals a one-texel border;
/// this file did not move with them, because it is a measurement
/// baseline rather than a re-derivation of the scene. Under `Nearest` the two
/// render the same image; under `Linear` they will not.
fn atlas() -> Pixels {
    narvo_testkit::AtlasLayout::UNPADDED.atlas()
}

/// Three overlapping 32 x 32 sprites on a diagonal, one atlas region each.
///
/// Chosen for edges rather than for meaning: three sprites give six vertical and
/// six horizontal edges, and they overlap in a chain — the middle sprite with
/// each of the outer two, which are disjoint from one another — so some of those
/// edges run through another sprite's colour rather than through the background.
/// At every camera in [`CASES`] the whole scene stays inside the target except at
/// `zoom_2`, where an eight-pixel band falls outside each of the four edges.
///
/// | sprite | centre | world extent | region (texels) |
/// | --- | --- | --- | --- |
/// | A | (-20, +20) | x -36..-4, y +4..+36 | top left, (0, 0) 8 x 8 |
/// | B | (0, 0) | x -16..16, y -16..16 | top right, (8, 0) 8 x 8 |
/// | C | (+20, -20) | x 4..36, y -36..-4 | bottom left, (0, 8) 8 x 8 |
fn scene(texture: &Pixels) -> [SpriteInstance; 3] {
    let placed = |x: f32, y: f32| SpritePlacement {
        x,
        y,
        rot_cos: 1.0,
        rot_sin: 0.0,
        scale_x: 32.0,
        scale_y: 32.0,
    };

    [
        SpriteInstance::new(
            placed(-20.0, 20.0),
            TextureRegion::from_texels(0, 0, 8, 8, texture),
        ),
        SpriteInstance::new(
            placed(0.0, 0.0),
            TextureRegion::from_texels(8, 0, 8, 8, texture),
        ),
        SpriteInstance::new(
            placed(20.0, -20.0),
            TextureRegion::from_texels(0, 8, 8, 8, texture),
        ),
    ]
}

/// The six cameras, and what each one does to where the edges land.
///
/// An edge at world `w` lands at image coordinate `X = 64 + (w - cam) * zoom`
/// across and `Y = 64 - (w - cam) * zoom` down on a 128 x 128 target — the signs
/// differ because image rows run against world y (ADR-0004) — so an edge is on a
/// pixel boundary exactly when that value is a whole number. Every sprite
/// coordinate in [`scene`] is a whole number, so the camera alone decides.
///
/// | case | camera | zoom | where a vertical edge lands | on the grid? |
/// | --- | --- | --- | --- | --- |
/// | `identity` | (0, 0) | 1 | `64 + w` | yes — today's regime |
/// | `zoom_2` | (0, 0) | 2 | `64 + 2w` | yes — a whole-number zoom keeps it |
/// | `offset_half_x` | (0.5, 0) | 1 | `63.5 + w`; down, `64 - w` | no — vertical edges on pixel *centres*, horizontal ones still on boundaries |
/// | `offset_half_xy` | (0.5, 0.5) | 1 | `63.5 + w`; down, `64.5 - w` | no — the worst case, centres on both axes |
/// | `zoom_1_5_quarter` | (0.25, 0.25) | 1.5 | `63.625 + 1.5 w` | no — eighths |
/// | `offset_and_zoom` | (0.3, -0.7) | 1.5 | `63.55 + 1.5 w` | no — nothing lands anywhere tidy |
///
/// `offset_half_x` and `offset_half_xy` are the sharpest: an edge exactly on a
/// pixel centre splits that pixel in half, which is the one input where two
/// rasterisers' fill rules have the most room to disagree.
const CASES: [(&str, CameraView); 6] = [
    ("identity", CameraView::IDENTITY),
    ("zoom_2", CameraView::new(0.0, 0.0, 2.0)),
    ("offset_half_x", CameraView::new(0.5, 0.0, 1.0)),
    ("offset_half_xy", CameraView::new(0.5, 0.5, 1.0)),
    ("zoom_1_5_quarter", CameraView::new(0.25, 0.25, 1.5)),
    ("offset_and_zoom", CameraView::new(0.3, -0.7, 1.5)),
];

/// Where this test writes its renders.
fn output_dir() -> PathBuf {
    golden_artifact_dir().join("raster")
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

/// Renders every case, writes it, and — with `NARVO_RASTER_BASELINE` set —
/// prints how far it sits from another rasteriser's run.
#[test]
fn the_off_grid_rasteriser_margin_is_recorded() {
    let Some(target) = target_or_skip(TARGET, TARGET) else {
        return;
    };

    let atlas = atlas();
    let sprites = scene(&atlas);
    let output = output_dir();
    std::fs::create_dir_all(&output).expect("the output directory must be creatable");

    let baseline = std::env::var_os(BASELINE_VAR).map(PathBuf::from);
    match &baseline {
        Some(dir) => println!("comparing against {}", dir.display()),
        None => println!("no {BASELINE_VAR} set: writing only"),
    }

    for (case, camera) in CASES {
        let rendered = target
            .render_sprites_viewed_by(&atlas, &sprites, camera)
            .expect("three sprites are far inside the batch limit");

        let path = output.join(format!("{case}.png"));
        rendered
            .save_png(&path)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        println!(
            "{case}: camera ({}, {}) zoom {} -> {}",
            camera.x,
            camera.y,
            camera.zoom,
            path.display()
        );

        if let Some(reference_dir) = &baseline {
            compare_against(reference_dir, case, &rendered, &output);
        }
    }
}

/// Prints how far `rendered` sits from `reference_dir/<case>.png`.
///
/// Through `Golden`, so the three numbers are the ones `BASELINE.md`'s
/// "Golden-image margin" section already reports and the thresholds are
/// `Tolerance::default()`. A missing file is reported and skipped rather than
/// failing: a machine that has not been handed a baseline yet is not a defect.
fn compare_against(reference_dir: &Path, case: &str, rendered: &Pixels, output: &Path) {
    let golden = Golden::new(reference_dir, output);

    match golden.verify(case, rendered) {
        Ok(report) => println!("  {case}: {}", report.measured_against(golden.tolerance)),
        Err(error) => println!("  {case}: {error}"),
    }
}

/// The scene really does put edges where [`CASES`] says it does.
///
/// Runs without a GPU. The table above is a claim about arithmetic, and the
/// whole measurement is read through it: if `offset_half_x` did not in fact put
/// vertical edges on pixel centres, a byte-identical result would say nothing
/// about the off-grid case. This computes the left edge of every sprite under
/// every camera and asserts which cases are on the grid and which are not.
#[test]
fn the_cases_are_on_and_off_the_grid_where_the_table_says() {
    let atlas = atlas();
    let sprites = scene(&atlas);

    // `X = half + (w - cam) * zoom`, the image coordinate of a world x.
    let image_x = |w: f32, camera: CameraView| {
        f64::from(TARGET) / 2.0 + (f64::from(w) - f64::from(camera.x)) * f64::from(camera.zoom)
    };

    for (case, camera) in CASES {
        let on_grid = matches!(case, "identity" | "zoom_2");
        let mut edges = Vec::new();

        for sprite in &sprites {
            let placement = sprite.placement;
            for edge in [
                placement.x - placement.scale_x / 2.0,
                placement.x + placement.scale_x / 2.0,
            ] {
                edges.push(image_x(edge, camera));
            }
        }

        let whole = edges.iter().all(|x| x.fract() == 0.0);
        println!("{case}: edges {edges:?} -> all whole: {whole}");

        assert_eq!(
            whole,
            on_grid,
            "case {case} was described as {} the grid, but its vertical edges are \
             {edges:?}. Every number this measurement produces is read as a \
             statement about that regime, so the description has to be true of the \
             arithmetic rather than of the intention.",
            if on_grid { "on" } else { "off" }
        );
    }
}

/// The half-pixel cases put edges exactly on pixel centres, not merely off the
/// boundary.
///
/// Runs without a GPU. "Off the grid" covers a fraction of 0.001 as well as one
/// of 0.5, and those are not the same test of a rasteriser. This pins the sharp
/// one.
#[test]
fn the_half_pixel_cases_put_edges_on_pixel_centres() {
    let atlas = atlas();
    let sprites = scene(&atlas);
    let camera = CameraView::new(0.5, 0.0, 1.0);

    for sprite in &sprites {
        let placement = sprite.placement;
        for edge in [
            placement.x - placement.scale_x / 2.0,
            placement.x + placement.scale_x / 2.0,
        ] {
            let x = f64::from(TARGET) / 2.0 + (f64::from(edge) - f64::from(camera.x));
            assert_eq!(
                x.fract(),
                0.5,
                "edge at world {edge} lands at image x {x}, which is not a pixel \
                 centre. `offset_half_x` is the sharpest case of the measurement \
                 only if every edge splits a pixel exactly in half."
            );
        }
    }
}
