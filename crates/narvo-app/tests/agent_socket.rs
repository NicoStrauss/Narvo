//! An agent drives a real `narvo` process over a real socket.
//!
//! **`#![cfg(feature = "ipc")]` is the whole reason this file compiles at all**,
//! and it is M4.8's lesson written down: that task added an integration test
//! importing a gated crate without the matching attribute, the local run was
//! green, and CI failed on both platforms. Without the feature there is no
//! transport in the binary this drives, so there is nothing here to run.
//!
//! # Why none of this is a race
//!
//! `--ticks 0 --ipc` reaches its budget before anything has happened and then
//! **waits for its first client**. So the client connects into a stopped run,
//! and every command lands where the test put it. The halt report that re-cut
//! M6.3c measured the alternative: a headless run is over in single-digit
//! milliseconds, and a test that connects to one is betting on the scheduler.
//!
//! What this file therefore proves is **delivery** — bytes become requests,
//! answers come back to the asker, a run resumes and ends. The *mechanism* of a
//! cut at an arbitrary tick belongs to the deterministic tests in
//! `headless::tests`, and that division is the one M6.3c's halt report drew.
//!
//! # These tests have flaked, twice, and were landed anyway (D21)
//!
//! **Read this before rerunning anything here.**
//!
//! Two failures have ever been seen, both in M6.3d's second session, both on a
//! freshly compiled test target, and **both consumed the whole 20-second brake**.
//! The run was alive and blocked; its client had already received every answer it
//! asked for. Nothing was ever diagnosed.
//!
//! What was measured afterwards, on the same bytes:
//!
//! | condition | platform | runs | failures |
//! |---|---|---|---|
//! | steady state | Windows | 800 | 0 |
//! | steady state | WSL | 200 | 0 |
//! | test target recompiled in each invocation | Windows | 500 | 0 |
//!
//! **1500 runs, 0 failures.** By the rule of three that is a rate below **0.60 %**
//! under the recompile condition and below **0.30 %** overall — an upper bound,
//! never an all-clear. No run in those 1500 took longer than **1.01 s** against a
//! 20-second brake, so the failure is bimodal: when it happens it blocks
//! completely, which is why the brake is not merely a number that is too small.
//!
//! Eight candidate causes were refuted by measurement rather than argument — a
//! stranded buffer line, a lost request, a lost answer, stderr backpressure, port
//! reuse, a connect race, the test harness itself, and **an `narvo` process
//! orphaned by a panicking test**, which was the seventh session's own
//! hypothesis and did not survive contact: an orphan does not outlive its parent
//! here by more than about a tenth of a second. The full account is in **seven**
//! reports under `target/reports/`: `M6.3d.md`, `M6.3d-fortsetzung.md`,
//! `M6.3d-diagnose.md`, `M6.3d-rate.md`, `M6.3d-last.md`, `M6.3d-abschluss.md`
//! and `M6.3d-landung.md`. They are not summarised here on purpose — a later
//! reader should not have to rediscover which candidates are already dead.
//!
//! **What the seventh session did find is a defect in the instrument, not in the
//! transport**, and it is fixed: the two unit tests in `transport.rs` that read
//! an answer off a real socket had no read deadline at all, so a cancelled
//! answer blocked them for ever while the tests in *this* file failed properly
//! at their bound. Every "the suite stopped talking" observation in M6.3d is
//! consistent with that, and red edge (a2) — which produced ten minutes of
//! silence before the fix — now produces five red tests in twenty-four seconds.
//! It does **not** explain the two original failures, which had no injection.
//!
//! **M6.4a found a third failure and it is a different animal**, named here so
//! that a later reader does not file it under D21. Two tests read the run's
//! stderr with `try_iter`, which is non-blocking, and a dead child does not mean
//! this process's own reader thread has drained the pipe yet: under load on a
//! freshly compiled target, **three of eight WSL runs** went red with an empty
//! vector, in 6 to 30 milliseconds, printing `[]`. It is diagnosed, its cause is
//! in this file rather than in the transport, and it is fixed — see
//! [`Serving::summary`]. Ten recompiled runs after the fix: zero failures. The
//! D21 flake remains what it was: a *blocked* run that consumes the whole brake.
//!
//! ## If one of these goes red, especially in CI
//!
//! **It is a first-class finding, and the rerun is the wrong move.** Do not rerun
//! and do not raise `PATIENCE`. Capture the run log; it is the artefact D21 named
//! as the reopening trigger.
//!
//! The reason is not discipline for its own sake: a CI runner is a **different
//! machine** from the one on which 1500 runs found nothing, so a failure captured
//! there is precisely the evidence that could not be produced locally. A rerun
//! throws it away. `…/scratchpad/m63d_rate.ps1` is the loop that measured the
//! numbers above and is also how a fix would later be shown to have worked — a
//! fix with no before-number is an opinion.

