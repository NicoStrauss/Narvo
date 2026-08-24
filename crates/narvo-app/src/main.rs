//! The Narvo runner: the executable that composes the engine crates.
//!
//! Responsibility: reading configuration from the command line, building the
//! engine in either windowed mode (a real window and swapchain) or headless mode
//! (no window, no surface, driven by synthetic frame durations so a run is
//! scriptable and reproducible), *driving* the main loop, and shutting
//! everything down in order.
//!
//! **Driving it rather than owning it, since M3.32.** The order a frame's work
//! runs in — every due tick, then one extraction, then at most one drawing —
//! belongs to [`narvo_core::frame::FrameLoop`], where it is covered without a
//! GPU. What is here is what only a composition root can supply: the clock that
//! reads the operating system ([`frame::Monotonic`]), what each phase actually
//! does ([`frame::SceneHost`]), and the window it runs in. The headless runner
//! still has a loop of its own, deliberately unmerged. What a test beside it
//! checks is narrower than that loop: one sequence of durations goes to a bare
//! `FixedTimestep` and the same sequence to a `FrameLoop` through a staged
//! clock, and the tick counts have to agree. It never calls `run`, so what is
//! pinned is the accumulator under both loops rather than `run`'s own.
//!
//! API boundary: this is the only crate that may depend on all the others and
//! the only place where a concrete engine configuration is assembled, so the
//! library crates never have to know which mode they are running in. It exposes
//! no library API — nothing links against it — which keeps composition-root
//! decisions out of `narvo-core`, `narvo-render2d` and `narvo-assets`.
//!
//! Everything graphical sits behind the `render` feature. Built without it, this
//! binary still runs the headless path and has neither wgpu nor winit anywhere
//! in its dependency tree, which is what the headless rule in CLAUDE.md asks for
//! and what the CI headless job verifies.

use std::io::{self, Write as _};
use std::path::Path;
use std::process::ExitCode;

use crate::recording::Recording;

/// The click demo's cue mapping.
///
/// **Ungated, and that is the decision rather than an omission.** Its only
/// audible consumer is the window, and the obvious move would have been to gate
/// it on `render` like `window` and `frame`. That would have put it outside
/// steps seven to nine of the verification set — the headless configuration —
/// which is precisely the M4.8 shape this file's other gates were written to
/// avoid. The module names no optional crate: `narvo-audio` arrives without its
/// `device` feature, so a headless build carries the cue types and the mapping
/// and no backend at all.
mod audio;
mod cli;
mod headless;

/// Where a request from the agent protocol meets a running world.
///
/// Ungated, for the reason [`audio`] and [`physics`] above and below it are:
/// answering a read is ordinary logic over a `&World` and needs no device, so the
/// three headless steps of the verification set compile and test it like any
/// other module. Putting it behind `render` would place the seam outside exactly
/// the checks that would catch a mistake in it — the M4.8 shape CLAUDE.md
/// records, and the shape M5.6c found again when the audio seam needed a hook in
/// the headless runner as well as in `SceneHost`.
///
/// Its production caller is [`headless::run`], which drains the inbox once per
/// tick. The window still has none, and **since M6.6a the reason is a different
/// one**. It used to be shape: `frame::SceneHost` held no `ComponentRegistry`,
/// so most of the commands could not be answered there at all. That obstacle is
/// gone — the host holds the simulation the scene was read against, registry
/// included — and the seam did not follow it, because the plan's own
/// re-examination (v1.04) found the assumption behind it refuted: an agent has
/// worked fully headless since M6.5b and M6.7b, so pulling the seam into the
/// window would add nothing to it and would cost a second answering moment.
/// ADR-0031 wrote down what the first second one cost. The registry is a
/// precondition for two human-side capabilities instead — an entity inspector
/// and a screenshot — and neither is this seam.
///
/// Since M6.4a two of its five commands read a file, through [`load_recording`],
/// [`scene_for`] and `scene_anchor::Anchor::read` — the same three this file
/// calls for `--replay` and `--scene`, so a path given over the socket and a path
/// given on the command line are read once, the same way, with the same refusals.
mod ipc;

