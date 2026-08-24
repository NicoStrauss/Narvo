//! The headless runner: the fixed-timestep loop with a simulation on it and
//! nothing else attached.
//!
//! Needs no GPU, no window and no graphics stack, which is the whole point of
//! the `render` feature gate. Compiled in every configuration, and the path an
//! agent verifies against most often.
//!
//! Since M2.3 the loop also owns the input boundary. Where a tick's input comes
//! from — a seeded stand-in for a player, or a recorded file — is the runner's
//! business and not the simulation's: the world is fed the same list either way
//! and cannot tell which. That indistinguishability is what makes a replay a
//! replay rather than a re-simulation, and ADR-0012 records it.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use narvo_audio::Sink as _;
use narvo_core::FixedTimestep;
use narvo_ecs::{EcsError, Rng, SystemContext, canonical_dump};
use narvo_input::InputEvent;

use crate::audio;
use crate::ipc::{After, Channel, Inbox, Moment, RunControl, Silent};
use crate::recording::Recording;
use crate::scene_anchor::Anchor;
use crate::sim::scene_file::SceneStartError;
use crate::sim::{self, Mode, RecordingMismatch};

/// What a headless run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Simulation ticks executed. Exactly what was asked for.
    pub ticks: u64,
    /// Entities alive at the end.
    pub entities: u32,
    /// The canonical dump of the final state, ready to hash or to diff.
    pub dump: String,
    /// Every input this run fed to the simulation, as a recording.
    ///
    /// Produced by every run, not only by one asked to record, and that is what
    /// keeps "what was fed" and "what would be written to a file" the same list
    /// by construction. A replay produces one too — and for a full-length replay
    /// it is equal to the recording it was given, which is a property worth
    /// testing and cheap to hold.
    pub recording: Recording,
}

/// A scene file that has been read and checked, ready to constitute a world.
///
/// The two halves travel together on purpose: the text is what the world is
/// built from, and the anchor is what says that text is the file the recording
/// names. Separating them would let a caller build from one and record the
/// other.
#[derive(Debug, Clone)]
pub struct LoadedScene {
    /// Which file it was, and what it hashed to.
    pub anchor: Anchor,
    /// Its text, from the read the anchor was taken over.
    pub text: String,
}

/// Where a run's input comes from, and what determines the run.
#[derive(Debug, Clone)]
pub enum Plan {
    /// Simulate, generating synthetic input from the seed.
    Live {
        /// Which simulation to drive.
        mode: Mode,
        /// The seed for the simulation and for the synthetic input.
        seed: u64,
        /// How many ticks to run.
        ticks: u64,
        /// The scene to constitute the world from, for [`Mode::SceneFile`].
        ///
        /// `None` for every other mode, whose world is built in code. Reading
        /// the file and computing its anchor happens before this plan is built,
        /// in `main.rs` — this module does no file I/O, which is what lets its
        /// own logic be tested without one.
        ///
        /// **The property is the module's, not the test module's**, and M6.4a is
        /// where the difference started to matter: a `load_scene` request names a
        /// path, so the four tests of that seam at the end of this file do write
        /// files. `crate::ipc` performs the read; nothing here does.
        scene: Option<LoadedScene>,
    },
    /// Replay a recording, feeding its inputs instead of generating any.
    ///
    /// Mode and seed are taken from the recording rather than from the caller:
    /// they are what the file says the run *was*, and a second opinion about
    /// them could only ever disagree.
    Replay {
        /// The recording to replay.
        recording: Recording,
        /// The scene this recording was constituted from, already checked.
        ///
        /// `Some` exactly when the recording carries an anchor. The check
        /// happens before this plan is built — in `main.rs`, which is what
        /// touches the disk — so by the time a run starts, the file has been
        /// read once and found to be the one the recording names (ADR-0019).
        scene: Option<LoadedScene>,
        /// Stop after this many ticks instead of at the recording's end.
        ///
        /// For bisecting: a divergence found at ten thousand ticks is chased by
        /// asking where it started. Never longer than the recording — running
        /// past the recorded input would silently continue with no input at all,
        /// which is not a replay of anything.
        ticks: Option<u64>,
    },
}

/// Why a run could not be made.
#[derive(Debug)]
pub enum RunError {
    /// A `scene-file` run reached the runner without a scene to build from.
    ///
    /// Not something a file can cause: a recording naming `scene-file` without
    /// an anchor is refused when it is parsed, and a live run without
    /// `--scene` is refused on the command line. It is here so that a future
    /// caller assembling a plan by hand gets an error rather than a world built
    /// from the wrong thing.
    SceneMissing,
    /// A scene file could not constitute a world.
    Scene(SceneStartError),
    /// Building or dumping the simulation failed.
    Ecs(EcsError),
    /// The recording asks for something the mode it names cannot do.
    Mismatch(RecordingMismatch),
    /// A replay was asked to run past the end of its recording.
    TooShort {
        /// Ticks that were asked for.
        asked: u64,
        /// Ticks the recording covers.
        covered: u64,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ecs(error) => error.fmt(f),
            Self::Scene(error) => error.fmt(f),
            Self::SceneMissing => f.write_str(
                "a scene-file run needs a scene to constitute its world from, and none reached \
                 the runner; pass --scene <file> for a live run, and for a replay use a \
                 recording that carries its scene anchor",
            ),
            Self::Mismatch(error) => error.fmt(f),
            Self::TooShort { asked, covered } => write!(
                f,
                "this recording covers {covered} ticks and {asked} were asked for; past the end \
                 of a recording a replay would run on with no input at all, which reproduces \
                 nothing. Record a longer run, or ask for at most {covered}"
            ),
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Ecs(error) => Some(error),
            Self::Scene(error) => Some(error),
            Self::Mismatch(error) => Some(error),
            Self::SceneMissing | Self::TooShort { .. } => None,
        }
    }
}

impl From<SceneStartError> for RunError {
    fn from(error: SceneStartError) -> Self {
        Self::Scene(error)
    }
}

impl From<EcsError> for RunError {
    fn from(error: EcsError) -> Self {
        Self::Ecs(error)
    }
}

impl From<RecordingMismatch> for RunError {
    fn from(error: RecordingMismatch) -> Self {
        Self::Mismatch(error)
    }
}

/// Frame durations fed to the accumulator, in microseconds.
///
/// Deliberately ragged and deliberately synthetic. Nothing here reads a clock:
/// a headless run has to produce the same numbers on every machine and every
/// day. An uneven pattern also exercises the accumulator's carry-over rather
/// than landing on tick boundaries.
const FRAME_TIMES_US: [u64; 6] = [16_700, 9_100, 33_400, 4_250, 21_900, 12_000];

/// Where one tick's input is taken from.
///
/// `pub` since M6.4a, because it is one of the five things a command can
/// redirect and `crate::ipc` therefore has to be able to name it. Nothing
/// outside this module constructs a [`Pilot`](Self::Pilot); the one place a
/// [`Recorded`](Self::Recorded) is built is [`begin`], which is what keeps
/// "a replay's mode and seed come from its file" true of the seam as well as of
/// the command line.
pub enum Source {
    /// A seeded stand-in for a player.
    Pilot(Rng),
    /// A recording, walked in order.
    Recorded {
        /// The inputs, in tick order.
        recording: Recording,
        /// Index of the next input not yet delivered.
        next: usize,
    },
}

impl Source {
    /// Everything that belongs to `tick`.
    fn take(&mut self, tick: u64, mode: Mode) -> Vec<InputEvent> {
        match self {
            Self::Pilot(rng) => sim::pilot(mode, rng),
            Self::Recorded { recording, next } => {
                let inputs = recording.inputs();
                let mut events = Vec::new();

                // The inputs are in tick order and the loop below visits every
                // tick from zero upwards, so everything for this tick is one
                // contiguous run starting at the cursor.
                while *next < inputs.len() && inputs[*next].tick == tick {
                    events.push(inputs[*next].event.clone());
                    *next += 1;
                }

                events
            }
        }
    }
}

/// Runs `plan` and returns its final state and the input it fed.
///
/// The fixed timestep is in the loop rather than beside it (ADR-0003): synthetic
/// frame durations go into the accumulator and whole ticks come out, which is
/// the shape the windowed loop has too. The systems never see a duration — only
/// a tick number — so a run of `n` ticks is exactly the prefix of a run of more,
/// which is what lets a replay be compared against the original at an
/// intermediate tick rather than only at the end.
///
/// # Errors
///
/// [`RunError`] if the simulation cannot be built or dumped, if a recording
/// names input the mode cannot consume, or if a replay was asked to run past the
/// end of its recording.
pub fn run(plan: Plan) -> Result<Run, RunError> {
    run_with(plan, &mut Inbox::new(), &mut Silent)
}

