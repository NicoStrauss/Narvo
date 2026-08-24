//! The `input` demo: a world that only moves because something outside it said
//! so.
//!
//! Nothing in this world draws a random number and nothing in it decides
//! anything. Every entity starts at rest, and the only thing that can change a
//! velocity is an [`InputEvent`] that arrived from outside the tick. That is the
//! property the recording is tested against: if a replay fed the wrong inputs,
//! or fed none, the world would sit still or move differently, and the state
//! hash would say so immediately.
//!
//! The input itself comes from [`pilot`] during a recorded run — a stand-in for
//! a player, seeded and living in the runner rather than in the world. ADR-0012
//! explains why it has to live outside: an input source is not part of the
//! simulation, it is what the simulation is reproducible *against*.

use narvo_ecs::{
    ComponentRegistry, EcsError, EntityId, Events, Rng, Scheduler, SystemContext, World,
    rotate_events,
};
use narvo_input::InputEvent;
use serde::{Deserialize, Serialize};

use super::{Position, Simulation, Velocity, movement};

/// How many entities can be steered.
const MOVERS: u32 = 32;

/// Every action name this mode understands, sorted.
///
/// A recording is checked against this list before a replay starts, so a file
/// naming an action this build does not have fails with the tick and the name
/// rather than being quietly ignored for ten thousand ticks.
pub const ACTIONS: [&str; 3] = ["select", "thrust", "turn"];

/// Which entity the inputs currently steer, and what has been done to it.
///
/// The counters are here so the input path is legible in a state dump: a replay
/// that delivered nothing shows `applied:0`, which is a far better diagnostic
/// than positions that merely look wrong.
#[derive(Debug, Serialize, Deserialize)]
struct Controls {
    /// Slot index of the entity `thrust` and `turn` apply to.
    selected: u32,
    /// Inputs that reached the state.
    applied: i64,
    /// Inputs whose action this mode does not know.
    ///
    /// Always zero in a validated run — a recording is checked before it is
    /// replayed. It is counted rather than ignored so that an unknown action
    /// reaching a system anyway is visible in the dump instead of silently
    /// doing nothing.
    ignored: i64,
}

/// The entity holding the input buffer and the controls.
///
/// Found through the sorted [`World::entity_ids`], for the reason
/// [`super::chance`] gives: query order is archetype order and is not
/// reproducible, and "there is only one of them" is not a property to lean on.
fn console(world: &World) -> EntityId {
    world
        .entity_ids()
        .into_iter()
        .find(|entity| world.has::<Controls>(*entity))
        .expect("the input world is built with exactly one console and nothing despawns it")
}

/// Applies everything the input buffer made readable this tick.
///
/// Events are applied in the order they were recorded, and the order matters: a
/// `select` followed by a `thrust` in the same tick steers the newly selected
/// entity. That is a deliberate property rather than an accident of the loop —
/// it is what makes the recording's line order observable in the state, and
/// therefore what makes a reordered recording produce a different hash.
fn apply_input(world: &mut World, _context: &SystemContext) {
    let console = console(world);

    // Copied out before anything is written: the buffer is borrowed from the
    // world, and steering an entity needs a second, mutable borrow.
    let events: Vec<InputEvent> = world
        .get::<Events<InputEvent>>(console)
        .expect("the console carries the input buffer")
        .iter()
        .cloned()
        .collect();

    if events.is_empty() {
        return;
    }

    let entities = world.entity_ids();
    let mut selected = world
        .get::<Controls>(console)
        .expect("the console carries the controls")
        .selected;
    let mut applied = 0_i64;
    let mut ignored = 0_i64;

    for event in events {
        let value = event.value();
        match event.action() {
            // `rem_euclid` rather than `%`: a negative value in a hand-edited
            // recording has to land somewhere defined, and the sign of `%` in
            // Rust would make it land outside the range.
            "select" => {
                selected = u32::try_from(value.rem_euclid(i64::from(MOVERS)))
                    .expect("rem_euclid by MOVERS is inside u32");
                applied += 1;
            }
            "thrust" => applied += i64::from(steer(world, &entities, selected, value, 0)),
            "turn" => applied += i64::from(steer(world, &entities, selected, 0, value)),
            _ => ignored += 1,
        }
    }

    let mut controls = world
        .get_mut::<Controls>(console)
        .expect("the console carries the controls");
    controls.selected = selected;
    controls.applied += applied;
    controls.ignored += ignored;
}

/// Adds to the selected entity's velocity. Reports whether it found one.
fn steer(world: &mut World, entities: &[EntityId], selected: u32, ddx: i64, ddy: i64) -> bool {
    let Some(target) = entities.iter().copied().find(|id| id.index() == selected) else {
        return false;
    };
    let Ok(mut velocity) = world.get_mut::<Velocity>(target) else {
        return false;
    };

    velocity.dx += ddx;
    velocity.dy += ddy;
    true
}