/// What the debug overlay says about one entity, as text.
///
/// **Gated on `render` *or* `test`, and that is [`watch`]'s pattern rather than
/// a new one.** Its only production caller is `frame::SceneHost`, which is
/// render-gated, so in a headless *build* every item here would be dead — the
/// four warnings the first compile of this module produced. Gating removes the
/// code, which is what "not used here" actually means; an `allow(dead_code)`
/// would make it look used and silence a genuinely dead item too.
///
/// The `test` half is what keeps the gate from costing coverage, and it matters
/// more here than for [`watch`]: **this module is the half of an inspector that
/// can be wrong.** Turning a world and a registry into lines needs no device, so
/// its tests run in all three configurations of the verification set — including
/// the headless ones, which is the M4.8 shape CLAUDE.md records.
///
/// It lays out through the text path ADR-0038 made production-ready and reaches
/// the renderer as the second batch of ADR-0039.
#[cfg(any(feature = "render", test))]
mod inspector;

/// The seam between `RigidBody` components and the physics facade.
///
/// Ungated for the same reason `audio` above is: physics needs no device, so the
/// mapping compiles and is tested by the three headless steps of the
/// verification set. Putting it behind `render` would place it outside exactly
/// the checks that would catch a mistake in it.
///
/// Its first caller in the binary is [`sim::physics`], the mode M5b.3b added.
///
/// Until then the module carried `expect(dead_code)` rather than `allow`,
/// precisely so that the day a runner called it the attribute would become an
/// unfulfilled lint and the workspace's deny would force its removal. That is
/// what happened: the first build after the mode was wired up reported
/// `this lint expectation is unfulfilled` and named the line. The mechanism is
/// recorded here because it worked, not because it needed explaining.
mod physics;
mod recording;

/// The repro runner's judgement: a run's state against the state expected of it.
///
/// Ungated for the reason [`ipc`], [`audio`] and [`physics`] are — comparing two
/// canonical dumps needs no device, so steps two, eight and ten of the
/// verification set compile and test it in every configuration. It performs no
/// I/O at all; [`read_expected`] below is the reading half, and keeping them
/// apart is what lets the judgement be unit-tested without a file and what makes
/// the ordering below checkable by reading rather than by trust.
mod repro;

mod scene_anchor;
mod sim;

/// The scene-file watcher.
///
/// **Gated on `render` *or* `test`, and both halves are deliberate.**
///
/// Its only production caller is `window.rs`, which is render-gated, so in a
/// headless build every item in it is dead — five warnings' worth, carried since
/// M4.6. The obvious silencer is an `allow(dead_code)`, and it is the wrong one:
/// it makes the module *look* used and would keep any genuinely dead item quiet
/// too. Gating removes the code instead, which is what "not used here" actually
/// means.
///
/// The `test` half is what keeps the gate from costing coverage. All eight of
/// M4.6's tests are pure filesystem and `std` logic — poll intervals, settling,
/// content digests, a missing file — and none of them touches a GPU or
/// `narvo-render2d`. Dropping them from the headless run would trade five
/// warnings for eight lost tests, which is a bad trade nobody would make
/// deliberately; `#[cfg(test)]` means the headless *test* build still compiles
/// and runs them while the headless *binary* carries nothing.
#[cfg(any(feature = "render", test))]
mod watch;

/// Window input: the winit translation table, and the rule by which a mapped
/// event reaches a tick.
///
/// Gated exactly like [`watch`] and for exactly its reason. The production
/// caller is `window.rs`, so a headless *binary* would carry the module dead;
/// but the delivery rule is winit-free by construction — that cut is the design's
/// acceptance criterion — and its tests drive synthetic device events against a
/// world with no GPU anywhere in sight. Dropping them from the headless run
/// would lose the coverage that matters most here.
///
/// Inside the module the split goes one level further: `translate`, the table
/// and `control_of` are `render`-gated because they name winit types, so the
/// headless test build compiles the delivery rule and its tests and nothing
/// else. The table's own tests therefore run in CI's two `checks` jobs, which
/// build with default features, and not in the `--no-default-features` steps.
#[cfg(any(feature = "render", test))]
mod input;

