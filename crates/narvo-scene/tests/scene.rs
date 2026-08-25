//! The scene format, end to end: the specimen file, every error path, the
//! domain boundary, and the round-trip property M4 closes on.
//!
//! One file rather than four, because all of it needs the same registry and
//! ADR-0016's rule for a shared fixture is a crate, not a copy — a fixture used
//! in one crate does not earn one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use narvo_ecs::{
    Burst, Camera, ComponentRegistry, EntityId, Follow, HitRect, Layer, LightSource, Lit, Occluder,
    RigidBody, Rng, Sampling, Shake, Sprite, Tally, Tint, Transform, World, canonical_dump,
    first_difference,
};
use narvo_scene::SceneError;
use proptest::prelude::*;

/// Every component type the engine itself registers, under the stable names the
/// rest of the workspace already uses.
///
/// The census is taken from the code rather than from a list in a document:
/// these are the eight types in `narvo-ecs` that are public, registrable and
/// carry `Serialize + DeserializeOwned`. Six of them are the standing
/// registration obligation of `ProjektPlan.md` §12; `rng` is the seventh
/// (ADR-0010) and `sprite` the eighth (M4.8). Two things are deliberately *not*
/// here and the report says why: `Events<T>` needs a payload type before it can
/// have a stable name, and `Wander` belongs to a demo simulation in
/// `narvo-app` rather than to the engine.
///
/// **This list is transcribed rather than taken from
/// `register_engine_components`, and M4.8 measured what that costs.** Adding an
/// eighth type to the engine set turned four tests red across the workspace and
/// this crate's were not among them — `the_specimen_uses_every_registered_component`
/// compares the specimen against *this* list, so a type the engine registers and
/// this list does not know is invisible to it. The independence is worth
/// keeping: a wrong `register_engine_components` would otherwise move both sides
/// of that comparison together. What was missing is a statement that the two
/// agree, which `the_transcribed_census_is_the_engine_s_own_set` now is.
fn registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    registry
        .register_component::<Transform>("transform")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Burst>("burst")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Layer>("layer")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Sampling>("sampling")
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
        .register_component::<Rng>("rng")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Sprite>("sprite")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Occluder>("occluder")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<HitRect>("hitrect")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Tally>("tally")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<RigidBody>("rigidbody")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Tint>("tint")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<LightSource>("lightsource")
        .expect("a fresh registry accepts this component");
    registry
        .register_component::<Lit>("lit")
        .expect("a fresh registry accepts this component");
    registry
}

/// The transcribed census and the engine's own registration agree.
///
/// **The guard M4.8's cascade found missing.** `the_specimen_uses_every_registered_component`
/// is only as good as [`registry`], and [`registry`] is a hand-written list; a
/// type added to `register_engine_components` and not here would leave that test
/// green while the specimen no longer covered the set. This compares the two
/// lists by name, which keeps them independent statements — the transcription
/// still has to be made by hand — while making a divergence loud.
///
/// Names rather than types on purpose: the name is what a scene file writes, and
/// two registrations of the same type under different names would be a different
/// defect that this should not quietly absorb.
#[test]
fn the_transcribed_census_is_the_engine_s_own_set() {
    let transcribed: Vec<&str> = registry().iter().map(|info| info.name()).collect();

    let mut engine = ComponentRegistry::new();
    narvo_ecs::register_engine_components(&mut engine).expect("a fresh registry accepts them");
    let registered: Vec<&str> = engine.iter().map(|info| info.name()).collect();

    assert_eq!(
        transcribed, registered,
        "this file's hand-written census and `register_engine_components` have \
         drifted apart, so the specimen coverage test is measuring a set the \
         engine no longer has"
    );
}

/// Where the committed specimen lives, resolved from the crate rather than from
/// the working directory a test runner happens to pick.
fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("example.ron")
}

/// Asserts two worlds are the same world, and says where they stop being one.
///
/// Through the canonical dump rather than through any comparison of this
/// crate's own: the dump is what the state hash is taken over, so "the same
/// world" has exactly one meaning in this project and this uses it.
/// `first_difference` turns a failure into a location instead of two walls of
/// text — which is the whole reason it exists (M2.6).
fn assert_same_world(left: &World, right: &World, registry: &ComponentRegistry, case: &str) {
    let a = canonical_dump(left, registry).expect("every component is registered");
    let b = canonical_dump(right, registry).expect("every component is registered");

    if let Some(difference) = first_difference(&a, &b) {
        panic!("{case}: the two worlds differ.\n{difference}\n\nleft:\n{a}\nright:\n{b}");
    }
}

// ---- the specimen ------------------------------------------------------

