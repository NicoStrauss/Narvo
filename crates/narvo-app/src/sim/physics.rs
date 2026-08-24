//! The demo simulation driven by the rigid-body solver.
//!
//! A ground and a stack of boxes that fall onto it and onto each other. It is
//! the first mode whose state is computed by `rapier` rather than by arithmetic
//! written here, and it exists for one purpose: to put physics into the
//! cross-platform comparison, on two foreign machines rather than on WSL beside
//! Windows. ADR-0013's amendment leaves that evidence explicitly open.
//!
//! # Two modules called `physics`, and which is which
//!
//! [`crate::physics`] is the seam — the mapping between `RigidBody` components
//! and `narvo_physics2d::Body`, built in M5b.3a. This module is the demo mode
//! that *uses* it. The pair is exactly [`crate::input`] and [`crate::sim::input`]:
//! the engine-facing mechanism, and the demo that drives it. **The mode does not
//! wire the solver a second time**; every tick goes through
//! [`crate::physics::simulate`], so what the matrix compares is the same path a
//! consumer will use — and since M5b.4 that is literal rather than nearly so: the
//! system, the gravity and the tick length moved into the seam module, and
//! `sim::scene_file` schedules the same function.
//!
//! # The world is built in code
//!
//! Like [`motion`](super::motion) and [`chance`](super::chance), and unlike
//! [`scene_file`](super::scene_file). A scene file for physics is M5b.4's, and
//! building one here would put a second variable — the loader — inside the first
//! cross-platform physics measurement.
//!
//! # What the scene is not built for
//!
//! It is **not** built to come to rest, and a reader expecting a settled stack
//! should not read the residual motion as a defect. ADR-0029 rebuilds the rapier
//! world every tick, which loses warm-starting, and M5b.2 measured the price: at
//! tick 10 000 a rebuilt world carries about a hundred times the residual motion
//! of a retained one. It jitters where a retained world rests. For a comparison
//! that is irrelevant as long as both sides jitter identically — which is
//! precisely what this mode is here to find out.
//!
//! # This scene damps a perturbation **that arrives before the first contact**,
//! and that is measured
//!
//! M5b.3b shifted one body's starting x by a single ULP and recorded the whole
//! matrix again. The difference shows at **`t1` and nowhere else**: by `t100`
//! all thirteen bodies are byte-identical to the unshifted run again, and they
//! stay so through `t10000`. The bodies land, the contact solver drives each one
//! to the same resting pose, and a one-bit difference in where it started is
//! resolved away rather than compounded.
//!
//! **The qualification in that heading is M5b.3c's and is not a hedge.** The
//! *same* one-ULP shift applied at tick 50 — after the first contact instead of
//! before it — is not absorbed at all: M5b.3c measured it reaching 12 of 13
//! bodies by `t100` and every checkpoint after. What separates the two is the
//! phase the perturbation lands in, not the sharpness of the scene, so the
//! sentence this heading used to carry — "damps a perturbation rather than
//! amplifying one" — was true of the case it was measured on and too broad as a
//! statement about the scene. `a_one_ulp_perturbation_after_the_first_contact_spreads`
//! below pins the half that is cheap to pin; the mechanism behind the pair stays
//! a hypothesis, which M5b.3c's report says and nothing here improves on.
//!
//! **That is not what M5b.1 predicted.** Its scene — 32 bodies, more mutual
//! contact, less settling — had 30 of 32 bodies differing within 100 ticks. The
//! figure did not carry over, and the scene here was deliberately **not** re-cut
//! to reproduce it: tuning a scene until a perturbation propagates is tuning the
//! experiment to its expected answer.
//!
//! What it means for the instrument, stated rather than glossed: `t1` catches a
//! one-ULP divergence and demonstrably does. The later checkpoints are not
//! useless — the state still differs between every neighbouring pair out to
//! `t10000`, so each carries its own comparison — but they are **weaker evidence
//! about a small divergence than the M5b.1 figure suggests**, because this scene
//! has an attractor and that one did not.

use narvo_ecs::{ComponentRegistry, EcsError, RigidBody, Scheduler, World};

use super::Simulation;

/// Dynamic bodies in the scene, beside the one fixed ground.
///
/// Chosen by measurement rather than by taste; the figures are in
/// `target/reports/M5b.3b.md`. The constraint is `ProjektPlan.md` §8: the other
/// demo modes run 10 000 ticks in 7–15 ms, and a physics mode cannot be brought
/// anywhere near that — the rebuild-per-tick of ADR-0029 is far more work. At
/// this count 10 000 ticks cost about 270 ms of simulation, some twenty to
/// thirty times the others, and the five matrix cases together add roughly a
/// second to each recording job. The count is chosen so the mode still
/// *resolves contacts* and still finishes inside a CI job, not so it matches the
/// others.
const BODIES: usize = 12;

/// Half-extents of the ground, wide enough that nothing walks off it.
const GROUND_HALF_X: f32 = 20.0;
const GROUND_HALF_Y: f32 = 0.5;

