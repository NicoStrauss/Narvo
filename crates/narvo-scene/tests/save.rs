//! The save format, and the one oracle that can tell a save from a lie.
//!
//! Two properties are checked here and they are not equally strong, which is why
//! the weaker one is written first and labelled:
//!
//! 1. **The round trip.** A world written and read back has the same canonical
//!    dump. It is the property ADR-0018 states for scenes, over the wider domain
//!    a save reaches. It is also the property a *wrong* save passes, because a
//!    dump prints live entities and their components and nothing else.
//! 2. **The continuation.** Save at tick *n*, load, run to tick *m*, and the
//!    dump at *m* equals that of a run that was never interrupted. Everything a
//!    dump cannot show — the free list, the tick — is inside this one and
//!    outside the first.
//!
//! The simulation the second property runs is built here rather than borrowed,
//! because it has to contain all three ways a save can be wrong at once: a
//! system that reads the tick, a generator that draws, and entities that are
//! spawned **and** despawned so that slots recycle.

use narvo_ecs::{
    ComponentRegistry, EntityId, Rng, Scheduler, SystemContext, Transform, World, canonical_dump,
    first_difference,
};
use narvo_scene::save::{self, SaveError, Savepoint};
use proptest::prelude::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// The fixture simulation
// ---------------------------------------------------------------------------

/// A short-lived thing: the tick it appeared on, and how long it lasts.
///
/// The birth tick is stored rather than a countdown so that the reaping rule is
/// a function of *now* and *then*, which is what makes a run that resumes at the
/// wrong tick reap the wrong entities. The span varies within a burst so that
/// the members of one burst die at three different moments, which is what leaves
/// several slots free at once and in an order that is not ascending.
#[derive(Debug, Serialize, Deserialize)]
struct Mote {
    born: u64,
    span: u64,
}

/// What the run has done so far, on the one entity that outlives everything.
#[derive(Debug, Serialize, Deserialize)]
struct Ledger {
    spawned: i64,
    despawned: i64,
}

/// A burst of motes appears every this many ticks.
const SPAWN_EVERY: u64 = 7;

/// How many motes one burst holds, and how long each of them lives.
///
/// Three spans rather than one, and none of them a multiple of
/// [`SPAWN_EVERY`]: a burst that died together would free its slots in one go
/// and the next burst would take them all back, so the free list would never
/// hold more than a moment's worth and its order would never matter.
const SPANS: [u64; 3] = [11, 17, 23];

/// The seed the fixture's generator starts from.
const SEED: u64 = 0x5eed;

fn registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    registry
        .register_component::<Transform>("transform")
        .expect("a fresh registry accepts it");
    registry
        .register_component::<Rng>("rng")
        .expect("a fresh registry accepts it");
    registry
        .register_component::<Mote>("mote")
        .expect("a fresh registry accepts it");
    registry
        .register_component::<Ledger>("ledger")
        .expect("a fresh registry accepts it");
    registry
}

/// The one entity that holds the generator and the ledger.
///
/// Found rather than remembered, because a loaded world hands back no handles -
/// it hands back a world, and everything a system needs has to be findable in
/// it. `entity_ids` is the canonical ascending enumeration, so this answers the
/// same way in every run.
fn director(world: &World) -> Option<EntityId> {
    world
        .entity_ids()
        .into_iter()
        .find(|id| world.has::<Ledger>(*id))
}

/// Moves every mote to where the tick and its own birth say it should be, with a
/// draw from the world's generator on top.
///
/// Three of the four things a save can forget meet in this one function: the
/// tick decides `x`, the generator decides `rotation`, and how many draws are
/// taken depends on how many motes are alive - so a run that resumed with the
/// wrong entity set diverges in the generator as well as in the positions.
fn drift(world: &mut World, context: &SystemContext) {
    let tick = context.tick();

    let motes: Vec<(EntityId, u64)> = world
        .entity_ids()
        .into_iter()
        .filter_map(|id| world.get::<Mote>(id).ok().map(|mote| (id, mote.born)))
        .collect();

    let Some(director) = director(world) else {
        return;
    };

    let mut jitters = Vec::with_capacity(motes.len());
    {
        let mut rng = world
            .get_mut::<Rng>(director)
            .expect("the director carries the generator");
        for _ in &motes {
            jitters.push(rng.next_u32_below(360));
        }
    }

    for ((id, born), jitter) in motes.into_iter().zip(jitters) {
        if let Ok(mut transform) = world.get_mut::<Transform>(id) {
            transform.x = (tick % 97) as f32;
            transform.y = born as f32;
            transform.rotation = f32::from(u16::try_from(jitter).unwrap_or(u16::MAX));
        }
    }
}

