//! The oracles for one cascade stage.
//!
//! An integration test rather than a unit test in `src/`, because everything
//! these need is public: a stage is built from [`StageLayout`], run through
//! [`OffscreenTarget::cascade_stage`] and read out of [`RadianceField`]. What
//! stayed in `cascade.rs` is the half that needs no GPU — the parameter
//! validation and the two source reads.
//!
//! # What each oracle can and cannot see
//!
//! M8.4 measured that four independent oracles can still be **unequal**: one of
//! them could not see a sign error in principle. Independence and reach are two
//! axes, and the reach of each of these is written on it. In short:
//!
//! | oracle | exact? | what it cannot see |
//! |---|---|---|
//! | (a) every probe alike | yes | anything that is wrong the same way everywhere — including a stage that never marched |
//! | (a) the value is the analytic one | no, a bound | a defect smaller than the summation's own rounding |
//! | (b) an enclosed probe reads zero | yes | the difference between blocked, exhausted, and never marched at all |
//! | (c) the law over distance | no, a ratio | anything that scales every probe by one factor |
//! | interval respected, both ends | yes | a ray that is short in the *right* direction |
//! | (d) two runs agree | yes | anything deterministic, which is every defect here |
//!
//! (a) and (b) are the pair that matters: (a) alone passes a stage that returns
//! the far radiance without marching, and (b) alone passes a stage whose hit
//! buffer was never written. Neither is redundant with the other and neither is
//! sufficient.

use narvo_render2d::{
    CascadeStage, Emission, OffscreenTarget, RadianceField, RenderError, Seeds, StageLayout,
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

/// An empty seed set and an emission map of the same size.
fn empty_scene(width: u32, height: u32) -> (Seeds, Emission) {
    (
        Seeds::new(width, height).expect("a seed set"),
        Emission::new(width, height).expect("an emission map"),
    )
}

/// Marks `points` as occluders emitting `rgb`.
fn occlude(seeds: &mut Seeds, emission: &mut Emission, points: &[(u32, u32)], rgb: [f32; 3]) {
    for &(x, y) in points {
        seeds.set(x, y).expect("inside the field");
        emission.set(x, y, rgb).expect("inside the map");
    }
}

/// The texels of probe `(x, y)`, radiance then escaped fraction.
fn probe(field: &RadianceField, x: u32, y: u32) -> [f32; 4] {
    let rgb = field.radiance(x, y).expect("a probe inside the grid");
    let escaped = field.escaped(x, y).expect("a probe inside the grid");
    [rgb[0], rgb[1], rgb[2], escaped]
}

// -- (a) the white chamber ------------------------------------------------

/// **Oracle (a), first claim: every probe reads the same value, exactly.**
///
/// A surround that emits the same everywhere with nothing in the way is
/// symmetric under translation, so every probe has to agree. Here the claim is
/// stronger than symmetry and that is worth saying plainly: every probe runs the
/// *identical* sequence of identical `f32` operations, so the agreement is
/// bitwise rather than approximate, and the assertion is bitwise.
///
/// **Reach.** This oracle sees nothing that is wrong the same way at every
/// probe. A stage that ignored the hit buffer entirely and returned the far
/// radiance would pass it — which is exactly injection J4, and why (b) exists.
#[test]
fn every_probe_in_a_white_chamber_reads_the_same_value() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission) = empty_scene(64, 64);
    let stage = CascadeStage::new(StageLayout {
        origin: [2.0, 2.0],
        spacing: 4.0,
        probes: [8, 8],
        near: 0.0,
        far: 4.0,
        directions: 32,
        far_radiance: [0.1, 0.3, 0.7],
    })
    .expect("a sound layout");

    let field = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs");
    assert_eq!((field.width(), field.height()), (8, 8));

    let first = probe(&field, 0, 0);
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(
                probe(&field, x, y).map(f32::to_bits),
                first.map(f32::to_bits),
                "probe ({x}, {y}) disagreed with probe (0, 0) in a chamber that is \
                 the same in every direction from every point"
            );
        }
    }
    assert_eq!(
        first[3], 1.0,
        "a chamber with no occluders in it blocked something"
    );
}

