//! Command line parsing.
//!
//! Hand-rolled on purpose. The surface is a handful of flags, and a parser
//! library would be a dependency the headless build has to carry for no benefit.
//! This module is deliberately free of any feature gate: which commands can
//! actually *run* depends on the `render` feature, but which ones can be *named*
//! does not, so the parser and its tests work in every configuration.

use std::fmt;
use std::iter::Peekable;
use std::path::PathBuf;

use crate::sim::Mode;

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open a window and show the render output. The default.
    Window(WindowOptions),
    /// Run the simulation with no window and no GPU.
    Headless(HeadlessOptions),
    /// Render one frame offscreen, write it to this path as a PNG, and exit.
    Screenshot(PathBuf),
    /// Run the frame loop for a fixed number of frames, report the timings, exit.
    Frames(FrameOptions),
    /// Print usage and exit.
    Help,
}

/// What a windowed run was asked for.
///
/// A struct rather than a second tuple field, following [`HeadlessOptions`] and
/// [`FrameOptions`]: two independent paths is the point at which a name per
/// field earns itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowOptions {
    /// The scene file to constitute the world from, and watch.
    ///
    /// `Some(path)` when `--scene <path>` was given without anything that
    /// implies a headless run: the world comes from that file and the window
    /// watches it for changes (ADR-0022). `None` is the sprite-field demo,
    /// unchanged.
    pub scene: Option<PathBuf>,
    /// The input mapping to drive the keyboard through.
    ///
    /// `None` means the window behaves exactly as it did before M5.3: keys do
    /// nothing except the built-in Escape. **There is deliberately no default
    /// mapping.** A file the engine invents would be a set of bindings nobody
    /// wrote, and `ProjektPlan.md` §2 excludes shipping one on the chance it is
    /// wanted.
    pub mapping: Option<PathBuf>,
}

/// How a frame-time measurement is configured.
///
/// The measurement `ProjektPlan.md` §6/M3 asks for is hardware-bound and cannot
/// be a test, so it is a command: something a person runs on the reference
/// machine and pastes into `docs/perf/BASELINE.md`. Every knob here changes what
/// the number means, which is why each one is reported beside the table rather
/// than assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameOptions {
    /// Frames to run after the warm-up.
    pub frames: usize,
    /// Frames to run and discard before measuring.
    pub warmup: usize,
    /// Sprites in the scene.
    pub sprites: usize,
    /// Take a present mode that does not wait for the display.
    ///
    /// The throughput question is about the work, and a vsync-paced run answers
    /// it with the display's refresh rate. This is how the display is taken out
    /// of the number; leaving it off is the other statement, the one about
    /// whether the loop holds a rate.
    pub uncapped: bool,
    /// Block on the GPU each frame, so its execution time is attributable.
    ///
    /// Changes what is measured — it serialises CPU and GPU instead of letting
    /// them overlap — so it produces a separate row rather than the main table.
    pub drain: bool,
    /// The scene to run instead of the sprite-field demo, if any.
    ///
    /// A measured run over a scene file is what makes the hot-reload latency of
    /// §6/M4 scriptable: `--frames N` ends by itself, so a script can start it,
    /// edit the file mid-run, and read the log afterwards.
    pub scene: Option<PathBuf>,
    /// Ask the window for a fixed series of sizes as the run goes.
    ///
    /// **An instrument, not a feature** (M7.1c). A surface reconfigure costs
    /// this machine two seconds per swapchain image inside `acquire`, and until
    /// that measurement nobody had resized a window and then looked at the phase
    /// table — the defect was present through M1 to M6b behind 1 367 tests. This
    /// makes the reproduction one command: `narvo --frames 400 --resize-probe`,
    /// and the `acquire` row of the table beneath it is the answer.
    ///
    /// It drives the window itself rather than waiting for a hand, so the same
    /// sequence runs on any platform — which is what a comparison between two of
    /// them needs.
    pub resize_probe: bool,
}

/// How a headless run is configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessOptions {
    /// Which demo simulation to drive. Ignored when replaying, which takes the
    /// mode from the recording.
    pub mode: Mode,
    /// The seed handed to the simulation. Ignored when replaying, for the same
    /// reason.
    pub seed: u64,
    /// Simulation ticks to execute.
    ///
    /// `None` means the default for a live run, and the recording's full length
    /// for a replay — which is why it is an option rather than a number with a
    /// default already applied: a replay has no business running for 600 ticks
    /// because nobody said otherwise.
    pub ticks: Option<u64>,
    /// What the run writes to stdout when it is over.
    pub report: Report,
    /// Where to write the run's input stream, if anywhere.
    pub record: Option<PathBuf>,
    /// A recording to replay instead of generating input.
    pub replay: Option<PathBuf>,
    /// The scene file a `scene-file` run is constituted from.
    ///
    /// Required by that mode and refused by every other, because a mode built
    /// from code has nothing to do with a file. On a replay it is not given at
    /// all: the recording says which scene it was made against, and a second
    /// opinion could only disagree (ADR-0019).
    pub scene: Option<PathBuf>,
    /// A canonical dump this run's final state has to be, if one was named.
    ///
    /// The repro runner's whole input beyond the run itself (M6.7a): the file
    /// holds a state an earlier run produced, and this run is judged against it.
    /// It never carries a state *hash* — a hash says that two states differ and
    /// never where, and the diagnosis is the point.
    ///
    /// Independent of [`report`](Self::report) rather than a fourth value of it:
    /// `--dump` says what goes to stdout, this says what the exit code means, and
    /// a run may sensibly do both.
    pub expect: Option<PathBuf>,
    /// Where to listen for an agent, if anywhere.
    ///
    /// **Parsed in every build and served only by one.** The field is not behind
    /// the `ipc` feature, so this struct has one shape and the parser has one set
    /// of tests; what the feature decides is whether `main.rs` can act on it or
    /// has to refuse it. That is the same split `--screenshot` and `--frames`
    /// already have with `render`, and it keeps the refusal a message rather than
    /// an unknown-flag error, which is what tells a user their build is the
    /// problem rather than their spelling.
    pub ipc: Option<String>,
}

