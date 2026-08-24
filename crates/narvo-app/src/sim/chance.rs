//! The `chance` demo: seeded randomness and events, driving everything.
//!
//! One entity — the director — holds the generator, the event buffer and the
//! counters. Every tick it draws an impulse and sends it; the tick after that,
//! the impulse is delivered and lands on one entity's velocity. Everything else
//! starts at rest.
//!
//! That last part is the design: the movers begin with zero velocity, so *all*
//! motion in this simulation is caused by a drawn number travelling through an
//! event. If the generator stopped varying, or delivery stopped happening, the
//! world would sit perfectly still — and a test that asserts the state changed
//! is then a test of both paths rather than an observation about arithmetic.
//!
//! Integer arithmetic throughout, deliberately. The floating-point question is
//! already asked by [`super::motion`], and asking it twice would mean a
//! cross-platform divergence here could be either the generator, the event path
//! or `f32` rounding, with no way to tell which from the hash alone.

use narvo_ecs::{
    ComponentRegistry, EcsError, EntityId, Events, Rng, Scheduler, SystemContext, World,
    rotate_events,
};
use serde::{Deserialize, Serialize};

use super::{Position, Simulation, Velocity, movement};

/// How many entities can be pushed around.
const MOVERS: u32 = 32;

/// A nudge to one entity's velocity, sent by the director and applied a tick
/// later.
///
/// It names its target by slot index rather than by [`EntityId`]: an event is
/// delivered a tick after it was sent, and in a simulation that despawned
/// entities a handle could be stale by then. Nothing in this demo despawns, so
/// the distinction does not bite here — it is written this way because the shape
/// of the message is what a later milestone inherits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Impulse {
    /// Slot index of the entity to push.
    target: u32,
    /// Added to the target's `dx`.
    ddx: i64,
    /// Added to the target's `dy`.
    ddy: i64,
}

/// The director's counters, and the reason the event round trip is observable.
///
/// Both numbers are in the state dump, so "every impulse that was sent was
/// received exactly once" is a property a reader can check on a dump without
/// running anything.
#[derive(Debug, Serialize, Deserialize)]
struct Ledger {
    /// Impulses handed to the buffer since the run began.
    impulses_sent: i64,
    /// Impulses taken back out of it and applied.
    impulses_applied: i64,
}

/// The entity holding the generator, the buffer and the counters.
///
/// Found by walking [`World::entity_ids`], which is sorted, rather than by
/// querying, which is archetype-ordered and not reproducible. There is exactly
/// one director, so the two would agree today — the sorted walk is used anyway,
/// because "it happens to be unique" is not a property this engine wants
/// anything to rest on.
fn director(world: &World) -> EntityId {
    world
        .entity_ids()
        .into_iter()
        .find(|entity| world.has::<Ledger>(*entity))
        .expect("the chance world is built with exactly one director and nothing despawns it")
}

/// Draws one impulse and sends it. Runs last, so what it sends is delivered in
/// the next tick and never in this one.
fn emit_impulses(world: &mut World, _context: &SystemContext) {
    let director = director(world);

    let impulse = {
        let mut rng = world
            .get_mut::<Rng>(director)
            .expect("the director carries the generator");

        // The draw order is part of the simulation. Three draws per tick, always
        // in this order: reordering these three lines changes every state from
        // that tick onwards, exactly as changing the seed would.
        let target = rng.next_u32_below(MOVERS);
        let ddx = i64::from(rng.next_u32_below(5)) - 2;
        let ddy = i64::from(rng.next_u32_below(3)) - 1;

        Impulse { target, ddx, ddy }
    };

    world
        .get_mut::<Events<Impulse>>(director)
        .expect("the director carries the impulse buffer")
        .send(impulse);

    world
        .get_mut::<Ledger>(director)
        .expect("the director carries the ledger")
        .impulses_sent += 1;
}