/// **Oracle (a), second claim: the value is the analytic one — and this half is
/// a bound, not an equality.**
///
/// The two claims are separated because only the first is exact. The second is a
/// quadrature over finitely many directions plus a sum of finitely many `f32`,
/// and each half contributes its own error:
///
/// - **The quadrature.** The `D`-direction mean is the rectangle rule on the
///   circle. For a surround with `|dL/dtheta| <= K` the error is at most
///   `pi * K / (2 * D)` — integrate the difference from each direction over the
///   arc it stands for and the `D` arcs sum to that. For a white chamber `K` is
///   zero, so **the quadrature is exact** and contributes nothing. That is why
///   the chamber is where the other half can be seen on its own.
/// - **The summation.** Adding `D` copies of `E` and dividing by `D` does *not*
///   give `E` back. The classical bound is `(D - 1) * u` relative, with
///   `u = 2^-24`; at `D = 64` that is `3.755e-6`. Measured over 200 000 random
///   values at `D = 64`, the worst gap was **16 ULP** and 70 % of values did not
///   come back exactly — so the bound is loose, and the effect is real.
///
/// The exact case is kept beside it and is not a special case of the bound: when
/// `E` is dyadic every partial sum is representable, so the answer is `E` to the
/// bit. A test that only checked the bound would pass a stage that had lost a
/// bit everywhere.
#[test]
fn a_white_chamber_reads_its_surround_within_the_summation_bound() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission) = empty_scene(64, 64);
    let layout = StageLayout {
        origin: [8.0, 8.0],
        spacing: 8.0,
        probes: [4, 4],
        near: 0.0,
        far: 4.0,
        directions: 64,
        far_radiance: [0.25, 0.5, 0.125],
    };

    // Dyadic: every partial sum of 64 copies is representable, so the mean is
    // the surround to the bit.
    let stage = CascadeStage::new(layout).expect("a sound layout");
    let field = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs");
    assert_eq!(
        field.radiance(2, 2).expect("a probe"),
        [0.25, 0.5, 0.125],
        "a dyadic surround did not come back exactly, so the sum lost a bit that \
         nothing in the arithmetic can explain"
    );

    // Not dyadic: the sum rounds, and the bound is (D - 1) * u relative.
    let surround = [0.1_f32, 0.3, 0.7];
    let stage = CascadeStage::new(StageLayout {
        far_radiance: surround,
        ..layout
    })
    .expect("a sound layout");
    let field = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs");
    let read = field.radiance(2, 2).expect("a probe");
    let bound = (64.0 - 1.0) * (f32::EPSILON / 2.0);
    for channel in 0..3 {
        let error = (read[channel] - surround[channel]).abs();
        assert!(
            error <= bound * surround[channel],
            "channel {channel} came back as {} against a surround of {}, which is \
             {error} - beyond the summation bound of {}",
            read[channel],
            surround[channel],
            bound * surround[channel]
        );
    }
}

// -- (b) an enclosed probe ------------------------------------------------

/// **Oracle (b): a probe walled in by things that do not glow reads zero,
/// exactly.**
///
/// The cheapest guard here and the one that cannot be argued with: every
/// direction meets an occluder whose emission is zero, so every summand is zero
/// and the sum is zero however it is ordered. The far radiance is deliberately
/// **not** zero, so a stage that answered without marching would read `1.0` and
/// be caught.
///
/// **Reach, and it is the narrow one.** This cannot tell blocked from exhausted
/// from never-marched-at-all: all three contribute nothing. It is paired with
/// `escaped`, which it also pins at zero — that is what separates "every ray
/// stopped" from "every ray was reported visible", and it is the half that
/// catches J4.
#[test]
fn a_probe_enclosed_by_dark_occluders_reads_zero() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene(32, 32);
    // A closed square ring six texels out from the probe, two texels thick so
    // no diagonal can thread it.
    let mut wall = Vec::new();
    for offset in -7_i32..=7 {
        for thickness in 0..2 {
            let far = 6 + thickness;
            for (x, y) in [
                (16 + offset, 16 - far),
                (16 + offset, 16 + far),
                (16 - far, 16 + offset),
                (16 + far, 16 + offset),
            ] {
                if (0..32).contains(&x) && (0..32).contains(&y) {
                    wall.push((x as u32, y as u32));
                }
            }
        }
    }
    occlude(&mut seeds, &mut emission, &wall, [0.0, 0.0, 0.0]);

    let stage = CascadeStage::new(StageLayout {
        origin: [16.5, 16.5],
        spacing: 1.0,
        probes: [1, 1],
        near: 0.0,
        far: 10.0,
        directions: 64,
        // Not zero: a stage that skipped the march would read this.
        far_radiance: [1.0, 1.0, 1.0],
    })
    .expect("a sound layout");

    let field = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs");
    assert_eq!(
        probe(&field, 0, 0),
        [0.0, 0.0, 0.0, 0.0],
        "a probe walled in by unlit occluders read something. A radiance above \
         zero means a direction escaped the enclosure; an escaped fraction above \
         zero means a direction was reported visible without having got out"
    );
}

