//! Prefabs: the equivalence that defines them, and every way one can go wrong.
//!
//! # The oracle
//!
//! ADR-0021 says expansion is a **load-time transformation and nothing else** —
//! a template is scene syntax like a name, and the world does not know one was
//! involved. That is not a sentence a reader has to take on trust: it is
//! checkable, by writing the same scene twice, once with a template and once
//! flat, and comparing the two worlds through `canonical_dump`. Every test in
//! the first section is a form of that comparison, and the property test makes
//! it a statement about generated combinations rather than about one example.

use std::path::PathBuf;

use narvo_ecs::{
    Camera, ComponentRegistry, Follow, Layer, Rng, Sampling, Shake, Transform, World,
    canonical_dump, first_difference,
};
use narvo_scene::{SceneError, Severity};
use proptest::prelude::*;

/// Every component type the engine registers. See `scene.rs`.
fn registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    for result in [
        registry.register_component::<Transform>("transform"),
        registry.register_component::<Layer>("layer"),
        registry.register_component::<Sampling>("sampling"),
        registry.register_component::<Camera>("camera"),
        registry.register_component::<Follow>("follow"),
        registry.register_component::<Shake>("shake"),
        registry.register_component::<Rng>("rng"),
    ] {
        result.expect("a fresh registry accepts each of these once");
    }
    registry
}

fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("prefabs.ron")
}

/// Loads two scenes and says where their worlds stop being one world.
///
/// Through `canonical_dump` and `first_difference`, which is the one definition
/// of "the same world" this project has and the one instrument for locating a
/// difference in it.
fn assert_same_world(left: &str, right: &str, case: &str) {
    let registry = registry();
    let a = narvo_scene::from_str(left, &registry)
        .unwrap_or_else(|error| panic!("{case}: the prefab scene does not load: {error}"));
    let b = narvo_scene::from_str(right, &registry)
        .unwrap_or_else(|error| panic!("{case}: the flat scene does not load: {error}"));

    let dump_a = canonical_dump(&a, &registry).expect("registered");
    let dump_b = canonical_dump(&b, &registry).expect("registered");

    if let Some(difference) = first_difference(&dump_a, &dump_b) {
        panic!(
            "{case}: the two worlds differ.\n{difference}\n\nprefab:\n{dump_a}\nflat:\n{dump_b}"
        );
    }
}

/// The committed example, and the flat scene it has to be equal to.
fn flat_twin() -> &'static str {
    concat!(
        "Scene(entities: [\n",
        "    (name: \"left\", components: {\n",
        "        \"transform\": (x: -8.0, y: 0.5, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n",
        "        \"layer\": (depth: 0.0),\n",
        "        \"sampling\": (filter: 1),\n",
        "    }),\n",
        "    (components: {\n",
        "        \"transform\": (x: 8.0, y: -0.5, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n",
        "        \"layer\": (depth: 2.0),\n",
        "    }),\n",
        "    (components: {\n",
        "        \"camera\": (x: 0.0, y: 0.0, zoom: 1.0),\n",
        "        \"follow\": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false),\n",
        "    }, refs: { \"follow\": { \"target\": \"left\" } }),\n",
        "])\n"
    )
}

// ---- the oracle ----------------------------------------------------------

/// **The property that defines prefabs**: the committed example and its
/// hand-written flat twin load to one world.
#[test]
fn the_example_and_its_flat_twin_load_to_one_world() {
    let prefabbed = std::fs::read_to_string(example_path()).expect("the example is readable");

    assert_same_world(&prefabbed, flat_twin(), "the committed example");
}

