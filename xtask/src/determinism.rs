//! The cross-platform half of the determinism suite.
//!
//! Everything a single machine can check about determinism is an ordinary test
//! in `crates/narvo-app/tests/determinism.rs`, and it runs in the verification
//! set like every other test. This module exists for the one question a test
//! cannot answer, because a test only ever observes the platform it is running
//! on: **do Windows and Linux compute the same state?**
//!
//! The shape is two halves that never run on the same machine:
//!
//! - `cargo xtask determinism record <dir>` runs a fixed matrix of cases through
//!   the `narvo` binary and writes each one's canonical dump to a file. Each CI
//!   job does this and uploads the directory.
//! - `cargo xtask determinism compare <a> <b>` takes two such directories,
//!   compares them file for file, and on a difference says which case, which
//!   tick, which entity and which component. A third CI job does this and fails
//!   the run if anything differs.
//!
//! # Why not simply check the expected hashes in
//!
//! Because ADR-0008 forbids it, and the reason it forbids it is the point: a
//! committed hash of a state moves when `ron` changes a separator or serde
//! changes a field order, turning the suite red while the simulation is
//! perfectly correct. Two directories produced from one commit by one
//! `Cargo.lock` move together, so a dependency bump changes both sides and the
//! comparison keeps meaning what it meant. Nothing here has to be nudged after
//! an upgrade.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// First line of a manifest, so an artifact from an incompatible build is
/// refused rather than half-compared.
const MANIFEST_MAGIC: &str = "narvo-determinism 1";

/// Name of the file listing what a recording run produced.
const MANIFEST: &str = "manifest.txt";

/// One case of the matrix: a way of running the simulation whose result both
/// platforms have to agree on.
struct Case {
    /// File name stem, and the name the comparison reports.
    ///
    /// It encodes the whole case — mode, seed, tick count — because that string
    /// is what a failure message has to hand a reader.
    name: &'static str,
    /// Arguments to `narvo`, without the report flag.
    args: &'static [&'static str],
    /// Whether the run also writes its input recording next to its dump.
    ///
    /// A recording is an artifact in its own right: M2.3 established by hand
    /// that two platforms produce byte-identical ones, and this is where that
    /// stops being a manual observation.
    record: bool,
}