#![cfg(feature = "ipc")]

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long anything here waits before deciding it is never going to happen.
///
/// **A brake, not a synchronisation, and the difference is the whole point.**
/// Every wait in this file is on something that takes microseconds — a loopback
/// round trip, a process exiting once its client has gone — so this number is not
/// tuned to any of them and must never be. It exists so that a failure is a *red
/// test* instead of a suite that stops talking, which is what the first version
/// of this file produced: `child.wait()` blocked for as long as the run did, the
/// run waited for as long as its client was attached, and any failure to connect
/// turned into a hang.
///
/// It is set far above anything a slower machine could need, because the cost of
/// the two mistakes is not symmetric: too large only delays a failure that is
/// already a failure, while too small invents one. **If a test ever fails on this
/// bound, the answer is not a bigger number** — it is that something is genuinely
/// stuck, and `ProjektPlan.md` §9.2's rule against tuning a timeout until a
/// flake disappears applies exactly here.
const PATIENCE: Duration = Duration::from_secs(20);

/// A serving run, and everything a test needs to talk to it and to end it.
struct Serving {
    child: Child,
    port: u16,
    /// Whatever the run printed to stderr after the listening line.
    stderr: mpsc::Receiver<String>,
}

/// Starts `narvo` serving an agent, and hands back the port it took.
///
/// The port comes out of the process's own stderr rather than being chosen here:
/// `--ipc 127.0.0.1:0` asks the operating system for a free one, which is what
/// keeps two of these tests running at once from colliding — and what keeps
/// whatever else is on the machine out of it. **A fixed port would be the
/// classic way this becomes flaky**, through a collision or a socket still in
/// `TIME_WAIT`, and it is worth saying that is not what these tests do.
///
/// The reading of stderr is on its own thread feeding a channel, because `std`
/// has no timed read on a pipe and a blocking one here is the hang this file
/// exists to have removed. The thread is a *test* thread; nothing in the engine
/// grows one.
fn serving(ticks: &str) -> Serving {
    serving_with(&["--headless", "--ticks", ticks, "--ipc", "127.0.0.1:0"])
}