/// The same run, with an inbox whose requests are answered at each tick boundary.
///
/// **The agent seam, and the whole of what M6.3a adds to a run.** An empty inbox
/// makes this exactly [`run`]: the drain below walks a queue and finds nothing,
/// so no run that nobody is asking about can be told apart from one made before
/// this function existed — which is the regression evidence `ProjektPlan.md`
/// §6/M6 asks for first, held here by
/// `an_empty_inbox_leaves_a_run_byte_identical` and across the whole determinism
/// suite besides.
///
/// The inbox is a parameter rather than a field of [`Plan`] because it is not
/// part of what the run *is*: a plan says which simulation, which seed and which
/// inputs, and two runs of one plan have to agree on their final state whatever
/// anybody asked them along the way. Being outside the world is ADR-0012's rule
/// for the input source, applied to a queue that is not even input; see
/// `crate::ipc`.
///
/// # Errors
///
/// Exactly what [`run`] returns. A request that cannot be answered produces an
/// error *response* and never an error here — asking about an entity that has
/// been despawned is not a reason to stop a simulation.
pub fn run_with(plan: Plan, inbox: &mut Inbox, channel: &mut dyn Channel) -> Result<Run, RunError> {
    let Begun {
        mut simulation,
        mut mode,
        mut source,
        mut budget,
        seed,
        anchor,
    } = begin(plan)?;

    let mut produced = Recording::new(mode, seed, budget);
    if let Some(anchor) = anchor {
        produced.anchor_to(anchor);
    }
    let mut timestep = FixedTimestep::default();
    let mut tick = 0_u64;
    let mut frame = 0_usize;

    // The headless half of the audio seam. It discards, and that is the whole
    // intent: a headless run has no device and must not gain one, and its
    // stdout is what the determinism suite compares — a cue printed here would
    // be a new output to keep byte-identical for no gain.
    //
    // What it buys instead is that `audio::cues_of` is *called* in the
    // configuration steps seven to nine build, so a mistake in the extraction
    // is a failure there rather than only in the window nobody automates. For
    // every mode but `scene-file` the world carries no `Tally` and the call
    // returns an empty list, so this costs one enumeration per tick and
    // produces nothing.
    let mut cues = audio::CueMemory::new(&simulation.world);
    let mut sink = narvo_audio::NullSink::discarding();

    // The three synthesised sounds, and nothing of the demo's own. A headless
    // run plays nothing, but it still resolves every handle a cue carries —
    // which is the check that used to exist only in the windowed path.
    let library = narvo_audio::SoundLibrary::new();

    // **The run's length, and the hinge M6.3d turned.**
    //
    // Until M6.3c this was the plan's `ticks` and nothing could move it, so how
    // far a run went was settled before tick 0. A `step` request raises it, which
    // is why the two loop conditions below read a variable rather than the
    // parameter — and `--ticks` keeps its meaning exactly: it is the budget a run
    // starts with, and with no command it is also the budget it ends with.
    //
    // Since M6.4a a `replay` **replaces** it with the recording's own length,
    // which is the one command that can also lower it. That is not a second
    // meaning for the budget: the run has become a different run, and the new
    // budget is that run's.
    //
    // **Whether the run is reproducing is no longer read once.** It was, until
    // M6.4a — `replaying` was a `let` before the loop, because it was a property
    // of the plan. A command that starts a replay makes it a property of the
    // moment instead, so it is asked of `source` at each drain rather than
    // remembered from before tick 0.
    loop {
        // **The wait, and the hinge M6.3c named.** Until this task an exhausted
        // budget simply ended the run; now it ends the run only when there is
        // nobody who could extend it. `Channel::attached` is the whole of the
        // answer to "does a run with the transport on but no client hang?" — with
        // nobody connected it is false and this is the same `break` the old
        // `while tick < budget` performed.
        //
        // **A wait has to answer, not merely queue.** The command that ends a
        // wait is a `step`, and a `step` raises the budget by being *answered*;
        // a wait that only enqueued would block forever on the very request that
        // was meant to release it. That is why there is a drain here as well as
        // inside the tick, and why `Moment` exists to tell the two apart.
        while tick >= budget {
            if !channel.attached() {
                break;
            }
            let Some(request) = channel.awaited() else {
                // Nobody left to wait for. A client that disconnected mid-wait
                // ends the run rather than hanging it.
                break;
            };
            inbox.push(request);
            let after = inbox.answer_pending(RunControl {
                at: Moment::Waiting { ticks_run: tick },
                simulation: &mut simulation,
                mode: &mut mode,
                source: &mut source,
                budget: &mut budget,
                band: &mut produced,
                cues: &mut cues,
            });
            channel.answered(&inbox.take_answers());

            // A replay started here is the reason this wait can end without a
            // `step`: the count goes back to zero and the recording's own length
            // becomes the budget, so the condition above is false on the next
            // test and the run walks the recording from its first tick.
            if after == After::Restart {
                tick = 0;
            }
        }
        if tick >= budget {
            break;
        }

        let micros = FRAME_TIMES_US[frame % FRAME_TIMES_US.len()];
        frame += 1;

        let due = timestep.advance(Duration::from_micros(micros));
        for _ in 0..due {
            if tick >= budget {
                break;
            }

            // Whatever the transport has taken delivery of since the last tick.
            // Never blocks: a run nobody is talking to pays one empty call per
            // tick, which is the same shape `audio::cues_of` already pays.
            for request in channel.arrived() {
                inbox.push(request);
            }

            // Fed between ticks, which is what makes it readable inside this one
            // once the rotation at the top of the tick runs. See the scheduler
            // in `sim::input`.
            let events = source.take(tick, mode);
            // D19's band cut, on the recording side: once a write has been
            // accepted the band says nothing about what came after, so it stops
            // taking input. The input itself still reaches the world below — the
            // simulation carries on, and only the account of it stops.
            if inbox.band_is_open() {
                for event in &events {
                    produced.push(tick, event.clone());
                }
            }
            sim::feed(mode, &mut simulation.world, events)?;

            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));

            // After the systems, so the counters this reads are the ones this
            // tick left behind; before the increment, so the cue carries the
            // tick that produced it. Once per tick including catch-up ticks,
            // which is what keeps the list independent of frame timing.
            for cue in audio::cues_of(&simulation.world, tick, &mut cues) {
                sink.submit(&cue, &library);
            }

            // The agent seam, at the same boundary and for a reason of the same
            // shape: after the systems, so what an answer reports is the state
            // this tick left behind — byte for byte what `canonical_dump` would
            // print here — and before the increment, so the tick that produced
            // the answer is this one. Once per tick including catch-up ticks,
            // which is what keeps an answer independent of frame timing;
            // `crate::ipc` records why ADR-0003 is what decides that and not
            // convenience.
            //
            // Since M6.3b it takes `&mut world`, because a request may now be a
            // write. What did not change is that `crate::ipc::read` still takes
            // `&World` — the read path's guarantee is intact and narrower.
            //
            // Since M6.3c it also carries the run's own state: the band, which an
            // accepted write cuts (D19), and the budget, which a granted step
            // raises. A command drawn in here therefore takes effect at the very
            // next test of the loop condition, one line below — which is what
            // makes a step able to extend the run it is running in.
            //
            // Since M6.4a it carries the simulation itself, because two of the
            // commands replace it whole. That is why the drain takes one struct
            // rather than a world and a registry: a world and the registry that
            // names its components have to move together or the next
            // `canonical_dump` fails, which is exactly what `sim::Simulation`
            // exists to prevent a caller from getting wrong.
            let after = inbox.answer_pending(RunControl {
                at: Moment::Tick(tick),
                simulation: &mut simulation,
                mode: &mut mode,
                source: &mut source,
                budget: &mut budget,
                band: &mut produced,
                cues: &mut cues,
            });
            channel.answered(&inbox.take_answers());

            // **A replay start is the one command that moves the counter, and it
            // has to be applied here rather than inside the drain.** The
            // increment below would otherwise turn the replay's tick 0 into
            // tick 1, and a recording's first inputs would be looked for at a
            // tick that had already gone past — `Source::take` matches the tick
            // exactly, so they would be silently dropped rather than reported.
            // Breaking out of the frame's remaining ticks costs nothing: the
            // outer loop re-enters with the new budget, and the accumulator is
            // deliberately left alone because the systems never see a duration.
            if after == After::Restart {
                tick = 0;
                break;
            }

            tick += 1;
        }
    }

    let dump = canonical_dump(&simulation.world, &simulation.registry)?;

    Ok(Run {
        // The ticks that ran, which is the budget as it finally stood. Written as
        // the counter rather than as the budget so that it says what happened
        // rather than what was allowed.
        //
        // **After a replay start it is the replay's own count**, because that
        // command puts the counter back to zero — the ticks the run made before
        // being redirected are not in this number and are not anywhere else
        // either. The cut band is what still describes them.
        ticks: tick,
        entities: simulation.world.len(),
        dump,
        recording: produced,
    })
}

/// A run as it stands the moment it starts, or the moment a command redirects it.
///
/// Every field is something [`run_with`] holds in a local, and the type exists so
/// that the two callers that produce one — the runner's own prologue and
/// `crate::ipc`'s replay handler — cannot disagree about what starting a run
/// means. That is the reuse S1 asked for rather than a second reconstitution
/// path beside the first.
pub struct Begun {
    /// The world, the names its state is written under, and its systems.
    pub simulation: sim::Simulation,
    /// Which simulation is being driven.
    pub mode: Mode,
    /// Where each tick's input comes from.
    pub source: Source,
    /// How many ticks the run is for, before any `step` raises it.
    pub budget: u64,
    /// The seed, which a replay takes from its file rather than from a caller.
    pub seed: u64,
    /// The scene the world was constituted from, when it came from a file.
    pub anchor: Option<Anchor>,
}

/// Turns a plan into the run it describes.
///
/// **The prologue of [`run_with`], and nothing else.** It was inline until
/// M6.4a; a `replay` request has to do exactly this and doing it a second time
/// beside the first is how the two would come to disagree about, say, whether
/// `validate_recording` runs before the first tick. Nothing here reads a clock or
/// touches a file: the scene text arrives already read and already checked, which
/// is what keeps every test in this module free of a temporary directory.
///
/// # Errors
///
/// [`RunError::TooShort`] if a replay was asked to run past its recording,
/// [`RunError::Mismatch`] if the recording names input the mode cannot consume,
/// [`RunError::SceneMissing`] if a `scene-file` run arrived without a scene, and
/// [`RunError::Scene`] or [`RunError::Ecs`] if the world cannot be built.
pub fn begin(plan: Plan) -> Result<Begun, RunError> {
    let (mode, seed, ticks, source, scene) = match plan {
        Plan::Live {
            mode,
            seed,
            ticks,
            scene,
        } => (mode, seed, ticks, Source::Pilot(Rng::new(seed)), scene),
        Plan::Replay {
            recording,
            scene,
            ticks,
        } => {
            let covered = recording.ticks();
            let ticks = ticks.unwrap_or(covered);
            if ticks > covered {
                return Err(RunError::TooShort {
                    asked: ticks,
                    covered,
                });
            }

            // Before a single tick runs, so a file this build cannot replay
            // faithfully is reported once and up front.
            sim::validate_recording(recording.mode(), recording.inputs())?;

            let (mode, seed) = (recording.mode(), recording.seed());
            (
                mode,
                seed,
                ticks,
                Source::Recorded { recording, next: 0 },
                scene,
            )
        }
    };

    // The one place the two kinds of initial state meet: a scene-file world
    // comes from the text that was checked against the anchor, everything else
    // from the code that has always built it.
    let simulation = match (&scene, mode) {
        (Some(loaded), _) => sim::scene_file::build(&loaded.text)?,
        (None, Mode::SceneFile) => return Err(RunError::SceneMissing),
        (None, _) => sim::build(mode, seed)?,
    };

    Ok(Begun {
        simulation,
        mode,
        source,
        budget: ticks,
        seed,
        anchor: scene.map(|loaded| loaded.anchor),
    })
}