/// The same enclosure, lit: the probe reads what the walls emit.
///
/// The counterpart that stops oracle (b) from being satisfiable by a stage that
/// returns zero unconditionally — which is a defect (b) is otherwise blind to,
/// and it costs one more call to say so.
#[test]
fn the_same_enclosure_lit_reads_what_the_walls_emit() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene(32, 32);
    let mut wall = Vec::new();
    for offset in -7_i32..=7 {
        for thickness in 0..2 {
            let far = 6 + thickness;
            for (x, y) in [
                (16 + offset, 16 - far),
                (16 + offset, 16 + far),
                (16 - far, 16 + offset),
                (16 + far, 16 + offset),
            ] {
                if (0..32).contains(&x) && (0..32).contains(&y) {
                    wall.push((x as u32, y as u32));
                }
            }
        }
    }
    occlude(&mut seeds, &mut emission, &wall, [0.25, 0.5, 0.125]);

    let stage = CascadeStage::new(StageLayout {
        origin: [16.5, 16.5],
        spacing: 1.0,
        probes: [1, 1],
        near: 0.0,
        far: 10.0,
        directions: 64,
        far_radiance: [1.0, 1.0, 1.0],
    })
    .expect("a sound layout");

    let field = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs");
    // Dyadic emission and a fully enclosed probe: every summand is the same
    // representable value, so this is exact for the reason the white chamber is.
    assert_eq!(
        probe(&field, 0, 0),
        [0.25, 0.5, 0.125, 0.0],
        "an enclosure that emits was not read at the seeds that emit it. A zero \
         here means the emission was sampled at the stopping point instead of at \
         the seed, which is up to a whole texel in front of it"
    );
}

// -- the interval ---------------------------------------------------------

/// **A probe does not see past its far end.**
///
/// The same lamp, once inside the interval and once beyond it. With the far
/// radiance at zero, a probe that reads anything at all beyond its far end has
/// marched further than it was told to — which is injection J1, and in a
/// hierarchy it would be counted twice.
#[test]
fn a_probe_reads_nothing_beyond_its_far_end() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let lamp: Vec<(u32, u32)> = (60..69).map(|y| (40_u32, y as u32)).collect();

    let read_with = |far: f32, directions: u32| {
        let (mut seeds, mut emission) = empty_scene(128, 128);
        occlude(&mut seeds, &mut emission, &lamp, [1.0, 1.0, 1.0]);
        let stage = CascadeStage::new(StageLayout {
            origin: [16.5, 64.5],
            spacing: 1.0,
            probes: [1, 1],
            near: 0.0,
            far,
            directions,
            far_radiance: [0.0, 0.0, 0.0],
        })
        .expect("a sound layout");
        let field = target
            .cascade_stage(&seeds, &emission, &stage)
            .expect("the stage runs");
        probe(&field, 0, 0)
    };

    // The lamp stands 23.5 texels away. An interval stopping at 16 cannot reach
    // it; one reaching 32 can.
    let short = read_with(16.0, 128);
    let long = read_with(32.0, 256);
    assert_eq!(
        [short[0], short[1], short[2]],
        [0.0, 0.0, 0.0],
        "a probe whose interval stops at 16 texels read a lamp 23.5 texels away"
    );
    assert!(
        long[0] > 0.0,
        "a probe whose interval reaches 32 texels read nothing from a lamp 23.5 \
         texels away, so the two halves of this oracle are not comparing the same \
         thing"
    );
    assert_eq!(
        short[3], 1.0,
        "every direction of the short interval should have escaped it"
    );
}

