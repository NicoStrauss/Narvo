//! A decaying camera shake: the [`Shake`] component and the offset it produces.
//!
//! M3.30 answered the question ADR-0010 poses — a shake writes the camera, the
//! camera is world state, so a shake is simulation state — and found that the
//! *randomness* is not the binding part: a decaying oscillation needs no
//! generator at all and is replay-safe by construction. This is that
//! oscillation. There is no [`Rng`](crate::Rng) here and none is wanted.
//!
//! # No transcendental function, and that is not a stylistic preference
//!
//! The obvious shape is `amplitude * sin(phase)`. **`f32::sin` is not specified
//! to the bit**, and the same call can return neighbouring values on two
//! platforms; ADR-0013 makes cross-platform agreement a hard goal, so a sine
//! would put the camera — which is in the state hash — on top of the one
//! operation this engine cannot pin. The wave below is a **triangle**, built
//! from addition, subtraction, multiplication and comparison only. All four are
//! exactly rounded by IEEE 754, so every platform performs the same operations
//! in the same order and gets the same bits.
//!
//! It is not a claim that a triangle looks better than a sine. It is the claim
//! that a triangle is reproducible and a sine is not, and for a value that
//! reaches the state hash that decides it.

use serde::{Deserialize, Serialize};

/// One period of the triangle wave, in phase units.
///
/// A power of two, so that [`wrap`]'s divide and multiply by it are exact and
/// the wrap costs no precision beyond the addition that preceded it.
const PERIOD: f32 = 4.0;

/// A decaying oscillation of the camera, and the state it has reached.
///
/// Put it on the same entity as the [`Camera`](crate::Camera) it shakes.
/// [`compose_camera`](crate::compose_camera) — the camera's single composition
/// point — advances it and adds [`Shake::offset`] to whatever base it composed.
///
/// # What it is over
///
/// An exponential decay never reaches zero, and a camera that wobbles below one
/// ULP forever is a defect rather than an effect. So the shake has a **defined
/// end**: once `amplitude <= cutoff` the component sets `amplitude` and `phase`
/// to exactly `0.0`, [`Shake::offset`] returns exactly `(0.0, 0.0)`, and the
/// composed camera is bit-identical to its base. `at_rest` is that state: it is
/// checkable, in the hash, and not a tolerance a reader has to guess at — and it
/// is reachable under the precondition the validation section below states, not
/// unconditionally.
///
/// The end is a **state, not an absence**: a world carrying an expired `Shake`
/// renders exactly what a world with no `Shake` renders, and the two still hash
/// differently, because one of them carries a component. That is stated as a
/// property and asserted in `an_expired_shake_composes_the_same_camera_as_no_shake`.
///
/// # Two impulses: the stronger one wins
///
/// [`Shake::arm`] raises the amplitude to the **maximum** of the current and the
/// new one and leaves the phase alone.
///
/// *Rejected: adding.* Two impulses in consecutive ticks would sum, and a burst
/// of them would grow the amplitude without bound — the camera would leave the
/// screen for a reason no single call site asked for.
///
/// *Rejected: replacing.* A weak impulse arriving during a strong shake would
/// cut the strong one short, which is visibly wrong and is the common case
/// (a small event during a big one).
///
/// Leaving the phase alone is the other half: re-arming mid-swing must not
/// teleport the camera, and a phase reset would do exactly that.
///
/// # The fields are bare scalars (ADR-0014)
///
/// Seven `f32` and nothing else. `phase` is in the same units as `frequency`
/// rather than radians, because nothing here takes a sine and radians would
/// imply otherwise.
///
/// # What is not validated, and what that costs
///
/// A component is storage and does not validate, as `Layer` does not reject a
/// `NaN` depth. The consequences are worth writing down rather than leaving to
/// be discovered, because two of them break promises this very doc makes:
///
/// - **`decay >= 1.0` never ends.** The amplitude does not shrink, so the
///   cutoff is never reached and "the shake has a defined end" is false for that
///   configuration. The end above is reachable **for `0.0 <= decay < 1.0` with a
///   finite amplitude and cutoff**, which is the whole of the sensible range and
///   none of the rest.
/// - **A `NaN` amplitude is immortal.** `at_rest` compares `== 0.0` and the
///   cutoff check compares `<=`; both are false against `NaN`, so the shake runs
///   forever and feeds `NaN` into the camera. Deterministic, reproducible, and
///   useless — a wrong picture rather than a wedged tick, which is the line
///   [`wrap`] holds.
/// - **A negative `frequency` runs the wave backwards**, which is harmless:
///   [`wrap`] brings the phase back into `0.0 .. 4.0` from either side, so the
///   offset stays bounded by the amplitude.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Shake {
    /// Half the peak-to-peak travel, in world units. Decays every tick.
    pub amplitude: f32,
    /// Phase units advanced per tick. One period is `4.0`, so `1.0` is a
    /// quarter turn per tick and `4.0` stands still.
    pub frequency: f32,
    /// The fraction of the amplitude that survives a tick. `0.5` halves it.
    pub decay: f32,
    /// Where in the period the wave currently sits, in `0.0 .. 4.0`.
    pub phase: f32,
    /// At or below this amplitude the shake is over and snaps to rest.
    pub cutoff: f32,
    /// The base to offset from when no other contributor supplies one.
    ///
    /// A [`Follow`](crate::Follow) on the same entity supplies the base and this
    /// is ignored. Without one, a shake still needs to know what it is shaking
    /// *around*, and re-reading the camera would accumulate its own offset every
    /// tick — so the base is captured once, by [`Shake::around`].
    pub base_x: f32,
    /// The base along y. See [`Shake::base_x`].
    pub base_y: f32,
}