/// The agent transport: a localhost TCP listener behind the `ipc` feature.
///
/// **Gated exactly like [`watch`] and [`input`], and for their reason plus one of
/// its own.** Theirs: a production caller that only some configurations have, and
/// `allow(dead_code)` would make the module *look* used while a gate removes it,
/// which is what "not used here" actually means. Its own: D20 decided localhost
/// TCP **on the condition that it is off by default**, because the survey
/// measured that a loopback listener has no access check on either platform. A
/// release binary contains none of this.
///
/// The `test` half is what keeps the gate from costing coverage, and it is worth
/// more here than for `watch`: the module is pure `std::net` and `narvo-ipc`, so
/// its tests run in **both** configurations the verification set already covers —
/// including `--no-default-features`, which is where an agent workflow lives.
#[cfg(any(feature = "ipc", test))]
mod transport;

#[cfg(feature = "render")]
mod assets;

/// The golden tests for the two scenes this crate blesses, and nothing else.
///
/// **A module with no production items**, which is unusual enough to say why.
/// M6b.1 moved the extraction and the hit test out to `narvo-view2d`
/// (ADR-0041); what could not go with them are the tests that compare a rendered
/// world against a PNG under `tests/golden/`, because a golden test belongs
/// beside its reference file. One of them additionally reaches
/// `sim::scene_file::count_actions`, which is `pub(crate)` and stays so, which is
/// why these are unit tests inside `src/` rather than an integration test.
///
/// Gated on `render` like the module they came from, and for its reason: every
/// test in here needs an `OffscreenTarget`.
#[cfg(feature = "render")]
mod blessed_scenes;

#[cfg(feature = "render")]
mod content;
#[cfg(feature = "render")]
mod frame;
#[cfg(feature = "render")]
mod screenshot;
#[cfg(feature = "render")]
mod window;

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("narvo: {error}\n");
            eprintln!("{}", cli::USAGE);
            return ExitCode::FAILURE;
        }
    };

    match command {
        cli::Command::Help => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        cli::Command::Headless(options) => run_headless(options),
        cli::Command::Screenshot(path) => run_screenshot(&path),
        cli::Command::Window(options) => run_window(&options),
        cli::Command::Frames(options) => run_frames(options),
    }
}

