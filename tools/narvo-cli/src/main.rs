//! `narvo-cli` — the engine's command-line tools.
//!
//! Today one subcommand: `validate`, which checks a scene file and prints what
//! is wrong with it.
//!
//! # This binary is deliberately thin
//!
//! Everything it knows about scenes it asks `narvo-scene`. What lives here is
//! argument parsing, the registry to check against, and one line of text per
//! finding. That split is the point: the hot-reload log of a later M4 task and
//! M6's introspection service are two more consumers of the same findings, and a
//! check that lived in this binary would be unreachable to both.
//!
//! # The output format
//!
//! ```text
//! scenes/example.ron:12:24: error: entity 3 carries "positon", which no component is …
//! scenes/example.ron:4:18: warning: entity 0 is named with the empty string, which …
//! 1 error, 1 warning
//! ```
//!
//! `file:line:column: severity: message` is what a compiler prints and what an
//! editor's error list, a `grep`, and an agent reading a transcript all already
//! know how to read. Nothing about it is new, which is the reason to use it.
//!
//! Findings go to **stdout**, not stderr: they are this program's output, not a
//! complaint about running it. Stderr carries the two things that are — a usage
//! mistake and a file that could not be read.
//!
//! # Exit codes
//!
//! | code | meaning |
//! | --- | --- |
//! | 0 | the scene has no errors (and no warnings, under `--deny-warnings`) |
//! | 1 | the scene has findings that count as errors |
//! | 2 | the tool could not do its job: bad arguments, or the file could not be read |
//!
//! The third is an addition to what M4.2 asked for, and it earns its place: a
//! script that cannot tell "the scene is broken" from "you spelled the path
//! wrong" will treat a typo as a content failure. `0`/`1` keep exactly the
//! meanings they were given.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use narvo_ecs::{ComponentRegistry, EcsError, register_engine_components};
use narvo_scene::ValidationReport;

/// What `--help` prints, and what a usage mistake prints after its complaint.
const USAGE: &str = "\
narvo-cli — tools for the Narvo engine

USAGE:
    narvo-cli validate <scene.ron> [--deny-warnings]

SUBCOMMANDS:
    validate    Check a scene file and report everything wrong with it.

OPTIONS:
    --deny-warnings    Treat warnings as errors, so any finding fails the run.
    -h, --help         Print this message.

EXIT CODES:
    0    no findings that count as errors
    1    the scene has errors
    2    bad arguments, or the file could not be read
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match run(&arguments) {
        Ok(code) => code,
        Err(complaint) => {
            eprintln!("narvo-cli: {complaint}");
            eprint!("\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Does the work, or says what the caller got wrong.
///
/// Split from `main` so that the failure path has a type rather than a pair of
/// `eprintln!`s in two places.
fn run(arguments: &[String]) -> Result<ExitCode, String> {
    let mut arguments = arguments.iter().map(String::as_str);

    match arguments.next() {
        None => Err("no subcommand. Try `narvo-cli validate <scene.ron>`".to_owned()),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some("validate") => validate(arguments),
        Some(other) => Err(format!("unknown subcommand `{other}`")),
    }
}

/// `validate <file> [--deny-warnings]`.
fn validate<'a>(arguments: impl Iterator<Item = &'a str>) -> Result<ExitCode, String> {
    let mut path: Option<PathBuf> = None;
    let mut deny_warnings = false;

    for argument in arguments {
        match argument {
            "--deny-warnings" => deny_warnings = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option `{other}`"));
            }
            other if path.is_some() => {
                return Err(format!(
                    "`validate` takes one file, and `{other}` is a second one"
                ));
            }
            other => path = Some(PathBuf::from(other)),
        }
    }

    let path = path.ok_or("`validate` needs a scene file to check")?;
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;

    let registry = engine_registry().map_err(|error| format!("building the registry: {error}"))?;
    let report = narvo_scene::validate(&text, &registry);

    print(&path, &report);

    let failed = !report.loads() || (deny_warnings && report.warnings() > 0);
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Prints every finding, then one line saying how many there were.
fn print(path: &Path, report: &ValidationReport) {
    let path = path.display();

    for finding in report.findings() {
        match finding.location() {
            Some(location) => println!(
                "{path}:{location}: {}: {}",
                finding.severity(),
                finding.message()
            ),
            // A finding the loader could not place still has to be printed, and
            // printing it without the two colons keeps the located form
            // unambiguous for anything parsing these lines.
            None => println!("{path}: {}: {}", finding.severity(), finding.message()),
        }
    }

    println!("{}", summary(report));
}

/// The closing line: what was found, in words rather than as a status code.
fn summary(report: &ValidationReport) -> String {
    if report.is_empty() {
        return "no findings".to_owned();
    }

    let mut parts = Vec::new();
    if report.errors() > 0 {
        parts.push(plural(report.errors(), "error"));
    }
    if report.warnings() > 0 {
        parts.push(plural(report.warnings(), "warning"));
    }

    parts.join(", ")
}

/// `1 error` / `2 errors`.
fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The component types the engine itself offers, under their stable names.
///
/// # One list, in the engine, since M4.3
///
/// It used to be spelled out here. M4.3 added a second consumer — the runner's
/// `scene-file` mode, which constitutes a world from a file and has to register
/// exactly what a scene may carry — and two hand-kept copies of one list is one
/// place too many to forget a new type. The list moved to
/// `narvo_ecs::register_engine_components`, which is the engine making a
/// statement about its own components.
///
/// **That is not what ADR-0018 forbids.** The ADR keeps a type list out of
/// `narvo-scene`, so the *format* stays component-open and a scene may carry
/// whatever its caller registered. This is the opposite direction, and a caller
/// is still free to register these, some of them, or none, plus its own.
///
/// # Errors
///
/// Anything the registry raises, which for a fresh registry is nothing.
fn engine_registry() -> Result<ComponentRegistry, EcsError> {
    let mut registry = ComponentRegistry::new();
    register_engine_components(&mut registry)?;

    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::{engine_registry, plural, summary};
    use narvo_scene::ValidationReport;

    #[test]
    fn the_registry_holds_the_engine_component_set() {
        let registry = engine_registry().expect("a fresh registry accepts each once");
        let names: Vec<&str> = registry.iter().map(|info| info.name()).collect();

        assert_eq!(
            names,
            vec![
                "burst",
                "camera",
                "follow",
                "hitrect",
                "layer",
                "lightsource",
                "lit",
                "occluder",
                "rigidbody",
                "rng",
                "sampling",
                "shake",
                "sprite",
                "tally",
                "tint",
                "transform"
            ],
            "adding an engine component type means adding it here too"
        );
    }

    #[test]
    fn an_empty_report_says_so_rather_than_saying_zero() {
        assert_eq!(summary(&ValidationReport::default()), "no findings");
    }

    #[test]
    fn counts_are_pluralised() {
        assert_eq!(plural(1, "error"), "1 error");
        assert_eq!(plural(0, "error"), "0 errors");
        assert_eq!(plural(2, "warning"), "2 warnings");
    }
}
