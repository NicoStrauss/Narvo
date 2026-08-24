//! `Linear` through the production path: what padding buys, and what a mixed
//! batch does.
//!
//! M3.23's second and third proofs. Neither consults a blessed reference and
//! neither draws a blessed scene — the scenes here are this file's own, for the
//! reason `ProjektPlan.md` §12 gives: no blessed image moves in this task, so
//! the evidence that `Linear` works has to come from somewhere else.
//!
//! Everything below goes through `OffscreenTarget::render_sprites`, which is the
//! production entry point: `batch_runs` cuts the order into runs, each run binds
//! its own sampler. Nothing here builds a pipeline of its own — that is what the
//! measurement files do, and comparing against one of those is
//! `linear_blessed_margin.rs`'s `production_at_linear_is_byte_identical_to_this_file_s_linear`.

use narvo_render2d::{
    OffscreenTarget, Pixels, RenderError, SpriteFilter, SpriteInstance, SpritePlacement,
};
use narvo_testkit::AtlasLayout;

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The target edge these scenes draw into.
const TARGET: u32 = 64;

/// This file's atlas: the shared padded one (D16), 20 x 20.
const LAYOUT: AtlasLayout = AtlasLayout::PADDED;

fn target_or_skip() -> Option<OffscreenTarget> {
    match OffscreenTarget::new(TARGET, TARGET) {
        Ok(target) => {
            println!("adapter in use: {}", target.adapter_summary());
            Some(target)
        }
        Err(error @ RenderError::NoAdapter { .. }) => {
            assert!(
                std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                "{REQUIRE_GPU_VAR} is set, so a missing adapter is a failure: {error}"
            );
            println!("{SKIP_MARKER}: {error}");
            None
        }
        Err(other) => panic!("the offscreen target could not be created: {other}"),
    }
}

/// The one sprite of the padding scene: cell (1, 0) — green over dark green —
/// magnified four output pixels to the texel.
///
/// 32 x 32 output pixels over an 8 x 8 content region, centred, so it covers
/// image x 16..47 and y 16..47. Four pixels to the texel is what gives `Linear`
/// something to blend and puts the blend where the derivation below expects it.
fn magnified_region(atlas: &Pixels, filter: SpriteFilter) -> Vec<SpriteInstance> {
    vec![
        SpriteInstance::new(
            SpritePlacement {
                x: 0.0,
                y: 0.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 32.0,
                scale_y: 32.0,
            },
            LAYOUT.region(1, 0, atlas),
        )
        .sampled(filter),
    ]
}