/// Runs the simulation and writes the requested report.
///
/// Exactly one thing reaches stdout — the report — and everything else goes to
/// stderr. That separation is what makes `narvo --hash > run.txt` on two
/// machines produce two files a `diff` can compare: no progress line, no
/// summary, nothing but the payload.
fn run_headless(options: cli::HeadlessOptions) -> ExitCode {
    let plan = match &options.replay {
        Some(path) => match load_recording(path) {
            Ok(recording) => {
                let scene = match scene_for(&recording) {
                    Ok(scene) => scene,
                    Err(message) => {
                        eprintln!("narvo: {message}");
                        return ExitCode::FAILURE;
                    }
                };

                headless::Plan::Replay {
                    recording,
                    scene,
                    ticks: options.ticks,
                }
            }
            Err(message) => {
                eprintln!("narvo: {message}");
                return ExitCode::FAILURE;
            }
        },
        None => {
            // A live scene-file run establishes the anchor instead of checking
            // one, from the same single read the world is built from.
            let scene = match &options.scene {
                None => None,
                Some(path) => match scene_anchor::Anchor::read(path) {
                    Ok((anchor, text)) => Some(headless::LoadedScene { anchor, text }),
                    Err(error) => {
                        eprintln!("narvo: {error}");
                        return ExitCode::FAILURE;
                    }
                },
            };

            headless::Plan::Live {
                mode: options.mode,
                seed: options.seed,
                ticks: options.ticks.unwrap_or(cli::DEFAULT_TICKS),
                scene,
            }
        }
    };

    // **Read before the simulation starts, and that is a property rather than a
    // convenience.** An oracle a runner could derive from the run it judges would
    // make every repro pass, which is the one construction that turns a
    // verification tool into decoration. Here the expected state is on disk
    // before a single tick has run and nothing downstream writes it, so the
    // exclusion is in the shape of the code and not in a comment about care.
    let expected = match &options.expect {
        None => None,
        Some(path) => match read_expected(path) {
            Ok(text) => Some(text),
            Err(message) => {
                eprintln!("narvo: {message}");
                return ExitCode::FAILURE;
            }
        },
    };

    let run = match drive(plan, options.ipc.as_deref()) {
        Ok(run) => run,
        Err(message) => {
            eprintln!("narvo: {message}");
            return ExitCode::FAILURE;
        }
    };

    let ticks = run.ticks;
    let verdict = expected
        .as_deref()
        .map(|expected| repro::judge(expected, &run.dump));

    if let Some(path) = &options.record
        && let Err(message) = write_recording(path, &run.recording)
    {
        eprintln!("narvo: {message}");
        return ExitCode::FAILURE;
    }

    // The mode and the seed are on this line because they decide the result: a
    // hash pasted into a report without them says nothing, and this is the line
    // that sits next to it in a terminal. They are read back off the run rather
    // than off the command line, because a replay takes both from its file.
    eprintln!(
        "narvo: mode {}, seed {}, {} ticks over {} entities, {} recorded inputs, reporting {}",
        run.recording.mode(),
        run.recording.seed(),
        run.ticks,
        run.entities,
        run.recording.inputs().len(),
        match options.report {
            cli::Report::Summary => "a summary",
            cli::Report::Hash => "the state hash",
            cli::Report::Dump => "the canonical dump",
        }
    );

    let report = match options.report {
        cli::Report::Summary => format!(
            "headless: {} ticks over {} entities\n",
            run.ticks, run.entities
        ),
        // Sixteen lowercase hex digits, zero padded, so two hashes are the same
        // width and a diff points at the character that differs.
        cli::Report::Hash => format!("{:016x}\n", narvo_ecs::state_hash(&run.dump)),
        cli::Report::Dump => run.dump,
    };

    let written = write_report(&report);

    // The verdict is the last thing said and the only thing that moves the exit
    // code. It goes to stderr because stdout carries the report and nothing else
    // — the property that makes `narvo --hash > run.txt` comparable across
    // machines, and one a repro run has no business being the exception to.
    let (Some(verdict), Some(path)) = (verdict, options.expect.as_deref()) else {
        return written;
    };

    eprintln!(
        "narvo: {}",
        repro::Judgement {
            verdict: &verdict,
            expected: path,
            ticks,
        }
    );

    if verdict.reproduced() {
        written
    } else {
        ExitCode::FAILURE
    }
}

/// Reads the state a run is expected to produce, naming the file in what fails.
///
/// A sibling of [`load_recording`] and written to its shape: the file is read
/// here, in the one module that touches the disk, and the text goes to
/// [`repro::judge`], which reads nothing. That split is `headless::Plan`'s
/// arrangement for the scene file and it buys the same thing — the judgement can
/// be tested without a filesystem, and the fact that the expected state is an
/// *input* is visible in the signature rather than promised in prose.
///
/// An unreadable file is an error and never a divergence. The two would
/// otherwise look the same from a script's exit code, and "the repro is broken"
/// is a different finding from "the state moved".
fn read_expected(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| {
        format!(
            "could not read the expected state {}: {error}. It is written by a run - \
             `narvo ... --dump > {}` - and is never committed to this repository (ADR-0035)",
            path.display(),
            path.display()
        )
    })
}

