//! Three sprites seen through a camera that is neither at the origin nor at
//! zoom 1, and the fifth reference image.
//!
//! `texture_region.rs` pins what a sprite shows and `sprite_placement.rs` pins
//! where it lands; this pins **where the view is**. It is the first scene in the
//! repository whose sprite edges do not all fall on pixel boundaries, which is
//! the regime `docs/perf/BASELINE.md` measured for the first time in M3.12 —
//! three rasterisers, six cameras, byte-identical throughout. Without that
//! measurement this file would be asking a human to bless an image CI might not
//! reproduce.
//!
//! # The camera
//!
//! `(6.0, -2.0)` at `zoom = 1.5`. Not a round position and not a round zoom, on
//! purpose: a camera at the origin would leave the scene where a fixed
//! projection put it, and a whole-number zoom keeps every edge on the grid
//! (`BASELINE.md`'s `zoom_2` case measures exactly that).
//!
//! An edge at world `w` lands at image coordinate `X = 64 + (w - 6) * 1.5`
//! across and `Y = 64 - (w + 2) * 1.5` down, so a pixel `(px, py)` samples the
//! world point `(6 + (px - 63.5) / 1.5, -2 + (63.5 - py) / 1.5)`.
//!
//! | sprite | centre | world extent | region (texels) | covers pixels |
//! | --- | --- | --- | --- | --- |
//! | A | (-12, +18) | x -28..4, y 2..34 | top left, (1, 1) 8 x 8 | px 13-60, py 10-57 |
//! | B | (+14.5, +18) | x -1.5..30.5, y 2..34 | top right, (11, 1) 8 x 8 | px 53-100, py 10-57 |
//! | C | (+6, -8) | x -10..22, y -24..8 | bottom left, (1, 11) 8 x 8 | px 40-87, py 49-96 |
//!
//! **B's x is a half unit**, so its vertical edges land at `X = 52.75` and
//! `X = 100.75`. It is the only blessed reference in the repository with an edge
//! off the pixel grid — it became the first when `f03df1f` blessed it, which is
//! why `BASELINE.md` had to measure the regime before this file could ask.
//! Everything else in the table is a whole number, which is what makes the two
//! probes either side of `52.75` a statement about a quarter of a pixel rather
//! than about arithmetic in general.
//!
//! **That quarter is now visible in the image**, not only inferable from a pair
//! of probes: at [`narvo_render2d::SAMPLE_COUNT`] samples, exactly one of pixel
//! 52's four samples lies past `52.75`, so the column shows a quarter of B over
//! three quarters of A. The reference that was blessed before MSAA does not have
//! it and has to be blessed again; column 100 changes for the same reason, at
//! three quarters instead of one.
//!
//! Draw order is A, B, C, in slice order; there is no depth buffer, so C is in
//! front of both and B is in front of A.

use std::path::{Path, PathBuf};

use narvo_render2d::{
    CameraView, Golden, OffscreenTarget, Pixels, RenderError, SpriteFilter, SpriteInstance,
    SpritePlacement, TextureRegion, golden_artifact_dir,
};

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The blessed name of this scene's reference image.
const SCENE: &str = "camera_regions_128x128";

/// The render target's edge. Square, so one number.
const TARGET: u32 = 128;

/// This file's atlas layout: four 8 x 8 regions, each with a one-texel
/// border of duplicated edge texels (D13, shared per ADR-0016).
const LAYOUT: narvo_testkit::AtlasLayout = narvo_testkit::AtlasLayout::PADDED;

/// The view this scene is drawn through.
const CAMERA: CameraView = CameraView::new(6.0, -2.0, 1.5);

/// The atlas's eight colours, upper then lower half of each quadrant.
const A_UPPER: [u8; 4] = narvo_testkit::TOP_LEFT_UPPER;
const A_LOWER: [u8; 4] = narvo_testkit::TOP_LEFT_LOWER;
const B_UPPER: [u8; 4] = narvo_testkit::TOP_RIGHT_UPPER;
const B_LOWER: [u8; 4] = narvo_testkit::TOP_RIGHT_LOWER;
const C_UPPER: [u8; 4] = narvo_testkit::BOTTOM_LEFT_UPPER;
const C_LOWER: [u8; 4] = narvo_testkit::BOTTOM_LEFT_LOWER;
const UNUSED_UPPER: [u8; 4] = narvo_testkit::BOTTOM_RIGHT_UPPER;
const UNUSED_LOWER: [u8; 4] = narvo_testkit::BOTTOM_RIGHT_LOWER;

