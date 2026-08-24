//! The oracles a hierarchy has and a single stage cannot.
//!
//! M8.5a's eleven still apply to every level in isolation. Three things are new
//! here, and each is the reason a cascade exists at all:
//!
//! - **a source beyond level zero's interval is seen, via a higher level.** A
//!   single stage reads exactly zero there — M8.5a pinned that as an exact
//!   test — and repairing it is the whole point of the hierarchy;
//! - **the composed answer does not exceed what one all-encompassing interval
//!   gives.** That is the double-counting oracle, and its bound is derived
//!   below rather than chosen;
//! - **the two merge forms are compared**, which is §2(b) and the number M8.5a
//!   could not produce.
//!
//! # Reach, stated per oracle
//!
//! | oracle | exact? | what it cannot see |
//! |---|---|---|
//! | a source beyond level 0 is seen | no, a threshold | a level in the middle being skipped, if a lower one still reaches the source |
//! | the composed answer does not exceed one interval | no, a derived bound | anything that makes the answer too *small* |
//! | the white chamber composes to the sky | **yes** | anything that cancels between levels |
//! | two runs agree | yes | every deterministic defect, which is all of them |
//! | the two forms diverge | a ratio | a defect both forms share |

use narvo_render2d::{
    Cascade, CascadeLayout, CascadeStage, Emission, MergeForm, OffscreenTarget, RadianceField,
    RenderError, Seeds, StageLayout,
};

/// A target to borrow a device from, or `None` on a machine with no adapter.
fn target_or_skip() -> Option<OffscreenTarget> {
    match OffscreenTarget::new(8, 8) {
        Ok(target) => Some(target),
        Err(RenderError::NoAdapter { .. }) => None,
        Err(other) => {
            panic!("the offscreen target failed for a reason that is not absence: {other}")
        }
    }
}

const FIELD: u32 = 128;

/// The cascade every test here varies one field of.
///
/// `f0 = 2` texels, so the intervals are `[0, 2]`, `[2, 10]`, `[10, 42]`,
/// `[42, 170]`. `D_0 = 32` satisfies the penumbra inequality at every level: the
/// asymptotic bound is `8·pi·f0/3 = 16.8`, and 32 clears it.
fn base_layout(levels: u32) -> CascadeLayout {
    CascadeLayout {
        origin: [0.0, 0.0],
        base_spacing: 4.0,
        base_interval: 2.0,
        base_directions: 32,
        levels,
        sky: [0.0, 0.0, 0.0],
    }
}

fn empty_scene() -> (Seeds, Emission) {
    (
        Seeds::new(FIELD, FIELD).expect("a seed set"),
        Emission::new(FIELD, FIELD).expect("an emission map"),
    )
}

fn occlude(seeds: &mut Seeds, emission: &mut Emission, points: &[(u32, u32)], rgb: [f32; 3]) {
    for &(x, y) in points {
        seeds.set(x, y).expect("inside the field");
        emission.set(x, y, rgb).expect("inside the map");
    }
}

/// A filled disc of occluders.
fn disc(cx: i32, cy: i32, radius: i32) -> Vec<(u32, u32)> {
    let mut points = Vec::new();
    for y in -radius..=radius {
        for x in -radius..=radius {
            if x * x + y * y <= radius * radius {
                let (px, py) = (cx + x, cy + y);
                if (0..FIELD as i32).contains(&px) && (0..FIELD as i32).contains(&py) {
                    points.push((px as u32, py as u32));
                }
            }
        }
    }
    points
}

/// The red channel at the probe whose position is `(x, y)` texels.
fn at(field: &RadianceField, x: u32, y: u32, spacing: u32) -> f64 {
    f64::from(
        field
            .radiance(x / spacing, y / spacing)
            .expect("a probe inside the grid")[0],
    )
}

// -- the oracle only a hierarchy can have ---------------------------------