/// Adds a burst of motes every [`SPAWN_EVERY`] ticks.
fn populate(world: &mut World, context: &SystemContext) {
    let tick = context.tick();
    if !tick.is_multiple_of(SPAWN_EVERY) {
        return;
    }

    for span in SPANS {
        let mote = world.spawn();
        world
            .insert(mote, Mote { born: tick, span })
            .expect("just spawned");
        world
            .insert(mote, Transform::default())
            .expect("just spawned");
    }

    if let Some(director) = director(world) {
        world
            .get_mut::<Ledger>(director)
            .expect("the director carries the ledger")
            .spawned += i64::try_from(SPANS.len()).unwrap_or(i64::MAX);
    }
}

/// Removes every mote that has outlived its span.
fn reap(world: &mut World, context: &SystemContext) {
    let tick = context.tick();

    // Collected before anything is despawned: a query holds a borrow, and
    // despawning inside one would be mutating the thing being read.
    let doomed: Vec<EntityId> = world
        .entity_ids()
        .into_iter()
        .filter(|id| {
            world
                .get::<Mote>(*id)
                .is_ok_and(|mote| tick >= mote.born + mote.span)
        })
        .collect();

    for id in &doomed {
        world.despawn(*id).expect("it was alive a moment ago");
    }

    if let Some(director) = director(world) {
        world
            .get_mut::<Ledger>(director)
            .expect("the director carries the ledger")
            .despawned += i64::try_from(doomed.len()).unwrap_or(i64::MAX);
    }
}

/// A fresh run: the world it starts in and the order its systems run in.
fn start() -> (World, Scheduler) {
    let mut world = World::new();
    let director = world.spawn();
    world
        .insert(director, Rng::new(SEED))
        .expect("just spawned");
    world
        .insert(
            director,
            Ledger {
                spawned: 0,
                despawned: 0,
            },
        )
        .expect("just spawned");

    let mut scheduler = Scheduler::new();
    scheduler
        .add_system("reap", reap)
        .expect("a fresh scheduler");
    scheduler
        .add_system("populate", populate)
        .expect("a fresh scheduler");
    scheduler
        .add_system("drift", drift)
        .expect("a fresh scheduler");

    (world, scheduler)
}

/// Runs the ticks `from..to`.
fn run(world: &mut World, scheduler: &Scheduler, from: u64, to: u64) {
    for tick in from..to {
        scheduler.run(world, &SystemContext::new(tick));
    }
}

/// The tick a save is taken at, and the tick both runs are compared at.
///
/// `SAVE_AT` is not round: it was chosen by walking the fixture and reading off
/// a tick where the free list holds **three** slots whose indices are not
/// ascending — `9v4`, `2v6`, `4v8` — so that a save which lost the free list's
/// *order* fails as loudly as one that lost the list. `assert_not_trivial` holds
/// that property rather than trusting the number.
const SAVE_AT: u64 = 104;
const COMPARE_AT: u64 = 200;

// ---------------------------------------------------------------------------
// Oracle 1 - the round trip, and it is the weaker one
// ---------------------------------------------------------------------------

/// What one generated entity carries. Every component is optional, so the
/// strategy reaches the empty entity as well as the full one.
#[derive(Debug, Clone)]
struct GeneratedEntity {
    transform: Option<(f32, f32)>,
    rng: Option<(u64, u64)>,
    mote: Option<(u64, u64)>,
}

/// A world of the shape a *save* can describe, which is wider than a scene's.
#[derive(Debug, Clone)]
struct Generated {
    /// One entry per entity spawned, and what it carries.
    entities: Vec<GeneratedEntity>,
    /// Which entities are despawned, in the order they are despawned. Indices
    /// are taken modulo the entity count, and a repeat is a despawn that fails
    /// and is ignored - both of which a real run does too.
    kills: Vec<usize>,
    /// How many entities are spawned afterwards, into the freed slots.
    respawns: usize,
}