/// What the target is cleared to, and therefore what "no sprite" looks like.
const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

/// Pixel 52 of row 30: a quarter of B's green over three quarters of A's red.
///
/// **Derived, not read off a render.** B's left edge is at `X = 52.75`. Of the
/// four standard sample positions in a pixel, the x offsets are `0.125`,
/// `0.375`, `0.625` and `0.875`, so exactly one of them — `52.875` — is past the
/// edge. A covers all four. The resolve averages in *linear* light, because the
/// target is `Rgba8UnormSrgb`:
///
/// - red: `0.75` linear, and `1.055 * 0.75.powf(1.0 / 2.4) - 0.055` is
///   `0.8808`, which is `225` of `255`;
/// - green: `0.25` linear, `1.055 * 0.25.powf(1.0 / 2.4) - 0.055` is `0.5371`,
///   which is `137`.
///
/// `the_quarter_covered_pixel_is_the_srgb_average_of_three_reds_and_one_green`
/// recomputes both numbers from the transfer function rather than trusting this
/// comment.
///
/// Before [`narvo_render2d::SAMPLE_COUNT`] this probe expected `A_UPPER`: with
/// one sample per pixel the centre at `52.5` falls short of `52.75` and the
/// pixel is purely A's. The probe therefore says strictly more than it used to.
/// It used to bracket the edge together with its neighbour at 53; now it
/// **measures the coverage**, and a quarter-pixel shift in either direction
/// changes its value rather than leaving it alone.
const B_EDGE_QUARTER_OVER_A: [u8; 4] = [225, 137, 0, 255];

/// The 20 x 20 padded atlas: four 8 x 8 content quadrants, each split into an
/// upper and a lower half of four rows, and each carrying a one-texel border of
/// its own duplicated edge texels (M3.21, D13). The fixture shape of M3.9 and
/// M3.11, grown by the border.
///
/// The colour is chosen from the *content* coordinate and the clamp is what
/// duplicates.
///
/// **Until M3.29 the border was unreachable from this scene** — `Nearest` never
/// reads one, which is why this file's blessed reference did not move when M3.21
/// gave the atlas its border, and why
/// `every_region_of_the_atlas_carries_its_border` was the only thing saying the
/// border was nevertheless right. Since M3.29 the scene draws at `Linear`
/// ([`scene`]), and bilinear reaches one texel past the sample point, so the
/// border is read at every region edge and at every partly covered silhouette
/// pixel, where `center` puts the sample outside the sprite. It is what keeps
/// those reads honest: the texel past a region is a copy of the region's own
/// edge texel, so the blessed image does not bleed.
///
/// **Which half of the border the guard actually pins is worth being exact
/// about**, because the two halves are not alike here. `assert_every_region_is_padded`'s
/// own doc says it: this atlas's colour does not vary with x inside a cell, so
/// every left- and right-border texel equals its source whatever the generator
/// does, and what the guard pins is the **top and bottom** border rows. The
/// reads this scene's off-grid columns make are into the **left and right**
/// border — texel 10 at column 52 and texel 19 at column 100 — which the guard
/// cannot distinguish from content. Those two columns are witnessed by the
/// blessed image and by nothing else.
///
/// One region per sprite, which §9.2 of `ProjektPlan.md` requires of any scene a
/// human has to read: three sprites sharing one texture is what made M3.10's
/// reference unreadable, because two of them meeting in the same colour left no
/// visible boundary. **The bottom-right quadrant is sampled by nothing**, so
/// yellow and olive may not appear anywhere in a correct frame.
fn atlas() -> Pixels {
    narvo_testkit::AtlasLayout::PADDED.atlas()
}

/// The content region of the cell at `(cell_x, cell_y)`.
///
/// The region points at the **content**, never at the border: padding surrounds
/// a region's coordinates rather than moving them.
fn region(cell_x: u32, cell_y: u32, texture: &Pixels) -> TextureRegion {
    LAYOUT.region(cell_x, cell_y, texture)
}

