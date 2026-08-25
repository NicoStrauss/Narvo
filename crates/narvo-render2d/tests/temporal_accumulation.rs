//! The oracles an accumulation has and a single frame cannot.
//!
//! M8.5a checked a cascade as a function; M8.6 checked a surface cache as a
//! sequence over an unchanging scene. An accumulator is a sequence over a
//! **moving** one, and that is a third thing: the operator it applies is
//! different every frame, because the reprojection depends on the motion.
//!
//! Four properties become checkable that were not, and the order matters —
//! the third is what makes any reference image of an accumulated field a
//! well-defined thing at all:
//!
//! - **a motion of nothing is the identity.** Byte for byte, on both resample
//!   arms. If it is not, the resampling is broken and no convergence measurement
//!   could still show it, because a field that never sits still never converges;
//! - **the reprojection moves what it says it moves.** A whole probe of motion
//!   shifts the field by a whole probe, and a fresh value appears exactly where
//!   history ran out — never a clamped neighbour, which is ghosting;
//! - **a static scene stops changing, and stays stopped.** The hash of the
//!   accumulated field settles on one value and holds it. An accumulated field
//!   depends on how many frames preceded it, so it is only a well-defined thing
//!   once it has stopped moving;
//! - **and accumulating a converged field again changes nothing.** Idempotence,
//!   byte for byte. That is an *equality* rather than a bound, and the module
//!   header of `accumulate.rs` derives why.
//!
//! # The derivation, before the tests were written
//!
//! `h + (f - h) / d` with `d` a power of two:
//!
//! 1. **at `f == h`** the difference is `+0.0`, the division is `+0.0`, and
//!    `h + 0.0` is `h` for every finite `h` that is not `-0.0`. An **equality**;
//! 2. **at zero motion** the reprojection reads the probe it is writing (nearest)
//!    or collapses its four taps onto that probe (bilinear), so `reprojected == h`
//!    exactly, and (1) applies. An **equality**;
//! 3. **over frames with `f` fixed**, `h - f` shrinks by exactly `1 - 1/d` until
//!    `(f - h) / d` is below half an ulp of `h` and the addition returns `h`. That
//!    is a **bound**: the field stops *near* `f` and not necessarily at it. The
//!    convergence test below measures both halves — that it stops, and how far
//!    from `f` it stopped.
//!
//! # The wall against passing for a second reason
//!
//! The convergence oracle is the one that can pass because a hash runs over the
//! wrong thing: a field of zeros stops changing immediately and stops changing
//! *correctly*. So it asserts three things and not one — that the field is not
//! trivial, that the hash **moved at least once**, and only then that it stopped.
//! M8.6's `the scene bounced nothing, so this oracle proved nothing` is the
//! precedent, and it is the single most useful line that report produced.

