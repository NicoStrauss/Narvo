//! The determinism suite: everything one machine can check about reproducibility.
//!
//! These drive the `narvo` binary rather than calling into it. That is partly
//! forced — `narvo-app` is a binary crate with no library target, so a file in
//! `tests/` can only spawn it — and partly the point: what CI ships and what an
//! agent runs is the executable, and a suite that certified an internal API
//! would be certifying something nobody uses.
//!
//! # What is checked where
//!
//! - **Here:** the end-to-end properties, through the process boundary and
//!   through real files on disk. Two runs agree; different seeds do not; a
//!   replay reproduces its original at five checkpoints; a tampered recording
//!   does not; a recording survives a round trip and replays in another process.
//! - **In `headless.rs` and `recording.rs`:** the same properties in process,
//!   where they are faster to run and closer to the code that would break them.
//!   The overlap is deliberate. These catch a break in the binary or in the file
//!   path that a unit test would not see.
//! - **In `xtask determinism`:** the one question no test can answer, because a
//!   test observes only the platform it runs on — whether Windows and Linux
//!   agree. See that module; it runs as its own CI job.
//!
//! # No expected values
//!
//! Nothing here asserts a hash. Every check compares two runs produced by this
//! same build, which is what ADR-0008 requires and why a `ron` or serde release
//! moves both sides together instead of turning the suite red for nothing.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Ticks the reproducibility checks run for.
///
/// The closing criterion of §6/M2 names ten thousand, so that is what the suite
/// runs; anything shorter would be testing something the milestone did not ask
/// about. It costs a few milliseconds per run — see the measurements in the M2.4
/// report — which is why no second, shorter tier was needed.
const TICKS: &str = "10000";

/// Where a replay is compared against its original.
///
/// The end alone would say *that* two runs diverged and never *when*. With a
/// physics solver arriving in M5 that difference is the whole value of the
/// instrument, so the checkpoints are part of the suite rather than a debugging
/// aid somebody might reach for later.
const CHECKPOINTS: [&str; 5] = ["1", "100", "1000", "5000", "10000"];

/// Every simulation mode the binary offers.
///
/// `scene` is the one that draws, and it is here because a scenario whose
/// picture depends on state the hash cannot see would be a replay in name only.
/// It registers `Transform`, `Layer`, `Sampling`, `Camera`, `Follow` and
/// `Shake`, so all six are in the dump these tests compare — which is what makes
/// it the first *drawing* scenario the determinism suite covers. It runs here at
/// `scene::SPRITES` rather than at the 50 000 the frame-time measurement uses;
/// the duty is about which components are registered, not how many entities
/// carry them.
/// **`physics` is deliberately not here, and the reason belongs beside the
/// list.** M5b.3b put it in the *cross-platform* matrix instead, which is where
/// its evidence had to come from: WSL and Windows share a CPU, so only two CI
/// runners can say whether the solver agrees between machines. Adding it here as
/// well would run it five more times at ten thousand ticks — about 1.8 s on the
/// reference machine, measured — for a property its own tests in
/// `sim::physics` already assert at a shorter length. That is a cost decision
/// and it is written down rather than left to be inferred from an absence.
///
/// The same courtesy the other way round is now mechanical: `xtask`'s
/// `every_mode_is_in_the_matrix_or_excused_in_writing` fails if a mode leaves
/// the cross-platform matrix without a recorded reason. **This list has no such
/// guard**, so this comment is the whole of it.
const MODES: [&str; 4] = ["motion", "chance", "input", "scene"];

/// The modes whose state a seed can reach.
const SEEDED_MODES: [&str; 2] = ["chance", "input"];

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

/// The state hash of a live run.
fn hash(mode: &str, seed: &str, ticks: &str) -> String {
    narvo(&[
        OsStr::new("--mode"),
        OsStr::new(mode),
        OsStr::new("--seed"),
        OsStr::new(seed),
        OsStr::new("--ticks"),
        OsStr::new(ticks),
        OsStr::new("--hash"),
    ])
}

/// The canonical dump of a live run.
fn dump(mode: &str, seed: &str, ticks: &str) -> String {
    narvo(&[
        OsStr::new("--mode"),
        OsStr::new(mode),
        OsStr::new("--seed"),
        OsStr::new(seed),
        OsStr::new("--ticks"),
        OsStr::new(ticks),
        OsStr::new("--dump"),
    ])
}