/// **The second proof: the border does its job through the production path, and
/// `Linear` is demonstrably running.**
///
/// Two probes on one image, and they answer different questions.
///
/// # The derivation, before anything rendered
///
/// The region is 8 texels over 32 output pixels, so four pixels to the texel.
/// For sprite row `j` (image row `16 + j`) the sampled coordinate is
/// `v_texel = 1 + (j + 0.5) / 4`, and bilinear blends the two texels whose
/// centres — at `k + 0.5` — bracket it.
///
/// **Probe A, the region's left edge.** Image column 16 samples
/// `u_texel = 11 + 0.5 / 4 = 11.125`, which sits left of texel 11's centre at
/// 11.5, so `Linear` blends texel 11 with texel **10** at weight 0.375. Texel 10
/// is the cell's border column, and the border is a copy of content column 0 —
/// so both partners are the same green and the column comes out **pure**. In the
/// unpadded 16 x 16 atlas the same column would have sampled `u_texel = 8.125`
/// and blended texels 7 and 8 — and texel 7 is the *left* quadrant's last
/// column, red. (The two atlases do not share a coordinate frame, so the
/// counterfactual has to be named in its own: it is not texel 10.) That is what
/// padding buys, measured here through production rather than through a
/// measurement pipeline.
///
/// **Probe B, the interior half-boundary.** The region's upper half (content
/// rows 0-3, green) meets its lower half (rows 4-7, dark green) at
/// `v_texel = 5`. Sprite row 15 samples `v_texel = 4.875`, between texel 4's
/// centre (4.5) and texel 5's (5.5), so `Linear` blends them with weight 0.375
/// on texel 5. Under `Nearest` the same row floors to texel 4 and is pure green.
/// **A blend here is the proof that the second sampler is bound**, and it is a
/// place no blessed image looks.
///
/// The predicted value distinguishes two hypotheses, which is why it is written
/// out. Blending in **linear** light — the atlas being an sRGB texture, decoded
/// on sample and re-encoded on write — gives
/// `0.625 * 1.0 + 0.375 * 0.216 = 0.706`, encoded back to **219**. Blending in
/// encoded space would give `0.625 * 255 + 0.375 * 128 = 207`. The test asserts
/// the weaker, robust property (strictly between the two colours) and prints the
/// value, so the report can say which hypothesis the hardware took.
#[test]
fn padding_keeps_a_region_edge_pure_under_linear_while_the_interior_blends() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let atlas = LAYOUT.atlas();
    // The fixture is only worth probing if its border is right.
    LAYOUT.assert_every_region_is_padded(&atlas);

    let green = [0, 255, 0, 255];
    let dark_green = [0, 128, 0, 255];

    let nearest = target
        .render_sprites(&atlas, &magnified_region(&atlas, SpriteFilter::Nearest))
        .expect("one sprite is inside the batch limit");
    let linear = target
        .render_sprites(&atlas, &magnified_region(&atlas, SpriteFilter::Linear))
        .expect("one sprite is inside the batch limit");

    // Probe A: the region's leftmost column, in the upper half.
    let edge_nearest = nearest.pixel(16, 20).expect("inside the target");
    let edge_linear = linear.pixel(16, 20).expect("inside the target");
    println!("edge column: Nearest {edge_nearest:?}, Linear {edge_linear:?}");
    assert_eq!(
        edge_nearest, green,
        "the control: under `Nearest` the region's first column is its own colour"
    );
    assert_eq!(
        edge_linear, green,
        "under `Linear` the region's first column blended with something that is \
         not its own colour. The border is what makes the blend partner a copy \
         of column 0, so either the border is wrong or the region points at it."
    );

    // Probe B: the interior boundary between the two halves.
    let inner_nearest = nearest.pixel(30, 31).expect("inside the target");
    let inner_linear = linear.pixel(30, 31).expect("inside the target");
    println!("interior boundary: Nearest {inner_nearest:?}, Linear {inner_linear:?}");
    assert_eq!(
        inner_nearest, green,
        "the control: under `Nearest` row 31 floors to the upper half"
    );
    assert!(
        inner_linear[1] > dark_green[1] && inner_linear[1] < green[1],
        "row 31 must be a blend of the two halves under `Linear`, strictly \
         between {} and {}. It reads {:?} — which means the batch was drawn with \
         the `Nearest` sampler.",
        dark_green[1],
        green[1],
        inner_linear
    );
    assert_eq!(
        inner_linear[0], 0,
        "nothing red belongs anywhere in this scene: {inner_linear:?}"
    );
}

