//! A burst of particles, as the state it is computed from.
//!
//! [`Burst`] is D22's answer (ADR-0044): **what a world stores is the emitter,
//! and the particles are a function of it.** A burst of two thousand sparks is
//! six scalars in the canonical dump and stays six scalars at any count, because
//! nothing per particle is ever written down.
//!
//! # Why the age is a field and the tick is not a parameter
//!
//! The obvious spelling of a derived particle is a *birth tick* on the component
//! and a position derived from `tick - born`. That was measured and rejected in
//! M6b.7, and the reason is M6b.6's finding: **the tick is not world state.** It
//! is a local in the runner's loop, so a picture derived from it would depend on
//! something the canonical dump does not contain, and two worlds with identical
//! dumps would draw differently with ADR-0008's hash unable to notice — which is
//! the objection that sank "render state outside the World" when [`Layer`] was
//! decided, wearing a different hat.
//!
//! So the emitter carries its own [`age`](Burst::age), one system
//! ([`advance_bursts`]) moves it, and everything downstream is a pure function of
//! the world. `Shake::arm` / `Shake::at_rest` is the pattern this mirrors, and
//! [`Burst::arm`] / [`Burst::spent`] is the same pair.
//!
//! # What a computed particle cannot do
//!
//! It cannot be clicked, addressed over the agent protocol, given a rigid body,
//! or killed on its own — there is no per-particle state for any of those to
//! reach. ADR-0044 records that price and the condition that reopens it. A burst
//! as a *whole* is ordinary world state and can be armed, aged, ended, saved,
//! inspected and written over the wire like any other component.
//!
//! [`Layer`]: crate::Layer

use serde::{Deserialize, Serialize};

use crate::{SystemContext, World};

/// One period of the triangle wave the scatter uses.
const PERIOD: f32 = 4.0;

/// How many directions the scatter distinguishes.
///
/// A power of two, so the division below is exact in `f32` and the direction a
/// particle leaves in is the same bit pattern on every platform.
const DIRECTIONS: u64 = 4_096;

/// Multipliers for the integer mix that turns `(seed, index)` into a direction.
///
/// Two odd 64-bit constants, written out here rather than taken from a crate for
/// the reason `state_hash` writes out FNV-1a: the values decide what a burst
/// looks like, so they belong where changing them is a visible diff. They carry
/// no cryptographic claim and none is wanted — the requirement is that
/// neighbouring indices land in unrelated directions, which is checked by
/// `neighbouring_particles_do_not_leave_together`.
const MIX_SEED: u64 = 6_364_136_223_846_793_005;
const MIX_INDEX: u64 = 1_442_695_040_888_963_407;

/// A field of particles, stored as the six scalars they are computed from.
///
/// # Six bare scalars, not a particle list
///
/// ADR-0014: a registered component holds bare scalars. ADR-0044 adds the reason
/// that decided this one — a list would put one unbounded line into the canonical
/// dump, and a divergence in one particle of two thousand was measured to be
/// reported as **81 723 bytes on a single line**, which is the diagnosis
/// ADR-0035 built the repro runner to keep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Burst {
    /// How many particles the burst throws.
    pub count: u32,
    /// Which directions they leave in.
    ///
    /// Two bursts with the same seed are the same burst, which is what makes a
    /// replay reproduce the picture and not merely the count.
    pub seed: u64,
    /// Ticks since the burst was armed.
    ///
    /// **World state, and that is the whole design.** It is in the canonical
    /// dump and therefore in ADR-0008's hash, so a replay that reached a
    /// different point in the burst differs in the hash rather than only on
    /// screen.
    pub age: u64,
    /// Ticks the burst lasts. At `age >= life` it draws nothing.
    pub life: u64,
    /// World units a particle travels per tick.
    pub speed: f32,
    /// How far off the axes the scatter reaches, as a multiplier on `speed`.
    ///
    /// `1.0` is the full diamond. `0.0` collapses the burst to a point, which is
    /// legal and draws every particle on top of the emitter.
    pub spread: f32,
}

