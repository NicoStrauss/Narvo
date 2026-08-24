//! The oracles feedback has and a single cascade cannot.
//!
//! A cascade is a function: one emission map in, one radiance field out, and
//! M8.5a and M8.5b check it as one. A surface cache is a *sequence*, and three
//! things become checkable that were not:
//!
//! - **the step is what it claims to be.** One frame equals a cascade over the
//!   cache's own emission, byte for byte, and the bounced field equals albedo
//!   times that radiance, texel by texel, against a model this file computes
//!   itself. Those two together pin the whole write-back;
//! - **a closed chamber holds its field.** With albedo one and no direct light, a
//!   uniform field is a **fixed point** — every loss is an energy leak and every
//!   gain is energy from nothing. It is an equality and not a bound;
//! - **and it decays monotonically below one.** At albedo one half the same
//!   chamber halves exactly at every step. That is the half a limit alone cannot
//!   report: a sequence that reaches the right limit while overshooting on the
//!   way has a defect the limit hides.
//!
//! # The chamber, derived rather than chosen
//!
//! §3 of M8.6's brief asks for the exact form of the plan's "white chamber with
//! albedo 1 must converge to 1 and must not run away", and the derivation moved
//! the setup rather than the number. **With a direct source present and albedo
//! one, a closed chamber does not converge at all** — it is a perfect resonator
//! being fed, and the field grows without bound, correctly. What converges is a
//! chamber with albedo *below* one; what is *exact* is a chamber with albedo one
//! and **no** direct source, which holds whatever uniform field it is given.
//!
//! Three properties of the arrangement below make the exactness real rather than
//! approximate, and each is a constraint the test had to be built around:
//!
//! 1. **The direction count is a power of two.** A probe's answer is the mean of
//!    `D` copies of `v`, and summing `D` copies of `v` is exact whenever the
//!    partial sums are — which they are for `v = 1` — while dividing by a power
//!    of two only decrements an exponent. This is ADR-0051's rule paying out.
//! 2. **The wall is a ring inset from the field's edge**, so that every wall
//!    texel takes its bounce from a probe that is strictly inside the field. A
//!    probe *on* the boundary has directions whose interval begins outside the
//!    field; those carry the sky, and the sky is not part of the chamber. Insetting
//!    the ring closes the loop over interior probes only, and is why the sky can
//!    be left at zero instead of being made part of the uniform field.
//! 3. **Every direction meets the wall.** The interval reaches further than the
//!    field's diagonal, so no direction ends where it started looking, and a
//!    probe standing *inside* the wall is blocked at its own texel and reads its
//!    own emission — which is `v` like everything else.
//!
//! # Reach, stated per oracle
//!
//! | oracle | exact? | what it cannot see |
//! |---|---|---|
//! | a frame equals a cascade over the cache's emission | **yes** | a defect in the cascade itself, which M8.5a and M8.5b own |
//! | the bounced field is albedo times the radiance | **yes** | nothing in the write-back; it is the write-back's own model |
//! | the chamber holds at albedo one | **yes** | anything uniform: an overlap, a wrong probe index, a swapped channel |
//! | the chamber halves at albedo one half | **yes** | the same, minus a missing or doubled albedo |
//! | the sequence is monotone | a comparison | a defect that decays at the right rate for the wrong reason |
//! | a coloured albedo does not couple the channels | **yes** | a defect symmetric in all three channels |
//! | two runs agree | **yes** | every deterministic defect, which is nearly all of them |