#[cfg(test)]
mod tests {
    use super::{FRAME_TIMES_US, FixedTimestep};
    use std::time::Duration;

    /// A clock the test moves by hand, standing still while it is read.
    struct HeldClock {
        now: Duration,
    }

    impl narvo_core::frame::Clock for HeldClock {
        fn now(&mut self) -> Duration {
            self.now
        }
    }

    /// A host that does nothing but count the ticks each frame asked for.
    #[derive(Default)]
    struct CountingHost {
        per_frame: Vec<u32>,
        this_frame: u32,
    }

    impl narvo_core::frame::FrameHost for CountingHost {
        type Error = std::convert::Infallible;

        fn tick(&mut self) -> Result<(), Self::Error> {
            self.this_frame += 1;
            Ok(())
        }

        fn extract(&mut self) -> Result<(), Self::Error> {
            self.per_frame.push(self.this_frame);
            self.this_frame = 0;
            Ok(())
        }

        fn acquire(&mut self) -> Result<narvo_core::frame::Acquisition, Self::Error> {
            Ok(narvo_core::frame::Acquisition::Ready)
        }

        fn encode(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn present(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn the_library_loop_hands_out_the_ticks_this_accumulator_would() {
        // `run`'s doc says the fixed timestep here is "the shape the windowed
        // loop has too". Since M3.32 that windowed loop exists as
        // `narvo_core::frame::FrameLoop`, so the sentence is checkable rather
        // than merely plausible.
        //
        // **The two loops are not merged**, deliberately. `run` counts to an
        // exact tick budget and breaks mid-frame to hit it, which the library
        // loop does not do, and rewriting the code that produces every state
        // hash is not something this task can verify: ADR-0008 stores no
        // expected hash anywhere, so the determinism suite compares two runs of
        // the *same* build and would stay green through a tick-accounting
        // regression.
        //
        // **What this test does and does not reach.** It does not call `run`;
        // `run` owns a tick budget and an input source, and neither is what
        // could drift. What it compares is the accumulator underneath both: a
        // bare `FixedTimestep` fed one sequence of durations by hand, against
        // the same durations reaching the library loop's own accumulator through
        // `step`. If `FrameLoop` ever stopped handing `advance` the whole
        // interval, or called it more than once a frame, this goes red.
        //
        // **The sequence has to be the one the library loop actually feeds**,
        // which is not `FRAME_TIMES_US`. Its first frame has no predecessor, so
        // it feeds `ZERO` and the first duration is consumed as the time origin
        // — every later frame is one duration behind. Comparing against
        // `FRAME_TIMES_US` from index 1 instead looks right and is not: the two
        // accumulators would then sit a permanent 33 334 ns apart (16 700 µs
        // against a 16 666 666 ns step), agreeing only for as long as that
        // offset happens not to cross a tick boundary. That is drift dressed as
        // agreement, and an earlier version of this test asserted it.
        let durations: Vec<Duration> =
            std::iter::once(Duration::ZERO)
                .chain((1..FRAME_TIMES_US.len() * 20).map(|frame| {
                    Duration::from_micros(FRAME_TIMES_US[frame % FRAME_TIMES_US.len()])
                }))
                .collect();

        let mut hand_rolled = FixedTimestep::default();
        let expected: Vec<u32> = durations
            .iter()
            .map(|frame_time| hand_rolled.advance(*frame_time))
            .collect();

        let mut clock = HeldClock {
            now: Duration::ZERO,
        };
        let mut host = CountingHost::default();
        let mut library = narvo_core::frame::FrameLoop::new(FixedTimestep::default());

        for frame_time in &durations {
            clock.now += *frame_time;
            library
                .step(&mut clock, &mut host)
                .expect("a counting host cannot fail");
        }

        assert_eq!(
            host.per_frame, expected,
            "the library loop scheduled ticks the bare accumulator would not"
        );

        // And the one real difference, which is in how each loop is *driven*
        // rather than in what the accumulator does: `run` hands its first
        // duration over immediately, while a frame loop has no previous frame to
        // measure the first one against.
        let mut as_run_drives_it = FixedTimestep::default();
        assert_eq!(
            as_run_drives_it.advance(Duration::from_micros(FRAME_TIMES_US[0])),
            1,
            "this loop consumes its first frame duration outright"
        );
        assert_eq!(
            host.per_frame[0], 0,
            "the library loop's first frame has nothing to measure against"
        );
    }
    use super::{LoadedScene, Plan, Run, RunError, run, run_with};
    use crate::ipc::Collected;
    use crate::recording::Recording;
    use crate::scene_anchor::Anchor;
    use crate::sim::Mode;
    use narvo_ecs::state_hash;

    /// The seed the mode-independent tests use. Any value would do.
    const SEED: u64 = 7;

    /// The ticks a replay is compared against the original at.
    ///
    /// The point of the intermediate ones: a replay that only agrees at the end
    /// cannot localise a divergence, and localising one is the whole reason a
    /// repro file is worth having.
    const CHECKPOINTS: [u64; 5] = [1, 100, 1_000, 5_000, 10_000];

    /// The modes whose world is built in code, which is every mode `live` can
    /// drive without a file.
    ///
    /// Not `Mode::ALL` any more, and the difference is the point: `scene-file`
    /// is constituted from a file, so a loop that hands `live` a mode cannot
    /// include it. Its own coverage is the three tests at the end of this
    /// module, which give it a scene.
    const CODE_BUILT: [Mode; 4] = [Mode::Motion, Mode::Chance, Mode::Input, Mode::Scene];

    /// A scene with something in it that moves, for the `scene-file` tests.
    ///
    /// A follow converging on a target and a shake decaying to rest: enough that
    /// a run of ten ticks differs from a run of none, which is what the tests
    /// below need in order to mean anything.
    const SCENE: &str = "Scene(entities: [
            (name: \"target\", components: {
                \"transform\": (x: 8.0, y: -3.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),
            }),
            (
                components: {
                    \"camera\": (x: 0.0, y: 0.0, zoom: 1.0),
                    \"follow\": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false),
                    \"shake\": (amplitude: 4.0, frequency: 1.0, decay: 0.5, phase: 0.0, cutoff: 0.001, base_x: 0.0, base_y: 0.0),
                },
                refs: { \"follow\": { \"target\": \"target\" } },
            ),
        ])
";

    /// A `scene-file` plan over [`SCENE`], with an anchor over that same text.
    fn scene_plan(ticks: u64) -> Plan {
        Plan::Live {
            mode: Mode::SceneFile,
            seed: SEED,
            ticks,
            scene: Some(LoadedScene {
                anchor: Anchor::from_parts(
                    "scenes/in-memory.ron".to_owned(),
                    narvo_core::sha256::hex(&narvo_core::sha256::sha256(SCENE.as_bytes())),
                ),
                text: SCENE.to_owned(),
            }),
        }
    }

    fn live(mode: Mode, ticks: u64) -> Run {
        run(Plan::Live {
            mode,
            seed: SEED,
            ticks,
            scene: None,
        })
        .expect("the demo simulations always run")
    }

    fn replay(recording: Recording, ticks: Option<u64>) -> Run {
        run(Plan::Replay {
            recording,
            scene: None,
            ticks,
        })
        .expect("a recording this build wrote replays")
    }

    /// The M5 closing criterion, in process: a real mapping file drives a real
    /// recording, and the recording replays.
    ///
    /// # Why in process, and why over the `input` demo
    ///
    /// Neither half can be expressed on a command line today, and both reasons
    /// are structural rather than missing wiring:
    ///
    /// - `--mapping` is refused beside every headless-implying flag, and
    ///   `--record` and `--replay` are two of those (`cli.rs`). A mapping and a
    ///   recording cannot appear in one invocation.
    /// - The window, which is where a mapping *can* be given, records nothing:
    ///   `Recording::new` has one non-test site and it is in this file.
    ///   ADR-0022's cut rule is decided and still has no site.
    ///
    /// So the chain is composed here. It runs over the `input` demo rather than
    /// over a scene, and that is a second structural fact rather than a
    /// convenience: `Mode::SceneFile::actions()` is empty, so `consumes_input`
    /// is false and `validate_recording` refuses a recording that carries any
    /// input for it — before tick 0. A scene-file run has neither an input
    /// producer nor a validator that expects one, which is consistent; building
    /// both is the window-recorder task ADR-0022's revision condition names.
    ///
    /// # What is proved
    ///
    /// Device events -> the mapping from `scenes/input_demo.mapping.ron` ->
    /// `InputEvent`s -> a recording rendered to text -> parsed back -> replayed.
    /// The dumps agree at every checkpoint, and a tampered file does not.
    #[test]
    fn a_mapping_file_drives_a_recording_that_replays() {
        let mapping = demo_mapping();
        let recording = recording_through(&mapping, 1_000);

        // The recording is at the action level and carries no device term.
        // D8's decision, checked against the bytes rather than trusted
        // (ADR-0012's M5.2 amendment).
        let text = recording.render();
        for device in ["KeyW", "KeyS", "KeyA", "KeyD", "Digit1", "Digit2", "Digit3"] {
            assert!(
                !text.contains(device),
                "the recording names the device term {device}:
{text}"
            );
        }
        assert!(
            text.contains("thrust"),
            "and it does name the actions:
{text}"
        );

        // Parsed back from its own text, the way a replay gets one.
        let parsed = Recording::parse(&text).expect("what was written reads back");
        assert_eq!(parsed.inputs(), recording.inputs());

        for ticks in [1, 100, 1_000] {
            let first = replay(parsed.clone(), Some(ticks));
            let second = replay(
                Recording::parse(&text).expect("what was written reads back"),
                Some(ticks),
            );

            assert_eq!(
                first.dump, second.dump,
                "two replays of one mapped recording diverged at tick {ticks}"
            );
        }
    }