/// The matrix, identical on both platforms by construction — it is this table.
///
/// The checkpoints are here for the same reason the replay test has them: a
/// comparison that only looks at the end state can say *that* two platforms
/// disagree but not *when* they started to, and by M5 with a physics solver in
/// the loop that difference is the whole value of the instrument.
const CASES: &[Case] = &[
    Case {
        name: "motion.t1",
        args: &["--mode", "motion", "--ticks", "1"],
        record: false,
    },
    Case {
        name: "motion.t100",
        args: &["--mode", "motion", "--ticks", "100"],
        record: false,
    },
    Case {
        name: "motion.t1000",
        args: &["--mode", "motion", "--ticks", "1000"],
        record: false,
    },
    Case {
        name: "motion.t5000",
        args: &["--mode", "motion", "--ticks", "5000"],
        record: false,
    },
    Case {
        name: "motion.t10000",
        args: &["--mode", "motion", "--ticks", "10000"],
        record: false,
    },
    Case {
        name: "chance.seed1.t1",
        args: &["--mode", "chance", "--seed", "1", "--ticks", "1"],
        record: false,
    },
    Case {
        name: "chance.seed1.t100",
        args: &["--mode", "chance", "--seed", "1", "--ticks", "100"],
        record: false,
    },
    Case {
        name: "chance.seed1.t1000",
        args: &["--mode", "chance", "--seed", "1", "--ticks", "1000"],
        record: false,
    },
    Case {
        name: "chance.seed1.t5000",
        args: &["--mode", "chance", "--seed", "1", "--ticks", "5000"],
        record: false,
    },
    Case {
        name: "chance.seed1.t10000",
        args: &["--mode", "chance", "--seed", "1", "--ticks", "10000"],
        record: false,
    },
    Case {
        name: "chance.seed2.t10000",
        args: &["--mode", "chance", "--seed", "2", "--ticks", "10000"],
        record: false,
    },
    Case {
        name: "input.seed1.t1",
        args: &["--mode", "input", "--seed", "1", "--ticks", "1"],
        record: false,
    },
    Case {
        name: "input.seed1.t100",
        args: &["--mode", "input", "--seed", "1", "--ticks", "100"],
        record: false,
    },
    Case {
        name: "input.seed1.t1000",
        args: &["--mode", "input", "--seed", "1", "--ticks", "1000"],
        record: false,
    },
    Case {
        name: "input.seed1.t5000",
        args: &["--mode", "input", "--seed", "1", "--ticks", "5000"],
        record: false,
    },
    Case {
        name: "input.seed1.t10000",
        args: &["--mode", "input", "--seed", "1", "--ticks", "10000"],
        record: true,
    },
    Case {
        name: "input.seed2.t10000",
        args: &["--mode", "input", "--seed", "2", "--ticks", "10000"],
        record: true,
    },
    // The first case whose initial state comes from a file rather than from
    // code (M4.3). It records on purpose: the recording carries the scene's
    // path and its SHA-256, so the cross-platform comparison is what would
    // catch a path written verbatim — `crates\...` on Windows against
    // `crates/...` on Linux — rather than in the normal form ADR-0019 fixes.
    //
    // The path is relative to the working directory, which is the workspace
    // root for `cargo xtask` and for both CI jobs.
    // The rigid-body solver, and the first cases whose result a dependency
    // computes rather than arithmetic in this repository (M5b.3b). They are what
    // ADR-0013's amendment leaves open: WSL and Windows share a CPU, so the
    // agreement M5b.1 measured could not distinguish "deterministic" from "same
    // machine". Two CI runners are two foreign machines.
    //
    // No seed, because the mode draws no random numbers, and no recording,
    // because it consumes no input - the same reasons `motion` has neither.
    //
    // The checkpoints are the table's own five and needed no exception. Measured
    // in M5b.3b: the first body reaches the ground between tick 20 and tick 25,
    // so `t1` is the pre-contact anchor and every later checkpoint is inside the
    // region where a rebuilt and a retained world part company. The state still
    // differs between every pair of neighbouring checkpoints, out to t10000, so
    // none of the five is along for the ride.
    Case {
        name: "physics.t1",
        args: &["--mode", "physics", "--ticks", "1"],
        record: false,
    },
    Case {
        name: "physics.t100",
        args: &["--mode", "physics", "--ticks", "100"],
        record: false,
    },
    Case {
        name: "physics.t1000",
        args: &["--mode", "physics", "--ticks", "1000"],
        record: false,
    },
    Case {
        name: "physics.t5000",
        args: &["--mode", "physics", "--ticks", "5000"],
        record: false,
    },
    Case {
        name: "physics.t10000",
        args: &["--mode", "physics", "--ticks", "10000"],
        record: false,
    },
    Case {
        name: "scene-file.t1000",
        args: &[
            "--mode",
            "scene-file",
            "--scene",
            "crates/narvo-app/scenes/determinism-case.ron",
            "--ticks",
            "1000",
        ],
        record: true,
    },
    // The game light (M8.8), and it is here because it is the newest thing in
    // the state hash that is *computed* rather than authored. `illuminate` runs
    // `sqrt`, division and a slab test over every receiver every tick, and
    // `light.rs` claims in its own header that those agree on two platforms
    // because IEEE-754 requires them to be correctly rounded. **Nothing checked
    // that claim until this case existed**: `determinism-case.ron` carries no
    // light, so the scene-file case above compares a world in which `illuminate`
    // writes nothing at all.
    //
    // `lit.ron` puts a receiver at each of the three-ray fan's four outcomes, so
    // the comparison covers a fully lit reading, a fully dark one and both
    // partial ones rather than only the extremes.
    //
    // **The named limit:** the scene is static, so this compares the arithmetic
    // and not its evolution — one tick's worth of computation, repeated. A case
    // whose receivers moved would be stronger, and the way to get one is a light
    // over a physics scene; that would move `physics_drop.ron`'s hash, which is
    // another task's evidence, so it is written down here rather than taken.
    //
    // No recording: the scene consumes no input, the same reason `physics` has
    // none.
    Case {
        name: "scene-file.lit.t1000",
        args: &[
            "--mode",
            "scene-file",
            "--scene",
            "crates/narvo-app/scenes/lit.ron",
            "--ticks",
            "1000",
        ],
        record: false,
    },
];

