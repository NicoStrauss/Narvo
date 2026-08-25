//! The binary, driven as a user drives it.
//!
//! Everything about *what* is a finding is tested in `narvo-scene`, where the
//! checks live. What is tested here is only what this crate owns: the argument
//! surface, the line format, the summary, and the exit codes — the last of which
//! are the part a script depends on and the part nothing else can assert.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Runs the binary with `arguments` and hands back what it did.
fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_narvo-cli"))
        .args(arguments)
        .output()
        .expect("the binary was just built by the test harness")
}

/// The committed specimen, resolved from this crate rather than from a
/// working directory a test runner happened to pick.
fn specimen() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("crates")
        .join("narvo-scene")
        .join("scenes")
        .join("example.ron")
}

/// Writes `text` to a scratch file cargo gives integration tests, and hands back
/// the path. Nothing is written inside the repository.
fn scratch(name: &str, text: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::write(&path, text).expect("the target scratch directory is writable");
    path
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("the tool writes UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("the tool writes UTF-8")
}

fn code(output: &Output) -> i32 {
    output
        .status
        .code()
        .expect("the tool exits rather than being signalled")
}

/// The everyday green case, and the drift guard on this crate's registry.
///
/// The specimen carries every registered engine component, so a type missing
/// from `engine_registry` turns this red with a message naming it — which is
/// what stands in for a compiler check that cannot exist.
#[test]
fn the_specimen_validates_clean_through_the_binary() {
    let output = run(&["validate", &specimen().display().to_string()]);

    assert_eq!(code(&output), 0, "stdout:\n{}", stdout(&output));
    assert_eq!(stdout(&output).trim(), "no findings");
    assert!(stderr(&output).is_empty(), "{}", stderr(&output));
}

/// A broken scene: exit 1, one line per finding, positions in the line.
#[test]
fn a_broken_scene_exits_one_and_places_every_finding() {
    let path = scratch(
        "broken.ron",
        concat!(
            "Scene(entities: [\n",
            "    (name: \"player\", components: { \"layer\": (depth: 1.0) }),\n",
            "    (name: \"player\", components: { \"nope\": (x: 1.0) }),\n",
            "])\n"
        ),
    );

    let output = run(&["validate", &path.display().to_string()]);
    let text = stdout(&output);

    assert_eq!(code(&output), 1, "stdout:\n{text}");

    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "two findings and a summary:\n{text}");
    assert!(
        lines[0].contains(":3:12: error: two entities are both named"),
        "{text}"
    );
    assert!(
        lines[1].contains(":3:44: error: entity 1 (\"player\") carries \"nope\""),
        "{text}"
    );
    assert_eq!(lines[2], "2 errors");

    // Every finding line opens with the path it was asked about, so a reader
    // with several files in a log can tell them apart.
    let prefix = path.display().to_string();
    assert!(lines[0].starts_with(&prefix), "{text}");
    assert!(lines[1].starts_with(&prefix), "{text}");
}

/// A warning alone passes, and `--deny-warnings` is what makes it fail.
///
/// Both directions in one test, because the flag's whole meaning is the
/// difference between them.
#[test]
fn deny_warnings_is_what_turns_a_warning_into_a_failure() {
    let path = scratch(
        "warning-only.ron",
        "Scene(entities: [(name: \"\", components: { \"layer\": (depth: 1.0) })])\n",
    );
    let path = path.display().to_string();

    let permissive = run(&["validate", &path]);
    assert_eq!(
        code(&permissive),
        0,
        "a warning is not an error:\n{}",
        stdout(&permissive)
    );
    assert!(
        stdout(&permissive).contains(": warning: "),
        "{}",
        stdout(&permissive)
    );
    assert!(
        stdout(&permissive).trim_end().ends_with("1 warning"),
        "{}",
        stdout(&permissive)
    );

    let strict = run(&["validate", &path, "--deny-warnings"]);
    assert_eq!(code(&strict), 1, "--deny-warnings has to fail the run");
    assert_eq!(
        stdout(&strict),
        stdout(&permissive),
        "the flag changes the verdict, not the report"
    );
}