use narvo_render2d::{
    Accumulator, Albedo, Blend, Cascade, CascadeLayout, CascadeStage, Emission, MergeForm, Motion,
    OffscreenTarget, RadianceField, RenderError, Resample, Seeds, StageLayout,
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

// ------------------------------------------------------------ the synthetic --

/// Probes across in the synthetic grid. Odd and not a multiple of the workgroup,
/// so a dispatch that rounded its bounds the wrong way would run off the edge.
const PROBES_X: u32 = 17;
/// Probes down. Different from [`PROBES_X`], so a transposed index is visible.
const PROBES_Y: u32 = 11;
/// Texels between probes. A whole number, and a power of two so that a motion in
/// texels converts to probes exactly.
const SPACING: f32 = 4.0;

/// The stage the synthetic accumulator is built over.
///
/// Only its probe count and spacing are read, but it is a real validated stage
/// rather than two loose numbers — which is the whole reason
/// [`OffscreenTarget::accumulator`] takes one.
fn synthetic_stage() -> CascadeStage {
    CascadeStage::new(StageLayout {
        origin: [4.0, 4.0],
        spacing: SPACING,
        probes: [PROBES_X, PROBES_Y],
        near: 0.0,
        far: 2.0,
        directions: 16,
        far_radiance: [0.0, 0.0, 0.0],
    })
    .expect("the synthetic stage is sound")
}

/// A field no cascade would produce, in values every operation below is exact on.
///
/// **Small even integers and quarters.** That is not decoration: it is what makes
/// `h + (f - h) / d` exact for `d` up to eight, so an assertion can be an equality
/// with no tolerance and a failure is a defect rather than a rounding. Every probe
/// differs from every neighbour, so a reprojection that reads the wrong probe is
/// visible at that probe.
fn pattern(offset: u32) -> RadianceField {
    let mut field = RadianceField::new(PROBES_X, PROBES_Y).expect("a usable grid");
    for y in 0..PROBES_Y {
        for x in 0..PROBES_X {
            let n = y * PROBES_X + x + offset;
            field
                .set(
                    x,
                    y,
                    [
                        f32::from(u16::try_from(n % 61).expect("below 61")) * 2.0,
                        f32::from(u16::try_from(n % 37).expect("below 37")) * 4.0,
                        f32::from(u16::try_from(n % 23).expect("below 23")) * 8.0,
                    ],
                    f32::from(u16::try_from(n % 5).expect("below 5")) / 4.0,
                )
                .expect("inside the grid");
        }
    }
    field
}

/// A field in which every probe carries the same power-of-two value.
///
/// The uniform case is J3's oracle: weights that do not sum to one cannot leave a
/// uniform field alone. Powers of two so that even the bilinear arm is exact on
/// it — `u * (1 - w) + u * w` is `u` exactly when `u` is a power of two and the
/// weights are the sixteen-bit dyadic fractions the reprojection produces.
fn uniform(value: f32) -> RadianceField {
    let mut field = RadianceField::new(PROBES_X, PROBES_Y).expect("a usable grid");
    for y in 0..PROBES_Y {
        for x in 0..PROBES_X {
            field
                .set(x, y, [value, value, value], 0.5)
                .expect("inside the grid");
        }
    }
    field
}

/// Every probe's four channels, so an assertion can name the probe it failed at.
fn probes(field: &RadianceField) -> Vec<((u32, u32), [f32; 4])> {
    let mut out = Vec::new();
    for y in 0..field.height() {
        for x in 0..field.width() {
            let rgb = field.radiance(x, y).expect("a probe inside the grid");
            let escaped = field.escaped(x, y).expect("a probe inside the grid");
            out.push(((x, y), [rgb[0], rgb[1], rgb[2], escaped]));
        }
    }
    out
}

/// A field's bytes, hashed. FNV-1a over the raw `f32` bits.
///
/// **No value is ever committed** (ADR-0008): every comparison below is between
/// two hashes produced in the same run, which is what the whole determinism suite
/// does and why a dependency bump moves both sides together.
fn digest(field: &RadianceField) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for value in field.texels() {
        for byte in value.to_bits().to_ne_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// The CPU model of the blend, one probe's worth.
///
/// **Written out here rather than shared with the kernel**, for the reason
/// `surface_cache.rs` gives about `nearest_probe`: a shared helper would make the
/// oracle and the thing it judges one expression, and a wrong rounding would then
/// agree with itself. This is the same arithmetic derived from the same sentence.
fn blended(history: [f32; 4], fresh: [f32; 4], divisor: u32) -> [f32; 4] {
    let d = divisor as f32;
    [
        history[0] + (fresh[0] - history[0]) / d,
        history[1] + (fresh[1] - history[1]) / d,
        history[2] + (fresh[2] - history[2]) / d,
        history[3] + (fresh[3] - history[3]) / d,
    ]
}

/// The CPU model of the nearest reprojection, one probe's worth.
///
/// Returns the source probe, or `None` where there is no history — which is what
/// the kernel answers with the fresh value. Integer arithmetic, mirroring
/// `accumulate.wgsl`, and derived from the same sentence rather than shared.
fn nearest_source(x: u32, y: u32, motion_probes: (i32, i32)) -> Option<(u32, u32)> {
    let sx = i64::from(x) - i64::from(motion_probes.0);
    let sy = i64::from(y) - i64::from(motion_probes.1);
    if sx < 0 || sy < 0 || sx >= i64::from(PROBES_X) || sy >= i64::from(PROBES_Y) {
        return None;
    }
    Some((
        u32::try_from(sx).expect("checked non-negative and below the grid"),
        u32::try_from(sy).expect("checked non-negative and below the grid"),
    ))
}

/// An accumulator over the synthetic grid, with `history` already loaded.
fn loaded(
    target: &OffscreenTarget,
    blend: Blend,
    resample: Resample,
    history: &RadianceField,
) -> Accumulator {
    let mut accumulator = target
        .accumulator(&synthetic_stage(), blend, resample)
        .expect("the synthetic stage is a usable grid");
    accumulator
        .set_field(history)
        .expect("the field is the grid's size");
    accumulator
}

// -------------------------------------------------------------- the scene --

/// The lit scene's field extent.
const SCENE: u32 = 48;

/// A scene with a wall, a lamp behind it and a dusting, so that no two probes
/// agree by accident.
fn scene() -> (Seeds, Emission, Albedo) {
    let mut seeds = Seeds::new(SCENE, SCENE).expect("a seed set");
    let mut emission = Emission::new(SCENE, SCENE).expect("an emission map");
    let mut albedo = Albedo::new(SCENE, SCENE).expect("an albedo map");

    for y in 6..42 {
        seeds.set(20, y).expect("inside the field");
        albedo
            .set(20, y, [0.5, 0.25, 0.125])
            .expect("inside the map");
    }
    for y in 22..26 {
        for x in 32..36 {
            seeds.set(x, y).expect("inside the field");
            emission
                .set(x, y, [1.0, 0.75, 0.5])
                .expect("inside the map");
            albedo.set(x, y, [0.25, 0.5, 0.75]).expect("inside the map");
        }
    }
    let mut state: u32 = 0x2f6f_2b79;
    for _ in 0..120 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let x = (state >> 8) % SCENE;
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let y = (state >> 8) % SCENE;
        seeds.set(x, y).expect("inside the field");
        albedo.set(x, y, [0.75, 0.5, 0.25]).expect("inside the map");
    }
    (seeds, emission, albedo)
}

/// The scene's cascade: **one level**, for M8.6's reason.
///
/// M8.5b measured a single level to be one field on all eight adapter/backend
/// pairs and a composed cascade to be two, so an accumulation built on one level
/// tests the accumulation without the composition's unexplained split standing in
/// front of it.
fn scene_cascade() -> Cascade {
    Cascade::new(
        CascadeLayout {
            origin: [0.0, 0.0],
            base_spacing: 4.0,
            base_interval: 2.0,
            base_directions: 32,
            levels: 1,
            sky: [0.0, 0.0, 0.0],
        },
        SCENE,
        SCENE,
    )
    .expect("the scene's cascade is sound")
}

// --------------------------------------------------------------- the tests --

/// **A motion of nothing reprojects the field exactly, on both arms.**
///
/// §3's third assurance, and the one the other two rest on: if the resampling is
/// not the identity at zero motion, nothing sits still, and no convergence
/// measurement could report it — a field that keeps moving keeps moving whether
/// the accumulation is right or wrong.
///
/// The blend is `1 in 2` rather than `NONE`, so the assertion sees the history
/// through the result: with `f` and `h` different at every probe, `h + (f - h)/2`
/// determines `h`. A reprojection that returned a neighbouring probe would land
/// on a different number, because [`pattern`] gives no two probes the same value.
#[test]
fn a_motion_of_nothing_reprojects_the_field_exactly() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let history = pattern(0);
    let fresh = pattern(1_000);
    let blend = Blend::one_in(2).expect("two is a power of two");

    for resample in [Resample::Nearest, Resample::Bilinear] {
        let mut accumulator = loaded(&target, blend, resample, &history);
        let out = target
            .accumulate(&mut accumulator, &fresh, Motion::STILL)
            .expect("a still frame accumulates");

        let expected: Vec<((u32, u32), [f32; 4])> = probes(&history)
            .into_iter()
            .zip(probes(&fresh))
            .map(|((at, h), (_, f))| (at, blended(h, f, 2)))
            .collect();
        assert_eq!(
            probes(&out),
            expected,
            "{resample:?} does not reproject a still field onto itself"
        );
    }
}

