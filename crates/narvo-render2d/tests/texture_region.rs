//! Three sprites, one atlas texture, three different regions of it.
//!
//! The scene M3.9 adds, and the first one in this repository whose image
//! depends on *which part* of a texture a sprite samples. `sprite_placement.rs`
//! pins where a sprite lands; this pins what it shows.
//!
//! # The atlas
//!
//! Four 8 x 8 content quadrants, each split into an upper and a lower half of
//! four rows each — eight colours in all. The halves are what makes a vertical
//! mirror of a region visible: a region of four solid quadrants would sample
//! the same colour either way up, and the flip would pass every probe.
//!
//! # The border (M3.21, D13)
//!
//! Every quadrant carries a one-texel border of its own duplicated edge texels,
//! so each 8 x 8 region sits in a 10 x 10 cell and the atlas is **20 x 20**. The
//! border is what a bilinear sampler will blend against once D13's `Linear`
//! lands; today's `Nearest` never reads it, which is the property this file
//! proves by the reference image not moving.
//!
//! | content texels | cell in the atlas | quadrant | rows 0-3 | rows 4-7 |
//! | --- | --- | --- | --- | --- |
//! | (1, 1) 8 x 8 | (0, 0) | top left | red | dark red |
//! | (11, 1) 8 x 8 | (1, 0) | top right | green | dark green |
//! | (1, 11) 8 x 8 | (0, 1) | bottom left | blue | dark blue |
//! | (11, 11) 8 x 8 | (1, 1) | bottom right | yellow | olive |
//!
//! **The bottom-right quadrant is drawn by nothing.** Three sprites use the
//! other three, so yellow and olive may not appear anywhere in a correct image
//! — which turns "a region was ignored and the whole texture was drawn" into a
//! property of the *whole frame*, not of the pixels somebody thought to probe.
//! `no_pixel_comes_from_the_quadrant_no_sprite_uses` is that check.
//!
//! # The scene
//!
//! The three placements of M3.6, value for value, so the geometry is the one
//! `sprite_placement.rs` already probes. Its
//! `no_two_sprites_in_the_batch_scene_overlap` proves *its own* copy disjoint
//! and cannot see this one — each file under `tests/` is its own binary — so
//! this file carries `no_two_sprites_in_the_atlas_scene_overlap` for the copy
//! it uses. Two of the three are non-uniformly scaled. All edges land on
//! integer pixel boundaries, so no output pixel is partly covered and the
//! rasteriser has no coverage decision to make; that is the regime both
//! blessed images already sit in, and it is what M3 will leave behind when
//! rotation or a camera arrives.
//!
//! | sprite | centre | scale | region (texels) | covers pixels |
//! | --- | --- | --- | --- | --- |
//! | A | (-32, +32) | 32 x 32 | top left, (1, 1) 8 x 8 | x 16-47, y 16-47 |
//! | B | (+24, +32) | 48 x 24 | top right, (11, 1) 8 x 8 | x 64-111, y 20-43 |
//! | C | (-16, -40) | 24 x 40 | bottom left, (1, 11) 8 x 8 | x 36-59, y 84-123 |
//!
//! # The probes, derived before the first render
//!
//! For a pixel `(px, py)` of a 128 x 128 target the sample point is the pixel
//! centre, at world `(px - 63.5, 63.5 - py)`. Inside a sprite of centre `c` and
//! scale `s` that is the unit-square coordinate
//! `u1 = (world_x - c_x) / s_x + 0.5`, `v1 = 0.5 - (world_y - c_y) / s_y`, and
//! the region maps it to `u = u_left + u1 * (u_right - u_left)` and
//! `v = v_top + v1 * (v_bottom - v_top)`. The texel is `(u * 20, v * 20)`,
//! rounded down, because the sampler is `Nearest`.
//!
//! **The border moves every one of these by a whole texel and no fraction.** A
//! region's edges go from `k / 16` to `(k + 1 + 2 * cell) / 20` while its span
//! stays 8 texels of a 20-texel atlas, so `u * 20` is `u * 16` plus an integer:
//! 1 for the first cell on an axis and 3 for the second, on u and v alike.
//! `Nearest` rounds down, the fractional part is what decides where, and the
//! fractional part is untouched. That closes only with the clause under "Half
//! texels" below: no sample lands on a region boundary, so the shifted index is
//! still strictly inside the content block. Without it the argument would break
//! at the exclusive upper edge, where the old index 8 was the next quadrant and
//! the new index 9 is this cell's border texel.
//! That is the whole reason the reference image cannot move, and it is why the
//! margins below are the same numbers M3.9 derived.
//!
//! | probe | pixel | world | u1, v1 | u, v | texel | expected |
//! | --- | --- | --- | --- | --- | --- | --- |
//! | A upper | (40, 20) | (-23.5, +43.5) | 0.766, 0.141 | 0.356, 0.106 | (7, 2) | red |
//! | A lower | (40, 40) | (-23.5, +23.5) | 0.766, 0.766 | 0.356, 0.356 | (7, 7) | dark red |
//! | B upper | (70, 24) | (+6.5, +39.5) | 0.135, 0.188 | 0.604, 0.125 | (12, 2) | green |
//! | B lower | (70, 40) | (+6.5, +23.5) | 0.135, 0.854 | 0.604, 0.392 | (12, 7) | dark green |
//! | C upper | (40, 90) | (-23.5, -26.5) | 0.188, 0.163 | 0.125, 0.615 | (2, 12) | blue |
//! | C lower | (40, 118) | (-23.5, -54.5) | 0.188, 0.863 | 0.125, 0.895 | (2, 17) | dark blue |
//! | between A and B | (56, 30) | — | outside all three | — | — | black |
//! | between A and C | (30, 70) | — | outside all three | — | — | black |
//! | above B | (88, 12) | — | outside all three | — | — | black |
//!
//! Every texel above sits at least a twelfth of a texel from its own edges, so
//! none of them is a coin flip between two neighbours. The tightest is sprite
//! B's `u = 12.083`, a twelfth from the boundary at 12; the others have between
//! a **tenth** and half a texel of room — C lower's `v = 17.900` is 0.100 from
//! the boundary at 18, which the "eighth" this clause used to say excluded. That
//! is M3.9's own slip, carried in its text since M3.9 and corrected here rather
//! than re-endorsed; the twelfth, which is the load-bearing number, holds.
//! These are M3.9's margins unchanged, for
//! the reason given above — the border shifted the integer part and nothing
//! else, and a whole-texel shift is exactly the change `Nearest` cannot see.
//!
//! # Half texels
//!
//! A region's boundary is a texel *edge*: sprite B's region starts at
//! `u = 0.55`, which is texel 11.0. B's leftmost pixel column is 64, whose
//! centre is half a pixel inside the sprite, so it samples `u = 0.5542`, texel
//! 11.08 —
//! inside the region, not on its edge. The general form: with the sprite `n`
//! pixels wide and the region `m` texels wide, the first sample sits
//! `m / (2n)` texels in. Nothing here ever samples exactly on a region
//! boundary, and that is a property of pixel centres rather than of a
//! deliberate inset — no half-texel inset is applied here, and D13 chose the
//! border above over one. The two are alternatives, not a pair: the inset would
//! shrink the sampled rectangle, the border grows the atlas around it.