    /// The red edge of the proof above: a recording that was edited does not
    /// reproduce.
    #[test]
    fn a_tampered_mapped_recording_diverges() {
        let mapping = demo_mapping();
        let text = recording_through(&mapping, 1_000).render();

        let original = replay(
            Recording::parse(&text).expect("what was written reads back"),
            Some(1_000),
        );

        // One magnitude, one digit. The smallest edit a file can carry.
        let tampered_text = {
            let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
            let body = lines
                .iter()
                .position(|line| line.starts_with(char::is_numeric))
                .expect("the recording has at least one input");
            let fields: Vec<&str> = lines[body].split_whitespace().collect();
            let value: i64 = fields[2].parse().expect("the value is a number");
            lines[body] = format!("{} {} {}", fields[0], fields[1], value + 1);
            lines.join(
                "
",
            )
        };

        let tampered = replay(
            Recording::parse(&tampered_text).expect("still a valid recording"),
            Some(1_000),
        );

        assert_ne!(
            original.dump, tampered.dump,
            "a tampered recording reproduced the original, so the hash sees nothing"
        );
    }

    /// The mapping this project ships for the `input` demo, from disk.
    ///
    /// One read and `from_str`, not `from_file`: the M5.2 survey recorded that
    /// function's own second read as a hazard, and the window path already
    /// declines to inherit it.
    fn demo_mapping() -> narvo_input::Mapping {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scenes")
            .join("input_demo.mapping.ron");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));

        narvo_input::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// A recording built by pushing a synthetic device stream through `mapping`.
    ///
    /// The stream is deliberately dull and deliberately not random: a fixed
    /// pattern of presses and releases, so the recording is the same on every
    /// machine and the proof is about the chain rather than about a generator.
    fn recording_through(mapping: &narvo_input::Mapping, ticks: u64) -> Recording {
        use narvo_input::{Control, DeviceEvent};

        let mut recording = Recording::new(Mode::Input, SEED, ticks);

        for tick in 0..ticks {
            // Something on roughly one tick in seven, which keeps the file
            // sparse the way ADR-0012 Decision 2 wants it.
            let stream = match tick % 7 {
                0 => vec![DeviceEvent::press(Control::KeyW)],
                2 => vec![DeviceEvent::release(Control::KeyW)],
                3 => vec![
                    DeviceEvent::press(Control::Digit2),
                    DeviceEvent::press(Control::KeyD),
                ],
                5 => vec![DeviceEvent::release(Control::KeyD)],
                _ => Vec::new(),
            };

            for event in mapping.map(&stream) {
                recording.push(tick, event);
            }
        }

        recording
    }

    #[test]
    fn a_replay_reproduces_the_original_at_every_checkpoint_and_at_the_end() {
        // The closing criterion of §6/M2, sharpened: not just the same final
        // state, the same state all the way along.
        let recorded = live(Mode::Input, 10_000);

        for ticks in CHECKPOINTS {
            let original = live(Mode::Input, ticks);
            let replayed = replay(recorded.recording.clone(), Some(ticks));

            assert_eq!(
                replayed.dump, original.dump,
                "the replay diverges from the original by tick {ticks}"
            );
            assert_eq!(state_hash(&replayed.dump), state_hash(&original.dump));
            assert_eq!(replayed.ticks, ticks);
        }
    }

    #[test]
    fn a_full_length_replay_needs_no_tick_count() {
        let recorded = live(Mode::Input, 10_000);
        let replayed = replay(recorded.recording.clone(), None);

        assert_eq!(replayed.dump, recorded.dump);
        assert_eq!(replayed.ticks, 10_000);
    }

    #[test]
    fn a_replay_produces_the_recording_it_was_given() {
        // If the two ever differed, the file would not be a faithful account of
        // what the run actually consumed.
        let recorded = live(Mode::Input, 2_000);
        let replayed = replay(recorded.recording.clone(), None);

        assert_eq!(replayed.recording, recorded.recording);
        assert_eq!(replayed.recording.render(), recorded.recording.render());
    }

    #[test]
    fn a_tampered_recording_produces_a_different_state() {
        // Without this every other test here is satisfied by a "replay" that
        // ignores the file and simply simulates again from the seed.
        let recorded = live(Mode::Input, 1_000);
        let text = recorded.recording.render();

        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let body = lines
            .iter()
            .position(|line| line.starts_with(char::is_numeric))
            .expect("a thousand ticks of this mode always record something");

        // One character of one value, in one line, of one tick.
        let fields: Vec<&str> = lines[body].split_whitespace().collect();
        let value: i64 = fields[2].parse().expect("the value is a number");
        lines[body] = format!("{} {} {}", fields[0], fields[1], value + 1);

        let tampered = Recording::parse(&format!("{}\n", lines.join("\n")))
            .expect("the tampered file is still well formed");
        assert_ne!(tampered, recorded.recording);

        let replayed = replay(tampered, None);

        assert_ne!(
            replayed.dump, recorded.dump,
            "changing one recorded value has to change the state"
        );
        assert_ne!(state_hash(&replayed.dump), state_hash(&recorded.dump));
    }

    #[test]
    fn dropping_one_recorded_input_changes_the_state() {
        let recorded = live(Mode::Input, 1_000);
        let text = recorded.recording.render();

        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let body = lines
            .iter()
            .position(|line| line.starts_with(char::is_numeric))
            .expect("a thousand ticks of this mode always record something");
        lines.remove(body);

        let shortened = Recording::parse(&format!("{}\n", lines.join("\n")))
            .expect("removing a line leaves a well formed file");
        let replayed = replay(shortened, None);

        assert_ne!(replayed.dump, recorded.dump);
    }

    #[test]
    fn a_run_with_no_input_records_a_valid_empty_recording_and_replays_it() {
        for mode in [Mode::Motion, Mode::Chance] {
            let recorded = live(mode, 1_000);
            assert!(
                recorded.recording.inputs().is_empty(),
                "mode {mode} should record no input"
            );

            // Through the text, not around it: an empty recording has to survive
            // being written and read like any other.
            let text = recorded.recording.render();
            let parsed = Recording::parse(&text).expect("an empty recording is still a recording");
            let replayed = replay(parsed, None);

            assert_eq!(replayed.dump, recorded.dump, "mode {mode} does not replay");
        }
    }

    #[test]
    fn a_replay_past_the_end_of_its_recording_is_refused() {
        let recorded = live(Mode::Input, 100);

        let error = run(Plan::Replay {
            recording: recorded.recording,
            ticks: Some(101),
            scene: None,
        })
        .expect_err("there is no recorded input past tick 99");

        assert!(matches!(
            error,
            RunError::TooShort {
                asked: 101,
                covered: 100
            }
        ));
        assert!(error.to_string().contains("100"));
    }

    #[test]
    fn a_recording_whose_mode_cannot_consume_it_is_refused() {
        // Hand-editing a recording's mode line is an easy mistake and a
        // catastrophic one: every input would be dropped and the replay would
        // look like a clean run of something else.
        let recorded = live(Mode::Input, 100);
        let text = recorded
            .recording
            .render()
            .replace("mode input", "mode motion");

        let recording = Recording::parse(&text).expect("still well formed");
        let error = run(Plan::Replay {
            recording,
            ticks: None,
            scene: None,
        })
        .expect_err("motion cannot consume input");

        assert!(matches!(error, RunError::Mismatch(_)));
        let message = error.to_string();
        assert!(message.contains("motion"), "{message}");
    }

    #[test]
    fn a_recording_naming_an_unknown_action_is_refused() {
        let text = "narvo-recording 1\nmode input\nseed 1\nticks 10\n3 teleport 1\nend\n";
        let recording = Recording::parse(text).expect("well formed, just not replayable");

        let error = run(Plan::Replay {
            recording,
            ticks: None,
            scene: None,
        })
        .expect_err("there is no teleport action");

        let message = error.to_string();
        assert!(message.contains("teleport"), "{message}");
        assert!(message.contains('3'), "the tick should be named: {message}");
    }

    #[test]
    fn the_recorded_mode_moves_only_because_input_arrives() {
        // If it moved on its own, a replay could agree with the original without
        // the recording having done anything at all.
        let with_input = live(Mode::Input, 1_000);
        let silent = replay(Recording::new(Mode::Input, SEED, 1_000), None);

        assert!(!with_input.recording.inputs().is_empty());
        assert_ne!(with_input.dump, silent.dump);
    }

    /// The M2.2b tests, still green with the runner rebuilt around a plan.
    #[test]
    fn two_runs_of_ten_thousand_ticks_produce_the_same_state() {
        for mode in CODE_BUILT {
            let first = live(mode, 10_000);
            let second = live(mode, 10_000);

            assert_eq!(first, second, "mode {mode} is not reproducible");
            assert_eq!(state_hash(&first.dump), state_hash(&second.dump));
        }
    }