/// **A source beyond level zero's interval is seen, and only because of the
/// levels above it.**
///
/// The lamp stands 30 texels from the probe. Level zero reaches 2 and level one
/// reaches 10, so neither can see it; level two reaches 42 and does. A cascade
/// of one level must therefore read **exactly zero** — which is M8.5a's own
/// exact test, reused as the control — and a cascade of three must not.
///
/// **Reach.** This is a threshold, not a value. It cannot see a level in the
/// middle being skipped as long as some level still reaches the source, which is
/// what injection J1 exploits and why the double-counting oracle is beside it.
#[test]
fn a_source_beyond_the_first_interval_is_seen_through_a_higher_level() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene();
    occlude(&mut seeds, &mut emission, &disc(94, 64, 3), [1.0, 1.0, 1.0]);

    for form in [MergeForm::Aggregate, MergeForm::Directional] {
        let one = Cascade::new(base_layout(1), FIELD, FIELD).expect("one level");
        let three = Cascade::new(base_layout(3), FIELD, FIELD).expect("three levels");

        let shallow = target
            .cascade(&seeds, &emission, &one, form)
            .expect("the cascade runs");
        let deep = target
            .cascade(&seeds, &emission, &three, form)
            .expect("the cascade runs");

        assert_eq!(
            at(&shallow, 64, 64, 4),
            0.0,
            "{form:?}: a cascade reaching two texels saw a lamp thirty texels away"
        );
        assert!(
            at(&deep, 64, 64, 4) > 0.01,
            "{form:?}: three levels did not see a lamp that level two's interval \
             contains — the hierarchy is not composing at all. Read {}",
            at(&deep, 64, 64, 4)
        );
    }
}

/// **Every band is answered by the level that owns it — one source per band.**
///
/// The oracle above says a source beyond level zero is seen; it does **not** say
/// *which* level saw it, so a cascade that skipped a middle level would still
/// pass it as long as some other level reached the source. That is injection J1,
/// and this is the oracle written to close it: a source is placed inside each
/// level's own band, and each must be invisible to a cascade that stops below it
/// and visible to one that reaches it.
///
/// The bands are `[0, 2]`, `[2, 10]` and `[10, 42]`, so distances of 1, 6 and 30
/// texels fall one in each. The "invisible" half is **exact** — a cascade whose
/// top interval ends before the source reads zero, because the sky is zero and
/// nothing else is in the scene.
#[test]
fn every_band_is_answered_by_the_level_that_owns_it() {
    let Some(target) = target_or_skip() else {
        return;
    };
    // (distance, the fewest levels that can reach it)
    for (distance, needs) in [(1_i32, 1_u32), (6, 2), (30, 3)] {
        let (mut seeds, mut emission) = empty_scene();
        occlude(
            &mut seeds,
            &mut emission,
            &disc(64 + distance, 64, 1),
            [1.0, 1.0, 1.0],
        );
        for form in [MergeForm::Aggregate, MergeForm::Directional] {
            if needs > 1 {
                let short =
                    Cascade::new(base_layout(needs - 1), FIELD, FIELD).expect("a sound cascade");
                assert_eq!(
                    at(
                        &target
                            .cascade(&seeds, &emission, &short, form)
                            .expect("the cascade runs"),
                        64,
                        64,
                        4
                    ),
                    0.0,
                    "{form:?}: a cascade of {} levels saw a source {distance} texels away, which lies in the band of level {}",
                    needs - 1,
                    needs - 1
                );
            }
            let enough = Cascade::new(base_layout(needs), FIELD, FIELD).expect("a sound cascade");
            let read = at(
                &target
                    .cascade(&seeds, &emission, &enough, form)
                    .expect("the cascade runs"),
                64,
                64,
                4,
            );
            assert!(
                read > 0.0,
                "{form:?}: a cascade of {needs} levels did not see a source {distance} texels away, although level {} owns that band. A level that is skipped in the merge shows up exactly here",
                needs - 1
            );
        }
    }
}

// -- the double-counting bound --------------------------------------------