use std::path::{Path, PathBuf};

use narvo_render2d::{
    Golden, OffscreenTarget, Pixels, RenderError, SpriteFilter, SpriteInstance, SpritePlacement,
    TextureRegion, golden_artifact_dir,
};

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The blessed name of this scene's reference image.
const SCENE: &str = "sprite_atlas_regions_128x128";

/// The atlas's eight colours, upper half then lower half of each quadrant.
const TOP_LEFT_UPPER: [u8; 4] = narvo_testkit::TOP_LEFT_UPPER;
const TOP_LEFT_LOWER: [u8; 4] = narvo_testkit::TOP_LEFT_LOWER;
const TOP_RIGHT_UPPER: [u8; 4] = narvo_testkit::TOP_RIGHT_UPPER;
const TOP_RIGHT_LOWER: [u8; 4] = narvo_testkit::TOP_RIGHT_LOWER;
const BOTTOM_LEFT_UPPER: [u8; 4] = narvo_testkit::BOTTOM_LEFT_UPPER;
const BOTTOM_LEFT_LOWER: [u8; 4] = narvo_testkit::BOTTOM_LEFT_LOWER;
const BOTTOM_RIGHT_UPPER: [u8; 4] = narvo_testkit::BOTTOM_RIGHT_UPPER;
const BOTTOM_RIGHT_LOWER: [u8; 4] = narvo_testkit::BOTTOM_RIGHT_LOWER;