/// **Accumulating a converged field again changes nothing.**
///
/// §3's second assurance, and it is an **equality**: `h + (h - h) / d` is
/// `h + 0.0`, which is `h` for every finite `h` that is not a negative zero.
/// Eight rounds, both arms, every divisor from one to sixty-four — and the field
/// is compared to the field it started as, not to the previous round, so a drift
/// of one ulp per round cannot hide.
///
/// The negative-zero edge is checked rather than argued: the header of
/// `accumulate.rs` names it as the one value for which the equality would fail,
/// and [`pattern`] carries zeros in every channel at some probe, so the assertion
/// below would see `-0.0` appearing if anything produced one.
#[test]
fn accumulating_a_converged_field_changes_nothing() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let converged = pattern(0);
    let start = probes(&converged);

    for divisor in [1, 2, 8, 64] {
        let blend = Blend::one_in(divisor).expect("a power of two");
        for resample in [Resample::Nearest, Resample::Bilinear] {
            let mut accumulator = loaded(&target, blend, resample, &converged);
            for round in 1..=8 {
                let out = target
                    .accumulate(&mut accumulator, &converged, Motion::STILL)
                    .expect("a still frame accumulates");
                assert_eq!(
                    probes(&out),
                    start,
                    "one in {divisor} with {resample:?} moved a converged field at \
                     round {round}"
                );
                for (at, value) in probes(&out) {
                    for (channel, v) in value.iter().enumerate() {
                        assert!(
                            !(*v == 0.0 && v.is_sign_negative()),
                            "probe {at:?} channel {channel} came back as a negative zero, \
                             which is the one value the idempotence equality does not hold for"
                        );
                    }
                }
            }
        }
    }
}