/// Records a run and returns the path of the recording.
fn record(directory: &Path, mode: &str, seed: &str, ticks: &str, name: &str) -> PathBuf {
    let path = directory.join(name);
    narvo(&[
        OsStr::new("--mode"),
        OsStr::new(mode),
        OsStr::new("--seed"),
        OsStr::new(seed),
        OsStr::new("--ticks"),
        OsStr::new(ticks),
        OsStr::new("--record"),
        path.as_os_str(),
    ]);
    path
}

/// The state hash of a replay, optionally stopped early.
fn replay_hash(recording: &Path, ticks: Option<&str>) -> String {
    let mut arguments: Vec<&OsStr> = vec![OsStr::new("--replay"), recording.as_os_str()];
    if let Some(ticks) = ticks {
        arguments.push(OsStr::new("--ticks"));
        arguments.push(OsStr::new(ticks));
    }
    arguments.push(OsStr::new("--hash"));
    narvo(&arguments)
}

/// Fails naming *where* two states first differ, instead of printing both.
///
/// A dump is a couple of thousand bytes, so `assert_eq!` on two of them buries
/// the one changed number in a wall of text that has to be diffed by eye. The
/// locating is [`narvo_ecs::first_difference`]'s — the dump is that crate's
/// format, so the tool that reads it belongs beside the one that writes it.
/// All this adds is the caller's own label.
///
/// This file can reach `narvo_ecs` because cargo hands a package's
/// `[dependencies]` to every target it builds, integration tests included. It
/// cannot reach `narvo-app`'s own modules — a binary crate has no library
/// target — which is why the five lines below exist twice in this workspace.
fn assert_same_state(label: &str, left: &str, right: &str) {
    if let Some(difference) = narvo_ecs::first_difference(left, right) {
        panic!("{label}: the two states differ.\n{difference}");
    }
}

/// A directory for one test's artifacts, under the ignored target directory.
///
/// One per test, because nextest runs tests in parallel processes and a test
/// that tidies up after itself would otherwise delete a file another is using.
fn artifact_dir(test: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    std::fs::create_dir_all(&directory).expect("the output directory must be creatable");
    directory
}

#[test]
fn every_mode_is_reproducible_over_ten_thousand_ticks() {
    // The first half of the closing criterion in §6/M2, as a test rather than as
    // a line in a report.
    for mode in MODES {
        assert_same_state(
            &format!("mode {mode}, two runs of {TICKS} ticks from the same seed"),
            &dump(mode, "1", TICKS),
            &dump(mode, "1", TICKS),
        );
        assert_eq!(hash(mode, "1", TICKS), hash(mode, "1", TICKS));
    }
}

#[test]
fn ten_thousand_ticks_change_every_mode() {
    // Without this the test above is satisfied by three simulations that do
    // nothing at all.
    for mode in MODES {
        assert_ne!(
            dump(mode, "1", "0"),
            dump(mode, "1", TICKS),
            "mode {mode} is unchanged after {TICKS} ticks"
        );
    }
}

#[test]
fn different_seeds_produce_different_states() {
    // The negative half of reproducibility: a generator returning a constant
    // would pass every comparison above.
    for mode in SEEDED_MODES {
        assert_ne!(
            hash(mode, "1", TICKS),
            hash(mode, "2", TICKS),
            "mode {mode} ignores its seed"
        );
    }
}

#[test]
fn a_seed_cannot_reach_the_mode_that_has_nothing_random_in_it() {
    assert_eq!(hash("motion", "1", TICKS), hash("motion", "999999", TICKS));
}