/// What a headless run puts on stdout.
///
/// Exactly one of these per invocation, and nothing else goes to stdout at all -
/// diagnostics go to stderr. That is what makes `narvo --hash > a.txt` on one
/// platform and on another two files a `diff` can compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// A line of prose about what the run did. The default.
    Summary,
    /// The 64-bit state hash as sixteen hex digits, on its own line.
    Hash,
    /// The full canonical dump of the final state.
    Dump,
}

/// Ticks a headless run executes unless `--ticks` says otherwise.
pub const DEFAULT_TICKS: u64 = 600;

/// Seed a headless run uses unless `--seed` says otherwise.
///
/// Arbitrary, as any seed is. What matters is that it is fixed and written down
/// in one place: two machines that both leave `--seed` alone are comparing the
/// same simulation, which is what the cross-platform check needs.
pub const DEFAULT_SEED: u64 = 1;

/// What `--mode` accepts, as the error message spells it.
///
/// Kept next to the parser rather than generated from
/// [`Mode::ALL`](crate::sim::Mode::ALL) because it is prose; a test below fails
/// if a mode is ever added without appearing here.
const MODE_NAMES: &str = "one of `motion`, `chance`, `input`, `scene`, `scene-file` or `physics`";

/// Frames a measurement runs unless `--frames` says otherwise.
///
/// A thousand, which is the sample size `ProjektPlan.md` §6/M3's distribution is
/// asked over. At 60 Hz that is about seventeen seconds of window.
pub const DEFAULT_FRAMES: usize = 1_000;

/// Frames discarded before measuring, unless `--warmup` says otherwise.
///
/// The first frames of a run allocate every buffer for the first time and fill a
/// swapchain that starts empty, so they describe the start rather than the run.
/// Sixty is about a second.
pub const DEFAULT_WARMUP: usize = 60;

/// Where `--screenshot` writes when no path is given.
///
/// Relative to the working directory, and under `target/` because the
/// repository ignores that. A screenshot taken to look at something should not
/// leave an untracked file in the working tree, which is exactly what the
/// previous default - the current directory - did.
pub const DEFAULT_SCREENSHOT_PATH: &str = "target/screenshots/screenshot.png";

/// Usage text, printed for `--help` and after a parse error.
pub const USAGE: &str = "\
narvo - runner for the Narvo engine

Usage:
  narvo                     Open a window and show the render output
  narvo --headless          Run the simulation with no window
  narvo --mode NAME         Which simulation to run (implies --headless)
                             motion: integer movement plus one f32 (default)
                             chance: seeded randomness and events
                             input:  driven only by the input stream
                             physics: rigid bodies falling onto a ground
  narvo --seed N            Seed for the simulation (implies --headless, default 1)
  narvo --ticks N           Run N ticks (implies --headless, default 600)
  narvo --record PATH       Write the run's input stream to PATH
  narvo --replay PATH       Replay PATH instead of generating input
  narvo --scene PATH        Constitute the world from a scene file. On its own it
                             opens a window and reloads when the file changes
  narvo --mapping PATH      Bind the window's keyboard through a mapping file.
                             Without it, keys do nothing; there is no default
  narvo --hash              Print the state hash after the run, nothing else
  narvo --dump              Print the canonical state dump after the run
  narvo --expect PATH       Require the run's final state to be the canonical dump
                             in PATH. Exit 1 and say where they part if it is not
  narvo --screenshot [PATH] Render one frame offscreen, write it as a PNG, exit
  narvo --frames N          Run N frames of the scene in a window and time them
  narvo --warmup N          Discard N frames before measuring (default 60)
  narvo --sprites N         Sprites in the measured scene (default 50000)
  narvo --uncapped          Take a present mode that does not wait for the display
  narvo --resize-probe      Resize the window four times while measuring, so the
                             `acquire` row is about a surface reconfigure
  narvo --drain             Block on the GPU each frame, so its cost is attributable
                             Without PATH: target/screenshots/screenshot.png
  narvo --help              Print this text

The simulations --mode accepts are `motion`, `chance`, `input`, `scene`,
`scene-file` and `physics`. `physics` is the one the rigid-body solver drives: a
stack of boxes falling onto a ground, and the first mode whose state a
dependency computes.
`scene-file` builds its world from a scene file given with --scene,
and a recording of such a run refuses to replay against a changed file. Naming
the mode implies --headless; --scene on its own opens a window on the file and
watches it, so editing the file reloads the world between two ticks.
`scene` is the one that draws: a field of sprites and a camera that follows a
wandering target. The window runs it, and --frames measures it.

Everything except --screenshot and the window needs no GPU. Those two need a
build with the `render` feature, which is on by default.

--hash and --dump write to stdout and nothing else does, so

  narvo --mode chance --seed 1 --ticks 10000 --hash > run.txt

is comparable byte for byte between machines.

A recording carries its own mode, seed and length, so --replay takes none of
them. --ticks may still shorten a replay, which is how a divergence is bisected:

  narvo --mode input --ticks 10000 --record bug.rec --hash
  narvo --replay bug.rec --ticks 500 --dump

Those two lines and --expect are a repro test. The first writes the recording,
the second writes down the state it produces, and the third is what anybody runs
afterwards to see whether that state comes back:

  narvo --replay bug.rec --ticks 500 --dump > expected.dump
  narvo --replay bug.rec --ticks 500 --expect expected.dump