use narvo_render2d::{
    Albedo, Cascade, CascadeLayout, Emission, MergeForm, OffscreenTarget, RadianceField,
    RenderError, Seeds, SurfaceCache,
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

// ------------------------------------------------------------- the chamber --

/// The chamber's field extent.
const CHAMBER: u32 = 48;

/// Texels between level-zero probes in the chamber. A whole number, which
/// `SurfaceCache` requires and `bounce.wgsl` says why.
const CHAMBER_SPACING: u32 = 6;

/// The wall ring's outer and inner bounds, inclusive.
///
/// Inset from the field edge by four texels so that every wall texel's nearest
/// probe has an index in `1..=7` — strictly inside the probe grid, and therefore
/// a probe none of whose directions begin outside the field. The module header
/// carries why that is what makes the chamber closed.
const RING_OUTER: u32 = 4;
const RING_INNER: u32 = 7;

/// Probe indices whose every direction stays inside the field.
///
/// `probe_count(48, 6)` is `floor(48 / 6) + 1 = 9`, so the grid runs `0..=8` and
/// the probes at `0` and `8` sit exactly on the field's edge. Those two are the
/// ones with escaping directions, and nothing the chamber's loop passes through
/// reads them.
const INTERIOR: std::ops::RangeInclusive<u32> = 1..=7;

/// The chamber's cascade: one level, and an interval longer than the diagonal.
///
/// **One level on purpose.** M8.5b measured a single level to be one field on all
/// eight adapter/backend pairs and a composed cascade to be two, so a chamber
/// built on one level tests the *feedback* without the composition's unexplained
/// split standing in front of it. `far` is 72 against a 48 x 48 field's diagonal
/// of 67.9, so no direction reaches its far end before the field's edge clips it
/// and every direction inside the ring meets the ring.
///
/// `D = 512` clears the penumbra inequality's `ceil(2 * pi * 72) = 453` and is a
/// power of two, which is what makes the mean of `D` copies of a value that
/// value again.
fn chamber_cascade() -> Cascade {
    Cascade::new(
        CascadeLayout {
            origin: [0.0, 0.0],
            base_spacing: CHAMBER_SPACING as f32,
            base_interval: 72.0,
            base_directions: 512,
            levels: 1,
            // Zero, and it is never read by any probe this test asserts on — the
            // ring is inset so the loop closes over interior probes. A sky equal
            // to the chamber's own field would make the assertions pass for a
            // second reason, which is exactly what an oracle must not have.
            sky: [0.0, 0.0, 0.0],
        },
        CHAMBER,
        CHAMBER,
    )
    .expect("the chamber's cascade is sound")
}

/// The wall: a ring of occluders three texels thick, inset from the edge.
fn chamber_walls() -> Seeds {
    let mut seeds = Seeds::new(CHAMBER, CHAMBER).expect("a seed set");
    for y in 0..CHAMBER {
        for x in 0..CHAMBER {
            let outer = (RING_OUTER..=CHAMBER - 1 - RING_OUTER).contains(&x)
                && (RING_OUTER..=CHAMBER - 1 - RING_OUTER).contains(&y);
            let inner = (RING_INNER..=CHAMBER - 1 - RING_INNER).contains(&x)
                && (RING_INNER..=CHAMBER - 1 - RING_INNER).contains(&y);
            if outer && !inner {
                seeds.set(x, y).expect("inside the field");
            }
        }
    }
    seeds
}

/// A chamber cache: no direct light, a uniform albedo, and a uniform starting
/// field of `start` in every channel.
fn chamber(target: &OffscreenTarget, albedo: [f32; 3], start: f32) -> SurfaceCache {
    let dark = Emission::new(CHAMBER, CHAMBER).expect("an emission map");
    let reflectance = Albedo::uniform(CHAMBER, CHAMBER, albedo).expect("an albedo map");
    let mut cache = target
        .surface_cache(
            &chamber_walls(),
            &dark,
            &reflectance,
            &chamber_cascade(),
            MergeForm::default(),
        )
        .expect("the chamber's cache");

    let mut seeded = Emission::new(CHAMBER, CHAMBER).expect("an emission map");
    for y in 0..CHAMBER {
        for x in 0..CHAMBER {
            seeded
                .set(x, y, [start, start, start])
                .expect("inside the map");
        }
    }
    cache
        .set_bounced(&seeded)
        .expect("the field is the map's size");
    cache
}

/// Every interior probe's radiance, as a flat list, so an assertion can name the
/// probe it failed at.
fn interior(field: &RadianceField) -> Vec<((u32, u32), [f32; 3])> {
    let mut out = Vec::new();
    for y in INTERIOR {
        for x in INTERIOR {
            out.push((
                (x, y),
                field.radiance(x, y).expect("an interior probe exists"),
            ));
        }
    }
    out
}

// ------------------------------------------------------- the scene oracles --

/// A field with something to be non-uniform about: a wall, a lamp behind it and
/// a scattered dusting, so that no two probes agree by accident.
fn scene() -> (Seeds, Emission, Albedo) {
    const SIZE: u32 = 64;
    let mut seeds = Seeds::new(SIZE, SIZE).expect("a seed set");
    let mut emission = Emission::new(SIZE, SIZE).expect("an emission map");
    let mut albedo = Albedo::new(SIZE, SIZE).expect("an albedo map");

    // A wall down the middle, and a lamp behind it.
    for y in 8..56 {
        seeds.set(28, y).expect("inside the field");
        albedo
            .set(28, y, [0.5, 0.25, 0.125])
            .expect("inside the map");
    }
    for y in 30..34 {
        for x in 44..48 {
            seeds.set(x, y).expect("inside the field");
            emission
                .set(x, y, [1.0, 0.75, 0.5])
                .expect("inside the map");
            albedo.set(x, y, [0.25, 0.5, 0.75]).expect("inside the map");
        }
    }
    // A dusting, so the field is not two flat regions.
    let mut state: u32 = 0x2f6f_2b79;
    for _ in 0..200 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let x = (state >> 8) % SIZE;
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let y = (state >> 8) % SIZE;
        seeds.set(x, y).expect("inside the field");
        albedo.set(x, y, [0.75, 0.5, 0.25]).expect("inside the map");
    }
    (seeds, emission, albedo)
}