/// The same, for a run whose command line is not `--ticks N`.
///
/// M6.4a needs one: a write during a replay is refused now, and the only way to
/// have a replay serving an agent is to start one — `--replay … --ipc …` has
/// always been a legal pair.
fn serving_with(arguments: &[&str]) -> Serving {
    let mut child = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args(arguments)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary this test was built beside starts");

    let stderr = child.stderr.take().expect("stderr was piped");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let line = receiver
        .recv_timeout(PATIENCE)
        .expect("the run says where it is listening, well inside the brake");
    let port = line
        .rsplit_once(':')
        .and_then(|(_, port)| port.trim().parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no port in {line:?}"));

    Serving {
        child,
        port,
        stderr: receiver,
    }
}

/// **Ends the run when the test that started it goes away.**
///
/// Rust's `Child` does not kill on drop — the standard library says so in
/// `Child`'s own documentation ("there is no implementation of `Drop` for child
/// processes, so if you do not ensure the `Child` has exited then it will
/// continue to run") — so before this existed a test that panicked left its
/// `narvo` behind. Not merely running, but *stuck*: a serving run with nobody
/// attached waits for its first client for ever, which is the price D21 accepted
/// for `--ipc`, and a panic pointed that price at the harness itself.
///
/// **The search method that establishes it, and its limit.** Liveness is checked
/// here through the run's own listening port rather than a process list: a
/// listener exists exactly as long as the process holding it does, and the check
/// is inside the test, at the instant after the drop, rather than a snapshot
/// taken afterwards from outside — which is what the M6.3d diagnosis found
/// wanting, since a process list sees one point in time and neither the moment
/// before nor the moment after. What it cannot rule out is another process
/// taking the same ephemeral port in the microseconds between; a PID check has
/// the mirror-image weakness, because Windows reuses PIDs.
///
/// **Where this stops, stated rather than implied.** `Drop` runs on an ordinary
/// return and during a panic unwind — the case that produced the orphans. It
/// does **not** run when the process aborts (a `panic = "abort"` profile, a
/// double panic, `std::process::abort`) or when the test process is itself
/// killed from outside. A run left behind by one of those is not something this
/// can reach, and this claims no more than the two cases it covers.
impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();

        // Reaping, bounded like everything else in this file. A `wait` that
        // never returned would be a fresh version of the very hang the brake
        // exists to have removed, sited in the one place that runs during an
        // unwind and could therefore swallow the failure that caused it.
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::yield_now(),
            }
        }
    }
}

impl Serving {
    /// Connects to the run, with every later read bounded.
    fn connect(&self) -> BufReader<TcpStream> {
        let socket =
            TcpStream::connect(("127.0.0.1", self.port)).expect("connecting to the run we started");
        socket
            .set_read_timeout(Some(PATIENCE))
            .expect("a read bound on a socket we own");
        BufReader::new(socket)
    }

    /// Every line the run wrote to stderr after the listening one, once it has
    /// ended.
    ///
    /// **Bounded waiting rather than `try_iter`, and the difference is a flake
    /// that was measured.** Until M6.4a this was `self.stderr.try_iter()` at each
    /// call site, which is non-blocking: the child having exited does not mean
    /// the *reader thread in this process* has drained the pipe and pushed the
    /// lines into the channel yet. Under load on a freshly compiled target that
    /// thread is descheduled often enough to matter — **three of eight runs on
    /// WSL** failed with an empty vector, in two different tests, and the
    /// assertion message printed exactly `[]`.
    ///
    /// It is the file's own doctrine applied to the last place that had escaped
    /// it: wait, but never unboundedly. `recv_timeout` ends on
    /// `Disconnected` — the reader thread drops its sender when the pipe reaches
    /// end of file, which a dead child guarantees — so the normal case returns as
    /// soon as the lines are in, and `PATIENCE` is there only so that a reader
    /// thread that never finishes is a red test rather than a hang.
    ///
    /// **It is not the D21 flake.** That one consumes the whole brake and leaves
    /// a live, blocked run; this failed in 6 to 30 ms with an empty channel, and
    /// its cause is in this file rather than in the transport.
    fn summary(&self) -> Vec<String> {
        let deadline = Instant::now() + PATIENCE;
        let mut lines = Vec::new();

        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.stderr.recv_timeout(left) {
                Ok(line) => lines.push(line),
                // Disconnected: the pipe is at end of file and there is nothing
                // more to come. Timeout: the brake, which is a failure the
                // caller's own assertion will report.
                Err(_) => return lines,
            }
        }
    }

    /// Whatever the run wrote to stdout, which is its report and nothing else.
    ///
    /// Read after the run has ended, so the pipe is at end of file and this
    /// cannot block. Draining it is also what keeps a report longer than the
    /// pipe's buffer from stalling the run that is writing it — not a hazard at
    /// sixteen hex digits, and named because the next report might not be.
    fn stdout(&mut self) -> String {
        use std::io::Read as _;

        let mut text = String::new();
        if let Some(mut out) = self.child.stdout.take() {
            let _ = out.read_to_string(&mut text);
        }
        text
    }

    /// Waits for the run to end, and **kills it rather than waiting for ever**.
    ///
    /// Returns whether it ended on its own. A test that gets `false` has found a
    /// stuck run, which is the failure this bound exists to turn into a red.
    fn finished_on_its_own(&mut self) -> bool {
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            match self.child.try_wait().expect("asking after our own child") {
                Some(status) => return status.success(),
                None => std::thread::yield_now(),
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        false
    }
}