#[test]
fn a_replay_reproduces_its_original_at_every_checkpoint() {
    // The second half of the closing criterion, sharpened: not only the same end
    // state, the same state all the way along.
    let directory = artifact_dir("determinism-checkpoints");
    let recording = record(&directory, "input", "1", TICKS, "run.rec");

    for ticks in CHECKPOINTS {
        let original = hash("input", "1", ticks);
        let replayed = replay_hash(&recording, Some(ticks));

        assert_eq!(
            replayed, original,
            "the replay diverges from its original by tick {ticks}"
        );
    }

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_full_length_replay_reproduces_the_whole_dump() {
    let directory = artifact_dir("determinism-full-replay");
    let recording = record(&directory, "input", "1", TICKS, "run.rec");

    let replayed = narvo(&[
        OsStr::new("--replay"),
        recording.as_os_str(),
        OsStr::new("--dump"),
    ]);

    assert_same_state(
        "mode input, a full-length replay against its original",
        &dump("input", "1", TICKS),
        &replayed,
    );
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_tampered_recording_produces_a_different_state() {
    // The check that gives every other replay test its meaning: without it, a
    // "replay" that ignored the file and simulated again from the seed would
    // pass all of them.
    let directory = artifact_dir("determinism-tampered");
    let recording = record(&directory, "input", "1", TICKS, "run.rec");
    let original = replay_hash(&recording, None);

    let text = std::fs::read_to_string(&recording).expect("just written");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let body = lines
        .iter()
        .position(|line| line.starts_with(char::is_numeric))
        .expect("a run of this length always records input");

    // One value, on one line, of one tick.
    let fields: Vec<&str> = lines[body].split_whitespace().collect();
    let value: i64 = fields[2].parse().expect("the value is a number");
    lines[body] = format!("{} {} {}", fields[0], fields[1], value + 1);

    let tampered = directory.join("tampered.rec");
    std::fs::write(&tampered, format!("{}\n", lines.join("\n"))).expect("writable");

    assert_ne!(
        replay_hash(&tampered, None),
        original,
        "changing one recorded input did not change the state, so the replay is \
         not reading the file"
    );

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn dropping_one_recorded_input_changes_the_state() {
    let directory = artifact_dir("determinism-dropped");
    let recording = record(&directory, "input", "1", TICKS, "run.rec");
    let original = replay_hash(&recording, None);

    let text = std::fs::read_to_string(&recording).expect("just written");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let body = lines
        .iter()
        .position(|line| line.starts_with(char::is_numeric))
        .expect("a run of this length always records input");
    lines.remove(body);

    let shortened = directory.join("shortened.rec");
    std::fs::write(&shortened, format!("{}\n", lines.join("\n"))).expect("writable");

    assert_ne!(replay_hash(&shortened, None), original);
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn recording_one_run_twice_produces_the_same_file() {
    // Reproducibility of the artifact itself, through the file path: the same
    // run written twice is the same bytes, so a recording can be compared
    // between two machines without normalising anything first. That is what the
    // cross-platform job in `xtask determinism` relies on.
    //
    // Two neighbouring properties are checked elsewhere, because the command
    // line cannot express them: that a *replay* reproduces the recording it was
    // given is `headless.rs`'s `a_replay_produces_the_recording_it_was_given`
    // (`--replay` and `--record` are a deliberate conflict — a run either
    // consumes a recording or produces one), and that the text round-trips
    // through the parser is `recording.rs`.
    let directory = artifact_dir("determinism-recording-twice");
    let first = record(&directory, "input", "1", TICKS, "first.rec");
    let second = record(&directory, "input", "1", TICKS, "second.rec");

    let bytes = std::fs::read(&first).expect("written");
    assert_eq!(
        bytes,
        std::fs::read(&second).expect("written"),
        "two recordings of one run differ"
    );
    assert!(
        !bytes.contains(&b'\r'),
        "a recording is LF on every platform"
    );

    // ... and it is a recording that actually replays, not just a file.
    assert_eq!(replay_hash(&first, None), hash("input", "1", TICKS));

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_recording_replays_in_a_process_that_did_not_produce_it() {
    // Every invocation here is its own process, so this is the shape a repro
    // travels in: one machine writes the file, another reads it. The
    // cross-*platform* case is the same idea one step further and lives in
    // `xtask determinism`, because a test cannot reach the other platform.
    let directory = artifact_dir("determinism-cross-process");
    let recording = record(&directory, "input", "1", TICKS, "run.rec");

    assert_eq!(replay_hash(&recording, None), hash("input", "1", TICKS));
    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_mode_with_no_input_records_an_empty_recording_and_replays_it() {
    let directory = artifact_dir("determinism-empty-recording");

    for mode in ["motion", "chance"] {
        let recording = record(&directory, mode, "1", TICKS, &format!("{mode}.rec"));
        let text = std::fs::read_to_string(&recording).expect("written");

        assert!(
            text.lines().all(|line| !line.starts_with(char::is_numeric)),
            "mode {mode} should record no input, got:\n{text}"
        );
        assert_eq!(
            replay_hash(&recording, None),
            hash(mode, "1", TICKS),
            "mode {mode} does not replay"
        );
    }

    std::fs::remove_dir_all(&directory).ok();
}

#[test]
fn a_replay_may_not_run_past_the_end_of_its_recording() {
    // The defensive half. A replay that ran on with no input would look like a
    // longer reproduction of something that was never recorded.
    let directory = artifact_dir("determinism-too-short");
    let recording = record(&directory, "input", "1", "100", "run.rec");

    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args([OsStr::new("--replay"), recording.as_os_str()])
        .args([OsStr::new("--ticks"), OsStr::new("101")])
        .output()
        .expect("the narvo binary must be runnable");

    assert!(
        !output.status.success(),
        "running past the recording must fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("100"),
        "the error must name the length: {stderr}"
    );

    std::fs::remove_dir_all(&directory).ok();
}
