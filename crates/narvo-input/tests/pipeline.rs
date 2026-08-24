//! A mapping file, end to end and headless: file → mapping → device events →
//! `InputEvent`s → the engine's own event pipeline.
//!
//! # Why this test is an integration test and not a unit test
//!
//! It is the one place in M5.1 where this crate meets `narvo-ecs`, and the
//! meeting is the point. `narvo-input` does not depend on `narvo-ecs` — the
//! whole crate cut rests on it not needing to (ADR-0025) — so the only way to
//! demonstrate that a mapping's output travels through the engine's event buffer
//! is from outside both, through a `dev-dependency` that no production build
//! ever sees.
//!
//! That also makes this the mechanical check of the survey's central finding:
//! `Events<T>` puts no bound on `T` beyond `serde`'s and `std`'s, so a type
//! defined in a crate `narvo-ecs` has never heard of is registrable, hashable
//! and rotatable exactly like one it owns. If that ever stopped being true, this
//! file would stop compiling.
//!
//! Nothing here is recorded and nothing here is a window. D8 stays open.

use std::path::Path;

use narvo_ecs::{
    ComponentRegistry, Events, Scheduler, SystemContext, World, canonical_dump, rotate_events,
};
use narvo_input::{Control, DeviceEvent, InputEvent, Mapping};

/// The committed mapping file, loaded from disk the way a game would load it.
fn demo_mapping() -> Mapping {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("mappings")
        .join("demo.ron");

    narvo_input::from_file(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// One tick of a player leaning on the controls: thrust down, a mover chosen,
/// then a turn, then thrust released.
fn one_tick_of_input() -> Vec<DeviceEvent> {
    vec![
        DeviceEvent::press(Control::KeyW),
        DeviceEvent::press(Control::Digit2),
        DeviceEvent::press(Control::ArrowRight),
        DeviceEvent::release(Control::KeyW),
        // Unbound: the file says nothing about it, so neither does the mapping.
        DeviceEvent::press(Control::Escape),
        DeviceEvent::release(Control::Digit2),
    ]
}

/// What that tick means, spelled out rather than derived from the mapping.
///
/// Written by hand on purpose: a expected list computed from the same mapping
/// the code under test used would agree with itself whatever the mapping said.
fn expected() -> Vec<(&'static str, i64)> {
    vec![
        ("thrust", 1),
        ("select", 2),
        ("turn", 1),
        ("thrust", 0),
        // `Escape` is unbound and `Digit2`'s binding is `OnPress`, so the two
        // events after the release of `KeyW` produce nothing at all.
    ]
}

#[test]
fn a_mapping_file_drives_the_engines_event_pipeline() {
    let mapping = demo_mapping();
    let events = mapping.map(&one_tick_of_input());

    assert_eq!(
        events
            .iter()
            .map(|event| (event.action(), event.value()))
            .collect::<Vec<_>>(),
        expected()
    );

    // The pipeline, exactly as `narvo-app`'s `input` mode builds it: a buffer
    // on one entity, the rotation registered first, the input fed in between
    // ticks (ADR-0012 §5).
    let mut world = World::new();
    let console = world.spawn();
    world
        .insert(console, Events::<InputEvent>::new())
        .expect("a fresh entity takes a component");

    let mut scheduler = Scheduler::new();
    scheduler
        .add_system("input/rotate", rotate_events::<InputEvent>)
        .expect("the first system by that name");

    for event in events {
        world
            .get_mut::<Events<InputEvent>>(console)
            .expect("the buffer is there")
            .send(event);
    }

    // ADR-0011: sent is not readable until the rotation has run.
    assert_eq!(
        world
            .get::<Events<InputEvent>>(console)
            .expect("the buffer is there")
            .len(),
        0
    );

    scheduler.run(&mut world, &SystemContext::new(1));

    let readable: Vec<(String, i64)> = world
        .get::<Events<InputEvent>>(console)
        .expect("the buffer is there")
        .iter()
        .map(|event| (event.action().to_owned(), event.value()))
        .collect();

    assert_eq!(
        readable
            .iter()
            .map(|(action, value)| (action.as_str(), *value))
            .collect::<Vec<_>>(),
        expected(),
        "the pipeline delivers what the mapping produced, in that order"
    );

    // ... and it is gone the tick after, which is the other half of ADR-0011.
    scheduler.run(&mut world, &SystemContext::new(2));
    assert_eq!(
        world
            .get::<Events<InputEvent>>(console)
            .expect("the buffer is there")
            .len(),
        0
    );
}

#[test]
fn a_buffer_of_mapped_events_is_registrable_and_reaches_the_canonical_dump() {
    // The crate-cut check. `InputEvent` now lives in a crate `narvo-ecs` does
    // not depend on, and this asserts that nothing about the registry, the dump
    // or the state hash noticed: the buffer registers under the same stable name
    // `narvo-app` already uses, and its rendering is the same one it had while
    // the type lived inside `narvo-ecs`.
    let mapping = demo_mapping();
    let events = mapping.map(&[DeviceEvent::press(Control::KeyW)]);

    let mut registry = ComponentRegistry::new();
    registry
        .register_component::<Events<InputEvent>>("input")
        .expect("a type from another crate registers like any other");

    let mut world = World::new();
    let console = world.spawn();
    world
        .insert(console, Events::<InputEvent>::new())
        .expect("a fresh entity takes a component");
    for event in events {
        world
            .get_mut::<Events<InputEvent>>(console)
            .expect("the buffer is there")
            .send(event);
    }

    let dump = canonical_dump(&world, &registry).expect("every component is registered");

    assert_eq!(
        dump,
        "entities 1\nentity 0v1\n  input (pending:[(action:\"thrust\",value:1)],readable:[])\n"
    );
}