/// Draws one tick's worth of synthetic input.
///
/// The stand-in for a player. Three quarters of ticks carry nothing, which is
/// what makes a recording sparse and therefore readable — and what exercises the
/// rule that an empty tick is not written down.
///
/// This runs during a recorded run and never during a replay: on replay the
/// events come from the file and this generator is not constructed at all. That
/// is what makes the tampering test meaningful, and it is only safe because the
/// generator is *not* part of the world — if it were, a replay would have to
/// advance it identically or the dumps would differ.
pub fn pilot(rng: &mut Rng) -> Vec<InputEvent> {
    if rng.next_u32_below(4) != 0 {
        return Vec::new();
    }

    let count = 1 + rng.next_u32_below(2);

    (0..count)
        .map(|_| {
            match rng.next_u32_below(3) {
                0 => InputEvent::new("select", i64::from(rng.next_u32_below(MOVERS))),
                1 => InputEvent::new("thrust", i64::from(rng.next_u32_below(5)) - 2),
                _ => InputEvent::new("turn", i64::from(rng.next_u32_below(3)) - 1),
            }
            .expect("the action names in this function are identifiers")
        })
        .collect()
}

/// Builds the simulation.
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

/// Builds the starting world: the movers first, then the console.
fn build_world() -> Result<World, EcsError> {
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
        // At rest. Everything that ever moves does so because input arrived.
        world.insert(entity, Velocity { dx: 0, dy: 0 })?;
    }

    let console = world.spawn();
    world.insert(console, Events::<InputEvent>::new())?;
    world.insert(
        console,
        Controls {
            selected: 0,
            applied: 0,
            ignored: 0,
        },
    )?;

    Ok(world)
}

/// Registers every component type this mode uses, the input buffer included.
fn build_registry() -> Result<ComponentRegistry, EcsError> {
    let mut registry = ComponentRegistry::new();

    registry.register_component::<Position>("position")?;
    registry.register_component::<Velocity>("velocity")?;
    registry.register_component::<Controls>("controls")?;
    registry.register_component::<Events<InputEvent>>("input")?;

    Ok(registry)
}

/// The run order.
///
/// Rotation runs first, and that placement is what gives input its timing: the
/// runner sends a tick's input into the buffer *between* ticks, so the rotation
/// at the top of the tick makes it readable within that same tick. ADR-0011's
/// one-tick delay applies to a system sending to another system; the runner is
/// not a system and does not send from inside a tick, so there is no ordering
/// hazard to protect against here. ADR-0012 records that reading.
fn build_scheduler() -> Result<Scheduler, EcsError> {
    let mut scheduler = Scheduler::new();

    scheduler.add_system("input/rotate", rotate_events::<InputEvent>)?;
    scheduler.add_system("input/apply", apply_input)?;
    scheduler.add_system("movement", movement)?;

    Ok(scheduler)
}

