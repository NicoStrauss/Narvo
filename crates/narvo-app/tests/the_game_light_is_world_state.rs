//! The game light survives the three things M8.8 §3 makes mandatory.
//!
//! Each of the three is a different mechanism and each gets a test of its own,
//! because a component that is merely *stored* is easy and the claim here is
//! stronger:
//!
//! - **headless** — the level is computed with no GPU, no `wgpu` device and no
//!   `render` feature. This file is deliberately **not** gated on `render`, so
//!   `cargo nextest run -p narvo-app --no-default-features` (step 8 of the
//!   verification set) runs it against a binary that has no renderer at all;
//! - **replay-exact** — ADR-0032's guarantee read from the light's side: a run
//!   reproducing a recording reproduces the levels it computed, bit for bit;
//! - **scalar** — GDD-L6a. One number per entity, no colour, checked on the
//!   dump's own text rather than on the type, because the text is what travels
//!   into a save (ADR-0043) and across the agent protocol (ADR-0030).
//!
//! Deliberately over the binary rather than over a library call, for M8.3b's
//! reason: the three mechanisms meet in the runner, and a unit test that
//! assembled a world in memory would be checking the parts rather than the
//! thing. `narvo-ecs`' own tests check the parts.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// The workspace root, which every command below runs from.
///
/// **Not a convenience.** A scene path has to be *relative* and spelled with
/// forward slashes, because a recording carries it and ADR-0019 requires the
/// anchor to mean the same file on another machine and another platform.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// The scene M8.8 added, as the runner insists on seeing it.
fn scene() -> String {
    "crates/narvo-app/scenes/lit.ron".to_owned()
}

/// The binary under test, beside this test's own executable.
fn binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("the test binary knows where it is");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join(format!("narvo{}", std::env::consts::EXE_SUFFIX))
}

