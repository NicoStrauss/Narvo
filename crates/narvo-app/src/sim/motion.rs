//! The `motion` demo: integer movement and one accumulating `f32`.
//!
//! **Frozen.** This simulation is the regression evidence for everything built
//! on the state hash. Its dump at ten thousand ticks was recorded when the hash
//! landed in M2.2 and is compared against by hand at the start and end of every
//! task that touches the engine's state path. Adding a component, changing an
//! initial value, reordering the systems or renaming a stable name all move that
//! hash, and a moved hash cannot be told apart from a simulation that actually
//! broke.
//!
//! So: nothing is added here. Something new goes in [`super::chance`] or in a
//! third mode. ADR-0008 explains why the recorded value is never checked in as a
//! test, which is also why this comment is the only thing guarding it.

use narvo_ecs::{ComponentRegistry, EcsError, Scheduler, SystemContext, World};
use serde::{Deserialize, Serialize};

use super::{Position, Simulation, Velocity, movement};

/// How many entities the demo world holds.
const ENTITIES: i64 = 32;

/// How much [`Wobble::phase`] advances per tick.
///
/// Not representable in binary floating point, and that is the point: the value
/// accumulates rounding over thousands of ticks, so the cross-platform
/// comparison has something to disagree about. An integer-only simulation would
/// match across platforms by construction and prove nothing.
const WOBBLE_STEP: f32 = 0.017;

/// A sawtooth in `0.0..1.0`, the one floating-point quantity in the run.
#[derive(Debug, Serialize, Deserialize)]
struct Wobble {
    phase: f32,
}

/// The floating-point half: advance the phase, wrap it back into `0.0..1.0`.
///
/// Addition and subtraction of `f32` are exactly specified by IEEE-754 and Rust
/// does not contract them into fused operations, so two platforms that follow
/// the standard produce identical bits. Whether they actually do is the
/// question M2's kill criterion asks, and this is what it asks it with.
fn wobble(world: &mut World, _context: &SystemContext) {
    let mut wobbling = world.query::<&mut Wobble>();
    for (_id, wobble) in wobbling.iter() {
        wobble.phase += WOBBLE_STEP;
        if wobble.phase >= 1.0 {
            wobble.phase -= 1.0;
        }
    }
}

/// Builds the frozen simulation.
///
/// Takes no arguments on purpose: two runs are identical because there is
/// nothing that could make them differ, not because the caller passed the same
/// values twice.
///
/// # Errors
///
/// Anything the world or the registry can raise while being built.
pub fn build() -> Result<Simulation, EcsError> {
    Ok(Simulation {
        world: build_world()?,
        registry: build_registry()?,
        scheduler: build_scheduler()?,
    })
}

fn build_world() -> Result<World, EcsError> {
    let mut world = World::new();

    for index in 0..ENTITIES {
        let entity = world.spawn();

        world.insert(
            entity,
            Position {
                x: index,
                y: -index,
            },
        )?;
        world.insert(
            entity,
            Velocity {
                dx: index % 5 - 2,
                dy: index % 3 - 1,
            },
        )?;

        // Only two entities in three wobble, so the dump contains both entities
        // with three components and entities with two - which is what makes the
        // per-entity component ordering observable at all.
        if index % 3 != 0 {
            world.insert(
                entity,
                Wobble {
                    phase: index as f32 * 0.013,
                },
            )?;
        }
    }

    Ok(world)
}

/// Registers every component type this mode uses.
///
/// Every one of them: a type missing here is a type the state hash cannot see,
/// and the canonical dump refuses to serialize a world in that condition rather
/// than quietly leaving part of the state out of the comparison.
fn build_registry() -> Result<ComponentRegistry, EcsError> {
    let mut registry = ComponentRegistry::new();

    registry.register_component::<Position>("position")?;
    registry.register_component::<Velocity>("velocity")?;
    registry.register_component::<Wobble>("wobble")?;

    Ok(registry)
}

/// The run order. Registration order is execution order, strictly sequential.
fn build_scheduler() -> Result<Scheduler, EcsError> {
    let mut scheduler = Scheduler::new();

    scheduler.add_system("movement", movement)?;
    scheduler.add_system("wobble", wobble)?;

    Ok(scheduler)
}

#[cfg(test)]
mod tests {
    use super::{ENTITIES, build, build_registry, build_scheduler, build_world};
    use crate::sim::assert_same_state;
    use narvo_ecs::canonical_dump;

    #[test]
    fn the_demo_world_is_built_identically_every_time() {
        let first = build_world().expect("the demo world always builds");
        let second = build_world().expect("the demo world always builds");
        let registry = build_registry().expect("the demo registry always builds");

        assert_same_state(
            "the motion world built twice",
            &canonical_dump(&first, &registry).expect("everything is registered"),
            &canonical_dump(&second, &registry).expect("everything is registered"),
        );
    }

    #[test]
    fn every_component_the_demo_uses_is_registered() {
        let world = build_world().expect("the demo world always builds");
        let registry = build_registry().expect("the demo registry always builds");

        let dump = canonical_dump(&world, &registry)
            .expect("the demo registers every component type it inserts");

        assert!(dump.contains("position "));
        assert!(dump.contains("velocity "));
        assert!(dump.contains("wobble "));
    }

    #[test]
    fn the_demo_world_has_the_entities_it_claims() {
        let world = build_world().expect("the demo world always builds");

        assert_eq!(u64::from(world.len()), ENTITIES.unsigned_abs());
    }

    #[test]
    fn the_run_order_is_the_registration_order() {
        let scheduler = build_scheduler().expect("the demo scheduler always builds");

        assert_eq!(
            scheduler.system_names().collect::<Vec<_>>(),
            vec!["movement", "wobble"]
        );
    }

    #[test]
    fn this_mode_stayed_frozen() {
        // The shape of the frozen simulation, asserted directly. Not the state
        // hash - ADR-0008 forbids committing one - but the things that would
        // have to change for the hash to move: which components exist, how many
        // entities there are, and what runs in what order. A change here is a
        // deliberate act, and this test is where it announces itself.
        let simulation = build().expect("the demo always builds");

        assert_eq!(u64::from(simulation.world.len()), ENTITIES.unsigned_abs());
        assert_eq!(
            simulation
                .registry
                .iter()
                .map(narvo_ecs::ComponentInfo::name)
                .collect::<Vec<_>>(),
            vec!["position", "velocity", "wobble"]
        );
        assert_eq!(
            simulation.scheduler.system_names().collect::<Vec<_>>(),
            vec!["movement", "wobble"]
        );
    }
}
