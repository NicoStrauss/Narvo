//! What the registry wrote is what comes back off the wire.
//!
//! This is the machine-checkable half of ADR-0030. The claim it holds is not
//! "the protocol is lossless" — nothing is — but the narrower and checkable one:
//! **the protocol carries exactly what the canonical dump carries**, because the
//! bytes are the same bytes. Where the registry loses something, so does this,
//! and the tests say which case that is.
//!
//! `narvo-ecs` is a dev-dependency here and never a normal one. This crate is
//! a leaf beside `narvo-input`, `narvo-audio` and `narvo-physics2d`, and a
//! production edge to the ECS would put `hecs` under every future MCP client.

use std::num::NonZeroU32;

use narvo_ecs::{ComponentRegistry, EntityId, World};
use narvo_ipc::{ComponentValue, EntityName, Response};
use serde::{Deserialize, Serialize};

/// A component that is one float, so a round trip is about the float rather
/// than about a struct shape.
///
/// A local type rather than the engine's `Transform`: the property under test is
/// the path, not any particular component, and pinning it to engine vocabulary
/// would make this test move whenever that vocabulary does.
#[derive(Debug, Serialize, Deserialize)]
struct Probe {
    x: f32,
}

/// A second component, so the canonical order has something to order.
#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    n: u64,
}

/// The values that stress a float path, by bit pattern.
///
/// The first two are one mantissa bit apart — the pair a `format!("{:.6}")`
/// rendering would merge, which is the defect class M5b.3a caught. The last
/// three are the three `serde_json` has no representation for.
fn stress() -> Vec<(&'static str, f32)> {
    vec![
        ("0.1", 0.1_f32),
        ("one bit above 0.1", f32::from_bits(0.1_f32.to_bits() + 1)),
        ("+0.0", 0.0_f32),
        ("-0.0", -0.0_f32),
        ("smallest subnormal", f32::from_bits(1)),
        ("MIN_POSITIVE", f32::MIN_POSITIVE),
        ("MAX", f32::MAX),
        ("-MAX", -f32::MAX),
        ("infinity", f32::INFINITY),
        ("negative infinity", f32::NEG_INFINITY),
        ("NaN", f32::NAN),
    ]
}

/// A registry holding the two test components.
fn registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    registry
        .register_component::<Probe>("probe")
        .expect("a fresh registry accepts it");
    registry
        .register_component::<Marker>("marker")
        .expect("a fresh registry accepts it");
    registry
}

/// The conversion M6.3 will have to write, done here by hand.
///
/// One line, and it is deliberately a conversion rather than a shared type: an
/// `EntityId` came from a world, an `EntityName` came from outside, and only one
/// of the two has been checked against anything.
fn name_of(entity: EntityId) -> EntityName {
    EntityName::new(
        entity.index(),
        NonZeroU32::new(entity.generation()).expect("hecs generations count from one"),
    )
}

/// Every stress value crosses the protocol and reads back with the same bits.
///
/// The whole path: insert, serialize through the registry's type-erased path,
/// put the text in a response, render to JSON, parse the JSON, take the text
/// out, read it back through the registry onto a fresh entity, compare
/// `to_bits`. `==` would call `0.0` and `-0.0` equal and two `NaN`s unequal, so
/// it cannot see what a state hash sees; `to_bits` can (ADR-0014).
#[test]
fn every_stress_value_crosses_the_protocol_with_its_bits_intact() {
    let registry = registry();

    for (label, value) in stress() {
        let mut world = World::new();
        let written = world.spawn();
        world.insert(written, Probe { x: value }).expect("alive");

        let text = registry
            .serialize_component("probe", &world, written)
            .expect("alive and registered")
            .expect("the entity carries one");

        let sent = Response::GetComponent {
            entity: name_of(written),
            component: "probe".to_owned(),
            value: Some(text.clone()),
            ticks_run: 0,
        };
        let crossed = Response::from_json(&sent.to_json()).expect("this crate just wrote it");
        assert_eq!(crossed, sent, "{label} did not survive the round trip");

        let carried = match &crossed {
            Response::GetComponent { value: Some(v), .. } => v.clone(),
            other => panic!("{label}: expected a component answer, got {other:?}"),
        };
        assert_eq!(
            carried, text,
            "{label}: the protocol changed the registry's bytes"
        );

        let read_back = world.spawn();
        registry
            .deserialize_component("probe", &mut world, read_back, &carried)
            .unwrap_or_else(|error| {
                panic!("{label}: the registry could not read it back: {error}")
            });

        let round = world.get::<Probe>(read_back).expect("just inserted").x;
        assert_eq!(
            round.to_bits(),
            value.to_bits(),
            "{label}: {:#010x} came back as {:#010x} (text was {text})",
            value.to_bits(),
            round.to_bits()
        );
    }
}