/// Builds the simulation.
///
/// # Errors
///
/// Anything the world, the registry or the scheduler can raise while being
/// built, all of which would be programming mistakes in the lines below.
pub fn build() -> Result<Simulation, EcsError> {
    Ok(Simulation {
        world: build_world()?,
        registry: build_registry()?,
        scheduler: build_scheduler()?,
    })
}

/// The ground and the falling stack.
///
/// **The start values are deliberately not exactly representable in binary.**
/// That is ADR-0013's reasoning about `motion`'s `0.017`, applied here: a scene
/// of halves and quarters would agree across platforms by construction and would
/// demonstrate nothing about the solver. Every body also starts with a velocity
/// and a spin, so the comparison covers all seven scalars a step writes rather
/// than only the two a falling box changes.
fn build_world() -> Result<World, EcsError> {
    let mut world = World::new();

    let ground = world.spawn();
    world.insert(
        ground,
        RigidBody::fixed(GROUND_HALF_X, GROUND_HALF_Y)
            .at(0.0, -GROUND_HALF_Y)
            .with_material(0.0, 0.73, 1.0),
    )?;

    for index in 0..BODIES {
        #[expect(
            clippy::cast_precision_loss,
            reason = "BODIES is a small constant, so the index is exact in f32"
        )]
        let offset = index as f32;

        let entity = world.spawn();
        world.insert(
            entity,
            RigidBody::dynamic(0.41, 0.29)
                .at(-1.9 + offset * 0.37, 0.9 + offset * 1.1)
                .with_velocity((offset - 2.0) * 0.31, -0.17)
                .with_angvel((offset - 3.0) * 0.19)
                .with_material(0.42, 0.73, 1.3),
        )?;
    }

    Ok(world)
}

/// Registers every component type this mode uses.
///
/// One type, and it is the whole of the mode's state: ADR-0029's rule is that
/// nothing physical lives outside the registry, so a body that is not here is a
/// body the hash cannot see. Registering the engine set instead would put ten
/// unused names in the registry and change nothing about the dump.
fn build_registry() -> Result<ComponentRegistry, EcsError> {
    let mut registry = ComponentRegistry::new();

    registry.register_component::<RigidBody>("rigidbody")?;

    Ok(registry)
}

/// The run order. One system, and registration order is execution order.
fn build_scheduler() -> Result<Scheduler, EcsError> {
    let mut scheduler = Scheduler::new();

    scheduler.add_system("physics", crate::physics::simulate)?;

    Ok(scheduler)
}

#[cfg(test)]
mod tests {
    use super::{BODIES, build, build_world};
    use crate::sim::assert_same_state;
    use narvo_ecs::{RigidBody, SystemContext, canonical_dump};

    /// Runs the mode for `ticks` and returns the simulation.
    fn run_to(ticks: u64) -> super::Simulation {
        let mut simulation = build().expect("the mode always builds");
        for tick in 0..ticks {
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
        }
        simulation
    }

    fn dump_at(ticks: u64) -> String {
        let simulation = run_to(ticks);
        canonical_dump(&simulation.world, &simulation.registry).expect("everything is registered")
    }

    #[test]
    fn the_world_has_a_ground_and_the_stack() {
        let world = build_world().expect("builds");
        let ids = world.entity_ids();

        assert_eq!(ids.len(), BODIES + 1, "one ground plus {BODIES} bodies");

        let kinds: Vec<u8> = ids
            .iter()
            .map(|id| world.get::<RigidBody>(*id).expect("carries a body").kind)
            .collect();
        assert_eq!(kinds[0], RigidBody::FIXED, "the ground is fixed");
        assert!(
            kinds[1..].iter().all(|kind| *kind == RigidBody::DYNAMIC),
            "everything above the ground falls"
        );
    }

    #[test]
    fn two_runs_produce_the_same_state() {
        assert_same_state(
            "the physics mode, twice to tick 100",
            &dump_at(100),
            &dump_at(100),
        );
    }

    #[test]
    fn the_run_changes_something() {
        // Without this the test above is satisfied by a world that never moves.
        assert_ne!(dump_at(0), dump_at(100));
    }

    /// The mode resolves contacts inside the range the matrix checks.
    ///
    /// **This is the property M5b.3 named as the reason a physics case is worth
    /// having.** M5b.2 measured that a rebuilt and a retained world are identical
    /// through free fall and part company at the first tick that resolves a
    /// contact, so a mode whose bodies were still falling at every checkpoint
    /// would be comparing the easy half. The threshold is the matrix's first
    /// checkpoint above `t1`.
    #[test]
    fn contacts_are_resolved_before_the_first_hundred_ticks() {
        let simulation = run_to(100);

        let landed = simulation
            .world
            .entity_ids()
            .into_iter()
            .filter_map(|id| simulation.world.get::<RigidBody>(id).ok())
            .filter(|body| body.kind == RigidBody::DYNAMIC)
            .filter(|body| body.y < 1.0)
            .count();

        assert!(
            landed >= 1,
            "no body has reached the ground by tick 100, so every matrix \
             checkpoint would be measuring free fall - which is the region \
             M5b.2 found a rebuilt and a retained world agree in"
        );
    }