    #[test]
    fn two_runs_with_different_seeds_produce_different_states() {
        for mode in [Mode::Chance, Mode::Input] {
            let first = run(Plan::Live {
                mode,
                seed: 1,
                ticks: 10_000,
                scene: None,
            })
            .expect("runs");
            let second = run(Plan::Live {
                mode,
                seed: 2,
                ticks: 10_000,
                scene: None,
            })
            .expect("runs");

            assert_ne!(first.dump, second.dump, "mode {mode} ignores its seed");
            assert_ne!(state_hash(&first.dump), state_hash(&second.dump));
        }
    }

    #[test]
    fn the_seed_does_not_reach_the_frozen_mode() {
        let first = run(Plan::Live {
            mode: Mode::Motion,
            seed: 1,
            ticks: 1_000,
            scene: None,
        })
        .expect("runs");
        let second = run(Plan::Live {
            mode: Mode::Motion,
            seed: 999_999,
            ticks: 1_000,
            scene: None,
        })
        .expect("runs");

        assert_eq!(first.dump, second.dump);
    }

    #[test]
    fn ten_thousand_ticks_actually_change_the_state() {
        for mode in CODE_BUILT {
            let start = live(mode, 0);
            let end = live(mode, 10_000);

            assert_ne!(start.dump, end.dump, "mode {mode} never changed");
            assert_ne!(state_hash(&start.dump), state_hash(&end.dump));
            assert_eq!(start.entities, end.entities);
        }
    }

    #[test]
    fn a_run_executes_exactly_the_ticks_it_was_asked_for() {
        for mode in CODE_BUILT {
            for ticks in [0, 1, 7, 100] {
                assert_eq!(live(mode, ticks).ticks, ticks);
            }
        }
    }

    #[test]
    fn more_ticks_keep_changing_the_state() {
        for mode in CODE_BUILT {
            assert_ne!(live(mode, 100).dump, live(mode, 101).dump, "mode {mode}");
        }
    }

    #[test]
    fn the_dump_has_no_carriage_returns_on_any_platform() {
        for mode in CODE_BUILT {
            let outcome = live(mode, 10);

            assert!(!outcome.dump.contains('\r'));
            assert!(outcome.dump.ends_with('\n'));
        }
    }

    // ---- the scene-file mode -------------------------------------------

    /// A world built from a file is reproducible, like every world built from
    /// code.
    ///
    /// The coverage `CODE_BUILT` had to give up, given back for the one mode it
    /// excluded — and it is the property the new determinism case rests on.
    #[test]
    fn two_scene_file_runs_produce_the_same_state() {
        let first = run(scene_plan(200)).expect("the scene loads");
        let second = run(scene_plan(200)).expect("the scene loads");

        assert_eq!(first.dump, second.dump);
        assert_eq!(state_hash(&first.dump), state_hash(&second.dump));
    }

    /// Ticks actually change a scene-built world.
    ///
    /// Without this the case could load a file, tick nothing, and hash the same
    /// on both platforms for the least interesting reason there is.
    #[test]
    fn ticks_change_a_scene_built_world() {
        let start = run(scene_plan(0)).expect("the scene loads");
        let end = run(scene_plan(10)).expect("the scene loads");

        assert_ne!(start.dump, end.dump, "the wired system never ran");
        assert_eq!(start.entities, end.entities, "no entity came or went");
    }

    /// A run constituted from a scene records the anchor it was given.
    #[test]
    fn a_scene_file_run_records_its_anchor() {
        let outcome = run(scene_plan(5)).expect("the scene loads");

        let anchor = outcome
            .recording
            .scene()
            .expect("a scene-file recording carries its anchor");
        assert_eq!(anchor.path(), "scenes/in-memory.ron");
        assert_eq!(
            anchor.digest(),
            narvo_core::sha256::hex(&narvo_core::sha256::sha256(SCENE.as_bytes()))
        );
    }

    /// The whole chain: record from a scene, write the file, read it back, and
    /// replay it against the same scene to the same state.
    ///
    /// This is what ADR-0019 exists to make possible, end to end and without a
    /// disk: the recording round-trips through its own text, the anchor survives
    /// it, and the replay reaches the state the original did.
    #[test]
    fn a_scene_file_recording_round_trips_and_replays_to_the_same_state() {
        let original = run(scene_plan(50)).expect("the scene loads");

        let text = original.recording.render();
        let parsed = Recording::parse(&text).expect("what this build wrote, it reads");
        assert_eq!(parsed, original.recording);

        let scene = parsed.scene().expect("the anchor survived the round trip");
        let replayed = run(Plan::Replay {
            scene: Some(LoadedScene {
                anchor: scene.clone(),
                text: SCENE.to_owned(),
            }),
            recording: parsed,
            ticks: None,
        })
        .expect("a replay against the same scene");

        assert_eq!(original.dump, replayed.dump);
        assert_eq!(state_hash(&original.dump), state_hash(&replayed.dump));
    }

    /// A scene-file plan without a scene is refused rather than built wrongly.
    #[test]
    fn a_scene_file_run_without_a_scene_is_refused() {
        let error = run(Plan::Live {
            mode: Mode::SceneFile,
            seed: SEED,
            ticks: 1,
            scene: None,
        })
        .expect_err("there is nothing to build a world from");

        assert!(matches!(error, RunError::SceneMissing));
        assert!(error.to_string().contains("--scene"), "{error}");
    }

    /// A scene that does not load stops the run, with the fault located.
    #[test]
    fn a_scene_that_does_not_load_stops_the_run() {
        let broken = "Scene(entities: [(components: { \"nope\": (x: 1.0) })])";
        let error = run(Plan::Live {
            mode: Mode::SceneFile,
            seed: SEED,
            ticks: 1,
            scene: Some(LoadedScene {
                anchor: Anchor::from_parts("scenes/broken.ron".to_owned(), "0".repeat(64)),
                text: broken.to_owned(),
            }),
        })
        .expect_err("`nope` is not a component");

        assert!(matches!(error, RunError::Scene(_)));
        assert!(error.to_string().contains("\"nope\""), "{error}");
    }

    // ---- the agent seam (M6.3a) ----------------------------------------

    use crate::ipc::Inbox;
    use narvo_ipc::{Request, Response};

    /// The block a canonical dump writes for one entity, without its header.
    ///
    /// A five-line copy of the helper `ipc`'s own tests use, for the reason
    /// `tests/determinism.rs` keeps one: a `#[cfg(test)] mod` is not reachable
    /// from another module, and widening either one to share it would put a test
    /// helper in the production namespace.
    fn block_of(dump: &str, entity: &str) -> Vec<String> {
        dump.lines()
            .skip_while(|line| *line != format!("entity {entity}"))
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .map(str::to_owned)
            .collect()
    }

    /// An entity answer, rendered as the dump lines it corresponds to.
    fn answer_lines(response: &Response) -> Vec<String> {
        match response {
            Response::GetEntity { components, .. } => components
                .iter()
                .map(|component| format!("  {} {}", component.name, component.value))
                .collect(),
            other => panic!("expected the entity answer, got {other:?}"),
        }
    }

    /// **The observation point, pinned in all three directions it could sit in.**
    ///
    /// One request, waiting before the run starts, over five ticks of a
    /// simulation whose state moves every tick. The answer has to be the world as
    /// tick 0 left it — not the world tick 0 started from, which is where a drain
    /// placed before the systems would read, and not the final state, which is
    /// where a drain outside the loop would read. Both wrong answers are asserted
    /// against rather than described, because `motion` moves by a constant every
    /// tick and all three states are distinct.
    #[test]
    fn a_request_is_answered_against_the_state_the_tick_left_behind() {
        let probe = crate::sim::build(Mode::Motion, SEED).expect("the demo simulations build");
        let first = probe.world.entity_ids()[0];
        let spelled = format!("{}v{}", first.index(), first.generation());

        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push(Request::GetEntity {
            entity: spelled
                .parse()
                .expect("this test writes a well-formed name"),
        });

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 5,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        let answers = answers.answers;
        assert_eq!(answers.len(), 1);
        let observed = answer_lines(&answers[0]);

        assert_eq!(
            observed,
            block_of(&live(Mode::Motion, 1).dump, &spelled),
            "the answer is not the state the first tick left behind"
        );
        assert_ne!(
            observed,
            block_of(&live(Mode::Motion, 0).dump, &spelled),
            "the answer is the world as tick 0 found it, so the drain sits before the \
             systems rather than after them"
        );
        assert_ne!(
            observed,
            block_of(&outcome.dump, &spelled),
            "the answer is the run's final state, so the drain is not at a tick boundary \
             at all"
        );
    }

    /// A request waiting at the start is answered once, whatever follows.
    ///
    /// **What this instrument reaches.** Three ticks rather than two, though two
    /// would do here: a drain that answered without emptying the queue produces
    /// one answer per tick, which is linear rather than periodic, so it is already
    /// visible at the second. The third is there because the M5.3 precedent is
    /// that a two-step test can be green about a recurrence a three-step test
    /// catches, and the cost of the extra tick is nothing.
    ///
    /// **What it cannot reach**: a drain that runs *twice* in one tick. The second
    /// pass finds an empty queue, so it is unobservable by construction — a read
    /// seam is idempotent under a repeated drain precisely because draining
    /// empties, which is the property this test pins. There is no test that would
    /// catch it and this note is what stands in for one.
    #[test]
    fn a_request_is_answered_once_however_many_ticks_follow() {
        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push(Request::ListEntities);

        run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 3,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        assert_eq!(
            answers.answers.len(),
            1,
            "one request produced more than one answer, so the queue is read rather than \
             drained"
        );
    }