/// What the target is cleared to, and therefore what "no sprite here" looks
/// like.
const BACKGROUND: [u8; 4] = [0, 0, 0, 255];

/// This file's atlas layout: four 8 x 8 regions, each with a one-texel
/// border of duplicated edge texels (D13, shared per ADR-0016).
const LAYOUT: narvo_testkit::AtlasLayout = narvo_testkit::AtlasLayout::PADDED;

/// The 20 x 20 padded atlas described in this file's documentation.
///
/// The colour is chosen from the *content* coordinate, and the clamp is what
/// duplicates: a texel in the border takes the colour of the content texel
/// nearest to it, which is the definition
/// `narvo_render2d::check_region_padding` holds the fixture to.
fn atlas() -> Pixels {
    narvo_testkit::AtlasLayout::PADDED.atlas()
}

/// The content region of the cell at `(cell_x, cell_y)`.
///
/// The region points at the **content**, never at the border — padding
/// surrounds a region's coordinates rather than moving them, so this is an
/// ordinary [`TextureRegion::from_texels`] over the inner 8 x 8 block.
fn region(cell_x: u32, cell_y: u32, texture: &Pixels) -> TextureRegion {
    LAYOUT.region(cell_x, cell_y, texture)
}

/// Where content texel `(x, y)` of the cell at `(cell_x, cell_y)` sits in the
/// padded atlas.
///
/// The probe tables in this file's documentation are derived in content
/// coordinates, because that is what the derivation is about. This is the one
/// place the border enters them.
const fn content_texel(cell_x: u32, cell_y: u32, x: u32, y: u32) -> (u32, u32) {
    LAYOUT.content_texel(cell_x, cell_y, x, y)
}

/// The three sprites of the scene: the placements of M3.6, three regions.
///
/// **`Linear` since M3.24**, the first of the four blessed scenes converted
/// (D13, order in `ProjektPlan.md` §12). One place: this function is the only
/// definition of this scene, and the three margin files carry their own
/// deliberately-`Nearest` replicas, so nothing else in the workspace moves.
///
/// **No silhouette moves**, measured rather than argued: every one of the 320
/// changed pixels lies inside one of the three sprite footprints. The region
/// edges also stay pure, because the atlas is padded (M3.21) — the blend partner
/// one texel out is a copy of the region's own edge texel.
///
/// **One thing changes now, and M3.24 had two.** What changes is a blend across
/// each region's upper/lower half-boundary — three horizontal seams, one per
/// sprite, where a hard step becomes a short ramp. Measured, they are the whole
/// difference: sprite A's 128 pixels are rows 30–33 across all 32 of its
/// columns, B's 96 are rows 31–32 across all 48, and C's 96 are rows 102–105
/// across all 24. 128 + 96 + 96 = 320, and each band straddles its own sprite's
/// half-boundary — at rows 32, 32 and 104. The band widths are the
/// magnifications: B is squashed at 3 px per texel and gets two rows, C is
/// stretched at 5 px per texel and gets four.
///
/// The second thing was a **one-to-two pixel diagonal seam** along each sprite's
/// top-left-to-bottom-right line. The quad is two triangles sharing that edge
/// (`INDICES` in `quad.rs`), so a pixel on it is fully covered by the quad but
/// only partly by each triangle, and `centroid` interpolation therefore sampled
/// it off the pixel centre and could land in a different texel. Under `Nearest`
/// the displacement never crossed a texel edge and the seam was invisible; under
/// `Linear` it was not, and it carried this scene's worst deviation — pixel
/// (48, 104), 71 counts against the old reference.
///
/// **D17 removed it in M3.26**, and this scene is where that becomes visible:
/// the qualifier is `center` now, the diagonal is gone, and pixel (48, 104)
/// reads `[0, 0, 192]` like the rest of its row. The comparison against the old
/// reference drops to **320 of 16384, worst 64** — the pure-interior figure, all
/// of it on the three horizontal seams.
fn atlas_scene(texture: &Pixels) -> [SpriteInstance; 3] {
    [
        SpriteInstance::new(
            SpritePlacement {
                x: -32.0,
                y: 32.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 32.0,
                scale_y: 32.0,
            },
            region(0, 0, texture),
        )
        .sampled(SpriteFilter::Linear),
        SpriteInstance::new(
            SpritePlacement {
                x: 24.0,
                y: 32.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 48.0,
                scale_y: 24.0,
            },
            region(1, 0, texture),
        )
        .sampled(SpriteFilter::Linear),
        SpriteInstance::new(
            SpritePlacement {
                x: -16.0,
                y: -40.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 24.0,
                scale_y: 40.0,
            },
            region(0, 1, texture),
        )
        .sampled(SpriteFilter::Linear),
    ]
}