/// **A whole probe of motion shifts the field by a whole probe, and what runs off
/// the edge comes back fresh.**
///
/// Two properties in one assertion, because they are one behaviour: the
/// reprojection is a shift, and the band it cannot fill is filled with *this*
/// frame's answer rather than with a clamped neighbour. A clamp is J2 — a
/// plausible picture over a wrong field — and it is caught here by the band's
/// values being the fresh field's rather than the history's edge column repeated.
///
/// Both arms, because a whole-probe motion has a zero fractional part and the
/// bilinear arm must then collapse onto the single probe the nearest arm reads.
#[test]
fn a_whole_probe_of_motion_shifts_the_field_and_the_edge_comes_back_fresh() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let history = pattern(0);
    let fresh = pattern(1_000);
    let blend = Blend::one_in(2).expect("two is a power of two");

    for (dx, dy) in [(1_i32, 0_i32), (0, 1), (-2, 3), (PROBES_X as i32 + 1, 0)] {
        let motion = Motion {
            dx: dx as f32 * SPACING,
            dy: dy as f32 * SPACING,
        };
        for resample in [Resample::Nearest, Resample::Bilinear] {
            let mut accumulator = loaded(&target, blend, resample, &history);
            let out = target
                .accumulate(&mut accumulator, &fresh, motion)
                .expect("a whole-probe motion accumulates");

            let mut expected = Vec::new();
            let mut disoccluded = 0_usize;
            for ((at, f), _) in probes(&fresh).into_iter().zip(probes(&history)) {
                let value = match nearest_source(at.0, at.1, (dx, dy)) {
                    Some((sx, sy)) => {
                        let rgb = history.radiance(sx, sy).expect("a source probe");
                        let escaped = history.escaped(sx, sy).expect("a source probe");
                        blended([rgb[0], rgb[1], rgb[2], escaped], f, 2)
                    }
                    None => {
                        disoccluded += 1;
                        // No history: the reprojection wrote the fresh value, so
                        // the blend is `f + (f - f) / 2`, which is `f` exactly.
                        f
                    }
                };
                expected.push((at, value));
            }
            assert!(
                disoccluded > 0,
                "a motion of ({dx}, {dy}) probes disoccluded nothing, so this case \
                 proved nothing about the band"
            );
            assert_eq!(
                probes(&out),
                expected,
                "{resample:?} at a motion of ({dx}, {dy}) probes"
            );
        }
    }
}

/// **A uniform field survives any motion, on both arms — J3's oracle.**
///
/// Weights that do not sum to one cannot leave a uniform field alone: too little
/// and it darkens, too much and it brightens, and either way it does so slowly,
/// which is exactly the defect a single frame cannot show. Sixty-four frames of
/// arbitrary fractional motion, and the field must come back byte-identical every
/// time.
///
/// It reaches further than the blend. The reprojection's own weights are in it
/// too — the bilinear arm's four taps have to sum to one at every fractional
/// offset, and a uniform field is the only input for which that is visible
/// independently of what the taps land on.
#[test]
fn a_uniform_field_survives_any_motion() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let field = uniform(0.25);
    let start = probes(&field);
    let blend = Blend::one_in(4).expect("four is a power of two");

    for resample in [Resample::Nearest, Resample::Bilinear] {
        let mut accumulator = loaded(&target, blend, resample, &field);
        let mut state: u32 = 0x9e37_79b9;
        for frame in 1..=64 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let dx = ((state >> 8) % 2_048) as f32 / 256.0 - 4.0;
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let dy = ((state >> 8) % 2_048) as f32 / 256.0 - 4.0;
            let out = target
                .accumulate(&mut accumulator, &field, Motion { dx, dy })
                .expect("a fractional motion accumulates");
            assert_eq!(
                probes(&out),
                start,
                "{resample:?} moved a uniform field at frame {frame}, motion ({dx}, {dy}). \
                 Weights that do not sum to one is what that looks like"
            );
        }
    }
}

/// **The reprojection uses this frame's motion and not the previous one — J1's
/// oracle, and the one a still frame cannot be.**
///
/// §5 asks which oracle sees a one-frame offset in the transform, and whether the
/// zero-motion identity does. **It does not, and it cannot**: at a motion of
/// nothing, this frame's transform and the previous one are the same transform.
/// The oracle has to be a sequence in which two consecutive frames move
/// *differently*, and the assertion has to be against a model that applies each
/// to the frame it belongs to.
///
/// Three frames, motions of nothing, one probe and three probes. A run that used
/// the previous frame's motion would shift by nothing, then by nothing, then by
/// one — which is a different field at every probe the shift crosses.
#[test]
fn the_reprojection_uses_this_frames_motion() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let blend = Blend::one_in(2).expect("two is a power of two");
    let steps: [(i32, i32); 3] = [(0, 0), (1, 0), (3, -1)];

    let mut accumulator = loaded(&target, blend, Resample::Nearest, &pattern(0));
    let mut model = probes(&pattern(0));
    let mut out = pattern(0);

    for (frame, (dx, dy)) in steps.into_iter().enumerate() {
        let fresh = pattern(1_000 + frame as u32 * 97);
        out = target
            .accumulate(
                &mut accumulator,
                &fresh,
                Motion {
                    dx: dx as f32 * SPACING,
                    dy: dy as f32 * SPACING,
                },
            )
            .expect("the frame accumulates");
        model = probes(&fresh)
            .into_iter()
            .map(|(at, f)| {
                let value = match nearest_source(at.0, at.1, (dx, dy)) {
                    Some((sx, sy)) => {
                        let index = (sy * PROBES_X + sx) as usize;
                        blended(model[index].1, f, 2)
                    }
                    None => f,
                };
                (at, value)
            })
            .collect();
        assert_eq!(
            probes(&out),
            model,
            "frame {frame} of the sequence, motion ({dx}, {dy}) probes"
        );
    }
    // The could-have-failed half: the three motions have to have produced three
    // different fields, or the sequence proved nothing about which one was used.
    assert_ne!(
        digest(&out),
        digest(&pattern(0)),
        "the sequence ended where it began"
    );
}