/// Runs `plan`, serving an agent on `ipc` when one was asked for.
///
/// The two halves of the `ipc` gate meet here and nowhere else: with the feature
/// on, an address binds a listener and the run is driven through it; without it,
/// an address is refused by name. That is `run_screenshot`'s and `run_frames`'
/// arrangement with `render`, and it is what makes a build that cannot serve an
/// agent say so instead of reporting an unknown flag.
#[cfg(feature = "ipc")]
fn drive(plan: headless::Plan, ipc: Option<&str>) -> Result<headless::Run, String> {
    let Some(address) = ipc else {
        return headless::run(plan).map_err(|error| error.to_string());
    };

    let mut endpoint = transport::Endpoint::bind(address)
        .map_err(|error| format!("could not listen for an agent on {address}: {error}"))?;

    // To stderr, because stdout carries the report and nothing else — the
    // property that makes `narvo --hash > run.txt` comparable across machines.
    // The bound address rather than the requested one, so that asking for port 0
    // tells the caller which port it actually got.
    match endpoint.local_addr() {
        Ok(bound) => eprintln!("narvo: listening for an agent on {bound}"),
        Err(error) => eprintln!("narvo: listening for an agent on {address} ({error})"),
    }

    headless::run_with(plan, &mut ipc::Inbox::new(), &mut endpoint)
        .map_err(|error| error.to_string())
}

/// The same, in a build that has no transport.
///
/// The refusal names the feature rather than the flag, because the flag is not
/// the mistake: the parser understood it, and this build simply has no listener
/// to offer. D20 put the transport behind a gate that is off by default because
/// a loopback listener has no access check, which is the sentence a user asking
/// for one deserves to see.
#[cfg(not(feature = "ipc"))]
fn drive(plan: headless::Plan, ipc: Option<&str>) -> Result<headless::Run, String> {
    if let Some(address) = ipc {
        return Err(format!(
            "this build has no agent transport, so it cannot listen on {address}; \
             build with --features ipc to get one. It is off by default because a \
             localhost listener has no access check — any process on the machine \
             can connect to it (D20)"
        ));
    }

    headless::run(plan).map_err(|error| error.to_string())
}

/// Reads and checks the scene a recording was made against, if it names one.
///
/// The scene anchor is checked **before a plan exists** and so before a single
/// tick can run (ADR-0019). A replay against a scene that is no longer the
/// recorded one is refused rather than run and compared afterwards, because the
/// state hash cannot tell "the file changed" from "the simulation diverged".
///
/// Its own function since M6.4a, and that is the point: a `replay` request over
/// the agent socket has to make exactly this check, and a second copy of it is
/// how the socket would come to accept a recording the command line refuses.
/// `crate::ipc::start_replay` is the other caller.
///
/// # Errors
///
/// The anchor's own message — which names the file, both digests when they
/// differ, and what to do about it — as a string, because both callers turn it
/// into text and neither branches on it.
fn scene_for(recording: &Recording) -> Result<Option<headless::LoadedScene>, String> {
    let Some(anchor) = recording.scene() else {
        return Ok(None);
    };

    let text = anchor.verify().map_err(|error| error.to_string())?;

    Ok(Some(headless::LoadedScene {
        anchor: anchor.clone(),
        text,
    }))
}

/// Reads a recording, naming the file in whatever goes wrong.
///
/// The parser's own messages already name the line and the problem; this only
/// adds which file they are about, because a bisecting session has several open
/// at once.
fn load_recording(path: &Path) -> Result<Recording, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read the recording {}: {error}", path.display()))?;

    Recording::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Reads an input mapping, naming the file in whatever goes wrong.
///
/// # One read, and why it is spelled out here
///
/// `narvo_input::from_file` exists and does the same two steps, and this does
/// not call it: it reads the file itself and hands the text to
/// `narvo_input::from_str`. The reason is the rule ADR-0019 states for the
/// scene anchor — *hash and load come from one read*, so that nothing can change
/// between them — and while a mapping carries no anchor today, the M5.2 survey
/// recorded `from_file`'s own second read as a hazard rather than a defect. Doing
/// the read here keeps this path at one, and keeps the I/O message in the same
/// shape as every other one this file produces.
///
/// The parse error's own message carries the line and column `ron` computed
/// (M4.2's measure, ADR-0025); this only adds which file it is about.
#[cfg(feature = "render")]
fn load_mapping(path: &Path) -> Result<narvo_input::Mapping, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("could not read the mapping {}: {error}", path.display()))?;

    narvo_input::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Writes a recording, creating the directory above it if need be.