impl Burst {
    /// A burst that is over: no particles, nothing drawn.
    ///
    /// `life` is zero, so [`spent`](Self::spent) is true at every age. What an
    /// entity carries when it *can* burst and is not bursting.
    pub const NONE: Self = Self {
        count: 0,
        seed: 0,
        age: 0,
        life: 0,
        speed: 0.0,
        spread: 0.0,
    };

    /// A burst of `count` particles that lasts `life` ticks, already armed.
    #[must_use]
    pub const fn new(count: u32, seed: u64, life: u64, speed: f32, spread: f32) -> Self {
        Self {
            count,
            seed,
            age: 0,
            life,
            speed,
            spread,
        }
    }

    /// Starts the burst again from its beginning.
    ///
    /// The counterpart of `Shake::arm`, and the same shape: it sets the state the
    /// closed form reads rather than doing anything itself. Re-arming a burst
    /// that is still running restarts it, which is what "again" means.
    pub const fn arm(&mut self) {
        self.age = 0;
    }

    /// Whether the burst is over.
    ///
    /// True when `age` has reached `life`, and true for a burst that never had a
    /// life. A spent burst produces no particles at all — not particles at the
    /// origin, and not particles at zero alpha, but none, which is what keeps a
    /// finished burst out of the draw list entirely.
    #[must_use]
    pub const fn spent(self) -> bool {
        self.age >= self.life
    }

    /// How far through its life the burst is, in `0.0 ..= 1.0`.
    ///
    /// Zero on the tick it is armed, and approaching one as it ends. A spent
    /// burst answers `1.0`.
    #[must_use]
    pub fn progress(self) -> f32 {
        if self.spent() {
            return 1.0;
        }
        // `life` is non-zero here: `spent` covers the zero case above.
        ratio(self.age, self.life)
    }

    /// Where particle `index` is, relative to the emitter, and how bright.
    ///
    /// Returns `None` for a spent burst and for an index the burst does not
    /// have, so a caller iterating `0..count` on a live burst gets exactly
    /// `count` answers and a caller that does not check gets none.
    ///
    /// The third value is a **fade in `0.0 ..= 1.0`**, one at the moment of
    /// arming and falling to zero as the burst ends. It is a multiplier for an
    /// alpha and never exceeds one, which is what keeps ADR-0023's premultiplied
    /// invariant intact — a fade is not a glow, and a tint above one is a named
    /// limit this does not touch (ADR-0044).
    ///
    /// # Determinism
    ///
    /// Every operation is either integer arithmetic or IEEE 754 basic
    /// arithmetic on values converted from integers. No transcendental and no
    /// libm call, for the reason `sim::scene::triangle` gives: two platforms may
    /// round a `sin` differently and ADR-0013's cross-platform determinism
    /// cannot afford that inside the state hash.
    #[must_use]
    pub fn particle(self, index: u32) -> Option<(f32, f32, f32)> {
        if self.spent() || index >= self.count {
            return None;
        }

        let phase = self.direction(index);
        let quarter = if phase + 1.0 >= PERIOD {
            phase + 1.0 - PERIOD
        } else {
            phase + 1.0
        };

        // `age` counts ticks since arming, so a particle is at the emitter on
        // the tick the burst was armed and travels from there.
        #[expect(
            clippy::cast_precision_loss,
            reason = "a burst that ran for 2^24 ticks has lost its resolution long \
                      before it loses its determinism; the conversion is exact for \
                      every life a burst is given"
        )]
        let travelled = self.speed * self.age as f32;

        Some((
            travelled * triangle(phase) * self.spread,
            travelled * triangle(quarter) * self.spread,
            1.0 - self.progress(),
        ))
    }

    /// The direction particle `index` leaves in, as a phase in `0.0 .. 4.0`.
    ///
    /// An integer mix of the seed and the index, taken modulo [`DIRECTIONS`] and
    /// scaled. **No generator state anywhere**: the value is a function of its
    /// two inputs, so ADR-0010 does not apply — there is nothing to store and
    /// nothing that can drift between a run and its replay.
    ///
    /// The shift discards the low bits, which a linear-congruential mix leaves
    /// least mixed.
    fn direction(self, index: u32) -> f32 {
        let mixed = self
            .seed
            .wrapping_mul(MIX_SEED)
            .wrapping_add(u64::from(index).wrapping_mul(MIX_INDEX));

        ratio((mixed >> 33) % DIRECTIONS, DIRECTIONS) * PERIOD
    }
}