/// **A probe does not see inside its near end.**
///
/// The other half of the interval, and the one a hierarchy needs: a level starts
/// where the level below it stopped, and a probe that reached back inside its
/// near end would count what the level below already counted.
#[test]
fn a_probe_reads_nothing_inside_its_near_end() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene(128, 128);
    // A ring of lamps eight texels from the probe.
    let lamps: Vec<(u32, u32)> = (0..64)
        .map(|k| {
            let theta = std::f64::consts::TAU * f64::from(k) / 64.0;
            (
                (64.5 + 8.0 * theta.cos()).round() as u32,
                (64.5 + 8.0 * theta.sin()).round() as u32,
            )
        })
        .collect();
    occlude(&mut seeds, &mut emission, &lamps, [1.0, 1.0, 1.0]);

    let run = |near: f32| {
        let stage = CascadeStage::new(StageLayout {
            origin: [64.5, 64.5],
            spacing: 1.0,
            probes: [1, 1],
            near,
            far: 24.0,
            directions: 256,
            far_radiance: [0.0, 0.0, 0.0],
        })
        .expect("a sound layout");
        let field = target
            .cascade_stage(&seeds, &emission, &stage)
            .expect("the stage runs");
        probe(&field, 0, 0)
    };

    let from_zero = run(0.0);
    let from_beyond = run(16.0);
    assert!(
        from_zero[0] > 0.5,
        "a probe starting at zero did not see a ring of lamps eight texels out"
    );
    assert_eq!(
        [from_beyond[0], from_beyond[1], from_beyond[2]],
        [0.0, 0.0, 0.0],
        "a probe whose interval starts at 16 texels read a ring of lamps at 8"
    );
    assert_eq!(
        from_beyond[3], 1.0,
        "every direction starting outside the ring should have escaped"
    );
}

/// A degenerate interval answers about the probe's own texel and nothing else.
///
/// `near == far == 0` builds zero-length rays, which is legal: the march reads
/// the texel the probe stands in. A probe standing on a lamp reads it; a probe
/// standing on nothing reads the far radiance, because a zero-length ray that
/// meets nothing has reached its far end.
#[test]
fn a_degenerate_interval_answers_about_the_probe_s_own_texel() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene(32, 32);
    occlude(&mut seeds, &mut emission, &[(4, 4)], [0.5, 0.25, 0.125]);

    let run = |origin: [f32; 2]| {
        let stage = CascadeStage::new(StageLayout {
            origin,
            spacing: 1.0,
            probes: [1, 1],
            near: 0.0,
            far: 0.0,
            directions: 1,
            far_radiance: [1.0, 1.0, 1.0],
        })
        .expect("a sound layout");
        let field = target
            .cascade_stage(&seeds, &emission, &stage)
            .expect("the stage runs");
        probe(&field, 0, 0)
    };

    assert_eq!(
        run([4.5, 4.5]),
        [0.5, 0.25, 0.125, 0.0],
        "a probe standing on a lamp with a zero-length interval did not read it"
    );
    assert_eq!(
        run([20.5, 20.5]),
        [1.0, 1.0, 1.0, 1.0],
        "a probe standing on nothing with a zero-length interval did not reach \
         its far end"
    );
}

// -- the directions themselves --------------------------------------------