/// The oracle's own flank: a flat twin that differs by one field value is seen,
/// and `first_difference` names the place.
///
/// Without this, `assert_same_world` could be comparing two things that are
/// equal for a reason other than the one under test — or comparing nothing at
/// all.
#[test]
fn a_flat_twin_that_differs_in_one_field_is_caught_and_located() {
    let registry = registry();
    let prefabbed = std::fs::read_to_string(example_path()).expect("readable");
    // One value changed: the second sprite's depth, 2.0 instead of the template's.
    let wrong = flat_twin().replace("\"layer\": (depth: 2.0)", "\"layer\": (depth: 3.0)");
    assert_ne!(wrong, flat_twin(), "the substitution has to have happened");

    let a = narvo_scene::from_str(&prefabbed, &registry).expect("loads");
    let b = narvo_scene::from_str(&wrong, &registry).expect("loads");

    let difference = first_difference(
        &canonical_dump(&a, &registry).expect("registered"),
        &canonical_dump(&b, &registry).expect("registered"),
    )
    .expect("one field differs, so the dumps must differ");

    assert_eq!(difference.component(), Some("layer"));
    assert_eq!(difference.entity().map(|entity| entity.index()), Some(1));
    assert!(difference.left().is_some_and(|line| line.contains("2.0")));
    assert!(difference.right().is_some_and(|line| line.contains("3.0")));
}

/// Filling alone, with nothing else going on.
#[test]
fn an_instance_that_only_fills_holes_equals_the_flat_entity() {
    assert_same_world(
        concat!(
            "Scene(prefabs: { \"p\": (components: {\n",
            "    \"transform\": (rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n",
            "}) }, entities: [\n",
            "    (from: \"p\", fill: { \"transform\": { \"x\": 1.5, \"y\": -2.5 } }),\n",
            "])"
        ),
        concat!(
            "Scene(entities: [(components: {\n",
            "    \"transform\": (x: 1.5, y: -2.5, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n",
            "})])"
        ),
        "fill only",
    );
}

/// Adding a component the template does not have.
#[test]
fn an_instance_can_add_a_component() {
    assert_same_world(
        concat!(
            "Scene(prefabs: { \"p\": (components: { \"layer\": (depth: 1.0) }) }, entities: [\n",
            "    (from: \"p\", components: { \"sampling\": (filter: 1) }),\n",
            "])"
        ),
        "Scene(entities: [(components: { \"layer\": (depth: 1.0), \"sampling\": (filter: 1) })])",
        "add",
    );
}

/// Replacing a template component: the instance's body wins verbatim.
#[test]
fn an_instance_can_replace_a_whole_component() {
    assert_same_world(
        concat!(
            "Scene(prefabs: { \"p\": (components: { \"layer\": (depth: 1.0) }) }, entities: [\n",
            "    (from: \"p\", components: { \"layer\": (depth: 9.0) }),\n",
            "])"
        ),
        "Scene(entities: [(components: { \"layer\": (depth: 9.0) })])",
        "replace",
    );
}

/// Removing a template component, said out loud.
#[test]
fn an_instance_can_remove_a_component_explicitly() {
    assert_same_world(
        concat!(
            "Scene(prefabs: { \"p\": (components: {\n",
            "    \"layer\": (depth: 1.0), \"sampling\": (filter: 1),\n",
            "}) }, entities: [ (from: \"p\", without: [\"sampling\"]) ])"
        ),
        "Scene(entities: [(components: { \"layer\": (depth: 1.0) })])",
        "remove",
    );
}

/// Omission is inheritance, which is the other half of removal being explicit.
#[test]
fn saying_nothing_inherits_rather_than_removes() {
    assert_same_world(
        concat!(
            "Scene(prefabs: { \"p\": (components: {\n",
            "    \"layer\": (depth: 1.0), \"sampling\": (filter: 1),\n",
            "}) }, entities: [ (from: \"p\") ])"
        ),
        "Scene(entities: [(components: { \"layer\": (depth: 1.0), \"sampling\": (filter: 1) })])",
        "inherit",
    );
}

/// A template may declare a reference; it resolves in the instance's scene.
///
/// The template cannot resolve it itself — it is not in the world, and the
/// entity it names need not exist until an instance is spawned. Two instances of
/// one template therefore point at whatever *their* scene calls `anchor`.
#[test]
fn a_template_reference_resolves_in_the_instances_scene() {
    assert_same_world(
        concat!(
            "Scene(prefabs: { \"eye\": (\n",
            "    components: { \"camera\": (x: 0.0, y: 0.0, zoom: 1.0),\n",
            "                  \"follow\": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false) },\n",
            "    refs: { \"follow\": { \"target\": \"anchor\" } },\n",
            ") }, entities: [\n",
            "    (name: \"anchor\", components: { \"layer\": (depth: 0.0) }),\n",
            "    (from: \"eye\"),\n",
            "])"
        ),
        concat!(
            "Scene(entities: [\n",
            "    (name: \"anchor\", components: { \"layer\": (depth: 0.0) }),\n",
            "    (components: { \"camera\": (x: 0.0, y: 0.0, zoom: 1.0),\n",
            "                   \"follow\": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false) },\n",
            "     refs: { \"follow\": { \"target\": \"anchor\" } }),\n",
            "])"
        ),
        "template reference",
    );
}