fn generated_entity() -> impl Strategy<Value = GeneratedEntity> {
    (
        prop::option::of((any::<f32>(), any::<f32>())),
        prop::option::of((any::<u64>(), any::<u64>())),
        prop::option::of((any::<u64>(), any::<u64>())),
    )
        .prop_map(|(transform, rng, mote)| GeneratedEntity {
            transform,
            rng,
            mote,
        })
}

fn generated() -> impl Strategy<Value = Generated> {
    (
        prop::collection::vec(generated_entity(), 1..7),
        prop::collection::vec(any::<usize>(), 0..5),
        0..4_usize,
    )
        .prop_map(|(entities, kills, respawns)| Generated {
            entities,
            kills,
            respawns,
        })
}

fn build(plan: &Generated) -> World {
    let mut world = World::new();
    let ids: Vec<EntityId> = plan.entities.iter().map(|_| world.spawn()).collect();

    for (entity, id) in plan.entities.iter().zip(&ids) {
        let GeneratedEntity {
            transform,
            rng,
            mote,
        } = entity;

        if let Some((x, y)) = *transform {
            world
                .insert(
                    *id,
                    Transform {
                        x,
                        y,
                        rotation: 0.0,
                        scale_x: 1.0,
                        scale_y: 1.0,
                    },
                )
                .expect("just spawned");
        }
        if let Some((seed, stream)) = *rng {
            world
                .insert(*id, Rng::with_stream(seed, stream))
                .expect("just spawned");
        }
        if let Some((born, span)) = *mote {
            world
                .insert(*id, Mote { born, span })
                .expect("just spawned");
        }
    }

    for kill in &plan.kills {
        // A repeated index is a despawn of something already gone, which is an
        // ordinary thing for a run to attempt and is ignored here as it is
        // there.
        let _ = world.despawn(ids[kill % ids.len()]);
    }

    for index in 0..plan.respawns {
        let id = world.spawn();
        world
            .insert(
                id,
                Mote {
                    born: index as u64,
                    span: 1,
                },
            )
            .expect("just spawned");
    }

    world
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// **The weaker oracle, and it is first so that it is not mistaken for the
    /// strong one.** A world survives being written as a save and read back.
    ///
    /// The domain is the one ADR-0018 handed over: worlds with despawn history,
    /// gaps in their slots and generations above one - every world, not just the
    /// ones a scene can constitute.
    #[test]
    fn a_world_survives_a_save_and_a_load(plan in generated()) {
        let registry = registry();
        let world = build(&plan);

        let text = save::to_string(&world, 4200, &registry)
            .map_err(|error| TestCaseError::fail(format!("writing failed: {error}")))?;
        let loaded = save::from_str(&text, &registry)
            .map_err(|error| TestCaseError::fail(format!("reading failed: {error}\n{text}")))?;

        prop_assert_eq!(loaded.tick, 4200);

        let before = canonical_dump(&world, &registry).expect("registered");
        let after = canonical_dump(&loaded.world, &registry).expect("registered");

        if let Some(difference) = first_difference(&before, &after) {
            return Err(TestCaseError::fail(format!(
                "the world did not survive the round trip.\n{difference}\n\nsave:\n{text}"
            )));
        }

        // What the dump cannot see, checked beside it: the same free list means
        // the same handles from the next spawn onwards.
        prop_assert_eq!(loaded.world.freelist(), world.freelist());
    }
}

// ---------------------------------------------------------------------------
// Oracle 2 - the continuation, and it is the real one
// ---------------------------------------------------------------------------