/// Sends one request and reads one answer, within the brake.
fn ask(socket: &mut BufReader<TcpStream>, request: &str) -> String {
    socket
        .get_mut()
        .write_all(format!("{request}\n").as_bytes())
        .expect("write to a socket we opened");
    socket.get_mut().flush().expect("flush");

    let mut answer = String::new();
    socket
        .read_line(&mut answer)
        .expect("an answer comes back inside the brake");
    answer.trim_end().to_owned()
}

/// **An agent connects to a running engine, reads it, steers it, and lets it go.**
///
/// The end-to-end claim of M6.3d in one test: a real process, a real socket, and
/// no in-process double anywhere. It reads the world, grants the run three ticks,
/// reads it again, and disconnects — and the run then ends, because a client that
/// has been and gone is not one to wait for.
#[test]
fn an_agent_reads_a_running_engine_and_steers_it() {
    let mut run = serving("0");
    let mut socket = run.connect();

    // The run is stopped at tick 0, waiting. A read is answered against that.
    let listed = ask(&mut socket, "\"list_entities\"");
    assert!(
        listed.starts_with("{\"list_entities\":{\"entities\":["),
        "{listed}"
    );

    // Three more ticks, granted from outside a process that had none left.
    assert_eq!(
        ask(&mut socket, "{\"step\":{\"ticks\":3}}"),
        "{\"step\":{\"granted\":3,\"ticks_run\":0}}"
    );

    // And the engine is still there afterwards, answering against the world
    // those three ticks left behind.
    //
    // **The two answers are no longer the same bytes, and that is the finding
    // rather than a nuisance.** Since M6.7b an answer says how many ticks had run
    // when it was given, so a read before the step and a read after it differ in
    // exactly one field — which is the property M6.7a measured to be missing. The
    // entity set is what has to be unchanged, so the entity set is what is
    // compared, and the moment is asserted to have moved rather than ignored.
    let listed_again = ask(&mut socket, "\"list_entities\"");
    let entities = |answer: &str| {
        answer
            .split_once("],")
            .map(|(head, _)| head.to_owned())
            .unwrap_or_else(|| panic!("not an entity list: {answer}"))
    };
    assert_eq!(
        entities(&listed),
        entities(&listed_again),
        "the entity set is not what moved"
    );
    //
    // **The relation, never the number.** The first draft of this pinned the
    // second read at `ticks_run: 3` and went red at 2: a request lands on
    // whatever moment the run has reached, and the three granted ticks are still
    // running while it arrives. That is this file's own opening warning — a test
    // that connects to a moving run is betting on the scheduler — caught by the
    // instrument the same task was adding. So: the first read is dated before any
    // tick, the second after at least one, and neither can be past the budget.
    let dated = |answer: &str| -> u64 {
        answer
            .rsplit_once("\"ticks_run\":")
            .and_then(|(_, tail)| tail.trim_end_matches("}}").parse().ok())
            .unwrap_or_else(|| panic!("no ticks_run in {answer}"))
    };
    assert_eq!(dated(&listed), 0, "the run was waiting at tick zero");
    let after = dated(&listed_again);
    assert!(
        (1..=3).contains(&after),
        "the second read has to be dated inside the three granted ticks, got {after}"
    );

    drop(socket);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );
}