The expected state is a file and never a value in this repository (ADR-0035): it
is written by a run and handed over, not committed.";

/// Why a command line could not be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// `--mode scene-file` was given without a scene to constitute a world from.
    SceneRequired,
    /// `--scene` was given for a mode that is built from code.
    SceneWithoutSceneFile {
        /// The mode that was asked for.
        mode: Mode,
    },
    /// `--scene` was given alongside `--replay`.
    SceneWithReplay,
    /// `--mapping` was given for a run that has no keyboard.
    MappingWithoutWindow {
        /// The flag that took the window away, or that made the run unattended.
        other: &'static str,
    },
    /// An argument that is not one of the known flags.
    UnknownArgument(String),
    /// Two flags were given that cannot both apply.
    Conflicting {
        /// The flag that was seen first.
        first: &'static str,
        /// The one that cannot join it.
        second: &'static str,
        /// Why they exclude each other, as a clause.
        ///
        /// Carried rather than baked in: there are four such pairs now and each
        /// conflicts for its own reason. "Cannot both be given" on its own tells
        /// the reader nothing they had not already worked out.
        why: &'static str,
    },
    /// Two flags were given that both say what to print.
    AmbiguousReport(&'static str, &'static str),
    /// A flag that needs a value was given without one.
    MissingValue(&'static str),
    /// A flag's value could not be read as what the flag needs.
    InvalidValue {
        /// The flag whose value was wrong.
        flag: &'static str,
        /// What was given instead.
        value: String,
        /// What the flag would have accepted, as a noun phrase.
        ///
        /// Carried per flag rather than baked into the message: three flags take
        /// a value now and they expect three different things, and "needs a
        /// whole number of ticks" printed for `--mode` sends the reader looking
        /// in the wrong place. Error messages are agent feedback.
        expected: &'static str,
    },
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SceneRequired => f.write_str(
                "`--mode scene-file` builds its world from a scene file, so it needs \
                 `--scene <file>` saying which one",
            ),
            Self::SceneWithoutSceneFile { mode } => write!(
                f,
                "`--scene` applies to `--mode scene-file`, and `{mode}` builds its world in \
                 code; drop one of the two"
            ),
            Self::SceneWithReplay => f.write_str(
                "`--replay` already knows which scene its recording was made against, and \
                 checks the file against it; `--scene` would be a second opinion that could \
                 only disagree",
            ),
            Self::MappingWithoutWindow { other } => write!(
                f,
                "`--mapping` binds keys for a window, and `{other}` asks for a run that has no \
                 keyboard to bind; drop one of the two"
            ),
            Self::UnknownArgument(argument) => {
                write!(f, "unknown argument `{argument}`")
            }
            Self::Conflicting { first, second, why } => {
                write!(f, "`{first}` and `{second}` cannot both be given: {why}")
            }
            Self::AmbiguousReport(first, second) => write!(
                f,
                "`{first}` and `{second}` both say what to print, and a run prints one thing; \
                 give one of them"
            ),
            Self::MissingValue(flag) => {
                write!(f, "`{flag}` needs a value after it")
            }
            Self::InvalidValue {
                flag,
                value,
                expected,
            } => {
                write!(f, "`{flag}` needs {expected}, got `{value}`")
            }
        }
    }
}

impl std::error::Error for CliError {}