/// The three sprites of this file's documentation, in draw order.
///
/// # `Linear` since M3.29 — the fourth and last conversion
///
/// D13's round closes here (order in `ProjektPlan.md` §12). **One place:** this
/// function is the only construction of the scene that feeds the blessed
/// reference, and the wish is a field of the `SpriteInstance` it returns. There is no
/// world in this story and there cannot be — `narvo-render2d` has no
/// dependency on `narvo-ecs`, in `[dependencies]` or `[dev-dependencies]`, so
/// the question M3.28 had to answer for `layer_order_regions_128x128` (component
/// on the entities, or field on the sprite?) has exactly one available answer
/// here.
///
/// Three other files transcribe the same camera and the same three placements by
/// hand — `msaa_blessed_margin.rs:267`, `linear_blessed_margin.rs:251` and
/// `draw_order_margin.rs:213` — and none of them moves, **for two different
/// reasons that are worth keeping apart.** `linear_blessed_margin.rs` names a
/// wish on every sprite it renders (`.sampled(wish)`, :775) and is immune by
/// construction. The other two build theirs with `SpriteInstance::new`
/// (`msaa_blessed_margin.rs:743`, `draw_order_margin.rs:411`), which takes the
/// `Nearest` **default** — they are deliberately-`Nearest` replicas in the M3.24
/// sense, and what protects them is that this change is at a call site and not
/// at that default. Change `SpriteInstance::new` instead and both would move silently.
///
/// **This is the scene D17 was argued over**, and the one that had to go last:
/// sprite B's vertical edges sit at `X = 52.75` and `X = 100.75`, the only
/// off-grid edges in any blessed image, so under MSAA it has partly covered
/// silhouette pixels **over atlas content** — the configuration where `center`
/// extrapolates a fragment's uv outside its own sprite. The padded atlas is what
/// makes that safe: the texel one step past a region is a copy of the region's
/// own edge texel (M3.21), so the extrapolated read returns the same colour.
///
/// **What changes, in three classes, and the third one is the interesting one.**
///
/// - **Interior blend bands, 804 pixels.** At 6 px per texel the blend spans the
///   rows whose sample `v` falls between texel centres 3.5 and 4.5 — **rows
///   31–36** for A and B, **rows 70–75** for C. Those band edges land at
///   continuous y = 31.0, 37.0, 70.0 and 76.0: **integers**, so a sample point
///   cannot be moved into a band or out of one, whichever qualifier is in force.
/// - **Partly covered silhouette pixels, 12 of them.** This scene has **87**,
///   which is not a new number — they are exactly the 87 pixels M3.15 blessed
///   when MSAA arrived (column 52 rows 10–48, column 100 rows 10–57). **Only the
///   12 that a band crosses change.** The other 75 keep their values *exactly*,
///   because both coverage partners sample content that is uniform in the row,
///   and a bilinear blend of equal taps is that same colour.
/// - **Nothing else.** No horizontal blend exists anywhere: the atlas colour does
///   not vary with x, and at a region's left and right edge the blend partner is
///   the duplicated border.
///
/// So `[225, 137, 0]` at (52, 30) — the orange column M3.15 gave the maintainer
/// as a mnemonic, three quarters of A's red against one quarter of B's green —
/// **does not move**. Six rows lower, at (52, 32), the same column reads
/// `[204, 124, 0]`, because there the two partners are themselves on their ramps.
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
        SpriteInstance::new(placed(-12.0, 18.0), region(0, 0, texture))
            .sampled(SpriteFilter::Linear),
        SpriteInstance::new(placed(14.5, 18.0), region(1, 0, texture))
            .sampled(SpriteFilter::Linear),
        SpriteInstance::new(placed(6.0, -8.0), region(0, 1, texture)).sampled(SpriteFilter::Linear),
    ]
}