/// **The composed answer does not exceed what one all-encompassing interval
/// gives, and the bound is derived.**
///
/// # The derivation
///
/// With intervals that tile `[0, t_top]` without overlap, each direction's
/// contribution is the emission of the **first** thing it meets — exactly what a
/// single stage over `[0, t_top]` computes. The two therefore measure one
/// physical quantity, and differ only in how finely they sample the circle:
/// a source of angular size `alpha` is met by `n` directions with
/// `|n/D - alpha/(2·pi)| <= 1/D`. So for one source of radiance `L`,
///
/// ```text
///     |composed - single| <= L · (1/D_seeing + 1/D_single)
/// ```
///
/// where `D_seeing` is the direction count of the level whose interval contains
/// the source. Here `D_seeing = D_single = 512`, so the bound is `2L/512`.
///
/// **Two terms are added to it, and both are named rather than absorbed.** The
/// march stops within a texel of what stopped it, so a source's *effective*
/// angular size differs by up to `2/r` radians between two marches that start at
/// different distances — `1/(pi·r)` in fraction, which at `r = 30` is `0.0106`.
/// And the probe is read at one grid position in both cases, so there is no
/// interpolation term at all: probe `(16, 16)` of level zero is index `(8, 8)`
/// of level one and `(4, 4)` of level two, all even, so every lookup on its path
/// is a lookup and not an interpolation.
///
/// **Double counting is not subtle against this.** If a level marches into the
/// next level's interval it sees the source too, and the composed value becomes
/// `a + (1-a)b` where `b` is the correct fraction — strictly greater than `b`
/// whenever `a > 0`, and by a whole angular fraction rather than by a
/// quantisation step.
///
/// # What the measurement turned this test into
///
/// The **directional** form matches the single interval **exactly** — excess
/// `0.000000` — so for it this is a conservation oracle with a derived bound and
/// a real red edge.
///
/// The **aggregate** form does not: it reads `0.268555` against `0.164062`, an
/// excess of `0.104492` where the bound is `0.049554`. That is not double
/// counting in the sense of a band marched twice; it is the aggregate merge
/// spreading the level above's mean over every escaping direction, so light one
/// direction could have carried is counted by all of them. The assertion is
/// therefore inverted for that form — it must *exceed* — because a version that
/// stopped exceeding would mean the two merges had converged, and that is a
/// finding this test must not let pass in silence.
#[test]
fn the_composed_answer_does_not_exceed_one_all_encompassing_interval() {
    let Some(target) = target_or_skip() else {
        return;
    };
    // **The lamp sits at 8 texels, inside level one's band `[2, 10]`, and that
    // placement is a measurement rather than a convenience.** The first version
    // of this test put it at 30, in level *two*'s band — where no widening of a
    // lower level's interval can reach it, because level two is the top. A band
    // that only one level can ever own cannot be double counted, so the oracle
    // had no red edge at all and J3 walked straight past it. At 8 texels a
    // level whose near end slips below 2 sees the lamp as well, and the bound
    // catches it.
    let (mut seeds, mut emission) = empty_scene();
    occlude(&mut seeds, &mut emission, &disc(72, 64, 3), [1.0, 1.0, 1.0]);

    // One stage over the union of the three intervals, at the top level's
    // angular resolution.
    let single = CascadeStage::new(StageLayout {
        origin: [64.0, 64.0],
        spacing: 4.0,
        probes: [1, 1],
        near: 0.0,
        far: 42.0,
        directions: 512,
        far_radiance: [0.0, 0.0, 0.0],
    })
    .expect("a sound stage");
    let reference = f64::from(
        target
            .cascade_stage(&seeds, &emission, &single)
            .expect("the stage runs")
            .radiance(0, 0)
            .expect("a probe")[0],
    );
    assert!(
        reference > 0.01,
        "the all-encompassing stage did not see the lamp, so this comparison says \
         nothing. Read {reference}"
    );

    // The angular quantisation of the level that owns the band — level one, at
    // 128 directions — plus the single stage's 512, plus `1/(pi·r)` at `r = 8`
    // for the margin's effect on the lamp's apparent size.
    let bound = 1.0 / 128.0 + 1.0 / 512.0 + 1.0 / (std::f64::consts::PI * 8.0);
    for form in [MergeForm::Directional, MergeForm::Aggregate] {
        let cascade = Cascade::new(base_layout(3), FIELD, FIELD).expect("three levels");
        let composed = at(
            &target
                .cascade(&seeds, &emission, &cascade, form)
                .expect("the cascade runs"),
            64,
            64,
            4,
        );
        eprintln!(
            "M8.5b double-count: {form:?} composed={composed:.6} single={reference:.6} bound={bound:.6} excess={:.6}",
            composed - reference
        );
        match form {
            MergeForm::Directional => assert!(
                composed <= reference + bound,
                "the directional cascade read {composed} where one interval reads \
                 {reference}, which is beyond the derived bound of {bound}. A band \
                 counted twice is the way to exceed it"
            ),
            // **Measured, and a property of the form rather than a defect to be
            // tolerated.** The aggregate merge gives every escaping direction the
            // level above's *mean*, so light that only one direction could have
            // carried is spread over all of them, and the total lands above what
            // a single interval over the same band computes. It is the leak of
            // `the_aggregate_form_leaks_light_through_a_solid_wall` seen as a
            // conservation failure instead of as a missing shadow.
            //
            // Asserted rather than merely printed, so that an aggregate merge
            // which stopped exceeding the bound would be noticed: that would mean
            // the two merges had converged, which is exactly the finding §2(b)
            // looks for and must not pass in silence.
            MergeForm::Aggregate => assert!(
                composed > reference + bound,
                "the aggregate cascade read {composed} against a single interval's \
                 {reference}, within the derived bound of {bound}. It has stopped \
                 exceeding the bound, which means the two merges have converged — \
                 report that, do not delete this assertion"
            ),
        }
    }
}