/// Runs the matrix and writes its artifacts into `directory`.
pub fn record(directory: &Path) -> ExitCode {
    let binary = match narvo_binary() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("xtask: {message}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = fs::create_dir_all(directory) {
        eprintln!("xtask: cannot create {}: {error}", directory.display());
        return ExitCode::FAILURE;
    }

    let mut manifest = String::from(MANIFEST_MAGIC);
    manifest.push('\n');

    for case in CASES {
        let mut command = Command::new(&binary);
        command.args(case.args);

        let recording = directory.join(format!("{}.rec", case.name));
        if case.record {
            command.arg("--record").arg(&recording);
        }
        command.arg("--dump");

        let output = match command.output() {
            Ok(output) => output,
            Err(error) => {
                eprintln!(
                    "xtask: could not run {} for case {}: {error}",
                    binary.display(),
                    case.name
                );
                return ExitCode::FAILURE;
            }
        };

        if !output.status.success() {
            eprintln!("xtask: case {} failed:", case.name);
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            return ExitCode::FAILURE;
        }

        // The dump is the process's stdout, byte for byte. Written with `write`
        // rather than a redirect so no shell can reinterpret the line endings on
        // the way - the two platforms have to produce the same bytes, and a
        // comparison that normalised them first would be comparing something
        // else.
        let dump = directory.join(format!("{}.dump", case.name));
        if let Err(error) = fs::write(&dump, &output.stdout) {
            eprintln!("xtask: cannot write {}: {error}", dump.display());
            return ExitCode::FAILURE;
        }
        manifest.push_str(&format!("{}.dump\n", case.name));

        if case.record {
            if !recording.is_file() {
                eprintln!(
                    "xtask: case {} was asked to record but wrote no {}",
                    case.name,
                    recording.display()
                );
                return ExitCode::FAILURE;
            }
            manifest.push_str(&format!("{}.rec\n", case.name));
        }
    }

    let manifest_path = directory.join(MANIFEST);
    if let Err(error) = fs::write(&manifest_path, &manifest) {
        eprintln!("xtask: cannot write {}: {error}", manifest_path.display());
        return ExitCode::FAILURE;
    }

    eprintln!(
        "xtask: recorded {} cases into {}",
        CASES.len(),
        directory.display()
    );
    ExitCode::SUCCESS
}