/// The probes of this file's documentation, as `(x, y, expected, why)`.
const PROBES: [(u32, u32, [u8; 4], &str); 9] = [
    (40, 20, TOP_LEFT_UPPER, "A, upper half of its region"),
    (40, 40, TOP_LEFT_LOWER, "A, lower half of its region"),
    (70, 24, TOP_RIGHT_UPPER, "B, upper half of its region"),
    (70, 40, TOP_RIGHT_LOWER, "B, lower half of its region"),
    (40, 90, BOTTOM_LEFT_UPPER, "C, upper half of its region"),
    (40, 118, BOTTOM_LEFT_LOWER, "C, lower half of its region"),
    (56, 30, BACKGROUND, "background between A and B"),
    (30, 70, BACKGROUND, "background between A and C"),
    (88, 12, BACKGROUND, "background above B"),
];

/// Where blessed reference images live. Read-only for anything automated.
fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Where a failing comparison writes what it rendered and how it differs.
///
/// [`golden_artifact_dir`] since M3.11, for the reason `golden_image.rs` gives:
/// one directory for every golden artifact in the repository, derived the one
/// way that is available to a unit test as well as to an integration test.
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

/// The fixture has to be able to fail for every mistake the probes name.
///
/// Runs without a GPU: it is a statement about the atlas, not about the
/// renderer. Nine colours, pairwise distinct — eight texel colours and the
/// background. If two of them ever coincided, a probe would pass against the
/// wrong region or the wrong half of one, and nothing would go red.
#[test]
fn the_atlas_can_tell_every_region_and_every_half_apart() {
    let mut colours = [
        TOP_LEFT_UPPER,
        TOP_LEFT_LOWER,
        TOP_RIGHT_UPPER,
        TOP_RIGHT_LOWER,
        BOTTOM_LEFT_UPPER,
        BOTTOM_LEFT_LOWER,
        BOTTOM_RIGHT_UPPER,
        BOTTOM_RIGHT_LOWER,
        BACKGROUND,
    ];
    colours.sort_unstable();
    colours.windows(2).for_each(|pair| {
        assert_ne!(
            pair[0], pair[1],
            "two of the nine colours are equal, so at least one probe could pass \
             against the wrong region, the wrong half of a region, or the cleared \
             background"
        );
    });
}

