//! Where a finding is, and the pass that collects every finding at once.
//!
//! A second file rather than more of `scene.rs`, because these are questions
//! about a different thing: `scene.rs` asks whether the format carries a world,
//! this asks whether it tells an author what is wrong with a file. The registry
//! fixture is duplicated between them, which ADR-0016's rule permits — a fixture
//! used inside one crate does not earn a crate of its own — and the seven names
//! are checked against the registry rather than against each other by
//! `the_specimen_uses_every_registered_component` next door.

use std::collections::BTreeSet;
use std::path::PathBuf;

use narvo_ecs::{
    Burst, Camera, ComponentRegistry, Follow, HitRect, Layer, LightSource, Lit, Occluder,
    RigidBody, Rng, Sampling, Shake, Sprite, Tally, Tint, Transform, World,
};
use narvo_scene::{Finding, Location, SceneError, Severity};

/// Every component type the engine itself registers. See `scene.rs`.
fn registry() -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    for result in [
        registry.register_component::<Transform>("transform"),
        registry.register_component::<Burst>("burst"),
        registry.register_component::<Layer>("layer"),
        registry.register_component::<Sampling>("sampling"),
        registry.register_component::<Camera>("camera"),
        registry.register_component::<Follow>("follow"),
        registry.register_component::<Shake>("shake"),
        registry.register_component::<Rng>("rng"),
        registry.register_component::<Sprite>("sprite"),
        registry.register_component::<Occluder>("occluder"),
        registry.register_component::<HitRect>("hitrect"),
        registry.register_component::<Tally>("tally"),
        registry.register_component::<RigidBody>("rigidbody"),
        registry.register_component::<Tint>("tint"),
        registry.register_component::<LightSource>("lightsource"),
        registry.register_component::<Lit>("lit"),
    ] {
        result.expect("a fresh registry accepts each of these once");
    }
    registry
}

/// This file's census and the engine's own registration agree.
///
/// The twin of `scene.rs`'s guard of the same name, and here for the same
/// reason M4.8 found: this crate carries **two** hand-written copies of the
/// engine set, and when an eighth type was added neither noticed. What noticed
/// was `the_specimen_has_nothing_to_report`, several steps downstream, reporting
/// the specimen's new component as unregistered — a true statement about this
/// registry and a misleading one about the engine.
#[test]
fn the_transcribed_census_is_the_engine_s_own_set() {
    let transcribed: Vec<&str> = registry().iter().map(|info| info.name()).collect();

    let mut engine = ComponentRegistry::new();
    narvo_ecs::register_engine_components(&mut engine).expect("a fresh registry accepts them");
    let registered: Vec<&str> = engine.iter().map(|info| info.name()).collect();

    assert_eq!(
        transcribed, registered,
        "this file's hand-written census and `register_engine_components` have \
         drifted apart, so every finding below is measured against a set the \
         engine no longer has"
    );
}

/// Where the committed specimen lives.
fn example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenes")
        .join("example.ron")
}