// ---- order ---------------------------------------------------------------

/// **File order is spawn order, instances and flat entities alike.**
///
/// ADR-0018's rule, extended to the case M4.5 introduces: an instance is one
/// list entry, so mixing the two kinds must not disturb the slots. Interleaved
/// on purpose, because a bug that appended expansions would still pass a test
/// where all the instances came last.
#[test]
fn instances_and_flat_entities_share_one_ascending_slot_order() {
    let registry = registry();
    let world = narvo_scene::from_str(
        concat!(
            "Scene(prefabs: { \"p\": (components: { \"layer\": (depth: 7.0) }) }, entities: [\n",
            "    (components: { \"layer\": (depth: 0.0) }),\n",
            "    (from: \"p\"),\n",
            "    (components: { \"layer\": (depth: 2.0) }),\n",
            "    (from: \"p\", components: { \"layer\": (depth: 3.0) }),\n",
            "    (components: { \"layer\": (depth: 4.0) }),\n",
            "])"
        ),
        &registry,
    )
    .expect("loads");

    let ids = world.entity_ids();
    assert_eq!(ids.len(), 5);
    for (slot, id) in ids.iter().enumerate() {
        assert_eq!(id.index(), u32::try_from(slot).expect("five entities"));
        assert_eq!(id.generation(), 1);
    }

    // The depths in slot order say which entry became which entity.
    let depths: Vec<f32> = ids
        .iter()
        .map(|id| world.get::<Layer>(*id).expect("each carries one").depth)
        .collect();
    assert_eq!(depths, vec![0.0, 7.0, 2.0, 3.0, 4.0]);
}

// ---- the writer stays flat -----------------------------------------------

/// The writer emits no prefab syntax, and its output reloads to the same world.
///
/// The consequence of expansion being load-time: a `World` does not know a
/// template was involved, so `to_string` cannot write one back — the same
/// reasoning ADR-0018 gives for names, and the same test shape.
#[test]
fn the_writer_emits_no_prefab_syntax_and_reloads_to_the_same_world() {
    let registry = registry();
    let loaded = narvo_scene::from_file(&example_path(), &registry).expect("loads");

    let written = narvo_scene::to_string(&loaded, &registry).expect("writes");

    for syntax in ["prefabs", "from:", "fill:", "without:"] {
        assert!(
            !written.contains(syntax),
            "the writer emitted {syntax:?}:\n{written}"
        );
    }

    let reloaded = narvo_scene::from_str(&written, &registry).expect("reloads");
    let difference = first_difference(
        &canonical_dump(&loaded, &registry).expect("registered"),
        &canonical_dump(&reloaded, &registry).expect("registered"),
    );
    assert!(difference.is_none(), "{difference:?}");
}

// ---- the error paths -----------------------------------------------------

fn failure(text: &str) -> (SceneError, String) {
    let error = narvo_scene::from_str(text, &registry())
        .err()
        .unwrap_or_else(|| panic!("this scene was supposed to be rejected:\n{text}"));
    let rendered = error.to_string();
    (error, rendered)
}

/// **The collision.** The template sets a field and the instance fills it.
#[test]
fn filling_a_field_the_template_already_sets_is_a_hard_error() {
    let (error, message) = failure(concat!(
        "Scene(prefabs: { \"p\": (components: {\n",
        "    \"transform\": (x: 1.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n",
        "}) }, entities: [\n",
        "    (from: \"p\", fill: { \"transform\": { \"x\": 5.0 } }),\n",
        "])"
    ));

    match &error {
        SceneError::FieldCollision {
            index,
            prefab,
            component,
            field,
            ..
        } => {
            assert_eq!(*index, 0);
            assert_eq!(prefab, "p");
            assert_eq!(component, "transform");
            assert_eq!(field, "x");
        }
        other => panic!("expected a field collision, got {other:?}"),
    }

    // Both origins named, and no precedence offered.
    assert!(message.contains("transform.x"), "{message}");
    assert!(message.contains("template \"p\""), "{message}");
    assert!(message.contains("never"), "{message}");
    assert!(error.location().is_some(), "{message}");
}