/// **The oracle a wrong save fails.** A run interrupted by a save and a load
/// reaches the same state as one that was never interrupted.
///
/// A save that forgets the tick passes the round trip and fails here on the
/// first tick. A save that forgets the free list passes the round trip and fails
/// here the first time something spawns. Neither is visible in a canonical dump
/// taken at the moment of saving, which is what makes this test the one that
/// carries the task.
#[test]
fn a_loaded_run_continues_where_the_saved_one_stopped() {
    let registry = registry();

    let (mut uninterrupted, scheduler) = start();
    run(&mut uninterrupted, &scheduler, 0, COMPARE_AT);

    let (mut interrupted, scheduler) = start();
    run(&mut interrupted, &scheduler, 0, SAVE_AT);

    assert_not_trivial(&interrupted, &registry);

    let text = save::to_string(&interrupted, SAVE_AT, &registry).expect("every component is known");
    let Savepoint { mut world, tick } = save::from_str(&text, &registry).expect("just written");
    assert_eq!(tick, SAVE_AT, "the save carries the tick it was taken at");

    run(&mut world, &scheduler, tick, COMPARE_AT);

    let expected = canonical_dump(&uninterrupted, &registry).expect("registered");
    let actual = canonical_dump(&world, &registry).expect("registered");

    assert!(
        first_difference(&expected, &actual).is_none(),
        "a resumed run diverged from an uninterrupted one:\n{}",
        first_difference(&expected, &actual).expect("just asserted present")
    );

    // The two runs also agree about what comes *next*, which the dump does not
    // show: the next spawn has to land in the same slot at the same generation.
    assert_eq!(world.freelist(), uninterrupted.freelist());
}

/// Refuses to let the oracle above become an empty world.
///
/// Every clause is one of the ways a saved world can be trivial, and a trivial
/// world round-trips no matter what the format does. They are assertions rather
/// than a comment for exactly that reason.
fn assert_not_trivial(world: &World, registry: &ComponentRegistry) {
    let ids = world.entity_ids();
    assert!(
        ids.len() >= 3,
        "the saved world should hold several entities, held {}",
        ids.len()
    );

    assert!(
        ids.iter().any(|id| id.generation() > 1),
        "no slot has been recycled, so the save never exercises a generation above one"
    );

    let free = world.freelist();
    assert!(
        free.len() >= 2,
        "fewer than two slots are free, so the *order* of the free list never matters here: {free:?}"
    );
    assert!(
        free.windows(2)
            .any(|pair| pair[0].index() > pair[1].index()),
        "the free list is in ascending order, so a save that sorted it would still pass: {free:?}"
    );
    assert!(
        free.iter().any(|id| id.generation() > 1),
        "no free slot has been handed out before, so its generation carries nothing: {free:?}"
    );

    assert!(
        ids.iter().any(|id| world.has::<Mote>(*id)),
        "no mote is alive"
    );

    // The generator has drawn: its state is no longer the one a fresh one holds.
    let director = director(world).expect("the director outlives everything");
    let drawn = registry
        .serialize_component("rng", world, director)
        .expect("registered")
        .expect("the director carries a generator");
    let mut fresh = World::new();
    let untouched = fresh.spawn();
    fresh
        .insert(untouched, Rng::new(SEED))
        .expect("just spawned");
    let unused = registry
        .serialize_component("rng", &fresh, untouched)
        .expect("registered")
        .expect("just inserted");
    assert_ne!(
        drawn, unused,
        "the generator has not drawn, so a save that dropped its state would pass"
    );

    let ledger = world
        .get::<Ledger>(director)
        .expect("the director carries the ledger");
    assert!(
        ledger.spawned > 0 && ledger.despawned > 0,
        "the run should have both spawned and despawned, did {ledger:?}"
    );
}

/// The tick is the second thing a canonical dump cannot show, and this is what
/// says so in one place.
///
/// It runs the same simulation twice from the same saved state and feeds the
/// second one a tick that is off by one. The dumps at the moment of loading are
/// identical - that is the point - and the states one tick later are not.
#[test]
fn the_tick_is_state_the_canonical_dump_does_not_carry() {
    let registry = registry();

    let (mut world, scheduler) = start();
    run(&mut world, &scheduler, 0, SAVE_AT);

    let text = save::to_string(&world, SAVE_AT, &registry).expect("every component is known");
    let mut right = save::from_str(&text, &registry).expect("just written");
    let mut wrong = save::from_str(&text, &registry).expect("just written");

    let at_load = canonical_dump(&right.world, &registry).expect("registered");
    assert_eq!(
        at_load,
        canonical_dump(&wrong.world, &registry).expect("registered"),
        "two loads of one save start identical"
    );

    run(&mut right.world, &scheduler, right.tick, right.tick + 1);
    run(&mut wrong.world, &scheduler, wrong.tick + 1, wrong.tick + 2);

    assert_ne!(
        canonical_dump(&right.world, &registry).expect("registered"),
        canonical_dump(&wrong.world, &registry).expect("registered"),
        "resuming one tick out should change the state, or the fixture reads no tick"
    );
}

