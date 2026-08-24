//! Seeded random numbers whose state is ordinary world state.
//!
//! Read ADR-0010 before changing anything here. The two properties this module
//! exists to hold are that a generator is seeded explicitly and never from the
//! environment, and that its state is a component like any other — so it appears
//! in the canonical dump, in the state hash, and in a diff between two runs that
//! disagree.

use serde::{Deserialize, Serialize};

/// Multiplier of the 64-bit LCG underneath, from the PCG reference
/// implementation (`pcg_basic.c`, `pcg32_random_r`).
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// Stream [`Rng::new`] uses.
///
/// Any value works — the increment derived from it is forced odd below, which is
/// all the generator requires — so this is arbitrary and fixed rather than
/// meaningful. Zero is the least arbitrary arbitrary value, and it yields the
/// increment 1.
const DEFAULT_STREAM: u64 = 0;

/// A seeded pseudo-random generator, storable on an entity.
///
/// PCG-XSH-RR 64/32: a 64-bit linear congruential generator whose state is
/// permuted down to 32 bits of output by an xorshift and a data-dependent
/// rotation. The algorithm is written out in this file rather than taken from a
/// crate, for the reason ADR-0008 gives for FNV-1a and ADR-0010 repeats: the
/// promise wanted here is *stability*, and stability comes from the constants
/// being frozen in this repository, where changing them is a visible diff, and
/// not one dependency bump away.
///
/// # Determinism
///
/// Nothing in here reads the clock, the environment, an operating-system entropy
/// source or a thread-local. A generator is constructed from a seed the caller
/// names and from nothing else, and its whole state is these two integers. Two
/// generators built from the same seed produce the same sequence on every
/// machine: the arithmetic is wrapping integer arithmetic on fixed-width types,
/// which is exactly specified and has no platform freedom in it.
///
/// # The state belongs in the world
///
/// A generator held in a local, a closure capture or a `static` is invisible to
/// the canonical dump, so two runs could disagree about the next draw while
/// every hash said they agreed — a divergence the instrument cannot see. Store
/// it as a component, register it, and it lands in the dump beside everything
/// else. That is the whole reason this type derives `Serialize` and
/// `Deserialize`, and ADR-0010 records it as a decision rather than a habit.
///
/// # Examples
///
/// ```
/// use narvo_ecs::{ComponentRegistry, Rng, World};
///
/// let mut world = World::new();
/// let entity = world.spawn();
/// world.insert(entity, Rng::new(1))?;
///
/// // Registering it is what puts the generator's state into the state hash.
/// let mut registry = ComponentRegistry::new();
/// registry.register_component::<Rng>("rng")?;
///
/// let draw = world.get_mut::<Rng>(entity)?.next_u32_below(6);
/// assert!(draw < 6);
/// # Ok::<(), narvo_ecs::EcsError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng {
    /// The LCG state. Advances on every draw.
    state: u64,
    /// The odd addend that selects the stream. Fixed at construction.
    ///
    /// The reference implementation calls this `inc`; the longer name is for the
    /// reader of a state dump, where this appears by name.
    increment: u64,
}