/// Parses arguments, which must already have the program name removed.
///
/// # Errors
///
/// [`CliError`] naming what was wrong, so the caller can print it above the
/// usage text rather than making the user guess.
pub fn parse<I>(arguments: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = String>,
{
    let mut arguments = arguments.into_iter().peekable();

    // Which headless-ish flag was seen first, so a conflict with --screenshot
    // can name the flag the caller actually typed.
    let mut headless: Option<&'static str> = None;
    let mut mode: Option<Mode> = None;
    let mut seed: Option<u64> = None;
    let mut ticks: Option<u64> = None;
    let mut report: Option<(&'static str, Report)> = None;
    let mut screenshot = None;
    let mut record: Option<PathBuf> = None;
    let mut replay: Option<PathBuf> = None;
    let mut expect: Option<PathBuf> = None;
    let mut scene: Option<PathBuf> = None;
    let mut ipc: Option<String> = None;
    let mut mapping: Option<PathBuf> = None;
    let mut frames: Option<usize> = None;
    let mut warmup: Option<usize> = None;
    let mut sprites: Option<usize> = None;
    let mut uncapped = false;
    let mut resize_probe = false;
    let mut drain = false;

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => return Ok(Command::Help),
            "--headless" => {
                headless.get_or_insert("--headless");
            }
            "--hash" => {
                set_report(&mut report, "--hash", Report::Hash)?;
                headless.get_or_insert("--hash");
            }
            "--dump" => {
                set_report(&mut report, "--dump", Report::Dump)?;
                headless.get_or_insert("--dump");
            }
            "--mode" => {
                let value = take_value(&mut arguments, "--mode")?;
                let Some(chosen) = Mode::parse(&value) else {
                    return Err(CliError::InvalidValue {
                        flag: "--mode",
                        value,
                        expected: MODE_NAMES,
                    });
                };
                mode = Some(chosen);
                headless.get_or_insert("--mode");
            }
            "--seed" => {
                let value = take_value(&mut arguments, "--seed")?;
                seed = Some(value.parse().map_err(|_| CliError::InvalidValue {
                    flag: "--seed",
                    value,
                    expected: "a whole number",
                })?);
                headless.get_or_insert("--seed");
            }
            "--ticks" => {
                let value = take_value(&mut arguments, "--ticks")?;
                ticks = Some(value.parse().map_err(|_| CliError::InvalidValue {
                    flag: "--ticks",
                    value,
                    expected: "a whole number of ticks",
                })?);
                headless.get_or_insert("--ticks");
            }
            "--ipc" => {
                // Headless-implying: an address to serve an agent on is a
                // statement about how the run is driven, and the window has no
                // seam to serve one from.
                //
                // The parenthetical here used to be "`SceneHost` holds no
                // registry", and since M6.6a it holds one. The flag is
                // unchanged and so is this arm: what keeps the seam out of the
                // window is no longer a missing field but a decision — v1.04
                // measured that moving it there adds nothing an agent cannot
                // already do headless and costs a second answering moment
                // (ADR-0031).
                ipc = Some(take_value(&mut arguments, "--ipc")?);
                headless.get_or_insert("--ipc");
            }
            "--record" => {
                record = Some(PathBuf::from(take_value(&mut arguments, "--record")?));
                headless.get_or_insert("--record");
            }
            "--replay" => {
                replay = Some(PathBuf::from(take_value(&mut arguments, "--replay")?));
                headless.get_or_insert("--replay");
            }
            "--expect" => {
                // Headless-implying, like `--hash` and `--dump`: judging a run
                // against a state is a statement about a simulation, and the
                // window has no final state to be judged on.
                expect = Some(PathBuf::from(take_value(&mut arguments, "--expect")?));
                headless.get_or_insert("--expect");
            }
            "--scene" => {
                // Deliberately does **not** imply `--headless`. It says *what*
                // to run, not *how*: `--mode scene-file --scene x.ron` opens a
                // window and watches the file (M4.6), and the same pair with
                // `--ticks` or `--hash` runs headless because those imply it.
                scene = Some(PathBuf::from(take_value(&mut arguments, "--scene")?));
            }
            "--mapping" => {
                // Deliberately does **not** imply anything. It configures the
                // window's keyboard and selects no mode; a run that has no
                // window is told so rather than having the flag ignored.
                mapping = Some(PathBuf::from(take_value(&mut arguments, "--mapping")?));
            }
            "--frames" => {
                let value = take_value(&mut arguments, "--frames")?;
                frames = Some(value.parse().map_err(|_| CliError::InvalidValue {
                    flag: "--frames",
                    value,
                    expected: "a whole number of frames",
                })?);
            }
            "--warmup" => {
                let value = take_value(&mut arguments, "--warmup")?;
                warmup = Some(value.parse().map_err(|_| CliError::InvalidValue {
                    flag: "--warmup",
                    value,
                    expected: "a whole number of frames",
                })?);
            }
            "--sprites" => {
                let value = take_value(&mut arguments, "--sprites")?;
                sprites = Some(value.parse().map_err(|_| CliError::InvalidValue {
                    flag: "--sprites",
                    value,
                    expected: "a whole number of sprites",
                })?);
            }
            "--uncapped" => uncapped = true,
            "--resize-probe" => resize_probe = true,
            "--drain" => drain = true,
            "--screenshot" => {
                // The path is optional. Anything starting with `-` is the next
                // flag rather than a filename, so `--screenshot --headless`
                // reports the conflict instead of trying to write to a file
                // called `--headless`.
                let takes_next = arguments.peek().is_some_and(|next| !next.starts_with('-'));

                screenshot = Some(if takes_next {
                    PathBuf::from(arguments.next().expect("peeked at it a line ago"))
                } else {
                    PathBuf::from(DEFAULT_SCREENSHOT_PATH)
                });
            }
            other => return Err(CliError::UnknownArgument(other.to_owned())),
        }
    }

    // A measurement is a third thing a run can be, and it shares nothing with
    // the other two. Any of its flags selects it, so `--sprites 100` alone is a
    // measurement of a hundred sprites rather than a silently ignored flag.
    let measuring = frames.is_some() || warmup.is_some() || sprites.is_some() || uncapped || drain;

    // `--mapping` configures a keyboard, and the three commands below either
    // have no window or run unattended. Checked once, here, rather than as
    // three separate conflict pairs: the rule is "only the window has a
    // keyboard", and one check says that where three would only imply it.
    if mapping.is_some() && (measuring || headless.is_some() || screenshot.is_some()) {
        let other = headless
            .or_else(|| screenshot.is_some().then_some("--screenshot"))
            .unwrap_or("--frames");
        return Err(CliError::MappingWithoutWindow { other });
    }

    if measuring {
        if let Some(flag) = headless {
            return Err(CliError::Conflicting {
                first: "--frames",
                second: flag,
                why: "a frame-time measurement needs a window, and that flag asks for no window",
            });
        }
        if screenshot.is_some() {
            return Err(CliError::Conflicting {
                first: "--frames",
                second: "--screenshot",
                why: "one writes a file and exits, the other runs a loop and times it",
            });
        }

        return Ok(Command::Frames(FrameOptions {
            frames: frames.unwrap_or(DEFAULT_FRAMES),
            warmup: warmup.unwrap_or(DEFAULT_WARMUP),
            sprites: sprites.unwrap_or(crate::sim::scene::MEASURED_SPRITES),
            uncapped,
            resize_probe,
            drain,
            scene,
        }));
    }

    // A scene names the world, and the world names the mode: `--scene x.ron`
    // alone is a scene-file run. Saying `--mode scene-file` as well is allowed
    // and is what a recording's header needs, but it also implies `--headless`,
    // which is why the window form does not require it.
    let mode = match (mode, &scene) {
        (None, Some(_)) => Some(Mode::SceneFile),
        (given, _) => given,
    };

    match (headless, screenshot) {
        // Both would be contradictory: one writes a file and stops, the other
        // runs a loop. Guessing which was meant is worse than saying so.
        (Some(flag), Some(_)) => Err(CliError::Conflicting {
            first: flag,
            second: "--screenshot",
            why: "one writes a file and exits, the other runs a loop",
        }),
        (_, Some(path)) => Ok(Command::Screenshot(path)),
        (Some(_), None) => {
            // A recording carries the mode, the seed and the length of the run
            // it was made from. Accepting a second opinion on any of them would
            // mean replaying something other than what was recorded, which is
            // the one thing a replay must never do quietly.
            if replay.is_some() {
                for (flag, given, why) in [
                    (
                        "--record",
                        record.is_some(),
                        "a run either consumes a recording or produces one",
                    ),
                    (
                        "--mode",
                        mode.is_some(),
                        "the recording already says which simulation it is of",
                    ),
                    (
                        "--seed",
                        seed.is_some(),
                        "the recording already carries the seed it was made with",
                    ),
                ] {
                    if given {
                        return Err(CliError::Conflicting {
                            first: "--replay",
                            second: flag,
                            why,
                        });
                    }
                }
            }

            let mode = mode.unwrap_or_default();

            // The two halves of one rule, checked here rather than in the
            // runner: the mode that is constituted from a file needs one, and a
            // mode built from code has nothing to do with one.
            if mode == Mode::SceneFile && scene.is_none() && replay.is_none() {
                return Err(CliError::SceneRequired);
            }
            if mode != Mode::SceneFile && scene.is_some() {
                return Err(CliError::SceneWithoutSceneFile { mode });
            }
            if replay.is_some() && scene.is_some() {
                return Err(CliError::SceneWithReplay);
            }

            Ok(Command::Headless(HeadlessOptions {
                mode,
                seed: seed.unwrap_or(DEFAULT_SEED),
                ticks,
                report: report.map_or(Report::Summary, |(_, report)| report),
                record,
                replay,
                expect,
                scene,
                ipc,
            }))
        }
        (None, None) => Ok(Command::Window(WindowOptions { scene, mapping })),
    }
}