/// The world `scenes/example.ron` describes, built by hand.
///
/// Hand-built on purpose: comparing the file against something derived from the
/// file would pass however wrong the loader is. Every value here was written
/// from the file by reading it, which is what a reader of the format has to be
/// able to do.
fn reference_world() -> World {
    let mut world = World::new();

    let eye = world.spawn();
    let anonymous = world.spawn();
    let player = world.spawn();

    world
        .insert(eye, Camera::new(0.0, 0.0, 2.0))
        .expect("the entity was just spawned");
    world
        .insert(eye, Follow::damped(player, 0.0, 0.0, 0.5))
        .expect("the entity was just spawned");
    world
        .insert(
            eye,
            Shake {
                amplitude: 4.0,
                frequency: 1.0,
                decay: 0.5,
                phase: 0.0,
                cutoff: 0.25,
                base_x: 0.0,
                base_y: 0.0,
            },
        )
        .expect("the entity was just spawned");

    world
        .insert(
            anonymous,
            Transform {
                x: -8.0,
                y: 0.5,
                ..Transform::IDENTITY
            },
        )
        .expect("the entity was just spawned");
    world
        .insert(anonymous, Layer::at(-1.0))
        .expect("the entity was just spawned");
    world
        .insert(anonymous, RigidBody::dynamic(0.5, 0.25).at(-8.0, 0.5))
        .expect("the entity was just spawned");
    world
        .insert(anonymous, LightSource::new(24.0, 1.0, 0.75))
        .expect("the entity was just spawned");

    world
        .insert(
            player,
            Transform {
                x: 3.5,
                y: -2.25,
                ..Transform::IDENTITY
            },
        )
        .expect("the entity was just spawned");
    world
        .insert(player, Layer::at(1.0))
        .expect("the entity was just spawned");
    world
        .insert(player, Sampling::linear())
        .expect("the entity was just spawned");
    world
        .insert(player, Rng::new(1))
        .expect("the entity was just spawned");
    world
        .insert(player, Sprite::new("hero"))
        .expect("the entity was just spawned");
    world
        .insert(player, HitRect::new(16.0, 16.0, "select", 1))
        .expect("a fresh entity takes a component");
    world
        .insert(player, Occluder::new(8.0, 12.0))
        .expect("a fresh entity takes a component");
    world
        .insert(player, Tally::new("select"))
        .expect("a fresh entity takes a component");
    world
        .insert(player, Tint::rgb(1.0, 0.75, 0.5))
        .expect("a fresh entity takes a component");
    // Spent, matching the specimen: `Burst::new` arms, so the age is set after
    // it rather than being spelled with a struct literal that would have to be
    // rewritten field for field if the type grew one.
    world
        .insert(
            player,
            Burst {
                age: 12,
                ..Burst::new(24, 7, 12, 1.5, 1.0)
            },
        )
        .expect("a fresh entity takes a component");
    // Whatever an author writes is overwritten by the first tick, so the
    // specimen and this reference agree on the *starting* value and nothing
    // more — which is `Lit::DARK`, and is why the specimen writes zero.
    world
        .insert(player, Lit::DARK)
        .expect("a fresh entity takes a component");

    world
}

#[test]
fn the_example_scene_loads_into_the_reference_world() {
    let registry = registry();
    let loaded = narvo_scene::from_file(&example_path(), &registry)
        .expect("the committed specimen has to load");

    assert_same_world(&loaded, &reference_world(), &registry, "the specimen");
}

/// The forward reference in the specimen actually points at the player.
///
/// `assert_same_world` would catch a wrong target too, but only as two lines of
/// dump text. This says the thing itself, so a failure names the defect rather
/// than its shadow.
#[test]
fn the_specimen_resolves_its_forward_reference_to_the_entity_declared_last() {
    let registry = registry();
    let world = narvo_scene::from_file(&example_path(), &registry).expect("loads");

    let ids = world.entity_ids();
    let eye = ids[0];
    let player = ids[2];

    let follow = *world.get::<Follow>(eye).expect("the eye carries a follow");
    assert_eq!(
        follow.target, player,
        "the reference named \"player\", which is the entity declared last"
    );
    assert!(
        world.get::<Transform>(follow.target).is_ok(),
        "the resolved handle has to be live and carry what the target carries"
    );
}

/// File order is spawn order, and a fresh world hands out ascending slots at
/// generation 1.
///
/// The loader does not rely on this - it keeps the handles the world gives it -
/// so this is the assertion that lets the *format* promise it. Without it,
/// "the n-th entity of the file is slot n" would be a sentence in an ADR that
/// nothing checks.
#[test]
fn file_order_is_spawn_order_and_slots_ascend() {
    let registry = registry();
    let world = narvo_scene::from_file(&example_path(), &registry).expect("loads");

    let ids = world.entity_ids();
    assert_eq!(ids.len(), 3);
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(id.index(), u32::try_from(index).expect("three entities"));
        assert_eq!(
            id.generation(),
            1,
            "a fresh world has never recycled a slot, so every generation is the first"
        );
    }
}