/// Compares two directories produced by [`record`].
pub fn compare(left: &Path, right: &Path) -> ExitCode {
    let (left_manifest, right_manifest) = match (read(left, MANIFEST), read(right, MANIFEST)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(message), _) | (_, Err(message)) => {
            eprintln!("xtask: {message}");
            eprintln!(
                "xtask: both sides have to be directories written by \
                 `cargo xtask determinism record`. A missing one means a job did not run \
                 or its artifact was not downloaded - which would otherwise look exactly \
                 like a comparison that passed."
            );
            return ExitCode::FAILURE;
        }
    };

    // Checked before anything is compared. Two sides that agree on every file
    // they happen to share, while one of them is missing half the matrix, is the
    // failure this whole suite exists to rule out: a green result that verified
    // less than it claims.
    if left_manifest != right_manifest {
        eprintln!("xtask: the two sides did not record the same cases.");
        report_manifest_difference(left, &left_manifest, right, &right_manifest);
        return ExitCode::FAILURE;
    }

    let files: Vec<&str> = left_manifest
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .collect();

    if files.is_empty() {
        eprintln!(
            "xtask: the manifests list no files at all, so there is nothing to compare. \
             That is a failure, not a pass."
        );
        return ExitCode::FAILURE;
    }

    // The loop below walks the *manifest*, never the directory. Without this
    // check a file that is in an artifact and not in its manifest is therefore
    // not compared at all: it can sit on both sides, hold different bytes on
    // each, and the run still ends with "every listed file identical".
    //
    // That is not a hypothesis. M5b.3 measured it on this exact code before the
    // check existed - a `physics.t1000.dump` planted into both directories with
    // one number changed on one side came back as `21 files identical`, exit 0.
    // It is the same class as the empty manifest above, one step further in:
    // an answer that looks like a pass and was never asked the question.
    //
    // The directory is read at run time and compared against the manifest that
    // was just read at run time. Neither side is compiled in, which is the shape
    // the drift guards in `main.rs` use and for the same reason: a baked-in
    // expectation reports on the tree as it stood when the binary was built.
    for directory in [left, right] {
        let unlisted = match unlisted_entries(directory, &files) {
            Ok(unlisted) => unlisted,
            Err(message) => {
                eprintln!("xtask: {message}");
                return ExitCode::FAILURE;
            }
        };

        if !unlisted.is_empty() {
            eprintln!(
                "xtask: {} holds entries the manifest does not list:",
                directory.display()
            );
            for name in unlisted {
                eprintln!("        {name}");
            }
            eprintln!(
                "xtask: the comparison walks the manifest, not the directory, so an \
                 unlisted file is never looked at - it would differ on the two sides \
                 and the run would still report every listed file identical. Either \
                 the manifest lost an entry, or something other than \
                 `cargo xtask determinism record` wrote into the directory."
            );
            return ExitCode::FAILURE;
        }
    }

    for file in &files {
        let (a, b) = match (read(left, file), read(right, file)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(message), _) | (_, Err(message)) => {
                eprintln!("xtask: {message}");
                return ExitCode::FAILURE;
            }
        };

        if a == b {
            continue;
        }

        eprintln!(
            "xtask: {} and {} disagree about {file}.",
            left.display(),
            right.display()
        );
        eprintln!();
        eprintln!("{}", locate(file, &a, &b));
        eprintln!();
        eprintln!(
            "xtask: this is a cross-platform divergence. Both sides were built from one \
             commit and one Cargo.lock, so it is the simulation that differs, not the \
             dependency set."
        );
        return ExitCode::FAILURE;
    }

    eprintln!(
        "xtask: {} files identical across {} and {}",
        files.len(),
        left.display(),
        right.display()
    );
    ExitCode::SUCCESS
}

/// Says which cases each side has that the other does not.
fn report_manifest_difference(left: &Path, a: &str, right: &Path, b: &str) {
    let only = |mine: &str, theirs: &str| -> Vec<String> {
        mine.lines()
            .filter(|line| !line.trim().is_empty())
            .filter(|line| !theirs.lines().any(|other| other == *line))
            .map(str::to_owned)
            .collect()
    };

    for (side, missing) in [(left, only(a, b)), (right, only(b, a))] {
        if missing.is_empty() {
            continue;
        }
        eprintln!("    only in {}:", side.display());
        for line in missing {
            eprintln!("        {line}");
        }
    }
}