impl Rng {
    /// Creates a generator from `seed`, on the default stream.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self::with_stream(seed, DEFAULT_STREAM)
    }

    /// Creates a generator from `seed` on the stream `stream`.
    ///
    /// Two generators with the same seed and different streams produce different
    /// sequences, which is what a caller wants when two subsystems must not draw
    /// from one shared sequence and thereby couple their behaviour.
    ///
    /// The seeding procedure is the reference implementation's
    /// `pcg32_srandom_r`, step for step: the increment is `(stream << 1) | 1` so
    /// it is odd, the state starts at zero, and two draws are made and discarded
    /// with the seed added between them. It looks like a ritual and it is one —
    /// it is what mixes a low-entropy seed such as `1` into the state instead of
    /// letting the first few draws inherit its structure. Deviating from it
    /// would invalidate the published test vector this module checks itself
    /// against.
    #[must_use]
    pub fn with_stream(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            increment: (stream << 1) | 1,
        };

        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();

        rng
    }

    /// Draws the next 32-bit value and advances the state.
    ///
    /// Every bit of the result is usable; there is no weak low bit to avoid, as
    /// there is in a bare LCG. That is what the output permutation buys.
    pub fn next_u32(&mut self) -> u32 {
        // The output is computed from the state *before* the step, matching the
        // reference implementation. Computing it from the stepped state would
        // produce a differently-phased sequence that no published vector covers.
        let previous = self.state;
        self.state = previous
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(self.increment);

        let xorshifted = (((previous >> 18) ^ previous) >> 27) as u32;
        let rotation = (previous >> 59) as u32;

        // The reference writes this as
        // `(xorshifted >> rot) | (xorshifted << ((-rot) & 31))`. For a rotation
        // in `0..32` — which `previous >> 59` always is — that is exactly a
        // 32-bit rotate right, and `rotate_right` says so without the reader
        // having to check that the shift count can never reach 32. The published
        // test vector below covers the equivalence.
        xorshifted.rotate_right(rotation)
    }

    /// Draws the next value in `0..bound`, without bias.
    ///
    /// The obvious `next_u32() % bound` is biased whenever `bound` does not
    /// divide 2³²: the low residues occur once more often than the high ones.
    /// This is the reference implementation's rejection scheme — draws below a
    /// threshold are discarded and redrawn, leaving a range that `bound` divides
    /// exactly.
    ///
    /// The loop is unbounded in principle and terminates with probability one;
    /// the chance of even a second iteration is under one in two for every
    /// `bound`, and the number of draws consumed is a deterministic function of
    /// the state, so a rejection costs a run nothing in reproducibility.
    ///
    /// # Panics
    ///
    /// Panics if `bound` is zero. There is no value in `0..0` to return, and a
    /// caller asking for one has a bug that is cheaper to find here than three
    /// systems later.
    pub fn next_u32_below(&mut self, bound: u32) -> u32 {
        assert!(
            bound > 0,
            "Rng::next_u32_below needs a bound above zero; `0..0` contains no value to draw"
        );

        // `bound.wrapping_neg() % bound` is `2^32 % bound` written so it fits in
        // 32 bits: the count of low values that would make the range uneven.
        let threshold = bound.wrapping_neg() % bound;

        loop {
            let draw = self.next_u32();
            if draw >= threshold {
                return draw % bound;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    /// The published test vector, and the reason this module can claim to
    /// implement PCG rather than something that merely looks like it.
    ///
    /// Source: the PCG reference implementation `imneme/pcg-c-basic`. Its
    /// `pcg32-demo.c` seeds with `pcg32_srandom_r(&rng, 42u, 54u)` when run
    /// without `-r`, and the sample output published at
    /// <https://www.pcg-random.org/using-pcg-c-basic.html> shows the first six
    /// 32-bit draws of round 1 as the values asserted below.
    ///
    /// These literals are allowed for the same reason the FNV vectors in
    /// `state.rs` are, and the distinction is the one ADR-0008 draws: they pin
    /// an *algorithm* against its publication, and nothing about them moves when
    /// `ron`, `serde` or the compiler changes. A hash of a *state* would.
    #[test]
    fn the_published_reference_seed_reproduces_the_published_output() {
        let mut rng = Rng::with_stream(42, 54);

        let drawn: Vec<u32> = (0..6).map(|_| rng.next_u32()).collect();

        assert_eq!(
            drawn,
            vec![
                0xa15c_02b7,
                0x7b47_f409,
                0xba1d_3330,
                0x83d2_f293,
                0xbfa4_784b,
                0xcbed_606e,
            ]
        );
    }

    #[test]
    fn the_constants_are_the_published_ones() {
        // A typo in either would still pass every "two runs agree" test in this
        // workspace, because both runs would be wrong together. Only a published
        // value catches it, which is the whole argument of ADR-0008 applied to
        // the generator instead of to the hash.
        assert_eq!(super::MULTIPLIER, 6_364_136_223_846_793_005);
        assert_eq!(
            Rng::new(0).increment,
            1,
            "stream 0 must give the odd increment 1"
        );
    }

    #[test]
    fn the_same_seed_produces_the_same_sequence() {
        let mut first = Rng::new(12_345);
        let mut second = Rng::new(12_345);

        let left: Vec<u32> = (0..64).map(|_| first.next_u32()).collect();
        let right: Vec<u32> = (0..64).map(|_| second.next_u32()).collect();

        assert_eq!(left, right);
        assert_eq!(first, second, "the states must have advanced identically");
    }

    #[test]
    fn a_different_seed_produces_a_different_sequence() {
        // Without this the test above passes on a generator that returns a
        // constant, which is the same defect as a state hash that ignores its
        // input.
        let mut first = Rng::new(1);
        let mut second = Rng::new(2);

        let left: Vec<u32> = (0..64).map(|_| first.next_u32()).collect();
        let right: Vec<u32> = (0..64).map(|_| second.next_u32()).collect();

        assert_ne!(left, right);
    }

    #[test]
    fn a_different_stream_produces_a_different_sequence() {
        let mut first = Rng::with_stream(7, 1);
        let mut second = Rng::with_stream(7, 2);

        let left: Vec<u32> = (0..64).map(|_| first.next_u32()).collect();
        let right: Vec<u32> = (0..64).map(|_| second.next_u32()).collect();

        assert_ne!(left, right);
    }

    #[test]
    fn drawing_advances_the_state() {
        let mut rng = Rng::new(99);
        let before = rng.clone();

        rng.next_u32();

        assert_ne!(before, rng);
    }

    #[test]
    fn a_bounded_draw_stays_inside_its_bound() {
        let mut rng = Rng::new(2024);

        for bound in [1_u32, 2, 3, 5, 6, 7, 32, 1000, u32::MAX] {
            for _ in 0..64 {
                assert!(
                    rng.next_u32_below(bound) < bound,
                    "bound {bound} was exceeded"
                );
            }
        }
    }

    #[test]
    fn a_bound_of_one_always_draws_zero() {
        let mut rng = Rng::new(5);

        for _ in 0..32 {
            assert_eq!(rng.next_u32_below(1), 0);
        }
    }

    #[test]
    fn a_bounded_draw_reaches_every_value_it_can() {
        // A rejection scheme that rejected too much would silently narrow the
        // range; this notices that where a bounds check cannot.
        let mut rng = Rng::new(77);
        let mut seen = [false; 6];

        for _ in 0..600 {
            seen[rng.next_u32_below(6) as usize] = true;
        }

        assert!(
            seen.iter().all(|hit| *hit),
            "some face of the die never came up"
        );
    }

    #[test]
    fn bounded_draws_are_reproducible() {
        let mut first = Rng::new(31);
        let mut second = Rng::new(31);

        // Deliberately a bound that does not divide 2^32, so the rejection path
        // is actually reachable and has to be reproducible along with the rest.
        let left: Vec<u32> = (0..256).map(|_| first.next_u32_below(6)).collect();
        let right: Vec<u32> = (0..256).map(|_| second.next_u32_below(6)).collect();

        assert_eq!(left, right);
    }

    #[test]
    #[should_panic(expected = "needs a bound above zero")]
    fn a_bound_of_zero_panics_rather_than_dividing_by_zero() {
        Rng::new(0).next_u32_below(0);
    }

    #[test]
    fn a_generator_serializes_as_its_two_fields() {
        // The dump is the diagnostic when two runs disagree, so what a generator
        // looks like in it is part of the observable surface rather than an
        // implementation detail.
        let rng = Rng::with_stream(0, 0);

        let text = ron::to_string(&rng).expect("a generator always serializes");
        assert!(text.starts_with("(state:"), "unexpected rendering: {text}");
        assert!(text.contains("increment:1"), "unexpected rendering: {text}");

        let restored: Rng = ron::from_str(&text).expect("what was just written reads back");
        assert_eq!(restored, rng);
    }
}