/// **A static scene stops changing, and stays stopped — §3's first assurance, and
/// the instrument §4 asks for.**
///
/// The real path rather than a synthetic one: a surface cache producing a frame of
/// lighting, and an accumulator over its probe grid. Both are converging at once —
/// the bounce toward its fixed point, the accumulation toward the bounce — and the
/// claim is that the pair stops.
///
/// **Three assertions and not one**, because this is the oracle that can pass for
/// the wrong reason. A field of zeros stops changing immediately and stops
/// changing correctly, so:
///
/// 1. the accumulated field carries a non-zero radiance somewhere;
/// 2. the hash **moved** — there is more than one distinct value in the run;
/// 3. and only then, that the last value repeats to the end.
///
/// The number of frames it takes is reported by the failure message rather than
/// asserted, because a bound on it would be a number derived from an argument. It
/// is measured in M8.7's report.
#[test]
fn a_static_scene_stops_changing_and_stays_stopped() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();
    let mut cache = target
        .surface_cache(&seeds, &emission, &albedo, &cascade, MergeForm::default())
        .expect("the scene's cache");
    let stage = *cascade.level(0).expect("a cascade has at least one level");
    let mut accumulator = target
        .accumulator(
            &stage,
            Blend::one_in(4).expect("four is a power of two"),
            Resample::Nearest,
        )
        .expect("level zero is a usable grid");

    const FRAMES: usize = 96;
    let mut digests = Vec::with_capacity(FRAMES);
    let mut last = None;
    for _ in 0..FRAMES {
        let fresh = target.bounce(&mut cache).expect("a frame of feedback");
        let out = target
            .accumulate(&mut accumulator, &fresh, Motion::STILL)
            .expect("a still frame accumulates");
        digests.push(digest(&out));
        last = Some(out);
    }
    let field = last.expect("ninety-six frames were run");

    let lit = probes(&field)
        .into_iter()
        .filter(|(_, value)| value[0] > 0.0 || value[1] > 0.0 || value[2] > 0.0)
        .count();
    assert!(
        lit > 0,
        "nothing in the scene was lit, so this oracle proved nothing about convergence"
    );

    let distinct = {
        let mut sorted = digests.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.len()
    };
    assert!(
        distinct > 1,
        "the accumulated field never changed at all, so it did not converge — it \
         started still, and this oracle proved nothing"
    );

    let settled = digests
        .iter()
        .rposition(|d| *d != digests[FRAMES - 1])
        .map_or(0, |i| i + 1);
    let tail = FRAMES - settled;
    assert!(
        tail >= FRAMES / 4,
        "the accumulated field settled only {tail} frames before the end of {FRAMES} \
         (first settled at frame {settled}, {distinct} distinct hashes). It has not \
         been shown to have stopped rather than to be moving slowly"
    );
}

/// **A converged field is a fixed point of the real path too.**
///
/// The synthetic idempotence test uses a field a caller chose. This one uses the
/// field the scene actually converged to, and asserts that another sixteen frames
/// change no byte of it. That is the difference between "the arithmetic is
/// idempotent" and "this feature converges to something idempotent".
#[test]
fn the_scene_converges_to_a_fixed_point_of_its_own() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();
    let mut cache = target
        .surface_cache(&seeds, &emission, &albedo, &cascade, MergeForm::default())
        .expect("the scene's cache");
    let stage = *cascade.level(0).expect("a cascade has at least one level");
    let mut accumulator = target
        .accumulator(
            &stage,
            Blend::one_in(4).expect("four is a power of two"),
            Resample::Nearest,
        )
        .expect("level zero is a usable grid");

    for _ in 0..96 {
        let fresh = target.bounce(&mut cache).expect("a frame of feedback");
        target
            .accumulate(&mut accumulator, &fresh, Motion::STILL)
            .expect("a still frame accumulates");
    }
    let settled = accumulator.field().expect("the accumulated field");
    let expected = probes(&settled);
    assert!(
        expected.iter().any(|(_, value)| value[0] > 0.0),
        "the converged field is dark, so this oracle proved nothing"
    );

    for frame in 1..=16 {
        let fresh = target.bounce(&mut cache).expect("a frame of feedback");
        let out = target
            .accumulate(&mut accumulator, &fresh, Motion::STILL)
            .expect("a still frame accumulates");
        assert_eq!(
            probes(&out),
            expected,
            "the converged scene moved again at frame {frame}"
        );
    }
}