// -- conservation ----------------------------------------------------------

/// **A white chamber composes to the sky, exactly, however many levels there
/// are.**
///
/// With nothing in the way every direction of every level escapes, so each level
/// carries the one above unchanged and the bottom must read the top's sky. It is
/// **exact** rather than bounded, and that is the interpolation weights being
/// powers of two: four equal samples sum pairwise to exactly four times one, and
/// the division by four is exact. A weight that was not a power of two would
/// show up here as a last-bit drift that grew with the level count.
///
/// **Reach.** Blind to anything that cancels between levels — in particular to a
/// level being skipped, since skipping a level that carries its input unchanged
/// changes nothing. That is exactly J1, and it is why the source oracle above is
/// not redundant with this one.
#[test]
fn a_white_chamber_composes_to_the_sky_exactly() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission) = empty_scene();
    let sky = [0.25_f32, 0.5, 0.125];

    for levels in 1..=4_u32 {
        for form in [MergeForm::Aggregate, MergeForm::Directional] {
            let cascade = Cascade::new(
                CascadeLayout {
                    sky,
                    ..base_layout(levels)
                },
                FIELD,
                FIELD,
            )
            .expect("a sound cascade");
            let field = target
                .cascade(&seeds, &emission, &cascade, form)
                .expect("the cascade runs");
            for (x, y) in [(0_u32, 0_u32), (5, 5), (16, 16), (32, 32), (17, 4)] {
                assert_eq!(
                    field.radiance(x, y).expect("a probe inside the grid"),
                    sky,
                    "{form:?}, {levels} levels: probe ({x}, {y}) did not compose to \
                     the sky exactly. A drift here is an interpolation weight that \
                     is not a power of two"
                );
                assert_eq!(
                    field.escaped(x, y).expect("a probe inside the grid"),
                    1.0,
                    "{form:?}, {levels} levels: probe ({x}, {y}) blocked something in \
                     an empty chamber"
                );
            }
        }
    }
}

/// Two runs of one cascade agree byte for byte, in both forms.
#[test]
fn two_runs_of_one_cascade_agree_byte_for_byte() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene();
    let scatter: Vec<(u32, u32)> = (0..96)
        .map(|k: u32| ((k * 11 + 5) % FIELD, (k * 29 + 17) % FIELD))
        .collect();
    occlude(&mut seeds, &mut emission, &scatter, [0.1, 0.3, 0.7]);

    for form in [MergeForm::Aggregate, MergeForm::Directional] {
        let cascade = Cascade::new(
            CascadeLayout {
                sky: [0.05, 0.05, 0.05],
                ..base_layout(3)
            },
            FIELD,
            FIELD,
        )
        .expect("a sound cascade");
        let first = target
            .cascade(&seeds, &emission, &cascade, form)
            .expect("the cascade runs");
        let second = target
            .cascade(&seeds, &emission, &cascade, form)
            .expect("the cascade runs again");
        assert_eq!(
            first.texels(),
            second.texels(),
            "{form:?}: two runs of one cascade over one world disagreed"
        );
        assert!(
            first.texels().iter().any(|value| *value > 0.0),
            "{form:?}: the scene this compares is blank"
        );
    }
}