/// The probes, **derived before anything rendered**.
///
/// A sprite covering image pixels `X0..X1` across and `Y0..Y1` down samples,
/// at pixel `(px, py)`, the atlas texel
/// `(rx + 8 (px + 0.5 - X0) / (X1 - X0), ry + 8 (py + 0.5 - Y0) / (Y1 - Y0))`,
/// rounded down. The extents come from the
/// table at the top of this file; each sprite is 48 pixels across at this zoom.
/// `(rx, ry)` is the region's **content** origin, which the border moved from
/// `(0, 0)` / `(8, 0)` / `(0, 8)` to `(1, 1)` / `(11, 1)` / `(1, 11)` — a whole
/// number of texels, so every texel below moved by an integer and no fraction
/// changed.
///
/// **Rounding down was `Nearest`'s rule, and the scene has drawn at `Linear`
/// since M3.29 — the probes survive anyway, and not by luck.** Every one of the
/// ten sits where all four bilinear taps carry one colour: the atlas colour does
/// not vary with x inside a cell, and no probe's row falls in a blend band (the
/// bands are rows 31–36 and 70–75; the probe rows are 4, 20, 30, 45, 50, 55, 60
/// and 80). So flooring and blending give the same answer at these ten points,
/// and the table below is unchanged. A probe one row further into a band would
/// have needed a mix value.
///
/// | probe | pixel | covered by | in front | texel | expected |
/// | --- | --- | --- | --- | --- | --- |
/// | background | (4, 4) | nothing | — | — | black |
/// | A upper | (20, 20) | A | A | (2.25, 2.75) | red |
/// | A lower | (20, 50) | A | A | (2.25, 7.75) | dark red |
/// | B upper | (90, 20) | B | B | (17.29, 2.75) | green |
/// | B lower | (90, 45) | B | B | (17.29, 6.92) | dark green |
/// | C upper | (60, 60) | C | C | (4.42, 12.92) | blue |
/// | C lower | (60, 80) | C | C | (4.42, 16.25) | dark blue |
/// | at B's edge | (52, 30) | A fully, B by one sample of four | both | A (7.58, 4.42), B (11.02, 4.42) | a quarter of B's green over A's red |
/// | right of B's edge | (53, 30) | A, B | B | (11.13, 4.42) | green |
/// | C over A | (45, 55) | A, C | C | (1.92, 12.08) | blue |
///
/// **The pair (52, 30) and (53, 30) is the point of the scene.** B's left edge
/// sits at `X = 52.75`. Pixel 52's centre is at 52.5, a quarter of a pixel to
/// the left of it, and pixel 53's centre at 53.5, three quarters to the right —
/// so the pair says where a *non-integer* edge fell, to a quarter of a pixel.
/// Under MSAA pixel 52's colour no longer comes from that centre — the fragment
/// shader still runs there, but which samples the resolve counts is decided at
/// the four sample positions, and only `52.875` is past the edge. The quarter
/// is the same quarter; it is now the pixel's value rather than the reason its
/// value is A's.
/// **Every camera error below that moves the geometry across leaves at least one
/// of them changed**; the fourth, zoom on x only, moves nothing across and so
/// leaves the pair exactly as it is, which is why the axis question is settled by
/// a GPU-free test rather than by this pair.
///
/// The tightest sample of the ten is (53, 30) at texel x `11.13`, an eighth of a
/// texel inside B's region, which begins at 11. With `Nearest` that was
/// decisive; M3.12's report worked out what `Linear` would do with it. **Since
/// M3.29 it is no longer a forecast**: the scene draws at `Linear`, that sample
/// blends absolute texels 10 and 11, and texel 10 is B's own left border column
/// rather than A's region — which is why this probe still reads `B_UPPER`
/// exactly. It is the padding, on the read the M3.12 report singled out as the
/// tightest in the table.
///
/// # Which camera error each probe catches
///
/// Correct: `X = 64 + (w - 6) * 1.5`, `Y = 64 - (w + 2) * 1.5`.
///
/// | error | what it does to the image | first probe that reddens |
/// | --- | --- | --- |
/// | camera position ignored | `X = 64 + 1.5 w`, `Y = 64 - 1.5 w`: 9 px right, 3 px down | (20, 20) → black |
/// | camera position's sign flipped | `X = 64 + (w + 6) * 1.5`: 18 px right, 6 px down | (20, 20) → black |
/// | zoom ignored | every sprite 32 px instead of 48, pulled toward the camera | (20, 20) → black |
/// | zoom on x only | 48 px across, 32 px down | (20, 20) → black |
///
/// Every one of the four moves (20, 20) off sprite A, so the first probe catches
/// all four and identifies none. What the ten probes can and cannot separate,
/// recomputed rather than assumed:
///
/// - **(90, 20) is the only probe that separates a shift from a resize.** It
///   stays green under both position errors — 9 px right and 3 down, or 18 px
///   right and 6 down, since B is wide enough to keep it either way — and turns
///   black under both zoom errors.
/// - **(20, 50), (90, 45), (53, 30) and (45, 55) separate "zoom on x only" from
///   the other three**, reddening under those three and staying put under it.
/// - **The two position errors are indistinguishable to these ten probes.**
///   Ignoring the camera and flipping its sign produce the same colour at all
///   ten, because 9 px right / 3 down and 18 px right / 6 down happen to land
///   every probe in the same place. Telling them apart would need a probe this
///   set does not have, and M3.12 reports that rather than adding one.
/// - **(52, 30) reddens under three of the four**, and did so under none of
///   them before MSAA. Its expectation is a blend that only a quarter coverage
///   at `52.75` produces, and three of the errors move B's left edge away from
///   that: to `61.75` with the position ignored, to `70.75` with its sign
///   flipped, to `56.5` with the zoom ignored. In each case pixel 52 is inside A
///   alone and comes back pure red. Under "zoom on x only" the x geometry is
///   untouched and row 30 stays in both upper halves, so the probe holds — which
///   puts it in the same group as (20, 50), (90, 45), (53, 30) and (45, 55).
///   Recomputed for M3.15 from the four substitute projections, not carried over.
/// - **(4, 4), (60, 60) and (60, 80) redden under none of the four.** They are
///   here for other jobs — the background and C's identity — and contribute
///   nothing to this table.
///
/// A GPU-free test carries what no probe can: that zoom reaches both axes at
/// all — `a_camera_translates_and_scales_and_does_both_on_both_axes` in
/// `sprite.rs` asserts the two axes separately, where an image can only show the
/// composite.
const PROBES: [(u32, u32, [u8; 4], &str); 10] = [
    (4, 4, BACKGROUND, "background, outside every sprite"),
    (20, 20, A_UPPER, "A alone, upper half of its region"),
    (20, 50, A_LOWER, "A alone, lower half of its region"),
    (90, 20, B_UPPER, "B alone, upper half of its region"),
    (90, 45, B_LOWER, "B alone, lower half of its region"),
    (60, 60, C_UPPER, "C alone, upper half of its region"),
    (60, 80, C_LOWER, "C alone, lower half of its region"),
    (
        52,
        30,
        B_EDGE_QUARTER_OVER_A,
        "the quarter of B's edge pixel at X = 52.75, over A",
    ),
    (53, 30, B_UPPER, "first column right of it, B over A"),
    (45, 55, C_UPPER, "C over A"),
];

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

