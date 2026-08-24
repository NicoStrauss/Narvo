//! A camera that tracks an entity: the [`Follow`] component and its system.
//!
//! [`Follow`] is simulation state for the same reason [`Camera`] is: M3.12 made
//! the camera a component rather than a render setting, so **anything that
//! writes a camera each tick writes world state**. A follow whose smoothed
//! position lived outside the world would make a replay draw a different picture
//! with ADR-0008's hash blind to it — the gap M3.12 closed for the camera itself
//! and M3.23 closed for the sampler wish.
//!
//! **"In the hash" is a property of a scenario, not of the type**, and saying
//! otherwise is the error §6.10 records twice. Until M3.32 no scenario
//! registered either, because none of the demo simulations drew a sprite, and
//! this paragraph said so.
//!
//! **One does now.** `narvo-app`'s `scene` mode draws, and it registers
//! `Transform`, `Layer`, `Sampling`, `Camera`, `Follow` and `Shake` together —
//! discharging the standing obligation of `ProjektPlan.md` §12 for the first
//! time. So the conditional that used to be the only true statement here — *a
//! scenario that registers the camera gets the follow in its hash too, or its
//! replay is not a replay* — now has an instance, and the obligation stays
//! exactly as binding for the next scenario that draws.
//!
//! # Where the behaviour lives, and why it is here
//!
//! [`compose_camera`] reads a target's [`Transform`] and writes a [`Camera`].
//! Both are this crate's types and the renderer never sees either: ADR-0015
//! keeps `narvo-render2d` free of the ECS, so the system cannot live there.
//! The remaining candidates were this crate and `narvo-app`.
//!
//! **`narvo-app` was rejected, and the deciding fact is structural rather than
//! aesthetic: it is a binary crate with no library target.** Its demo
//! simulations do define their own components — `Position` and `Velocity` in
//! `sim.rs`, `Wobble` in `sim/motion.rs` — so the precedent for putting one there
//! exists, but everything put there is unreachable from every other crate,
//! permanently. That is right for a demo's `Wobble` and wrong for a camera
//! capability the engine offers. `Layer` and `Sampling` are the closer
//! precedent: the *data* lives here beside the camera it describes.
//!
//! What `narvo-app` keeps is the part that needs both halves — the rendered
//! witness in `sprite_batch.rs`, because a test that ticks a world *and* draws
//! it can only live in the one crate that sees the ECS and the renderer.
//!
//! # The composition point (M3.31)
//!
//! M3.30 left one question open — **who owns the camera when two contributors
//! want to write it** — and M3.31 answers it here. The answer has two halves and
//! both are needed:
//!
//! 1. **Each contributor owns its own state.** `Follow` keeps its smoothed point
//!    in [`Follow::x`] and [`Follow::y`]; [`Shake`] keeps its amplitude and
//!    phase. Neither reads the camera back, so neither can be fed its own output
//!    on the next tick.
//! 2. **[`compose_camera`] is the single writer**, and it computes
//!    `camera = base + Σ offsets` from those states. It is the only place in the
//!    workspace that assigns to a [`Camera`].
//!
//! That makes the camera a **pure function of the components after the tick**,
//! which is a property a test can check rather than a convention a reader has to
//! keep — `the_camera_is_exactly_the_base_plus_the_offset` recomputes it and
//! fails if a second writer appears or the order flips.
//!
//! *Rejected: last writer with a system order.* Follow writes, then a shake
//! system adds. It works while both run and both write, and breaks quietly when
//! one does not: a lost follow stops writing, so the shake's offset accumulates
//! onto its own previous output. Worse, nothing in the resulting value says who
//! produced it, and a third behaviour has to be inserted into an order that is
//! only implied by registration.
//!
//! *Rejected: composing at view-build time*, with `Camera` staying follow-pure
//! and `camera_of` adding the offset. Its ADR-0008 story is actually the
//! prettiest — the offset is state, the effective view a derived function, so
//! nothing composed is ever stored to disagree with. It loses on reach:
//! `camera_of` lives in `narvo-app`, so the camera's meaning would be split
//! across two crates and **every other view-builder would silently lose the
//! shake** — the same shape as the silent mis-following M3.30 excluded. The
//! offset stays a component, which is that candidate's good half; the composing
//! moves here.
//!
//! **A third behaviour costs one term.** Bounds or a zoom effect adds a
//! component for its own state and one line at the composition below; it does
//! not need to know what else contributes, and it cannot be forgotten by a
//! caller.

use serde::{Deserialize, Serialize};

use crate::{Camera, EntityId, Shake, SystemContext, Transform, World};