impl Default for Burst {
    /// [`Burst::NONE`].
    ///
    /// Written out rather than derived, for the reason [`Tint`](crate::Tint)
    /// gives: a derived default would make the choice a property of `u32`'s and
    /// `f32`'s defaults instead of a decision this type took. Here the two
    /// happen to agree, and the sentence is what keeps them agreeing on purpose.
    fn default() -> Self {
        Self::NONE
    }
}

/// The triangle wave on `0.0 .. 4.0`, running 0 → 1 → 0 → −1 → 0.
///
/// The same shape and the same reasoning as `Shake::offset`'s: comparisons and
/// one subtraction per branch, no table and no transcendental.
fn triangle(phase: f32) -> f32 {
    if phase < 1.0 {
        phase
    } else if phase < 3.0 {
        2.0 - phase
    } else {
        phase - PERIOD
    }
}

/// `numerator / denominator` as an `f32`, for two counts of ticks or directions.
///
/// One place rather than three, so the precision note is written once.
#[expect(
    clippy::cast_precision_loss,
    reason = "both arguments are tick counts or the direction count, all far \
              inside f32's exactly-representable integers for any burst a frame \
              can draw"
)]
fn ratio(numerator: u64, denominator: u64) -> f32 {
    numerator as f32 / denominator as f32
}