/// `--deny-warnings` on a clean scene still passes.
#[test]
fn deny_warnings_does_not_fail_a_clean_scene() {
    let output = run(&[
        "validate",
        &specimen().display().to_string(),
        "--deny-warnings",
    ]);

    assert_eq!(code(&output), 0, "{}", stdout(&output));
}

/// A file that is not there is the tool's problem, not the scene's.
#[test]
fn a_file_that_cannot_be_read_exits_two() {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("no-such-scene.ron");
    let _ = std::fs::remove_file(&missing);

    let output = run(&["validate", &missing.display().to_string()]);

    assert_eq!(
        code(&output),
        2,
        "a missing file must not read as a broken scene"
    );
    assert!(
        stderr(&output).contains("no-such-scene.ron"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).is_empty(), "{}", stdout(&output));
}

/// A scene that is not RON is the scene's problem, and exits 1.
#[test]
fn a_file_that_is_not_ron_exits_one_with_a_position() {
    let path = scratch("not-ron.ron", "Scene(entities: [(name: \"a\"\n");

    let output = run(&["validate", &path.display().to_string()]);
    let text = stdout(&output);

    assert_eq!(code(&output), 1, "{text}");
    assert!(
        text.contains(": error: the scene is not valid RON"),
        "{text}"
    );
    assert_eq!(text.lines().last(), Some("1 error"), "{text}");
}

#[test]
fn a_usage_mistake_exits_two_and_prints_usage_on_stderr() {
    for arguments in [
        vec![],
        vec!["validate"],
        vec!["invent"],
        vec!["validate", "a.ron", "b.ron"],
        vec!["validate", "a.ron", "--not-a-flag"],
    ] {
        let output = run(&arguments);

        assert_eq!(code(&output), 2, "for {arguments:?}");
        assert!(
            stderr(&output).contains("USAGE:"),
            "for {arguments:?}: {}",
            stderr(&output)
        );
        assert!(stdout(&output).is_empty(), "for {arguments:?}");
    }
}

#[test]
fn help_is_not_a_mistake() {
    for flag in ["--help", "-h", "help"] {
        let output = run(&[flag]);

        assert_eq!(code(&output), 0, "for {flag}");
        assert!(stdout(&output).contains("USAGE:"), "for {flag}");
        assert!(stderr(&output).is_empty(), "for {flag}");
    }
}

/// The writer's output validates with exactly the F4 warning, through the tool.
///
/// The end-to-end shape of the warning that M4.1 named and this task built: save
/// a world, and the tool tells you the file you got is machine-written before
/// you start editing it.
#[test]
fn a_machine_written_scene_warns_through_the_tool() {
    // Produced by round-tripping the specimen, so it is genuinely writer output
    // rather than a hand-made imitation of it.
    let registry = {
        use narvo_ecs::{
            Burst, Camera, ComponentRegistry, Follow, HitRect, Layer, LightSource, Lit, Occluder,
            RigidBody, Rng, Sampling, Shake, Sprite, Tally, Tint, Transform,
        };
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
            result.expect("a fresh registry accepts each once");
        }

        // **The second hand-copy in this file, and M5.7's addition.**
        // `the_transcribed_census_is_the_engine_s_own_set` below guards the
        // *other* one; this fixture registry was transcribed just as much and
        // guarded not at all, so a new engine component would leave it behind
        // silently. What would then fail is this test's `from_file`, reporting
        // the specimen's new component as unregistered — a true statement about
        // this local registry and a misleading one about the engine, which is
        // exactly the M4.8 cascade the guards exist to stop.
        //
        // Independence is kept on purpose: the list above is still written by
        // hand, because building both sides from `register_engine_components`
        // would let a wrong `register_engine_components` move them together.
        let transcribed: Vec<&str> = registry.iter().map(|info| info.name()).collect();
        let mut engine = ComponentRegistry::new();
        narvo_ecs::register_engine_components(&mut engine).expect("a fresh registry accepts them");
        let registered: Vec<&str> = engine.iter().map(|info| info.name()).collect();
        assert_eq!(
            transcribed, registered,
            "this fixture's hand-written census and `register_engine_components` have \
             drifted apart, so the round trip below is exercising a set the engine no \
             longer has"
        );

        registry
    };
    let world = narvo_scene::from_file(&specimen(), &registry).expect("the specimen loads");
    let written = narvo_scene::to_string(&world, &registry).expect("and writes");
    let path = scratch("written.ron", &written);

    let output = run(&["validate", &path.display().to_string()]);
    let text = stdout(&output);

    assert_eq!(code(&output), 0, "the writer's output loads:\n{text}");
    assert!(text.contains(": warning: "), "{text}");
    assert!(text.contains("positional entity handle"), "{text}");
    assert_eq!(text.lines().last(), Some("1 warning"), "{text}");

    // ... and under --deny-warnings that same file fails.
    let strict = run(&["validate", &path.display().to_string(), "--deny-warnings"]);
    assert_eq!(code(&strict), 1);
}

