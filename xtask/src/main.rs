//! `cargo xtask ci` — one command for this repository's verification set.
//!
//! The set is eleven cargo invocations (see the *Verification set* section of
//! `CLAUDE.md`). Typing eleven commands costs an agent turns and, worse, allows
//! one of them to be quietly skipped: a task that reports "green" after nine of
//! eleven is indistinguishable from one that ran all eleven. This binary runs
//! them in the documented order, stops at the first failure, and names the
//! failing command in a form that can be pasted back into a shell.
//!
//! The count in that paragraph was **fourteen** until U2 and is a comment rather
//! than a checked value — nothing in this file compares it to
//! [`VERIFICATION_SET`], so it went stale the moment the set shrank and no test
//! would have said so. It is corrected here by hand for the same reason
//! `CLAUDE.md`'s opening rule gives: a comment outlives the change that made it
//! wrong, and the next reader cannot tell a checked number from a plausible one.
//!
//! What it deliberately does *not* do: change the set. No substitutions, no
//! merging of steps, no reordering, no cleverness about what could be skipped.
//! The entire value of the tool is that a local run and a CI run do the same
//! thing, and any divergence makes it worse than eleven typed commands. Two tests
//! below enforce that against `CLAUDE.md` and against the CI workflow.
//!
//! Output policy: every child's stdout and stderr is inherited and passed
//! through unfiltered, because compiler and test output *is* the payload — an
//! agent needs the errors verbatim. This binary's own progress and failure
//! messages go to stderr, the same stream cargo reports on.
//!
//! **That last sentence used to end "so the interleaving stays in order when the
//! run is piped to a file", and U2 measured that to be false across the WSL
//! bridge.** Redirecting a `wsl -e … cargo xtask ci` to a file lost the headers
//! of steps one and two entirely: this binary's `eprintln!` is unbuffered while
//! a child cargo's stdout is block-buffered, and the relay does not preserve the
//! order between them. On Windows the ordering does hold, which is why the claim
//! survived this long. Read a redirected WSL log for its *content* — the
//! `Compiling` lines and the final step list — and not for the position of a
//! header within it.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod determinism;
mod whitespace;

/// One step of the verification set.
struct Step {
    /// The command line without the leading `cargo`.
    ///
    /// Rendered back by [`Step::command_line`] it is byte for byte the line in
    /// `CLAUDE.md`, which is what makes a failure message copy-pasteable and
    /// what `every_step_is_documented_in_claude_md` checks.
    argv: &'static [&'static str],
    /// Substrings that all have to appear in `.github/workflows/ci.yml` for this
    /// step to count as covered there.
    ///
    /// They are not identical to [`Step::argv`], because CI runs the same set
    /// with `--locked` and, for cargo-deny, through a prebuilt action rather
    /// than a shell command. See `every_step_of_the_verification_set_also_runs_in_ci`
    /// for what that test does and does not catch.
    ///
    /// Only the tests read this, so the plain binary build sees it as dead. The
    /// allow is scoped to that build rather than blanket, so the field going
    /// genuinely unused - the tests dropping it - still warns.
    #[cfg_attr(not(test), allow(dead_code))]
    ci_markers: &'static [&'static str],
    /// Crate names that must not appear in the step's own output.
    ///
    /// Empty for every step whose verdict is its exit code. The one step where
    /// it is not empty is `cargo tree`, which succeeds whatever it prints — so
    /// the *content* is the check, exactly as CI's `grep -Eiw` makes it.
    forbidden: &'static [&'static str],
}

impl Step {
    /// The step as a shell line, exactly as `CLAUDE.md` documents it.
    fn command_line(&self) -> String {
        format!("cargo {}", self.argv.join(" "))
    }
}

/// Crate names the headless dependency tree may not contain.
///
/// **The same list `.github/workflows/ci.yml` greps for**, and
/// `the_forbidden_crates_are_the_ones_ci_greps_for` holds the two together by
/// reading the workflow's `grep -Eiw` line. That test is what makes this a
/// shared decision rather than a second implementation free to drift — see
/// [`VERIFICATION_SET`]'s note on why the step is reproduced here at all.
///
/// # Why `kira` and `cpal` joined the list in M5.6c
///
/// The first five are graphics, gated behind `render`. Audio is a second gate of
/// exactly the same shape: `narvo-audio` turns its `device` feature on by
/// default so that `cargo deny check` always resolves kira, and the workspace
/// routes the crate with `default-features = false` so a headless `narvo-app`
/// gets the cue vocabulary without a backend. Two settings in two manifests hold
/// that apart, and either of them being changed by hand would drag an audio
/// stack — and on Linux an ALSA link dependency — into a build that is supposed
/// to run on a machine with no devices at all.
///
/// That is the same failure the graphics half of this list exists to catch, so
/// it is caught the same way rather than by a second mechanism.
const FORBIDDEN_IN_HEADLESS: &[&str] = &[
    "wgpu",
    "winit",
    "naga",
    "raw-window-handle",
    "image",
    "kira",
    "cpal",
];