// -- §2(b): where the two forms part ---------------------------------------

/// **A solid wall, and the mechanism by which the aggregate form leaks.**
///
/// The wall stands 20 texels from the probe; the lamp 60 texels beyond it. The
/// wall falls inside level two's interval `[10, 42]` and the lamp inside level
/// three's `[42, 170]`.
///
/// Level three's directions **start at 42**, which is past the wall — so from
/// level three's point of view the lamp is unoccluded, and level three's answer
/// is bright. That is correct: level three is only ever asked about the band
/// beyond 42, and in that band the lamp really is visible.
///
/// What differs is who takes it. The directional form gives each escaping
/// direction the upper radiance **of its own arc**, and the arc pointing at the
/// lamp did not escape — it hit the wall — so the light does not come down. The
/// aggregate form has only one upper number and applies it to every escaping
/// direction, so the light comes down through the wall. This is not a subtle
/// numerical difference; it is a wall that does not cast a shadow.
///
/// # The assertion is a parity, because the measurement turned out to be one
///
/// At a probe whose grid index is **even on both axes**, every level's lookup is
/// a lookup rather than an interpolation — index `i` maps to upper index `i/2`
/// with weight one. There the directional form reads **exactly zero**: the
/// shadow is perfect. At an odd index the probe sits halfway between two upper
/// probes, one of which stands closer to the wall and can see past it, so a few
/// per cent leaks in.
///
/// So the two forms fail differently and the test says which is which: the
/// directional form's residual is **interpolation**, bounded by probe spacing
/// and reducible by shrinking it; the aggregate form's is **composition**, and
/// no probe spacing removes it — it is 18 % to 49 % at aligned and interpolated
/// probes alike.
#[test]
fn the_aggregate_form_leaks_light_through_a_solid_wall() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let wall: Vec<(u32, u32)> = (0..FIELD).map(|y| (40_u32, y)).collect();
    let lamp = disc(100, 64, 4);

    let (mut open_seeds, mut open_emission) = empty_scene();
    occlude(&mut open_seeds, &mut open_emission, &lamp, [1.0, 1.0, 1.0]);
    let (mut seeds, mut emission) = empty_scene();
    occlude(&mut seeds, &mut emission, &lamp, [1.0, 1.0, 1.0]);
    occlude(&mut seeds, &mut emission, &wall, [0.0, 0.0, 0.0]);
    let cascade = Cascade::new(base_layout(4), FIELD, FIELD).expect("four levels");

    // Probes on the lamp's row, at even and at odd indices, all in front of the
    // wall. `y = 64` is index 16, which is even, so the parity is the x index's.
    let aligned = [(0_u32, 64_u32), (16, 64), (32, 64)];
    let between = [(20_u32, 64_u32), (36, 64)];

    for form in [MergeForm::Directional, MergeForm::Aggregate] {
        let shaded_field = target
            .cascade(&seeds, &emission, &cascade, form)
            .expect("the cascade runs");
        let open_field = target
            .cascade(&open_seeds, &open_emission, &cascade, form)
            .expect("the cascade runs");

        for (px, py) in aligned.into_iter().chain(between) {
            let shaded = at(&shaded_field, px, py, 4);
            let opened = at(&open_field, px, py, 4);
            assert!(
                opened > 0.001,
                "{form:?}: the unoccluded lamp read {opened} at ({px}, {py}), so there is no scale to measure a shadow against"
            );
            let share = shaded / opened;
            eprintln!(
                "M8.5b wall: {form:?} probe ({px},{py}) index ({},{}) open={opened:.6} shadowed={shaded:.6} share={share:.4}",
                px / 4,
                py / 4
            );
            match form {
                MergeForm::Directional if aligned.contains(&(px, py)) => assert_eq!(
                    shaded, 0.0,
                    "the directional form let light past a solid wall at ({px}, {py}), where every level's lookup is a lookup and nothing is interpolated. That is a composition defect, not an interpolation one"
                ),
                MergeForm::Directional => assert!(
                    share < 0.20,
                    "the directional form let {share} through at ({px}, {py}), which is more than the interpolation between two upper probes can account for"
                ),
                MergeForm::Aggregate => assert!(
                    share > 0.10,
                    "the aggregate form held a shadow at ({px}, {py}) — share {share}. If it no longer leaks, the difference between the two forms has gone and that is the finding"
                ),
            }
        }
    }
}