/// Puts one tick's input where the simulation will read it.
///
/// Called by the runner before the tick runs. Returns the events it accepted so
/// the caller can record them, which is what keeps "what was fed" and "what was
/// written to the file" the same list by construction rather than by agreement.
///
/// # Errors
///
/// Anything reading the buffer can raise.
pub fn feed(world: &mut World, events: Vec<InputEvent>) -> Result<(), EcsError> {
    if events.is_empty() {
        return Ok(());
    }

    let console = console(world);
    let mut buffer = world.get_mut::<Events<InputEvent>>(console)?;
    for event in events {
        buffer.send(event);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ACTIONS, Controls, MOVERS, build, build_scheduler, console, feed, pilot};
    use crate::sim::{Simulation, Velocity};
    use narvo_ecs::{Rng, SystemContext};
    use narvo_input::InputEvent;

    fn event(action: &str, value: i64) -> InputEvent {
        InputEvent::new(action, value).expect("the test names are valid")
    }

    /// Drives a simulation, feeding `inputs(tick)` before each tick.
    fn run(ticks: u64, mut inputs: impl FnMut(u64) -> Vec<InputEvent>) -> Simulation {
        let mut simulation = build().expect("the demo always builds");
        for tick in 0..ticks {
            feed(&mut simulation.world, inputs(tick)).expect("the console exists");
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
        }
        simulation
    }

    fn controls(simulation: &Simulation) -> (u32, i64, i64) {
        let console = console(&simulation.world);
        let controls = simulation
            .world
            .get::<Controls>(console)
            .expect("the console carries the controls");
        (controls.selected, controls.applied, controls.ignored)
    }

    fn velocities(simulation: &Simulation) -> Vec<(i64, i64)> {
        let mut query = simulation.world.query::<&Velocity>();
        let mut all: Vec<(i64, i64)> = query
            .iter()
            .map(|(_id, velocity)| (velocity.dx, velocity.dy))
            .collect();
        all.sort_unstable();
        all
    }

    /// The canonical dump, which unlike [`velocities`] keeps *which* entity a
    /// value belongs to.
    fn dump(simulation: &Simulation) -> String {
        narvo_ecs::canonical_dump(&simulation.world, &simulation.registry)
            .expect("every component of this mode is registered")
    }

    #[test]
    fn a_world_with_no_input_never_moves() {
        // The property everything else here rests on: this simulation has no
        // internal source of change at all.
        let idle = run(500, |_tick| Vec::new());

        assert!(
            velocities(&idle)
                .iter()
                .all(|(dx, dy)| *dx == 0 && *dy == 0)
        );
        assert_eq!(controls(&idle), (0, 0, 0));
    }

    #[test]
    fn input_reaches_the_state_in_the_tick_it_was_fed_for() {
        // Fed before tick 0, readable in tick 0 - the rotation at the top of the
        // tick is what arranges that, and it is the timing a recording assumes.
        let simulation = run(1, |tick| {
            if tick == 0 {
                vec![event("thrust", 5)]
            } else {
                Vec::new()
            }
        });

        assert_eq!(controls(&simulation).1, 1, "the input was not applied");
        assert!(velocities(&simulation).contains(&(5, 0)));
    }

    #[test]
    fn selection_steers_which_entity_moves() {
        let simulation = run(2, |tick| match tick {
            0 => vec![event("select", 3), event("thrust", 7)],
            _ => Vec::new(),
        });

        let (selected, applied, ignored) = controls(&simulation);
        assert_eq!((selected, applied, ignored), (3, 2, 0));
        assert!(velocities(&simulation).contains(&(7, 0)));
    }

    #[test]
    fn the_order_within_a_tick_is_observable() {
        // select-then-thrust steers entity 5; thrust-then-select steers whatever
        // was selected before, which is entity 0. So a recording whose lines
        // were reordered cannot produce the same state - and the comparison has
        // to be over the dump rather than over `velocities`, because the latter
        // sorts and would hide which entity moved.
        let first = run(1, |_| vec![event("select", 5), event("thrust", 1)]);
        let second = run(1, |_| vec![event("thrust", 1), event("select", 5)]);

        assert_eq!(
            velocities(&first),
            velocities(&second),
            "the same values move, which is exactly why this needs the dump"
        );
        assert_ne!(dump(&first), dump(&second));
    }

    #[test]
    fn a_negative_selection_lands_inside_the_range() {
        // A hand-edited recording can say anything; it has to land somewhere
        // defined rather than panicking or steering nothing.
        let simulation = run(1, |_| vec![event("select", -1)]);

        assert_eq!(controls(&simulation).0, MOVERS - 1);
    }

    #[test]
    fn an_unknown_action_is_counted_rather_than_silently_dropped() {
        let simulation = run(1, |_| vec![event("teleport", 1)]);

        assert_eq!(controls(&simulation), (0, 0, 1));
    }

    #[test]
    fn the_run_order_is_the_registration_order() {
        let scheduler = build_scheduler().expect("builds");

        assert_eq!(
            scheduler.system_names().collect::<Vec<_>>(),
            vec!["input/rotate", "input/apply", "movement"]
        );
    }

    #[test]
    fn the_pilot_is_reproducible_and_leaves_most_ticks_empty() {
        let mut first = Rng::new(1);
        let mut second = Rng::new(1);

        let left: Vec<Vec<InputEvent>> = (0..1_000).map(|_| pilot(&mut first)).collect();
        let right: Vec<Vec<InputEvent>> = (0..1_000).map(|_| pilot(&mut second)).collect();

        assert_eq!(left, right);

        let busy = left.iter().filter(|tick| !tick.is_empty()).count();
        assert!(
            (150..400).contains(&busy),
            "the pilot should leave most ticks empty, {busy} of 1000 had input"
        );
        assert!(
            left.iter().any(|tick| tick.len() == 2),
            "some tick should carry more than one input"
        );
    }

    #[test]
    fn a_different_seed_gives_the_pilot_a_different_stream() {
        let mut first = Rng::new(1);
        let mut second = Rng::new(2);

        let left: Vec<Vec<InputEvent>> = (0..1_000).map(|_| pilot(&mut first)).collect();
        let right: Vec<Vec<InputEvent>> = (0..1_000).map(|_| pilot(&mut second)).collect();

        assert_ne!(left, right);
    }

    #[test]
    fn the_pilot_only_produces_actions_this_mode_knows() {
        // The recording written by a live run has to pass the validation a
        // replayed one is put through.
        let mut rng = Rng::new(3);

        for _ in 0..2_000 {
            for event in pilot(&mut rng) {
                assert!(
                    ACTIONS.contains(&event.action()),
                    "the pilot produced an unknown action: {}",
                    event.action()
                );
            }
        }
    }

    #[test]
    fn the_action_list_is_sorted_and_free_of_duplicates() {
        // It is searched and printed in error messages; a duplicate would make
        // the message look confused and a wrong order would make it hard to scan.
        let mut sorted = ACTIONS;
        sorted.sort_unstable();
        assert_eq!(sorted, ACTIONS);
        assert!(ACTIONS.windows(2).all(|pair| pair[0] != pair[1]));
    }
}