/// **A first frame is its own field, and `forget` makes the next one a first
/// frame again.**
///
/// The accumulator has no history to reproject before anything has been
/// accumulated, and the alternative to saying so is blending against a field of
/// zeros — which would darken the first `divisor` frames of every run. It is
/// checked at a divisor of sixty-four, where that defect would be a factor of
/// sixty-four rather than a subtlety.
#[test]
fn a_first_frame_is_its_own_field() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let fresh = pattern(7);
    let expected = probes(&fresh);
    let blend = Blend::one_in(64).expect("sixty-four is a power of two");

    for resample in [Resample::Nearest, Resample::Bilinear] {
        let mut accumulator = target
            .accumulator(&synthetic_stage(), blend, resample)
            .expect("the synthetic stage is a usable grid");
        assert!(!accumulator.has_history(), "a new accumulator has history");

        let first = target
            .accumulate(&mut accumulator, &fresh, Motion::STILL)
            .expect("a first frame accumulates");
        assert_eq!(probes(&first), expected, "{resample:?}'s first frame");
        assert!(accumulator.has_history(), "a frame left no history");
        assert_eq!(accumulator.frames(), 1);

        // A second frame of something else, then a cut.
        let other = pattern(500);
        target
            .accumulate(&mut accumulator, &other, Motion::STILL)
            .expect("a second frame accumulates");
        accumulator.forget();
        assert!(!accumulator.has_history(), "forget left history behind");

        let after = target
            .accumulate(&mut accumulator, &fresh, Motion::STILL)
            .expect("a frame after a cut accumulates");
        assert_eq!(
            probes(&after),
            expected,
            "{resample:?} after a cut did not start over"
        );
        assert_eq!(accumulator.frames(), 3, "forget moved the frame count");
    }
}

/// **Two runs of one accumulation agree byte for byte.**
///
/// The determinism check every GPU path in this crate carries. M8.5b and M8.6 both
/// recorded that their copy caught none of their injections, because every defect
/// in a deterministic path is deterministic; this one is kept for the same reason
/// theirs were — it would catch a race, and it is the only in-tree check that a
/// sequence of frames is stable under being run twice. **It still has no red
/// edge**, and M8.7's report says so again.
#[test]
fn two_runs_of_one_accumulation_agree_byte_for_byte() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let blend = Blend::one_in(8).expect("eight is a power of two");

    let run = |resample: Resample| {
        let mut accumulator = loaded(&target, blend, resample, &pattern(0));
        let mut digests = Vec::new();
        for frame in 0..12_u32 {
            let fresh = pattern(1_000 + frame * 37);
            let out = target
                .accumulate(
                    &mut accumulator,
                    &fresh,
                    Motion {
                        dx: frame as f32 * 1.5,
                        dy: -(frame as f32) * 0.75,
                    },
                )
                .expect("the frame accumulates");
            digests.push(digest(&out));
        }
        digests
    };

    for resample in [Resample::Nearest, Resample::Bilinear] {
        assert_eq!(
            run(resample),
            run(resample),
            "{resample:?} does not reproduce itself"
        );
    }
}

/// **The two arms agree where the motion is whole and part company where it is
/// not — and that is what makes the second arm a measurement rather than a
/// duplicate.**
///
/// If they agreed everywhere, keeping both would be stock; if they disagreed at a
/// whole-probe motion, one of them would be wrong. The measurement M8.7's §2
/// reports lives in the second half of this.
#[test]
fn the_two_arms_agree_on_whole_probes_and_not_on_fractions() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let history = pattern(0);
    let fresh = pattern(1_000);
    let blend = Blend::one_in(2).expect("two is a power of two");

    let once = |resample: Resample, motion: Motion| {
        let mut accumulator = loaded(&target, blend, resample, &history);
        digest(
            &target
                .accumulate(&mut accumulator, &fresh, motion)
                .expect("the frame accumulates"),
        )
    };

    let whole = Motion {
        dx: 2.0 * SPACING,
        dy: -SPACING,
    };
    assert_eq!(
        once(Resample::Nearest, whole),
        once(Resample::Bilinear, whole),
        "the two arms disagree at a motion of whole probes, where the bilinear \
         weights are one and zero"
    );

    let fraction = Motion {
        dx: SPACING / 2.0,
        dy: 0.0,
    };
    assert_ne!(
        once(Resample::Nearest, fraction),
        once(Resample::Bilinear, fraction),
        "the two arms agree at a motion of half a probe, so the bilinear arm is not \
         interpolating and M8.7's second measurement arm is a copy of its first"
    );
}

