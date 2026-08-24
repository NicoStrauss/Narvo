//! The repro runner, driven as the thing a person and an agent actually run.
//!
//! `crates/narvo-app/src/repro.rs` holds the judgement and its wording, tested
//! in process against strings. This file is the other half: a real binary, real
//! files on disk, and the exit code a script branches on. The division is
//! `tests/determinism.rs`'s and it is here for its reason — what CI ships and
//! what an agent invokes is the executable, and a suite that certified an
//! internal function would certify something nobody runs.
//!
//! # The demonstration in the failure state is the point of this file
//!
//! A repro runner that always passes is decoration, and M6.7b rests entirely on
//! this one working. So the runner is shown **red at a real difference** — a
//! second seed's simulation, a run stopped at another tick — rather than at a
//! constructed assertion, and the tests below are named for those cases rather
//! than being a footnote to the green one.
//!
//! # No expected values here either
//!
//! Nothing in this file holds a state, a hash or a dump. Every expected state is
//! written by a run this same test started, moments earlier, from this same
//! build — which is ADR-0008's rule applied to an instrument that exists to
//! compare against an expectation. What is committed is the *procedure*.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// What a run left behind.
struct Ran {
    /// Whether it exited zero.
    ok: bool,
    /// Everything it wrote to stdout — the report, and nothing else.
    stdout: String,
    /// Everything it wrote to stderr — the summary, and the verdict.
    stderr: String,
}

/// Runs `narvo` and returns what it produced, whether or not it succeeded.
///
/// Unlike `tests/determinism.rs`'s helper this does **not** require success: a
/// repro that fails is the ordinary outcome half these tests are about.
fn narvo(arguments: &[&OsStr]) -> Ran {
    narvo_in(Path::new("."), arguments)
}

/// The same, from a chosen working directory.
///
/// One test needs it: ADR-0019 requires a recording's scene path to be relative
/// and forward-slashed, so that the file means the same thing on another machine.
/// The only way to record against a scene under `CARGO_TARGET_TMPDIR` is
/// therefore to run from there and name it plainly.
fn narvo_in(directory: &Path, arguments: &[&OsStr]) -> Ran {
    let Output {
        status,
        stdout,
        stderr,
    } = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .current_dir(directory)
        .args(arguments)
        .output()
        .expect("the narvo binary must be runnable");

    Ran {
        ok: status.success(),
        stdout: String::from_utf8(stdout).expect("narvo writes UTF-8 to stdout"),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    }
}

/// A directory for one test's artifacts, under the ignored target directory.
///
/// One per test, because nextest runs tests in parallel processes and a shared
/// directory would let one test's recording be overwritten by another's.
fn artifact_dir(test: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("repro")
        .join(test);
    std::fs::create_dir_all(&directory).expect("the output directory must be creatable");
    directory
}

/// Records a run of the input mode and writes down the state it reached.
///
/// The two halves of making a repro, in the order a person makes them: a band
/// that says what was fed, and a dump that says what came out. Returns both
/// paths.
fn make_repro(directory: &Path, seed: &str, ticks: &str) -> (PathBuf, PathBuf) {
    let band = directory.join("bug.rec");
    let expected = directory.join("expected.dump");

    let ran = narvo(&[
        OsStr::new("--mode"),
        OsStr::new("input"),
        OsStr::new("--seed"),
        OsStr::new(seed),
        OsStr::new("--ticks"),
        OsStr::new(ticks),
        OsStr::new("--record"),
        band.as_os_str(),
        OsStr::new("--dump"),
    ]);
    assert!(ran.ok, "recording the run failed: {}", ran.stderr);
    std::fs::write(&expected, ran.stdout).expect("the expected state must be writable");

    (band, expected)
}

/// Replays a band and judges the run against an expected state.
fn repro(band: &Path, expected: &Path, ticks: Option<&str>) -> Ran {
    let mut arguments: Vec<&OsStr> = vec![OsStr::new("--replay"), band.as_os_str()];
    if let Some(ticks) = ticks {
        arguments.push(OsStr::new("--ticks"));
        arguments.push(OsStr::new(ticks));
    }
    arguments.push(OsStr::new("--expect"));
    arguments.push(expected.as_os_str());
    narvo(&arguments)
}

#[test]
fn a_repro_of_a_recorded_run_reproduces_it() {
    let directory = artifact_dir("reproduces");
    let (band, expected) = make_repro(&directory, "1", "500");

    let ran = repro(&band, &expected, None);

    assert!(
        ran.ok,
        "a band replayed against its own state: {}",
        ran.stderr
    );
    assert!(
        ran.stderr.contains("reproduced - after 500 ticks"),
        "{}",
        ran.stderr
    );
}