/// The scene's cascade: three levels, so the composition is exercised too.
fn scene_cascade() -> Cascade {
    Cascade::new(
        CascadeLayout {
            origin: [0.0, 0.0],
            base_spacing: 4.0,
            base_interval: 2.0,
            base_directions: 32,
            levels: 3,
            sky: [0.0, 0.0, 0.0],
        },
        64,
        64,
    )
    .expect("the scene's cascade is sound")
}

/// The index of the probe nearest `d` texels from the grid origin.
///
/// **The CPU model of `bounce.wgsl`'s `nearest_probe`, written out here rather
/// than shared.** A shared helper would make the oracle and the thing it judges
/// one expression, and then a wrong rounding would agree with itself. This is the
/// same arithmetic derived from the same sentence, and that is the point.
fn nearest_probe(d: i32, spacing: i32, last: i32) -> i32 {
    if d <= 0 {
        return 0;
    }
    ((d + spacing / 2) / spacing).clamp(0, last)
}

// --------------------------------------------------------------- the tests --

/// **A frame is a cascade over the cache's own emission, byte for byte.**
///
/// Both halves of the recurrence, checked against instruments that already
/// existed: the first frame must equal a bare cascade over the direct light,
/// because the bounced field starts at zero; the second must equal a bare cascade
/// over `direct + bounced`, which is what the write-back produced in between.
///
/// The second half is what makes this an oracle for the *feedback* rather than
/// for the cascade: a cache that ran the write-back and then marched the old
/// emission again would pass the first and fail the second.
#[test]
fn a_frame_is_a_cascade_over_the_caches_own_emission() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();

    for form in [MergeForm::Aggregate, MergeForm::Directional] {
        let mut cache = target
            .surface_cache(&seeds, &emission, &albedo, &cascade, form)
            .expect("a cache");
        assert_eq!(cache.frames(), 0, "a fresh cache has run no frame");

        let first = target.bounce(&mut cache).expect("one frame");
        let bare = target
            .cascade(&seeds, &emission, &cascade, form)
            .expect("a bare cascade");
        assert_eq!(
            first.texels(),
            bare.texels(),
            "{form:?}: the first frame is not the cascade over the direct light, \
             so a cache that has never bounced is a different renderer"
        );
        assert_eq!(cache.frames(), 1);

        // What bounced becomes the next frame's emission, so a bare cascade over
        // `direct + bounced` has to reproduce the next frame exactly.
        let bounced = cache.bounced().expect("the bounced field reads back");
        let mut fed = Emission::new(64, 64).expect("an emission map");
        for y in 0..64 {
            for x in 0..64 {
                let d = emission.get(x, y).expect("inside the map");
                let b = bounced.get(x, y).expect("inside the map");
                fed.set(x, y, [d[0] + b[0], d[1] + b[1], d[2] + b[2]])
                    .expect("a sum of two non-negative maps");
            }
        }
        let second = target.bounce(&mut cache).expect("a second frame");
        let expected = target
            .cascade(&seeds, &fed, &cascade, form)
            .expect("a bare cascade");
        assert_eq!(
            second.texels(),
            expected.texels(),
            "{form:?}: the second frame did not march what the first frame lit"
        );
        assert_eq!(cache.frames(), 2);
    }
}