/// Renders the scene through [`CAMERA`].
fn render(target: &OffscreenTarget) -> Pixels {
    let atlas = atlas();
    target
        .render_sprites_viewed_by(&atlas, &scene(&atlas), CAMERA)
        .expect("three sprites are far inside the batch limit")
}

/// The scene is the fixture the probe derivation assumed.
///
/// Runs without a GPU. Two claims the table leans on: the zoom is not 1, and
/// exactly two edges — sprite B's vertical ones — miss the pixel grid, by the
/// three quarters of a pixel the probe pair either side of `52.75` measures.
///
/// That the camera is also away from the **origin** is deliberately *not*
/// asserted here, and saying so is the honest half: `assert_ne!(CAMERA,
/// CameraView::IDENTITY)` would pass for a camera at the origin with a non-unit
/// zoom. What pins the position is the probe table, where every expectation was
/// derived from `(6.0, -2.0)`.
#[test]
fn the_scene_is_off_the_grid_in_exactly_one_place() {
    let atlas = atlas();
    let sprites = scene(&atlas);

    assert_ne!(CAMERA, CameraView::IDENTITY);
    assert_ne!(
        CAMERA.zoom, 1.0,
        "a scene at zoom 1 would not exercise what M3.12 introduces"
    );

    let image_x = |w: f32| {
        f64::from(TARGET) / 2.0 + (f64::from(w) - f64::from(CAMERA.x)) * f64::from(CAMERA.zoom)
    };
    let image_y = |w: f32| {
        f64::from(TARGET) / 2.0 - (f64::from(w) - f64::from(CAMERA.y)) * f64::from(CAMERA.zoom)
    };

    let mut off_grid = Vec::new();
    for (index, sprite) in sprites.iter().enumerate() {
        let p = sprite.placement;
        for (axis, edge) in [
            ("x", image_x(p.x - p.scale_x / 2.0)),
            ("x", image_x(p.x + p.scale_x / 2.0)),
            ("y", image_y(p.y - p.scale_y / 2.0)),
            ("y", image_y(p.y + p.scale_y / 2.0)),
        ] {
            println!("sprite {index} {axis} edge -> {edge}");
            if edge.fract() != 0.0 {
                off_grid.push((index, axis, edge));
            }
        }
    }

    assert_eq!(
        off_grid.len(),
        2,
        "exactly two edges — sprite B's two vertical ones — must miss the pixel \
         grid, and they must miss it by the quarter pixel the probe pair either \
         side of 52.75 measures. Got {off_grid:?}"
    );
    for (index, axis, edge) in off_grid {
        assert_eq!(index, 1, "only sprite B is off the grid");
        assert_eq!(axis, "x");
        assert_eq!(
            edge.fract(),
            0.75,
            "B's edge at {edge} no longer sits three quarters of a pixel past a \
             boundary, so the probes at 52 and 53 stop saying what they were \
             derived to say"
        );
    }
}