#[test]
fn a_repro_run_against_another_seeds_simulation_diverges_and_names_where() {
    // **The demonstration in the failure state**, at a difference the engine
    // produces rather than one a test asserts: same mode, same length, one seed
    // apart. If this ever goes green the runner has stopped comparing, and
    // everything M6.7b concludes from it would be worth nothing.
    let directory = artifact_dir("another_seed");
    let (_, expected) = make_repro(&directory, "1", "500");

    let ran = narvo(&[
        OsStr::new("--mode"),
        OsStr::new("input"),
        OsStr::new("--seed"),
        OsStr::new("2"),
        OsStr::new("--ticks"),
        OsStr::new("500"),
        OsStr::new("--expect"),
        expected.as_os_str(),
    ]);

    assert!(
        !ran.ok,
        "a different simulation must not pass: {}",
        ran.stderr
    );
    assert!(
        ran.stderr.contains("diverged - after 500 ticks"),
        "{}",
        ran.stderr
    );
    // The diagnosis, not just the verdict: which entity and which component.
    assert!(ran.stderr.contains("entity    "), "{}", ran.stderr);
    assert!(ran.stderr.contains("component "), "{}", ran.stderr);
    assert!(
        ran.stderr.contains("left is") && ran.stderr.contains("right is this run"),
        "{}",
        ran.stderr
    );
}

#[test]
fn a_repro_stopped_at_another_tick_diverges_and_says_which_tick_it_reached() {
    // The shape a **cut band** takes, and the reason the tick is in every
    // verdict. ADR-0032 records that a band cut by D19 is byte-indistinguishable
    // from an ordinary recording of a shorter run, so nothing can tell a reader
    // that a band stops early — what the runner can do is say how far it got, and
    // that is what this pins.
    let directory = artifact_dir("another_tick");
    let (band, expected) = make_repro(&directory, "1", "500");

    let ran = repro(&band, &expected, Some("400"));

    assert!(!ran.ok, "a shorter run must not pass: {}", ran.stderr);
    assert!(
        ran.stderr.contains("diverged - after 400 ticks"),
        "{}",
        ran.stderr
    );
}

#[test]
fn the_expected_state_is_never_written_by_the_run_that_is_judged() {
    // **The direct check that the oracle cannot come from its own specimen.**
    // `--expect` and `--record` are pointed at one path that does not exist. The
    // expected state is read before the simulation starts, so the run fails
    // there — and the file is still absent afterwards, which is what says the
    // run had no chance to produce what it was judged against.
    let directory = artifact_dir("never_written");
    let path = directory.join("both.file");
    let _ = std::fs::remove_file(&path);

    let ran = narvo(&[
        OsStr::new("--mode"),
        OsStr::new("input"),
        OsStr::new("--ticks"),
        OsStr::new("10"),
        OsStr::new("--record"),
        path.as_os_str(),
        OsStr::new("--expect"),
        path.as_os_str(),
    ]);

    assert!(!ran.ok, "{}", ran.stderr);
    assert!(
        ran.stderr.contains("could not read the expected state"),
        "{}",
        ran.stderr
    );
    assert!(
        !path.exists(),
        "the run wrote {} after being told to expect it",
        path.display()
    );
}

#[test]
fn an_unreadable_expected_state_is_an_error_and_not_a_divergence() {
    // A broken repro and a moved state are two findings, and a script that sees
    // only the exit code would be told the same thing by both. The message is
    // what separates them, so it is what is asserted.
    let directory = artifact_dir("unreadable");
    let (band, _) = make_repro(&directory, "1", "20");

    let ran = repro(&band, &directory.join("nothing-here.dump"), None);

    assert!(!ran.ok, "{}", ran.stderr);
    assert!(
        ran.stderr.contains("could not read the expected state"),
        "{}",
        ran.stderr
    );
    assert!(!ran.stderr.contains("diverged"), "{}", ran.stderr);
    assert!(!ran.stderr.contains("reproduced"), "{}", ran.stderr);
}