///
/// `render` produces LF line endings and `write` passes bytes through
/// unchanged on every platform, so a recording made on Windows is byte for byte
/// one made on Linux — which is what lets one be replayed on the other.
fn write_recording(path: &Path, recording: &Recording) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "could not create the directory for {}: {error}",
                path.display()
            )
        })?;
    }

    std::fs::write(path, recording.render())
        .map_err(|error| format!("could not write the recording {}: {error}", path.display()))
}

/// Writes the report to stdout as bytes.
///
/// `write_all` rather than `print!` so nothing between here and the file can
/// reinterpret the text. The report is built with `\n` throughout and Rust does
/// not translate line endings on any platform, so the bytes that leave here are
/// the bytes a redirect stores — on Windows as much as on Linux.
fn write_report(report: &str) -> ExitCode {
    let mut stdout = io::stdout().lock();

    match stdout
        .write_all(report.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("narvo: could not write the report: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "render")]
fn run_screenshot(path: &Path) -> ExitCode {
    match screenshot::capture(path) {
        Ok(()) => {
            println!(
                "wrote {}x{} screenshot to {}",
                screenshot::WIDTH,
                screenshot::HEIGHT,
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("narvo: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Without the `render` feature there is no offscreen path to capture from.
#[cfg(not(feature = "render"))]
fn run_screenshot(path: &Path) -> ExitCode {
    eprintln!(
        "narvo: cannot write {}: this binary was built without the `render` feature, \
         so it has no renderer. Rebuild with default features, or use --headless.",
        path.display()
    );
    ExitCode::FAILURE
}

#[cfg(feature = "render")]
fn run_window(options: &cli::WindowOptions) -> ExitCode {
    let mut plan = match options.scene.as_deref() {
        None => window::RunPlan::demo(sim::scene::MEASURED_SPRITES),
        Some(path) => match window::RunPlan::from_scene(path) {
            Ok(plan) => plan,
            Err(message) => {
                eprintln!("narvo: {message}");
                return ExitCode::FAILURE;
            }
        },
    };

    if let Some(path) = options.mapping.as_deref() {
        match load_mapping(path) {
            Ok(mapping) => plan.mapping = Some(mapping),
            Err(message) => {
                eprintln!("narvo: {message}");
                return ExitCode::FAILURE;
            }
        }
    }

    match window::run(plan) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("narvo: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the frame loop for a fixed number of frames and reports the timings.
///
/// The measurement is hardware-bound, so it is a command rather than a test:
/// `ProjektPlan.md` §6/M3 makes the 50 000-sprite figure a logged measurement on
/// the reference GPU without a gate, and CI has no display to run it on.
#[cfg(feature = "render")]
fn run_frames(options: cli::FrameOptions) -> ExitCode {
    let plan = window::RunPlan {
        scene: options.scene.clone(),
        sprites: options.sprites,
        frames: Some(options.frames),
        warmup: options.warmup,
        uncapped: options.uncapped,
        resize_probe: options.resize_probe,
        drain: options.drain,
        // A measurement runs unattended, and the CLI refuses `--mapping`
        // alongside it for that reason.
        mapping: None,
    };

    match window::run(plan) {
        Ok(Some(measured)) => {
            window::report(&measured);
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("narvo: the window was closed before the run finished");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("narvo: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Without the `render` feature there is no window to measure frames in.
#[cfg(not(feature = "render"))]
fn run_frames(_options: cli::FrameOptions) -> ExitCode {
    eprintln!(concat!(
        "narvo: this binary was built without the `render` feature, so it ",
        "cannot open a window and has no frames to time."
    ));
    ExitCode::FAILURE
}

/// Without the `render` feature there is no window to open.
///
/// It takes the options and ignores them for the same reason the render half
/// takes them: the *parser* is feature-free by design (see `cli`'s module
/// documentation), so both halves of this function have to accept everything a
/// command line can name — including `--mapping`, which a build with no window
/// has nowhere to apply.
#[cfg(not(feature = "render"))]
fn run_window(_options: &cli::WindowOptions) -> ExitCode {
    eprintln!(
        "narvo: this binary was built without the `render` feature, so it cannot open a \
         window. Run it with --headless, or rebuild with default features."
    );
    eprintln!("\n{}", cli::USAGE);
    ExitCode::FAILURE
}