/// The three values `serde_json` cannot spell arrive as text and stay text.
///
/// This is the concrete form of ADR-0030's reason. A JSON *number* path writes
/// `null` for all three and then refuses to read it back; carrying the
/// registry's RON inside a JSON string means `serde_json` never sees a float.
#[test]
fn the_values_json_has_no_number_for_cross_as_the_registry_spelled_them() {
    let registry = registry();
    let expected = [
        (f32::INFINITY, "(x:inf)"),
        (f32::NEG_INFINITY, "(x:-inf)"),
        (f32::NAN, "(x:NaN)"),
    ];

    for (value, spelling) in expected {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Probe { x: value }).expect("alive");

        let text = registry
            .serialize_component("probe", &world, entity)
            .expect("alive and registered")
            .expect("the entity carries one");
        assert_eq!(text, spelling);

        let json = Response::GetComponent {
            entity: name_of(entity),
            component: "probe".to_owned(),
            value: Some(text.clone()),
            ticks_run: 0,
        }
        .to_json();

        // Verbatim in the JSON, not merely recoverable from it. RON's rendering
        // of a component contains no character JSON escapes, so the bytes are
        // findable as they stand.
        assert!(
            json.contains(spelling),
            "{spelling} is not in the JSON as written: {json}"
        );
        assert!(
            !json.contains("null"),
            "a non-finite value reached a JSON number after all: {json}"
        );
    }
}

/// What the rejected design would have done, held as a test rather than as
/// prose.
///
/// ADR-0030 rests on this measurement, so it is checked in every run rather than
/// quoted from a report. It is the third kind of literal ADR-0008 allows — a
/// content anchor over something a dependency generates, where the dependency
/// moving the value is the finding. If `serde_json` ever gains a representation
/// for these three, that ADR's revision condition has fired.
#[test]
fn a_json_number_path_would_have_lost_all_three_of_them() {
    for value in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        let as_number = serde_json::to_string(&value).expect("serde_json writes something");
        assert_eq!(as_number, "null", "{value} no longer becomes null");
        assert!(
            serde_json::from_str::<f32>(&as_number).is_err(),
            "null now reads back as an f32"
        );
    }

    // And the finite values it does carry, it carries exactly — which is the
    // rejected design's best argument and is recorded as such.
    for (label, value) in stress() {
        if !value.is_finite() {
            continue;
        }
        let text = serde_json::to_string(&value).expect("a finite f32 always serializes");
        let back: f32 = serde_json::from_str(&text).expect("what it wrote, it reads");
        assert_eq!(back.to_bits(), value.to_bits(), "{label}");
    }
}