/// Applies everything the buffer made readable this tick, and counts it.
fn apply_impulses(world: &mut World, _context: &SystemContext) {
    let director = director(world);

    // Copied out before anything is written: the buffer is borrowed from the
    // world, and applying an impulse needs a second, mutable borrow of a
    // different entity. Same shape as the reaper in the determinism test.
    let impulses: Vec<Impulse> = world
        .get::<Events<Impulse>>(director)
        .expect("the director carries the impulse buffer")
        .iter()
        .copied()
        .collect();

    if impulses.is_empty() {
        return;
    }

    // Sorted, so resolving a target does not depend on archetype layout.
    let entities = world.entity_ids();
    let mut applied = 0_i64;

    for impulse in impulses {
        let target = entities
            .iter()
            .copied()
            .find(|entity| entity.index() == impulse.target);

        // An impulse aimed at a slot that holds nothing is dropped rather than
        // being an error. Nothing in this demo despawns, so it cannot happen
        // here; a later simulation that does despawn gets a defined answer
        // instead of a panic on the first stale target.
        let Some(target) = target else { continue };
        let Ok(mut velocity) = world.get_mut::<Velocity>(target) else {
            continue;
        };

        velocity.dx += impulse.ddx;
        velocity.dy += impulse.ddy;
        applied += 1;
    }

    world
        .get_mut::<Ledger>(director)
        .expect("the director carries the ledger")
        .impulses_applied += applied;
}

/// Builds the simulation for `seed`.
///
/// # Errors
///
/// Anything the world or the registry can raise while being built.
pub fn build(seed: u64) -> Result<Simulation, EcsError> {
    Ok(Simulation {
        world: build_world(seed)?,
        registry: build_registry()?,
        scheduler: build_scheduler()?,
    })
}

/// Builds the starting world: the movers first, then the director.
///
/// The order matters. Movers occupy slots `0..MOVERS`, so an impulse drawn in
/// `0..MOVERS` names one of them directly and can never address the director,
/// which has no velocity to push.
fn build_world(seed: u64) -> Result<World, EcsError> {
    let mut world = World::new();

    for index in 0..i64::from(MOVERS) {
        let entity = world.spawn();

        world.insert(
            entity,
            Position {
                x: index,
                y: -index,
            },
        )?;
        // At rest. Everything that ever moves does so because an impulse arrived.
        world.insert(entity, Velocity { dx: 0, dy: 0 })?;
    }

    let director = world.spawn();
    world.insert(director, Rng::new(seed))?;
    world.insert(director, Events::<Impulse>::new())?;
    world.insert(
        director,
        Ledger {
            impulses_sent: 0,
            impulses_applied: 0,
        },
    )?;

    Ok(world)
}

/// Registers every component type this mode uses — including the generator and
/// the event buffer.
///
/// Those two are the point of the milestone. A generator outside the registry is
/// a generator whose state two runs can disagree about while every hash reports
/// agreement, and an unregistered buffer hides a pending event the same way.
/// ADR-0010 and ADR-0011 record both as decisions.
fn build_registry() -> Result<ComponentRegistry, EcsError> {
    let mut registry = ComponentRegistry::new();

    registry.register_component::<Position>("position")?;
    registry.register_component::<Velocity>("velocity")?;
    registry.register_component::<Rng>("rng")?;
    registry.register_component::<Events<Impulse>>("impulses")?;
    registry.register_component::<Ledger>("ledger")?;

    Ok(registry)
}

/// The run order. Registration order is execution order, strictly sequential.
///
/// Rotation runs first so that what was sent last tick is readable before
/// anything looks, and emission runs last so that what it sends belongs to the
/// next tick. Everything in between sees one fixed set of events, whatever its
/// own position in this list.
fn build_scheduler() -> Result<Scheduler, EcsError> {
    let mut scheduler = Scheduler::new();

    scheduler.add_system("impulses/rotate", rotate_events::<Impulse>)?;
    scheduler.add_system("impulses/apply", apply_impulses)?;
    scheduler.add_system("movement", movement)?;
    scheduler.add_system("impulses/emit", emit_impulses)?;

    Ok(scheduler)
}

#[cfg(test)]
mod tests {
    use super::{Ledger, MOVERS, build, build_registry, build_scheduler, build_world, director};
    use crate::sim::{Position, Velocity};
    use narvo_ecs::SystemContext;

    /// Drives a freshly built simulation for `ticks` ticks.
    fn run(seed: u64, ticks: u64) -> crate::sim::Simulation {
        let mut simulation = build(seed).expect("the demo always builds");
        for tick in 0..ticks {
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
        }
        simulation
    }

    fn ledger(simulation: &crate::sim::Simulation) -> (i64, i64) {
        let director = director(&simulation.world);
        let ledger = simulation
            .world
            .get::<Ledger>(director)
            .expect("the director carries the ledger");
        (ledger.impulses_sent, ledger.impulses_applied)
    }

