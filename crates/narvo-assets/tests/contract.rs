//! The asset contract, checked: what ADR-0020 promises a consumer.
//!
//! Three groups, and the middle one is the reason this file is an integration
//! test rather than a unit module. The packer's conformance to
//! `check_region_padding` can only be asserted by *calling* that function, and
//! it lives in `narvo-render2d` — a crate `narvo-assets` must not depend on in
//! production, because `narvo-app`'s headless build asserts with `cargo tree
//! --edges normal` that no graphics crate is reachable. A **dev**-dependency
//! does not appear on a normal edge, so the guard is callable here and invisible
//! there. ADR-0016 recorded the same shape for `narvo-testkit`.

use narvo_assets::{Atlas, PackError, REGION_PADDING_TEXELS, SourceRegion, pack};
use narvo_render2d::{Pixels, check_region_padding};

/// A small heterogeneous set: the case the uniform fixtures do not cover.
///
/// Different widths, different heights, and a one-texel region, because a
/// region whose content is a single texel is the one where "the nearest content
/// texel" is the same texel for all eight border directions.
fn mixed() -> Vec<SourceRegion> {
    vec![
        SourceRegion::solid("wide", 12, 3, [200, 30, 40, 255]).expect("a valid region"),
        SourceRegion::solid("tall", 3, 12, [30, 200, 40, 255]).expect("a valid region"),
        SourceRegion::solid("square", 8, 8, [40, 30, 200, 255]).expect("a valid region"),
        SourceRegion::solid("dot", 1, 1, [250, 250, 10, 255]).expect("a valid region"),
        SourceRegion::solid("bar", 16, 2, [10, 200, 200, 255]).expect("a valid region"),
    ]
}

/// The atlas as the renderer would see it.
fn as_pixels(atlas: &Atlas) -> Pixels {
    Pixels::from_rgba8(atlas.width(), atlas.height(), atlas.rgba().to_vec())
        .expect("the packer produces a well-formed image")
}

// ---- the guard, both flanks ---------------------------------------------

/// **Every region of a packed atlas satisfies the renderer's padding guard.**
///
/// The contract's central claim, and it is checked by calling the guard rather
/// than by re-implementing its rule here — which would only prove that two
/// copies of my own reasoning agree.
#[test]
fn every_packed_region_satisfies_the_renderers_padding_guard() {
    let atlas = pack(mixed()).expect("a set this small always fits");
    let texture = as_pixels(&atlas);

    let mut checked = 0;
    for (name, place) in atlas.regions() {
        check_region_padding(
            place.left(),
            place.top(),
            place.width(),
            place.height(),
            REGION_PADDING_TEXELS,
            &texture,
        )
        .unwrap_or_else(|defect| panic!("region {name:?} is not padded as claimed: {defect:?}"));
        checked += 1;
    }

    // The count before the verdict: a loop over an empty table passes every
    // assertion inside it. §6.10 records this lesson twice.
    assert_eq!(checked, 5, "every region has to have been checked");
}

/// The guard **sees** a border that is one texel short of the claim.
///
/// The other flank, and the one that says the test above is an instrument
/// rather than a formality. The atlas is built correctly and then asked about
/// with a border one wider than it was packed with: the texels at that distance
/// belong to the untouched background, not to the region's edge, so a guard that
/// looks at content rather than at spacing must object.
#[test]
fn the_guard_sees_a_border_that_is_one_texel_short() {
    let atlas = pack(mixed()).expect("fits");
    let texture = as_pixels(&atlas);
    let place = atlas.region("square").expect("it was packed");

    let defect = check_region_padding(
        place.left(),
        place.top(),
        place.width(),
        place.height(),
        REGION_PADDING_TEXELS + 1,
        &texture,
    )
    .expect_err("the packer padded one texel, not two");

    // It names a texel rather than merely failing.
    assert!(
        format!("{defect:?}").contains("WrongTexel") || format!("{defect:?}").contains("NoRoom"),
        "unexpected defect: {defect:?}"
    );
}

/// The border width this crate uses is the one the renderer expects.
///
/// The two constants live in two crates that cannot see each other in
/// production, so nothing but this stops them drifting apart — and drift would
/// mean atlases the renderer's guard rejects, discovered by whoever next drew
/// one.
#[test]
fn the_padding_width_agrees_with_the_renderers() {
    assert_eq!(REGION_PADDING_TEXELS, narvo_render2d::REGION_PADDING_TEXELS);
}

// ---- determinism ---------------------------------------------------------

/// **The order regions arrive in does not reach the atlas.**
///
/// Every rotation of the input, plus the reverse, all the way to the same
/// anchor. This is the property ADR-0020 chose input-order invariance for: a
/// caller that reorders two lines of its own source must not move a committed
/// constant.
#[test]
fn permuting_the_input_does_not_move_anything() {
    let expected = pack(mixed()).expect("fits");

    let mut permutations = Vec::new();
    for rotation in 0..mixed().len() {
        let mut regions = mixed();
        regions.rotate_left(rotation);
        permutations.push(regions);
    }
    let mut reversed = mixed();
    reversed.reverse();
    permutations.push(reversed);

    assert_eq!(permutations.len(), 6);
    for regions in permutations {
        let atlas = pack(regions).expect("fits");

        assert_eq!(atlas.anchor(), expected.anchor());
        assert_eq!(atlas.rgba(), expected.rgba());
        assert_eq!(
            atlas.regions().collect::<Vec<_>>(),
            expected.regions().collect::<Vec<_>>()
        );
    }
}