/// **Sub-probe motions add up until the grid moves — and without that, they never
/// move it at all.**
///
/// The oracle for the arrears `Accumulator::unapplied` keeps, and it exists
/// because M8.7's probe measured what happens without them: a nearest
/// reprojection shifts by a whole probe, so a camera moving a quarter of a probe
/// per frame is rounded to nothing **every frame** and the stored field stands
/// still while the scene slides underneath it. That is not a stair-step, it is a
/// misregistration that grows without bound — 0.12 to 0.49 of the field's own RMS
/// at a tenth of a probe per frame, against about 1e-5 at a whole probe.
///
/// The arrangement makes the shift the *only* thing the answer depends on: the
/// fresh field is **dark**, so a blend of one in two leaves `history / 2` at every
/// probe that had history and exactly zero at every probe that did not. Four
/// frames of a quarter probe therefore have to end at `shift(history) / 16` with
/// the shift a whole probe — and if the arrears were dropped, at `history / 16`
/// with no shift, which differs at every probe because [`pattern`] gives no two
/// the same value.
///
/// The schedule is derived here from the rule rather than shared with the code
/// that performs it, the same way [`nearest_source`] is.
#[test]
fn sub_probe_motions_accumulate_until_the_grid_moves() {
    let Some(target) = target_or_skip() else {
        return;
    };
    /// Fixed-point units to a probe, as `accumulate.wgsl` declares them.
    const UNIT: i32 = 1 << 16;
    /// The whole-probe shift a nearest reprojection performs, derived from the
    /// kernel's `ix = (x * UNIT - offset + HALF) >> SHIFT`.
    fn applied(total: i32) -> i32 {
        -((UNIT / 2 - total).div_euclid(UNIT)) * UNIT
    }

    const FRAMES: usize = 4;
    // One texel of motion at a spacing of four is a quarter of a probe.
    let motion = Motion { dx: 1.0, dy: 0.0 };
    let step = UNIT / 4;

    let mut residual = 0_i32;
    let mut shift = 0_i32;
    let mut moved_at = Vec::new();
    for frame in 0..FRAMES {
        let total = step + residual;
        let this = applied(total);
        residual = total - this;
        if this != 0 {
            moved_at.push(frame);
        }
        shift += this / UNIT;
    }
    assert_eq!(
        shift, 1,
        "four quarter-probe frames did not add up to one whole probe of shift"
    );
    assert!(
        !moved_at.is_empty(),
        "the grid never moved at all, so this oracle proved nothing"
    );
    assert_eq!(residual, 0, "the arrears did not come back to zero");

    let history = pattern(0);
    let dark = RadianceField::new(PROBES_X, PROBES_Y).expect("a usable grid");
    let mut accumulator = loaded(
        &target,
        Blend::one_in(2).expect("two is a power of two"),
        Resample::Nearest,
        &history,
    );
    let mut out = history.clone();
    for _ in 0..FRAMES {
        out = target
            .accumulate(&mut accumulator, &dark, motion)
            .expect("a sub-probe frame accumulates");
    }

    let expected: Vec<((u32, u32), [f32; 4])> = probes(&dark)
        .into_iter()
        .map(|(at, _)| {
            let value = match nearest_source(at.0, at.1, (shift, 0)) {
                Some((sx, sy)) => {
                    let rgb = history.radiance(sx, sy).expect("a source probe");
                    let escaped = history.escaped(sx, sy).expect("a source probe");
                    // Four halvings, every one of them exact: `pattern` is small
                    // dyadic values and the shift lands on frame three, so the
                    // probe that survives to the end is the shifted one.
                    [rgb[0] / 16.0, rgb[1] / 16.0, rgb[2] / 16.0, escaped / 16.0]
                }
                None => [0.0; 4],
            };
            (at, value)
        })
        .collect();
    assert_eq!(
        probes(&out),
        expected,
        "four quarter-probe frames did not shift the field by one whole probe. \
         Dropping the arrears is what makes a slow pan leave the history standing \
         still"
    );
    assert_eq!(
        accumulator.unapplied(),
        Motion { dx: 0.0, dy: 0.0 },
        "the arrears the accumulator reports are not the ones the schedule derives"
    );
}