/// The verification set, in the order `CLAUDE.md` defines it.
///
/// This is the single place the order and the flags live. Changing it means
/// changing `CLAUDE.md` and `.github/workflows/ci.yml` in the same commit; the
/// tests at the bottom of this file fail otherwise.
///
/// # The last three, and why they are reproduced rather than shared (M4.9)
///
/// Steps seven to nine are the headless configuration. They were CI-only until
/// M4.9, and that gap shipped a defect: M4.8 added an integration test that
/// imported an optional, render-gated crate without the matching
/// `#![cfg(feature = "render")]`, `cargo xtask ci` was green, and CI failed on
/// both platforms (run 31444728096). A local run that says "green" while three
/// CI steps have never been executed is the failure mode this binary exists to
/// prevent, applied to itself.
///
/// **Reproduced here rather than shared, and the alternatives were weighed:**
///
/// - *Have CI call `cargo xtask ci`.* One definition, no drift — but CI would
///   lose its per-step annotations, a failing clippy and a failing test would
///   report as one red step, and the workflow's own structure (matrix, caching,
///   the Linux-only condition on the tree check) would have to move into this
///   binary.
/// - *Have this binary read `ci.yml`.* One definition again — but it means
///   parsing YAML, and this crate's manifest keeps its dependencies deliberately
///   empty for reasons its own comment gives.
/// - *Reproduce, and test that the two agree.* What this repository already
///   chose for the steps it already had: [`Step::ci_markers`] and
///   `every_step_of_the_verification_set_also_runs_in_ci`. The mechanism exists,
///   it has held since M2, and extending it costs nothing new.
///
/// So the third. The drift the prompt worries about is real, and it is
/// converted into a failing test rather than argued away.
///
/// Two deliberate differences from CI, both in the safe direction: CI appends
/// `--locked` and runs nextest with `--profile ci` (the same difference every
/// other step already has, and for the same reason — a local run may update
/// `Cargo.lock` as part of the work), and CI runs the tree check on Linux only
/// while this runs it always.
const VERIFICATION_SET: &[Step] = &[
    Step {
        argv: &["build", "--workspace"],
        ci_markers: &["cargo build --workspace --locked"],
        forbidden: &[],
    },
    Step {
        argv: &["nextest", "run", "--workspace"],
        ci_markers: &["cargo nextest run --workspace --locked"],
        forbidden: &[],
    },
    Step {
        argv: &["test", "--doc", "--workspace"],
        ci_markers: &["cargo test --doc --workspace --locked"],
        forbidden: &[],
    },
    Step {
        argv: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        ci_markers: &["cargo clippy --workspace --all-targets --locked -- -D warnings"],
        forbidden: &[],
    },
    Step {
        argv: &["fmt", "--all", "--check"],
        ci_markers: &["cargo fmt --all --check"],
        forbidden: &[],
    },
    Step {
        argv: &["deny", "check"],
        // CI runs cargo-deny through Embark's action instead of the bare
        // command, so both halves are pinned: the action, and the subcommand it
        // is told to run.
        ci_markers: &["EmbarkStudios/cargo-deny-action", "command: check"],
        forbidden: &[],
    },
    Step {
        argv: &["build", "-p", "narvo-app", "--no-default-features"],
        ci_markers: &["cargo build -p narvo-app --no-default-features --locked"],
        forbidden: &[],
    },
    Step {
        argv: &["nextest", "run", "-p", "narvo-app", "--no-default-features"],
        ci_markers: &["cargo nextest run -p narvo-app --no-default-features --locked --profile ci"],
        forbidden: &[],
    },
    Step {
        argv: &[
            "tree",
            "-p",
            "narvo-app",
            "--no-default-features",
            "--edges",
            "normal",
        ],
        ci_markers: &["cargo tree -p narvo-app --no-default-features --edges normal --locked"],
        forbidden: FORBIDDEN_IN_HEADLESS,
    },
    // The tenth, added in M6.3d. D20 put the agent transport behind a feature
    // that is off by default, so the two configurations above both have it off
    // and the one that has it on was built by nothing. That is the M4.8 shape
    // steps seven to nine exist to prevent, one gate along — and it was not
    // hypothetical: the first `--features ipc` build of that task reported an
    // unused item that no other step could have seen.
    Step {
        argv: &["nextest", "run", "-p", "narvo-app", "--features", "ipc"],
        ci_markers: &["cargo nextest run -p narvo-app --features ipc --locked --profile ci"],
        forbidden: &[],
    },
    // The eleventh, added in V4, and the fourth step in this list that exists
    // because a configuration was checked by nothing. `cargo clippy --workspace`
    // unifies the features of the members it builds: `narvo-app`'s `render`
    // turns `narvo-audio/device` on, so every workspace-wide step sees that
    // crate *with* a device and the configuration without one is linted by
    // nobody. What that hid was measured rather than assumed — three `Mishap`
    // variants are constructed only in the device-gated `kira_sink.rs`, so
    // `narvo-audio` without `device` carried a dead_code warning while step
    // four reported green.
    //
    // One crate rather than a matrix. A crate with n optional features has 2^n
    // configurations and this set checks five; V4 added the one a finding names
    // and measured the rest to be clean, so the boundary is a measurement and
    // not a preference. The wider cut was priced in the same measurement:
    // linting the whole headless configuration costs 93 packages and 34.7 s cold
    // against this step's 1 package and 3.3 s, and it catches nothing further
    // once this crate is clean.
    Step {
        argv: &[
            "clippy",
            "-p",
            "narvo-audio",
            "--no-default-features",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
        ci_markers: &[
            "cargo clippy -p narvo-audio --no-default-features --all-targets --locked -- -D warnings",
        ],
        forbidden: &[],
    },
    // **Three steps stood here until U2, and their absence is the statement.**
    // The twelfth ran the headless game, the thirteenth read its tree and the
    // fourteenth linted it. All three named `forge-loop`, which was a workspace
    // member from M7.1 until U2 and is now a standalone crate outside this
    // repository. A step naming a package the workspace does not have is not a
    // weaker check; it is a build error.
    //
    // **What the three were for did not go away with them — it moved.** They
    // existed because a game's `render` feature is on by default, so
    // `cargo clippy --workspace` unified it on for everybody and the headless
    // configuration was checked by nobody. That reasoning now belongs to whoever
    // builds the game, and one half of it had to travel with the manifest: the
    // standalone crate declares `narvo-audio` with `default-features = false`
    // itself, which the workspace used to declare on its behalf.
    //
    // That last sentence is written here rather than left implicit, because this
    // is the one place the fact could be lost. A member cannot override an
    // inherited dependency's `default-features` — cargo refuses and says so in
    // as many words — which is why the routed form lived at the workspace at
    // all. Outside a workspace there is nothing to inherit from, so the same
    // words have to stand in the game's own manifest or its headless build
    // quietly acquires an audio device. The step that would have caught that is
    // one of the three this comment replaces.
];

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.as_slice() {
        [task] if task == "ci" => run_ci(),
        [task] if task == "whitespace" => run_whitespace(),
        [task, sub, directory] if task == "determinism" && sub == "record" => {
            determinism::record(Path::new(directory))
        }
        [task, sub, left, right] if task == "determinism" && sub == "compare" => {
            determinism::compare(Path::new(left), Path::new(right))
        }
        _ => usage(&args),
    }
}

/// Runs every step in order and stops at the first failure.
fn run_ci() -> ExitCode {
    // Cargo sets CARGO to its own path for the processes it spawns, so a run
    // started by `cargo xtask` keeps using the same toolchain rather than
    // whatever a rustup shim on PATH would resolve to.
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let root = workspace_root();
    let total = VERIFICATION_SET.len();

    // First, because it costs milliseconds and because the class it catches
    // survives every other step in this list: a collapsed line continuation
    // compiles, passes clippy, and is invisible to a `contains` assertion.
    // Finding it before a five-minute build is the whole reason it goes here
    // rather than at the end.
    eprintln!();
    eprintln!("=== xtask ci [prelude] cargo xtask whitespace");
    if run_whitespace() != ExitCode::SUCCESS {
        eprintln!();
        eprintln!("xtask: the whitespace check failed; none of the {total} steps ran.");
        return ExitCode::FAILURE;
    }

    for (index, step) in VERIFICATION_SET.iter().enumerate() {
        let number = index + 1;
        let line = step.command_line();
        eprintln!();
        eprintln!("=== xtask ci [{number}/{total}] {line}");

        // A step with a forbidden list is judged on what it printed rather than
        // on its exit code, so its output has to be captured. It is echoed
        // afterwards, unchanged, because CI echoes it too and because a reader
        // needs the tree to see what pulled a crate in.
        if !step.forbidden.is_empty() {
            match run_forbidding(&cargo, &root, step, number, total) {
                Ok(()) => continue,
                Err(code) => return code,
            }
        }

        let status = Command::new(&cargo)
            .args(step.argv)
            .current_dir(&root)
            .status();

        let status = match status {
            Ok(status) => status,
            Err(error) => {
                eprintln!();
                eprintln!("xtask: step {number} of {total} could not be started: {error}");
                eprintln!("xtask: it would have been");
                eprintln!();
                eprintln!("    {line}");
                eprintln!();
                return ExitCode::FAILURE;
            }
        };

        if !status.success() {
            let remaining = total - number;
            eprintln!();
            match status.code() {
                Some(code) => {
                    eprintln!("xtask: step {number} of {total} failed with exit code {code}.");
                }
                None => eprintln!("xtask: step {number} of {total} was killed by a signal."),
            }
            eprintln!("xtask: {remaining} step(s) after it did not run.");
            eprintln!("xtask: rerun just this step with");
            eprintln!();
            eprintln!("    {line}");
            eprintln!();

            // Propagate the child's code, so a caller can still tell clippy
            // (101) from a failing test (100). A signal, or anything that does
            // not fit a process exit code, becomes 1 - still non-zero, which is
            // the part that matters.
            let propagated = status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .filter(|code| *code != 0)
                .unwrap_or(1);
            return ExitCode::from(propagated);
        }
    }

    eprintln!();
    eprintln!("xtask: verification set green, all {total} steps:");
    eprintln!("    ok  cargo xtask whitespace");
    for step in VERIFICATION_SET {
        eprintln!("    ok  {}", step.command_line());
    }

    ExitCode::SUCCESS
}

