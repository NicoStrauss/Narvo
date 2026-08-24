//! Determinism precursor: two identically built worlds, driven through the same
//! thousand ticks, have to end up with the same canonical state.
//!
//! This is an integration test rather than a unit test on purpose: it reaches
//! the crate the way a caller does, through the public API and nothing else, and
//! so it demonstrates that a whole simulation can be built, run and dumped
//! without naming `hecs` once.
//!
//! Demonstrates, not enforces. Cargo hands a package's `[dependencies]` to every
//! target it builds, so a file in this directory *can* write `hecs::World::new()`
//! and it will compile - the seam is upheld here by discipline, not by the build.
//! ADR-0005's argument that a facade breach shows up as a `Cargo.toml` diff holds
//! for crates downstream of `narvo-ecs`, which have no hecs dependency to reach
//! for; it does not hold inside this package.
//!
//! What it does *not* do is hash the state. Comparing the full dump is stronger
//! at this size and reports which entity diverged instead of that something did;
//! the canonical hash arrives with the headless runner in M2.2 and will be built
//! on the same two orders used here - [`World::entity_ids`] and
//! [`ComponentRegistry::iter`].

use narvo_ecs::{ComponentRegistry, EcsError, EntityId, Scheduler, SystemContext, World};
use serde::{Deserialize, Serialize};

/// How many entities the run starts with.
const INITIAL_ENTITIES: i64 = 64;

/// How many ticks each run is driven for.
const TICKS: u64 = 1_000;

