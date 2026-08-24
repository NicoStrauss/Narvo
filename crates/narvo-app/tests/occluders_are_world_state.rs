//! Occluders survive the three things that make something world state.
//!
//! M8.3b's end-to-end oracle, driving the built binary the way
//! `determinism.rs` does. A component that is *stored* is easy; a component that
//! is stored, hashed, and reproduced by a replay is the claim this file checks,
//! and each of the three is a different mechanism:
//!
//! - the **dump** is `canonical_dump`, so the occluder is in ADR-0008's state
//!   hash and in an ADR-0043 save;
//! - the **hash** moving with the world is what makes two different worlds two
//!   different hashes rather than one;
//! - the **replay** is ADR-0032's, and it reproduces a recorded run without
//!   taking orders from anybody.
//!
//! Deliberately over the binary rather than over a library call: the three
//! mechanisms meet in the runner, and a unit test that assembled a world in
//! memory would be checking the parts rather than the thing.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;

/// The workspace root, which every command below runs from.
///
/// **Not a convenience.** A scene path has to be *relative* and spelled with
/// forward slashes, because a recording carries it and ADR-0019 requires the
/// anchor to mean the same file on another machine and another platform — the
/// runner refuses an absolute one and says so. So the working directory is fixed
/// and the paths are relative to it, which is also how a person runs the engine.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// The scene M8.3b added, as the runner insists on seeing it.
fn scene() -> String {
    "crates/narvo-app/scenes/occluders.ron".to_owned()
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

fn hash_of(path: &str, ticks: &str) -> String {
    narvo(&[
        OsStr::new("--headless"),
        OsStr::new("--scene"),
        OsStr::new(path),
        OsStr::new("--seed"),
        OsStr::new("7"),
        OsStr::new("--ticks"),
        OsStr::new(ticks),
        OsStr::new("--hash"),
    ])
    .lines()
    .next_back()
    .expect("a hash run prints one")
    .trim()
    .to_owned()
}

/// A directory under `target/` for this test's artifacts, per ADR-0035.
///
/// Returns the path **relative to the workspace root** and with forward slashes,
/// for the reason [`workspace_root`] gives: it is handed straight to `--scene`.
fn artifact_dir(name: &str) -> String {
    let relative = format!("target/occluder-artifacts/{name}");
    std::fs::create_dir_all(workspace_root().join(&relative))
        .expect("the artifact directory must be creatable");
    relative
}

/// Writes `text` to `name` inside `directory` and returns the relative path.
fn write_scene(directory: &str, text: &str) -> String {
    let relative = format!("{directory}/occluders.ron");
    std::fs::write(workspace_root().join(&relative), text).expect("the scene must be writable");
    relative
}

/// The occluders reach the canonical dump, which is what puts them in the state
/// hash.
///
/// Asserted on the dump's own text rather than on a count, because the spelling
/// is the thing that travels: it is what a save writes, what the agent protocol
/// carries as registry bytes (ADR-0030), and what two platforms compare.
#[test]
fn the_scene_s_occluders_reach_the_canonical_dump() {
    let dumped = dump_of(&scene(), "1");

    for expected in [
        "occluder (half_width:1.0,half_height:6.0)",
        "occluder (half_width:2.0,half_height:2.0)",
        "occluder (half_width:1.5,half_height:1.5)",
    ] {
        assert!(
            dumped.contains(expected),
            "the dump does not carry `{expected}`:\n{dumped}"
        );
    }

    // And the crate carries both shapes, with different extents — the case the
    // two component types exist separately for.
    assert!(
        dumped.contains("hitrect (half_width:3.0,half_height:3.0")
            && dumped.contains("occluder (half_width:1.5,half_height:1.5)"),
        "the crate does not carry a hit area and a light blocker of different \
         sizes, so this scene stopped exercising the reason they are two types"
    );
}

/// **§4(a): the same world hashes the same, and a different world does not.**
///
/// The second half is the one that matters and is the harder one to get: a
/// component that is stored but *not* registered would leave both runs agreeing
/// while the worlds differed. The comparison is against a world this test writes
/// with one occluder moved, never against a committed hash value (ADR-0008).
#[test]
fn moving_an_occluder_moves_the_hash_and_leaving_it_alone_does_not() {
    let original = scene();
    let twice = (hash_of(&original, "100"), hash_of(&original, "100"));
    assert_eq!(
        twice.0, twice.1,
        "two runs over one scene reported two hashes"
    );

    let moved = {
        let text = std::fs::read_to_string(workspace_root().join(&original))
            .expect("the scene is readable");
        let shifted = text.replace(
            r#""transform": (x: -6.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),"#,
            r#""transform": (x: -5.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),"#,
        );
        assert_ne!(text, shifted, "the wall's transform was not found to move");
        write_scene(&artifact_dir("moved"), &shifted)
    };

    assert_ne!(
        twice.0,
        hash_of(&moved, "100"),
        "moving the wall one unit did not move the state hash, so the occluder \
         is in the world without being in the hash"
    );

    let resized = {
        let text = std::fs::read_to_string(workspace_root().join(&original))
            .expect("the scene is readable");
        let wider = text.replace(
            r#""occluder": (half_width: 1.0, half_height: 6.0),"#,
            r#""occluder": (half_width: 1.25, half_height: 6.0),"#,
        );
        assert_ne!(text, wider, "the wall's extents were not found to change");
        write_scene(&artifact_dir("resized"), &wider)
    };

    assert_ne!(
        twice.0,
        hash_of(&resized, "100"),
        "widening the wall did not move the state hash"
    );
}

/// **§4(b): a replay reproduces the occluder set exactly.**
///
/// A recording is made over the scene and replayed in a second process. The
/// replay's dump has to be the original's, occluders included — which is
/// ADR-0032's guarantee read from the light's side: a replay answers questions
/// and takes no orders, so the walls it reproduces are the walls that were
/// recorded.
#[test]
fn a_replay_reproduces_the_occluders_it_recorded() {
    let recording = format!("{}/occluders.narvorec", artifact_dir("replay"));

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

    assert_eq!(
        replayed_body,
        dump_of(&scene(), "50"),
        "a replay did not reproduce the world it recorded"
    );
    assert!(
        replayed_body.contains("occluder (half_width:1.0,half_height:6.0)"),
        "the replay reproduced a world with no wall in it:\n{replayed_body}"
    );
}

/// **§3: the occluder set is readable from the simulation side, with no GPU
/// type in the way.**
///
/// The coupling M8.3b exists to create has two consumers and only one of them
/// exists — the image light M8.4 builds. The other is a game reading occluders
/// in its own tick, which is M8.8's, and nothing here is built for it. What is
/// checked is the weaker property that keeps it possible: a reader that links
/// **`narvo-ecs` and nothing else** can find every occluder in a world and its
/// position.
///
/// It is a compile-and-run statement rather than a claim: this test's only
/// import is `narvo_ecs`, it names no renderer type, and it reaches the same
/// three rectangles the scene declares. If a later change made occluders
/// reachable only through the extraction — by moving the type behind
/// `narvo-view2d`, say — this stops compiling.
#[test]
fn a_reader_with_only_the_ecs_can_find_every_occluder() {
    use narvo_ecs::{ComponentRegistry, Occluder, Transform, register_engine_components};

    let mut registry = ComponentRegistry::new();
    register_engine_components(&mut registry).expect("a fresh registry accepts them");
    let world = narvo_scene::from_file(&workspace_root().join(scene()), &registry)
        .expect("the committed scene loads");

    let mut found: Vec<(f32, f32, f32, f32)> = Vec::new();
    for entity in world.entity_ids() {
        let Ok(occluder) = world.get::<Occluder>(entity) else {
            continue;
        };
        let Ok(transform) = world.get::<Transform>(entity) else {
            continue;
        };
        found.push((
            transform.x,
            transform.y,
            occluder.half_width,
            occluder.half_height,
        ));
    }

    assert_eq!(
        found,
        vec![
            (-6.0, 0.0, 1.0, 6.0),
            (5.0, 2.0, 2.0, 2.0),
            (0.0, -4.0, 1.5, 1.5),
        ],
        "a sim-side reader did not see the scene's three occluders in \
         `entity_ids` order"
    );

    // The blocker the crate carries is not its hit area, which is the whole
    // reason a game reading this gets a *light* answer and not a click answer.
    let crate_entity = world.entity_ids()[2];
    let blocker = world
        .get::<Occluder>(crate_entity)
        .expect("the crate blocks light");
    let clickable = world
        .get::<narvo_ecs::HitRect>(crate_entity)
        .expect("the crate is clickable");
    assert_ne!(
        (blocker.half_width, blocker.half_height),
        (clickable.half_width, clickable.half_height),
        "the crate's two rectangles are the same size, so this test would pass \
         even if the reader had picked up the wrong one"
    );
}