impl Shake {
    /// A shake at rest around `(base_x, base_y)`, ready to be armed.
    #[must_use]
    pub const fn around(base_x: f32, base_y: f32) -> Self {
        Self {
            amplitude: 0.0,
            frequency: 1.0,
            decay: 0.5,
            phase: 0.0,
            cutoff: 0.0,
            base_x,
            base_y,
        }
    }

    /// A shake that will run from `amplitude` down to `cutoff`, **around the
    /// origin**.
    ///
    /// The base is `(0.0, 0.0)`, which is right when a [`Follow`](crate::Follow)
    /// supplies the base and the shake's own is ignored — the common case, and
    /// every call site in this crate. **Without a follow it is a trap**: putting
    /// one of these on a camera at (50, 50) moves that camera to the origin on
    /// the first tick, because the composition takes the base from the shake and
    /// assigns rather than adds. Use [`Shake::around`] and [`Shake::arm`] for a
    /// standalone shake, and see
    /// `a_bare_new_shake_pulls_a_standalone_camera_to_the_origin`, which pins the
    /// behaviour so that it is recorded rather than lurking.
    #[must_use]
    pub const fn new(amplitude: f32, frequency: f32, decay: f32, cutoff: f32) -> Self {
        Self {
            amplitude,
            frequency,
            decay,
            phase: 0.0,
            cutoff,
            base_x: 0.0,
            base_y: 0.0,
        }
    }

    /// Raises the amplitude to `amplitude` if that is larger, leaving the phase.
    ///
    /// This is the impulse API: one call, no ordering rules, and idempotent
    /// against a weaker repeat. See the type's documentation for why the
    /// maximum rather than a sum or a replacement.
    pub const fn arm(&mut self, amplitude: f32) {
        if amplitude > self.amplitude {
            self.amplitude = amplitude;
        }
    }

    /// Whether the shake has finished and is contributing exactly nothing.
    #[must_use]
    pub fn at_rest(self) -> bool {
        self.amplitude == 0.0
    }

    /// Advances one tick: decay, then phase, then the end check.
    ///
    /// One call is one tick (ADR-0003). Nothing reads a clock, so catch-up —
    /// several ticks inside one advance — is several calls and produces what
    /// several separate ticks produce.
    ///
    /// The order is load bearing and is the reason
    /// [`compose_camera`](crate::compose_camera) can be checked against a pure
    /// function of the stored state: **advance first, then compose**, so the
    /// offset the camera carries is always [`Shake::offset`] of the state the
    /// tick left behind.
    pub fn advance(&mut self) {
        if self.at_rest() {
            return;
        }

        self.amplitude *= self.decay;
        self.phase = wrap(self.phase + self.frequency);

        if self.amplitude <= self.cutoff {
            self.amplitude = 0.0;
            self.phase = 0.0;
        }
    }