/// A hole nobody filled: what happens, and where it points.
///
/// The task asked what this produces rather than assuming — it is the
/// component's own deserializer complaining about a missing field, wrapped with
/// the entity and the component, and pointed at the **template's** body, which
/// is where the hole is.
#[test]
fn a_hole_that_nobody_fills_is_the_components_own_missing_field() {
    let (error, message) = failure(concat!(
        "Scene(prefabs: { \"p\": (components: {\n",
        "    \"transform\": (rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n",
        "}) }, entities: [ (from: \"p\") ])"
    ));

    assert!(matches!(error, SceneError::Component { .. }), "{error:?}");
    assert!(
        message.contains("could not read its \"transform\""),
        "{message}"
    );
    assert!(message.contains("Unexpected missing field"), "{message}");
    // Line 2 is the template's body — where the hole is, not where the instance
    // is. That is the honest place: the instance says nothing about `x`.
    assert_eq!(
        error.location().map(narvo_scene::Location::line),
        Some(2),
        "{message}"
    );
}

#[test]
fn an_unknown_prefab_lists_the_ones_the_scene_defines() {
    let (error, message) = failure(concat!(
        "Scene(prefabs: { \"sprite\": (components: { \"layer\": (depth: 0.0) }) }, entities: [\n",
        "    (from: \"spirte\"),\n",
        "])"
    ));

    match &error {
        SceneError::UnknownPrefab {
            index, name, known, ..
        } => {
            assert_eq!(*index, 0);
            assert_eq!(name, "spirte");
            assert_eq!(known, &vec!["sprite".to_owned()]);
        }
        other => panic!("expected an unknown prefab, got {other:?}"),
    }
    assert!(message.contains("\"sprite\""), "{message}");
    assert!(error.location().is_some(), "{message}");
}

#[test]
fn two_templates_may_not_share_a_name() {
    let (error, message) = failure(concat!(
        "Scene(prefabs: {\n",
        "    \"p\": (components: { \"layer\": (depth: 0.0) }),\n",
        "    \"p\": (components: { \"layer\": (depth: 1.0) }),\n",
        "}, entities: [])"
    ));

    match &error {
        SceneError::DuplicatePrefabName {
            name,
            first,
            second,
            ..
        } => {
            assert_eq!(name, "p");
            assert_eq!((*first, *second), (0, 1));
        }
        other => panic!("expected a duplicate prefab name, got {other:?}"),
    }
    assert!(message.contains("entry 0 and entry 1"), "{message}");
}

#[test]
fn removing_a_component_the_template_does_not_have_is_refused() {
    let (error, message) = failure(concat!(
        "Scene(prefabs: { \"p\": (components: { \"layer\": (depth: 0.0) }) }, entities: [\n",
        "    (from: \"p\", without: [\"sampling\"]),\n",
        "])"
    ));

    match &error {
        SceneError::RemovingAbsentComponent {
            index,
            component,
            present,
            ..
        } => {
            assert_eq!(*index, 0);
            assert_eq!(component, "sampling");
            assert_eq!(present, &vec!["layer".to_owned()]);
        }
        other => panic!("expected a removal error, got {other:?}"),
    }
    assert!(message.contains("does not have"), "{message}");
    assert!(
        message.contains("the known ones are \"layer\""),
        "{message}"
    );
    assert!(error.location().is_some(), "{message}");
}

#[test]
fn filling_a_component_the_entity_does_not_carry_is_refused() {
    let (error, message) = failure(concat!(
        "Scene(prefabs: { \"p\": (components: { \"layer\": (depth: 0.0) }) }, entities: [\n",
        "    (from: \"p\", fill: { \"transform\": { \"x\": 1.0 } }),\n",
        "])"
    ));

    assert!(
        matches!(error, SceneError::FillForAbsentComponent { .. }),
        "{error:?}"
    );
    assert!(message.contains("does not add one"), "{message}");
}