#[derive(Debug, Serialize, Deserialize)]
struct Position {
    x: i64,
    y: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Velocity {
    dx: i64,
    dy: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Health {
    remaining: i32,
}

/// Marks the entity that keeps the run's counters.
///
/// The reaper needs a number to derive its replacements from, and a system
/// cannot hold one: it is a `fn` pointer with no captures. So the counter lives
/// in the world like every other piece of simulation state, which is exactly the
/// architecture rule this crate enforces - and it means the counter shows up in
/// the dump below and is compared along with everything else.
#[derive(Debug, Serialize, Deserialize)]
struct Bookkeeping {
    spawned: i64,
    reaped: i64,
}

/// Integer arithmetic only: no floats, no clock, no randomness. Position moves
/// by velocity every tick.
fn movement(world: &mut World, _context: &SystemContext) {
    let mut moving = world.query::<(&mut Position, &Velocity)>();
    for (_id, (position, velocity)) in moving.iter() {
        position.x += velocity.dx;
        position.y += velocity.dy;
    }
}

/// Everything with health loses one point per tick, so the population turns over
/// steadily rather than the run being a straight line.
fn aging(world: &mut World, _context: &SystemContext) {
    let mut living = world.query::<&mut Health>();
    for (_id, health) in living.iter() {
        health.remaining -= 1;
    }
}

/// Despawns everything that ran out of health and spawns a replacement for each,
/// which is what forces slot recycling and rising generations into the run.
///
/// The dead are collected before anything is despawned - a query holds a borrow
/// of the world - and the collected list is sorted first. The sort is not
/// cosmetic: query order is archetype order, and letting it decide which slot
/// gets recycled first would make the result depend on storage layout.
fn reaper(world: &mut World, context: &SystemContext) {
    let mut dead: Vec<EntityId> = {
        let mut living = world.query::<&Health>();
        living
            .iter()
            .filter(|(_id, health)| health.remaining <= 0)
            .map(|(id, _health)| id)
            .collect()
    };
    dead.sort_unstable();

    for entity in dead {
        world.despawn(entity).expect("the entity was just queried");
        bump(world, 0, 1);

        let replacement = world.spawn();
        let seed = context.tick() as i64 + entity.index() as i64;
        world
            .insert(replacement, Position { x: seed, y: -seed })
            .expect("the entity was just spawned");
        world
            .insert(
                replacement,
                Velocity {
                    dx: seed % 7 - 3,
                    dy: seed % 5 - 2,
                },
            )
            .expect("the entity was just spawned");
        world
            .insert(
                replacement,
                Health {
                    remaining: 20 + (seed % 13) as i32,
                },
            )
            .expect("the entity was just spawned");
        bump(world, 1, 0);
    }
}

/// Adds to the run's counters, wherever they live.
fn bump(world: &mut World, spawned: i64, reaped: i64) {
    let mut books = world.query::<&mut Bookkeeping>();
    for (_id, book) in books.iter() {
        book.spawned += spawned;
        book.reaped += reaped;
    }
}

/// Builds the starting world. Called twice with no arguments beyond the ones
/// baked in here, so both runs begin identically by construction.
fn build_world() -> Result<World, EcsError> {
    let mut world = World::new();

    let books = world.spawn();
    world.insert(
        books,
        Bookkeeping {
            spawned: 0,
            reaped: 0,
        },
    )?;

    for index in 0..INITIAL_ENTITIES {
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
        // Staggered lifetimes, so entities die on different ticks and the churn
        // is spread out instead of arriving as one wave.
        world.insert(
            entity,
            Health {
                remaining: 5 + (index % 37) as i32,
            },
        )?;
    }

    Ok(world)
}

fn build_scheduler() -> Result<Scheduler, EcsError> {
    let mut scheduler = Scheduler::new();
    scheduler.add_system("movement", movement)?;
    scheduler.add_system("aging", aging)?;
    scheduler.add_system("reaper", reaper)?;
    Ok(scheduler)
}

fn build_registry() -> Result<ComponentRegistry, EcsError> {
    let mut registry = ComponentRegistry::new();
    // Registered out of alphabetical order; the registry sorts.
    registry.register_component::<Velocity>("velocity")?;
    registry.register_component::<Position>("position")?;
    registry.register_component::<Bookkeeping>("bookkeeping")?;
    registry.register_component::<Health>("health")?;
    Ok(registry)
}

/// One line per entity and registered component, in canonical order: entities by
/// [`EntityId`], components by stable name.
type Dump = Vec<(EntityId, &'static str, Option<String>)>;

fn dump(world: &World, registry: &ComponentRegistry) -> Result<Dump, EcsError> {
    let mut lines = Vec::new();

    for entity in world.entity_ids() {
        for info in registry.iter() {
            lines.push((entity, info.name(), info.serialize(world, entity)?));
        }
    }

    Ok(lines)
}

/// Runs the whole simulation once and returns its final canonical state.
fn run() -> Result<Dump, EcsError> {
    let mut world = build_world()?;
    let scheduler = build_scheduler()?;
    let registry = build_registry()?;

    for tick in 0..TICKS {
        scheduler.run(&mut world, &SystemContext::new(tick));
    }

    dump(&world, &registry)
}

#[test]
fn two_identical_runs_end_in_the_same_canonical_state() -> Result<(), EcsError> {
    let first = run()?;
    let second = run()?;

    assert_eq!(first, second);
    Ok(())
}

#[test]
fn the_run_the_determinism_check_compares_is_not_a_trivial_one() -> Result<(), EcsError> {
    // A determinism test over a world that never changes passes for the wrong
    // reason. These assertions pin down that the run actually churned: entities
    // died, slots came back with higher generations, and positions moved.
    let mut world = build_world()?;
    let scheduler = build_scheduler()?;
    let registry = build_registry()?;

    let before = dump(&world, &registry)?;

    for tick in 0..TICKS {
        scheduler.run(&mut world, &SystemContext::new(tick));
    }

    let after = dump(&world, &registry)?;
    assert_ne!(before, after, "a thousand ticks have to change something");

    let books = world.entity_ids();
    let counters = books
        .iter()
        .find_map(|entity| world.get::<Bookkeeping>(*entity).ok())
        .expect("the bookkeeping entity survives - it has no health to lose");
    assert!(
        counters.reaped > INITIAL_ENTITIES,
        "the population should have turned over more than once, got {} reaped",
        counters.reaped
    );
    assert_eq!(
        counters.spawned, counters.reaped,
        "every reaped entity is replaced exactly once"
    );

    assert!(
        world
            .entity_ids()
            .iter()
            .any(|entity| entity.generation() > 1),
        "recycled slots have to appear, or generations are never exercised"
    );

    Ok(())
}

#[test]
fn the_canonical_dump_is_sorted_and_complete() -> Result<(), EcsError> {
    let mut world = build_world()?;
    let scheduler = build_scheduler()?;
    let registry = build_registry()?;

    for tick in 0..64 {
        scheduler.run(&mut world, &SystemContext::new(tick));
    }

    let lines = dump(&world, &registry)?;
    let entities = world.entity_ids();

    assert_eq!(lines.len(), entities.len() * registry.len());
    assert!(entities.windows(2).all(|pair| pair[0] < pair[1]));

    // Every entity contributes one block, and inside a block the component names
    // are in the registry's canonical order.
    let names: Vec<&'static str> = registry.iter().map(|info| info.name()).collect();
    assert_eq!(names, vec!["bookkeeping", "health", "position", "velocity"]);

    for (block, entity) in lines.chunks(registry.len()).zip(&entities) {
        let block_names: Vec<&'static str> = block.iter().map(|(_id, name, _)| *name).collect();
        assert_eq!(block_names, names);
        assert!(block.iter().all(|(id, _, _)| id == entity));
    }

    Ok(())
}