/// **A half-plane blocks the half of the directions that point at it.**
///
/// Written because the other oracles are all blind to one thing: whether the
/// directions are *uniformly* spaced. A white chamber gives every direction the
/// same summand, so it reads the same however they are distributed; an enclosure
/// blocks all of them; the distance law is a ratio. Skew the directions and none
/// of those notices — which is injection J2, and this is the oracle written for
/// it.
///
/// # The number, derived
///
/// The probe stands `d` texels above a half-plane of occluders, with an interval
/// reaching `far`. A direction at angle `theta` below the horizontal reaches
/// depth `far * sin(theta)`, and the march stops it once that is within a texel
/// of the wall — so it blocks when `sin(theta) >= s` with `s = (d - 1) / far`.
/// The directions that do are the arc from `arcsin(s)` to `pi - arcsin(s)`, so
///
/// ```text
///     escaped = 1 - (pi - 2 * arcsin(s)) / (2 * pi)
///             = 1/2 + arcsin(s) / pi
/// ```
///
/// which is a little over a half, and the excess is the shallow directions that
/// run out of interval before they reach the wall. With `d = 5.5` and
/// `far = 64`, `s = 0.0703` and the prediction is **0.5224**.
///
/// The tolerance is four directions of 512 either way — the margin is "within a
/// texel" rather than exactly one texel, and the ray's position is quantised to
/// 1/256 of a texel, so the two directions nearest the threshold are the ones
/// this cannot call in advance.
#[test]
fn a_half_plane_blocks_half_the_directions() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene(128, 128);
    let wall: Vec<(u32, u32)> = (0..128)
        .flat_map(|x| (70..128).map(move |y| (x, y)))
        .collect();
    occlude(&mut seeds, &mut emission, &wall, [0.0, 0.0, 0.0]);

    let stage = CascadeStage::new(StageLayout {
        origin: [64.5, 64.5],
        spacing: 1.0,
        probes: [1, 1],
        near: 0.0,
        far: 64.0,
        directions: 512,
        far_radiance: [1.0, 1.0, 1.0],
    })
    .expect("a sound layout");

    let field = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs");
    let escaped = f64::from(field.escaped(0, 0).expect("a probe"));

    let s: f64 = (70.0 - 64.5 - 1.0) / 64.0;
    let expected = 0.5 + s.asin() / std::f64::consts::PI;
    let tolerance = 4.0 / 512.0;
    assert!(
        (escaped - expected).abs() <= tolerance,
        "a half-plane let {escaped} of the directions past it, against a derived \
         {expected}. A number far from a half means the directions are not evenly \
         spaced around the circle"
    );
}

// -- (c) the law over distance --------------------------------------------

/// **Oracle (c): what a probe with an interval measures falls as 1/r, and the
/// derivation is what says so — not the plan.**
///
/// # The derivation
///
/// A probe's answer is a **mean over directions**, not a flux over a
/// circumference. A wall of half-height `h` at distance `r`, seen end-on,
/// subtends `alpha = 2 * atan(h / r)`. Of `D` uniformly spaced directions,
/// `D * alpha / (2 * pi)` land on it, each carrying the wall's radiance `L`. So
///
/// ```text
///     mean = L * alpha / (2 * pi) = L * atan(h / r) / pi
///          -> L * h / (pi * r)    for r >> h
/// ```
///
/// **1/r, and it is 1/r because the angular size falls as 1/r** — not because of
/// any inverse-square-in-the-plane argument. The plan's warning against 1/r² is
/// right and its reason arrives at the right exponent by a different route; the
/// two stop agreeing the moment the source is not a point, which is why the
/// derivation is written out here and the law is checked as a *ratio*.
///
/// # What the interval does to it, which is the finding
///
/// The law holds **only inside the interval**. Below the near end the directions
/// start past the wall and read zero; beyond the far end they stop short and
/// read the far radiance. So a probe with an interval does not measure 1/r over
/// `r` at all — it measures 1/r on an annulus and something else outside it,
/// and the two tests above pin exactly that. A cascade level *is* that annulus.
///
/// # Why this is a ratio and not an absolute value
///
/// Two reasons, both derived. The march stops within one texel of what stopped
/// it (`march.wgsl`'s margin), so the wall behaves as though it were about a
/// texel taller at each end — a systematic offset in `h` that no tolerance
/// should be asked to absorb. And the count of directions landing on the wall is
/// an **integer**, so the reading is quantised in steps of `L / D`: at `r = 64`
/// with `D = 1024` the wall takes about 25 directions, so one direction either
/// way is 4 %. Both cancel in `mean * r` compared across radii.
///
/// The assertion is that `mean * r` is constant to within 15 %, and that
/// discriminates hugely: under 1/r² it would fall by a factor of four between
/// each pair of radii.
#[test]
fn what_a_probe_with_an_interval_measures_falls_as_one_over_r() {
    let Some(target) = target_or_skip() else {
        return;
    };
    // A wall nine texels tall at x = 128, seen from the left.
    let wall: Vec<(u32, u32)> = (124..133).map(|y| (128_u32, y)).collect();

    let read_at = |r: u32| {
        let (mut seeds, mut emission) = empty_scene(256, 256);
        occlude(&mut seeds, &mut emission, &wall, [1.0, 1.0, 1.0]);
        let stage = CascadeStage::new(StageLayout {
            origin: [128.5 - r as f32, 128.5],
            spacing: 1.0,
            probes: [1, 1],
            near: 0.0,
            far: 96.0,
            directions: 1024,
            far_radiance: [0.0, 0.0, 0.0],
        })
        .expect("a sound layout");
        let field = target
            .cascade_stage(&seeds, &emission, &stage)
            .expect("the stage runs");
        f64::from(field.radiance(0, 0).expect("a probe")[0])
    };

    let radii = [16_u32, 32, 64];
    let products: Vec<f64> = radii.iter().map(|&r| read_at(r) * f64::from(r)).collect();
    for (r, product) in radii.iter().zip(&products) {
        assert!(
            *product > 0.0,
            "the probe at r = {r} read nothing from a wall inside its interval"
        );
    }

    let lowest = products.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = products.iter().copied().fold(0.0_f64, f64::max);
    assert!(
        highest <= lowest * 1.15,
        "mean * r is not constant across radii: {products:?}. Under 1/r it varies \
         only by quantisation and the march's margin; under 1/r^2 it would fall \
         by four between each pair"
    );

    // And the discrimination, stated rather than implied: a 1/r^2 law would put
    // the r = 64 product at a quarter of the r = 32 one.
    let inverse_square_would_be = products[1] / 4.0;
    assert!(
        products[2] > inverse_square_would_be * 2.0,
        "the reading at r = 64 is consistent with an inverse-square law, which \
         this geometry cannot produce: {products:?}"
    );
}