/// Renders a report the way a reader sees it, for assertion messages.
fn rendered(report: &narvo_scene::ValidationReport) -> String {
    report
        .findings()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- where a finding is ------------------------------------------------

/// **The standing guard on the offset mechanism.**
///
/// Positions in this crate are recovered by subtracting two addresses: a
/// component body is a `&str` borrowed out of the scene text, so the difference
/// between its pointer and the text's is a byte offset into the file. The borrow
/// is a *lifetime* guarantee rather than a promise `ron` could quietly withdraw
/// — a `&'a RawValue` cannot point anywhere but into the `&'a str` that was
/// parsed — but the arithmetic on top of it is this crate's, and `locate`
/// refuses to trust it: it compares the slice at the offset it computed against
/// the fragment before returning a position.
///
/// This test is what makes that refusal visible. If the borrow ever stopped
/// holding, `locate` would return `None`, every position would quietly go
/// missing, and **every other test in this crate would still pass** — they
/// assert on messages, and a message without a position is still the same
/// message. Here the position *is* the assertion, counted by hand from the
/// literal below.
#[test]
fn the_offset_mechanism_locates_a_known_fragment() {
    // Line 4 is `            "nope": (x: 1.0),` — twelve spaces of indent, then
    // `"nope": ` is eight characters, so the body opens at column 21.
    let text = "Scene(entities: [\n    (\n        components: {\n            \"nope\": (x: 1.0),\n        },\n    ),\n])\n";

    let report = narvo_scene::validate(text, &registry());
    let finding = report
        .findings()
        .first()
        .expect("an unregistered component is a finding");

    let location = finding.location().unwrap_or_else(|| {
        panic!(
            "the finding has no position, so the offset mechanism recovered none. Either \
             `ron` stopped borrowing the body out of the input, or `locate`'s slice check \
             rejected an offset it had computed. Both are real changes and neither may pass \
             quietly: positions would simply stop appearing.\nfinding: {finding}"
        )
    });

    assert_eq!(
        (location.line(), location.column()),
        (4, 21),
        "the body of the unregistered component is on line 4 at column 21: {finding}"
    );
}

/// The same fault, written further down, is reported further down.
///
/// The point of a position is that it follows the input. A constant — or one
/// derived from the entity index rather than from the text — would satisfy every
/// other assertion in this crate, so this is what says the number means
/// something. Three paddings rather than two, because two points also fit a
/// formula that is wrong at the third.
#[test]
fn a_fault_moves_with_the_line_it_is_written_on() {
    let line_of = |padding: usize| {
        let filler = "    // a line that says nothing\n".repeat(padding);
        let text = format!(
            "Scene(entities: [\n{filler}    (name: \"a\", components: {{ \"positon\": (x: 1.0) }}),\n])\n"
        );
        narvo_scene::validate(&text, &registry())
            .findings()
            .first()
            .and_then(Finding::location)
            .map(Location::line)
            .unwrap_or_else(|| panic!("no position at padding {padding}"))
    };

    assert_eq!(line_of(0), 2);
    assert_eq!(line_of(5), 7);
    assert_eq!(line_of(12), 14);
}

/// A writer-side error has no position, because there is no input to point into.
#[test]
fn a_writer_side_error_carries_no_position() {
    let registry = registry();
    let mut world = World::new();
    let first = world.spawn();
    let _second = world.spawn();
    world.despawn(first).expect("the entity is alive");

    let error = narvo_scene::to_string(&world, &registry).expect_err("not a scene");

    assert_eq!(
        error.location(),
        None,
        "a world is not a text, so nothing about it has a line number"
    );
    assert!(error.to_string().starts_with("EntityId("), "{error}");
}

// ---- collecting rather than stopping ------------------------------------

/// A scene with seven faults of six kinds reports all of them in one pass.
///
/// This is the whole reason `validate` exists beside `from_str`: an author with
/// seven mistakes gets seven messages instead of seven runs, because a fail-fast
/// loader hides each mistake behind the one before it.
///
/// The two reference faults are on two different entities on purpose, and the
/// reason is a property worth pinning: an unresolvable *target* and a reference
/// to an *absent component* cannot both come from one `refs` entry. A reference
/// whose component is not there is never resolved at all, so it produces one
/// finding rather than two — no cascade. Putting them on one entity would have
/// asserted six findings and quietly tested five.
#[test]
fn the_validator_reports_every_fault_in_one_pass() {
    let text = concat!(
        "Scene(entities: [\n",
        "    (name: \"player\", components: { \"layer\": (depth: 1.0) }),\n",
        "    (name: \"\", components: {}),\n",
        "    (name: \"player\", components: { \"layer\": (depth: 2.0), \"layer\": (depth: 3.0) }),\n",
        "    (\n",
        "        components: { \"nope\": (x: 1.0) },\n",
        "        refs: { \"shake\": { \"target\": \"player\" } },\n",
        "    ),\n",
        "    (\n",
        "        components: { \"follow\": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false) },\n",
        "        refs: { \"follow\": { \"target\": \"ghost\" } },\n",
        "    ),\n",
        "    (components: { \"layer\": (depht: 9.0) }),\n",
        "])\n"
    );

    let report = narvo_scene::validate(text, &registry());
    let all = rendered(&report);

    assert_eq!(
        report.findings().len(),
        7,
        "expected seven findings in one pass, got:\n{all}"
    );
    assert_eq!((report.errors(), report.warnings()), (6, 1), "{all}");
    assert!(!report.loads());

    for expected in [
        "named with the empty string",
        "two entities are both named \"player\"",
        "names the component \"layer\" twice",
        "carries \"nope\"",
        "at an entity named \"ghost\"",
        "gives a reference for \"shake\"",
        "could not read its \"layer\"",
    ] {
        assert!(
            all.contains(expected),
            "one pass should have reported {expected:?}:\n{all}"
        );
    }

    // Every one is placed, and they do not all point at one line.
    let lines: BTreeSet<usize> = report
        .findings()
        .iter()
        .map(|finding| {
            finding
                .location()
                .unwrap_or_else(|| panic!("unplaced finding: {finding}"))
                .line()
        })
        .collect();
    assert!(lines.len() >= 4, "findings bunched onto {lines:?}:\n{all}");
}

/// The validator and the loader never disagree about the first error.
///
/// They share one traversal and differ only in what their sink does, so this
/// checks that the sharing is real rather than that two implementations happen
/// to agree today. Run over every shape of broken scene, so a check added to one
/// path cannot quietly go missing from the other.
#[test]
fn the_validator_and_the_loader_agree_on_the_first_error() {
    let broken = [
        "Scene(entities: [(name: \"a\", components: { \"positon\": (x: 1.0) })])",
        "Scene(entities: [(name: \"p\", components: {}), (name: \"p\", components: {})])",
        "Scene(entities: [(components: { \"layer\": (depth: 1.0), \"layer\": (depth: 2.0) })])",
        "Scene(entities: [(components: { \"layer\": (depht: 1.0) })])",
        "Scene(entities: [(name: 5, components: {})])",
        concat!(
            "Scene(entities: [(name: \"p\", components: { \"layer\": (depth: 1.0) }), ",
            "(components: { \"follow\": (smoothing: 1.0, x: 0.0, y: 0.0, lost: false) }, ",
            "refs: { \"follow\": { \"target\": \"nobody\" } })])"
        ),
        concat!(
            "Scene(entities: [(components: { \"camera\": (x: 0.0, y: 0.0, zoom: 1.0) }, ",
            "refs: { \"follow\": { \"target\": \"nobody\" } })])"
        ),
        "Scene(entities: [(nmae: \"p\", components: {})])",
        "Scene(entities: [(name: \"p\", components: {}",
    ];

    for text in broken {
        let loaded = narvo_scene::from_str(text, &registry())
            .err()
            .unwrap_or_else(|| panic!("this scene was supposed to be rejected:\n{text}"));
        let report = narvo_scene::validate(text, &registry());

        let first_error = report
            .findings()
            .iter()
            .find(|finding| finding.severity() == Severity::Error)
            .unwrap_or_else(|| panic!("the validator found no error in:\n{text}"));

        assert_eq!(
            first_error.message(),
            loaded.message(),
            "the two paths disagree about:\n{text}"
        );
        assert_eq!(first_error.location(), loaded.location(), "for:\n{text}");
    }
}

/// A scene that is not RON stops at the syntax error and says only that.
#[test]
fn a_syntax_error_is_the_only_finding_because_there_is_nothing_left_to_walk() {
    let report = narvo_scene::validate("Scene(entities: [(name: \"a\"", &registry());

    assert_eq!(report.findings().len(), 1);
    assert_eq!(report.errors(), 1);
    assert!(report.findings()[0].message().contains("not valid RON"));
    assert!(report.findings()[0].location().is_some());
}

/// A clean scene reports nothing and says it loads.
#[test]
fn a_scene_with_nothing_wrong_reports_nothing() {
    let report = narvo_scene::validate(
        "Scene(entities: [(components: { \"layer\": (depth: 1.0) })])",
        &registry(),
    );

    assert!(report.is_empty(), "{}", rendered(&report));
    assert!(report.loads());
    assert_eq!((report.errors(), report.warnings()), (0, 0));
}

// ---- the two warning classes --------------------------------------------

/// The hand-written specimen is clean: no errors, and no warnings either.
///
/// The negative flank of both classes at once. A lint that fires on the
/// repository's own worked example is a lint nobody will keep.
#[test]
fn the_specimen_has_nothing_to_report() {
    let text = std::fs::read_to_string(example_path()).expect("the specimen is readable");
    let report = narvo_scene::validate(&text, &registry());

    assert!(
        report.is_empty(),
        "the committed specimen should validate clean, got:\n{}",
        rendered(&report)
    );
}

/// The writer's own output warns that its handles are positional.
///
/// The positive flank of the F4 class, and the case that matters: somebody who
/// saves a world and then starts editing the file is exactly who this is for,
/// because inserting an entity above a positional handle silently repoints it.
#[test]
fn the_writers_output_warns_that_its_handles_are_positional() {
    let registry = registry();
    let world = narvo_scene::from_file(&example_path(), &registry).expect("loads");
    let written = narvo_scene::to_string(&world, &registry).expect("writes");

    let report = narvo_scene::validate(&written, &registry);

    assert_eq!(report.errors(), 0, "the writer's output has to load");
    assert_eq!(
        report.warnings(),
        1,
        "exactly the one follow handle warns:\n{written}"
    );

    let warning = &report.findings()[0];
    assert_eq!(warning.severity(), Severity::Warning);
    assert!(
        warning.message().contains("positional entity handle"),
        "{warning}"
    );
    assert!(warning.message().contains("refs table"), "{warning}");
    assert!(
        warning.location().is_some(),
        "the warning has to say which body: {warning}"
    );
}

/// A written empty name warns; an omitted one does not.
///
/// Both flanks, because a warning that fired on every anonymous entity would be
/// worse than none — most entities in a real scene have no name.
#[test]
fn a_written_empty_name_warns_and_an_omitted_name_does_not() {
    let written = narvo_scene::validate(
        "Scene(entities: [(name: \"\", components: { \"layer\": (depth: 1.0) })])",
        &registry(),
    );
    assert_eq!((written.errors(), written.warnings()), (0, 1));
    assert!(
        written.findings()[0].message().contains("empty string"),
        "{}",
        written.findings()[0]
    );
    assert!(written.findings()[0].location().is_some());
    assert!(
        written.loads(),
        "an empty name is anonymous, not invalid - ADR-0018 is unchanged"
    );

    let omitted = narvo_scene::validate(
        "Scene(entities: [(components: { \"layer\": (depth: 1.0) })])",
        &registry(),
    );
    assert!(
        omitted.is_empty(),
        "an anonymous entity is ordinary and must not warn:\n{}",
        rendered(&omitted)
    );
}

/// An empty name is still anonymous — the warning is about that rule, not a
/// change to it.
#[test]
fn a_written_empty_name_still_means_anonymous() {
    let registry = registry();
    let world = narvo_scene::from_str(
        "Scene(entities: [(name: \"\", components: { \"layer\": (depth: 1.0) })])",
        &registry,
    )
    .expect("an empty name is legal");

    assert_eq!(world.len(), 1);
}

/// A `name` that is not a string is an error that says what was written.
#[test]
fn a_name_that_is_not_a_string_is_reported_where_it_was_written() {
    let error = narvo_scene::from_str("Scene(entities: [(name: 5, components: {})])", &registry())
        .expect_err("a name has to be text");

    match &error {
        SceneError::InvalidEntityName { index, written, .. } => {
            assert_eq!(*index, 0);
            assert_eq!(written, "5");
        }
        other => panic!("expected an invalid name error, got {other:?}"),
    }
    assert!(error.to_string().contains("not a string"), "{error}");
    assert!(error.location().is_some(), "{error}");
}

/// A body with no handle in it does not warn.
///
/// The negative flank of the F4 class on its own, so that the class is known to
/// discriminate rather than merely to fire.
#[test]
fn an_ordinary_body_does_not_warn_about_handles() {
    let report = narvo_scene::validate(
        concat!(
            "Scene(entities: [(components: { ",
            "\"transform\": (x: 1.0, y: 2.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0) ",
            "})])"
        ),
        &registry(),
    );

    assert!(report.is_empty(), "{}", rendered(&report));
}