/// Describes the first place two versions of one artifact differ.
///
/// For a dump this walks the canonical form and reports the entity and the
/// component, because "the hashes differ" is a fact nobody can act on. The case
/// name carries the mode and the tick count, so the four things a reader needs —
/// case, tick, entity, component — are all in the message.
///
/// # This duplicates `narvo_ecs::first_difference` on purpose
///
/// M2.6 moved that logic into `narvo-ecs`, where it belongs, and every caller
/// in the engine now uses it. This copy stays, and the reason is a trade rather
/// than an oversight: **`xtask` has no dependencies and is meant to keep none.**
/// Depending on `narvo-ecs` would make `cargo xtask ci` build the ECS tree —
/// hecs, serde, ron — before it can run its first step, on every invocation,
/// against the iteration budget of `ProjektPlan.md` §8.1. Forty lines of line
/// walking is the cheaper side of that trade.
///
/// It also handles a case the engine's version does not: a `.rec` file, which is
/// not a dump and has no entities in it.
///
/// If this is ever "cleaned up" by adding the dependency, the thing to measure
/// first is what it does to a warm `cargo xtask ci`.
fn locate(file: &str, left: &str, right: &str) -> String {
    let mut report = String::new();
    let mut entity = String::from("(before the first entity line)");

    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();

    for (index, (a, b)) in left_lines.iter().zip(right_lines.iter()).enumerate() {
        if a.starts_with("entity ") {
            (*a).clone_into(&mut entity);
        }

        if a == b {
            continue;
        }

        report.push_str(&format!("    case      {file}\n"));
        if file.ends_with(".dump") {
            report.push_str(&format!("    {entity}\n"));
            // A component line is the indented one; its first token is the
            // stable name the registry knows it by.
            if let Some(name) = a.strip_prefix("  ").and_then(|rest| rest.split(' ').next()) {
                report.push_str(&format!("    component {name}\n"));
            }
        }
        report.push_str(&format!("    line      {}\n", index + 1));
        report.push_str(&format!("    left      {a}\n"));
        report.push_str(&format!("    right     {b}\n"));
        return report;
    }

    // Same prefix, different length: one side stopped early or carried on.
    report.push_str(&format!("    case      {file}\n"));
    report.push_str(&format!(
        "    the two agree for {} lines and then differ in length: left has {}, right has {}\n",
        left_lines.len().min(right_lines.len()),
        left_lines.len(),
        right_lines.len()
    ));

    let longer = if left_lines.len() > right_lines.len() {
        ("left", &left_lines)
    } else {
        ("right", &right_lines)
    };
    if let Some(extra) = longer.1.get(left_lines.len().min(right_lines.len())) {
        report.push_str(&format!(
            "    first extra line, on the {}: {extra}\n",
            longer.0
        ));
    }

    report
}

/// Every entry of `directory` that `listed` does not name.
///
/// [`MANIFEST`] is excluded because it is the list rather than an item on it; a
/// manifest naming itself would have to be written by hand into every recording
/// and would say nothing.
///
/// Sorted before it is returned. `read_dir` promises no order whatsoever, and a
/// failure message that names the same two files in a different order on each
/// platform is harder to compare than one that does not.
fn unlisted_entries(directory: &Path, listed: &[&str]) -> Result<Vec<String>, String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;

    let mut unlisted = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("cannot read an entry of {}: {error}", directory.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if name != MANIFEST && !listed.contains(&name.as_str()) {
            unlisted.push(name);
        }
    }

    unlisted.sort();
    Ok(unlisted)
}