/// The specimen exercises every registered type, checked rather than claimed.
#[test]
fn the_specimen_uses_every_registered_component() {
    let registry = registry();
    let world = narvo_scene::from_file(&example_path(), &registry).expect("loads");

    let mut seen = BTreeSet::new();
    for entity in world.entity_ids() {
        for info in registry.iter() {
            if info
                .serialize(&world, entity)
                .expect("the entity is alive")
                .is_some()
            {
                seen.insert(info.name());
            }
        }
    }

    let registered: BTreeSet<&str> = registry.iter().map(|info| info.name()).collect();
    assert_eq!(
        seen, registered,
        "the specimen has to carry every registered component at least once, or a type \
         could stop round-tripping without this file noticing"
    );
}

/// The specimen through the second direction: file → world → text → world.
///
/// The dumps must agree. The two *texts* need not, and this asserts that they
/// do not, so that the freedom is a recorded property rather than an accident
/// waiting to be tightened by somebody who assumes byte equality holds. What
/// differs is exactly what a world does not carry: the entity names and the
/// symbolic reference, which are scene syntax.
#[test]
fn the_specimen_survives_a_round_trip_without_the_texts_matching() {
    let registry = registry();
    let original = std::fs::read_to_string(example_path()).expect("the specimen is readable");

    let loaded = narvo_scene::from_str(&original, &registry).expect("loads");
    let written = narvo_scene::to_string(&loaded, &registry).expect("writes");
    let reloaded = narvo_scene::from_str(&written, &registry).expect("reloads");

    assert_same_world(&loaded, &reloaded, &registry, "specimen round trip");

    assert_ne!(
        original, written,
        "byte equality is deliberately not promised; if it ever held, the reason would \
         need finding rather than celebrating"
    );
    // **A substring search over the whole document, and it is weaker than it
    // reads.** M4.8 tripped it without breaking anything: giving the specimen a
    // `sprite` whose region was called "player" made this fail, because a region
    // name is an ordinary string value and this cannot tell one occurrence of
    // "player" from another. The region was renamed rather than the assertion
    // rewritten — a stricter form would have to know the format's syntax, which
    // is what the rest of this test avoids on purpose — and the limit is
    // recorded here so the next collision is recognised rather than debugged.
    assert!(
        !written.contains("\"player\""),
        "a name is scene syntax and a world does not carry one, so it cannot come back out"
    );
    assert!(
        written.contains("(index:2,generation:1)"),
        "the reference has to come back as the handle it resolved to:\n{written}"
    );
}

/// A spliced reference is spelled exactly as the handle spells itself.
///
/// The scene crate never writes a field of `EntityId` by hand. If it did, that
/// second spelling would be free to drift from the one the registry writes for
/// a component that carries a handle — and a scene whose references were spelled
/// differently from the handles beside them would load into a world nobody
/// wrote. This compares the two renderings that appear in one file: the one the
/// splice produced on load, and the one `ron` produces for the same handle.
#[test]
fn a_spliced_handle_is_the_handles_own_serialization() {
    let registry = registry();
    let world = narvo_scene::from_file(&example_path(), &registry).expect("loads");

    let player = world.entity_ids()[2];
    let expected = ron::to_string(&player).expect("two integers serialize");

    let written = narvo_scene::to_string(&world, &registry).expect("writes");
    assert!(
        written.contains(&format!("target:{expected}")),
        "the reference resolved on load has to be the handle's own rendering \
         ({expected}), not a second spelling of it:\n{written}"
    );

    // ... and that rendering is what the loader accepts back, which is what
    // makes the writer's output a legal scene rather than a lookalike.
    let reloaded = narvo_scene::from_str(&written, &registry).expect("reloads");
    assert_same_world(&world, &reloaded, &registry, "handle spelling");
}

// ---- the error paths ---------------------------------------------------

/// Loads a scene expected to fail, and hands back the rendered message.
fn failure(text: &str) -> (SceneError, String) {
    let error = narvo_scene::from_str(text, &registry())
        .err()
        .unwrap_or_else(|| panic!("this scene was supposed to be rejected:\n{text}"));
    let rendered = error.to_string();
    (error, rendered)
}