/// Takes the value that follows a flag.
///
/// Anything starting with `-` is the next flag rather than a value, so
/// `--ticks --hash` reports that `--ticks` is missing its value instead of
/// complaining that `--hash` is not a number.
fn take_value<I>(arguments: &mut Peekable<I>, flag: &'static str) -> Result<String, CliError>
where
    I: Iterator<Item = String>,
{
    if !arguments.peek().is_some_and(|next| !next.starts_with('-')) {
        return Err(CliError::MissingValue(flag));
    }

    Ok(arguments.next().expect("peeked at it a line ago"))
}

/// Records which report was asked for, refusing a second, different one.
fn set_report(
    slot: &mut Option<(&'static str, Report)>,
    flag: &'static str,
    report: Report,
) -> Result<(), CliError> {
    match *slot {
        Some((previous, chosen)) if chosen != report => {
            Err(CliError::AmbiguousReport(previous, flag))
        }
        _ => {
            *slot = Some((flag, report));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CliError, Command, DEFAULT_SEED, HeadlessOptions, Report, WindowOptions, parse};
    use crate::sim::Mode;
    use std::path::PathBuf;

    /// Turns string literals into what `parse` expects.
    fn args(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|argument| (*argument).to_owned()).collect()
    }

    /// The headless options with everything left at its default.
    fn options() -> HeadlessOptions {
        HeadlessOptions {
            scene: None,
            mode: Mode::default(),
            seed: DEFAULT_SEED,
            ticks: None,
            report: Report::Summary,
            record: None,
            replay: None,
            expect: None,
            ipc: None,
        }
    }

    /// The headless command with everything but ticks and report at its default.
    fn headless(ticks: Option<u64>, report: Report) -> Command {
        Command::Headless(HeadlessOptions {
            ticks,
            report,
            ..options()
        })
    }

    #[test]
    fn no_arguments_means_the_window() {
        assert_eq!(
            parse(args(&[])),
            Ok(Command::Window(WindowOptions::default()))
        );
    }

    #[test]
    fn a_mapping_is_a_window_flag_and_carries_its_path() {
        assert_eq!(
            parse(args(&["--mapping", "keys.ron"])),
            Ok(Command::Window(WindowOptions {
                scene: None,
                mapping: Some(PathBuf::from("keys.ron")),
            }))
        );

        assert_eq!(
            parse(args(&["--scene", "live.ron", "--mapping", "keys.ron"])),
            Ok(Command::Window(WindowOptions {
                scene: Some(PathBuf::from("live.ron")),
                mapping: Some(PathBuf::from("keys.ron")),
            }))
        );
    }

    #[test]
    fn a_mapping_without_a_window_is_refused_and_names_the_flag_that_took_it() {
        // Three runs have no keyboard: no window at all, one offscreen frame,
        // and an unattended measurement.
        for (arguments, other) in [
            (vec!["--headless", "--mapping", "keys.ron"], "--headless"),
            (vec!["--ticks", "10", "--mapping", "keys.ron"], "--ticks"),
            (
                vec!["--screenshot", "--mapping", "keys.ron"],
                "--screenshot",
            ),
            (vec!["--frames", "10", "--mapping", "keys.ron"], "--frames"),
        ] {
            assert_eq!(
                parse(args(&arguments)),
                Err(CliError::MappingWithoutWindow { other }),
                "{arguments:?}"
            );
        }

        assert_eq!(
            CliError::MappingWithoutWindow {
                other: "--headless"
            }
            .to_string(),
            "`--mapping` binds keys for a window, and `--headless` asks for a run that has no \
             keyboard to bind; drop one of the two"
        );
    }

    #[test]
    fn a_mapping_without_a_value_says_so() {
        assert_eq!(
            parse(args(&["--mapping"])),
            Err(CliError::MissingValue("--mapping"))
        );
    }

    #[test]
    fn a_scene_on_its_own_opens_a_window_on_it() {
        // M4.6. `--scene` used to imply `--headless`, which made the window
        // unable to show a scene at all and made `--frames --scene` a
        // contradiction. It says *what* to run, not *how*.
        assert_eq!(
            parse(args(&["--scene", "live.ron"])),
            Ok(Command::Window(WindowOptions {
                scene: Some(PathBuf::from("live.ron")),
                mapping: None,
            }))
        );
    }

    #[test]
    fn naming_the_scene_file_mode_still_asks_for_a_headless_run() {
        // The other half of the same rule, and the half a recording needs: a
        // mode is a headless concept, so spelling it out keeps the old meaning.
        assert_eq!(
            parse(args(&["--mode", "scene-file", "--scene", "live.ron"])),
            Ok(Command::Headless(HeadlessOptions {
                mode: Mode::SceneFile,
                scene: Some(PathBuf::from("live.ron")),
                ..options()
            }))
        );
    }

    #[test]
    fn a_measured_run_may_name_the_scene_it_measures() {
        // What makes the hot-reload latency of §6/M4 scriptable: `--frames`
        // ends by itself, so a script can start a windowed run over a scene,
        // edit the file mid-run, and read the log afterwards.
        let Ok(Command::Frames(options)) =
            parse(args(&["--frames", "1200", "--scene", "live.ron"]))
        else {
            panic!("a measured run over a scene is a `Frames` command");
        };
        assert_eq!(options.frames, 1_200);
        assert_eq!(options.scene, Some(PathBuf::from("live.ron")));
    }

    #[test]
    fn the_headless_flag_is_recognised() {
        assert_eq!(
            parse(args(&["--headless"])),
            Ok(headless(None, Report::Summary))
        );
    }

    #[test]
    fn ticks_are_read_and_imply_a_headless_run() {
        assert_eq!(
            parse(args(&["--ticks", "10000"])),
            Ok(headless(Some(10_000), Report::Summary))
        );
    }

    #[test]
    fn zero_ticks_is_a_legitimate_run() {
        // The state before any system has run is a state like any other, and
        // being able to dump it is how a divergence gets bisected.
        assert_eq!(
            parse(args(&["--ticks", "0", "--dump"])),
            Ok(headless(Some(0), Report::Dump))
        );
    }

    #[test]
    fn hash_and_dump_each_imply_a_headless_run() {
        assert_eq!(parse(args(&["--hash"])), Ok(headless(None, Report::Hash)));
        assert_eq!(parse(args(&["--dump"])), Ok(headless(None, Report::Dump)));
    }

    #[test]
    fn the_cross_platform_invocation_parses() {
        assert_eq!(
            parse(args(&["--headless", "--ticks", "10000", "--hash"])),
            Ok(headless(Some(10_000), Report::Hash))
        );
    }

    #[test]
    fn repeating_the_same_report_is_not_a_conflict() {
        assert_eq!(
            parse(args(&["--hash", "--hash"])),
            Ok(headless(None, Report::Hash))
        );
    }

    #[test]
    fn asking_for_two_different_reports_is_rejected_rather_than_guessed() {
        assert_eq!(
            parse(args(&["--hash", "--dump"])),
            Err(CliError::AmbiguousReport("--hash", "--dump"))
        );
    }

    #[test]
    fn ticks_without_a_number_says_so() {
        assert_eq!(
            parse(args(&["--ticks"])),
            Err(CliError::MissingValue("--ticks"))
        );
        assert_eq!(
            parse(args(&["--ticks", "--hash"])),
            Err(CliError::MissingValue("--ticks"))
        );
    }

    #[test]
    fn a_tick_count_that_is_not_a_number_names_what_was_given() {
        let error = parse(args(&["--ticks", "many"])).expect_err("`many` is not a number");

        assert_eq!(
            error,
            CliError::InvalidValue {
                flag: "--ticks",
                value: "many".to_owned(),
                expected: "a whole number of ticks",
            }
        );
        assert!(error.to_string().contains("many"));
    }

    #[test]
    fn the_mode_is_read_and_implies_a_headless_run() {
        assert_eq!(
            parse(args(&["--mode", "chance"])),
            Ok(Command::Headless(HeadlessOptions {
                mode: Mode::Chance,
                ..options()
            }))
        );
    }

    #[test]
    fn the_seed_is_read_and_implies_a_headless_run() {
        assert_eq!(
            parse(args(&["--seed", "12345"])),
            Ok(Command::Headless(HeadlessOptions {
                seed: 12_345,
                ..options()
            }))
        );
    }

    #[test]
    fn the_cross_platform_chance_invocation_parses() {
        // The exact line the cross-platform comparison is run with, so a typo in
        // it is a failing test rather than two runs of two different things.
        assert_eq!(
            parse(args(&[
                "--mode", "chance", "--seed", "1", "--ticks", "10000", "--hash"
            ])),
            Ok(Command::Headless(HeadlessOptions {
                mode: Mode::Chance,
                seed: 1,
                ticks: Some(10_000),
                report: Report::Hash,
                ..options()
            }))
        );
    }

    #[test]
    fn an_unknown_mode_is_rejected_and_the_message_lists_the_ones_that_exist() {
        let error = parse(args(&["--mode", "chaos"])).expect_err("`chaos` is not a mode");

        assert_eq!(
            error,
            CliError::InvalidValue {
                flag: "--mode",
                value: "chaos".to_owned(),
                expected: super::MODE_NAMES,
            }
        );

        let message = error.to_string();
        assert!(
            message.contains("chaos"),
            "the message must name what was given: {message}"
        );
        for mode in Mode::ALL {
            assert!(
                message.contains(mode.name()),
                "the message has to offer `{}`: {message}",
                mode.name()
            );
        }
    }

    #[test]
    fn a_seed_that_is_not_a_number_names_the_flag_rather_than_ticks() {
        // The message used to be hard-coded to ticks, which sent the reader of a
        // `--seed` or `--mode` error looking in the wrong place.
        let error = parse(args(&["--seed", "soon"])).expect_err("`soon` is not a number");
        let message = error.to_string();

        assert!(message.contains("--seed"), "{message}");
        assert!(!message.contains("ticks"), "{message}");
    }

    #[test]
    fn a_mode_or_seed_without_a_value_says_so() {
        assert_eq!(
            parse(args(&["--mode"])),
            Err(CliError::MissingValue("--mode"))
        );
        assert_eq!(
            parse(args(&["--mode", "--hash"])),
            Err(CliError::MissingValue("--mode"))
        );
        assert_eq!(
            parse(args(&["--seed"])),
            Err(CliError::MissingValue("--seed"))
        );
    }

    #[test]
    fn the_defaults_are_the_frozen_simulation() {
        // `narvo --ticks 10000 --hash` has to keep meaning what it meant when
        // the M2.2 hash was recorded: the motion mode, unaffected by any seed.
        let Ok(Command::Headless(options)) = parse(args(&["--hash"])) else {
            panic!("--hash has to imply a headless run");
        };

        assert_eq!(options.mode, Mode::Motion);
    }

    #[test]
    fn the_screenshot_flag_takes_the_next_argument_as_its_path() {
        assert_eq!(
            parse(args(&["--screenshot", "out/frame.png"])),
            Ok(Command::Screenshot(PathBuf::from("out/frame.png")))
        );
    }

    #[test]
    fn a_screenshot_flag_without_a_path_falls_back_to_the_default_under_target() {
        assert_eq!(
            parse(args(&["--screenshot"])),
            Ok(Command::Screenshot(PathBuf::from(
                super::DEFAULT_SCREENSHOT_PATH
            )))
        );
    }

    #[test]
    fn the_default_screenshot_path_stays_under_target() {
        // The point of the default is that a screenshot never lands in the
        // working tree. If this ever moves out from under `target/`, it would
        // start leaving untracked files behind again.
        assert!(
            super::DEFAULT_SCREENSHOT_PATH.starts_with("target/"),
            "the default must stay inside the ignored build directory, got {}",
            super::DEFAULT_SCREENSHOT_PATH
        );
    }

    /// The two flags a conflict is between, whatever the reason given.
    fn conflict(raw: &[&str]) -> (&'static str, &'static str) {
        match parse(args(raw)).expect_err("these flags conflict") {
            CliError::Conflicting { first, second, why } => {
                assert!(!why.is_empty(), "a conflict has to say why");
                (first, second)
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn a_flag_after_screenshot_is_not_mistaken_for_a_path() {
        assert_eq!(
            conflict(&["--screenshot", "--headless"]),
            ("--headless", "--screenshot")
        );
    }

    #[test]
    fn headless_and_screenshot_together_are_rejected_rather_than_guessed() {
        assert_eq!(
            conflict(&["--headless", "--screenshot", "frame.png"]),
            ("--headless", "--screenshot")
        );
    }

    #[test]
    fn the_conflict_names_the_flag_that_was_actually_typed() {
        assert_eq!(
            conflict(&["--hash", "--screenshot", "frame.png"]),
            ("--hash", "--screenshot")
        );
    }

    #[test]
    fn recording_and_replaying_take_paths_and_imply_a_headless_run() {
        assert_eq!(
            parse(args(&["--record", "bug.rec"])),
            Ok(Command::Headless(HeadlessOptions {
                record: Some(PathBuf::from("bug.rec")),
                ..options()
            }))
        );
        assert_eq!(
            parse(args(&["--replay", "bug.rec"])),
            Ok(Command::Headless(HeadlessOptions {
                replay: Some(PathBuf::from("bug.rec")),
                ..options()
            }))
        );
    }

    #[test]
    fn a_replay_may_be_shortened_to_bisect_a_divergence() {
        // The one thing a replay does take alongside the file, and the reason it
        // does: finding where two runs first differ means running the same
        // recording to a series of shorter lengths.
        assert_eq!(
            parse(args(&["--replay", "bug.rec", "--ticks", "500"])),
            Ok(Command::Headless(HeadlessOptions {
                ticks: Some(500),
                replay: Some(PathBuf::from("bug.rec")),
                ..options()
            }))
        );
    }

    #[test]
    fn a_replay_refuses_a_second_opinion_about_what_it_is_replaying() {
        // Mode and seed are in the recording. Accepting them again would let a
        // command line say one thing and a file another, and a replay that
        // quietly follows the wrong one is the failure this whole feature is
        // meant to rule out.
        assert_eq!(
            conflict(&["--replay", "bug.rec", "--mode", "input"]),
            ("--replay", "--mode")
        );
        assert_eq!(
            conflict(&["--replay", "bug.rec", "--seed", "3"]),
            ("--replay", "--seed")
        );
        assert_eq!(
            conflict(&["--replay", "bug.rec", "--record", "again.rec"]),
            ("--replay", "--record")
        );
    }

    #[test]
    fn the_order_of_replay_and_the_flag_it_conflicts_with_does_not_matter() {
        assert_eq!(
            conflict(&["--mode", "input", "--replay", "bug.rec"]),
            ("--replay", "--mode")
        );
    }

    #[test]
    fn recording_and_replaying_without_a_path_say_so() {
        assert_eq!(
            parse(args(&["--record"])),
            Err(CliError::MissingValue("--record"))
        );
        assert_eq!(
            parse(args(&["--replay", "--hash"])),
            Err(CliError::MissingValue("--replay"))
        );
    }

    #[test]
    fn an_expected_state_takes_a_path_and_implies_a_headless_run() {
        assert_eq!(
            parse(args(&["--expect", "expected.dump"])),
            Ok(Command::Headless(HeadlessOptions {
                expect: Some(PathBuf::from("expected.dump")),
                ..options()
            }))
        );
    }

    #[test]
    fn a_repro_is_a_replay_with_an_expected_state_and_a_tick_count() {
        // The line M6.7a exists to make runnable, parsed as one thing: the
        // recording says what to feed, the tick count says where to stop, and the
        // expected state says what the answer has to be.
        assert_eq!(
            parse(args(&[
                "--replay",
                "bug.rec",
                "--ticks",
                "500",
                "--expect",
                "expected.dump"
            ])),
            Ok(Command::Headless(HeadlessOptions {
                ticks: Some(500),
                replay: Some(PathBuf::from("bug.rec")),
                expect: Some(PathBuf::from("expected.dump")),
                ..options()
            }))
        );
    }

    #[test]
    fn an_expected_state_and_a_report_are_independent_of_each_other() {
        // `--dump` decides what reaches stdout, `--expect` decides what the exit
        // code means. Asking for both is a run that prints its state *and* is
        // judged on it, which is how a repro is bisected without a second run.
        assert_eq!(
            parse(args(&["--expect", "expected.dump", "--dump"])),
            Ok(Command::Headless(HeadlessOptions {
                report: Report::Dump,
                expect: Some(PathBuf::from("expected.dump")),
                ..options()
            }))
        );
    }

    #[test]
    fn an_expected_state_without_a_path_says_so() {
        assert_eq!(
            parse(args(&["--expect"])),
            Err(CliError::MissingValue("--expect"))
        );
        assert_eq!(
            parse(args(&["--expect", "--hash"])),
            Err(CliError::MissingValue("--expect"))
        );
    }

    #[test]
    fn an_expected_state_is_refused_for_the_runs_that_have_no_final_state() {
        // A screenshot writes one frame and a measurement times a window; neither
        // ends with a simulation state anybody could expect anything of.
        assert_eq!(
            conflict(&["--expect", "expected.dump", "--screenshot", "frame.png"]),
            ("--expect", "--screenshot")
        );
        assert_eq!(
            conflict(&["--expect", "expected.dump", "--frames", "10"]),
            ("--frames", "--expect")
        );
        assert_eq!(
            parse(args(&[
                "--expect",
                "expected.dump",
                "--mapping",
                "keys.ron"
            ])),
            Err(CliError::MappingWithoutWindow { other: "--expect" })
        );
    }

    #[test]
    fn recording_alongside_a_screenshot_is_still_a_conflict() {
        assert_eq!(
            conflict(&["--record", "bug.rec", "--screenshot", "frame.png"]),
            ("--record", "--screenshot")
        );
    }

    #[test]
    fn the_usage_text_mentions_every_mode_and_every_flag_that_exists() {
        // The usage text is the only documentation a caller gets, and it is a
        // string constant nothing else would notice going stale.
        for mode in Mode::ALL {
            assert!(
                super::USAGE.contains(mode.name()),
                "the usage text does not mention mode `{}`",
                mode.name()
            );
            assert!(
                super::MODE_NAMES.contains(mode.name()),
                "the --mode error message does not offer `{}`",
                mode.name()
            );
        }

        for flag in [
            "--headless",
            "--mode",
            "--seed",
            "--ticks",
            "--record",
            "--replay",
            "--expect",
            "--hash",
            "--dump",
            "--screenshot",
            "--resize-probe",
            "--help",
        ] {
            assert!(
                super::USAGE.contains(flag),
                "the usage text does not mention {flag}"
            );
        }
    }

    #[test]
    fn an_unknown_argument_is_named_in_the_error() {
        let error = parse(args(&["--wat"])).expect_err("an unknown flag must be rejected");

        assert_eq!(error, CliError::UnknownArgument("--wat".to_owned()));
        assert!(
            error.to_string().contains("--wat"),
            "the message must name the argument: {error}"
        );
    }

    /// The resize probe is off unless it is asked for, and asking works.
    ///
    /// M7.1c's instrument, and the reason it has a test of its own: a flag that
    /// parses into nothing is exactly the shape M7.1a found — the run reports
    /// what it always reported, and the report reads like an answer. Removing
    /// the parser arm turns the second half red, and the first half is what says
    /// an ordinary `--frames` run is not silently resizing itself.
    #[test]
    fn the_resize_probe_is_off_unless_it_is_asked_for() {
        let Ok(Command::Frames(plain)) = parse(args(&["--frames", "400"])) else {
            panic!("a measured run is a `Frames` command");
        };
        assert!(
            !plain.resize_probe,
            "an ordinary measured run must not move its own window"
        );

        let Ok(Command::Frames(probed)) = parse(args(&["--frames", "400", "--resize-probe"]))
        else {
            panic!("a measured run is a `Frames` command");
        };
        assert!(
            probed.resize_probe,
            "--resize-probe parsed into nothing, so the run measures a still window \
             while reporting the flag was given"
        );
        assert_eq!(probed.frames, 400, "the probe changed an unrelated field");
    }

    #[test]
    fn both_spellings_of_help_work() {
        assert_eq!(parse(args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse(args(&["-h"])), Ok(Command::Help));
    }
}