#[test]
fn fill_and_without_need_a_template() {
    for (text, field) in [
        (
            "Scene(entities: [(components: { \"layer\": (depth: 0.0) }, fill: { \"layer\": { \"depth\": 1.0 } })])",
            "fill",
        ),
        (
            "Scene(entities: [(components: { \"layer\": (depth: 0.0) }, without: [\"layer\"])])",
            "without",
        ),
    ] {
        let (error, message) = failure(text);
        match &error {
            SceneError::PrefabOnlyField { field: found, .. } => assert_eq!(*found, field),
            other => panic!("expected a prefab-only field error for {field}, got {other:?}"),
        }
        assert!(message.contains("need `from`"), "{message}");
    }
}

/// The collecting validator reports prefab findings like any other, and raises
/// no new warning class.
#[test]
fn the_validator_collects_prefab_findings_without_a_new_warning_class() {
    let report = narvo_scene::validate(
        concat!(
            "Scene(prefabs: { \"p\": (components: { \"layer\": (depth: 0.0) }) }, entities: [\n",
            "    (from: \"nope\"),\n",
            "    (from: \"p\", without: [\"sampling\"]),\n",
            "    (from: \"p\", fill: { \"transform\": { \"x\": 1.0 } }),\n",
            "])"
        ),
        &registry(),
    );

    let all = report
        .findings()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(report.errors(), 3, "one pass, three findings:\n{all}");
    assert_eq!(
        report.warnings(),
        0,
        "M4.2 decided on exactly two warning classes and this adds none:\n{all}"
    );
    for expected in [
        "instantiates \"nope\"",
        "removes \"sampling\"",
        "fills a field",
    ] {
        assert!(all.contains(expected), "missing {expected:?}:\n{all}");
    }
    assert!(
        report
            .findings()
            .iter()
            .all(|finding| finding.severity() == Severity::Error),
        "{all}"
    );
}

// ---- the property --------------------------------------------------------

/// A generated template/instance pair, and the flat scene it must equal.
#[derive(Debug, Clone)]
struct Case {
    /// Which of the template's three components the instance removes.
    removed: Option<usize>,
    /// Whether the instance replaces the layer with its own body.
    replaces_layer: bool,
    /// Whether the instance adds a component the template lacks.
    adds_shake: bool,
    /// The two hole values.
    x: f32,
    y: f32,
    /// The template's depth, and the instance's if it replaces.
    template_depth: f32,
    instance_depth: f32,
}

/// The three components a template offers, so `removed` can name one.
const OFFERED: [&str; 3] = ["transform", "layer", "sampling"];

fn case() -> impl Strategy<Value = Case> {
    (
        proptest::option::of(0_usize..3),
        any::<bool>(),
        any::<bool>(),
        finite(),
        finite(),
        finite(),
        finite(),
    )
        .prop_map(
            |(removed, replaces_layer, adds_shake, x, y, template_depth, instance_depth)| Case {
                removed,
                replaces_layer,
                adds_shake,
                x,
                y,
                template_depth,
                instance_depth,
            },
        )
}

/// Floats that survive a text round trip, which is every `f32` but a
/// payload-carrying `NaN` (M4.1 measured it; the report records the finding).
fn finite() -> impl Strategy<Value = f32> {
    any::<u32>()
        .prop_map(f32::from_bits)
        .prop_filter("a NaN payload is outside the format's domain", |value| {
            !value.is_nan()
        })
}

impl Case {
    /// Whether removing the transform would leave its holes unfillable.
    fn removes_transform(&self) -> bool {
        self.removed == Some(0)
    }