/// Runs the binary and returns its stdout, failing loudly with stderr.
fn narvo(arguments: &[&OsStr]) -> String {
    let output = Command::new(binary())
        .current_dir(workspace_root())
        .args(arguments)
        .output()
        .expect("the narvo binary has to be runnable");
    assert!(
        output.status.success(),
        "narvo {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("narvo writes UTF-8")
}

/// The canonical dump of a run over `path`, without the banner line.
fn dump_of(path: &str, ticks: &str) -> String {
    let text = narvo(&[
        OsStr::new("--headless"),
        OsStr::new("--scene"),
        OsStr::new(path),
        OsStr::new("--seed"),
        OsStr::new("7"),
        OsStr::new("--ticks"),
        OsStr::new(ticks),
        OsStr::new("--dump"),
    ]);
    text.lines()
        .skip_while(|line| line.starts_with("narvo:"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `lit` level in a dump, in the dump's own order.
///
/// **Parsed as `f32` and not as `f64`, and that is the whole comparison.** The
/// stored value is an `f32`, and the dump writes its shortest round-tripping
/// spelling; widening that text to `f64` produces a *different* number from
/// widening the `f32` itself, so a comparison done in `f64` fails on two values
/// that are bit-identical. Parsing back into the type it was written from is
/// exact in both directions.
fn levels(dump: &str) -> Vec<f32> {
    dump.lines()
        .filter_map(|line| line.trim().strip_prefix("lit (level:"))
        .map(|rest| {
            rest.trim_end_matches(')')
                .parse::<f32>()
                .expect("a level is a number")
        })
        .collect()
}

/// A directory under `target/` for this test's artifacts, per ADR-0035.
fn artifact_dir(name: &str) -> String {
    let relative = format!("target/light-artifacts/{name}");
    std::fs::create_dir_all(workspace_root().join(&relative))
        .expect("the artifact directory must be creatable");
    relative
}

// -- (1) headless ----------------------------------------------------------

/// **§3(a): the game light is computed with no GPU, no device and no `render`
/// feature.**
///
/// The test does not *say* the renderer is absent; the configuration it runs in
/// does. Under `--no-default-features` this file is compiled against a
/// `narvo-app` whose dependency tree contains no `wgpu`, `winit` or `naga` —
/// which is verification step 9's `cargo tree` check, holding
/// `FORBIDDEN_IN_HEADLESS` — and the binary it drives is that same
/// configuration's. So a level arriving here at all is the property.
///
/// What is asserted beyond mere presence: the four receivers land on **four
/// different** levels covering the three-ray fan's whole range, so a light that
/// only ever answered "lit" or "dark" would fail here rather than pass by
/// producing plausible extremes.
#[test]
fn the_levels_are_computed_with_no_renderer_present() {
    let dump = dump_of(&scene(), "1");
    let levels = levels(&dump);

    assert_eq!(
        levels.len(),
        5,
        "the scene declares five receivers; the dump has {}:\n{dump}",
        levels.len()
    );

    let dark = levels.iter().filter(|level| **level == 0.0).count();
    assert_eq!(dark, 1, "exactly one receiver stands squarely in shadow");

    let mut distinct: Vec<f32> = levels.clone();
    distinct.sort_by(f32::total_cmp);
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        5,
        "the five receivers were meant to read five different levels, and read \
         {distinct:?} — a light with no penumbra would collapse them"
    );

    // The fan's four outcomes, as ratios rather than as stored values
    // (ADR-0008): the two penumbral receivers stand at the same distance band,
    // so the ratio of their levels is close to the ratio of their open rays.
    let scout = levels[3];
    let sentry = levels[2];
    assert!(
        scout > 0.0 && sentry > scout,
        "the penumbra was expected to resolve two distinct partial levels, and \
         gave scout {scout} and sentry {sentry}"
    );
    let ratio = sentry / scout;
    assert!(
        (1.5..2.5).contains(&ratio),
        "two rays over one ray should be about 2, and the levels give {ratio} \
         — which is what a fan that had collapsed to a single ray would break"
    );
}

/// The runner reports no renderer in this configuration, and the level is there
/// anyway.
///
/// A second, independent statement of the same property: the *banner* the
/// headless runner prints names the mode it ran, and the dump beside it carries
/// the levels. It costs nothing and it fails differently from the test above —
/// that one fails if the arithmetic is wrong, this one if the scene stops
/// reaching the runner at all.
#[test]
fn a_headless_run_reports_the_scene_and_carries_the_light() {
    let text = narvo(&[
        OsStr::new("--headless"),
        OsStr::new("--scene"),
        OsStr::new(&scene()),
        OsStr::new("--ticks"),
        OsStr::new("1"),
        OsStr::new("--dump"),
    ]);
    assert!(
        text.contains("lightsource (range:24.0,intensity:1.0,radius:0.75)"),
        "the lamp did not reach the dump:\n{text}"
    );
    assert!(
        text.lines().any(|line| line.trim().starts_with("lit (")),
        "no level reached the dump:\n{text}"
    );
}

// -- (2) replay-exact ------------------------------------------------------

/// **§3(b): a replay reproduces the levels it recorded, bit for bit.**
///
/// ADR-0032's guarantee read from the game light's side. The comparison is over
/// the dump's **text**, so it is exact in the sense that matters: a level that
/// came back one ulp different would spell differently and fail here.
#[test]
fn a_replay_reproduces_the_levels_it_recorded() {
    let recording = format!("{}/lit.narvorec", artifact_dir("replay"));

    narvo(&[
        OsStr::new("--headless"),
        OsStr::new("--scene"),
        OsStr::new(&scene()),
        OsStr::new("--seed"),
        OsStr::new("7"),
        OsStr::new("--ticks"),
        OsStr::new("50"),
        OsStr::new("--record"),
        OsStr::new(&recording),
    ]);
    assert!(
        workspace_root().join(&recording).exists(),
        "the run wrote no recording at {recording}"
    );

    let replayed = narvo(&[
        OsStr::new("--headless"),
        OsStr::new("--replay"),
        OsStr::new(&recording),
        OsStr::new("--dump"),
    ]);
    let replayed_body = replayed
        .lines()
        .skip_while(|line| line.starts_with("narvo:"))
        .collect::<Vec<_>>()
        .join("\n");

    let original = dump_of(&scene(), "50");
    assert_eq!(
        replayed_body, original,
        "a replay did not reproduce the world it recorded"
    );

    // And it cannot pass by reproducing a world with no light in it.
    let reproduced = levels(&replayed_body);
    assert_eq!(reproduced.len(), 5, "the replay reproduced no levels");
    assert!(
        reproduced.iter().any(|level| *level > 0.0),
        "the replay reproduced a world in which nothing is lit:\n{replayed_body}"
    );
}

/// Two runs over one scene end in one dump, and a moved wall ends in another.
///
/// The half that says the level is a function of the *world*: without the second
/// comparison, a light that returned a constant would satisfy the first.
#[test]
fn moving_a_wall_moves_the_levels_and_leaving_it_alone_does_not() {
    let original = dump_of(&scene(), "10");
    assert_eq!(
        original,
        dump_of(&scene(), "10"),
        "two runs over one scene produced two dumps"
    );

    let text = std::fs::read_to_string(workspace_root().join(scene()))
        .expect("the committed scene is readable");
    // One line, not a block. The working tree's line endings are the machine's
    // (`core.autocrlf`) while a Rust literal's are the source file's, so a
    // multi-line pattern would match on one checkout and silently not on
    // another — and "silently not" is the half that would turn this guard off
    // without turning it red.
    let moved = text.replace(
        "(x: -2.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0)",
        "(x: -2.0, y: 40.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0)",
    );
    assert_ne!(
        moved, text,
        "the wall was not found in the scene to move it"
    );

    let relative = format!("{}/lit.ron", artifact_dir("moved"));
    std::fs::write(workspace_root().join(&relative), &moved).expect("writable");

    let shifted = dump_of(&relative, "10");
    assert_ne!(
        original, shifted,
        "moving the wall out of the room did not move a single level"
    );
    assert!(
        levels(&shifted).iter().all(|level| *level > 0.0),
        "with the wall gone every receiver should see the lamp:\n{shifted}"
    );
}

// -- (3) scalar ------------------------------------------------------------

/// **§3(c): the level is one scalar, and colour stays outside.**
///
/// GDD-L6a, asserted on the dump's own text. Every `lit` line has to be exactly
/// `lit (level:<number>)` — one field, one number, no second component and no
/// channel. That is the form a save writes and the form the agent protocol
/// carries, so a colour arriving in the game light would have to change this
/// line and would be caught here rather than in a save somebody cannot load.
///
/// It is checked structurally rather than by looking for the word "colour": a
/// leak could be spelled `rgb`, `tint`, `hue` or nothing at all, and what they
/// would share is a second value inside the parentheses.
#[test]
fn a_level_is_one_number_and_carries_no_colour() {
    let dump = dump_of(&scene(), "1");

    let lines: Vec<&str> = dump
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("lit ("))
        .collect();
    assert_eq!(lines.len(), 5, "the scene declares five receivers:\n{dump}");

    for line in lines {
        let body = line
            .strip_prefix("lit (")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or_else(|| panic!("a level is not spelled as expected: {line}"));

        assert!(
            !body.contains(','),
            "a level carries more than one field, which is a second axis the \
             game light must not have (GDD-L6a): {line}"
        );
        assert!(
            !body.contains('[') && !body.contains('('),
            "a level carries a compound value rather than a scalar: {line}"
        );

        let value = body
            .strip_prefix("level:")
            .unwrap_or_else(|| panic!("a level's one field is not `level`: {line}"));
        value
            .parse::<f64>()
            .unwrap_or_else(|_| panic!("a level's one field is not a number: {line}"));
    }

    // And the source is scalar too: three numbers, none of them a channel.
    let source = dump
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("lightsource ("))
        .expect("the scene declares a lamp");
    assert_eq!(
        source, "lightsource (range:24.0,intensity:1.0,radius:0.75)",
        "the source's spelling moved; `intensity` is one number and has to stay one"
    );
}

/// **A reader that links `narvo-ecs` and nothing else computes the same levels
/// the runner did.**
///
/// M8.3b checked that the occluder *set* is reachable from the simulation side
/// with no GPU type in the way, and addressed the second consumer to this task.
/// This is that consumer, closing the statement: the whole light — components,
/// system and arithmetic — is reachable from a crate that depends on `hecs`,
/// `serde` and `ron` and on nothing else in this workspace.
///
/// It is a compile-and-run statement rather than a claim: the only engine
/// import below is `narvo_ecs` (plus `narvo_scene` to read the file), it names
/// no renderer type, and the levels it computes are compared against the ones
/// the *binary* wrote. If the light ever became reachable only through the
/// extraction, this file would stop compiling.
#[test]
fn a_reader_with_only_the_ecs_computes_the_runner_s_levels() {
    use narvo_ecs::{
        ComponentRegistry, Lit, SystemContext, illuminate, register_engine_components,
    };

    let mut registry = ComponentRegistry::new();
    register_engine_components(&mut registry).expect("a fresh registry accepts them");

    let mut world = narvo_scene::from_file(&workspace_root().join(scene()), &registry)
        .expect("the committed scene loads");
    illuminate(&mut world, &SystemContext::new(0));

    let mut mine: Vec<f32> = world
        .entity_ids()
        .into_iter()
        .filter_map(|entity| world.get::<Lit>(entity).ok().map(|lit| lit.level))
        .collect();

    let mut theirs = levels(&dump_of(&scene(), "1"));

    assert_eq!(
        mine.len(),
        5,
        "the sim-side reader found {} levels",
        mine.len()
    );
    mine.sort_by(f32::total_cmp);
    theirs.sort_by(f32::total_cmp);
    assert_eq!(
        mine, theirs,
        "a reader linking only the ECS computed different levels from the runner"
    );
    assert!(
        theirs.iter().any(|level| *level > 0.0),
        "the comparison would hold for two lights that both returned nothing"
    );
}