#[test]
fn a_ron_syntax_error_carries_the_line_and_the_column() {
    // The comma after the first field is missing, on line 4.
    let (error, message) = failure(
        "Scene(entities: [\n\
         \x20   (components: {\n\
         \x20       \"transform\": (x: 1.0, y: 2.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0)\n\
         \x20       \"layer\": (depth: 0.0),\n\
         \x20   }),\n\
         ])",
    );

    match &error {
        SceneError::Syntax { location, .. } => {
            assert_eq!(
                location.line(),
                4,
                "the missing comma is on line 4: {message}"
            );
            assert!(location.column() > 0);
        }
        other => panic!("expected a syntax error, got {other:?}"),
    }
    assert!(
        message.starts_with("4:"),
        "the message has to open with the position: {message}"
    );
    assert!(message.contains("not valid RON"), "{message}");
}

#[test]
fn an_unknown_component_name_lists_the_ones_that_are_registered() {
    let (error, message) =
        failure("Scene(entities: [(name: \"a\", components: { \"positon\": (x: 1.0, y: 2.0) })])");

    match &error {
        SceneError::UnknownComponent {
            index,
            entity_name,
            component,
            known,
            ..
        } => {
            assert_eq!(*index, 0);
            assert_eq!(entity_name.as_deref(), Some("a"));
            assert_eq!(component, "positon");
            assert!(known.contains(&"transform".to_owned()));
        }
        other => panic!("expected an unknown component error, got {other:?}"),
    }
    assert!(message.contains("entity 0 (\"a\")"), "{message}");
    assert!(message.contains("\"positon\""), "{message}");
    assert!(message.contains("\"transform\""), "{message}");
}

#[test]
fn an_unknown_reference_names_the_field_and_lists_the_names_the_scene_defines() {
    let (error, message) = failure(
        "Scene(entities: [\n\
         \x20   (name: \"player\", components: { \"layer\": (depth: 1.0) }),\n\
         \x20   (components: { \"follow\": (smoothing: 1.0, x: 0.0, y: 0.0, lost: false) },\n\
         \x20    refs: { \"follow\": { \"target\": \"playre\" } }),\n\
         ])",
    );

    match &error {
        SceneError::UnknownReference {
            index,
            component,
            field,
            name,
            known,
            ..
        } => {
            assert_eq!(*index, 1);
            assert_eq!(component, "follow");
            assert_eq!(field, "target");
            assert_eq!(name, "playre");
            assert_eq!(&**known, ["player".to_owned()]);
        }
        other => panic!("expected an unknown reference error, got {other:?}"),
    }
    assert!(message.contains("follow.target"), "{message}");
    assert!(message.contains("\"playre\""), "{message}");
    assert!(message.contains("\"player\""), "{message}");
}

#[test]
fn a_duplicate_entity_name_names_both_entities() {
    let (error, message) = failure(
        "Scene(entities: [\n\
         \x20   (name: \"player\", components: {}),\n\
         \x20   (components: {}),\n\
         \x20   (name: \"player\", components: {}),\n\
         ])",
    );

    match &error {
        SceneError::DuplicateEntityName {
            name,
            first,
            second,
            ..
        } => {
            assert_eq!(name, "player");
            assert_eq!(*first, 0);
            assert_eq!(*second, 2);
        }
        other => panic!("expected a duplicate name error, got {other:?}"),
    }
    assert!(message.contains("entity 0 and entity 2"), "{message}");
}

#[test]
fn a_duplicate_component_on_one_entity_is_rejected_rather_than_silently_dropped() {
    // The hazard a plain map would have hidden: serde's map visitor inserts, so
    // the second body would have won and the first would have vanished.
    let (error, message) = failure(
        "Scene(entities: [(components: { \"layer\": (depth: 1.0), \"layer\": (depth: 2.0) })])",
    );

    match &error {
        SceneError::DuplicateComponent {
            index, component, ..
        } => {
            assert_eq!(*index, 0);
            assert_eq!(component, "layer");
        }
        other => panic!("expected a duplicate component error, got {other:?}"),
    }
    assert!(message.contains("twice"), "{message}");
}

#[test]
fn a_reference_for_a_component_the_entity_does_not_carry_is_rejected() {
    let (error, message) = failure(
        "Scene(entities: [\n\
         \x20   (name: \"player\", components: { \"layer\": (depth: 1.0) }),\n\
         \x20   (components: { \"camera\": (x: 0.0, y: 0.0, zoom: 1.0) },\n\
         \x20    refs: { \"follow\": { \"target\": \"player\" } }),\n\
         ])",
    );

    match &error {
        SceneError::ReferenceForAbsentComponent {
            index, component, ..
        } => {
            assert_eq!(*index, 1);
            assert_eq!(component, "follow");
        }
        other => panic!("expected an absent component error, got {other:?}"),
    }
    assert!(message.contains("does not carry"), "{message}");
}