// ---------------------------------------------------------------------------
// The domain boundary, from the other side
// ---------------------------------------------------------------------------

/// The world `narvo_scene::to_string` refuses by name is the world this format
/// exists for.
///
/// Asserted together so that the handover ADR-0018 Decision 5 describes is a
/// property of the code rather than of two documents agreeing.
#[test]
fn a_world_the_scene_writer_refuses_is_one_the_save_writer_takes() {
    let registry = registry();

    let mut world = World::new();
    let ids: Vec<EntityId> = (0..3).map(|_| world.spawn()).collect();
    for id in &ids {
        world
            .insert(*id, Transform::default())
            .expect("just spawned");
    }
    world.despawn(ids[1]).expect("alive");

    let refused = narvo_scene::to_string(&world, &registry)
        .expect_err("a scene cannot describe a world with a hole in it");
    assert!(
        refused.to_string().contains("cannot be written as a scene"),
        "unexpected refusal: {refused}"
    );

    let text = save::to_string(&world, 0, &registry).expect("a save can");
    let loaded = save::from_str(&text, &registry).expect("just written");

    assert_eq!(loaded.world.entity_ids(), world.entity_ids());
    assert_eq!(loaded.world.freelist(), world.freelist());
}

// ---------------------------------------------------------------------------
// The unpleasant edges, and what stands after each
// ---------------------------------------------------------------------------

/// A world that is running while a load is attempted, and the dump that says it
/// has not moved.
fn running(registry: &ComponentRegistry) -> (World, String) {
    let (mut world, scheduler) = start();
    run(&mut world, &scheduler, 0, SAVE_AT);
    let dump = canonical_dump(&world, registry).expect("registered");
    (world, dump)
}

#[test]
fn a_truncated_save_is_refused_and_the_running_world_stands() {
    let registry = registry();
    let (running_world, before) = running(&registry);

    let text = save::to_string(&running_world, SAVE_AT, &registry).expect("registered");
    let truncated = &text[..text.len() / 2];

    let error = save::from_str(truncated, &registry).expect_err("the file stops mid-entity");
    assert!(
        matches!(error, SaveError::Syntax { .. }),
        "expected a syntax error, got {error:?}"
    );

    assert_eq!(
        canonical_dump(&running_world, &registry).expect("registered"),
        before,
        "the running world moved while a load failed"
    );
}

#[test]
fn a_save_with_an_unknown_field_is_refused_and_the_running_world_stands() {
    let registry = registry();
    let (running_world, before) = running(&registry);

    let text = save::to_string(&running_world, SAVE_AT, &registry).expect("registered");
    let with_extra = text.replace("    tick:", "    weather: Rain,\n    tick:");

    let error = save::from_str(&with_extra, &registry).expect_err("this build has no such field");
    let message = error.to_string();
    assert!(
        message.contains("Unexpected field named `weather` in `Save`"),
        "unexpected message: {message}"
    );

    assert_eq!(
        canonical_dump(&running_world, &registry).expect("registered"),
        before,
        "the running world moved while a load failed"
    );
}

#[test]
fn a_save_from_another_version_is_refused_and_the_running_world_stands() {
    let registry = registry();
    let (running_world, before) = running(&registry);

    let text = save::to_string(&running_world, SAVE_AT, &registry).expect("registered");

    // Both directions, because they fail in two different places: an older
    // version still has this build's shape and is caught by the plain check,
    // and a later one carries a field this build has never heard of, so it is
    // caught only because the version is asked for before the failure is
    // reported.
    let older = text.replace("version: 1,", "version: 0,");
    let newer = text.replace("version: 1,", "version: 2,\n    weather: Rain,");

    for (label, file) in [("older", older), ("newer", newer)] {
        let error = save::from_str(&file, &registry).expect_err("this build reads version 1");
        match error {
            SaveError::UnsupportedVersion { found, supported } => {
                assert_eq!(supported, save::FORMAT_VERSION);
                assert_eq!(found, if label == "older" { 0 } else { 2 });
            }
            other => panic!("{label}: expected a version refusal, got {other:?}"),
        }
    }

    assert_eq!(
        canonical_dump(&running_world, &registry).expect("registered"),
        before,
        "the running world moved while a load failed"
    );
}