/// **The bounced field is albedo times the radiance, texel by texel.**
///
/// The write-back's own model, computed here from the radiance the frame handed
/// back and the albedo map the caller built — two things this test has
/// independently of the GPU. Exact equality, because every step of it is a single
/// correctly-rounded multiplication.
///
/// This is the oracle with the largest reach in the file: it sees a missing
/// albedo, a doubled one, a channel swapped into another, a wrong probe index and
/// a write-back that read the wrong texture. What it cannot see is a wrong
/// *radiance*, which is M8.5a's and M8.5b's to own.
#[test]
fn the_bounced_field_is_the_albedo_times_the_radiance() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();
    let spacing = 4_i32;
    let probes = cascade.level(0).expect("level zero exists").layout().probes;

    let mut cache = target
        .surface_cache(&seeds, &emission, &albedo, &cascade, MergeForm::default())
        .expect("a cache");
    let radiance = target.bounce(&mut cache).expect("one frame");
    let bounced = cache.bounced().expect("the bounced field reads back");

    let mut checked = 0_u32;
    for y in 0..64 {
        for x in 0..64 {
            let px = nearest_probe(x as i32, spacing, probes[0] as i32 - 1);
            let py = nearest_probe(y as i32, spacing, probes[1] as i32 - 1);
            let r = radiance
                .radiance(px as u32, py as u32)
                .expect("the probe is inside the grid");
            let a = albedo.get(x, y).expect("inside the map");
            let expected = [a[0] * r[0], a[1] * r[1], a[2] * r[2]];
            let actual = bounced.get(x, y).expect("inside the map");
            assert_eq!(
                actual, expected,
                "texel ({x}, {y}) took albedo {a:?} against probe ({px}, {py}) = {r:?}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 64 * 64, "every texel was checked");

    // And the model could have failed: a scene where every product is zero would
    // agree with a write-back that does nothing at all.
    assert!(
        (0..64).any(|y| (0..64).any(|x| bounced
            .get(x, y)
            .expect("inside the map")
            .iter()
            .any(|c| *c > 0.0))),
        "the scene bounced nothing, so this oracle proved nothing"
    );
}

/// **The fixed point: a closed chamber with albedo one holds its field, exactly.**
///
/// The strongest oracle of the block, in the form §3's derivation gives it rather
/// than the form the plan words it in. No direct light, albedo one, a uniform
/// starting field of `1.0`: every interior probe must read exactly `1.0` at every
/// frame, for as many frames as are run.
///
/// Any loss is an energy leak. Any gain is energy from nothing. There is no
/// tolerance, and there is deliberately no limit being approached — the module
/// header carries why the plan's "converges to 1" describes a setup that, with a
/// source present, does not converge at all.
#[test]
fn a_closed_chamber_at_albedo_one_holds_its_field_exactly() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let mut cache = chamber(&target, [1.0, 1.0, 1.0], 1.0);

    for frame in 1..=4_u32 {
        let radiance = target.bounce(&mut cache).expect("one frame");
        for (probe, rgb) in interior(&radiance) {
            assert_eq!(
                rgb,
                [1.0, 1.0, 1.0],
                "frame {frame}, probe {probe:?}: the chamber did not hold its field. \
                 Below one is an energy leak; above one is energy from nothing"
            );
        }
        assert_eq!(cache.frames(), frame);
    }
}

/// **And below one it halves, exactly and monotonically.**
///
/// Albedo one half, and the sequence is `1, 1/2, 1/4, 1/8` — every value a power
/// of two, so the equality is exact rather than a bound.
///
/// **The whole sequence is asserted and not only its limit**, which is the
/// sharper half: a write-back that applied the albedo twice would converge to the
/// same zero from the same start, and only the rate says so. Monotonicity is
/// asserted beside the values, so a version that reached each value by
/// overshooting would still be caught if the values themselves ever changed.
#[test]
fn a_closed_chamber_below_albedo_one_halves_exactly_and_monotonically() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let mut cache = chamber(&target, [0.5, 0.5, 0.5], 1.0);

    let mut previous = f32::INFINITY;
    for (frame, expected) in [1.0_f32, 0.5, 0.25, 0.125].into_iter().enumerate() {
        let radiance = target.bounce(&mut cache).expect("one frame");
        for (probe, rgb) in interior(&radiance) {
            assert_eq!(
                rgb,
                [expected, expected, expected],
                "frame {}, probe {probe:?}: the chamber did not halve. A rate that is \
                 wrong while the limit is right is exactly what a limit cannot report",
                frame + 1
            );
        }
        assert!(
            expected < previous,
            "frame {}: {expected} did not fall below {previous}, so the sequence is \
             not monotone",
            frame + 1
        );
        previous = expected;
    }
}