#[test]
fn a_reference_into_a_body_that_is_not_a_struct_is_rejected_by_name() {
    let (error, message) = failure(
        "Scene(entities: [\n\
         \x20   (name: \"player\", components: { \"layer\": (depth: 1.0) }),\n\
         \x20   (components: { \"follow\": [1, 2] },\n\
         \x20    refs: { \"follow\": { \"target\": \"player\" } }),\n\
         ])",
    );

    match &error {
        SceneError::NotAStructBody {
            index, component, ..
        } => {
            assert_eq!(*index, 1);
            assert_eq!(component, "follow");
        }
        other => panic!("expected a not-a-struct-body error, got {other:?}"),
    }
    assert!(message.contains("`(…)` or `Name(…)`"), "{message}");
}

/// A body that is well-formed RON but not this component reports the component,
/// the entity and the position inside the body.
#[test]
fn a_body_that_is_not_the_component_names_the_component_and_keeps_the_position() {
    let (error, message) = failure("Scene(entities: [(components: { \"layer\": (depht: 1.0) })])");

    match &error {
        SceneError::Component {
            index, component, ..
        } => {
            assert_eq!(*index, 0);
            assert_eq!(component, "layer");
        }
        other => panic!("expected a component error, got {other:?}"),
    }
    assert!(
        message.contains("could not read its \"layer\""),
        "{message}"
    );
    // What survives is the deserializer's own complaint, position included. It
    // names the field that is *missing* rather than the typo that displaced it,
    // because `Layer` does not deny unknown fields - a fact about the component,
    // recorded here rather than assumed away.
    assert!(
        message.contains("1:") && message.contains("depth"),
        "the deserializer's own complaint, with its position inside the body, has to \
         survive: {message}"
    );
}

/// A field given both in the body and in `refs` is a conflict, not a silent
/// preference.
#[test]
fn a_reference_that_duplicates_a_field_in_the_body_is_rejected() {
    let (_error, message) = failure(
        "Scene(entities: [\n\
         \x20   (name: \"player\", components: { \"layer\": (depth: 1.0) }),\n\
         \x20   (components: { \"follow\": (target: (index: 0, generation: 1), smoothing: 1.0, \
         x: 0.0, y: 0.0, lost: false) },\n\
         \x20    refs: { \"follow\": { \"target\": \"player\" } }),\n\
         ])",
    );

    assert!(
        message.contains("duplicate") || message.contains("target"),
        "a field written twice has to be reported rather than resolved by position: {message}"
    );
}

/// A typo in the scene's own vocabulary is caught by name, with the alternatives.
#[test]
fn an_unknown_scene_field_lists_the_fields_that_exist() {
    let (_error, message) = failure("Scene(entities: [(nmae: \"player\", components: {})])");

    assert!(message.contains("nmae"), "{message}");
    assert!(
        message.contains("name") && message.contains("components") && message.contains("refs"),
        "the message should list what an entity may carry: {message}"
    );
}

// ---- the domain boundary -----------------------------------------------

/// A world with despawn history cannot be written as a scene, and says so.
///
/// This is the boundary ADR-0018 draws. A scene constitutes a *fresh* world, so
/// the shapes it can describe are the ones spawning alone can reach. A world
/// whose slots have gaps, or whose generations have moved past one, is a saved
/// state - the other consumer of this same round trip (`ProjektPlan.md` §6/M4,
/// "Szene und Save teilen den Kern") - and writing it as a scene would produce a
/// file that loads back into a differently shaped world while reporting success.
#[test]
fn a_world_with_despawn_history_is_refused_by_the_writer() {
    let registry = registry();
    let mut world = World::new();

    let first = world.spawn();
    let second = world.spawn();
    world
        .insert(second, Layer::at(1.0))
        .expect("the entity was just spawned");
    world.despawn(first).expect("the entity is alive");
    let recycled = world.spawn();
    assert_eq!(
        recycled.index(),
        first.index(),
        "this test needs the slot to be recycled to mean anything"
    );

    let error = narvo_scene::to_string(&world, &registry)
        .expect_err("a world with a recycled slot is not a scene");

    match &error {
        SceneError::NotSceneShaped { entity, .. } => {
            assert_eq!(entity.generation(), 2);
        }
        other => panic!("expected a domain error, got {other:?}"),
    }
    let message = error.to_string();
    assert!(message.contains("saved state"), "{message}");
}

/// The same boundary from the other side: a world whose slots merely have a gap.
#[test]
fn a_world_with_a_hole_in_its_slots_is_refused_by_the_writer() {
    let registry = registry();
    let mut world = World::new();

    let first = world.spawn();
    let _second = world.spawn();
    world.despawn(first).expect("the entity is alive");

    let error = narvo_scene::to_string(&world, &registry)
        .expect_err("slot 0 is empty, so this world is not a scene");

    assert!(matches!(error, SceneError::NotSceneShaped { .. }));
}