/// Each sprite samples a region of its own, and none samples the whole atlas.
///
/// Runs without a GPU.
#[test]
fn the_scene_gives_each_sprite_its_own_region_of_the_atlas() {
    let atlas = atlas();
    let sprites = scene(&atlas);

    for (index, sprite) in sprites.iter().enumerate() {
        assert_ne!(
            sprite.region.uv_bounds(),
            TextureRegion::WHOLE_TEXTURE.uv_bounds(),
            "sprite {index} samples the whole atlas, so it would show every other \
             sprite's colours as well as its own"
        );
    }

    for (i, a) in sprites.iter().enumerate() {
        for (j, b) in sprites.iter().enumerate().skip(i + 1) {
            assert_ne!(
                a.region.uv_bounds(),
                b.region.uv_bounds(),
                "sprites {i} and {j} share a region, so a swap between them would \
                 be invisible"
            );
        }
    }
}

#[test]
fn the_camera_puts_every_probe_where_the_derivation_says() {
    let Some(target) = target_or_skip(TARGET, TARGET) else {
        return;
    };

    let rendered = render(&target);

    for (x, y, expected, why) in PROBES {
        let actual = rendered
            .pixel(x, y)
            .unwrap_or_else(|| panic!("({x}, {y}) is outside the {TARGET} x {TARGET} target"));

        println!("({x}, {y}) {why}: {actual:?}");
        assert_eq!(
            actual, expected,
            "pixel ({x}, {y}), {why}: expected {expected:?}, rendered {actual:?}. \
             The expectation was derived from the camera, the projection and the \
             regions before this ran. A probe that has moved means the camera's \
             position or its zoom is being ignored, doubled, or applied to one \
             axis only."
        );
    }
}

/// No pixel of the frame comes from the quadrant no sprite asked for.
#[test]
fn no_pixel_comes_from_the_quadrant_no_sprite_uses() {
    let Some(target) = target_or_skip(TARGET, TARGET) else {
        return;
    };

    let rendered = render(&target);

    for y in 0..rendered.height() {
        for x in 0..rendered.width() {
            let pixel = rendered
                .pixel(x, y)
                .expect("x and y come from the image's own dimensions");
            assert!(
                pixel != UNUSED_UPPER && pixel != UNUSED_LOWER,
                "pixel ({x}, {y}) is {pixel:?}, a colour from the atlas's \
                 bottom-right quadrant. No sprite in this scene uses that \
                 quadrant, so it can only appear if a sprite sampled the whole \
                 atlas instead of its region."
            );
        }
    }
}