/// A malformed line is answered by the transport, and the run carries on.
#[test]
fn a_malformed_line_is_refused_over_the_wire_and_the_run_survives_it() {
    let mut run = serving("0");
    let mut socket = run.connect();

    let refused = ask(&mut socket, "not json at all");
    assert!(
        refused.starts_with("{\"error\":{\"message\":\"1:1: the text is not a request"),
        "{refused}"
    );

    // Still serving: the next request is answered normally.
    assert_eq!(
        ask(&mut socket, "{\"step\":{\"ticks\":1}}"),
        "{\"step\":{\"granted\":1,\"ticks_run\":0}}"
    );

    drop(socket);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );
}

/// **A write over the wire cuts the band**, which is D19 reaching the socket.
#[test]
fn a_write_over_the_wire_is_accepted_and_the_run_reports_it() {
    let mut run = serving("0");
    let mut socket = run.connect();

    assert_eq!(
        ask(&mut socket, "{\"step\":{\"ticks\":4}}"),
        "{\"step\":{\"granted\":4,\"ticks_run\":0}}"
    );

    let written = ask(
        &mut socket,
        "{\"set_component\":{\"entity\":\"0v1\",\"component\":\"position\",\"value\":\"(x:9999,y:-9999)\"}}",
    );
    assert!(
        written.starts_with("{\"set_component\":{\"entity\":\"0v1\",\"component\":\"position\","),
        "{written}"
    );

    drop(socket);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );

    // The run's own summary says how far it got, which is the grant and no more.
    let summary = run.summary();
    assert!(
        summary.iter().any(|line| line.contains("4 ticks")),
        "the run did not report four ticks: {summary:?}"
    );
}

/// **The brake, shown in the state it exists for.**
///
/// A new verification instrument is demonstrated failing, not only succeeding —
/// and this one has a specific failure to demonstrate, because the first version
/// of this file did not have it. A serving run that nobody ever connects to waits
/// for ever, which is what `--ipc` means; without a bound the suite would stop
/// talking here, and the first M6.3d session measured exactly that under red edge
/// (a1) and got a hang where it wanted a red.
///
/// **It is deliberately the cheap half of the demonstration.** Waiting out
/// `PATIENCE` would cost twenty seconds of every run, so this asserts the two
/// halves separately: that a stuck run is *still running* after a moment (so
/// `try_wait` would keep returning `None` and the old `child.wait()` would still
/// be blocked), and that killing it is what ends it. The bound's arithmetic is
/// `Instant` and a comparison; what needed showing is that the case it guards is
/// real and reachable.
#[test]
fn a_run_nobody_connects_to_is_stuck_and_the_brake_is_what_ends_it() {
    let mut run = serving("0");

    // Nobody connects. The run is waiting for its first client, so it is still
    // there — this is the state that used to hang the suite.
    for _ in 0..1_000 {
        std::thread::yield_now();
    }
    assert!(
        run.child
            .try_wait()
            .expect("asking after our own child")
            .is_none(),
        "a serving run with no client ended by itself, so this test guards nothing"
    );

    // And the brake is what gets us out of it.
    run.child.kill().expect("killing our own child");
    let status = run.child.wait().expect("it goes once killed");
    assert!(
        !status.success(),
        "a killed run reported success: {status:?}"
    );
}

/// **A test that panics does not leave its run behind**, which is the other half
/// of the same guarantee and the one that was missing.
///
/// The brake turns a stuck run into a red test. It does nothing about what
/// happens *after* that red: the panic unwinds, the `Serving` goes out of scope,
/// and until [`Serving`]'s `Drop` existed the `narvo` process it named simply
/// stayed — waiting for a first client that was never going to come, because the
/// test that would have connected had already failed. Every red socket test left
/// one, and a suite that reports a failure while quietly accumulating stuck
/// processes is the instrument this milestone spent six sessions distrusting.
///
/// This drives the case directly: a run is started, nobody connects, a panic is
/// raised and caught, and the run's listener is asked whether it is still there.
/// `catch_unwind` is what makes it observable — the `Serving` is moved into the
/// closure, so the unwind is what drops it, exactly as a failing test would.
///
/// Its own failure state was measured rather than argued: with the `Drop`
/// removed, the connection below **succeeds**, because the orphan is still
/// listening.
#[test]
fn a_run_whose_test_panicked_is_not_left_behind() {
    let run = serving("0");
    let port = run.port;

    // The run is serving and unattached, so it is waiting and will not end by
    // itself. Nothing but the drop can finish it.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _run = run;
        panic!("a test failing while a run is attached to it");
    }));
    assert!(outcome.is_err(), "the closure was supposed to panic");

    // The listener died with the process that held it.
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "the run outlived the test that started it: something is still listening \
         on port {port}"
    );
}