/// **The third proof: the first real mixed batch.**
///
/// Two sprites that overlap, wanting different samplers, in drawing order. The
/// decomposition cuts between them, so this is two runs and two draw calls with
/// two different samplers bound — the first time that has happened in the
/// production path.
///
/// The property under test is D15's, and it is the one M3.19 showed a naive two
/// draw calls would break: **the visible order is the sequence order across the
/// run boundary.** The second sprite is drawn second and owns the overlap,
/// whichever sampler each of them asked for.
///
/// Predicted before rendering: at the overlap the image shows the *front*
/// sprite's region — cell (0, 1), blue — and not the back one's green.
///
/// The order probe is deliberately filter-blind, and the reason is not that it
/// sits in the middle of a texel: at four pixels to the texel no sample ever
/// lands on a texel centre, and this one sits the maximum distance from one, so
/// `Linear` is blending four taps there. It agrees with `Nearest` because all
/// four taps carry the same colour — the shared fixture's colour does not vary
/// with x at all, and both sampled rows are in the same half.
///
/// **So the order probe cannot see a sampler, which is why this test also
/// carries two probes that can.** Without them, an implementation that bound run
/// 0's sampler for every run would pass — and no golden image would catch it
/// either, because every blessed scene is `Nearest`.
#[test]
fn a_mixed_batch_keeps_the_drawing_order_across_the_run_boundary() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let atlas = LAYOUT.atlas();
    let placed = |x: f32| SpritePlacement {
        x,
        y: 0.0,
        rot_cos: 1.0,
        rot_sin: 0.0,
        scale_x: 32.0,
        scale_y: 32.0,
    };

    // Back: cell (1, 0), green, `Nearest`. Front: cell (0, 1), blue, `Linear`.
    // They overlap in image columns 24..39.
    let back = SpriteInstance::new(placed(-8.0), LAYOUT.region(1, 0, &atlas));
    let front =
        SpriteInstance::new(placed(8.0), LAYOUT.region(0, 1, &atlas)).sampled(SpriteFilter::Linear);

    let rendered = target
        .render_sprites(&atlas, &[back, front])
        .expect("two sprites are inside the batch limit");

    // A column inside the overlap, and a row in each sprite's upper half.
    let overlap = rendered.pixel(32, 24).expect("inside the target");
    println!("overlap pixel: {overlap:?}");
    assert_eq!(
        overlap,
        [0, 0, 255, 255],
        "the sprite drawn second must own the overlap. Green here means the two \
         draw calls landed in the wrong order, which is exactly what D15 is about."
    );

    // And the back sprite is still visible where the front one does not cover it.
    let back_only = rendered.pixel(16, 24).expect("inside the target");
    assert_eq!(
        back_only,
        [0, 255, 0, 255],
        "the first run must still have drawn: {back_only:?}"
    );

    // **Each run got its own sampler.** Both sprites are 32 pixels tall from
    // image row 16 over an 8-texel region, so each has its upper/lower boundary
    // at row 31. Read one column that only the back sprite covers and one that
    // only the front one covers.
    let back_boundary = rendered.pixel(16, 31).expect("inside the target");
    let front_boundary = rendered.pixel(48, 31).expect("inside the target");
    println!("back (Nearest) {back_boundary:?}, front (Linear) {front_boundary:?}");
    assert_eq!(
        back_boundary,
        [0, 255, 0, 255],
        "the back run asked for `Nearest`, so its half-boundary must be sharp"
    );
    assert!(
        front_boundary[2] > 128 && front_boundary[2] < 255,
        "the front run asked for `Linear`, so its half-boundary must blend, \
         strictly between 128 and 255. It reads {front_boundary:?}, which means \
         both runs were drawn with one sampler."
    );

    // The order does not depend on which of the two wanted which sampler.
    let swapped = target
        .render_sprites(
            &atlas,
            &[
                SpriteInstance::new(placed(-8.0), LAYOUT.region(1, 0, &atlas))
                    .sampled(SpriteFilter::Linear),
                SpriteInstance::new(placed(8.0), LAYOUT.region(0, 1, &atlas)),
            ],
        )
        .expect("two sprites are inside the batch limit");
    assert_eq!(
        swapped.pixel(32, 24).expect("inside the target"),
        [0, 0, 255, 255],
        "swapping which run wants which sampler must not change who is in front"
    );
}

/// Every `SpriteFilter` has its own sampler slot, and they are not the same one.
///
/// GPU-free in spirit but not in fact — it needs a device to have two samplers
/// at all. What it pins is the thing an index-based table can get wrong silently:
/// that the two filters do not both resolve to the same sampler. Two renders of
/// a magnified texture that differ is the observable form of that.
#[test]
fn the_two_filters_are_not_the_same_sampler() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let atlas = LAYOUT.atlas();
    let nearest = target
        .render_sprites(&atlas, &magnified_region(&atlas, SpriteFilter::Nearest))
        .expect("one sprite is inside the batch limit");
    let linear = target
        .render_sprites(&atlas, &magnified_region(&atlas, SpriteFilter::Linear))
        .expect("one sprite is inside the batch limit");

    assert_ne!(
        nearest.rgba(),
        linear.rgba(),
        "the two filters produced the same image, so both runs bound one sampler"
    );
}
