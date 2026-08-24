//! What a recording of the drop scene actually carries — and what it does not.
//!
//! # The narrowing, made in M5b.4 and written here rather than implied
//!
//! ADR-0012 built the recording format so that **inputs** are reproduced: a run
//! writes down what arrived from outside the simulation, and a replay feeds the
//! same list back. `scenes/physics_drop.ron` has no input at all — a scene-file
//! world consumes none (`sim::pilot` returns an empty list for `Mode::SceneFile`,
//! `sim::feed` is unreachable for it, and `sim::validate_recording` refuses a
//! recording that carries any). So a recording of this scene is a header and
//! nothing else, and **replaying it cannot demonstrate input reproduction**,
//! because there is no input to reproduce.
//!
//! Saying that a replay of this scene "reproduces its original" is therefore a
//! two-run comparison wearing a replay's clothes: the same binary builds the same
//! world from the same file and steps it the same number of times, which
//! `sim::scene_file`'s own tests already assert without a recording anywhere.
//!
//! **What the replay does add is the scene's identity** (ADR-0019). The recording
//! names the file by path and by the SHA-256 of its bytes, and a replay checks
//! that hash before it constitutes anything. That is a property no two-run
//! comparison has and no state hash can express: it is what makes a recording a
//! repro of *this* content rather than of whatever is at that path today. The two
//! tests below are exactly that claim and its flank.
//!
//! # Why the flank tampers with the recording and not with the scene
//!
//! Both directions produce `AnchorError::Mismatch` and either would do. The
//! recording is written by the test into its own artifact directory; the scene is
//! a committed file, and a test that edited one and put it back would be the M2.6
//! trap with a timer on it.
//!
//! # What would make a replay of a physics scene say more, and why it is not here
//!
//! An input the scene could consume — a hit rectangle on a body, a click that
//! pushes it. That needs the scene-file mode to consume input headlessly, which
//! today it cannot: `Mode::actions()` is a fixed list per mode and a scene's
//! actions are content, so making one work means deciding what a scene-file
//! mode's action vocabulary *is*. That is a decision with an ADR in it, not a
//! line in this file, and M5b.4 reports it rather than smuggling it in beside a
//! reference image.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The scene, relative to this package's directory.
///
/// **Relative on purpose, and it has to be**: a recording carries its scene path
/// in a normal form and `Anchor::read` refuses an absolute one outright, so that
/// the file means the same thing on another machine and another platform. An
/// integration test runs with the package root as its working directory, so this
/// is the path from there.
const SCENE: &str = "scenes/physics_drop.ron";

/// Ticks the recording covers. Short: nothing here is about how far the scene
/// runs, and the picture's own tick is asserted where the picture is.
const TICKS: &str = "60";

/// Runs `narvo`, requiring it to succeed, and returns its stdout.
fn narvo(arguments: &[&OsStr]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args(arguments)
        .output()
        .expect("the narvo binary must be runnable");

    assert!(
        output.status.success(),
        "narvo {} failed: {}",
        arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "),
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).expect("narvo writes UTF-8 to stdout")
}

/// A directory for one test's artifacts, under the ignored target directory.
fn artifact_dir(test: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    std::fs::create_dir_all(&directory).expect("the output directory must be creatable");
    directory
}

/// Records a run of the drop scene and returns the recording's path.
fn record(into: &Path) -> PathBuf {
    let path = into.join("drop.rec");
    narvo(&[
        OsStr::new("--mode"),
        OsStr::new("scene-file"),
        OsStr::new("--scene"),
        OsStr::new(SCENE),
        OsStr::new("--ticks"),
        OsStr::new(TICKS),
        OsStr::new("--record"),
        path.as_os_str(),
    ]);
    path
}

/// The state hash of a live run of the scene.
fn live_hash() -> String {
    narvo(&[
        OsStr::new("--mode"),
        OsStr::new("scene-file"),
        OsStr::new("--scene"),
        OsStr::new(SCENE),
        OsStr::new("--ticks"),
        OsStr::new(TICKS),
        OsStr::new("--hash"),
    ])
}

/// The recording names the scene by path and digest, and carries no input.
///
/// Both halves are the point. The anchor is what a replay checks; the emptiness
/// is what bounds the claim, and asserting it here is what keeps a later reader
/// from believing this file demonstrates input reproduction.
#[test]
fn a_recording_of_the_drop_scene_names_its_scene_and_carries_no_input() {
    let directory = artifact_dir("drop-replay-anchor");
    let recording = record(&directory);
    let text = std::fs::read_to_string(&recording).expect("just written");

    let digest = {
        let bytes = std::fs::read(SCENE).expect("the committed scene reads");
        narvo_core::sha256::hex(&narvo_core::sha256::sha256(&bytes))
    };

    assert!(
        text.contains(&format!("scene {SCENE}\n")),
        "the recording does not name the scene it was made against:\n{text}"
    );
    assert!(
        text.contains(&format!("scene-sha256 {digest}\n")),
        "the recording's digest is not the scene file's own {digest}:\n{text}"
    );
    assert!(
        text.lines().all(|line| !line.starts_with(char::is_numeric)),
        "a scene-file run consumes no input, so its recording must have no input \
         lines:\n{text}"
    );

    // And it is a recording that replays, rather than only a file that parses.
    let replayed = narvo(&[
        OsStr::new("--replay"),
        recording.as_os_str(),
        OsStr::new("--hash"),
    ]);
    assert_eq!(
        replayed,
        live_hash(),
        "the replay did not reach the state the live run reached"
    );

    std::fs::remove_dir_all(&directory).ok();
}

/// **The flank: a recording whose anchor no longer matches its scene is
/// refused**, and the refusal names both digests.
///
/// Without this the test above is satisfied by a build that writes the anchor and
/// never reads it — which would look exactly the same from outside, right up to
/// the day somebody edited the scene and the replay silently started from another
/// world.
#[test]
fn a_replay_whose_anchor_does_not_match_the_scene_is_refused() {
    let directory = artifact_dir("drop-replay-tampered");
    let recording = record(&directory);
    let text = std::fs::read_to_string(&recording).expect("just written");

    // One hex digit of the digest, changed. The smallest edit that makes the
    // recording describe a different file.
    let line = text
        .lines()
        .find(|line| line.starts_with("scene-sha256 "))
        .expect("a scene-file recording carries its digest");
    let digest = line
        .strip_prefix("scene-sha256 ")
        .expect("just matched the prefix");
    let flipped = format!(
        "{}{}",
        if digest.starts_with('0') { '1' } else { '0' },
        &digest[1..]
    );

    let tampered = directory.join("tampered.rec");
    std::fs::write(
        &tampered,
        text.replace(line, &format!("scene-sha256 {flipped}")),
    )
    .expect("writable");

    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args([OsStr::new("--replay"), tampered.as_os_str()])
        .arg("--hash")
        .output()
        .expect("the narvo binary must be runnable");

    assert!(
        !output.status.success(),
        "replaying a recording whose anchor does not match its scene has to fail; \
         it succeeded and printed {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&flipped) && stderr.contains(digest),
        "the refusal has to name what was expected and what was found, so that a \
         reader can tell a moved file from a changed one. It said:\n{stderr}"
    );

    std::fs::remove_dir_all(&directory).ok();
}