/// Reads one file out of an artifact directory, naming it if it is not there.
fn read(directory: &Path, file: &str) -> Result<String, String> {
    let path = directory.join(file);
    fs::read_to_string(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// The `narvo` binary next to this one.
///
/// Found through `current_exe` rather than by guessing at `target/debug`,
/// because the target directory is configured per platform — on this project's
/// WSL side it lives inside the distro, outside the working copy entirely.
/// Whatever built xtask put `narvo` beside it.
fn narvo_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot locate this executable: {error}"))?;
    let directory = exe
        .parent()
        .ok_or_else(|| "this executable has no parent directory".to_owned())?;

    let name = if cfg!(windows) { "narvo.exe" } else { "narvo" };
    let candidate = directory.join(name);

    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(format!(
            "{} does not exist. Build it first:\n\n    cargo build --workspace\n",
            candidate.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{CASES, MANIFEST, MANIFEST_MAGIC, compare, locate, unlisted_entries};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::ExitCode;

    #[test]
    fn every_case_has_a_distinct_name() {
        // The name is the file stem and the handle a failure message uses. Two
        // cases sharing one would have the second silently overwrite the first,
        // and the matrix would quietly shrink.
        let mut names: Vec<&str> = CASES.iter().map(|case| case.name).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();

        assert_eq!(names.len(), total, "two cases share a name");
    }

    /// Modes the matrix deliberately does not carry, each with its reason.
    ///
    /// **The list is the point, not the omission.** A mode that is not compared
    /// across platforms is a mode whose cross-platform agreement nobody checks,
    /// and that is a decision rather than an oversight — so it is written down,
    /// beside the mode, where the next reader finds it.
    ///
    /// The reason each entry gives is prose and nothing verifies it. What the
    /// guard below verifies is that the entry *exists*: a mode may leave the
    /// matrix, but not silently.
    const NOT_IN_THE_MATRIX: &[(&str, &str)] = &[(
        "scene",
        "M3.32 recorded extending CASES to the drawing mode as \"the named, \
         deliberate omission of §2\" and checked its cross-platform agreement by \
         hand on Windows and WSL instead. The in-process suite in \
         crates/narvo-app/tests/determinism.rs does carry it, so what is missing \
         is the two-machine half rather than all coverage.",
    )];

    /// Every mode the binary offers, read out of `cli.rs` at run time.
    ///
    /// `MODE_NAMES` is the string `--mode` puts in its error message, and
    /// `the_usage_text_mentions_every_mode_and_every_flag_that_exists` in that
    /// same file already fails if a mode is added without appearing in it. So it
    /// is a list that cannot go stale in the direction that matters here, which
    /// is what makes it usable as a source.
    ///
    /// Read from disk rather than compiled in, the shape `main.rs`'s drift
    /// guards use: `xtask` has no dependency on `narvo-app` and is meant to keep
    /// none, and a baked-in copy would answer about the modes as they stood when
    /// this binary was built.
    fn modes_the_binary_offers() -> Vec<String> {
        let path = crate::workspace_root()
            .join("crates")
            .join("narvo-app")
            .join("src")
            .join("cli.rs");
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));

        let (_, after) = source.split_once("const MODE_NAMES").unwrap_or_else(|| {
            panic!(
                "{} no longer has a `const MODE_NAMES`, which is where this guard \
                 learns which modes exist. If it was renamed, rename it here too; \
                 a guard that cannot find its source reports green while checking \
                 nothing.",
                path.display()
            )
        });
        let declaration = after
            .split_once(';')
            .expect("a const declaration ends in a semicolon")
            .0;

        let names: Vec<String> = declaration
            .split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect();

        assert!(
            !names.is_empty(),
            "`MODE_NAMES` in {} names no modes in backticks, so this guard has \
             nothing to check the matrix against.",
            path.display()
        );

        names
    }

    /// Every mode is either in the matrix or excused in writing.
    ///
    /// **Replaces a hard-coded list of three.** That list predated `scene`,
    /// `scene-file` and `physics`, and it could not have noticed any of them: a
    /// guard that enumerates what it expects can only ever check the past. The
    /// property that should actually hold is the one stated here — *no mode
    /// leaves the cross-platform comparison without somebody writing down why* —
    /// and it holds for modes that do not exist yet.
    ///
    /// Deliberately **not** the other repair, which was weighed and rejected:
    /// widening the guard to demand every mode, and extending the matrix until it
    /// passes. Its argument is real and is the stronger one on coverage — a mode
    /// nobody compares is a gap, and M3.32's hand-check of `scene` on one machine
    /// is not the two-machine evidence the suite exists for. What defeats it is
    /// that it decides §2's question by way of a test: `scene` at five
    /// checkpoints is five more artifacts and a drawing scenario's worth of CI
    /// time, and M3.32 declined that on purpose. A guard should not quietly
    /// reverse a decision it was written to protect. This shape leaves the
    /// decision where it was made and makes the next one visible.
    #[test]
    fn every_mode_is_in_the_matrix_or_excused_in_writing() {
        let modes = modes_the_binary_offers();

        for mode in &modes {
            let in_matrix = CASES
                .iter()
                .any(|case| case.name == *mode || case.name.starts_with(&format!("{mode}.")));
            let excused = NOT_IN_THE_MATRIX.iter().find(|(name, _)| name == mode);

            match (in_matrix, excused) {
                (true, None) | (false, Some(_)) => {}
                (false, None) => panic!(
                    "mode `{mode}` is offered by the binary, is not in the \
                     cross-platform matrix, and is not in NOT_IN_THE_MATRIX. \
                     Either add cases for it or add it there with the reason it \
                     is left out - what is not allowed is neither, because that \
                     is a mode whose two-platform agreement nobody checks and \
                     nobody decided not to check."
                ),
                (true, Some(_)) => panic!(
                    "mode `{mode}` is in the matrix *and* in NOT_IN_THE_MATRIX, \
                     so the reason recorded there is describing something that is \
                     no longer true. Drop the entry."
                ),
            }
        }

        for (excused, _) in NOT_IN_THE_MATRIX {
            assert!(
                modes.iter().any(|mode| mode == excused),
                "NOT_IN_THE_MATRIX excuses `{excused}`, which is not a mode the \
                 binary offers. Either it was renamed or it is a typo, and either \
                 way the entry excuses nothing while looking like it does."
            );
        }
    }

    #[test]
    fn the_matrix_covers_the_checkpoints() {
        for ticks in ["t1", "t100", "t1000", "t5000", "t10000"] {
            assert!(
                CASES.iter().any(|case| case.name.ends_with(ticks)),
                "the matrix has no case at {ticks}"
            );
        }
        assert!(
            CASES.iter().any(|case| case.record),
            "no case records, so the recording artifact is never compared"
        );
    }

    #[test]
    fn a_differing_component_is_reported_with_its_entity_and_name() {
        let left =
            "entities 2\nentity 0v1\n  position (x:1,y:2)\nentity 1v1\n  position (x:5,y:6)\n";
        let right =
            "entities 2\nentity 0v1\n  position (x:1,y:2)\nentity 1v1\n  position (x:5,y:7)\n";

        let report = locate("chance.seed1.t10000.dump", left, right);

        assert!(report.contains("chance.seed1.t10000.dump"), "{report}");
        assert!(report.contains("entity 1v1"), "{report}");
        assert!(report.contains("component position"), "{report}");
        assert!(
            report.contains("(x:5,y:6)") && report.contains("(x:5,y:7)"),
            "{report}"
        );
    }

    #[test]
    fn a_difference_before_any_entity_is_still_reported() {
        let report = locate("motion.t1.dump", "entities 2\n", "entities 3\n");

        assert!(report.contains("before the first entity line"), "{report}");
        assert!(
            report.contains("entities 2") && report.contains("entities 3"),
            "{report}"
        );
    }

    #[test]
    fn a_length_difference_is_reported_rather_than_passing() {
        // One side stopping early shares a prefix with the other, so a naive
        // line-by-line walk would run out and report nothing.
        let left = "entities 1\nentity 0v1\n  position (x:1,y:2)\n";
        let right = "entities 1\nentity 0v1\n";

        let report = locate("input.seed1.t100.dump", left, right);

        assert!(report.contains("differ in length"), "{report}");
        assert!(report.contains("first extra line"), "{report}");
    }

    /// The one case name the filesystem tests write, so the manifest and the
    /// file it lists cannot drift apart inside a helper.
    const LISTED: &str = "motion.t1.dump";

    /// A fresh directory under the system temp directory.
    ///
    /// Built out of `std` alone, because `xtask/Cargo.toml` has no dependencies
    /// and this is not the reason to give it one. `nextest` runs each test in
    /// its own process, so the process id separates two tests and `label`
    /// separates the two sides of one.
    fn temp_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("narvo-xtask-{}-{label}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("cannot create {}: {error}", path.display()));
        path
    }

    /// Writes a one-case recording: a manifest, and the single file it lists.
    fn write_artifact(directory: &Path, dump: &str) {
        let manifest = format!("{MANIFEST_MAGIC}\n{LISTED}\n");
        for (name, contents) in [(MANIFEST, manifest.as_str()), (LISTED, dump)] {
            let path = directory.join(name);
            fs::write(&path, contents)
                .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        }
    }

    /// **The guard for the gap M5b.3 measured**: the manifest is the gate, and
    /// anything beside it is a failure rather than a file nobody looks at.
    ///
    /// [`compare`] walks the manifest, not the directory. Before this check a
    /// file that was in both artifacts and in neither manifest was never read —
    /// it could hold different bytes on the two platforms and the run still
    /// ended with "21 files identical", exit 0. That is the failure this whole
    /// suite exists to rule out, wearing the suite's own clothes: a green that
    /// verified less than it claims.
    ///
    /// The control half matters as much as the assertion. Without comparing the
    /// two clean directories first, this test would also pass if `compare` had
    /// simply stopped agreeing about anything.
    #[test]
    fn a_file_the_manifest_does_not_list_fails_the_comparison_instead_of_being_skipped() {
        let (left, right) = (
            temp_directory("unlisted-left"),
            temp_directory("unlisted-right"),
        );
        let dump = "entities 1\nentity 0v1\n  position (x:1,y:2)\n";
        write_artifact(&left, dump);
        write_artifact(&right, dump);

        let clean = compare(&left, &right);

        // The same two artifacts, plus one file neither manifest names, holding
        // different bytes on the two sides.
        for (directory, contents) in [(&left, "entities 1\n"), (&right, "entities 2\n")] {
            let path = directory.join("physics.t1000.dump");
            fs::write(&path, contents)
                .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        }

        let with_stray = compare(&left, &right);
        let named = unlisted_entries(&left, &[LISTED]).expect("the directory is readable");

        // Cleaned up before the assertions, so a failing one does not leave two
        // directories behind in the system temp directory every run.
        let _ = fs::remove_dir_all(&left);
        let _ = fs::remove_dir_all(&right);

        assert_eq!(
            clean,
            ExitCode::SUCCESS,
            "two identical one-case artifacts have to compare equal, or the \
             assertion below would pass for the wrong reason"
        );
        assert_ne!(
            with_stray,
            ExitCode::SUCCESS,
            "a file that is in both artifacts and in neither manifest was passed \
             over silently. It differed between the two sides, and the comparison \
             still succeeded - which is exactly the shape of green this suite is \
             built to prevent"
        );
        assert_eq!(
            named,
            vec!["physics.t1000.dump".to_owned()],
            "the unlisted file has to be named, not merely counted: a message \
             that says a directory has a stray without saying which one leaves \
             the reader to diff two artifact directories by hand"
        );
    }

    /// The manifest is not a stray in its own directory.
    ///
    /// It is the list rather than an item on it, so it is excluded by name. Were
    /// that exclusion to go, every comparison in the project would fail at once
    /// and the guard above would look like the defect.
    #[test]
    fn the_manifest_itself_is_not_reported_as_unlisted() {
        let directory = temp_directory("manifest-not-a-stray");
        write_artifact(&directory, "entities 0\n");

        let unlisted = unlisted_entries(&directory, &[LISTED]).expect("the directory is readable");

        let _ = fs::remove_dir_all(&directory);

        assert!(
            unlisted.is_empty(),
            "a directory holding nothing but its manifest and the file that \
             manifest lists has no unlisted entries, got {unlisted:?}"
        );
    }

    #[test]
    fn a_recording_difference_is_reported_without_entity_context() {
        // A `.rec` file has no entities in it, so the dump-shaped fields would
        // be noise; the line and both sides are what there is to say.
        let report = locate(
            "input.seed1.t10000.rec",
            "narvo-recording 1\n3 thrust 2\nend\n",
            "narvo-recording 1\n3 thrust 1\nend\n",
        );

        assert!(report.contains("input.seed1.t10000.rec"), "{report}");
        assert!(!report.contains("component"), "{report}");
        assert!(
            report.contains("3 thrust 2") && report.contains("3 thrust 1"),
            "{report}"
        );
    }
}