/// An entity carrying a component the registry does not know is refused, by the
/// same rule the canonical dump uses.
#[test]
fn the_writer_refuses_what_the_canonical_dump_refuses() {
    let mut registry = ComponentRegistry::new();
    registry
        .register_component::<Layer>("layer")
        .expect("a fresh registry accepts this component");

    let mut world = World::new();
    let entity = world.spawn();
    world
        .insert(entity, Transform::IDENTITY)
        .expect("the entity was just spawned");

    let error = narvo_scene::to_string(&world, &registry)
        .expect_err("a component outside the registry is outside the scene");

    assert!(matches!(error, SceneError::Ecs(_)), "got {error:?}");
    assert!(error.to_string().contains("not registered"), "{error}");
}

// ---- component-openness ------------------------------------------------

// The format's load-bearing structural claim (`ProjektPlan.md` §6/M4) is that it
// is *component-open*: driven by the registry, with no type list anywhere in the
// crate. The seven engine types cannot check that on their own, because they are
// all named-field structs and a format that only handled those would pass every
// test above. These five are shapes the engine does not currently have, and they
// are registered here and nowhere else.
//
// Being test types, they are deliberately outside ADR-0014's rule for *engine*
// components ("a registered component holds bare scalars"). That rule is about
// what the engine puts in the state hash; this is about what the scene format
// can carry, and the answer has to be "whatever the registry accepts" or M8+'s
// 3D types would be a rewrite rather than an extension.

/// A unit struct: no fields at all.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Marker;

/// A newtype: one unnamed field.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Health(i32);

/// A tuple struct with more than one field.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Span(i32, f32);

/// An enum, whose RON rendering is serde's choice rather than a scalar's.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum State {
    Idle,
    Running(u32),
    Waiting { until: i64 },
}

/// A struct nested inside a struct, with a sequence in it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Inventory {
    slots: Vec<Option<Health>>,
    tag: String,
}

fn open_registry() -> ComponentRegistry {
    let mut registry = registry();
    registry
        .register_component::<Marker>("marker")
        .expect("new");
    registry
        .register_component::<Health>("health")
        .expect("new");
    registry.register_component::<Span>("span").expect("new");
    registry.register_component::<State>("state").expect("new");
    registry
        .register_component::<Inventory>("inventory")
        .expect("new");
    registry
}

/// Component shapes the engine does not have survive the round trip untouched.
///
/// If this passes, "component-open" is a mechanism rather than an intention: the
/// five types below were added to a registry and to a scene, and not one line of
/// `narvo-scene` knows they exist.
#[test]
fn the_format_carries_component_shapes_the_engine_does_not_have() {
    let registry = open_registry();

    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();

    world.insert(a, Marker).expect("just spawned");
    world.insert(a, Health(-7)).expect("just spawned");
    world.insert(a, Span(3, -0.25)).expect("just spawned");
    world
        .insert(a, State::Waiting { until: -99 })
        .expect("just spawned");

    world.insert(b, State::Running(42)).expect("just spawned");
    world
        .insert(
            b,
            Inventory {
                slots: vec![Some(Health(1)), None, Some(Health(-2))],
                tag: "a string with a ) and a \" in it".to_owned(),
            },
        )
        .expect("just spawned");
    // ... alongside an engine component, so the two kinds share a file.
    world
        .insert(b, Transform::at(0.1, -0.1))
        .expect("just spawned");

    let text = narvo_scene::to_string(&world, &registry).expect("writes");
    let reloaded = narvo_scene::from_str(&text, &registry).expect("reads");

    assert_same_world(&world, &reloaded, &registry, "open component shapes");
    assert_eq!(
        *reloaded.get::<State>(b).expect("carried"),
        State::Running(42)
    );
    assert_eq!(*reloaded.get::<Marker>(a).expect("carried"), Marker);
}

/// A unit struct alone on an entity, which is the degenerate body `()`.
///
/// Worth its own case because it is the one shape where the splice's
/// empty-interior branch and RON's `ExpectedUnit` meet, and because a component
/// carrying no data at all is the ordinary way to tag an entity.
#[test]
fn a_component_with_no_data_survives_on_its_own() {
    let registry = open_registry();

    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Marker).expect("just spawned");

    let text = narvo_scene::to_string(&world, &registry).expect("writes");
    let reloaded = narvo_scene::from_str(&text, &registry).expect("reads");

    assert_same_world(&world, &reloaded, &registry, "a bare marker");
    assert!(reloaded.get::<Marker>(entity).is_ok());
}