/// A committed scene, named the way an agent has to name one.
///
/// Relative, because a scene path may not be absolute (ADR-0019's normal form),
/// and therefore resolved against the working directory the *engine process*
/// inherits from this test — which cargo sets to the package root.
const A_REAL_SCENE: &str = "scenes/determinism-case.ron";

/// Records a short run with the same binary, and hands back the file.
///
/// Under `CARGO_TARGET_TMPDIR`, which cargo gives an integration test and which
/// may be absolute: a *recording* path has no normal form to keep, so unlike a
/// scene it is read wherever it is.
fn recorded(name: &str, ticks: &str) -> String {
    let path = format!("{}/{name}", env!("CARGO_TARGET_TMPDIR"));

    let made = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args([
            "--headless",
            "--mode",
            "input",
            "--ticks",
            ticks,
            "--record",
            &path,
        ])
        .output()
        .expect("it runs");
    assert!(made.status.success(), "could not record: {made:?}");

    path
}

/// **An agent loads a scene over the wire, and the run becomes that scene.**
///
/// The first half of M6.4a end to end: a real process, a real file, and the
/// answer carrying the digest of the bytes that were taken. The count is checked
/// against a `list_entities` taken afterwards rather than against a number
/// written here, so the scene may gain an entity without this test becoming a
/// thing that has to be edited to stay true.
#[test]
fn an_agent_loads_a_scene_over_the_wire_and_the_run_becomes_it() {
    let mut run = serving("0");
    let mut socket = run.connect();

    let before = ask(&mut socket, "\"list_entities\"");

    let loaded = ask(
        &mut socket,
        &format!("{{\"load_scene\":{{\"path\":\"{A_REAL_SCENE}\"}}}}"),
    );
    assert!(
        loaded.starts_with(&format!(
            "{{\"load_scene\":{{\"path\":\"{A_REAL_SCENE}\",\"digest\":\""
        )),
        "{loaded}"
    );

    let after = ask(&mut socket, "\"list_entities\"");
    assert_ne!(before, after, "the world was not replaced: {after}");

    // `entities` is no longer the last field — `ticks_run` follows it since
    // M6.7b — so the comma is what ends it rather than the closing braces.
    let count = after.matches("v1").count();
    assert!(
        loaded.contains(&format!("\"entities\":{count},")),
        "the answer's count is not the world's: {loaded} against {after}"
    );

    drop(socket);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );
}

/// **An agent starts a replay over the wire, and the run becomes it.**
///
/// The answer says what the *file* said the run was — mode, seed and length —
/// none of which the client supplied. The run then walks the recording to its
/// end without being granted anything, which is the tick counter having gone
/// back to the recording's own zero.
#[test]
fn an_agent_starts_a_replay_over_the_wire_and_the_run_walks_it() {
    let path = recorded("agent-replay.rec", "40").replace('\\', "\\\\");

    let mut run = serving("0");
    let mut socket = run.connect();

    let started = ask(
        &mut socket,
        &format!("{{\"replay\":{{\"path\":\"{path}\"}}}}"),
    );
    assert!(
        started.starts_with("{\"replay\":{"),
        "the replay was refused: {started}"
    );
    assert!(started.contains("\"mode\":\"input\""), "{started}");
    assert!(started.contains("\"ticks\":40"), "{started}");

    drop(socket);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );

    // Forty ticks, from a run whose command line asked for none.
    let summary = run.summary();
    assert!(
        summary.iter().any(|line| line.contains("40 ticks")),
        "the run did not walk the recording: {summary:?}"
    );
}