/// Two runs of the same input are the same bytes.
#[test]
fn two_packs_of_one_set_are_byte_identical() {
    assert_eq!(pack(mixed()).expect("fits"), pack(mixed()).expect("fits"));
}

/// No two regions overlap, **padding included**.
///
/// Padding is the part worth saying: two regions whose content does not overlap
/// but whose borders do would each pass `check_region_padding` — each border
/// texel would be a copy of *someone's* edge — while the renderer bled one into
/// the other. The glyph atlas carries the same property for the same reason.
#[test]
fn no_two_regions_overlap_and_every_one_is_inside_the_atlas() {
    let atlas = pack(mixed()).expect("fits");
    let border = REGION_PADDING_TEXELS;

    let outer: Vec<(&str, u32, u32, u32, u32)> = atlas
        .regions()
        .map(|(name, place)| {
            (
                name,
                place.left() - border,
                place.top() - border,
                place.left() + place.width() + border,
                place.top() + place.height() + border,
            )
        })
        .collect();

    for &(name, _left, _top, right, bottom) in &outer {
        assert!(
            right <= atlas.width() && bottom <= atlas.height(),
            "{name} runs outside the atlas"
        );
    }

    let mut compared = 0;
    for (index, &(a, a_left, a_top, a_right, a_bottom)) in outer.iter().enumerate() {
        for &(b, b_left, b_top, b_right, b_bottom) in &outer[index + 1..] {
            let apart =
                a_right <= b_left || b_right <= a_left || a_bottom <= b_top || b_bottom <= a_top;
            assert!(apart, "the padded footprints of {a} and {b} overlap");
            compared += 1;
        }
    }
    assert_eq!(compared, 10, "every pair has to have been compared");
}

/// A region's texels reach the atlas **verbatim**.
///
/// The packer copies; it does not resample, premultiply or convert. A consumer
/// that hands in a colour gets that colour back, which is what makes the pixel
/// probes downstream mean anything.
#[test]
fn region_pixels_land_in_the_atlas_unchanged() {
    let sources = mixed();
    let atlas = pack(sources.clone()).expect("fits");

    for source in &sources {
        let place = atlas.region(source.name()).expect("it was packed");
        assert_eq!(
            (place.width(), place.height()),
            (source.width(), source.height())
        );

        for y in 0..source.height() {
            for x in 0..source.width() {
                let from = (y as usize * source.width() as usize + x as usize) * 4;
                let into = ((place.top() + y) as usize * atlas.width() as usize
                    + (place.left() + x) as usize)
                    * 4;
                assert_eq!(
                    &atlas.rgba()[into..into + 4],
                    &source.rgba()[from..from + 4],
                    "{} differs at ({x}, {y})",
                    source.name()
                );
            }
        }
    }
}

/// The atlas is square, a power of two, and no larger than it needs to be.
#[test]
fn the_atlas_grows_by_doubling_and_stops_when_it_fits() {
    let atlas = pack(mixed()).expect("fits");

    assert_eq!(atlas.width(), atlas.height());
    assert!(atlas.width().is_power_of_two(), "{}", atlas.width());

    // One size down would not have held it — which is what "stops when it fits"
    // means, and without this the packer could return 8192 every time and pass
    // every other assertion in this file.
    let half = atlas.width() / 2;
    let padded_area: u64 = mixed()
        .iter()
        .map(|region| {
            u64::from(region.width() + 2 * REGION_PADDING_TEXELS)
                * u64::from(region.height() + 2 * REGION_PADDING_TEXELS)
        })
        .sum();
    assert!(
        padded_area > u64::from(half) * u64::from(half) || half == 0,
        "an atlas of {half} would have had room for {padded_area} texels"
    );
}

// ---- the anchor ----------------------------------------------------------

/// **The committed anchor over a fixed set.**
///
/// # Provenance, and what to do when this fails
///
/// The value below was produced by this repository, which is exactly the kind of
/// constant ADR-0008 forbids for a *state* hash — and permits for a **content
/// anchor over a generated artefact**, which is what this is (CLAUDE.md, the
/// ADR-0008 entry, third kind of literal). The distinction is that nothing
/// outside this repository can move it: the regions are written just above, the
/// packer is in this workspace, and SHA-256 is frozen by FIPS 180-4. No
/// dependency bump can change it, so a break is a finding rather than noise.
///
/// **When it breaks:** the packer's placement or the anchor's own encoding
/// changed. That is allowed and needs no blessing — the value is fully
/// derivable, which is what separates it from a golden image — but it needs a
/// *reported reason*. ADR-0020 carries the procedure: say what moved, put the
/// new value in, and record it in the task's report. What it must never be is
/// pasted in to make a red run green.
#[test]
fn the_anchor_over_a_fixed_set_is_what_it_was() {
    let atlas = pack(mixed()).expect("fits");

    // Counted before compared: an anchor over an atlas that silently lost a
    // region would otherwise be a stable value nobody could question. M3.34
    // learned this and wrote it down; it costs one line.
    assert_eq!(atlas.len(), 5);
    assert_eq!(atlas.width(), 32);

    assert_eq!(
        atlas.anchor(),
        "65858027e81260b9cc9b52f8353f2cd90f3b24edb8cadfb22c779fba0888ee95",
        "the packed atlas is not the one this anchor was taken over. Either a placement \
         moved or a pixel did; both are legitimate and both need a reported reason, per \
         ADR-0020's re-anchor procedure. Do not paste the new value in without one."
    );
}