/// The author's own bytes reach the component, comments and spacing included.
///
/// This is the claim ADR-0018's Decision 4 rests on, and it is asserted rather
/// than trusted: a loader that normalised a body would still pass every
/// round-trip test above, because it would normalise both sides the same way.
/// Here the body is written the way a person writes one - odd spacing, a block
/// comment in the middle, a trailing comma - and the *value* that comes out is
/// compared against one built in Rust.
#[test]
fn a_hand_written_body_reaches_the_component_verbatim() {
    let registry = open_registry();

    let world = narvo_scene::from_str(
        "Scene(entities: [(components: {\n\
         \x20   \"transform\": (\n\
         \x20       x:  0.1,   /* the tightest float in the probe set */\n\
         \x20       y: -0.0,   // and a negative zero, whose sign a formatter can drop\n\
         \x20       rotation: 3.1415927,\n\
         \x20       scale_x: 1.0,\n\
         \x20       scale_y: 1.0,\n\
         \x20   ),\n\
         })])",
        &registry,
    )
    .expect("comments and spacing are RON, so they load");

    let entity = world.entity_ids()[0];
    let transform = *world.get::<Transform>(entity).expect("carried");

    // On bit patterns: `==` calls 0.0 and -0.0 equal, which is exactly the loss
    // this test exists to notice.
    assert_eq!(transform.x.to_bits(), 0.1_f32.to_bits());
    assert_eq!(
        transform.y.to_bits(),
        (-0.0_f32).to_bits(),
        "the negative zero lost its sign somewhere between the file and the component"
    );
    assert_eq!(
        transform.rotation.to_bits(),
        core::f32::consts::PI.to_bits()
    );
}

// ---- the round trip ----------------------------------------------------

/// Floats a scene-constructible world may hold.
///
/// The domain is every `f32` **except a `NaN` carrying a payload**, and that
/// exclusion is a measurement rather than a precaution. RON renders every `NaN`
/// as `NaN` or `-NaN`, so a payload does not survive `to_string` →
/// `from_str`: `0x7fc00001` comes back as `0x7fc00000`. The two canonical quiet
/// `NaN`s do survive and are generated below. Infinities survive exactly and are
/// generated too.
///
/// That is a property of the canonical form this project already had — the state
/// hash is taken over the same rendering — and not something the scene format
/// introduces. The report records it as a finding.
fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        // The values that stress a text round trip, from the probe set the
        // component tests in `narvo-ecs` already use.
        9 => prop::sample::select(vec![
            0.1_f32,
            f32::from_bits(0.1_f32.to_bits() + 1),
            -1.0 / 3.0,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::MAX,
            -f32::MAX,
            -0.0,
            0.0,
            core::f32::consts::PI,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NAN,
            -f32::NAN,
        ]),
        // ... and the whole space besides, minus the payload-carrying `NaN`s.
        11 => any::<u32>()
            .prop_map(f32::from_bits)
            .prop_filter("a NaN payload is outside the format's domain", |value| {
                !value.is_nan()
            }),
    ]
}

/// One entity of a generated world: which components it carries and what they
/// hold.
///
/// `Follow`'s target is not here. It cannot be: a target is a handle, and no
/// handle exists until the world is built, so the target is drawn as an *index*
/// into the entity list and resolved once the spawns have happened. That is the
/// same two-pass shape the loader has, for the same reason.
#[derive(Debug, Clone)]
struct GeneratedEntity {
    transform: Option<Transform>,
    layer: Option<Layer>,
    sampling: Option<Sampling>,
    camera: Option<Camera>,
    follow: Option<(usize, f32, f32, f32, bool)>,
    shake: Option<Shake>,
    rng: Option<(u64, u64)>,
    sprite: Option<String>,
}

/// Region names the property generates.
///
/// **Not `any::<String>()`, and the restriction is a decision rather than a
/// convenience.** A region name is a file stem, so the alphabet that can reach
/// this component through the loader M4.8 built is bounded by what a file system
/// accepts. What proptest is asked for here is therefore the *realistic* range
/// plus the two characters that make RON's escaping do work — a quote and a
/// backslash — because those are what a round trip could actually lose.
///
/// The unrestricted case is not skipped, it is moved: `narvo_ecs::sprite`'s own
/// `a_region_name_survives_the_ron_round_trip_with_anything_in_it` puts newlines,
/// unicode and the empty string through the same serialization. Splitting them
/// keeps this property fast and puts the adversarial names where a failure names
/// the component rather than a generated scene.
fn any_region_name() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-z0-9_\\\\\"-]{1,12}").expect("a valid regex")
}