    /// **A run nobody is asking about is the run that was there before.**
    ///
    /// The regression criterion `ProjektPlan.md` §6/M6 names, in process and per
    /// mode: same ticks, same entities, same dump, same recording. The
    /// cross-platform half is the determinism suite, which drives the built binary
    /// and therefore takes this path with an empty inbox on every case it records.
    #[test]
    fn an_empty_inbox_leaves_a_run_byte_identical() {
        for mode in CODE_BUILT {
            let plain = live(mode, 500);

            let mut inbox = Inbox::new();
            let mut answers = Collected::default();
            let watched = run_with(
                Plan::Live {
                    mode,
                    seed: SEED,
                    ticks: 500,
                    scene: None,
                },
                &mut inbox,
                &mut answers,
            )
            .expect("the demo simulations always run");

            assert_eq!(plain, watched, "mode {mode} moved because an inbox exists");
            assert!(
                answers.answers.is_empty(),
                "an empty inbox produced an answer for mode {mode}"
            );
        }
    }

    // ---- the write and D19's band cut (M6.3b) --------------------------

    /// A write of a plainly out-of-trajectory position onto the first entity.
    fn set_position() -> Request {
        Request::SetComponent {
            entity: "0v1".parse().expect("this test writes a well-formed name"),
            component: "position".to_owned(),
            value: "(x:9999,y:-9999)".to_owned(),
        }
    }

    /// **The proof that the band carries.**
    ///
    /// The reading of "the state at the cut tick", fixed before the measurement
    /// and not after it: the seam runs *after* tick *N*'s systems, so the state a
    /// write first alters is the state tick *N* left behind. The band is
    /// therefore cut to `N + 1` ticks, and a replay of it runs ticks 0 through
    /// *N* and reaches exactly that state — the world as it stood **immediately
    /// before** the first accepted write.
    ///
    /// **N is 0 here and can only be 0**, and that is a limit of M6.3a's cut
    /// rather than of this one: nothing can enqueue a request while a run is in
    /// flight until a socket can (M6.3c), so every request waiting at the start
    /// is answered at the first boundary. The cut at an arbitrary tick is covered
    /// at the seam level in `crate::ipc`, where the tick is a parameter.
    ///
    /// **It runs on `motion` rather than on `input`, and that was measured rather
    /// than chosen.** The first version of this test used `input` and its
    /// non-vacuity assertion failed: a one-tick `input` run reaches the same dump
    /// as a zero-tick one, because that world moves only when input arrives and
    /// the pilot draws none for tick 0. A fixture whose first ticks do not move
    /// would satisfy the comparison below with the cut in the wrong place, so the
    /// requirement is asserted here instead of assumed.
    #[test]
    fn the_cut_band_replays_to_the_state_the_write_landed_on() {
        // What this fixture has to provide, stated as a check rather than as a
        // belief: consecutive early ticks reach different states, so "replays to
        // tick 1" is a claim only one cut can satisfy.
        assert_ne!(live(Mode::Motion, 0).dump, live(Mode::Motion, 1).dump);
        assert_ne!(live(Mode::Motion, 1).dump, live(Mode::Motion, 2).dump);

        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push(set_position());

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 500,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        assert!(
            matches!(answers.answers.as_slice(), [Response::SetComponent { .. }]),
            "the write was not accepted, so there is nothing to prove"
        );

        // The run went on and only the band stopped, which is this task's reading
        // of D19: it cuts the band and says nothing about the run.
        assert_eq!(outcome.ticks, 500);
        assert_eq!(
            outcome.recording.ticks(),
            1,
            "the band was not cut to the tick the write landed on"
        );

        // Through the text, the way a replay gets one: a cut band has to survive
        // being written and read like any other recording.
        let text = outcome.recording.render();
        let parsed = Recording::parse(&text).expect("a cut band is still a recording");
        let replayed = replay(parsed, None);

        assert_eq!(
            replayed.dump,
            live(Mode::Motion, 1).dump,
            "the cut band does not reproduce the state the write landed on"
        );
        assert_eq!(replayed.ticks, 1);

        // Neither half of that is vacuous. The state at the cut is neither the
        // state the run started from nor the one a cut a tick later would name,
        // and the write really did move the run off the trajectory it would have
        // taken without one.
        assert_ne!(replayed.dump, live(Mode::Motion, 0).dump);
        assert_ne!(replayed.dump, live(Mode::Motion, 2).dump);
        assert_ne!(
            outcome.dump,
            live(Mode::Motion, 500).dump,
            "the run reached the state it would have reached without the write, so \
             nothing was written"
        );
    }