/// This file's transcribed census and the engine's own registration agree.
///
/// **The third copy to get this guard — and "and last" was wrong, which M5.7
/// found and M5b.3a is paying off here at the line rather than in a note.** The
/// completeness claim this sentence used to make was never measured: adding an
/// eleventh type in M5b.3a turned *six* tests red across the workspace, in five
/// files, and one further transcription stayed green because it has no guard at
/// all — `narvo-scene/tests/prefabs.rs`, whose comment says "Every component
/// type the engine registers" over a list of seven of ten. The rule is the one
/// this guard states; what was untrue is that it had already been applied
/// everywhere.
///
/// M4.8 measured what their absence costs. Registering an eighth type turned
/// four tests red across the workspace and *none* of the ones that exist to
/// notice a new type were among them — `narvo-scene`'s specimen-coverage test
/// compares the specimen against a list in its own file, and this file's inline
/// copy is the same shape. What eventually failed was a downstream test several
/// steps away, reporting the specimen's new component as unregistered: a true
/// statement about the local registry and a misleading one about the engine.
///
/// The transcription itself stays hand-written on purpose. If both sides came
/// from `register_engine_components`, a wrong `register_engine_components` would
/// move them together and the coverage tests above would keep passing. What was
/// missing was never the independence — it was a statement that the two agree,
/// which is this.
#[test]
fn the_transcribed_census_is_the_engine_s_own_set() {
    use narvo_ecs::{
        Burst, Camera, ComponentRegistry, Follow, HitRect, Layer, LightSource, Lit, Occluder,
        RigidBody, Rng, Sampling, Shake, Sprite, Tally, Tint, Transform,
    };

    let mut transcribed = ComponentRegistry::new();
    for result in [
        transcribed.register_component::<Transform>("transform"),
        transcribed.register_component::<Burst>("burst"),
        transcribed.register_component::<Layer>("layer"),
        transcribed.register_component::<Sampling>("sampling"),
        transcribed.register_component::<Camera>("camera"),
        transcribed.register_component::<Follow>("follow"),
        transcribed.register_component::<Shake>("shake"),
        transcribed.register_component::<Rng>("rng"),
        transcribed.register_component::<Sprite>("sprite"),
        transcribed.register_component::<Occluder>("occluder"),
        transcribed.register_component::<HitRect>("hitrect"),
        transcribed.register_component::<Tally>("tally"),
        transcribed.register_component::<RigidBody>("rigidbody"),
        transcribed.register_component::<Tint>("tint"),
        transcribed.register_component::<LightSource>("lightsource"),
        transcribed.register_component::<Lit>("lit"),
    ] {
        result.expect("a fresh registry accepts each once");
    }

    let mut engine = ComponentRegistry::new();
    narvo_ecs::register_engine_components(&mut engine).expect("a fresh registry accepts them");

    let transcribed: Vec<&str> = transcribed.iter().map(|info| info.name()).collect();
    let registered: Vec<&str> = engine.iter().map(|info| info.name()).collect();

    assert_eq!(
        transcribed, registered,
        "this file's hand-written census and `register_engine_components` have \
         drifted apart, so the scene it round-trips below is being validated \
         against a set the engine no longer has"
    );
}