fn generated_entity() -> impl Strategy<Value = GeneratedEntity> {
    (
        proptest::option::of((any_f32(), any_f32(), any_f32(), any_f32(), any_f32())),
        proptest::option::of(any_f32()),
        proptest::option::of(any::<u8>()),
        proptest::option::of((any_f32(), any_f32(), any_f32())),
        proptest::option::of((
            any::<usize>(),
            any_f32(),
            any_f32(),
            any_f32(),
            any::<bool>(),
        )),
        proptest::option::of((
            any_f32(),
            any_f32(),
            any_f32(),
            any_f32(),
            any_f32(),
            any_f32(),
            any_f32(),
        )),
        proptest::option::of((any::<u64>(), any::<u64>())),
        proptest::option::of(any_region_name()),
    )
        .prop_map(
            |(transform, layer, sampling, camera, follow, shake, rng, sprite)| GeneratedEntity {
                transform: transform.map(|(x, y, rotation, scale_x, scale_y)| Transform {
                    x,
                    y,
                    rotation,
                    scale_x,
                    scale_y,
                }),
                layer: layer.map(Layer::at),
                sampling: sampling.map(|filter| Sampling { filter }),
                camera: camera.map(|(x, y, zoom)| Camera::new(x, y, zoom)),
                follow,
                shake: shake.map(
                    |(amplitude, frequency, decay, phase, cutoff, base_x, base_y)| Shake {
                        amplitude,
                        frequency,
                        decay,
                        phase,
                        cutoff,
                        base_x,
                        base_y,
                    },
                ),
                rng,
                sprite,
            },
        )
}

/// Builds a world of the shape a scene can describe: freshly spawned, ascending
/// slots, every generation one, every `Follow` pointing at an entity that is
/// there.
fn build(entities: &[GeneratedEntity]) -> World {
    let mut world = World::new();
    let ids: Vec<EntityId> = entities.iter().map(|_| world.spawn()).collect();

    for (entity, id) in entities.iter().zip(&ids) {
        if let Some(transform) = entity.transform {
            world.insert(*id, transform).expect("just spawned");
        }
        if let Some(layer) = entity.layer {
            world.insert(*id, layer).expect("just spawned");
        }
        if let Some(sampling) = entity.sampling {
            world.insert(*id, sampling).expect("just spawned");
        }
        if let Some(camera) = entity.camera {
            world.insert(*id, camera).expect("just spawned");
        }
        if let Some((target, smoothing, x, y, lost)) = entity.follow {
            // Wrapped into range, so a generated index always names a live
            // entity: a dangling target is not something a scene can express.
            let target = ids[target % ids.len()];
            world
                .insert(
                    *id,
                    Follow {
                        target,
                        smoothing,
                        x,
                        y,
                        lost,
                    },
                )
                .expect("just spawned");
        }
        if let Some(shake) = entity.shake {
            world.insert(*id, shake).expect("just spawned");
        }
        if let Some((seed, stream)) = entity.rng {
            world
                .insert(*id, Rng::with_stream(seed, stream))
                .expect("just spawned");
        }
        if let Some(region) = &entity.sprite {
            world
                .insert(*id, Sprite::new(region.clone()))
                .expect("just spawned");
        }
    }

    world
}

proptest! {
    // 512 cases rather than proptest's default 256: the strategy has seven
    // independent optional components per entity, so the interesting shapes -
    // an entity carrying all seven, a follow pointing at itself - need a few
    // hundred draws before they are likely rather than possible. The run costs
    // well under a second, so the budget in §8.1 is untouched.
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// **The closing criterion of M4, as a property**: a world a scene can
    /// describe survives being written and read back.
    ///
    /// Compared through `canonical_dump`, because that is the one definition of
    /// "the same world" this project has and the one the state hash is taken
    /// over. A failure prints `first_difference`, so a shrunk counterexample
    /// arrives as *which entity, which component, which side said what* rather
    /// than as two dumps to diff by eye.
    #[test]
    fn a_scene_shaped_world_survives_world_to_text_to_world(
        entities in prop::collection::vec(generated_entity(), 1..6)
    ) {
        let registry = registry();
        let world = build(&entities);

        let text = narvo_scene::to_string(&world, &registry)
            .map_err(|error| TestCaseError::fail(format!("writing failed: {error}")))?;
        let reloaded = narvo_scene::from_str(&text, &registry)
            .map_err(|error| TestCaseError::fail(format!("reading failed: {error}\n{text}")))?;

        let before = canonical_dump(&world, &registry).expect("registered");
        let after = canonical_dump(&reloaded, &registry).expect("registered");

        if let Some(difference) = first_difference(&before, &after) {
            return Err(TestCaseError::fail(format!(
                "the world did not survive the round trip.\n{difference}\n\nscene:\n{text}"
            )));
        }
    }
}