/// **§2(b)'s arrangement: a bright source visible only through a narrow slit.**
///
/// The wall has a four-texel gap on the probe row. A probe **aligned** with the
/// slit can see the lamp through it; a probe well off to the side cannot. The
/// directional form should hold that contrast and the aggregate form should
/// smear it, because the aggregate form applies one upper number to every
/// escaping direction whether or not that direction points through the gap.
///
/// **The check that this arrangement can show anything at all** is the first
/// assertion: the directional form itself must separate on-axis from off-axis by
/// more than its own quantisation floor of `1/D_0`. Without that, a small
/// difference between the forms would mean nothing — which is the trap M8.5a's
/// thin scene fell into one level down.
#[test]
fn the_two_forms_part_on_a_source_seen_through_a_slit() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let wall: Vec<(u32, u32)> = (0..FIELD)
        .filter(|y| !(62..66).contains(y))
        .map(|y| (40_u32, y))
        .collect();
    let lamp = disc(100, 64, 4);

    let (mut seeds, mut emission) = empty_scene();
    occlude(&mut seeds, &mut emission, &lamp, [1.0, 1.0, 1.0]);
    occlude(&mut seeds, &mut emission, &wall, [0.0, 0.0, 0.0]);
    let cascade = Cascade::new(base_layout(4), FIELD, FIELD).expect("four levels");

    let mut contrast = Vec::new();
    for form in [MergeForm::Directional, MergeForm::Aggregate] {
        let field = target
            .cascade(&seeds, &emission, &cascade, form)
            .expect("the cascade runs");
        let on_axis = at(&field, 20, 64, 4);
        let off_axis = at(&field, 20, 20, 4);
        let ratio = if off_axis > 0.0 {
            on_axis / off_axis
        } else {
            f64::INFINITY
        };
        eprintln!(
            "M8.5b §2(b) slit: {form:?} on_axis={on_axis:.6} off_axis={off_axis:.6} contrast={ratio:.3}"
        );
        contrast.push((on_axis, off_axis, ratio));
    }

    let (dir_on, dir_off, dir_ratio) = contrast[0];
    let (_, _, agg_ratio) = contrast[1];
    // The quantisation floor: one of level zero's thirty-two directions.
    let floor = 1.0 / 32.0;
    assert!(
        dir_on - dir_off > floor * dir_on || dir_on > dir_off * 2.0,
        "the directional form separated on-axis {dir_on} from off-axis {dir_off} by \
         less than its own quantisation floor, so this arrangement cannot show a \
         difference between the forms and the comparison below would say nothing"
    );
    assert!(
        dir_ratio > agg_ratio,
        "the aggregate form held the slit's contrast at least as well as the \
         directional one: directional {dir_ratio}, aggregate {agg_ratio}"
    );
}

// -- the fit ---------------------------------------------------------------

/// A cascade validated against one field is refused against another.
#[test]
fn a_cascade_refuses_a_field_it_was_not_built_for() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let seeds = Seeds::new(64, 64).expect("a seed set");
    let emission = Emission::new(64, 64).expect("an emission map");
    let cascade = Cascade::new(base_layout(2), FIELD, FIELD).expect("a sound cascade");
    match target.cascade(&seeds, &emission, &cascade, MergeForm::Aggregate) {
        Err(RenderError::InvalidSize { width, height, .. }) => {
            assert_eq!((width, height), (64, 64));
        }
        other => panic!("a cascade built for another field was accepted: {other:?}"),
    }

    let wrong = Emission::new(32, 32).expect("an emission map");
    let right = Seeds::new(FIELD, FIELD).expect("a seed set");
    match target.cascade(&right, &wrong, &cascade, MergeForm::Aggregate) {
        Err(RenderError::EmissionSizeMismatch { .. }) => {}
        other => panic!("a mismatched emission map was accepted: {other:?}"),
    }
}