/// **A blend of one in one hands back the frame it was given, byte for byte.**
///
/// The control the whole family is read against, and it is *not* free: `h + (f -
/// h) / 1` is `f` only where `f - h` is exact, which is not every pair of floats.
/// M8.7 measured it on a real lit field — 50 of 50 frames identical — and this is
/// the in-tree half of that, over a history and a fresh field chosen to be as far
/// apart as [`pattern`] gets.
///
/// **The named limit, measured rather than argued:** it stops being true where the
/// two fields are orders of magnitude apart, because `f - h` then rounds. The
/// second half of this test is that case, and it is asserted to *differ* — so the
/// limit is a fact in the tree rather than a sentence in a doc comment.
#[test]
fn a_blend_of_one_in_one_returns_the_frame_it_was_given() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let history = pattern(0);
    let fresh = pattern(1_000);
    let expected = probes(&fresh);

    let mut accumulator = loaded(&target, Blend::NONE, Resample::Nearest, &history);
    for frame in 1..=4 {
        let out = target
            .accumulate(&mut accumulator, &fresh, Motion::STILL)
            .expect("a still frame accumulates");
        assert_eq!(
            probes(&out),
            expected,
            "frame {frame} of a blend of one in one"
        );
    }

    // The limit. A history sixteen million times the fresh field makes `f - h`
    // round, and `h + (f - h)` then misses `f`.
    let mut huge = RadianceField::new(PROBES_X, PROBES_Y).expect("a usable grid");
    for y in 0..PROBES_Y {
        for x in 0..PROBES_X {
            huge.set(x, y, [1.0e9, 1.0e9, 1.0e9], 1.0)
                .expect("inside the grid");
        }
    }
    let mut tiny = RadianceField::new(PROBES_X, PROBES_Y).expect("a usable grid");
    for y in 0..PROBES_Y {
        for x in 0..PROBES_X {
            tiny.set(x, y, [1.0, 1.0, 1.0], 0.0)
                .expect("inside the grid");
        }
    }
    let mut edge = loaded(&target, Blend::NONE, Resample::Nearest, &huge);
    let out = target
        .accumulate(&mut edge, &tiny, Motion::STILL)
        .expect("a still frame accumulates");
    assert_ne!(
        probes(&out),
        probes(&tiny),
        "a blend of one in one was exact even across nine orders of magnitude, so \
         the limit `Blend::NONE` documents does not exist and the doc is wrong"
    );
}

/// A field that is not the accumulator's grid is refused, naming both shapes.
#[test]
fn a_field_that_is_not_the_grid_is_refused() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let mut accumulator = target
        .accumulator(&synthetic_stage(), Blend::NONE, Resample::default())
        .expect("the synthetic stage is a usable grid");
    let wrong = RadianceField::new(PROBES_Y, PROBES_X).expect("a transposed grid");

    match target.accumulate(&mut accumulator, &wrong, Motion::STILL) {
        Err(RenderError::RadianceGridMismatch {
            grid_width,
            grid_height,
            field_width,
            field_height,
        }) => {
            assert_eq!(
                (grid_width, grid_height, field_width, field_height),
                (PROBES_X, PROBES_Y, PROBES_Y, PROBES_X),
                "the refusal names the wrong shapes"
            );
        }
        other => panic!("a transposed field was not refused: {other:?}"),
    }
    match accumulator.set_field(&wrong) {
        Err(RenderError::RadianceGridMismatch { .. }) => {}
        other => panic!("setting a transposed field was not refused: {other:?}"),
    }
    assert_eq!(
        accumulator.frames(),
        0,
        "a refusal advanced the accumulator"
    );
}

/// A motion no fixed-point offset can hold is refused rather than wrapped.
#[test]
fn a_motion_that_cannot_be_reprojected_is_refused() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let mut accumulator = target
        .accumulator(&synthetic_stage(), Blend::NONE, Resample::default())
        .expect("the synthetic stage is a usable grid");
    let fresh = pattern(0);

    for motion in [
        Motion {
            dx: f32::INFINITY,
            dy: 0.0,
        },
        Motion {
            dx: 0.0,
            dy: f32::NAN,
        },
        Motion { dx: 1.0e9, dy: 0.0 },
    ] {
        match target.accumulate(&mut accumulator, &fresh, motion) {
            Err(RenderError::MotionOutOfRange { .. }) => {}
            other => panic!("a motion of {motion:?} was not refused: {other:?}"),
        }
    }
    assert_eq!(
        accumulator.frames(),
        0,
        "a refusal advanced the accumulator"
    );
}

/// **An accumulator over a real cascade's level zero is the grid that cascade
/// reports.**
///
/// The seam M8.8 will stand on: `cascade.level(0)` and `surface_cache`'s radiance
/// have to be the same shape, and if they were not, every consumer would be
/// converting between two grids by hand. Checked against the field a frame
/// actually returns rather than against the layout, so it is the two paths
/// agreeing rather than one of them being read twice.
#[test]
fn an_accumulator_takes_the_grid_a_frame_returns() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let (seeds, emission, albedo) = scene();
    let cascade = scene_cascade();
    let mut cache = target
        .surface_cache(&seeds, &emission, &albedo, &cascade, MergeForm::default())
        .expect("the scene's cache");
    let stage = *cascade.level(0).expect("a cascade has at least one level");
    let accumulator = target
        .accumulator(&stage, Blend::NONE, Resample::default())
        .expect("level zero is a usable grid");

    let fresh = target.bounce(&mut cache).expect("a frame of feedback");
    assert_eq!(
        accumulator.probes(),
        [fresh.width(), fresh.height()],
        "an accumulator over level zero is not the shape level zero returns"
    );
    assert_eq!(accumulator.blend(), Blend::NONE);
    assert_eq!(accumulator.resample(), Resample::Nearest);
}