    /// After the cut the band takes no more input, while the world still does.
    #[test]
    fn a_cut_band_records_nothing_after_the_cut() {
        let whole = live(Mode::Input, 500);
        assert!(
            !whole.recording.inputs().is_empty(),
            "this demo is supposed to record input"
        );

        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push(set_position());
        let cut = run_with(
            Plan::Live {
                mode: Mode::Input,
                seed: SEED,
                ticks: 500,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        assert!(
            cut.recording.inputs().iter().all(|input| input.tick < 1),
            "the band kept recording past the cut: {:?}",
            cut.recording.inputs()
        );
        assert!(
            cut.recording.inputs().len() < whole.recording.inputs().len(),
            "the cut band holds as much input as the uncut one"
        );

        // And the input still reached the world: the simulation is driven only by
        // input (`Mode::Input`), so a run that stopped consuming it would have
        // stopped moving.
        assert_ne!(
            cut.dump,
            live(Mode::Input, 1).dump,
            "the run stopped at the cut"
        );
    }

    /// **A refused write leaves a run byte-identical.**
    ///
    /// The regression criterion of §6/M6 extended to the write half: a request
    /// that is turned away must not be distinguishable from one that was never
    /// made, in the state, in the band, or in the tick count.
    #[test]
    fn a_refused_write_leaves_a_run_byte_identical() {
        let plain = live(Mode::Input, 500);

        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push(Request::SetComponent {
            entity: "4000000v1"
                .parse()
                .expect("well formed, and addresses nothing"),
            component: "position".to_owned(),
            value: "(x:1,y:1)".to_owned(),
        });
        let watched = run_with(
            Plan::Live {
                mode: Mode::Input,
                seed: SEED,
                ticks: 500,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        assert_eq!(plain, watched, "a refused write moved the run");
        assert!(matches!(
            answers.answers.as_slice(),
            [Response::Error { .. }]
        ));
    }

    // ---- run control, and the cut at a chosen tick (M6.3c) -------------

    /// The tick the M6.3c proofs cut at. Any tick well inside the run would do;
    /// what matters is that it is not zero, which is what M6.3b could not reach.
    const CUT_TICK: u64 = 43;

    /// **The inherited duty, discharged: a cut at a tick the test chose.**
    ///
    /// Everything `the_cut_band_replays_to_the_state_the_write_landed_on` proves
    /// at tick 0, at tick 43. The reading is M6.3b's unchanged — the seam runs
    /// after tick *N*'s systems, so the band is cut to `N + 1` and a replay of it
    /// reaches the world as it stood immediately before the write.
    ///
    /// **The last assertion is the one that makes the other three mean anything.**
    /// If the instrument delivered at tick 0 instead of at 43, the band would be
    /// cut to one tick and the replay would reach tick 1 — so this test would
    /// still be *a* test, of the wrong thing. `assert_ne!` against the one-tick
    /// state is what refuses that, and `undelivered` is what refuses the case
    /// where the request never arrived at all.
    #[test]
    fn a_cut_at_a_chosen_tick_replays_to_the_state_the_write_landed_on() {
        // What this fixture has to provide, checked rather than believed: the
        // states on either side of the cut differ, so only one cut satisfies the
        // comparison below.
        assert_ne!(
            live(Mode::Motion, CUT_TICK).dump,
            live(Mode::Motion, CUT_TICK + 1).dump
        );
        assert_ne!(
            live(Mode::Motion, CUT_TICK + 1).dump,
            live(Mode::Motion, CUT_TICK + 2).dump
        );

        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push_at(CUT_TICK, set_position());

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 500,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        assert_eq!(inbox.undelivered(), 0, "the request never came due");
        assert!(
            matches!(answers.answers.as_slice(), [Response::SetComponent { .. }]),
            "the write was not accepted, so there is nothing to prove"
        );

        assert_eq!(outcome.ticks, 500, "the run stopped at the cut");
        assert_eq!(outcome.recording.ticks(), CUT_TICK + 1);

        let text = outcome.recording.render();
        let parsed = Recording::parse(&text).expect("a cut band is still a recording");
        let replayed = replay(parsed, None);

        assert_eq!(
            replayed.dump,
            live(Mode::Motion, CUT_TICK + 1).dump,
            "the cut band does not reproduce the state the write landed on"
        );
        assert_eq!(replayed.ticks, CUT_TICK + 1);
        assert_ne!(replayed.dump, live(Mode::Motion, CUT_TICK).dump);
        assert_ne!(replayed.dump, live(Mode::Motion, CUT_TICK + 2).dump);
        assert_ne!(
            replayed.dump,
            live(Mode::Motion, 1).dump,
            "the cut is at tick 1, so the request was delivered at tick 0 and every \
             assertion above is about the wrong tick"
        );
    }

    /// A band cut at tick 43 keeps the input from before it and none after.
    ///
    /// The half M6.3b could only state: at tick 0 a band has almost nothing in it,
    /// so "keeps what came before the cut" was not observable. Here it is —
    /// `input` records on roughly one tick in seven, so a cut at 43 leaves real
    /// content behind it, and the count refuses a delivery at tick 0 as surely as
    /// the state comparison above does.
    #[test]
    fn a_band_cut_at_a_chosen_tick_keeps_what_came_before_it() {
        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push_at(CUT_TICK, set_position());

        let cut = run_with(
            Plan::Live {
                mode: Mode::Input,
                seed: SEED,
                ticks: 500,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        assert_eq!(inbox.undelivered(), 0);
        assert_eq!(cut.recording.ticks(), CUT_TICK + 1);
        assert!(
            !cut.recording.inputs().is_empty(),
            "a cut at tick {CUT_TICK} is supposed to keep the input before it"
        );
        assert!(
            cut.recording
                .inputs()
                .iter()
                .all(|input| input.tick <= CUT_TICK),
            "the band kept recording past the cut: {:?}",
            cut.recording.inputs()
        );

        // The uncut run of the same plan records more, so the cut removed
        // something rather than the mode simply being quiet.
        assert!(cut.recording.inputs().len() < live(Mode::Input, 500).recording.inputs().len());
    }

    /// **A run that goes further than it was told to, which `--ticks` cannot do.**
    ///
    /// The content this half of M6.3c has beyond a tick budget: the run's length
    /// stops being settled before tick 0. A four-tick run takes a grant of five
    /// at tick 3 and runs nine — and the last two assertions are what make it a
    /// *nine-tick run* rather than a four-tick run with a bigger number attached:
    /// its final state is the state nine ticks reach, and its band replays to it.
    #[test]
    fn a_step_extends_a_run_past_the_budget_it_started_with() {
        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push_at(3, Request::Step { ticks: 5 });

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 4,
                scene: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("the demo simulations always run");

        assert_eq!(inbox.undelivered(), 0);
        assert!(matches!(
            answers.answers.as_slice(),
            [Response::Step { granted: 9, .. }]
        ));

        assert_eq!(outcome.ticks, 9, "four ticks granted five more");
        assert_eq!(
            outcome.recording.ticks(),
            9,
            "the band did not follow the run"
        );
        assert_eq!(
            outcome.dump,
            live(Mode::Motion, 9).dump,
            "the extended run did not reach the state nine ticks reach"
        );
        assert_ne!(outcome.dump, live(Mode::Motion, 4).dump);

        // The band describes the run that happened: replaying it reaches the same
        // state, which a band still claiming four ticks could not do.
        let parsed =
            Recording::parse(&outcome.recording.render()).expect("an extended band is a recording");
        assert_eq!(replay(parsed, None).dump, outcome.dump);
    }

    // ---- the wait (M6.3d) ----------------------------------------------

    /// **A run that has used its budget waits, and a `step` releases it.**
    ///
    /// The behaviour this task exists for, proved without a socket: the channel
    /// hands over one command per wait, so which tick anything lands on is the
    /// test's business rather than the wall clock's.
    ///
    /// Four ticks, then a wait, then a grant of five, then a wait that finds an
    /// empty script and ends the run at nine. If the wait did not happen the run
    /// would stop at four, and the grant would never be asked for.
    #[test]
    fn a_run_that_has_used_its_budget_waits_and_a_step_releases_it() {
        let mut inbox = Inbox::new();
        let mut client = Collected::speaking([Request::Step { ticks: 5 }]);

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 4,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("the demo simulations always run");

        assert!(
            matches!(
                client.answers.as_slice(),
                [Response::Step { granted: 9, .. }]
            ),
            "the wait did not answer the step: {:?}",
            client.answers
        );
        assert_eq!(outcome.ticks, 9, "the run did not resume after the wait");
        assert_eq!(
            outcome.dump,
            live(Mode::Motion, 9).dump,
            "the resumed run is not a nine-tick run"
        );
    }

    /// **A run nobody is attached to does not wait**, which is the whole of why
    /// the transport can be compiled in without changing anything.
    ///
    /// The failure this guards is a hang, so it is worth saying what "green"
    /// means here: the test returning at all is the assertion. A wait that did
    /// not check `Channel::attached` first would never come back.
    #[test]
    fn a_run_with_nobody_attached_ends_where_its_budget_does() {
        for mode in CODE_BUILT {
            let plain = live(mode, 200);

            let mut inbox = Inbox::new();
            let mut nobody = Collected::default();
            let watched = run_with(
                Plan::Live {
                    mode,
                    seed: SEED,
                    ticks: 200,
                    scene: None,
                },
                &mut inbox,
                &mut nobody,
            )
            .expect("the demo simulations always run");

            assert_eq!(plain, watched, "mode {mode} moved because a channel exists");
            assert!(nobody.answers.is_empty());
        }
    }

    /// A client that stops talking ends the run rather than extending it.
    #[test]
    fn a_wait_that_runs_out_of_client_ends_the_run() {
        let mut inbox = Inbox::new();
        // One grant, then silence. The run takes the grant and then finds nobody.
        let mut client = Collected::speaking([Request::Step { ticks: 3 }]);

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 2,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("runs");

        assert_eq!(outcome.ticks, 5, "two granted three");
    }

    /// **A write accepted while the run is waiting cuts the band at the ticks
    /// that ran** — the case `cut_after`'s tick form could not express.
    ///
    /// The band covers exactly the four ticks that happened, and a replay of it
    /// reaches the state the write landed on. That state is the four-tick state,
    /// because no tick runs while a run waits.
    #[test]
    fn a_write_during_a_wait_cuts_the_band_at_the_ticks_that_ran() {
        let mut inbox = Inbox::new();
        let mut client = Collected::speaking([set_position(), Request::Step { ticks: 3 }]);

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 4,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("runs");

        assert!(
            matches!(
                client.answers.as_slice(),
                [Response::SetComponent { .. }, Response::Step { .. }]
            ),
            "the wait did not answer both: {:?}",
            client.answers
        );
        assert_eq!(outcome.ticks, 7, "the run resumed after the write");
        assert_eq!(
            outcome.recording.ticks(),
            4,
            "the band was not cut at the ticks that had run"
        );

        let parsed =
            Recording::parse(&outcome.recording.render()).expect("a cut band is a recording");
        assert_eq!(replay(parsed, None).dump, live(Mode::Motion, 4).dump);
    }

    /// **A write during a wait on a run that has not ticked cuts to zero.**
    ///
    /// The case that has no tick to name at all, and the reason `Recording::cut_to`
    /// exists: `cut_after`'s smallest answer is one, and one tick did not happen.
    #[test]
    fn a_write_while_waiting_before_any_tick_cuts_the_band_to_nothing() {
        let mut inbox = Inbox::new();
        let mut client = Collected::speaking([set_position(), Request::Step { ticks: 2 }]);

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 0,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("runs");

        assert_eq!(outcome.ticks, 2);
        assert_eq!(
            outcome.recording.ticks(),
            0,
            "a band for a run that had not ticked when the write landed covers no ticks"
        );

        // And it is still a recording: it renders, parses, and replays to nothing.
        let parsed = Recording::parse(&outcome.recording.render()).expect("still a recording");
        assert_eq!(parsed.ticks(), 0);
        assert_eq!(replay(parsed, None).dump, live(Mode::Motion, 0).dump);
    }

    /// **The two answering moments observe the same world — as a test, not as a
    /// sentence.**
    ///
    /// M6.3d's wait created a second place a request can be answered
    /// (`Moment::Waiting`) beside the one M6.3a chose (`Moment::Tick`), and
    /// ADR-0011's Context is explicit about what two behaviours on one channel
    /// cost: "a bug nobody can find afterwards". Every report since the first
    /// checkpoint has answered that with a claim — *no tick runs between them, so
    /// they see the same world* — and a claim is exactly what that warning is
    /// about.
    ///
    /// So it is measured. The **same** request is answered twice over: once at
    /// the last tick's drain, once at the wait that follows it, in two runs that
    /// are otherwise identical. `Moment::Tick(4).ticks_run()` and
    /// `Moment::Waiting { ticks_run: 5 }` are the same instant by construction,
    /// and the two answers have to be the same bytes. If a tick ever ran between
    /// them, or the wait's drain moved, this is what goes red.
    ///
    /// The third assertion is what stops it being vacuous: the answer is **not**
    /// the four-tick world, so the comparison is between two real observations of
    /// a moving simulation rather than between two empty ones.
    #[test]
    fn the_wait_answers_against_the_same_world_the_last_tick_did() {
        let probe = crate::sim::build(Mode::Motion, SEED).expect("the demo simulations build");
        let first = probe.world.entity_ids()[0];
        let named: narvo_ipc::EntityName = format!("{}v{}", first.index(), first.generation())
            .parse()
            .expect("this test writes a well-formed name");
        let asked = Request::GetEntity { entity: named };

        // Answered inside tick 4, which is the fifth tick: ticks_run == 5.
        let mut in_tick = Inbox::new();
        in_tick.push_at(4, asked.clone());
        let mut from_tick = Collected::default();
        run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 5,
                scene: None,
            },
            &mut in_tick,
            &mut from_tick,
        )
        .expect("runs");

        // Answered at the wait that follows the fifth tick: ticks_run == 5.
        let mut from_wait = Collected::speaking([asked]);
        run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 5,
                scene: None,
            },
            &mut Inbox::new(),
            &mut from_wait,
        )
        .expect("runs");

        assert_eq!(from_tick.answers.len(), 1);
        assert_eq!(from_wait.answers.len(), 1);
        assert_eq!(
            from_tick.answers[0].to_json(),
            from_wait.answers[0].to_json(),
            "the wait and the tick answered the same question about the same \
             instant and disagreed, which is the two-behaviours failure ADR-0011 \
             names"
        );

        // And it is an observation of a world that moves, not of an empty one.
        let mut earlier = Inbox::new();
        earlier.push_at(3, Request::GetEntity { entity: named });
        let mut from_earlier = Collected::default();
        run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 5,
                scene: None,
            },
            &mut earlier,
            &mut from_earlier,
        )
        .expect("runs");
        assert_ne!(
            from_tick.answers[0].to_json(),
            from_earlier.answers[0].to_json(),
            "tick 3 and tick 4 look the same, so this fixture cannot tell two \
             instants apart and the comparison above proves nothing"
        );
    }