    /// The offset this shake currently contributes, in world units.
    ///
    /// A pure function of the stored fields, which is what lets the composition
    /// guard recompute the camera instead of trusting it. At rest it is exactly
    /// `(0.0, 0.0)`, so an expired shake composes a camera bit-identical to its
    /// base.
    ///
    /// x and y are a quarter period apart, so the camera travels a diamond
    /// rather than a line — a line reads as a glitch, and the diamond is what a
    /// triangle wave gives instead of a circle for free.
    #[must_use]
    pub fn offset(self) -> (f32, f32) {
        if self.at_rest() {
            return (0.0, 0.0);
        }
        let quarter = {
            let shifted = self.phase + 1.0;
            if shifted >= PERIOD {
                shifted - PERIOD
            } else {
                shifted
            }
        };
        (
            self.amplitude * triangle(self.phase),
            self.amplitude * triangle(quarter),
        )
    }
}

/// Brings a phase back into one period, in constant time.
///
/// **It is not a loop, and that is a fix rather than a flourish.** The obvious
/// `while phase >= PERIOD { phase -= PERIOD }` **hangs** on an infinite
/// frequency: `inf >= 4.0` is true and `inf - 4.0` is `inf`, so the tick never
/// returns. A component does not validate its fields — `Layer` accepts a `NaN`
/// depth and `Follow` a divergent smoothing — but "does not validate" has to
/// mean a wrong picture, never a wedged simulation, so the arithmetic has to
/// terminate on any input at all.
///
/// `floor` is IEEE 754's roundToIntegralTowardNegative and is exactly specified,
/// as are the divide, the multiply and the subtract, so this is as reproducible
/// across platforms as the subtraction was. An infinite or `NaN` phase comes out
/// `NaN` and stays there, deterministically, which is the same answer every
/// platform gives.
fn wrap(phase: f32) -> f32 {
    phase - PERIOD * (phase / PERIOD).floor()
}

/// The triangle wave on `0.0 .. 4.0`, running 0 → 1 → 0 → −1 → 0.
///
/// Comparisons and one subtraction per branch. No table, no transcendental, and
/// no division: every value it can return is a difference of exactly
/// representable constants and the input.
fn triangle(phase: f32) -> f32 {
    if phase < 1.0 {
        phase
    } else if phase < 3.0 {
        2.0 - phase
    } else {
        phase - 4.0
    }
}

#[cfg(test)]
mod tests {
    use super::{Shake, triangle};

    /// The wave is continuous at the seams and hits its corners exactly.
    #[test]
    fn the_triangle_runs_zero_one_zero_minus_one_zero() {
        for (phase, expected) in [
            (0.0_f32, 0.0_f32),
            (0.5, 0.5),
            (1.0, 1.0),
            (1.5, 0.5),
            (2.0, 0.0),
            (2.5, -0.5),
            (3.0, -1.0),
            (3.5, -0.5),
        ] {
            assert_eq!(
                triangle(phase).to_bits(),
                expected.to_bits(),
                "triangle({phase}) should be {expected}"
            );
        }
    }

    /// The decay is exact on powers of two, and the end is a hard stop.
    ///
    /// Amplitude 8, decay 0.5, cutoff 0.5: the ticks give 4, 2, 1, then 0.5,
    /// which is *at* the cutoff and therefore snaps to exactly zero rather than
    /// continuing to 0.25. Every value is dyadic, so this is on `to_bits`.
    #[test]
    fn the_amplitude_halves_and_then_stops_at_the_cutoff() {
        let mut shake = Shake::new(8.0, 1.0, 0.5, 0.5);
        for expected in [4.0_f32, 2.0, 1.0] {
            shake.advance();
            assert_eq!(shake.amplitude.to_bits(), expected.to_bits());
            assert!(!shake.at_rest());
        }

        shake.advance();
        assert!(
            shake.at_rest(),
            "0.5 is at the cutoff, so the shake is over"
        );
        assert_eq!(shake.amplitude.to_bits(), 0.0_f32.to_bits());
        assert_eq!(shake.phase.to_bits(), 0.0_f32.to_bits());
        assert_eq!(shake.offset(), (0.0, 0.0));
    }

    /// Once at rest it stays there, however many ticks follow.
    #[test]
    fn a_finished_shake_stays_finished() {
        let mut shake = Shake::new(1.0, 1.0, 0.5, 0.75);
        shake.advance();
        assert!(shake.at_rest());

        for _ in 0..100 {
            shake.advance();
            assert_eq!(shake.amplitude.to_bits(), 0.0_f32.to_bits());
            assert_eq!(shake.offset(), (0.0, 0.0));
        }
    }