// -- (d) determinism ------------------------------------------------------

/// **Oracle (d): two runs of one stage agree, byte for byte.**
///
/// Within one machine this is what "deterministic and hashable" means, and it is
/// exact. **Across machines it is a claim this test cannot make**: a test sees
/// only the platform it runs on. That half was measured outside the tree, on
/// eight adapter/backend pairs in both profiles — one digest in 32 of 32 cells —
/// and `shaders/cascade.wgsl`'s header carries what holds it there.
#[test]
fn two_runs_of_one_stage_agree_byte_for_byte() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (mut seeds, mut emission) = empty_scene(64, 64);
    let occluders: Vec<(u32, u32)> = (0..64)
        .map(|k: u32| ((k * 7 + 3) % 64, (k * 13 + 11) % 64))
        .collect();
    occlude(&mut seeds, &mut emission, &occluders, [0.1, 0.3, 0.7]);

    let stage = CascadeStage::new(StageLayout {
        origin: [4.0, 4.0],
        spacing: 4.0,
        probes: [14, 14],
        near: 0.0,
        far: 6.0,
        directions: 64,
        far_radiance: [0.05, 0.05, 0.05],
    })
    .expect("a sound layout");

    let first = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs");
    let second = target
        .cascade_stage(&seeds, &emission, &stage)
        .expect("the stage runs again");
    assert_eq!(
        first.texels(),
        second.texels(),
        "two runs of one stage over one world disagreed"
    );
    assert!(
        first.texels().iter().any(|value| *value > 0.0),
        "the scene this compares is blank, so the comparison says nothing"
    );
}

// -- the fit --------------------------------------------------------------

/// A stage and an emission map that do not go together are refused.
#[test]
fn a_stage_refuses_inputs_that_do_not_go_together() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let seeds = Seeds::new(32, 32).expect("a seed set");
    let wrong = Emission::new(16, 16).expect("an emission map");
    let stage = CascadeStage::new(StageLayout {
        origin: [2.0, 2.0],
        spacing: 4.0,
        probes: [4, 4],
        near: 0.0,
        far: 4.0,
        directions: 32,
        far_radiance: [0.0, 0.0, 0.0],
    })
    .expect("a sound layout");

    match target.cascade_stage(&seeds, &wrong, &stage) {
        Err(RenderError::EmissionSizeMismatch {
            seed_width,
            emission_width,
            ..
        }) => assert_eq!((seed_width, emission_width), (32, 16)),
        other => panic!("a mismatched emission map was accepted: {other:?}"),
    }

    let right = Emission::new(32, 32).expect("an emission map");
    let outside = CascadeStage::new(StageLayout {
        origin: [2.0, 2.0],
        spacing: 4.0,
        probes: [4, 4],
        near: 4.0,
        far: 8.0,
        directions: 64,
        far_radiance: [0.0, 0.0, 0.0],
    })
    .expect("a sound layout");
    match target.cascade_stage(&seeds, &right, &outside) {
        Err(RenderError::ProbeOutsideField { .. }) => {}
        other => panic!("a probe standing inside its own near end was accepted: {other:?}"),
    }
}