/// A moved placement changes the anchor, and a changed pixel changes it too.
///
/// Both halves, because an anchor that noticed only one of them would be half an
/// instrument — and which half would not be obvious from a green run.
#[test]
fn the_anchor_moves_when_a_placement_moves_or_a_pixel_changes() {
    let base = pack(mixed()).expect("fits");

    // A renamed region sorts elsewhere, so the placements move while every
    // pixel value in the set stays the same.
    let mut renamed = mixed();
    renamed[2] = SourceRegion::solid("zzz", 8, 8, [40, 30, 200, 255]).expect("valid");
    let moved = pack(renamed).expect("fits");
    assert_ne!(
        moved.anchor(),
        base.anchor(),
        "a moved placement went unnoticed"
    );

    // One channel of one region, with every placement unchanged.
    let mut recoloured = mixed();
    recoloured[2] = SourceRegion::solid("square", 8, 8, [41, 30, 200, 255]).expect("valid");
    let repainted = pack(recoloured).expect("fits");
    assert_eq!(
        repainted.regions().collect::<Vec<_>>(),
        base.regions().collect::<Vec<_>>(),
        "this half of the test needs the placements to be identical"
    );
    assert_ne!(
        repainted.anchor(),
        base.anchor(),
        "a changed pixel went unnoticed"
    );
}

// ---- the error paths -----------------------------------------------------

#[test]
fn a_duplicate_name_is_refused_by_name() {
    let error = pack(vec![
        SourceRegion::solid("icon", 4, 4, [1, 2, 3, 4]).expect("valid"),
        SourceRegion::solid("icon", 8, 8, [5, 6, 7, 8]).expect("valid"),
    ])
    .expect_err("two regions cannot share a name");

    assert_eq!(
        error,
        PackError::DuplicateName {
            name: "icon".to_owned()
        }
    );
    assert!(error.to_string().contains("sorts by"), "{error}");
}

#[test]
fn a_zero_side_is_refused_before_a_region_exists() {
    let error = SourceRegion::solid("flat", 0, 4, [0, 0, 0, 0]).expect_err("no texels");

    assert!(matches!(error, PackError::EmptyRegion { .. }));
    assert!(error.to_string().contains("0x4"), "{error}");
}

#[test]
fn pixels_that_do_not_match_the_size_are_refused_with_both_counts() {
    let error = SourceRegion::new("short", 4, 4, vec![0; 60]).expect_err("4x4 RGBA8 is 64 bytes");

    assert!(matches!(error, PackError::PixelCount { .. }));
    let message = error.to_string();
    assert!(message.contains("64 bytes"), "{message}");
    assert!(message.contains("60"), "{message}");
}

#[test]
fn a_region_too_large_for_any_atlas_names_its_padded_size() {
    let region = SourceRegion::solid("huge", 9000, 2, [0, 0, 0, 255]).expect("valid, just big");
    let error = pack(vec![region]).expect_err("no atlas is 9002 wide");

    assert!(matches!(error, PackError::RegionTooLarge { .. }));
    let message = error.to_string();
    assert!(
        message.contains("9002"),
        "the padded size has to be named: {message}"
    );
    assert!(message.contains("8192"), "{message}");
}

/// A set where every region fits and the set does not.
///
/// Distinguished from the case above on purpose: the remedy differs — shrink one
/// region, or split the set — and a message that conflated them would send a
/// reader the wrong way.
#[test]
fn a_set_too_large_for_one_atlas_says_it_is_the_set() {
    let regions: Vec<SourceRegion> = (0..40)
        .map(|index| {
            SourceRegion::solid(format!("big{index:02}"), 2000, 2000, [0, 0, 0, 255])
                .expect("valid on its own")
        })
        .collect();

    let error = pack(regions).expect_err("forty of those do not fit");

    assert!(matches!(error, PackError::DoesNotFit { .. }));
    let message = error.to_string();
    assert!(message.contains("40 regions"), "{message}");
    assert!(message.contains("67108864"), "the limit's area: {message}");
    assert!(message.contains("split it"), "{message}");
}

/// An empty set is an empty atlas, not an error.
#[test]
fn packing_nothing_is_an_empty_atlas() {
    let atlas = pack(Vec::new()).expect("nothing is a legal amount to pack");

    assert!(atlas.is_empty());
    assert_eq!(atlas.len(), 0);
}