/// Whether `haystack` contains `needle` the way `grep -Eiw` would match it.
///
/// Case-insensitive, and bounded on both sides by something that is not a word
/// character — grep's word characters being `[A-Za-z0-9_]`. Written out rather
/// than approximated with a plain `contains`, because the difference is the
/// whole point of the flag: a plain search for `image` would fire on
/// `imagemagick`, and this must be the check CI makes, not a stricter or looser
/// one.
///
/// Faithfully reproduced includes its quirks: `-w` treats `-` as a boundary, so
/// this matches `image` inside `narvo-image` exactly as CI's grep does. Copying
/// the behaviour is the point; improving on it silently would be the drift the
/// guard test exists to prevent.
fn matches_word(haystack: &str, needle: &str) -> bool {
    let haystack = haystack.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';

    let mut from = 0;
    while let Some(offset) = haystack[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();

        let before_ok = start == 0 || !haystack[..start].chars().next_back().is_some_and(is_word);
        let after_ok =
            end == haystack.len() || !haystack[end..].chars().next().is_some_and(is_word);
        if before_ok && after_ok {
            return true;
        }

        from = start + 1;
    }

    false
}

/// Runs a step whose verdict is its output rather than its exit code.
///
/// Returns `Err(code)` with the code `run_ci` should return.
fn run_forbidding(
    cargo: &std::ffi::OsString,
    root: &Path,
    step: &Step,
    number: usize,
    total: usize,
) -> Result<(), ExitCode> {
    let line = step.command_line();

    let output = Command::new(cargo)
        .args(step.argv)
        .current_dir(root)
        .output();

    let output = match output {
        Ok(output) => output,
        Err(error) => {
            eprintln!();
            eprintln!("xtask: step {number} of {total} could not be started: {error}");
            eprintln!("xtask: it would have been");
            eprintln!();
            eprintln!("    {line}");
            eprintln!();
            return Err(ExitCode::FAILURE);
        }
    };

    let tree = String::from_utf8_lossy(&output.stdout);
    // Echoed unchanged, the way CI's `echo "$tree"` does: the tree is what tells
    // a reader which edge pulled a crate in.
    print!("{tree}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));

    if !output.status.success() {
        eprintln!();
        eprintln!("xtask: step {number} of {total} failed to produce a tree at all.");
        eprintln!("xtask: rerun just this step with");
        eprintln!();
        eprintln!("    {line}");
        eprintln!();
        return Err(ExitCode::FAILURE);
    }

    let found: Vec<&str> = step
        .forbidden
        .iter()
        .copied()
        .filter(|crate_name| tree.lines().any(|line| matches_word(line, crate_name)))
        .collect();

    if !found.is_empty() {
        let remaining = total - number;
        eprintln!();
        eprintln!(
            "xtask: step {number} of {total} found device crates in the headless tree: {}",
            found.join(", ")
        );
        eprintln!("xtask: the render feature gate has been bypassed.");
        eprintln!("xtask: {remaining} step(s) after it did not run.");
        eprintln!("xtask: rerun just this step with");
        eprintln!();
        eprintln!("    {line}");
        eprintln!();
        return Err(ExitCode::FAILURE);
    }

    eprintln!(
        "xtask: headless tree is free of {}",
        step.forbidden.join(", ")
    );
    Ok(())
}

/// Reports every collapsed line continuation in the tree's string literals.
///
/// A step of the verification set, and also a subcommand of its own so that a
/// calibration run can be done without the other steps. See
/// [`whitespace`](crate::whitespace) for the rule and what it deliberately
/// cannot see.
fn run_whitespace() -> ExitCode {
    let findings = whitespace::findings(&workspace_root());

    if findings.is_empty() {
        eprintln!("xtask: no collapsed line continuations in any string literal.");
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "xtask: {} collapsed line continuation(s) in string literals.",
        findings.len()
    );
    eprintln!();
    for finding in &findings {
        eprintln!("    {}:{}", finding.path.display(), finding.line);
        eprintln!("        {}", finding.text);
    }
    eprintln!();
    eprintln!(
        "A run of six or more spaces inside an ordinary string literal is what a \
         lost `\\` line continuation leaves behind. Fix it in an editor, never \
         through a tool that reprocesses escapes (ProjektPlan.md §9.2)."
    );

    ExitCode::FAILURE
}

/// Prints what this binary accepts and exits non-zero.
fn usage(args: &[String]) -> ExitCode {
    if args.is_empty() {
        eprintln!("xtask: no task given.");
    } else {
        let given = args.join(" ");
        eprintln!("xtask: `{given}` is not a task this runner knows.");
    }

    eprintln!();
    eprintln!("Tasks:");
    eprintln!();
    eprintln!("    cargo xtask ci");
    eprintln!();
    eprintln!("        Runs the verification set from CLAUDE.md in order and stops at the");
    eprintln!("        first failure. Individual steps are still meant to be run by hand");
    eprintln!("        while iterating - CLAUDE.md lists all eleven.");
    eprintln!();
    eprintln!("    cargo xtask determinism record <dir>");
    eprintln!("    cargo xtask determinism compare <dir-a> <dir-b>");
    eprintln!();
    eprintln!("        The cross-platform half of the determinism suite. `record` runs a");
    eprintln!("        fixed matrix through the narvo binary and writes the dumps;");
    eprintln!("        `compare` takes two such directories, one per platform, and fails");
    eprintln!("        naming the case, entity and component that differ. Everything a");
    eprintln!("        single machine can check is an ordinary test instead.");

    // 2 rather than 1, so "you called me wrong" is distinguishable from "the
    // verification set failed" by exit code alone.
    ExitCode::from(2)
}

/// The workspace root, which every step is run from.
///
/// Running from a fixed directory is what makes `cargo xtask ci` behave the same
/// no matter which subdirectory it was invoked from — `cargo deny check` in
/// particular resolves `deny.toml` against the working directory.
///
/// The path is this crate's manifest directory baked in at compile time, minus
/// one level. That holds as long as `xtask/` sits directly below the workspace
/// root, which is where the manifest puts it.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always sits one level below the workspace root")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::{VERIFICATION_SET, workspace_root};
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Reads a file at a workspace-relative path, failing with the full path.
    fn read(relative: &[&str]) -> String {
        let mut path = workspace_root();
        for segment in relative {
            path.push(segment);
        }

        fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    }

    /// The drift guard: every step of the set is also run by CI.
    ///
    /// It is a text search over the workflow, not a YAML parse, because the
    /// property worth checking is "this command still appears there" and parsing
    /// buys nothing for it.
    ///
    /// What it catches: a step added to or edited in [`VERIFICATION_SET`]
    /// without a matching change to the workflow — the direction that would let
    /// a task pass locally on a check CI never performs.
    ///
    /// What it does **not** catch, and this is not a gap to be papered over: the
    /// opposite direction. A step added to the workflow alone breaks none of
    /// these strings, so CI can verify strictly more than `cargo xtask ci` and
    /// this test stays green. It already does: the headless build, the headless
    /// test run and the `cargo tree` assertion that the headless tree has no
    /// graphics crates are CI-only. Nor does it check the *arguments* beyond the
    /// pinned substring, so a step that grew an extra flag in CI still passes.
    #[test]
    fn every_step_of_the_verification_set_also_runs_in_ci() {
        let workflow = read(&[".github", "workflows", "ci.yml"]);

        for step in VERIFICATION_SET {
            let line = step.command_line();
            for marker in step.ci_markers {
                assert!(
                    workflow.contains(marker),
                    "the CI workflow no longer contains {marker:?}, which is how `{line}` \
                     is run there. The verification set lives in three places and they \
                     move together: CLAUDE.md, xtask/src/main.rs and \
                     .github/workflows/ci.yml"
                );
            }
        }
    }

    /// Every workflow file, as path segments, that this repository runs in CI.
    ///
    /// Both are listed because the guard below is about runner time, and runner
    /// time is spent by whichever workflow is running.
    const WORKFLOWS: &[&[&str]] = &[
        &[".github", "workflows", "ci.yml"],
        &[".github", "workflows", "release-determinism.yml"],
    ];

    /// Splits a workflow into `(job name, job body)` pairs.
    ///
    /// A text scan rather than a YAML parse, for the reason
    /// [`every_step_of_the_verification_set_also_runs_in_ci`] gives and one
    /// more: `xtask`'s dependency list is deliberately empty, and a YAML crate
    /// bought to check one scalar per job would be the first entry in it.
    ///
    /// A job header is the only construct in these files at exactly two spaces
    /// of indentation that ends in a colon with nothing after it, and `jobs:` is
    /// the last top-level key in both — so everything after it is jobs.
    fn jobs_of(workflow: &str) -> Vec<(String, String)> {
        let Some((_, after)) = workflow.split_once("\njobs:\n") else {
            panic!("the workflow has no `jobs:` key, so this guard cannot read it");
        };

        let mut jobs: Vec<(String, String)> = Vec::new();
        for line in after.lines() {
            let is_header = line.starts_with("  ")
                && !line.starts_with("   ")
                && line.ends_with(':')
                && !line.trim_start().starts_with('#');

            if is_header {
                jobs.push((line.trim().trim_end_matches(':').to_string(), String::new()));
            } else if let Some(current) = jobs.last_mut() {
                current.1.push_str(line);
                current.1.push('\n');
            }
        }

        assert!(
            !jobs.is_empty(),
            "no job headers were found under `jobs:`, so this guard would pass \
             vacuously — which is the failure class it exists to avoid"
        );
        jobs
    }

    /// Every job carries a `timeout-minutes`, so no job can inherit six hours.
    ///
    /// **The number is not checked, its presence is.** What the value should be
    /// belongs to the job and moves when the job does; what must not happen is a
    /// job having none at all, because GitHub's default then applies and that
    /// default is six hours. M7.5d paid for that once: an `apt-get` hung for
    /// over fifty minutes before a line of Narvo code was compiled, and nothing
    /// would have stopped it short of the six-hour mark.
    ///
    /// Asserted on the **job**, not on a step. A step-level `timeout-minutes` is
    /// legal YAML in the same file and would satisfy a naive substring search
    /// while leaving the job itself unbounded, so the search is anchored to four
    /// spaces of indentation — the level a job's own keys sit at, one deeper
    /// than the header and shallower than anything inside `steps:`.
    #[test]
    fn every_ci_job_has_a_timeout() {
        for relative in WORKFLOWS {
            let workflow = read(relative);
            let file = relative.join("/");

            for (name, body) in jobs_of(&workflow) {
                assert!(
                    body.lines()
                        .any(|line| line.starts_with("    timeout-minutes:")),
                    "job `{name}` in {file} has no `timeout-minutes` of its own, so it \
                     inherits GitHub's default of six hours. Add one to the job (four \
                     spaces of indentation, beside `runs-on:`); a step-level timeout \
                     does not bound the job."
                );
            }
        }
    }

    /// The forbidden-crate list here is the one CI's `grep` uses.
    ///
    /// **The anti-drift guard for the steps that are a reimplementation rather
    /// than a command.** Steps seven and eight are cargo lines whose exit code is
    /// the verdict, so `ci_markers` covers them. Step nine is not: CI runs
    /// `cargo tree` and pipes it through `grep -Eiw '…'`, and this binary runs
    /// `cargo tree` and searches the output itself. Two implementations of one
    /// rule is exactly the drift the M4.9 prompt warns about, so the *rule* — the
    /// list of names — is compared instead of trusted.
    ///
    /// Parsed out of the grep line rather than matched as a whole string, so
    /// that reordering the alternation in the workflow is not a failure while
    /// adding or dropping a name is.
    ///
    /// **Every such line, not the first one.** M7.1 added a second tree check
    /// for the headless game, and a guard that read only the first would have
    /// left the new one comparing against a list nothing held it to — which is
    /// the shape of the hole V5 found in the *other* drift guard, in the tool
    /// built against exactly this class.
    #[test]
    fn the_forbidden_crates_are_the_ones_ci_greps_for() {
        let workflow = read(&[".github", "workflows", "ci.yml"]);

        let lines: Vec<&str> = workflow
            .lines()
            .filter(|line| line.contains("grep -Eiw"))
            .collect();

        assert!(
            !lines.is_empty(),
            "the CI workflow no longer has a `grep -Eiw` line, so the headless tree \
             check has moved or gone. This guard cannot compare against something \
             that is not there, and a guard that shrugs is worse than none"
        );

        let tree_steps = VERIFICATION_SET
            .iter()
            .filter(|step| !step.forbidden.is_empty())
            .count();
        assert_eq!(
            lines.len(),
            tree_steps,
            "there are {tree_steps} tree step(s) here and {} grep line(s) in the workflow; \
             one of them checks a tree nothing else does",
            lines.len()
        );

        let mut here: Vec<&str> = super::FORBIDDEN_IN_HEADLESS.to_vec();
        here.sort_unstable();

        for line in lines {
            let pattern = line
                .split('\'')
                .nth(1)
                .expect("the grep pattern is single-quoted in the workflow");

            let mut in_ci: Vec<&str> = pattern.split('|').collect();
            in_ci.sort_unstable();

            assert_eq!(
                here, in_ci,
                "FORBIDDEN_IN_HEADLESS and a grep pattern in the workflow have drifted \
                 apart. They are one decision — which crates may not reach a headless \
                 tree — and a local run that checked a different list from CI would be \
                 worse than not checking at all"
            );
        }
    }

    /// `grep -Eiw`'s word boundaries, reproduced.
    #[test]
    fn word_matching_is_bounded_the_way_grep_bounds_it() {
        assert!(super::matches_word("├── image v0.25.10", "image"));
        assert!(super::matches_word("└── WGPU v30.0.0", "wgpu"));
        assert!(super::matches_word(
            "├── raw-window-handle v0.6",
            "raw-window-handle"
        ));

        // A longer word that merely contains the name is not a match.
        assert!(!super::matches_word("├── imagemagick v1.0", "image"));
        assert!(!super::matches_word("├── wgpu_types v30", "wgpu"));

        // And the quirk that is grep's rather than ours: a dash is not a word
        // character, so a hyphenated name does match.
        assert!(super::matches_word("├── narvo-image v0.1.0", "image"));
    }

    /// The same guard against the other document that defines the set.
    ///
    /// `CLAUDE.md` is what an agent reads to learn the verification set, so it
    /// drifting away from what actually runs is the more expensive failure of
    /// the two. Unlike the CI markers these are exact: the rendered line has to
    /// appear verbatim, which is also what makes a failure message from
    /// `cargo xtask ci` something a reader can paste into a shell.
    #[test]
    fn every_step_is_documented_in_claude_md() {
        let agreement = read(&["CLAUDE.md"]);

        assert_eq!(
            VERIFICATION_SET.len(),
            11,
            "the verification set is eleven commands — six over the workspace, \
             three over the headless configuration, one over the agent transport \
             and one over audio without a device; changing that is an architecture \
             decision, not an edit to this table"
        );

        for step in VERIFICATION_SET {
            let line = step.command_line();
            assert!(
                agreement.contains(&line),
                "CLAUDE.md no longer documents `{line}`. The verification set lives in \
                 three places and they move together: CLAUDE.md, xtask/src/main.rs and \
                 .github/workflows/ci.yml"
            );
        }
    }

    /// The release-determinism workflow, as path segments.
    const RELEASE_WORKFLOW: &[&str] = &[".github", "workflows", "release-determinism.yml"];

    /// The same file, spelled the way GitHub writes it inside a `paths:` filter.
    ///
    /// This is how the workflow's own entry is recognised and set aside; see
    /// [`claude_md_names_every_answer_changing_trigger`].
    const RELEASE_WORKFLOW_PATH: &str = ".github/workflows/release-determinism.yml";

    /// The `CLAUDE.md` heading whose bullet list enumerates the triggers.
    const RELEASE_SECTION: &str = "### The release profile";

    /// The `paths:` filter of the release workflow, in file order.
    ///
    /// A line scan rather than a YAML parse, for the reason
    /// [`every_step_of_the_verification_set_also_runs_in_ci`] gives: the
    /// property is "which names are listed here", and a parser buys nothing for
    /// it while costing a dependency.
    ///
    /// Both failure modes are loud on purpose. A guard that cannot find its
    /// anchor and shrugs is worse than no guard, because it reports green while
    /// checking nothing — the failure class this project has hit three times.
    fn release_workflow_paths(workflow: &str) -> Vec<String> {
        let mut lines = workflow.lines().skip_while(|line| line.trim() != "paths:");

        assert!(
            lines.next().is_some(),
            "{RELEASE_WORKFLOW_PATH} has no `paths:` key, so there is nothing to \
             check CLAUDE.md against. Either the trigger filter was removed - in \
             which case the workflow now runs on every push and CLAUDE.md's \
             \"{RELEASE_SECTION}\" section is wrong about the whole mechanism - or \
             the key was renamed and this guard needs to follow it."
        );

        let paths: Vec<String> = lines
            .map_while(|line| {
                let entry = line.trim().strip_prefix("- ")?;
                Some(entry.trim().trim_matches('"').to_string())
            })
            .collect();

        assert!(
            !paths.is_empty(),
            "{RELEASE_WORKFLOW_PATH} has a `paths:` key with no entries under it. \
             An empty filter is not a narrower filter: GitHub falls back to running \
             the workflow on every push, which is the opposite of what ADR-0013 \
             decided."
        );

        paths
    }

    /// The bullet list under [`RELEASE_SECTION`] in `CLAUDE.md`.
    ///
    /// Deliberately the bullets alone and not the whole section. The prose around
    /// them is free to mention `ci.yml` or the workflow file without that
    /// counting as a claim about what triggers the job; the enumeration is the
    /// claim. Bounding it this way is what keeps the guard from going red for the
    /// wrong reason, and a guard that does that gets switched off.
    fn release_trigger_bullets(agreement: &str) -> String {
        let (_, after_heading) = agreement.split_once(RELEASE_SECTION).unwrap_or_else(|| {
            panic!(
                "CLAUDE.md has no \"{RELEASE_SECTION}\" heading, so this guard cannot \
                 find the trigger list it exists to check. If the section was \
                 renamed, rename it here too; if it was deleted, CLAUDE.md no longer \
                 tells anyone when {RELEASE_WORKFLOW_PATH} runs."
            )
        });

        let bullets: Vec<&str> = after_heading
            .lines()
            .take_while(|line| !line.starts_with('#'))
            .skip_while(|line| !line.starts_with("- "))
            .take_while(|line| !line.trim().is_empty())
            .collect();

        assert!(
            !bullets.is_empty(),
            "CLAUDE.md's \"{RELEASE_SECTION}\" section has no bullet list, so there \
             is no enumeration of triggers to compare against \
             {RELEASE_WORKFLOW_PATH}. The section exists; its list does not."
        );

        bullets.join("\n")
    }

    /// Every name written in backticks in `text`, in order.
    fn backtick_names(text: &str) -> Vec<String> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }

    /// Renders both sides for a failure message. The difference is useless
    /// without the two lists that produced it.
    fn both_sides(paths: &[String], bullets: &str) -> String {
        format!(
            "  {RELEASE_WORKFLOW_PATH} paths: {paths:?}\n  \
             CLAUDE.md \"{RELEASE_SECTION}\" names: {:?}",
            backtick_names(bullets)
        )
    }

    /// The drift guard for the trigger list: CLAUDE.md names every path that can
    /// change the release comparison's answer.
    ///
    /// The workflow's `paths:` is the truth and is never the thing under test —
    /// GitHub reads it, nobody else. CLAUDE.md is what a session reads *instead*
    /// of it, which is why its drifting is the expensive direction: a session
    /// editing `.cargo/config.toml` and consulting CLAUDE.md would conclude the
    /// release comparison does not cover that file.
    ///
    /// The workflow's own entry is excluded, and that is a decision rather than
    /// an oversight. The other entries are there because a change to them can
    /// change what the comparison computes; the self-entry is there so that a
    /// change to the job is validated by running it. CLAUDE.md's list is
    /// introduced as "it runs when its answer can change", so demanding the
    /// self-entry appear in it would force the prose to file a mechanical
    /// self-validation under a heading it does not belong to.
    ///
    /// What it does **not** catch: the workflow's own entry disappearing from the
    /// filter, by the exclusion above. Nor the order of either list, nor whether
    /// the *reason* each bullet gives is true — a bullet naming the right file
    /// for the wrong reason passes.
    #[test]
    fn claude_md_names_every_answer_changing_trigger() {
        let workflow = read(RELEASE_WORKFLOW);
        let paths = release_workflow_paths(&workflow);
        let bullets = release_trigger_bullets(&read(&["CLAUDE.md"]));

        let answer_changing: Vec<&String> = paths
            .iter()
            .filter(|p| *p != RELEASE_WORKFLOW_PATH)
            .collect();

        assert!(
            !answer_changing.is_empty(),
            "{RELEASE_WORKFLOW_PATH} triggers on nothing but itself, so the release \
             comparison can no longer be reached by changing what it measures.\n{}",
            both_sides(&paths, &bullets)
        );

        for path in answer_changing {
            assert!(
                bullets.contains(&format!("`{path}`")),
                "CLAUDE.md's \"{RELEASE_SECTION}\" list does not name `{path}`, but \
                 {RELEASE_WORKFLOW_PATH} triggers on it.\n{}\n  missing from \
                 CLAUDE.md: `{path}`\nThe trigger list lives in two places and they \
                 move together. The workflow is the truth; CLAUDE.md is what an \
                 agent reads instead of reading the workflow.",
                both_sides(&paths, &bullets)
            );
        }
    }

    /// The same guard in the other direction: CLAUDE.md names no trigger that the
    /// workflow does not have.
    ///
    /// Without this, deleting an entry from `paths:` leaves CLAUDE.md promising a
    /// guard that no longer exists — silently narrowing what is watched, which is
    /// exactly the "less often becomes switched off" that ADR-0013 argues against.
    ///
    /// Only `.toml`, `.lock` and `.yml` names are considered, because those are
    /// what a trigger can be: cargo, toolchain and workflow configuration. A
    /// bullet mentioning `[profile.release]` is prose, not a claim about the
    /// filter. Here the workflow's own path *is* allowed, since CLAUDE.md
    /// naming it would be true.
    ///
    /// `.lock` joined that list in V5, and the sentence above used to name
    /// `Cargo.lock` as its example of prose. D18 made the lock a trigger, so
    /// the example had become the counter-example — this direction rests on an
    /// extension heuristic (`ProjektPlan.md` §7.3 names it as such), and a
    /// heuristic that has not followed the filter checks the new entry not at
    /// all while staying green.
    #[test]
    fn claude_md_names_no_trigger_the_workflow_lacks() {
        let workflow = read(RELEASE_WORKFLOW);
        let paths = release_workflow_paths(&workflow);
        let bullets = release_trigger_bullets(&read(&["CLAUDE.md"]));

        for name in backtick_names(&bullets) {
            if !(name.ends_with(".toml") || name.ends_with(".lock") || name.ends_with(".yml")) {
                continue;
            }

            assert!(
                paths.contains(&name),
                "CLAUDE.md's \"{RELEASE_SECTION}\" list names `{name}` as a trigger, \
                 but {RELEASE_WORKFLOW_PATH} does not list it.\n{}\n  claimed but not \
                 a trigger: `{name}`\nEither the entry was dropped from `paths:` - in \
                 which case that file is no longer watched and CLAUDE.md promises a \
                 guard that does not exist - or CLAUDE.md names it wrongly.",
                both_sides(&paths, &bullets)
            );
        }
    }

    /// `workflow_dispatch` is in the enumeration without being a path, and it is
    /// checked on both sides rather than skipped.
    ///
    /// It is what makes "runs less often" survivable: without it a failure cannot
    /// be re-run and nobody can look at the answer without editing a file, so
    /// "less often" quietly becomes "switched off" (ADR-0013). Losing it from the
    /// workflow, or from the sentence that tells a reader it exists, are both
    /// worth a red run.
    #[test]
    fn the_release_workflow_can_still_be_run_by_hand() {
        let workflow = read(RELEASE_WORKFLOW);
        let bullets = release_trigger_bullets(&read(&["CLAUDE.md"]));

        assert!(
            workflow
                .lines()
                .any(|line| line.trim() == "workflow_dispatch:"),
            "{RELEASE_WORKFLOW_PATH} has no `workflow_dispatch:` trigger. A workflow \
             that runs on a path filter and cannot be started by hand cannot be \
             re-run after a failure and cannot be consulted on purpose, which is how \
             \"less often\" becomes \"switched off\" (ADR-0013)."
        );

        assert!(
            bullets.contains("`workflow_dispatch`"),
            "CLAUDE.md's \"{RELEASE_SECTION}\" list no longer mentions \
             `workflow_dispatch`, so nothing tells a reader the release comparison \
             can be run by hand.\n  CLAUDE.md names: {:?}",
            backtick_names(&bullets)
        );
    }

    /// No step may re-enter this binary.
    ///
    /// `cargo xtask ci` runs the test suite, and the test suite includes this
    /// crate. A step that invoked `cargo xtask` would therefore recurse until
    /// the machine gave out, and it would do so only on a full run — never while
    /// iterating on a single crate. Cheap to assert, expensive to discover.
    #[test]
    fn no_step_can_re_enter_xtask() {
        for step in VERIFICATION_SET {
            let line = step.command_line();
            assert!(
                !line.contains("xtask"),
                "`{line}` runs xtask from inside the verification set, which recurses \
                 without end: xtask ci -> nextest -> xtask ci -> ..."
            );
        }
    }

    /// The shape of the environment variable that turns a skipped GPU test into
    /// a failed one — without naming it.
    ///
    /// **The name is deliberately not written in this file.** `.github/workflows/ci.yml`
    /// is the authority for it, and a guard carrying its own copy would be
    /// comparing the tree against itself: green while the thing it describes had
    /// been renamed on disk. That is the M2.1c class, and the guard below exists
    /// precisely because nothing was comparing the two sides at all. So this
    /// constant is the *suffix* every candidate name ends with, and both sides
    /// are read from disk at run time.
    const REQUIRE_GPU_SUFFIX: &str = "REQUIRE_GPU";

    /// Every distinct `<PREFIX>_REQUIRE_GPU` name in a piece of text, in order.
    ///
    /// A token is a maximal run of upper-case letters, digits and underscores,
    /// which is how an environment variable is spelled on both sides: as a YAML
    /// key in the workflow, and inside a string literal in Rust.
    ///
    /// A token counts only if it **ends** with [`REQUIRE_GPU_SUFFIX`], is longer
    /// than it, and **begins with a letter**. All three are load-bearing, and
    /// the third was added because the first draft of this function collected a
    /// name out of its own documentation:
    ///
    /// - *longer than the suffix* keeps out the bare suffix, which is the
    ///   constant above — otherwise the guard finds its own spelling and passes
    ///   on it;
    /// - *ends with* keeps out `REQUIRE_GPU_VAR`, the Rust identifier most of the
    ///   reading files bind the name to. That identifier is a local spelling and
    ///   may be renamed freely; the string it holds may not;
    /// - *begins with a letter* keeps out the bare `_REQUIRE_GPU` that a prose
    ///   sentence writing `<PREFIX>_REQUIRE_GPU` leaves behind once the angle
    ///   bracket ends the token. It is also true of the thing being matched: an
    ///   environment variable set by a workflow is named, not suffixed.
    fn require_gpu_names(text: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut token = String::new();

        // One trailing non-word character, so a name at the very end of the file
        // is flushed by the same branch as every other one.
        for c in text.chars().chain(std::iter::once(' ')) {
            if c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' {
                token.push(c);
                continue;
            }

            if token.len() > REQUIRE_GPU_SUFFIX.len()
                && token.ends_with(REQUIRE_GPU_SUFFIX)
                && token.starts_with(|first: char| first.is_ascii_uppercase())
                && !names.contains(&token)
            {
                names.push(token.clone());
            }
            token.clear();
        }

        names
    }

    /// The GPU requirement variable has one name, and both sides use it.
    ///
    /// CI sets this variable so that a runner without a usable adapter is a red
    /// run rather than a silent skip; the render tests read it and turn their own
    /// skip into a failure when it is present. That is **one decision living in
    /// two files**, and until U2 nothing held the two together.
    ///
    /// U1a measured the gap rather than supposing it. The code was put back on
    /// the engine's previous name while the workflow was left alone, and **all
    /// thirty-two guards stayed green** — a CI run that skipped every GPU test
    /// while reporting fourteen green steps would have looked exactly like a
    /// verified one. That is the shape of every failure recorded at the top of
    /// `CLAUDE.md`: an answer that looks like data and is not.
    ///
    /// So both sides are read from disk and compared as sets, which makes the
    /// guard blind to the direction of a drift — a rename that moves the sources
    /// only, and one that moves the workflow only, are the same failure here.
    /// A rename to a name *without* the suffix empties the source side instead
    /// of populating it with a second name, which is what the non-empty
    /// assertion is for: a guard with nothing to compare must be loud, not
    /// vacuous.
    ///
    /// What it does **not** catch: whether a file that renders through an adapter
    /// reads the variable at all. A new render test that simply forgets to skip
    /// is invisible here, because this guard compares names and not call sites.
    #[test]
    fn the_gpu_requirement_variable_has_one_name() {
        let workflow = read(&[".github", "workflows", "ci.yml"]);
        let in_ci = require_gpu_names(&workflow);

        assert_eq!(
            in_ci.len(),
            1,
            "the CI workflow names {} variables ending in `{REQUIRE_GPU_SUFFIX}`: {in_ci:?}. \
             Exactly one is expected — it is the switch that makes a missing adapter a \
             failure there — and this guard cannot say which of several the sources \
             ought to be reading",
            in_ci.len()
        );
        let name = &in_ci[0];

        assert!(
            workflow
                .lines()
                .any(|line| line.trim().starts_with(&format!("{name}:"))),
            "`{name}` appears in {} but is never assigned there. A variable that is \
             only mentioned in a comment is not set for any step, so every GPU test \
             would skip in CI while this guard reported the two sides in agreement",
            ".github/workflows/ci.yml"
        );

        let mut files: Vec<PathBuf> = Vec::new();
        licence_bearing_files(&workspace_root(), &mut files);

        let mut in_sources: Vec<String> = Vec::new();
        let mut readers: Vec<String> = Vec::new();
        for file in files.iter().filter(|f| {
            f.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
        }) {
            let text = fs::read_to_string(file)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));

            let found = require_gpu_names(&text);
            if !found.is_empty() {
                readers.push(file.display().to_string());
            }
            for one in found {
                if !in_sources.contains(&one) {
                    in_sources.push(one);
                }
            }
        }
        in_sources.sort();

        assert!(
            !in_sources.is_empty(),
            "no .rs file in this repository names a variable ending in \
             `{REQUIRE_GPU_SUFFIX}`, while {} sets `{name}`. Either every render test \
             stopped honouring it — in which case CI's switch is inert and a runner \
             without an adapter is green again — or the name was changed to a shape \
             this guard cannot recognise. Both are the failure it was written for",
            ".github/workflows/ci.yml"
        );

        assert_eq!(
            in_sources,
            vec![name.clone()],
            "the workflow sets `{name}` and the sources read {in_sources:?}, across \
             {} file(s). They are one decision — whether a missing adapter is a \
             failure — and a drift between them switches off every GPU test in CI \
             without turning a single step red.\n  reading files: {readers:?}",
            readers.len()
        );
    }

    /// Directory names the licence walk never descends into.
    ///
    /// `target/` is the important one: it holds every dependency's own manifest,
    /// and those declare licences that are their authors' to declare. A guard
    /// that read them would fail on other people's crates, which is both wrong
    /// and unfixable from here.
    const NOT_WALKED: &[&str] = &["target", ".git"];

    /// Root-level file names that would grant a licence by existing.
    ///
    /// Scoped to the repository root deliberately. `crates/narvo-testkit/assets/`
    /// holds `DejaVuSansMono-LICENSE.txt`, and that one is *foreign* and has to
    /// stay exactly where it is — the font's own terms require its licence to
    /// travel beside it. A guard that swept the whole tree for `LICENSE*` would
    /// demand the deletion of the one licence file this repository is actually
    /// obliged to carry.
    const FORBIDDEN_LICENCE_FILES: &[&str] = &["LICENSE-MIT", "LICENSE-APACHE", "COPYING"];

    /// The sentence `LICENSE` and `README.md` both have to keep saying.
    const RESERVATION: &str = "All rights reserved.";

    /// Opening phrases of the two licences this repository used to grant.
    ///
    /// Checked against `LICENSE` so that replacing the notice with a real licence
    /// text is caught even though the file name stays the same. Neither phrase
    /// appears in a document that withholds permission, so neither can match by
    /// accident.
    const GRANTING_PHRASES: &[&str] = &[
        "Permission is hereby granted",
        "Licensed under the Apache License",
    ];

    /// Every `Cargo.toml` and every `.rs` file in the repository.
    ///
    /// Filtered during the walk rather than after it, so that nothing else is
    /// even collected — the working copy carries untracked files that are none of
    /// this guard's business, and a list it does not build is a list it cannot
    /// read by mistake.
    fn licence_bearing_files(directory: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()));

        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!("cannot read an entry of {}: {error}", directory.display())
            });
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();

            if path.is_dir() {
                if !NOT_WALKED.contains(&name.as_str()) {
                    licence_bearing_files(&path, out);
                }
            } else if name == "Cargo.toml" || name.ends_with(".rs") {
                out.push(path);
            }
        }
    }

    /// The repository grants no licence, in every place that could grant one.
    ///
    /// D5 reopened the licence question on 11.08.2026 and the direction it
    /// records is incompatible with the `MIT OR Apache-2.0` this repository had
    /// carried since M0. That grant was removed rather than exchanged, because a
    /// grant cannot be taken back from a copy published under it and the point is
    /// to keep the decision open. The repository being private contains the
    /// damage today; it does not prevent it, since visibility is one click.
    ///
    /// A one-off removal rots. What this test is here for is the *next* task: a
    /// new crate copied from an old one, a `cargo init` writing a `license` field
    /// by default, an editor adding an SPDX header, someone restoring
    /// `LICENSE-MIT` from git history because a build tool complained. Each of
    /// those is a plausible edit that silently re-grants the licence, and none of
    /// them breaks a build.
    ///
    /// **Read from disk at run time, never `include_str!`.** A baked-in copy
    /// would answer about the tree as it stood when this was compiled — green
    /// while the file it describes had changed. That is the M2.1c class.
    ///
    /// What it does **not** catch: prose anywhere other than `LICENSE` and
    /// `README.md` promising terms — a doc comment offering permission would pass
    /// here; a licence declared in `Cargo.lock` or in published registry metadata,
    /// neither of which this repository writes; and the case of somebody deleting
    /// this test along with the notice, which no test can guard against.
    #[test]
    fn the_repository_grants_no_licence() {
        let root = workspace_root();
        let mut files = Vec::new();
        licence_bearing_files(&root, &mut files);
        files.sort();

        let mut manifests = 0usize;
        let mut sources = 0usize;

        // The needle is assembled rather than written out, because this file is
        // one of the `.rs` files being searched and a literal here would make the
        // guard fail on itself.
        let spdx = format!("SPDX-{}", "License-Identifier");

        for path in &files {
            let shown = path.strip_prefix(&root).unwrap_or(path).display();
            let text = fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));

            if path.file_name().is_some_and(|name| name == "Cargo.toml") {
                manifests += 1;

                for (index, line) in text.lines().enumerate() {
                    assert!(
                        !line.trim_start().starts_with("license"),
                        "{shown}:{} declares a licence: `{}`\nNarvo grants none - all \
                         rights reserved, and no licence chosen yet (`LICENSE`, and D5 in \
                         ProjektPlan.md §11). A `license` or `license-file` key is a grant \
                         in the one form that tooling reads and copies onward, so the \
                         field stays absent rather than being set to something weaker. \
                         Removing this line is the fix; deciding a licence is a human's \
                         call and not this one's.",
                        index + 1,
                        line.trim()
                    );
                }

                assert!(
                    text.lines()
                        .any(|line| line.trim_start().starts_with("publish")),
                    "{shown} says nothing about `publish`, so it defaults to publishable. \
                     Two things rest on that field. A `cargo publish` would push \
                     unlicensed source to crates.io, which is as irrevocable as making \
                     the repository public; and `[licenses.private]` in deny.toml exempts \
                     these crates from the licence check by reading exactly this field, \
                     so a publishable crate here turns `cargo deny check` red as well."
                );
            } else {
                sources += 1;

                assert!(
                    !text.contains(&spdx),
                    "{shown} carries an SPDX licence header. It grants a licence per file, \
                     which is the same grant this repository removed from its manifests \
                     and its root - and a per-file one is harder to find again later."
                );
            }
        }

        // Both counts guard against the quiet failure: a walk that found nothing
        // asserts nothing and reports green. The bounds are floors rather than
        // equalities so that adding a crate or a module is not a failure, and the
        // floor is deliberately below the true count so that removing one is not
        // one either — U2 removed a member and this assertion was right to stay
        // quiet about it.
        //
        // The message used to say "fifteen - one root and fourteen members", and
        // that was **already wrong before U2**: the tree carried seventeen
        // manifests at commit afd08e1 and carries sixteen now (one root, fifteen
        // members), both measured rather than counted from memory. A floor cannot
        // catch a stale sentence beside it, which is why the numbers below name
        // where they came from.
        assert!(
            manifests >= 15,
            "the walk found only {manifests} manifests. This workspace has sixteen as of \
             U2 - one root and fifteen members - and the floor here is fifteen so that \
             adding or removing one crate is not a failure. Either the walk started \
             somewhere other than the repository root or it stopped early, and in both \
             cases everything above passed by not looking."
        );
        assert!(
            sources >= 100,
            "the walk found only {sources} Rust files, and this workspace has well over a \
             hundred. The SPDX half of this guard checked almost nothing."
        );

        for name in FORBIDDEN_LICENCE_FILES {
            assert!(
                !root.join(name).exists(),
                "{name} is back at the repository root. It was removed on 14.08.2026 \
                 together with the `license` fields; restoring it re-grants what the \
                 removal was for. The root carries `LICENSE`, which is a notice that \
                 there is no licence, and nothing else."
            );
        }

        let notice = fs::read_to_string(root.join("LICENSE")).unwrap_or_else(|error| {
            panic!(
                "cannot read the licence notice at {}: {error}\nIt is the one place a \
                 reader who lands on this repository is told that nothing is granted. \
                 Without it the manifests are merely silent, and silence is not a \
                 reservation.",
                root.join("LICENSE").display()
            )
        });

        assert!(
            notice.contains(RESERVATION),
            "LICENSE no longer says {RESERVATION:?}. That sentence is the whole content \
             of the file; a notice that does not reserve rights is not a notice."
        );

        for phrase in GRANTING_PHRASES {
            assert!(
                !notice.contains(phrase),
                "LICENSE contains {phrase:?}, so it grants a licence again - under the \
                 same file name, which is why the file existing is not what this checks. \
                 If a licence has genuinely been decided, that is an ADR and a change to \
                 this test, not an edit to one file."
            );
        }

        let readme = read(&["README.md"]);
        assert!(
            readme.contains(RESERVATION),
            "README.md no longer says {RESERVATION:?}. The machine-readable half of this \
             is guarded above, and this is the half a human actually reads - the v0.96 \
             rule is that a fact can be guarded while the text asserting it is not."
        );
    }
}