/// **A write over the wire during a replay is refused**, and this closes a hole
/// that was open and reachable.
///
/// `--replay … --ipc …` has always been a legal command line. Before M6.4a a
/// `set_component` over that socket was **accepted**: measured, and what came
/// out was a replay reporting a state hash that was not the recorded run's while
/// the recording it produced stayed byte-identical to the one it had been given.
/// A run's own account said "this is that recording" about a world that had been
/// written to.
///
/// The refusal is asserted here rather than only in `ipc::tests` because that is
/// where the hole was: over a socket, in a process, on a command line a user can
/// type.
#[test]
fn a_write_over_the_wire_during_a_replay_is_refused_and_the_replay_is_intact() {
    let path = recorded("agent-guarded.rec", "20");

    let mut run = serving_with(&[
        "--headless",
        "--replay",
        &path,
        "--ipc",
        "127.0.0.1:0",
        "--hash",
    ]);
    let mut socket = run.connect();

    // The reads are untouched: a replay answers questions.
    let listed = ask(&mut socket, "\"list_entities\"");
    assert!(listed.starts_with("{\"list_entities\":"), "{listed}");

    let refused = ask(
        &mut socket,
        "{\"set_component\":{\"entity\":\"0v1\",\"component\":\"position\",\"value\":\"(x:9999,y:-9999)\"}}",
    );
    assert!(
        refused.starts_with("{\"error\":{\"message\":\"set_component is refused during a replay: "),
        "{refused}"
    );

    drop(socket);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );

    // And the replay reached the state the recorded run reached, which is the
    // property the write used to break.
    let plain = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args(["--headless", "--replay", &path, "--hash"])
        .output()
        .expect("it runs");
    assert!(plain.status.success());
    assert_eq!(
        run.stdout(),
        String::from_utf8_lossy(&plain.stdout),
        "the replay that was talked to reported a different state hash"
    );
}

/// **The protocol crate's own client drives a real run**, which is the one place
/// it meets a real process over a real wire.
///
/// Everything else in this file writes its own bytes and reads answers with
/// `std`'s `read_line` — deliberately, and that is what makes those nine tests
/// an *independent* check on the framing rather than a mirror of it (M6.5a's
/// S5). This one is the opposite check: that the client an agent will actually
/// use talks to the engine as it is, not to a fake standing in for it.
///
/// `narvo_ipc` is reachable here because it is one of this package's
/// `[dependencies]`, which an integration test sees as well as the
/// `[dev-dependencies]` — CLAUDE.md's M2.1 entry, where assuming the opposite
/// made a comment false.
#[test]
fn the_protocol_clients_own_conversation_with_a_real_run() {
    use narvo_ipc::{Client, Request, Response};

    let mut run = serving("0");
    let mut client = Client::connect(&format!("127.0.0.1:{}", run.port), PATIENCE)
        .expect("connecting to the run we started");

    let listed = client
        .ask(&Request::ListEntities)
        .expect("an answer comes back");
    assert!(
        matches!(listed, Response::ListEntities { .. }),
        "{listed:?}"
    );

    // `--ticks 0` waits before any tick has run, so the moment is `Waiting` and
    // its count is zero — the case `cut_after` could not express and the one
    // `ticks_run` reports as the plain number it is.
    assert_eq!(
        client
            .ask(&Request::Step { ticks: 3 })
            .expect("an answer comes back"),
        Response::Step {
            granted: 3,
            ticks_run: 0,
        }
    );

    // A refusal is an answer like any other, and reaches the client as one
    // rather than as a transport failure.
    let refused = client
        .ask(&Request::LoadScene {
            path: "no/such/scene.ron".to_owned(),
        })
        .expect("a refusal is still an answer");
    assert!(matches!(refused, Response::Error { .. }), "{refused:?}");

    drop(client);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );
}