/// A camera's tracking of one entity, and the smoothed point it has reached.
///
/// Put it on the same entity as the [`Camera`] it drives. [`compose_camera`]
/// updates every entity that carries both.
///
/// # The fields are bare scalars, and the one that is not is argued
///
/// ADR-0014 keeps computation-library types out of registered components.
/// [`EntityId`] is not one: it is this crate's own identity type, and its serde
/// form is pinned as stable API in `entity.rs` precisely because M2.2 hashes
/// over serialized state. Storing the target as two loose `u32` fields instead
/// would serialize identically and **throw away the protection that makes this
/// component safe** — see the slot-reuse section below. So the handle is stored
/// as a handle.
///
/// # Slot reuse cannot make the camera follow the wrong entity
///
/// The hazard: a target dies, its slot is handed out again, and the camera
/// silently tracks a stranger. **The facade already closes it, and this
/// component inherits the closure rather than re-implementing it.**
///
/// `ProjektPlan.md` §12 does carry a slot-reuse surface, and it is worth being
/// exact that it is **a different one** — it says slot reuse breaks "later
/// spawned means later drawn", which is about draw order and not about a stale
/// handle resolving. The camera-retargeting reading is M3.30's own framing, and
/// it is the one this section answers.
/// An [`EntityId`] is a slot *and* a generation, and `entity.rs` states the
/// consequence: a handle kept across a despawn "becomes permanently invalid and
/// every access through it returns `EcsError::NoSuchEntity`". So a recycled slot
/// produces a *failed* lookup, not a wrong one, and the failure has defined
/// behaviour — [`Follow::lost`].
///
/// `slot_reuse_invalidates_the_handle_rather_than_retargeting_it` constructs the
/// case rather than trusting this paragraph.
///
/// # Smoothing is not validated
///
/// [`Follow::smoothing`] is the fraction of the remaining distance covered per
/// tick: `1.0` snaps, `0.5` halves the gap, `0.0` never moves. Values outside
/// `0.0 ..= 1.0` overshoot or diverge and are **not** rejected, for the reason
/// `Sampling` gives for an unknown code and `Layer` for a `NaN` depth: a
/// component is storage and does not validate. What consumes it decides, and
/// here the consumer is arithmetic that has no opinion.
///
/// # The registration obligation, stated rather than assumed
///
/// The duty is `ProjektPlan.md` §12's, unchanged — **a scenario that draws
/// sprites and follows something registers `Transform`, `Layer`, `Sampling`,
/// `Camera`, `Follow` and — since M3.31 — `Shake` together, or its replay is not
/// a replay.**
///
/// Since M3.32 it is discharged in one place: `narvo-app`'s `scene` mode, which
/// is the first scenario in this repository that draws. Every other
/// `register_component::<Follow>` call is in this file's own tests, whose
/// fixture worlds are never dumped, hashed, recorded or replayed except where a
/// hash is deliberately being compared. A second drawing scenario inherits the
/// duty in full; nothing about it is now automatic.
///
/// # Once lost, it stays lost
///
/// [`Follow::lost`] latches. Nothing in [`compose_camera`] clears it, because the
/// handle that failed can never succeed again — a despawned entity's id is
/// invalid for the life of the world. A caller that wants to retarget writes a
/// fresh `Follow` with a live handle, which is one `insert` and is the same
/// operation as aiming it in the first place.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Follow {
    /// The entity whose [`Transform`] the camera tracks.
    pub target: EntityId,
    /// Fraction of the remaining distance covered per tick. `1.0` is immediate.
    pub smoothing: f32,
    /// The smoothed point along x, in world units.
    pub x: f32,
    /// The smoothed point along y, in world units.
    pub y: f32,
    /// Set once the target has stopped being followable. The camera then holds.
    ///
    /// It is a field rather than a log line or a panic because a
    /// [`System`](crate::System) returns nothing — the signature is
    /// `fn(&mut World, &SystemContext)` — so the only place a system can report
    /// anything is the world. Being in the world also puts it in the canonical
    /// dump and the state hash, which is what makes "the target went away" a
    /// thing a replay reproduces instead of a thing that happens twice
    /// differently.
    pub lost: bool,
}

impl Follow {
    /// Tracks `target` immediately, starting from `(x, y)`.
    ///
    /// `smoothing` is `1.0`, so the camera is on the target from the first tick
    /// on and the starting point only matters before that tick runs.
    #[must_use]
    pub const fn immediate(target: EntityId, x: f32, y: f32) -> Self {
        Self {
            target,
            smoothing: 1.0,
            x,
            y,
            lost: false,
        }
    }

    /// Tracks `target` by covering `smoothing` of the gap each tick.
    #[must_use]
    pub const fn damped(target: EntityId, x: f32, y: f32, smoothing: f32) -> Self {
        Self {
            target,
            smoothing,
            x,
            y,
            lost: false,
        }
    }
}