    /// **How long a run waited is visible in nothing it produces.**
    ///
    /// S4's property, stated in M6.3d's first checkpoint report before anything
    /// was measured and unchanged since:
    ///
    /// > How long a run waited is visible nowhere in what it produces. The
    /// > canonical dump, the state hash and the recording are functions of the
    /// > ticks that ran and the requests that were answered, and of nothing else.
    /// > Two runs that wait for different lengths of wall-clock time, answering
    /// > the same requests, produce byte-identical artifacts.
    ///
    /// **This is where the deterministic fence meets the first non-deterministic
    /// surface in the project**, and until now it was a sentence. The two runs
    /// below answer the same script; one client takes a quarter of a second per
    /// wait and the other takes none. `Run` is compared whole — ticks, entity
    /// count, dump and recording — so any dependence on the clock anywhere in the
    /// wait would show here.
    ///
    /// The sleep is the point rather than an inconvenience: the property cannot
    /// be tested without making one run genuinely slower than the other, and
    /// there is no way to do that without spending the time.
    #[test]
    fn how_long_a_run_waited_is_visible_in_nothing_it_produces() {
        let script = [
            Request::Step { ticks: 3 },
            Request::ListEntities,
            Request::Step { ticks: 2 },
        ];

        let plan = || Plan::Live {
            mode: Mode::Motion,
            seed: SEED,
            ticks: 2,
            scene: None,
        };

        let mut brisk = Collected::speaking(script.clone());
        let quick = run_with(plan(), &mut Inbox::new(), &mut brisk).expect("runs");

        let mut slow = Collected::dawdling(script, Duration::from_millis(250));
        let patient = run_with(plan(), &mut Inbox::new(), &mut slow).expect("runs");

        assert_eq!(
            quick, patient,
            "a run that waited three quarters of a second longer produced a \
             different artifact, so the wall clock has reached inside the fence"
        );
        assert_eq!(
            brisk.answers.len(),
            slow.answers.len(),
            "the two clients did not ask the same things"
        );

        // Not vacuous: the waits really did happen and really did move the run.
        assert_eq!(
            quick.ticks, 7,
            "two ticks, then three granted, then two more"
        );
    }

    /// A step during a replay is refused, and the replay is untouched by it.
    #[test]
    fn a_step_during_a_replay_is_refused_and_changes_nothing() {
        let recorded = live(Mode::Input, 100);
        let plain = replay(recorded.recording.clone(), None);

        let mut inbox = Inbox::new();
        let mut answers = Collected::default();
        inbox.push_at(5, Request::Step { ticks: 50 });
        let steered = run_with(
            Plan::Replay {
                recording: recorded.recording.clone(),
                scene: None,
                ticks: None,
            },
            &mut inbox,
            &mut answers,
        )
        .expect("a recording this build wrote replays");

        assert_eq!(inbox.undelivered(), 0);
        match answers.answers.as_slice() {
            [Response::Error { message }] => assert!(
                message.starts_with("step is refused during a replay"),
                "{message}"
            ),
            other => panic!("expected one error answer, got {other:?}"),
        }

        assert_eq!(plain, steered, "a refused step moved the replay");
        assert_eq!(steered.ticks, 100, "the replay ran past its recording");
    }

    // ---- the two redirects (M6.4a) --------------------------------------

    /// A scratch file under the workspace's `target/`, named relative to the
    /// package root a test runs in.
    ///
    /// Relative because a scene path may not be absolute (`Anchor::read`), and
    /// under `target/` because the system temp directory is on another volume on
    /// this machine and therefore has no relative spelling at all. `crate::ipc`'s
    /// own tests carry the same helper and the same reason.
    fn scratch(name: &str, text: &str) -> String {
        std::fs::create_dir_all("../../target").expect("the target directory is writable");
        let path = format!("../../target/narvo-m64a-run-{}-{name}", std::process::id());
        std::fs::write(&path, text).expect("the target directory is writable");
        path
    }

    /// **A replay started over the seam reaches the state the command line's
    /// reaches.**
    ///
    /// The acceptance test of the second half of M6.4a, and the reason
    /// `crate::ipc::start_replay` goes through [`begin`] rather than assembling a
    /// run of its own: the two paths differ in nothing but where the path came
    /// from, so they have to agree byte for byte on the dump.
    ///
    /// **The frame accumulator is deliberately not reset by the redirect**, and
    /// this is what says that costs nothing: the run below has already fed four
    /// ragged frame durations into the timestep when the replay starts, and the
    /// one it is compared against starts from a fresh one. The systems never see
    /// a duration — only a tick number — so the tick sequence is 0, 1, 2, … in
    /// both and only their grouping into frames differs.
    #[test]
    fn a_replay_started_over_the_seam_reaches_the_command_lines_state() {
        let recorded = live(Mode::Input, 120);
        let path = scratch("seam.rec", &recorded.recording.render());
        let from_the_command_line = replay(recorded.recording.clone(), None);

        let mut inbox = Inbox::new();
        let mut client = Collected::default();
        inbox.push_at(4, Request::Replay { path: path.clone() });

        let redirected = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 50,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("the demo simulations always run");

        assert_eq!(inbox.undelivered(), 0, "the request never came due");
        match client.answers.as_slice() {
            [Response::Replay { mode, ticks, .. }] => {
                assert_eq!(mode, "input");
                assert_eq!(*ticks, 120);
            }
            other => panic!("expected one replay answer, got {other:?}"),
        }

        assert_eq!(
            redirected.dump, from_the_command_line.dump,
            "a replay started over the seam is not the replay --replay performs"
        );
        assert_eq!(
            redirected.ticks, 120,
            "the counter did not restart at the recording's own tick 0"
        );
        assert_ne!(
            redirected.dump,
            live(Mode::Motion, 50).dump,
            "the run reached the state it would have reached without the redirect"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// **The band the redirected run produces describes the ticks before the
    /// redirect, and stops there.**
    ///
    /// D19's guarantee applied to a command that is not a `set`: five ticks of
    /// `motion` really did run before the replay started, and a band of five
    /// `motion` ticks replays to exactly the state those five reached. What the
    /// band says nothing about is the replay that followed — which is right,
    /// because that run is the *other* recording's.
    #[test]
    fn the_band_of_a_redirected_run_is_the_prefix_it_can_still_reproduce() {
        let recorded = live(Mode::Input, 30);
        let path = scratch("prefix.rec", &recorded.recording.render());

        let mut inbox = Inbox::new();
        let mut client = Collected::default();
        inbox.push_at(4, Request::Replay { path: path.clone() });

        let redirected = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 50,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("the demo simulations always run");

        assert_eq!(
            redirected.recording.ticks(),
            5,
            "the band does not describe the five ticks that ran before the redirect"
        );
        assert_eq!(redirected.recording.mode(), Mode::Motion);

        let parsed =
            Recording::parse(&redirected.recording.render()).expect("a cut band is a recording");
        assert_eq!(
            replay(parsed, None).dump,
            live(Mode::Motion, 5).dump,
            "the cut band does not reproduce the prefix it claims"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// **A scene load leaves the run's tick counter where it is**, so a budget is
    /// spent once rather than once per load.
    ///
    /// The decision `crate::ipc::load_scene` records, measured at the only place
    /// it is observable: the run below is given fifty ticks and loads a scene
    /// three times, and it still ends at fifty. A counter that restarted the way
    /// the window's reload does would end at two hundred — and at four hundred
    /// for six loads, which is a client hanging a run by asking a reasonable
    /// question.
    #[test]
    fn a_scene_load_leaves_the_budget_where_it_was() {
        let path = scratch("load.ron", SCENE);

        let mut inbox = Inbox::new();
        let mut client = Collected::default();
        for tick in [3, 10, 20] {
            inbox.push_at(tick, Request::LoadScene { path: path.clone() });
        }

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 50,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("the demo simulations always run");

        assert_eq!(inbox.undelivered(), 0, "a request never came due");
        assert_eq!(client.answers.len(), 3);
        assert!(
            client
                .answers
                .iter()
                .all(|answer| matches!(answer, Response::LoadScene { .. })),
            "a load was refused: {:?}",
            client.answers
        );

        assert_eq!(
            outcome.ticks, 50,
            "three scene loads bought the run more ticks than it was given"
        );
        assert_eq!(
            outcome.recording.ticks(),
            4,
            "the band was not cut at the first load"
        );

        // The run really is the scene's now, not `motion`'s: a scene-file world
        // is what the last load left, so the dump is that and not the
        // fifty-tick motion state.
        assert_ne!(outcome.dump, live(Mode::Motion, 50).dump);
        assert!(outcome.dump.contains("camera"), "{}", outcome.dump);

        let _ = std::fs::remove_file(&path);
    }

    /// A run that loads a scene and is then asked to replay ends as the replay.
    ///
    /// The S4 pair that composes rather than colliding, driven through the real
    /// loop: the load leaves the run live, so the replay is taken; the band was
    /// cut by the load and stays where it was.
    #[test]
    fn a_load_then_a_replay_ends_as_the_replay_and_the_cut_does_not_move() {
        let recorded = live(Mode::Input, 40);
        let recording = scratch("both.rec", &recorded.recording.render());
        let scene = scratch("both.ron", SCENE);

        let mut inbox = Inbox::new();
        let mut client = Collected::default();
        inbox.push_at(
            2,
            Request::LoadScene {
                path: scene.clone(),
            },
        );
        inbox.push_at(
            6,
            Request::Replay {
                path: recording.clone(),
            },
        );

        let outcome = run_with(
            Plan::Live {
                mode: Mode::Motion,
                seed: SEED,
                ticks: 50,
                scene: None,
            },
            &mut inbox,
            &mut client,
        )
        .expect("the demo simulations always run");

        assert_eq!(inbox.undelivered(), 0);
        assert_eq!(outcome.ticks, 40, "the run is not the replay's length");
        assert_eq!(
            outcome.dump,
            replay(recorded.recording, None).dump,
            "the redirected run is not the replay it was asked for"
        );
        assert_eq!(
            outcome.recording.ticks(),
            3,
            "the replay moved the cut the load had already made"
        );

        let _ = std::fs::remove_file(&recording);
        let _ = std::fs::remove_file(&scene);
    }
}