/// **A run nobody asked to serve still ends on its own**, which is the property
/// every existing invocation depends on.
///
/// The binary here has the transport compiled in — this file only exists in that
/// configuration — and without `--ipc` it behaves exactly as it always did.
#[test]
fn a_run_without_the_flag_ends_by_itself_even_in_a_build_that_has_a_transport() {
    let finished = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args(["--headless", "--ticks", "600"])
        .output()
        .expect("it runs");

    assert!(finished.status.success());
    assert!(
        String::from_utf8_lossy(&finished.stdout).contains("600 ticks"),
        "{}",
        String::from_utf8_lossy(&finished.stdout)
    );
}

/// **The chain M6.7b exists to close, walked end to end.**
///
/// An agent asks a running engine for a dump, writes what comes back to a file,
/// and hands that file to the repro runner — which accepts it. Every link is a
/// real one: a real process, a real socket, a real file, and a second process
/// that never saw the first.
///
/// # The unmoved reference
///
/// The command-line dump. Nothing in M6.7b touched `--dump`, `--expect` or the
/// headless runner's report path, so `narvo --ticks N --dump` is a value this
/// task cannot have bent — and the wire dump is compared against it **byte for
/// byte** rather than against a second computation of the same thing. That is the
/// class M6.4a checked at `cut_after`→`cut_to`: code and its tests moving
/// together, checked against something that did not move.
///
/// # Why the tick is read out of the answer rather than assumed
///
/// A request lands on whatever moment the run has reached — measured in M6.7b, a
/// mid-flight run answers four sequential reads at four different ticks — so this
/// test asserts the *relation* instead of a number: whatever `ticks_run` the
/// engine reports, a run of exactly that many ticks produces exactly that state.
/// A pinned number would be a race; this cannot be.
#[test]
fn a_dump_off_the_wire_is_the_command_lines_dump_and_the_repro_runner_takes_it() {
    use narvo_ipc::{Client, Request, Response};

    let mut run = serving("0");
    let mut client = Client::connect(&format!("127.0.0.1:{}", run.port), PATIENCE)
        .expect("connecting to the run we started");

    // Seven ticks, so the world is not the one a run of zero ticks ends in and
    // the comparison below has something to be wrong about.
    let granted = client
        .ask(&Request::Step { ticks: 7 })
        .expect("an answer comes back");
    assert!(
        matches!(granted, Response::Step { granted: 7, .. }),
        "{granted:?}"
    );

    let dumped = client.ask(&Request::Dump).expect("an answer comes back");
    let Response::Dump { state, ticks_run } = dumped else {
        panic!("a dump request is answered with a dump: {dumped:?}");
    };
    assert!(state.starts_with("entities "), "{state}");

    drop(client);
    assert!(
        run.finished_on_its_own(),
        "the run did not end when its client went"
    );

    // The unmoved reference: the same simulation, the same length, on the command
    // line. `serving` takes the default mode and seed, so these are spelled out
    // rather than omitted — a default that moved would otherwise move this test's
    // meaning without moving its text.
    let ticks = ticks_run.to_string();
    let reference = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args([
            "--mode", "motion", "--seed", "1", "--ticks", &ticks, "--dump",
        ])
        .output()
        .expect("it runs");
    assert!(reference.status.success());

    assert_eq!(
        state.as_bytes(),
        reference.stdout.as_slice(),
        "the wire dump is not byte-identical to the command line's at {ticks} ticks"
    );

    // And the last link: the file an agent would write, handed to `--expect`.
    let directory = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("wire-repro");
    std::fs::create_dir_all(&directory).expect("the output directory must be creatable");
    let expected = directory.join("from-the-wire.dump");
    std::fs::write(&expected, state.as_bytes()).expect("the expected state must be writable");

    let judged = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args(["--mode", "motion", "--seed", "1", "--ticks", &ticks])
        .arg("--expect")
        .arg(&expected)
        .output()
        .expect("it runs");

    let said = String::from_utf8_lossy(&judged.stderr).into_owned();
    assert!(
        judged.status.success(),
        "the repro runner refused a dump this engine had just produced: {said}"
    );
    assert!(
        said.contains(&format!("reproduced - after {ticks} ticks")),
        "{said}"
    );
}