/// **Colour is what happens when albedo has three channels, and nothing else.**
///
/// A chamber whose walls reflect `[1, 1/2, 1/4]`, starting from white. Each
/// channel must follow its own geometric sequence and no other: red holds, green
/// halves, blue quarters. A kernel that coupled one channel into another — a
/// plausible picture and a wrong field — produces a sequence none of the three
/// follows.
#[test]
fn a_coloured_albedo_does_not_couple_the_channels() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let mut cache = chamber(&target, [1.0, 0.5, 0.25], 1.0);

    let mut expected = [1.0_f32, 1.0, 1.0];
    for frame in 1..=4_u32 {
        let radiance = target.bounce(&mut cache).expect("one frame");
        for (probe, rgb) in interior(&radiance) {
            assert_eq!(
                rgb, expected,
                "frame {frame}, probe {probe:?}: the channels did not stay apart"
            );
        }
        expected = [expected[0], expected[1] * 0.5, expected[2] * 0.25];
    }

    // The three sequences did part, so a defect that merged them could have been
    // seen. Without this the test would pass on a field that was grey throughout.
    assert!(
        expected[0] > expected[1] && expected[1] > expected[2],
        "the three channels never separated, so this oracle proved nothing"
    );
}

/// **A cache that absorbs everything bounces nothing, at every frame.**
///
/// The control for the two chambers above: albedo zero is the one value for which
/// the answer is knowable without running a cascade at all, and it must be
/// reached exactly rather than approached. It is also the shape of the injection
/// that removes the albedo from the write-back — that defect makes this test
/// return the radiance instead of zero.
#[test]
fn a_black_chamber_bounces_nothing() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let mut cache = chamber(&target, [0.0, 0.0, 0.0], 1.0);

    let first = target.bounce(&mut cache).expect("one frame");
    for (probe, rgb) in interior(&first) {
        assert_eq!(
            rgb,
            [1.0, 1.0, 1.0],
            "probe {probe:?}: the seeded field is what the first frame marches"
        );
    }
    for frame in 2..=3_u32 {
        let radiance = target.bounce(&mut cache).expect("one frame");
        for (probe, rgb) in interior(&radiance) {
            assert_eq!(
                rgb,
                [0.0, 0.0, 0.0],
                "frame {frame}, probe {probe:?}: a surface that absorbs everything \
                 still returned light"
            );
        }
    }
}