/// The atlas really is laid out the way the derivation assumed.
///
/// Runs without a GPU. The probe expectations were computed from "content texel
/// (6, 1) of the top-left quadrant is its upper half"; this asserts that of the
/// actual fixture, so the table above cannot quietly stop describing it. The
/// coordinates are content coordinates, put through [`content_texel`] — the same
/// arithmetic [`region`] uses, so a border of the wrong width moves both
/// together and cannot make this pass by accident.
#[test]
fn the_atlas_puts_each_colour_where_the_derivation_expects_it() {
    let atlas = atlas();

    // The last two columns are the module doc's own "texel" column, asserted
    // rather than recomputed. Without them the table and `content_texel` would
    // share `LAYOUT`'s `cell()` and `border` and move together, so a wrong
    // border width would leave the documentation stale with nothing to catch it.
    for (cell_x, cell_y, in_x, in_y, expected, at_x, at_y, what) in [
        (0, 0, 6, 1, TOP_LEFT_UPPER, 7, 2, "A upper probe's texel"),
        (0, 0, 6, 6, TOP_LEFT_LOWER, 7, 7, "A lower probe's texel"),
        (1, 0, 1, 1, TOP_RIGHT_UPPER, 12, 2, "B upper probe's texel"),
        (1, 0, 1, 6, TOP_RIGHT_LOWER, 12, 7, "B lower probe's texel"),
        (
            0,
            1,
            1,
            1,
            BOTTOM_LEFT_UPPER,
            2,
            12,
            "C upper probe's texel",
        ),
        (
            0,
            1,
            1,
            6,
            BOTTOM_LEFT_LOWER,
            2,
            17,
            "C lower probe's texel",
        ),
        (
            1,
            1,
            4,
            4,
            BOTTOM_RIGHT_LOWER,
            15,
            15,
            "the unused quadrant",
        ),
        (0, 0, 0, 0, TOP_LEFT_UPPER, 1, 1, "the first content texel"),
    ] {
        let (x, y) = content_texel(cell_x, cell_y, in_x, in_y);
        assert_eq!(
            (x, y),
            (at_x, at_y),
            "content texel ({in_x}, {in_y}) of cell ({cell_x}, {cell_y}) is atlas texel \
         ({at_x}, {at_y}) in this file's derivation table"
        );
        let actual = atlas
            .pixel(x, y)
            .unwrap_or_else(|| panic!("({x}, {y}) is outside the atlas"));
        assert_eq!(
            actual, expected,
            "atlas texel ({x}, {y}), {what}: expected {expected:?}, fixture has \
             {actual:?}"
        );
    }
}

/// The scene's regions are three different ones, and none is the whole texture.
///
/// Runs without a GPU. Without it, "each sprite gets its own region" would rest
/// on reading three constructor calls, which is exactly the kind of claim this
/// project has watched go stale.
#[test]
fn the_scene_gives_each_sprite_a_different_region() {
    let atlas = atlas();
    let scene = atlas_scene(&atlas);

    for (index, sprite) in scene.iter().enumerate() {
        assert_ne!(
            sprite.region.uv_bounds(),
            TextureRegion::WHOLE_TEXTURE.uv_bounds(),
            "sprite {index} samples the whole texture, so the scene would not show \
             whether regions work at all"
        );
    }

    for (i, a) in scene.iter().enumerate() {
        for (j, b) in scene.iter().enumerate().skip(i + 1) {
            assert_ne!(
                a.region.uv_bounds(),
                b.region.uv_bounds(),
                "sprites {i} and {j} share a region; the scene has to use three \
                 different ones or a swap between them is invisible"
            );
        }
    }
}

/// This file's own copy of the scene stays overlap-free.
///
/// Runs without a GPU. `sprite_placement.rs` has the same assertion over the
/// same nine numbers, and it is not enough: each file under `tests/` compiles
/// to its own binary, so that test reads its own array and would stay green
/// while this one drifted.
///
/// **Kept in M3.10 for a different reason than it was written for.** Draw order
/// was undecided when this was written; it is decided now, in `narvo-app`'s
/// `placements_of`, and an overlapping scene is no longer ill-defined. What
/// this still buys is that each probe below belongs to exactly one sprite *and*
/// to exactly one region: the whole point of the file is that a pixel names
/// which region its sprite sampled, and a contested pixel names two.
#[test]
fn no_two_sprites_in_the_atlas_scene_overlap() {
    let atlas = atlas();
    let extents: Vec<(f32, f32, f32, f32)> = atlas_scene(&atlas)
        .iter()
        .map(|sprite| {
            let p = sprite.placement;
            (
                p.x - p.scale_x / 2.0,
                p.x + p.scale_x / 2.0,
                p.y - p.scale_y / 2.0,
                p.y + p.scale_y / 2.0,
            )
        })
        .collect();

    for (i, a) in extents.iter().enumerate() {
        for (j, b) in extents.iter().enumerate().skip(i + 1) {
            let apart = a.1 <= b.0 || b.1 <= a.0 || a.3 <= b.2 || b.3 <= a.2;
            assert!(
                apart,
                "sprites {i} and {j} overlap: {a:?} against {b:?}. Overlapping \
                 sprites make the image depend on draw order, and every probe in \
                 this file would then be asserting something M3.9 is not entitled \
                 to assert."
            );
        }
    }
}