    /// The phase wraps and stays inside one period.
    #[test]
    fn the_phase_stays_inside_one_period() {
        let mut shake = Shake::new(1.0, 1.5, 1.0, -1.0);
        for _ in 0..64 {
            shake.advance();
            assert!(
                (0.0..4.0).contains(&shake.phase),
                "phase left its period: {}",
                shake.phase
            );
        }
    }

    /// A pathological frequency produces a wrong picture, never a wedged tick.
    ///
    /// The `while phase >= PERIOD` this replaced looped forever on an infinite
    /// frequency, because `inf - 4.0` is `inf`. A component is allowed to hold
    /// nonsense — `Layer` takes a `NaN` depth — but a tick that never returns is
    /// a different kind of failure, and one a test can only catch by running it.
    /// If this hangs, that is the finding.
    #[test]
    fn an_impossible_frequency_still_terminates() {
        for frequency in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 1e30, -1e30] {
            let mut shake = Shake::new(1.0, frequency, 1.0, -1.0);
            for _ in 0..8 {
                shake.advance();
            }
            let (x, y) = shake.offset();
            println!(
                "frequency {frequency} settles at phase {} -> ({x}, {y})",
                shake.phase
            );
        }
    }

    /// Arming takes the maximum and leaves the phase where it was.
    #[test]
    fn arming_takes_the_stronger_impulse_and_keeps_the_phase() {
        let mut shake = Shake::new(4.0, 1.0, 0.5, 0.1);
        shake.advance();
        let mid_swing = shake.phase;

        shake.arm(1.0);
        assert_eq!(
            shake.amplitude.to_bits(),
            2.0_f32.to_bits(),
            "a weaker impulse must not cut a running shake short"
        );

        shake.arm(9.0);
        assert_eq!(shake.amplitude.to_bits(), 9.0_f32.to_bits());
        assert_eq!(
            shake.phase.to_bits(),
            mid_swing.to_bits(),
            "re-arming must not teleport the camera by resetting the phase"
        );
    }

    /// Arming a shake that is at rest starts it again.
    #[test]
    fn arming_revives_a_finished_shake() {
        let mut shake = Shake::new(1.0, 1.0, 0.5, 0.75);
        shake.advance();
        assert!(shake.at_rest());

        shake.arm(3.0);
        assert!(!shake.at_rest());
        shake.advance();
        assert_eq!(shake.amplitude.to_bits(), 1.5_f32.to_bits());
    }

    /// The RON round trip is bit exact, as ADR-0014 requires of a float
    /// component.
    ///
    /// The obligation the M3.30 audit caught missing on `Follow`; it is not
    /// going to be caught missing twice.
    #[test]
    fn the_ron_round_trip_is_bit_exact_on_values_that_stress_it() {
        let one_tenth = 0.1_f32;
        for value in [
            one_tenth,
            f32::from_bits(one_tenth.to_bits() + 1),
            -1.0 / 3.0,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::MAX,
            -f32::MAX,
            -0.0,
            0.0,
            core::f32::consts::PI,
        ] {
            let original = Shake {
                amplitude: value,
                frequency: -value,
                decay: value,
                phase: -value,
                cutoff: value,
                base_x: -value,
                base_y: value,
            };
            let text = ron::to_string(&original).expect("a shake serializes");
            let returned: Shake = ron::from_str(&text)
                .unwrap_or_else(|error| panic!("{text} does not parse back: {error}"));

            println!("{:#010x} -> {text}", value.to_bits());
            for (field, before, after) in [
                ("amplitude", original.amplitude, returned.amplitude),
                ("frequency", original.frequency, returned.frequency),
                ("decay", original.decay, returned.decay),
                ("phase", original.phase, returned.phase),
                ("cutoff", original.cutoff, returned.cutoff),
                ("base_x", original.base_x, returned.base_x),
                ("base_y", original.base_y, returned.base_y),
            ] {
                assert_eq!(
                    before.to_bits(),
                    after.to_bits(),
                    "{field} did not survive the round trip bit for bit: {:#010x} \
                     became {:#010x}. A shake that loses bits here loses them in \
                     the canonical dump too, and a replay built on it shakes the \
                     camera differently while every hash reports agreement.",
                    before.to_bits(),
                    after.to_bits()
                );
            }
        }
    }
}