    #[test]
    fn every_impulse_that_was_sent_is_applied_exactly_once() {
        // The "received exactly once" requirement at simulation level. One
        // impulse is emitted per tick and one is in flight at the end of every
        // tick, so the two counters differ by exactly one - never more, which
        // would mean events were lost, and never less, which would mean one was
        // delivered twice.
        for ticks in [1_u64, 2, 3, 17, 100, 1_000] {
            let (sent, applied) = ledger(&run(7, ticks));

            assert_eq!(sent, ticks as i64, "one impulse per tick is emitted");
            assert_eq!(
                applied,
                sent - 1,
                "after {ticks} ticks exactly one impulse is still in flight"
            );
        }
    }

    #[test]
    fn nothing_is_delivered_in_the_tick_it_was_sent_in() {
        let (sent, applied) = ledger(&run(7, 1));

        assert_eq!((sent, applied), (1, 0));
    }

    #[test]
    fn the_world_starts_at_rest_and_only_events_can_move_it() {
        let start = build(7).expect("builds");
        let mut resting = start.world.query::<&Velocity>();
        assert!(
            resting
                .iter()
                .all(|(_id, velocity)| velocity.dx == 0 && velocity.dy == 0),
            "every mover has to start at rest, or the test below proves nothing"
        );
        drop(resting);

        // After enough ticks for impulses to arrive, something has moved - and
        // the only route from the generator to a position is through the buffer.
        let later = run(7, 200);
        let mut moved = later.world.query::<&Position>();
        assert!(
            moved
                .iter()
                .any(|(_id, position)| position.x != 0 || position.y != 0),
            "no impulse ever reached a position"
        );
    }

    #[test]
    fn the_run_order_is_the_registration_order() {
        let scheduler = build_scheduler().expect("the demo scheduler always builds");

        assert_eq!(
            scheduler.system_names().collect::<Vec<_>>(),
            vec![
                "impulses/rotate",
                "impulses/apply",
                "movement",
                "impulses/emit"
            ]
        );
    }

    #[test]
    fn active_events_do_not_change_the_run_order() {
        // The order is a property of the scheduler and of nothing else. Driving
        // the simulation until events are flowing must not reorder, drop or add
        // a system.
        let before = build_scheduler().expect("builds");
        let after = run(7, 500);

        assert_eq!(
            after.scheduler.system_names().collect::<Vec<_>>(),
            before.system_names().collect::<Vec<_>>()
        );
        assert_eq!(after.scheduler.len(), 4);
    }

    #[test]
    fn every_component_the_demo_uses_is_registered() {
        let world = build_world(7).expect("the demo world always builds");
        let registry = build_registry().expect("the demo registry always builds");

        let dump = narvo_ecs::canonical_dump(&world, &registry)
            .expect("the demo registers every component type it inserts");

        for name in ["position", "velocity", "rng", "impulses", "ledger"] {
            assert!(
                dump.contains(&format!("{name} ")),
                "{name} is missing from the dump"
            );
        }
    }

    #[test]
    fn the_generator_state_is_in_the_dump_and_advances_with_the_run() {
        // The requirement this milestone turns on: a generator whose state is
        // outside the dump makes a divergence in it invisible.
        let start = build(7).expect("builds");
        let start_dump =
            narvo_ecs::canonical_dump(&start.world, &start.registry).expect("registered");

        let later = run(7, 50);
        let later_dump =
            narvo_ecs::canonical_dump(&later.world, &later.registry).expect("registered");

        assert!(
            start_dump.contains("rng ("),
            "the generator is not in the dump"
        );
        assert!(
            later_dump.contains("rng ("),
            "the generator is not in the dump"
        );
        assert_ne!(
            start_dump, later_dump,
            "fifty ticks of drawing have to move the generator's state"
        );
    }

    #[test]
    fn the_event_buffer_is_in_the_dump() {
        let simulation = run(7, 10);
        let dump =
            narvo_ecs::canonical_dump(&simulation.world, &simulation.registry).expect("registered");

        assert!(
            dump.contains("impulses ("),
            "the event buffer is not in the dump: {dump}"
        );
    }

    #[test]
    fn the_world_holds_the_movers_it_claims_plus_one_director() {
        let world = build_world(7).expect("builds");

        assert_eq!(world.len(), MOVERS + 1);
    }
}