#[test]
fn an_empty_expected_state_is_not_a_pass() {
    // The class this repository keeps finding: an answer that looks like one and
    // was never asked the question. An empty file compared against a real dump
    // must not be green, and must not read as a divergence either.
    let directory = artifact_dir("empty");
    let (band, _) = make_repro(&directory, "1", "20");
    let empty = directory.join("empty.dump");
    std::fs::write(&empty, "").expect("an empty file must be writable");

    let ran = repro(&band, &empty, None);

    assert!(!ran.ok, "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("is empty, so this run was compared against nothing"),
        "{}",
        ran.stderr
    );
    assert!(!ran.stderr.contains("diverged"), "{}", ran.stderr);
    assert!(!ran.stderr.contains("reproduced"), "{}", ran.stderr);
}

#[test]
fn an_expected_state_with_a_byte_order_mark_is_refused_by_name() {
    // The commonest way a repro is written on this project's own platform:
    // Windows PowerShell's `>` puts `ef bb bf` in front of the dump. The bytes
    // below are that file, produced here rather than depended on from a shell, so
    // the test says the same thing on Linux.
    let directory = artifact_dir("byte_order_mark");
    let (band, expected) = make_repro(&directory, "1", "20");

    let marked = directory.join("marked.dump");
    let mut bytes = vec![0xef, 0xbb, 0xbf];
    bytes
        .extend_from_slice(&std::fs::read(&expected).expect("the expected state must be readable"));
    std::fs::write(&marked, bytes).expect("the marked file must be writable");

    let ran = repro(&band, &marked, None);

    assert!(!ran.ok, "{}", ran.stderr);
    assert!(
        ran.stderr.contains("begins with a byte-order mark"),
        "{}",
        ran.stderr
    );
    // The whole point: it must not read as a simulation that moved.
    assert!(!ran.stderr.contains("diverged"), "{}", ran.stderr);
}

#[test]
fn a_recording_whose_scene_moved_is_refused_before_anything_is_judged() {
    // ADR-0019 reaching the repro runner. The anchor is checked before a plan
    // exists, so a band that no longer matches its scene never reaches a
    // comparison — and the message must be the anchor's, naming the file and both
    // digests, rather than a verdict about a state nobody computed.
    let directory = artifact_dir("scene_moved");
    let scene = directory.join("case.ron");
    std::fs::copy("scenes/determinism-case.ron", &scene)
        .expect("the determinism scene must be readable from the package root");

    let recorded = narvo_in(
        &directory,
        &[
            OsStr::new("--mode"),
            OsStr::new("scene-file"),
            OsStr::new("--scene"),
            OsStr::new("case.ron"),
            OsStr::new("--ticks"),
            OsStr::new("200"),
            OsStr::new("--record"),
            OsStr::new("scene.rec"),
            OsStr::new("--dump"),
        ],
    );
    assert!(recorded.ok, "{}", recorded.stderr);

    let expected = directory.join("expected.dump");
    std::fs::write(&expected, recorded.stdout).expect("the expected state must be writable");

    // A change the world never sees, which is the point: the anchor is over the
    // file's bytes, so even a comment makes the recording refuse.
    let mut text = std::fs::read_to_string(&scene).expect("the copy must be readable");
    text.push_str("\n// a comment the world does not see\n");
    std::fs::write(&scene, text).expect("the copy must be writable");

    let ran = narvo_in(
        &directory,
        &[
            OsStr::new("--replay"),
            OsStr::new("scene.rec"),
            OsStr::new("--expect"),
            OsStr::new("expected.dump"),
        ],
    );

    assert!(!ran.ok, "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("is not the one this recording was made against"),
        "{}",
        ran.stderr
    );
    assert!(!ran.stderr.contains("diverged"), "{}", ran.stderr);
    assert!(!ran.stderr.contains("reproduced"), "{}", ran.stderr);
}

#[test]
fn the_report_still_reaches_stdout_when_the_verdict_is_a_divergence() {
    // `--expect` decides the exit code and `--dump` decides stdout, and a run may
    // do both — which is how a divergence is bisected without a second run. The
    // invariant that stdout carries the report and nothing else holds for a
    // failing repro too.
    let directory = artifact_dir("stdout_intact");
    let (band, expected) = make_repro(&directory, "1", "500");

    let ran = narvo(&[
        OsStr::new("--replay"),
        band.as_os_str(),
        OsStr::new("--ticks"),
        OsStr::new("400"),
        OsStr::new("--expect"),
        expected.as_os_str(),
        OsStr::new("--dump"),
    ]);

    assert!(!ran.ok, "{}", ran.stderr);
    assert!(
        ran.stdout.starts_with("entities "),
        "stdout has to be the dump and nothing else, got {:?}",
        ran.stdout.chars().take(40).collect::<String>()
    );
    assert!(
        !ran.stdout.contains("diverged"),
        "the verdict belongs on stderr"
    );
}