#[test]
fn each_sprite_shows_its_own_region_of_the_atlas() {
    let Some(target) = target_or_skip(128, 128) else {
        return;
    };

    let atlas = atlas();
    let rendered = target
        .render_sprites(&atlas, &atlas_scene(&atlas))
        .expect("three sprites are far inside the batch limit");

    for (x, y, expected, why) in PROBES {
        let actual = rendered
            .pixel(x, y)
            .unwrap_or_else(|| panic!("({x}, {y}) is outside the 128 x 128 target"));

        println!("({x}, {y}) {why}: {actual:?}");
        assert_eq!(
            actual, expected,
            "pixel ({x}, {y}), {why}: expected {expected:?}, rendered {actual:?}. \
             The expectation was derived from the projection and the region before \
             this ran. A sprite showing another sprite's colours means the regions \
             are swapped or all equal; a sprite showing the wrong half of its own \
             region means the region is upside down; a colour from a quadrant no \
             sprite uses means the region was ignored and the whole texture drawn."
        );
    }
}

/// No pixel of the frame comes from the quadrant no sprite asked for.
///
/// The whole-frame form of "the region was ignored". A sprite drawn full-area
/// instead of region-wise reaches into all four quadrants, and the fourth one
/// is in no sprite's region, so its two colours cannot legitimately appear
/// anywhere. This looks at all 16 384 pixels rather than the six the probes
/// name, which is the difference between catching the mistake and catching it
/// where somebody happened to look.
#[test]
fn no_pixel_comes_from_the_quadrant_no_sprite_uses() {
    let Some(target) = target_or_skip(128, 128) else {
        return;
    };

    let atlas = atlas();
    let rendered = target
        .render_sprites(&atlas, &atlas_scene(&atlas))
        .expect("three sprites are far inside the batch limit");

    for y in 0..rendered.height() {
        for x in 0..rendered.width() {
            let pixel = rendered
                .pixel(x, y)
                .expect("x and y come from the image's own dimensions");
            assert!(
                pixel != BOTTOM_RIGHT_UPPER && pixel != BOTTOM_RIGHT_LOWER,
                "pixel ({x}, {y}) is {pixel:?}, a colour from the atlas's \
                 bottom-right quadrant. No sprite in this scene uses that quadrant, \
                 so it can only appear if a sprite sampled the whole texture \
                 instead of its region."
            );
        }
    }
}

/// The blessed reference for the atlas scene.
///
/// **Expected to fail until the maintainer blesses the reference**, exactly as
/// M3.5's did. Nothing here writes one: the run leaves a `.actual.png` under
/// the target directory and says where. That is the designed outcome of M3.9,
/// not a defect.
#[test]
fn the_atlas_scene_matches_its_golden_reference() {
    let Some(target) = target_or_skip(128, 128) else {
        return;
    };

    let atlas = atlas();
    let rendered = target
        .render_sprites(&atlas, &atlas_scene(&atlas))
        .expect("three sprites are far inside the batch limit");

    let references = reference_dir();
    let output = output_dir();
    let golden = Golden::new(&references, &output);

    match golden.verify(SCENE, &rendered) {
        Ok(report) => println!(
            "golden match for \"{SCENE}\": {}",
            report.measured_against(golden.tolerance)
        ),
        Err(error) => panic!("{error}"),
    }
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