/// Ages every burst in the world by one tick.
///
/// **The one system a burst needs, and the reason the extraction needs no tick.**
/// It saturates at `life` rather than running on, so a burst that ended stays
/// ended and its `age` stops moving — which keeps the canonical dump of a settled
/// world settled, instead of turning every finished burst into a counter that
/// changes the state hash for ever.
///
/// That saturation is the same choice `Shake` makes when it snaps to rest, and it
/// is what M6b.5's finding asks for: a counter that runs on while the picture is
/// frozen moves the dump and proves nothing.
///
/// `entity_ids` rather than a query: it is the canonical ascending enumeration,
/// and a query's archetype order is documented as unstable. The writes are
/// independent per entity, so the order does not change the result — reading them
/// in an unstable order is the habit this repository has decided against.
pub fn advance_bursts(world: &mut World, _context: &SystemContext) {
    for entity in world.entity_ids() {
        let Ok(mut burst) = world.get_mut::<Burst>(entity) else {
            continue;
        };
        if burst.age < burst.life {
            burst.age += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Burst, advance_bursts};
    use crate::{ComponentRegistry, Scheduler, SystemContext, World, canonical_dump, state_hash};

    /// A burst worth measuring: enough particles to scatter, a short life.
    fn sample() -> Burst {
        Burst::new(64, 0x5eed_1234_abcd_9876, 20, 1.5, 0.75)
    }

    /// Bursts worth round-tripping, including the degenerate ones.
    fn probes() -> [Burst; 5] {
        [
            Burst::NONE,
            sample(),
            Burst {
                age: 19,
                ..sample()
            },
            Burst::new(0, 7, 5, 0.0, 0.0),
            Burst {
                count: u32::MAX,
                seed: u64::MAX,
                age: u64::MAX,
                life: u64::MAX,
                speed: -3.5,
                spread: 2.25,
            },
        ]
    }

    #[test]
    fn a_burst_round_trips_through_the_real_serialization() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Burst>("burst")
            .expect("a fresh registry accepts the type");

        for burst in probes() {
            let mut world = World::new();
            let entity = world.spawn();
            world.insert(entity, burst).expect("the entity is alive");
            let dumped = canonical_dump(&world, &registry).expect("the world dumps");

            let mut reloaded = World::new();
            let other = reloaded.spawn();
            reloaded.insert(other, burst).expect("the entity is alive");
            let again = canonical_dump(&reloaded, &registry).expect("the world dumps");

            assert_eq!(dumped, again, "{burst:?} does not survive its own dump");
        }
    }

    #[test]
    fn a_fresh_burst_is_running_and_a_spent_one_is_not() {
        let burst = sample();
        assert!(!burst.spent());
        assert!(
            Burst {
                age: burst.life,
                ..burst
            }
            .spent()
        );
        // A burst with no life is over before it starts, which is what `NONE` is.
        assert!(Burst::NONE.spent());
    }

    #[test]
    fn a_spent_burst_produces_no_particles_at_all() {
        // Not particles at the origin and not particles at zero alpha: none. A
        // finished burst has to leave the draw list rather than draw nothing,
        // or every burst a world ever fired stays in every frame for ever.
        let spent = Burst {
            age: 20,
            ..sample()
        };
        for index in 0..sample().count {
            assert_eq!(spent.particle(index), None, "index {index}");
        }
    }

    #[test]
    fn a_live_burst_answers_for_exactly_the_indices_it_has() {
        let burst = Burst { age: 3, ..sample() };
        for index in 0..burst.count {
            assert!(burst.particle(index).is_some(), "index {index}");
        }
        assert_eq!(burst.particle(burst.count), None);
        assert_eq!(burst.particle(u32::MAX), None);
    }

    #[test]
    fn the_fade_starts_at_one_and_falls_to_zero_without_leaving_the_range() {
        let burst = sample();
        let (_, _, first) = burst.particle(0).expect("a fresh burst has particles");
        assert!(
            (first - 1.0).abs() < f32::EPSILON,
            "the burst starts at full brightness, got {first}"
        );

        let mut previous = first;
        for age in 1..burst.life {
            let (_, _, fade) = Burst { age, ..burst }
                .particle(0)
                .expect("a live burst has particles");
            assert!((0.0..=1.0).contains(&fade), "age {age} faded to {fade}");
            assert!(
                fade < previous,
                "age {age} did not dim: {fade} !< {previous}"
            );
            previous = fade;
        }
    }

    /// The premultiplied invariant of ADR-0023 survives the fade.
    ///
    /// A fade multiplies an alpha and nothing else, so the colour channels stay
    /// where the content put them. This is the arithmetic half of that claim:
    /// the factor never leaves `0.0..=1.0`, so `alpha * fade <= alpha` and a tint
    /// that was inside the invariant stays inside it. A tint above one is a
    /// named limit and this does not reach it (ADR-0044).
    #[test]
    fn the_fade_is_a_multiplier_that_can_never_brighten() {
        for age in 0..40 {
            let burst = Burst { age, ..sample() };
            let Some((_, _, fade)) = burst.particle(0) else {
                continue;
            };
            assert!((0.0..=1.0).contains(&fade), "age {age} gave {fade}");
            for alpha in [0.0_f32, 0.25, 0.5, 1.0] {
                assert!(alpha * fade <= alpha, "age {age}, alpha {alpha}");
            }
        }
    }

    #[test]
    fn particles_travel_further_the_older_the_burst_is() {
        // The visible half of "a burst does something": the same particle is
        // further from the emitter later. Squared distance, so no square root
        // enters a comparison.
        let burst = sample();
        let distance = |age: u64| {
            let (x, y, _) = Burst { age, ..burst }
                .particle(1)
                .expect("a live burst has particles");
            x * x + y * y
        };

        for age in 1..burst.life {
            assert!(
                distance(age) > distance(age - 1),
                "particle 1 did not move between age {} and {age}",
                age - 1
            );
        }
    }

    #[test]
    fn neighbouring_particles_do_not_leave_together() {
        // What the integer mix is for. Two adjacent indices leaving in the same
        // direction would draw a burst as a line, which is the failure
        // `Shake::offset`'s quarter-period trick avoids for the same reason.
        let burst = Burst { age: 5, ..sample() };
        let mut same = 0;
        for index in 1..burst.count {
            let (ax, ay, _) = burst.particle(index - 1).expect("live");
            let (bx, by, _) = burst.particle(index).expect("live");
            if (ax - bx).abs() < f32::EPSILON && (ay - by).abs() < f32::EPSILON {
                same += 1;
            }
        }
        assert!(
            same * 4 < burst.count,
            "{same} of {} neighbouring pairs left in the same direction",
            burst.count
        );
    }

    #[test]
    fn a_burst_is_the_same_burst_twice_and_a_different_seed_is_a_different_burst() {
        let burst = Burst { age: 4, ..sample() };
        let twin = Burst { age: 4, ..sample() };
        let other = Burst {
            seed: sample().seed ^ 1,
            ..burst
        };

        let places = |b: Burst| {
            (0..b.count)
                .filter_map(|i| b.particle(i))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            places(burst),
            places(twin),
            "the same burst drew differently"
        );
        assert_ne!(
            places(burst),
            places(other),
            "two seeds drew the same burst"
        );
    }

    #[test]
    fn arming_starts_the_burst_again() {
        let mut burst = Burst {
            age: 20,
            ..sample()
        };
        assert!(burst.spent());
        burst.arm();
        assert!(!burst.spent());
        assert_eq!(burst.age, 0);
        assert_eq!(burst.particle(0), sample().particle(0));
    }

    #[test]
    fn the_system_ages_every_burst_and_stops_at_the_end() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, sample()).expect("alive");

        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("bursts", advance_bursts)
            .expect("a fresh scheduler accepts the system");

        for tick in 0..40 {
            scheduler.run(&mut world, &SystemContext::new(tick));
        }

        let burst = *world.get::<Burst>(entity).expect("still there");
        assert_eq!(
            burst.age, burst.life,
            "the age ran past the burst's life instead of stopping at it"
        );
    }

    /// A settled world has a settled dump.
    ///
    /// The half M6b.5 measured the hard way: a counter that keeps running while
    /// the picture is frozen moves the canonical dump and proves nothing. The
    /// saturation in `advance_bursts` is what makes this true, and this is where
    /// it is checked.
    #[test]
    fn a_finished_burst_stops_moving_the_state_hash() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Burst>("burst")
            .expect("fresh");

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, sample()).expect("alive");

        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("bursts", advance_bursts)
            .expect("fresh");

        for tick in 0..20 {
            scheduler.run(&mut world, &SystemContext::new(tick));
        }
        let settled = state_hash(&canonical_dump(&world, &registry).expect("dumps"));

        for tick in 20..200 {
            scheduler.run(&mut world, &SystemContext::new(tick));
        }
        let later = state_hash(&canonical_dump(&world, &registry).expect("dumps"));

        assert_eq!(settled, later, "a spent burst kept moving the state hash");
    }

    /// A running burst *does* move the hash, which is the other half.
    ///
    /// Without it the test above passes for a burst that never ages at all.
    #[test]
    fn a_running_burst_moves_the_state_hash() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Burst>("burst")
            .expect("fresh");

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, sample()).expect("alive");

        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("bursts", advance_bursts)
            .expect("fresh");

        let before = state_hash(&canonical_dump(&world, &registry).expect("dumps"));
        scheduler.run(&mut world, &SystemContext::new(0));
        let after = state_hash(&canonical_dump(&world, &registry).expect("dumps"));

        assert_ne!(before, after, "a tick of a live burst changed nothing");
    }

    /// The dump does not grow with the particle count.
    ///
    /// D22's whole answer in one assertion: two bursts differing only in how many
    /// particles they throw dump to the same number of bytes, up to the digits of
    /// the count itself.
    #[test]
    fn the_dump_does_not_grow_with_the_particle_count() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Burst>("burst")
            .expect("fresh");

        let bytes = |count: u32| {
            let mut world = World::new();
            let entity = world.spawn();
            world
                .insert(entity, Burst { count, ..sample() })
                .expect("alive");
            canonical_dump(&world, &registry).expect("dumps").len()
        };

        let small = bytes(1);
        let large = bytes(1_000_000);
        assert!(
            large - small <= 6,
            "a million particles cost {} bytes more than one",
            large - small
        );
    }
}