/// A `NaN` payload is lost, and it is the registry that loses it.
///
/// Stated as a test rather than as a caveat in a report, because the interesting
/// part is *where* the loss is: `ron` renders every `NaN` as `NaN`, so a payload
/// is already gone before the protocol sees anything. The canonical dump and the
/// state hash have exactly this limit today, and the protocol inherits it rather
/// than adding to it.
#[test]
fn a_nan_payload_is_lost_by_the_registry_before_the_protocol_sees_it() {
    let registry = registry();
    let mut world = World::new();

    let canonical = world.spawn();
    world
        .insert(canonical, Probe { x: f32::NAN })
        .expect("alive");
    let with_payload = world.spawn();
    world
        .insert(
            with_payload,
            Probe {
                x: f32::from_bits(0x7fc0_002a),
            },
        )
        .expect("alive");

    let a = registry
        .serialize_component("probe", &world, canonical)
        .expect("alive and registered")
        .expect("carries one");
    let b = registry
        .serialize_component("probe", &world, with_payload)
        .expect("alive and registered")
        .expect("carries one");

    assert_eq!(a, b, "two different NaNs no longer render the same");
    assert_eq!(a, "(x:NaN)");

    // The protocol carries whichever of them it is given, unchanged. The loss
    // is upstream of here and this is the assertion that says so.
    let sent = Response::GetComponent {
        entity: name_of(with_payload),
        component: "probe".to_owned(),
        value: Some(b.clone()),
        ticks_run: 0,
    };
    assert_eq!(
        Response::from_json(&sent.to_json()).expect("just written"),
        sent
    );
}

/// A full-entity answer carries what the canonical dump carries, in its order.
///
/// Built the way a handler will build one — walk the registry, keep what comes
/// back — and then checked against `canonical_dump` for the same entity. That is
/// what makes "canonical order" a property of this protocol rather than a hope
/// about it.
#[test]
fn an_entity_answer_holds_what_the_canonical_dump_holds_in_the_same_order() {
    let registry = registry();
    let mut world = World::new();

    let entity = world.spawn();
    world.insert(entity, Probe { x: 0.5 }).expect("alive");
    world.insert(entity, Marker { n: 7 }).expect("alive");
    let bare = world.spawn();

    let answer = |entity| Response::GetEntity {
        entity: name_of(entity),
        components: registry
            .iter()
            .filter_map(|info| {
                info.serialize(&world, entity)
                    .expect("the entity is alive")
                    .map(|text| ComponentValue::new(info.name(), text))
            })
            .collect(),
        ticks_run: 0,
    };

    let crossed = Response::from_json(&answer(entity).to_json()).expect("just written");
    let components = match crossed {
        Response::GetEntity { components, .. } => components,
        other => panic!("expected the entity answer, got {other:?}"),
    };

    // The registry iterates by stable name, so `marker` comes before `probe`
    // whatever order they were inserted or registered in.
    let rendered: Vec<String> = components
        .iter()
        .map(|c| format!("  {} {}", c.name, c.value))
        .collect();

    let dump = narvo_ecs::canonical_dump(&world, &registry).expect("everything is registered");
    let in_the_dump: Vec<String> = dump
        .lines()
        .skip_while(|line| *line != "entity 0v1")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .map(str::to_owned)
        .collect();

    assert_eq!(rendered, in_the_dump);
    assert_eq!(rendered, vec!["  marker (n:7)", "  probe (x:0.5)"]);

    // An entity carrying nothing answers with an empty list rather than with an
    // error, matching `ComponentInfo::serialize`'s `Ok(None)` for an absence.
    match Response::from_json(&answer(bare).to_json()).expect("just written") {
        Response::GetEntity { components, .. } => assert!(components.is_empty()),
        other => panic!("expected the entity answer, got {other:?}"),
    }
}

/// An entity list answers with the names the world's own order produces.
#[test]
fn an_entity_list_answer_carries_the_worlds_canonical_order() {
    let mut world = World::new();
    let first = world.spawn();
    let second = world.spawn();
    world.despawn(first).expect("alive");
    let recycled = world.spawn();

    let response = Response::ListEntities {
        entities: world.entity_ids().into_iter().map(name_of).collect(),
        ticks_run: 0,
    };

    let crossed = Response::from_json(&response.to_json()).expect("just written");
    assert_eq!(crossed, response);

    // Slot order, not generation order: the recycled slot 0 comes before slot 1
    // even though it was handed out last.
    assert_eq!(name_of(recycled).index(), name_of(first).index());
    assert_eq!(name_of(recycled).generation(), 2);
    assert_eq!(
        response.to_json(),
        r#"{"list_entities":{"entities":["0v2","1v1"],"ticks_run":0}}"#
    );
    assert_eq!(name_of(second).to_string(), "1v1");
}