/// **Two caches over one scene agree byte for byte, at every frame.**
///
/// The narrowest oracle here and the only one that would see a race: every other
/// defect in this file is deterministic and would be reproduced identically by
/// both runs. It is also the only in-tree check that a cache is stable at all
/// under repeated stepping.
#[test]
fn two_runs_of_one_cache_agree_byte_for_byte() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();

    let mut left = target
        .surface_cache(&seeds, &emission, &albedo, &cascade, MergeForm::default())
        .expect("a cache");
    let mut right = target
        .surface_cache(&seeds, &emission, &albedo, &cascade, MergeForm::default())
        .expect("a cache");

    for frame in 1..=3_u32 {
        let a = target.bounce(&mut left).expect("one frame");
        let b = target.bounce(&mut right).expect("one frame");
        assert_eq!(
            a.texels(),
            b.texels(),
            "frame {frame}: two caches over one scene parted"
        );
        let (a, b) = (
            left.bounced().expect("reads back"),
            right.bounced().expect("reads back"),
        );
        for y in 0..64 {
            for x in 0..64 {
                assert_eq!(
                    a.get(x, y),
                    b.get(x, y),
                    "frame {frame}: the two bounced fields parted at ({x}, {y})"
                );
            }
        }
    }
}

/// A map of the wrong size is refused, naming which of the two it was.
///
/// The emission and the albedo are separate refusals on purpose: a cache takes
/// both, and a message naming the wrong one would send a reader to the wrong
/// argument of the same call.
#[test]
fn a_map_that_is_not_the_seed_sets_size_is_refused() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();

    let small = Emission::new(32, 64).expect("an emission map");
    let error = target
        .surface_cache(&seeds, &small, &albedo, &cascade, MergeForm::default())
        .expect_err("a mismatched emission map is refused");
    assert!(
        matches!(
            error,
            RenderError::EmissionSizeMismatch {
                emission_width: 32,
                ..
            }
        ),
        "the refusal did not name the emission map: {error}"
    );

    let narrow = Albedo::new(64, 32).expect("an albedo map");
    let error = target
        .surface_cache(&seeds, &emission, &narrow, &cascade, MergeForm::default())
        .expect_err("a mismatched albedo map is refused");
    assert!(
        matches!(
            error,
            RenderError::AlbedoSizeMismatch {
                albedo_height: 32,
                ..
            }
        ),
        "the refusal did not name the albedo map: {error}"
    );
}

/// A cascade whose probe grid cannot be indexed in integers is refused.
///
/// ADR-0050 reaching an index, and the refusal happens when the cache is built
/// rather than when a texel takes a wrong probe.
#[test]
fn a_cache_over_a_fractional_probe_grid_is_refused() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let fractional = Cascade::new(
        CascadeLayout {
            origin: [0.0, 0.0],
            base_spacing: 4.5,
            base_interval: 2.0,
            base_directions: 32,
            levels: 3,
            sky: [0.0, 0.0, 0.0],
        },
        64,
        64,
    )
    .expect("a cascade with a fractional spacing is still a cascade");

    let error = target
        .surface_cache(
            &seeds,
            &emission,
            &albedo,
            &fractional,
            MergeForm::default(),
        )
        .expect_err("a fractional spacing is refused");
    assert!(
        matches!(
            error,
            RenderError::ProbeGridNotIntegral {
                name: "the level-zero probe spacing",
                ..
            }
        ),
        "the refusal did not name the spacing: {error}"
    );
}

/// The cache reports the merge it was built with, and `MergeForm`'s default is
/// D33's decision rather than whichever variant is written first.
#[test]
fn a_cache_runs_the_merge_it_was_given_and_the_default_is_directional() {
    assert_eq!(
        MergeForm::default(),
        MergeForm::Directional,
        "D33 decided the directional merge; a caller with no opinion must get it"
    );

    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();
    for form in [MergeForm::Aggregate, MergeForm::Directional] {
        let cache = target
            .surface_cache(&seeds, &emission, &albedo, &cascade, form)
            .expect("a cache");
        assert_eq!(cache.form(), form);
        assert_eq!(cache.cascade().level_count(), 3);
    }
}