    fn prefab_scene(&self) -> String {
        let mut entry = String::from("        (from: \"p\"");
        if !self.removes_transform() {
            entry.push_str(&format!(
                ", fill: {{ \"transform\": {{ \"x\": {:?}, \"y\": {:?} }} }}",
                self.x, self.y
            ));
        }
        if self.replaces_layer && self.removed != Some(1) {
            entry.push_str(&format!(
                ", components: {{ \"layer\": (depth: {:?}) }}",
                self.instance_depth
            ));
        } else if self.adds_shake {
            entry.push_str(", components: { \"shake\": (amplitude: 0.0, frequency: 1.0, decay: 0.5, phase: 0.0, cutoff: 0.0, base_x: 0.0, base_y: 0.0) }");
        }
        if let Some(index) = self.removed {
            entry.push_str(&format!(", without: [\"{}\"]", OFFERED[index]));
        }
        entry.push_str("),\n");

        format!(
            "Scene(prefabs: {{ \"p\": (components: {{\n\
             \x20   \"transform\": (rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n\
             \x20   \"layer\": (depth: {:?}),\n\
             \x20   \"sampling\": (filter: 1),\n\
             }}) }}, entities: [\n{entry}])\n",
            self.template_depth
        )
    }

    fn flat_scene(&self) -> String {
        let mut components = String::new();
        if self.removed != Some(0) {
            components.push_str(&format!(
                "        \"transform\": (x: {:?}, y: {:?}, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n",
                self.x, self.y
            ));
        }
        if self.removed != Some(1) {
            let depth = if self.replaces_layer {
                self.instance_depth
            } else {
                self.template_depth
            };
            components.push_str(&format!("        \"layer\": (depth: {depth:?}),\n"));
        }
        if self.removed != Some(2) {
            components.push_str("        \"sampling\": (filter: 1),\n");
        }
        if self.adds_shake && !(self.replaces_layer && self.removed != Some(1)) {
            components.push_str("        \"shake\": (amplitude: 0.0, frequency: 1.0, decay: 0.5, phase: 0.0, cutoff: 0.0, base_x: 0.0, base_y: 0.0),\n");
        }

        format!("Scene(entities: [\n    (components: {{\n{components}    }}),\n])\n")
    }
}

proptest! {
    // 256 cases: the shape space is eight combinations of the three structural
    // choices, and the floats are what make each draw new. A run costs well
    // under a second.
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// **Expansion is a load-time transformation**, over generated combinations
    /// of the four operations rather than over the examples above.
    ///
    /// The flat scene is built programmatically from the same case, so the two
    /// sides are different code producing the same world — not one string
    /// derived from the other.
    #[test]
    fn a_generated_instance_equals_its_flattened_form(case in case()) {
        let registry = registry();

        let prefabbed = narvo_scene::from_str(&case.prefab_scene(), &registry)
            .map_err(|error| TestCaseError::fail(
                format!("the prefab scene does not load: {error}\n{}", case.prefab_scene())
            ))?;
        let flat = narvo_scene::from_str(&case.flat_scene(), &registry)
            .map_err(|error| TestCaseError::fail(
                format!("the flat scene does not load: {error}\n{}", case.flat_scene())
            ))?;

        let a = canonical_dump(&prefabbed, &registry).expect("registered");
        let b = canonical_dump(&flat, &registry).expect("registered");

        if let Some(difference) = first_difference(&a, &b) {
            return Err(TestCaseError::fail(format!(
                "expansion is not the flattening.\n{difference}\n\nprefab scene:\n{}\nflat scene:\n{}",
                case.prefab_scene(),
                case.flat_scene()
            )));
        }
    }
}

/// A world is a world: nothing about it says a template was involved.
#[test]
fn a_prefab_world_is_indistinguishable_from_a_flat_one() {
    let registry = registry();
    let prefabbed = narvo_scene::from_str(
        &std::fs::read_to_string(example_path()).expect("readable"),
        &registry,
    )
    .expect("loads");
    let flat = narvo_scene::from_str(flat_twin(), &registry).expect("loads");

    assert_eq!(prefabbed.len(), flat.len());
    assert_eq!(
        canonical_dump(&prefabbed, &registry).expect("registered"),
        canonical_dump(&flat, &registry).expect("registered")
    );
    // ... and the two write out identically, which the writer test's reload
    // covers for one of them and this pins for the pair.
    assert_eq!(
        narvo_scene::to_string(&prefabbed, &registry).expect("writes"),
        narvo_scene::to_string(&flat, &registry).expect("writes")
    );
}

/// The world type is not needed beyond this; kept so the import is honest.
#[allow(dead_code)]
fn _uses_world(_: &World) {}