/// The realistic shape of "a save from an older build": a component that
/// existed when the file was written and does not exist now.
///
/// It is a fourth edge rather than a variant of the third, because a version
/// number cannot see it - the format did not change, the *engine* did.
#[test]
fn a_save_naming_a_component_this_build_does_not_know_is_refused() {
    let registry = registry();
    let (running_world, before) = running(&registry);

    let text = save::to_string(&running_world, SAVE_AT, &registry).expect("registered");
    let with_gone = text.replace("\"ledger\":", "\"morale\":");
    assert_ne!(with_gone, text, "the fixture should carry a ledger");

    let error = save::from_str(&with_gone, &registry).expect_err("no morale is registered");
    let message = error.to_string();
    assert!(
        message.contains("carries \"morale\", which no component is registered under"),
        "unexpected message: {message}"
    );
    assert!(
        message.contains("\"ledger\""),
        "the message should list what is known: {message}"
    );

    assert_eq!(
        canonical_dump(&running_world, &registry).expect("registered"),
        before,
        "the running world moved while a load failed"
    );
}

/// A save whose entity table skips a slot is refused by the slot, not accepted
/// as a smaller world.
///
/// This is what makes a save that dropped its free list impossible to express
/// rather than merely wrong: the table has to account for every slot below its
/// highest.
#[test]
fn a_save_whose_table_skips_a_slot_is_refused() {
    let registry = registry();

    let mut world = World::new();
    let ids: Vec<EntityId> = (0..3).map(|_| world.spawn()).collect();
    for id in &ids {
        world
            .insert(*id, Transform::default())
            .expect("just spawned");
    }
    world.despawn(ids[1]).expect("alive");

    let text = save::to_string(&world, 0, &registry).expect("registered");
    let without_free = text
        .lines()
        .filter(|line| !line.contains("(index: 1, generation: 2)"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(without_free, text, "the fixture should have a free slot");

    let error = save::from_str(&without_free, &registry).expect_err("slot 1 is named by nothing");
    let message = error.to_string();
    assert!(
        message.contains("slot 1 is named neither live nor free"),
        "unexpected message: {message}"
    );
}

/// One entity naming one component twice is refused rather than resolved.
#[test]
fn a_save_naming_one_component_twice_is_refused() {
    let registry = registry();

    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, Transform::default())
        .expect("just spawned");

    let text = save::to_string(&world, 0, &registry).expect("registered");
    let doubled = text.replace(
        "\"transform\":",
        "\"transform\": (x:9.0,y:9.0,rotation:0.0,scale_x:1.0,scale_y:1.0),\n                \"transform\":",
    );

    let error = save::from_str(&doubled, &registry).expect_err("one entity, one transform");
    let message = error.to_string();
    assert!(
        message.contains("names \"transform\" twice"),
        "unexpected message: {message}"
    );
}

/// A body that is not a valid rendering of its component names the component and
/// the entity.
#[test]
fn a_body_that_does_not_parse_names_the_component_and_the_entity() {
    let registry = registry();

    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, Transform::default())
        .expect("just spawned");

    let text = save::to_string(&world, 0, &registry).expect("registered");
    let broken = text.replace("rotation:0.0", "rotation:\"east\"");
    assert_ne!(broken, text, "the fixture should carry a rotation");

    let error = save::from_str(&broken, &registry).expect_err("a rotation is not a string");
    let message = error.to_string();
    assert!(
        message.contains("the \"transform\" of EntityId(0v1) could not be read back"),
        "unexpected message: {message}"
    );
}

/// A missing file names the path rather than panicking.
#[test]
fn a_save_that_is_not_there_names_the_path() {
    let registry = registry();
    let path = std::path::Path::new("this-save-does-not-exist.ron");

    let error = save::from_file(path, &registry).expect_err("nothing is there");
    assert!(
        matches!(error, SaveError::Io { .. }),
        "expected an io error, got {error:?}"
    );
    assert!(
        error.to_string().contains("this-save-does-not-exist.ron"),
        "the message should name the path: {error}"
    );
}