/// The blessed reference for the camera scene.
///
/// **Expected to fail until the maintainer blesses the reference again.** A
/// different state from M3.5's, M3.9's, M3.10's and M3.11's, which were waiting
/// for a first blessing: this one has been blessed twice already — in `f03df1f`,
/// and again after MSAA moved it (M3.15). Nothing here writes a reference either
/// way.
///
/// **It is red for the third time, and for a new reason: `Linear` (D13, M3.29).**
/// 816 of 16 384 pixels, worst channel 67 — two orders of magnitude more than
/// M3.15's 87, and in a different place. M3.15's 87 were the two off-grid
/// silhouette columns; these 816 are the interior blend bands, rows 31–36 and
/// 70–75, plus the 12 pixels where those bands cross the two silhouette columns.
/// **The other 75 of M3.15's 87 do not move at all** — including the orange
/// column at (52, 30), which [`PROBES`] still asserts as `[225, 137, 0]`.
///
/// M3.15's 87 remain the reason the geometry could be blessed at all, and that
/// argument is untouched: `BASELINE.md`'s off-grid margin section records
/// llvmpipe, WARP and an AMD discrete GPU, six cameras including two that put
/// every edge on a pixel centre, byte-identical throughout. Coverage is
/// geometry; `Linear` changes what a covered sample reads, not which samples are
/// covered.
#[test]
fn the_camera_scene_matches_its_golden_reference() {
    let Some(target) = target_or_skip(TARGET, TARGET) else {
        return;
    };

    let rendered = render(&target);

    let references = reference_dir();
    let output = golden_artifact_dir();
    let golden = Golden::new(&references, &output);

    match golden.verify(SCENE, &rendered) {
        Ok(report) => println!(
            "golden match for \"{SCENE}\": {}",
            report.measured_against(golden.tolerance)
        ),
        Err(error) => panic!("{error}"),
    }
}

/// [`B_EDGE_QUARTER_OVER_A`] recomputed from the sRGB transfer function.
///
/// Runs without a GPU, and deliberately does not render: a probe whose expected
/// value came out of the renderer it checks proves only that the renderer is
/// self-consistent. The two numbers below come from the coverage fraction and
/// the encode, and nothing else.
///
/// The transfer function is IEC 61966-2-1's, written out rather than taken from
/// a crate: `1.055 * c.powf(1.0 / 2.4) - 0.055` above the linear toe, which
/// `0.25` and `0.75` are both well clear of (the toe ends at `0.0031308`).
#[test]
fn the_quarter_covered_pixel_is_the_srgb_average_of_three_reds_and_one_green() {
    let encode = |linear: f64| {
        assert!(
            linear > 0.003_130_8,
            "{linear} is inside the linear toe, where this formula is the wrong one"
        );
        let encoded = 1.055 * linear.powf(1.0 / 2.4) - 0.055;
        // Round to nearest, which is what the resolve's fixed-point store does.
        (encoded * 255.0).round() as u8
    };

    // One of the four samples is past B's edge at 52.75; three are not.
    let covered = 1.0 / f64::from(narvo_render2d::SAMPLE_COUNT);
    assert!(
        (covered - 0.25).abs() < f64::EPSILON,
        "this derivation assumes four samples per pixel; SAMPLE_COUNT is {}",
        narvo_render2d::SAMPLE_COUNT
    );

    // A is full red, B full green, both at 1.0 linear in their own channel.
    let red = encode(1.0 - covered);
    let green = encode(covered);

    assert_eq!(
        [red, green, 0, 255],
        B_EDGE_QUARTER_OVER_A,
        "the probe at (52, 30) no longer equals a quarter of B over three \
         quarters of A. Either the coverage changed, or the resolve stopped \
         averaging in linear light."
    );
}

/// Every region of this file's atlas carries the border it claims.
///
/// One line, because the property and its message live at the shared fixture
/// place (D16, ADR-0016) rather than in each file that uses the fixture. Until
/// M3.22 this test and its red-demonstration twin were byte-identical in two
/// files — the seventh instance of the duplication D16 answers.
///
/// GPU-free. It guards THIS file's atlas only, and says nothing about any other
/// scene or draw path.
#[test]
fn every_region_of_the_atlas_carries_its_border() {
    LAYOUT.assert_every_region_is_padded(&atlas());
}