    /// Ticks the perturbation pair is measured over, and where the bit goes in.
    ///
    /// M5b.3c's own numbers: the perturbation is one ULP added to the bit pattern
    /// of the first dynamic body's `x`, applied once, and the comparison is at
    /// `t100`. `BEFORE_CONTACT` is the starting state, `AFTER_CONTACT` is tick 50
    /// — the scene's first contact is between tick 20 and 25 (M5b.3b), so one is
    /// on each side of it and the phase is the only variable.
    const PERTURBATION_TICKS: u64 = 100;
    const BEFORE_CONTACT: u64 = 0;
    const AFTER_CONTACT: u64 = 50;

    /// The mode's dump after `ticks`, with one ULP added to body 0's `x` before
    /// tick `inject` runs.
    ///
    /// The bit goes in between two ticks, which is where M5b.3c put it: at
    /// `inject = 0` that is the starting state the world was built with, and at
    /// `inject = 50` it is the state fifty ticks of solving left behind. Entity
    /// `[0]` is the ground and is skipped; `[1]` is the first falling box.
    fn dump_perturbed_at(inject: u64, ticks: u64) -> String {
        let mut simulation = build().expect("the mode always builds");

        for tick in 0..ticks {
            if tick == inject {
                let target = simulation.world.entity_ids()[1];
                let mut body = simulation
                    .world
                    .get_mut::<RigidBody>(target)
                    .expect("the first box carries a body");
                body.x = f32::from_bits(body.x.to_bits() + 1);
            }

            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
        }

        canonical_dump(&simulation.world, &simulation.registry).expect("everything is registered")
    }

    /// **A one-ULP difference introduced after the first contact spreads.**
    ///
    /// The half of M5b.3c's I1/I3 pair that is cheap to pin and expensive to
    /// rediscover, and the reason the pair matters at all: the *same*
    /// perturbation on the *same* field of the *same* body is absorbed when it
    /// arrives before the first contact and is not absorbed when it arrives
    /// after. Phase is the only variable between this test and its sibling below.
    ///
    /// What it is evidence for: that the cross-platform matrix's later
    /// checkpoints are not decoration. `physics.t100` and everything after it can
    /// carry a difference of the smallest size the format has — provided the
    /// difference is in the phase this test puts it in. What it is **not**
    /// evidence for is the smallest difference the matrix could catch: M5b.3c
    /// brackets the instrument from one side only, and nothing here narrows that.
    #[test]
    fn a_one_ulp_perturbation_after_the_first_contact_spreads() {
        let undisturbed = dump_at(PERTURBATION_TICKS);
        let perturbed = dump_perturbed_at(AFTER_CONTACT, PERTURBATION_TICKS);

        assert_ne!(
            undisturbed, perturbed,
            "one ULP added to a body's x at tick {AFTER_CONTACT} - after this \
             scene's first contact - left the state at tick {PERTURBATION_TICKS} \
             byte for byte what it was. M5b.3c measured that difference reaching \
             12 of 13 bodies, and it is what says a later checkpoint of the \
             cross-platform matrix can see anything at all."
        );
    }

    /// The same perturbation before the first contact is absorbed instead.
    ///
    /// **This is what makes the test above an assertion about phase rather than
    /// about arithmetic**, and it is the negative control M5b.3b's own module
    /// documentation describes: the contact solver drives each body to the same
    /// resting pose while it is still settling, so a difference present during
    /// that transient is resolved away.
    ///
    /// It also fixes the flank of its sibling without an injection: move that
    /// test's bit from [`AFTER_CONTACT`] to [`BEFORE_CONTACT`] and its `assert_ne`
    /// is asked about the two dumps this one asserts are equal.
    #[test]
    fn the_same_perturbation_before_the_first_contact_is_absorbed() {
        let undisturbed = dump_at(PERTURBATION_TICKS);
        let perturbed = dump_perturbed_at(BEFORE_CONTACT, PERTURBATION_TICKS);

        if let Some(difference) = narvo_ecs::first_difference(&undisturbed, &perturbed) {
            panic!(
                "one ULP added to a body's x before tick {BEFORE_CONTACT} - before \
                 this scene's first contact - was still visible at tick \
                 {PERTURBATION_TICKS}. M5b.3b measured it gone by then, and the \
                 module documentation above rests on that.\n{difference}"
            );
        }
    }

    /// The bodies turn, so the rotation pair is compared rather than assumed.
    #[test]
    fn the_stack_rotates_so_the_pair_is_covered() {
        let simulation = run_to(100);

        let turned = simulation
            .world
            .entity_ids()
            .into_iter()
            .filter_map(|id| simulation.world.get::<RigidBody>(id).ok())
            .filter(|body| body.kind == RigidBody::DYNAMIC)
            .filter(|body| body.rot_sin.to_bits() != 0.0f32.to_bits())
            .count();

        assert!(
            turned >= 3,
            "only {turned} bodies have turned by tick 100, so `rot_cos` and \
             `rot_sin` are in the dump without being moved"
        );
    }
}