/// **The camera's single composition point**: advances every contributor on an
/// entity that carries a [`Camera`], then writes the camera from them.
///
/// `camera = base + Σ offsets`, where the base is the [`Follow`]'s smoothed
/// point if there is a live one and [`Shake::base_x`]/[`Shake::base_y`]
/// otherwise, and the only offset today is [`Shake::offset`]. Advance first,
/// compose second — so the camera is a pure function of the state the tick left
/// behind, and `the_camera_is_exactly_the_base_plus_the_offset` can recompute it
/// instead of trusting it.
///
/// **It was called `follow_camera` until M3.36**, from M3.30 when following was
/// all it did. The name outlived that: since M3.31 a follow is one contributor
/// of two and this function's job is the composing, so the name now says
/// the job rather than the first contributor to it. It stays in `follow.rs`
/// because `Follow` is still where the base normally comes from, and moving the
/// module would touch more than the name. ADR-0017 records the rule.
///
/// One tick is one application: `p += (target - p) * smoothing`, evaluated on
/// [`Follow::x`] and [`Follow::y`], and one [`Shake::advance`]. The smoothed
/// point is the follow's own, not the camera's, so the shake cannot feed back
/// into the tracking — **after a shake expires the camera resumes exactly the
/// trail it would have had without one**, which
/// `the_camera_rejoins_the_shake_free_trail_once_the_shake_expires` asserts on
/// bit patterns.
///
/// # What each combination does
///
/// | on the entity | camera |
/// | --- | --- |
/// | `Camera` + `Follow` | the smoothed point, as in M3.30 |
/// | `Camera` + `Follow` + `Shake` | the smoothed point plus the offset |
/// | `Camera` + `Shake` | [`Shake::base_x`]/[`base_y`](Shake::base_y) plus the offset |
/// | `Camera` + a lost `Follow` | untouched — the camera holds, shake or not |
/// | `Camera` alone | untouched |
///
/// # The fixed timestep, and catch-up
///
/// This is a per-tick operation with no wall clock in it (ADR-0003), so *n*
/// ticks in one advance produce exactly what *n* single ticks produce —
/// `catch_up_ticks_match_the_same_ticks_run_singly` pins that on bit patterns.
/// When the runner discards surplus catch-up ticks past
/// `max_ticks_per_advance`, the camera simply does not advance for them, which
/// is the same thing every other system experiences and is ADR-0003's decision
/// rather than this system's.
///
/// # Order
///
/// Entities are visited in `World::entity_ids` order, which is index-major and
/// canonical. Nothing here depends on the order — each camera reads only its own
/// target — but iterating canonically keeps the operation reproducible if that
/// ever stops being true.
///
/// # Losing the target
///
/// If the target no longer exists, or exists without a [`Transform`], the camera
/// **holds its last position** and [`Follow::lost`] is set. It is not a panic:
/// a target that despawns is ordinary in a game, and a system that killed the
/// process for it would be unusable. It is not silence either — the flag is
/// world state, so it reaches the dump, the hash and any test that looks.
///
/// Once lost, a `Follow` stays lost even if a later entity occupies the slot;
/// that follows from the handle being permanently invalid rather than from a
/// rule here.
pub fn compose_camera(world: &mut World, _context: &SystemContext) {
    // Two passes, because a target's `Transform` and the camera's own
    // components cannot be borrowed at the same time. The first pass reads and
    // decides, the second writes; nothing between them can observe the split.
    let mut updates: Vec<(EntityId, Option<Follow>, Option<Shake>)> = Vec::new();

    for entity in world.entity_ids() {
        if world.get::<Camera>(entity).is_err() {
            continue;
        }

        let mut next = world.get::<Follow>(entity).ok().map(|follow| *follow);
        if let Some(follow) = next.as_mut()
            && !follow.lost
        {
            match world.get::<Transform>(follow.target) {
                Ok(target) => {
                    follow.x += (target.x - follow.x) * follow.smoothing;
                    follow.y += (target.y - follow.y) * follow.smoothing;
                }
                Err(_) => follow.lost = true,
            }
        }

        let mut shake = world.get::<Shake>(entity).ok().map(|shake| *shake);
        if let Some(shake) = shake.as_mut() {
            shake.advance();
        }

        if next.is_none() && shake.is_none() {
            continue;
        }
        updates.push((entity, next, shake));
    }

    for (entity, follow, shake) in updates {
        if let Some(follow) = follow
            && let Ok(mut carried) = world.get_mut::<Follow>(entity)
        {
            *carried = follow;
        }
        if let Some(shake) = shake
            && let Ok(mut carried) = world.get_mut::<Shake>(entity)
        {
            *carried = shake;
        }

        // The composition, and the only write to a `Camera` in the workspace.
        // The base comes from the follow when there is one and from the shake's
        // captured base otherwise; a lost follow contributes no base at all, so
        // the camera holds — which is M3.30's rule, unchanged.
        let base = match (follow, shake) {
            (Some(follow), _) if !follow.lost => Some((follow.x, follow.y)),
            (Some(_), _) => None,
            (None, Some(shake)) => Some((shake.base_x, shake.base_y)),
            (None, None) => None,
        };
        let Some((base_x, base_y)) = base else {
            continue;
        };

        let (offset_x, offset_y) = shake.map_or((0.0, 0.0), Shake::offset);
        if let Ok(mut camera) = world.get_mut::<Camera>(entity) {
            camera.x = base_x + offset_x;
            camera.y = base_y + offset_y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Follow, compose_camera};
    use crate::{
        Camera, ComponentRegistry, EntityId, Scheduler, Shake, SystemContext, Transform, World,
        canonical_dump, state_hash,
    };

    /// A world with a camera following a target, both at named starting points.
    fn world_with_follow(smoothing: f32, target_at: (f32, f32)) -> (World, EntityId, EntityId) {
        let mut world = World::new();

        let target = world.spawn();
        world
            .insert(
                target,
                Transform {
                    x: target_at.0,
                    y: target_at.1,
                    ..Transform::IDENTITY
                },
            )
            .expect("the entity was just spawned");

        let eye = world.spawn();
        world
            .insert(eye, Camera::at(0.0, 0.0))
            .expect("the entity was just spawned");
        world
            .insert(eye, Follow::damped(target, 0.0, 0.0, smoothing))
            .expect("the entity was just spawned");

        (world, eye, target)
    }

    fn tick(world: &mut World, n: u64) {
        compose_camera(world, &SystemContext::new(n));
    }

    fn camera_of(world: &World, eye: EntityId) -> (f32, f32) {
        let camera = world.get::<Camera>(eye).expect("the eye carries a camera");
        (camera.x, camera.y)
    }

    /// The registry the hash tests compare through.
    fn registry_with_follow() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts this component");
        registry
            .register_component::<Camera>("camera")
            .expect("a fresh registry accepts this component");
        registry
            .register_component::<Follow>("follow")
            .expect("a fresh registry accepts this component");
        registry
            .register_component::<Shake>("shake")
            .expect("a fresh registry accepts this component");
        registry
    }

    /// At `smoothing = 1.0` the camera is the target, from the first tick.
    ///
    /// On bit patterns, because "the camera reached the target" is exactly the
    /// claim a tolerance would soften into "nearly".
    #[test]
    fn an_immediate_follow_puts_the_camera_on_the_target_in_one_tick() {
        let (mut world, eye, _target) = world_with_follow(1.0, (12.5, -37.25));
        tick(&mut world, 1);

        let (x, y) = camera_of(&world, eye);
        assert_eq!(x.to_bits(), 12.5_f32.to_bits());
        assert_eq!(y.to_bits(), (-37.25_f32).to_bits());
    }

    /// The damped trail, value for value, derived before it ran.
    ///
    /// Start `0`, target `8`, `smoothing = 0.5`, so `p += (8 - p) / 2` gives
    /// 4, 6, 7, 7.5, 7.75 — every one a dyadic rational and therefore exact in
    /// `f32`, which is why this asserts bit patterns rather than a tolerance.
    /// Closed form: `x_n = 8 * (1 - 0.5^n)`.
    #[test]
    fn a_damped_follow_halves_the_gap_every_tick() {
        let (mut world, eye, _target) = world_with_follow(0.5, (8.0, 0.0));

        for (index, expected) in [4.0_f32, 6.0, 7.0, 7.5, 7.75].into_iter().enumerate() {
            let n = index as u64 + 1;
            tick(&mut world, n);
            let (x, _) = camera_of(&world, eye);
            assert_eq!(
                x.to_bits(),
                expected.to_bits(),
                "tick {n} of the damped trail: expected {expected}, got {x}"
            );
        }
    }

    /// The RON round trip is bit exact, which registering a float component
    /// requires.
    ///
    /// ADR-0014's consequence in as many words: `==` calls `0.0` and `-0.0`
    /// equal and two NaNs unequal, so only `to_bits` sees what a hash sees.
    /// `Transform`, `Layer`, `Camera` and `Sampling` each carry this test; a
    /// float component without one can lose a bit in the dump and every hash
    /// will still report agreement.
    #[test]
    fn the_ron_round_trip_is_bit_exact_on_values_that_stress_it() {
        let mut world = World::new();
        let target = world.spawn();
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
            for lost in [false, true] {
                let original = Follow {
                    target,
                    smoothing: value,
                    x: -value,
                    y: value,
                    lost,
                };
                let text = ron::to_string(&original).expect("a follow serializes");
                let returned: Follow = ron::from_str(&text)
                    .unwrap_or_else(|error| panic!("{text} does not parse back: {error}"));

                println!("{:#010x} lost={lost} -> {text}", value.to_bits());
                for (field, before, after) in [
                    ("smoothing", original.smoothing, returned.smoothing),
                    ("x", original.x, returned.x),
                    ("y", original.y, returned.y),
                ] {
                    assert_eq!(
                        before.to_bits(),
                        after.to_bits(),
                        "{field} did not survive the round trip bit for bit: {:#010x} \
                         became {:#010x}. A follow that loses bits here loses them in \
                         the canonical dump too, and a replay built on it points the \
                         camera somewhere else while every hash reports agreement.",
                        before.to_bits(),
                        after.to_bits()
                    );
                }
                assert_eq!(original.target, returned.target);
                assert_eq!(original.lost, returned.lost);
            }
        }
    }

    /// One tick is one application, which is what catch-up needs.
    ///
    /// ADR-0003's catch-up runs a system more than once in a frame, so the
    /// property that matters is that *n* applications compose: the state after
    /// *n* ticks must be the closed form `t·(1 − (1 − s)ⁿ)` from a start of
    /// zero, with nothing accumulated per frame and no clock read.
    ///
    /// **It is written against the closed form on purpose.** The obvious
    /// version — tick one world three times in a loop, tick another world three
    /// times in another loop, compare — is two identical pieces of code
    /// compared to each other, and passes however wrong the arithmetic is.
    #[test]
    fn each_tick_applies_the_smoothing_exactly_once() {
        let (mut world, eye, _target) = world_with_follow(0.5, (8.0, 0.0));

        // (1 - 0.5)^n for n = 1..=5, all exact in f32.
        for (index, remaining) in [0.5_f32, 0.25, 0.125, 0.0625, 0.03125]
            .into_iter()
            .enumerate()
        {
            let n = index as u64 + 1;
            tick(&mut world, n);
            let expected = 8.0_f32 * (1.0 - remaining);
            let (x, _) = camera_of(&world, eye);
            assert_eq!(
                x.to_bits(),
                expected.to_bits(),
                "after {n} ticks the camera must be at the closed form {expected}, \
                 not {x}: anything else means a tick applied the smoothing more \
                 or less than once"
            );
        }
    }

    /// Running the system through a `Scheduler` is running it directly.
    ///
    /// The catch-up path drives systems through the scheduler rather than by
    /// hand, so this pins the two together on bit patterns. Unlike a loop
    /// compared with an identical loop, the two sides here are different code.
    #[test]
    fn the_scheduler_path_and_the_direct_call_agree() {
        let mut scheduled = Scheduler::new();
        scheduled
            .add_system("follow", compose_camera)
            .expect("a fresh scheduler accepts one system");

        let (mut through, eye_a, _) = world_with_follow(0.25, (10.0, -6.0));
        let (mut direct, eye_b, _) = world_with_follow(0.25, (10.0, -6.0));

        for n in 1..=3 {
            scheduled.run(&mut through, &SystemContext::new(n));
            tick(&mut direct, n);
        }

        let (ax, ay) = camera_of(&through, eye_a);
        let (bx, by) = camera_of(&direct, eye_b);
        assert_eq!(ax.to_bits(), bx.to_bits());
        assert_eq!(ay.to_bits(), by.to_bits());
    }

    /// Two runs of the same world produce the same trail and the same hash.
    ///
    /// No value is stored: the two runs are compared against each other
    /// (ADR-0008), so a dependency bump moves both sides together.
    #[test]
    fn two_runs_of_the_same_world_follow_identically() {
        let registry = registry_with_follow();
        let trail = |world: &mut World, eye: EntityId| {
            (1..=8)
                .map(|n| {
                    tick(world, n);
                    let (x, y) = camera_of(world, eye);
                    (x.to_bits(), y.to_bits())
                })
                .collect::<Vec<_>>()
        };

        let (mut left, eye_l, _) = world_with_follow(0.3, (17.0, 4.5));
        let (mut right, eye_r, _) = world_with_follow(0.3, (17.0, 4.5));
        assert_eq!(trail(&mut left, eye_l), trail(&mut right, eye_r));

        let dump_l = canonical_dump(&left, &registry).expect("every component is registered");
        let dump_r = canonical_dump(&right, &registry).expect("every component is registered");
        assert_eq!(dump_l, dump_r);
        assert_eq!(state_hash(&dump_l), state_hash(&dump_r));
    }

    /// A changed `Follow` reaches the dump, so the hash notices it.
    #[test]
    fn changing_a_carried_follow_changes_the_hash() {
        let registry = registry_with_follow();
        let (mut world, _eye, _target) = world_with_follow(0.5, (8.0, 0.0));
        let before = canonical_dump(&world, &registry).expect("every component is registered");

        tick(&mut world, 1);
        let after = canonical_dump(&world, &registry).expect("every component is registered");

        assert_ne!(
            before, after,
            "a follow that moved must reach the dump, or a replay could put the \
             camera somewhere else and the state hash would not notice"
        );
        assert_ne!(state_hash(&before), state_hash(&after));
    }

    /// Registering `Follow` changes nothing for a world that carries none.
    ///
    /// Two registries compared, never a stored hash (ADR-0008). Its converse is
    /// the test above; without that one this could be satisfied by a component
    /// the dump ignores entirely.
    #[test]
    fn registering_the_follow_type_changes_nothing_for_a_world_that_has_none() {
        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts this component");
        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts this component");
        with.register_component::<Follow>("follow")
            .expect("a fresh registry accepts this component");

        let mut world = World::new();
        for i in 0..3_u8 {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        x: f32::from(i),
                        ..Transform::IDENTITY
                    },
                )
                .expect("the entity was just spawned");
        }

        let plain = canonical_dump(&world, &without).expect("every component is registered");
        let knowing = canonical_dump(&world, &with).expect("every component is registered");
        assert_eq!(plain, knowing);
        assert_eq!(state_hash(&plain), state_hash(&knowing));
    }

    /// A despawned target stops the camera and says so in the world.
    ///
    /// Not a panic and not silence: the camera holds its last value to the bit
    /// and [`Follow::lost`] records why, which puts the reason in the dump.
    #[test]
    fn losing_the_target_holds_the_camera_and_records_it() {
        let (mut world, eye, target) = world_with_follow(0.5, (8.0, 0.0));
        tick(&mut world, 1);
        let held = camera_of(&world, eye);
        assert_eq!(held.0.to_bits(), 4.0_f32.to_bits());

        world.despawn(target).expect("the target is alive");
        for n in 2..=5 {
            tick(&mut world, n);
        }

        let after = camera_of(&world, eye);
        assert_eq!(after.0.to_bits(), held.0.to_bits());
        assert_eq!(after.1.to_bits(), held.1.to_bits());
        assert!(
            world
                .get::<Follow>(eye)
                .expect("the eye carries a follow")
                .lost,
            "a follow whose target is gone must say so; a camera that quietly \
             stops is the failure this flag exists to prevent"
        );
    }

    /// A reused slot does not retarget the camera.
    ///
    /// `ProjektPlan.md` §12 names this hazard, and the facade closes it: an
    /// [`EntityId`] is a slot *and* a generation, so a handle kept across a
    /// despawn is permanently invalid rather than newly ambiguous. This
    /// constructs the case instead of trusting the paragraph — and it asserts
    /// the reuse itself, so that if hecs ever stops recycling slots the test
    /// reports the case as no longer constructible rather than passing
    /// vacuously.
    #[test]
    fn slot_reuse_invalidates_the_handle_rather_than_retargeting_it() {
        let (mut world, eye, target) = world_with_follow(1.0, (8.0, 0.0));
        tick(&mut world, 1);
        let held = camera_of(&world, eye);

        world.despawn(target).expect("the target is alive");
        let squatter = world.spawn();
        assert_eq!(
            squatter.index(),
            target.index(),
            "this test needs the slot to be recycled to mean anything; if hecs \
             stopped recycling, the hazard is untested rather than absent"
        );
        assert_eq!(squatter.generation(), target.generation() + 1);
        world
            .insert(
                squatter,
                Transform {
                    x: -900.0,
                    y: 900.0,
                    ..Transform::IDENTITY
                },
            )
            .expect("the entity was just spawned");

        tick(&mut world, 2);

        let after = camera_of(&world, eye);
        assert_eq!(
            after.0.to_bits(),
            held.0.to_bits(),
            "the camera followed the entity that took the dead target's slot"
        );
        assert_eq!(after.1.to_bits(), held.1.to_bits());
    }

    /// A follow without a camera on the same entity is left alone.
    #[test]
    fn a_follow_without_a_camera_is_not_a_camera() {
        let mut world = World::new();
        let target = world.spawn();
        world
            .insert(target, Transform::IDENTITY)
            .expect("the entity was just spawned");
        let orphan = world.spawn();
        world
            .insert(orphan, Follow::immediate(target, 5.0, 5.0))
            .expect("the entity was just spawned");

        tick(&mut world, 1);

        let follow = *world.get::<Follow>(orphan).expect("it carries a follow");
        assert_eq!(follow.x.to_bits(), 5.0_f32.to_bits());
        assert!(!follow.lost);
    }

    // ---- M3.31: the composition point ----------------------------------

    /// A camera that only shakes, around a captured base.
    fn world_with_shake(shake: Shake) -> (World, EntityId) {
        let mut world = World::new();
        let eye = world.spawn();
        world
            .insert(eye, Camera::at(shake.base_x, shake.base_y))
            .expect("the entity was just spawned");
        world
            .insert(eye, shake)
            .expect("the entity was just spawned");
        (world, eye)
    }

    /// **The composition guard.** The camera is exactly base plus offset, always.
    ///
    /// This is the property M3.31 decided, and §7 wants a guard rather than a
    /// comment for it. It recomputes the camera from the components the tick
    /// left behind and compares bit for bit, over four of the five rows in
    /// [`compose_camera`]'s combination table and over the whole life of a shake.
    ///
    /// **What it catches, precisely:** a second writer, a dropped offset, a
    /// flipped order. Both are demonstrated red in the report — `camera.x += 1.0`
    /// after the composition, and composing from the shake's pre-`advance` state.
    ///
    /// **What it cannot catch, because it re-implements the rule:** a wrong
    /// *base selection*. Its `match` is the implementation's `match`, so a
    /// changed precedence would change both sides together. The rows are covered
    /// instead by tests that name their expected value outright —
    /// `a_shake_runs_its_trail_and_then_rests`,
    /// `a_live_follow_ignores_the_shakes_own_base` and
    /// `a_lost_follow_holds_the_camera_through_a_shake`.
    #[test]
    fn the_camera_is_exactly_the_base_plus_the_offset() {
        let target_at = (17.0, -5.0);

        // Follow alone, follow with a shake, and a shake alone.
        let mut cases: Vec<(World, EntityId)> = Vec::new();
        let (world, eye, _) = world_with_follow(0.5, target_at);
        cases.push((world, eye));
        let (mut world, eye, _) = world_with_follow(0.5, target_at);
        world
            .insert(eye, Shake::new(6.0, 1.0, 0.5, 0.25))
            .expect("the eye is alive");
        cases.push((world, eye));
        let mut shaking = Shake::new(6.0, 1.0, 0.5, 0.25);
        shaking.base_x = -3.0;
        shaking.base_y = 11.0;
        let (world, eye) = world_with_shake(shaking);
        cases.push((world, eye));

        for (case, (mut world, eye)) in cases.into_iter().enumerate() {
            for n in 1..=8 {
                tick(&mut world, n);

                let follow = world.get::<Follow>(eye).ok().map(|follow| *follow);
                let shake = world.get::<Shake>(eye).ok().map(|shake| *shake);
                let (base_x, base_y) = match (follow, shake) {
                    (Some(follow), _) if !follow.lost => (follow.x, follow.y),
                    (Some(_), _) => continue,
                    (None, Some(shake)) => (shake.base_x, shake.base_y),
                    (None, None) => continue,
                };
                let (offset_x, offset_y) = shake.map_or((0.0, 0.0), Shake::offset);
                let (x, y) = camera_of(&world, eye);

                assert_eq!(
                    x.to_bits(),
                    (base_x + offset_x).to_bits(),
                    "case {case}, tick {n}: the camera is not base + offset along x. \
                     Either something else wrote it, or the composition read the \
                     shake before advancing it."
                );
                assert_eq!(
                    y.to_bits(),
                    (base_y + offset_y).to_bits(),
                    "case {case}, tick {n}: the camera is not base + offset along y"
                );
            }
        }
    }

    /// The shake trail, derived before it ran, on a camera with no follow.
    ///
    /// Amplitude 8, decay 0.5, cutoff 0.5, frequency 1.0 — a quarter period per
    /// tick — so the offsets are (4, 0), (0, −2), (−1, 0) and then rest. Every
    /// value dyadic, hence `to_bits`.
    #[test]
    fn a_shake_runs_its_trail_and_then_rests() {
        let (mut world, eye) = world_with_shake(Shake::new(8.0, 1.0, 0.5, 0.5));

        for (index, expected) in [(4.0_f32, 0.0_f32), (0.0, -2.0), (-1.0, 0.0), (0.0, 0.0)]
            .into_iter()
            .enumerate()
        {
            let n = index as u64 + 1;
            tick(&mut world, n);
            let (x, y) = camera_of(&world, eye);
            assert_eq!(x.to_bits(), expected.0.to_bits(), "tick {n}, x");
            assert_eq!(y.to_bits(), expected.1.to_bits(), "tick {n}, y");
        }

        assert!(
            world
                .get::<Shake>(eye)
                .expect("the eye carries a shake")
                .at_rest(),
            "the amplitude reached the cutoff, so the shake must be over rather \
             than wobbling below it forever"
        );
    }

    /// An expired shake composes the same camera as no shake, and hashes
    /// differently.
    ///
    /// Both halves on purpose. The render must be identical — that is what "over"
    /// means — and the hashes must not be, because one world carries a component
    /// and ADR-0008's dump is a statement about what is *there*, not about what
    /// is visible.
    #[test]
    fn an_expired_shake_composes_the_same_camera_as_no_shake() {
        let registry = registry_with_follow();

        let (mut shaken, eye_s, _) = world_with_follow(0.5, (9.0, -3.0));
        shaken
            .insert(eye_s, Shake::new(1.0, 1.0, 0.5, 0.75))
            .expect("the eye is alive");
        let (mut plain, eye_p, _) = world_with_follow(0.5, (9.0, -3.0));

        // One tick ends the shake: 1.0 * 0.5 = 0.5, which is below the cutoff.
        for n in 1..=4 {
            tick(&mut shaken, n);
            tick(&mut plain, n);
        }
        assert!(
            world_shake(&shaken, eye_s).at_rest(),
            "the shake should be over by now, or this test proves nothing"
        );

        let (sx, sy) = camera_of(&shaken, eye_s);
        let (px, py) = camera_of(&plain, eye_p);
        assert_eq!(sx.to_bits(), px.to_bits());
        assert_eq!(sy.to_bits(), py.to_bits());

        let dump_s = canonical_dump(&shaken, &registry).expect("every component is registered");
        let dump_p = canonical_dump(&plain, &registry).expect("every component is registered");
        assert_ne!(
            state_hash(&dump_s),
            state_hash(&dump_p),
            "an expired shake is still a component, so the dumps must differ even \
             though the cameras agree"
        );
    }

    fn world_shake(world: &World, eye: EntityId) -> Shake {
        *world.get::<Shake>(eye).expect("the eye carries a shake")
    }

    /// Once the shake expires the camera rejoins the shake-free trail exactly.
    ///
    /// The property the composition was designed for: the shake is added to the
    /// follow's output and never to its input, so it leaves no trace in the
    /// tracking. Follow chases a moving target throughout, so a feedback path
    /// would show up as a permanent divergence rather than a transient one.
    #[test]
    fn the_camera_rejoins_the_shake_free_trail_once_the_shake_expires() {
        let (mut shaken, eye_s, target_s) = world_with_follow(0.5, (0.0, 0.0));
        shaken
            .insert(eye_s, Shake::new(4.0, 1.0, 0.5, 0.4))
            .expect("the eye is alive");
        let (mut plain, eye_p, target_p) = world_with_follow(0.5, (0.0, 0.0));

        let mut rejoined_from = None;
        for n in 1..=12 {
            for (world, target) in [(&mut shaken, target_s), (&mut plain, target_p)] {
                let mut transform = world.get_mut::<Transform>(target).expect("alive");
                transform.x += 1.0;
                transform.y -= 0.5;
                drop(transform);
                tick(world, n);
            }

            if world_shake(&shaken, eye_s).at_rest() {
                let (sx, sy) = camera_of(&shaken, eye_s);
                let (px, py) = camera_of(&plain, eye_p);
                assert_eq!(
                    sx.to_bits(),
                    px.to_bits(),
                    "tick {n}: the shake has expired, so the camera must be back on \
                     the shake-free trail bit for bit. A difference here means the \
                     shake reached the follow's input."
                );
                assert_eq!(sy.to_bits(), py.to_bits());
                rejoined_from.get_or_insert(n);
            }
        }

        assert!(
            rejoined_from.is_some_and(|n| n <= 6),
            "the shake should have expired well inside twelve ticks; it did not, \
             so this test never checked what it exists to check"
        );
    }

    /// Two runs of the same shaking world agree on the trail and on the hash.
    #[test]
    fn two_runs_of_a_shaking_world_agree() {
        let registry = registry_with_follow();
        let trail = |world: &mut World, eye: EntityId| {
            (1..=10)
                .map(|n| {
                    tick(world, n);
                    let (x, y) = camera_of(world, eye);
                    (x.to_bits(), y.to_bits())
                })
                .collect::<Vec<_>>()
        };

        let build = || {
            let (mut world, eye, _) = world_with_follow(0.3, (12.0, 7.0));
            world
                .insert(eye, Shake::new(5.0, 1.5, 0.7, 0.05))
                .expect("the eye is alive");
            (world, eye)
        };
        let (mut left, eye_l) = build();
        let (mut right, eye_r) = build();
        assert_eq!(trail(&mut left, eye_l), trail(&mut right, eye_r));

        let dump_l = canonical_dump(&left, &registry).expect("every component is registered");
        let dump_r = canonical_dump(&right, &registry).expect("every component is registered");
        assert_eq!(state_hash(&dump_l), state_hash(&dump_r));
    }

    /// A changed `Shake` reaches the dump; a world without one is unaffected by
    /// the type being known.
    #[test]
    fn the_hash_sees_a_shake_and_ignores_its_absence() {
        let registry = registry_with_follow();
        let (mut world, _eye) = world_with_shake(Shake::new(4.0, 1.0, 0.5, 0.1));
        let before = canonical_dump(&world, &registry).expect("every component is registered");
        tick(&mut world, 1);
        let after = canonical_dump(&world, &registry).expect("every component is registered");
        assert_ne!(state_hash(&before), state_hash(&after));

        let mut without = ComponentRegistry::new();
        without
            .register_component::<Camera>("camera")
            .expect("a fresh registry accepts this component");
        let mut with = ComponentRegistry::new();
        with.register_component::<Camera>("camera")
            .expect("a fresh registry accepts this component");
        with.register_component::<Shake>("shake")
            .expect("a fresh registry accepts this component");

        let mut bare = World::new();
        let eye = bare.spawn();
        bare.insert(eye, Camera::at(1.0, 2.0))
            .expect("the entity was just spawned");
        let plain = canonical_dump(&bare, &without).expect("every component is registered");
        let knowing = canonical_dump(&bare, &with).expect("every component is registered");
        assert_eq!(plain, knowing);
        assert_eq!(state_hash(&plain), state_hash(&knowing));
    }

    /// The scheduler path and the direct call agree, shake included.
    ///
    /// M3.30's lesson, applied: the two sides must be different code, or the
    /// test cannot fail. One goes through `Scheduler::run`, the other calls the
    /// system.
    #[test]
    fn the_scheduler_path_and_the_direct_call_agree_with_a_shake() {
        let mut scheduled = Scheduler::new();
        scheduled
            .add_system("compose", compose_camera)
            .expect("a fresh scheduler accepts one system");

        let build = || {
            let (mut world, eye, _) = world_with_follow(0.25, (10.0, -6.0));
            world
                .insert(eye, Shake::new(3.0, 1.0, 0.6, 0.05))
                .expect("the eye is alive");
            (world, eye)
        };
        let (mut through, eye_a) = build();
        let (mut direct, eye_b) = build();

        for n in 1..=6 {
            scheduled.run(&mut through, &SystemContext::new(n));
            tick(&mut direct, n);
        }

        let (ax, ay) = camera_of(&through, eye_a);
        let (bx, by) = camera_of(&direct, eye_b);
        assert_eq!(ax.to_bits(), bx.to_bits());
        assert_eq!(ay.to_bits(), by.to_bits());
    }

    /// A live follow supplies the base and the shake's own is ignored.
    ///
    /// The composition guard cannot say this — its base selection is the
    /// implementation's, so a changed precedence would move both sides together.
    /// This one names the answer outright: the shake's base is a deliberately
    /// absurd (1000, −1000), so if precedence ever flipped the camera would be a
    /// thousand units away rather than one ULP off.
    #[test]
    fn a_live_follow_ignores_the_shakes_own_base() {
        let (mut world, eye, _target) = world_with_follow(1.0, (6.0, -2.0));
        let mut shake = Shake::new(2.0, 1.0, 0.5, 0.4);
        shake.base_x = 1000.0;
        shake.base_y = -1000.0;
        world.insert(eye, shake).expect("the eye is alive");

        // Tick 1: follow snaps to (6, -2); shake amplitude 1.0, phase 1, so the
        // offset is (1.0, 0.0). Tick 2: amplitude 0.5 is above the 0.4 cutoff,
        // phase 2, offset (0.0, -0.5). Both from the follow's base, never (1000, -1000).
        for (index, expected) in [(7.0_f32, -2.0_f32), (6.0, -2.5)].into_iter().enumerate() {
            tick(&mut world, index as u64 + 1);
            let (x, y) = camera_of(&world, eye);
            assert_eq!(
                x.to_bits(),
                expected.0.to_bits(),
                "tick {}: expected {} but got {x}; a value near 1000 means the \
                 shake's base won over the follow's point",
                index + 1,
                expected.0
            );
            assert_eq!(y.to_bits(), expected.1.to_bits());
        }
    }

    /// `Shake::new` on a standalone camera pulls it to the origin. Pinned, not
    /// endorsed.
    ///
    /// The constructor leaves the base at `(0.0, 0.0)`, and the composition
    /// *assigns* rather than adds, so a camera at (50, 50) with a bare
    /// `Shake::new` and no follow lands near the origin on the first tick. It is
    /// documented on the constructor and recorded here so that it is a known
    /// sharp edge rather than a surprise; the report proposes the fix.
    #[test]
    fn a_bare_new_shake_pulls_a_standalone_camera_to_the_origin() {
        let mut world = World::new();
        let eye = world.spawn();
        world
            .insert(eye, Camera::at(50.0, 50.0))
            .expect("the entity was just spawned");
        world
            .insert(eye, Shake::new(2.0, 1.0, 0.5, 0.4))
            .expect("the entity was just spawned");

        tick(&mut world, 1);

        let (x, y) = camera_of(&world, eye);
        assert_eq!(
            (x.to_bits(), y.to_bits()),
            (1.0_f32.to_bits(), 0.0_f32.to_bits()),
            "the camera is composed from the shake's base of (0, 0) plus the \
             offset, not from where the camera happened to be. Use \
             `Shake::around` for a standalone shake."
        );
    }

    /// A lost follow holds the camera even while a shake is running.
    ///
    /// The combination table in [`compose_camera`]'s doc says the camera is
    /// untouched; without this, "untouched" would be a sentence rather than a
    /// property, and the shake would be the obvious thing to move it anyway.
    #[test]
    fn a_lost_follow_holds_the_camera_through_a_shake() {
        let (mut world, eye, target) = world_with_follow(0.5, (8.0, 0.0));
        world
            .insert(eye, Shake::new(4.0, 1.0, 0.5, 0.05))
            .expect("the eye is alive");
        tick(&mut world, 1);

        // Captured *before* the losing tick, so that the tick on which the
        // follow goes lost is itself asserted. Capturing after it would leave a
        // mutation that honours the latch one tick late alive.
        let held = camera_of(&world, eye);
        world.despawn(target).expect("the target is alive");

        for n in 2..=8 {
            tick(&mut world, n);
            let (x, y) = camera_of(&world, eye);
            assert_eq!(x.to_bits(), held.0.to_bits(), "tick {n}");
            assert_eq!(y.to_bits(), held.1.to_bits(), "tick {n}");
        }
    }
}
