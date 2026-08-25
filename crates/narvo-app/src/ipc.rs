//! Where a request from `narvo-ipc` meets a running world, and when.
//!
//! `narvo-ipc` owns what a request *is* and refuses to know what answers one;
//! `narvo-ecs` owns the world and has never heard of a protocol. This module is
//! the map between them, and it lives here for the reason ADR-0015 gives for the
//! sprite extraction and ADR-0029 for the physics mapping: the crate that already
//! sees both sides is the one that maps between them. ADR-0030 supplies the other
//! half of that — `narvo-ipc` stays a leaf, because an edge from it to the ECS
//! "would put `hecs` underneath every future MCP client for the sake of two
//! `u32`" — so the mapping cannot live on either side of the boundary it maps.
//!
//! There is no socket, no thread and no clock here. Those are M6.3d — the cut
//! moved when M6.3c was halted at its first survey question, and the line now
//! runs between deterministic and blocking rather than between transport and
//! stepping.
//!
//! **There is a file system since M6.4a**, and it is worth saying where the line
//! moved to rather than leaving the sentence above to be read as covering it. Two
//! of the commands name a *path* — a scene to constitute a world from, a
//! recording to reproduce — so answering one means reading a file. The reads go
//! through the functions that already do them for the command line
//! (`Anchor::read`, `crate::load_recording`, `crate::scene_for`), and `headless`
//! keeps its own no-I/O property exactly: the runner still touches no file, and a
//! *request answered at its seam* may. What is still true without qualification
//! is that nothing here reads a clock or a source of entropy.
//!
//! # A replay answers questions and takes no orders
//!
//! The rule M6.4a arrived at, stated once here because it is now what decides
//! four of the five commands. A run reproducing a recording refuses `step`,
//! `set_component`, `load_scene` and `replay`, each with the same sentence and a
//! consequence of its own ([`refuse_while_replaying`]); the reads are untouched,
//! because looking at a replayed world changes nothing about what it reproduces.
//!
//! M6.3c wrote the first half of that for `step` alone, out of
//! `RunError::TooShort`'s own reasoning. **The `set` half closed a hole that was
//! open and reachable**: `--replay … --ipc …` has always been a legal command
//! line, and a write over that socket was accepted — measured before it was
//! refused, and what came out was a replay reporting a state hash that was not
//! the recorded run's while the recording it produced stayed byte-identical to
//! the one it had been given. The band cut cannot report that, because a band cut
//! to the length it already had is byte-indistinguishable from one never cut
//! (`Recording::cut_to`).
//!
//! # The write, and what it costs a recording
//!
//! Since M6.3b a request can also **set** a component, at the same boundary a
//! read is answered at and in the same drain. Two consequences, and neither is
//! this module's invention:
//!
//! - The registry's writing path ends in `World::insert`, so a set **adds** a
//!   component the entity does not carry. The answer says which happened —
//!   `previous: null` is exactly the added case — rather than leaving an agent to
//!   find out by asking again.
//! - A run that **accepts** a write has its recording cut at that tick (D19,
//!   ADR-0012's M6.2 amendment). "Accepts" is D19's own word and it is the
//!   trigger: a refused write leaves the world and the band alone, and a write
//!   that stores the value already there cuts the band like any other, because
//!   the band's guarantee is about what it can reproduce and not about what
//!   changed.
//!
//! # The moment: once per tick, against the state the tick left behind
//!
//! [`Inbox::answer_pending`] is called from `headless::run` after the systems and
//! before the tick counter moves — the same boundary `audio::cues_of` is read at,
//! deliberately, because a tick with two observation points in it is the failure
//! ADR-0011's Context describes: one channel with two behaviours.
//!
//! **Per tick rather than per frame**, and ADR-0003 is what decides it. An
//! advance runs 0 to 8 ticks (`FixedTimestep::DEFAULT_MAX_TICKS_PER_ADVANCE`) and
//! overspill is discarded rather than deferred, so a per-frame drain would answer
//! against whichever state the accumulator happened to stop at — the answer would
//! be a function of how fast the machine is. Per tick, it is a function of the
//! simulation alone: a request drawn in during tick *N* observes exactly the
//! world `canonical_dump` would print at that moment, which is what
//! `an_entity_answer_is_the_canonical_dumps_block_for_that_entity` holds.
//!
//! Since M6.3b that sentence has one qualification and it is not a loosening: a
//! drain answers its requests in arrival order, so a read behind a *write* in the
//! same drain sees what the write left. "The world at that moment" is still
//! exact; the moment simply advances within the drain. See
//! [`Inbox::answer_pending`].
//!
//! # The queue is outside the world, and that is ADR-0012's rule
//!
//! A pending request is not simulation state. ADR-0011's own test for what
//! belongs in the hash is "decides what happens next tick", and a read decides
//! nothing; ADR-0012 put the input *source* outside the world for the stronger
//! version of the same reason. Inside, an [`Inbox`] would have to be a registered
//! component, so an empty one would still be in every canonical dump and every
//! state hash would move — which is exactly the regression `ProjektPlan.md` §6/M6
//! forbids ("ein Lauf ohne verbundenen Klienten ist byte-identisch zum Stand vor
//! dem Transport").
//!
//! # The read path still cannot write, and that is still the compiler saying so
//!
//! [`read`] takes `&World` and [`write`] takes `&mut World`, which is why the
//! module gaining a write did not cost the read half its guarantee: for the
//! access [`read`] uses — `entity_ids` and the registry's type-erased
//! `serialize` — `&World` rules out mutation, because `World::get_mut` requires
//! `&mut World`. The claim is narrower than M6.3a's and is stated narrowly: the
//! *module* writes now, the *read path* still cannot.
//!
//! M6.4a widened what the module can do again — two commands replace the world
//! entirely — and did not widen that claim: both take a [`Stage`], and [`read`]
//! is still handed a `&World` taken out of one.
//!
//! It is not a general seal even there. ADR-0005 records that a `&mut T` query is
//! reachable through a shared `&World`, because hecs checks those borrows
//! dynamically; this module contains no query at all, which is what makes the
//! guarantee hold. `reading_a_world_never_changes_it` measures the same property
//! from the other side, and M6.3a measured exactly how far it reaches.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::num::NonZeroU32;
use std::path::Path;

use narvo_ecs::{ComponentRegistry, EcsError, EntityId, World};
use narvo_ipc::{ComponentValue, EntityName, Request, Response};

use crate::audio::CueMemory;
use crate::headless::{self, Begun, Plan, RunError, Source};
use crate::recording::Recording;
use crate::scene_anchor::{Anchor, AnchorError};
use crate::sim::scene_file::SceneStartError;
use crate::sim::{self, Mode, Simulation};

/// The requests waiting for a tick boundary, and the answers taken at one.
///
/// Held by the runner rather than by the world — see the module documentation
/// for why that is ADR-0012's rule rather than a convenience.
#[derive(Debug, Default)]
pub struct Inbox {
    /// Requests that have arrived and not yet been answered, in arrival order.
    pending: VecDeque<Request>,
    /// Answers produced at tick boundaries and not yet taken.
    answers: Vec<Response>,
    /// The tick the first write was accepted at, once one has been.
    ///
    /// The band is cut there and stays cut. Held here rather than in the
    /// recording because it is what the *runner* has to know — after the cut it
    /// stops recording input — and because a `Recording` that carried it would
    /// compare unequal to an identical one that had not been cut, which is a
    /// property `a_replay_produces_the_recording_it_was_given` rests on.
    cut_at: Option<u64>,
    /// Requests waiting for a tick that has not come yet.
    ///
    /// **A test instrument and nothing else**, which is why it is `cfg(test)`
    /// rather than a delivery schedule the protocol offers. Everything pushed
    /// with [`push`](Self::push) lands at the next drain, so before M6.3c every
    /// end-to-end proof in this crate happened at tick 0; a cut at an arbitrary
    /// tick needs a request that arrives *later*, and the thing that will really
    /// deliver one is a socket (M6.3d). Until then this stands in for it, and
    /// `the_instrument_delivers_at_the_tick_it_was_given` is what says it does —
    /// because an instrument that quietly delivered everything at tick 0 would
    /// make every "cut at N" proof here a tick-0 proof in costume.
    #[cfg(test)]
    scheduled: Vec<(u64, Request)>,
}

/// Everything a drain may reach: the world it answers against, and the run
/// around it.
///
/// Grouped into one type rather than passed as seven parameters, because the
/// grouping is the point — these are exactly the things a request can reach, and
/// the list has grown twice for a reason worth reading in order. M6.3c added
/// `budget` and `band`, the two things a `step` and a `set` move. **M6.4a added
/// the simulation itself**, because a scene load and a replay start replace it
/// whole, and with it `mode` and `source`, which are the other two halves of
/// "which run is this".
///
/// The world and its registry travel as one [`Simulation`] rather than as two
/// borrows, and that is not tidiness: a world swapped without its registry is a
/// world whose next `canonical_dump` fails, which is the failure
/// `sim::Simulation`'s own documentation says the type exists to make
/// unassemblable by accident.
pub struct RunControl<'a> {
    /// When this drain is happening.
    pub at: Moment,
    /// The world, the names its components are written under, and its systems.
    ///
    /// Replaced whole by a scene load and by a replay start; read, and at most
    /// written one component at a time, by everything else.
    pub simulation: &'a mut Simulation,
    /// Which simulation the run is driving, which decides what a tick's input
    /// means. A scene load makes it [`Mode::SceneFile`]; a replay takes the
    /// recording's own.
    pub mode: &'a mut Mode,
    /// Where each tick's input comes from — and therefore whether the run is
    /// reproducing a recording, which is what refuses every command that steers.
    pub source: &'a mut Source,
    /// The run's total tick budget. A `step` raises it, a replay replaces it,
    /// and nothing else moves it.
    pub budget: &'a mut u64,
    /// The recording being produced, which an accepted write cuts and a granted
    /// step lengthens.
    pub band: &'a mut Recording,
    /// The cue baseline the runner keeps beside the world.
    ///
    /// Reseeded whenever the world is replaced, for the reason
    /// [`CueMemory::new`](crate::audio::CueMemory::new) states and
    /// `SceneHost::replace_world` already acts on: a counter's *value* is state
    /// and its *movement* is the event, so a memory carried across a swap would
    /// read a scene authored with `count: 5` as five purchases arriving at once.
    pub cues: &'a mut CueMemory,
}

/// The parts of a run a command may replace, borrowed together.
///
/// The subset of [`RunControl`] that a redirect writes to. It exists so that
/// [`answer`] can be handed one value per request in a loop that is also
/// mutating the band, rather than five reborrows spelled out at every call.
struct Stage<'a> {
    /// When this drain is happening, so an answer can date itself.
    ///
    /// **Read, never chosen.** The moment is the runner's and it is established
    /// by [`Moment`] before a single request is looked at, so nothing here can
    /// answer for a moment other than the one it is in — which is exactly what
    /// ADR-0031's one-observation-point rule buys, spent here for the first time.
    at: Moment,
    simulation: &'a mut Simulation,
    mode: &'a mut Mode,
    source: &'a mut Source,
    budget: &'a mut u64,
    cues: &'a mut CueMemory,
}

impl Stage<'_> {
    /// Whether the run is reproducing a recording rather than making one.
    fn replaying(&self) -> bool {
        matches!(self.source, Source::Recorded { .. })
    }
}

/// What a drain leaves for the runner to do, beyond taking the answers.
///
/// A return value rather than a field on the run's state, because the one thing
/// in this class cannot be done by the drain at all: the tick counter is the
/// *loop's*, and the loop increments it one line after the drain. A handler that
/// set it to zero would have it incremented to one before the first tick of the
/// new run, which is how a recording's opening inputs would be looked for at a
/// tick already gone past — silently, because `Source::take` matches the tick
/// exactly and simply finds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum After {
    /// Nothing. Every drain that took no replay start.
    Carry,
    /// The run is a different run now, and its tick count begins again at zero.
    Restart,
}

/// When a drain is happening, of the two moments there are.
///
/// **There are two since M6.3d, and that is a finding rather than a design.**
/// M6.3c named the hinge to this task as two loop conditions and claimed nothing
/// else in `run_with` would have to move. It had to: a run that is *waiting* runs
/// no tick, so a request that arrives while it waits has no tick to be answered
/// in — and it must still be answered, because the command that ends the wait is
/// a `step`, and a `step` only raises the budget by being answered. A wait that
/// merely queued would block forever on the request that was meant to release it.
///
/// The two moments observe the **same world**: no tick runs between the last
/// in-tick drain and a wait, so nothing has changed. What differs is only how
/// many ticks have run, which is what a band cut needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moment {
    /// Inside tick `n`, after its systems and before the counter moves.
    Tick(u64),
    /// Between ticks, with the run waiting for a command. `ticks_run` have run.
    Waiting {
        /// How many ticks the run has executed.
        ticks_run: u64,
    },
}

impl Moment {
    /// How many ticks have run by this moment.
    ///
    /// Inside tick `n` that is `n + 1`, because the tick's systems have already
    /// run — which is the quantity `Recording::cut_after` has always computed.
    /// While waiting it is the count itself, and it may be zero, which is the
    /// case `cut_after` could not express.
    #[must_use]
    pub fn ticks_run(self) -> u64 {
        match self {
            Self::Tick(n) => n.saturating_add(1),
            Self::Waiting { ticks_run } => ticks_run,
        }
    }

    /// The tick this moment is inside, if it is inside one.
    #[cfg(test)]
    fn tick(self) -> Option<u64> {
        match self {
            Self::Tick(n) => Some(n),
            Self::Waiting { .. } => None,
        }
    }
}

/// Where requests come from and where answers go, between ticks.
///
/// **The seam a transport plugs into, and it knows no `World`.** That is the
/// whole of D20's reversibility clause: the shell below this trait turns bytes
/// into [`Request`]s and [`Response`]s into bytes, and nothing else, so replacing
/// localhost TCP with something else is replacing one implementation of four
/// methods.
///
/// Two implementations exist from the first commit — [`Silent`], which is what a
/// run without the feature uses, and the TCP endpoint in `crate::transport` — so
/// this is not a seam built for a caller that does not exist.
pub trait Channel {
    /// Requests that have arrived since the last call. **Never blocks.**
    fn arrived(&mut self) -> Vec<Request>;

    /// Blocks until a request arrives, or until nothing can arrive any more.
    ///
    /// `None` means the wait is over for good — nobody is connected and nobody
    /// is going to be — and the runner takes that as the end of the run.
    fn awaited(&mut self) -> Option<Request>;

    /// Whether waiting could produce anything at all.
    ///
    /// The runner asks this **before** it waits, and it is the whole of the
    /// answer to "does a run with the feature on but no client hang?". It does
    /// not: with nobody attached this is false and the run ends exactly as it
    /// did before the transport existed.
    fn attached(&self) -> bool;

    /// Hands answers back to whoever asked.
    fn answered(&mut self, answers: &[Response]);
}

/// The channel of a run that nobody is talking to.
///
/// Every method is the empty answer, and `attached` is false, so a run driven
/// through this is byte-for-byte the run that existed before M6.3d. It is what
/// [`crate::headless::run`] passes, and what every run in a build without the
/// `ipc` feature uses, because there is nothing else to pass.
#[derive(Debug, Default)]
pub struct Silent;

impl Channel for Silent {
    fn arrived(&mut self) -> Vec<Request> {
        Vec::new()
    }

    fn awaited(&mut self) -> Option<Request> {
        None
    }

    fn attached(&self) -> bool {
        false
    }

    fn answered(&mut self, _answers: &[Response]) {}
}

/// A channel that keeps every answer instead of sending it anywhere.
///
/// **Test-only, and it exists because M6.3d moved where an answer goes.** Before
/// this task an answer stayed in the [`Inbox`] until a test took it; now the
/// runner hands each drain's answers to [`Channel::answered`] and the inbox is
/// empty by the time a run returns. That is the production behaviour — an answer
/// that nobody collected would be an answer nobody gets — and it means a test
/// that wants to see one has to be the thing it was handed to.
/// It also **speaks**, which is what makes the wait testable without a socket.
/// A script of requests handed out one per [`awaited`](Channel::awaited) call is
/// a client that never races the tick loop, so a test can assert exactly which
/// commands a wait consumed and in which order. An empty script is a channel
/// nobody is attached to, and a run driven by that one is the run that existed
/// before this task.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct Collected {
    /// Every answer the run produced, in the order it produced them.
    pub answers: Vec<Response>,
    /// What a waiting run is handed, one per wait.
    script: VecDeque<Request>,
    /// How long this client takes to answer each wait.
    ///
    /// A slow client, so that a test can make one run wait far longer than
    /// another while both answer the same requests. That is the only variable
    /// S4's property is about, and it cannot be varied without a delay somewhere.
    delay: std::time::Duration,
}

#[cfg(test)]
impl Collected {
    /// A channel that will hand `script` to a waiting run, in order.
    pub fn speaking(script: impl IntoIterator<Item = Request>) -> Self {
        Self {
            answers: Vec::new(),
            script: script.into_iter().collect(),
            delay: std::time::Duration::ZERO,
        }
    }

    /// The same client, taking `delay` to say each thing.
    pub fn dawdling(script: impl IntoIterator<Item = Request>, delay: std::time::Duration) -> Self {
        Self {
            delay,
            ..Self::speaking(script)
        }
    }
}

#[cfg(test)]
impl Channel for Collected {
    fn arrived(&mut self) -> Vec<Request> {
        Vec::new()
    }

    fn awaited(&mut self) -> Option<Request> {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        self.script.pop_front()
    }

    /// Attached exactly while the script has something left.
    ///
    /// So a run waits for as long as this client has something to say and ends
    /// when it stops — which is what a real client disconnecting does, and what
    /// keeps a test from hanging if a wait is entered that nothing will leave.
    fn attached(&self) -> bool {
        !self.script.is_empty()
    }

    fn answered(&mut self, answers: &[Response]) {
        self.answers.extend_from_slice(answers);
    }
}

/// What answering a request did beyond producing an answer.
///
/// Decided in `answer`'s exhaustive match, so a command added later cannot be
/// classified by omission — the same reason that match has no `_` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// Nothing outside the answer. Every read.
    Nothing,
    /// The world was written to, which cuts the band (D19).
    ///
    /// **One component, or all of it.** M6.4a's scene load reaches the same arm
    /// as M6.3b's `set` and is the measurement S2 asked for: the cut mechanism
    /// generalised without a second one being written, because D19 was expressed
    /// as "a run that *accepts* a write" from the start rather than as
    /// "a `set_component` arrived".
    Wrote,
    /// The world was written to **and** the run is a different run now.
    ///
    /// A replay start, and the only effect that is not fully applied by the
    /// drain — see [`After`]. It cuts the band exactly as [`Wrote`](Self::Wrote)
    /// does; the second half is the tick count, which only the loop can move.
    Restarted,
    /// The run's budget was raised, which the band follows.
    Granted,
}

impl Inbox {
    /// An inbox with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts a request in the queue, to be answered at the next tick boundary.
    ///
    /// **The caller M6.3a predicted has arrived.** This carried
    /// `expect(dead_code)` with the reason "the seam precedes its first caller:
    /// nothing enqueues a request until a socket does", and the attribute was an
    /// `expect` rather than an `allow` precisely so that the day one appeared the
    /// expectation would go unfulfilled and the workspace's deny would force it
    /// out. That is what happened: `headless::run_with` pumps a [`Channel`] into
    /// here once a tick.
    pub fn push(&mut self, request: Request) {
        self.pending.push_back(request);
    }

    /// Answers everything waiting, against `world` as it stands at `tick`.
    ///
    /// **Exactly once per request**: the queue is emptied as it is walked, so a
    /// request that has been answered is gone whatever happens on the ticks after
    /// it. `a_request_is_answered_once_however_many_ticks_follow` holds that.
    ///
    /// **In arrival order, each against what the one before it left.** So a `get`
    /// behind a `set` in the same drain sees the value that `set` wrote. That is
    /// the only ordering a queue can have and it is worth stating, because it
    /// narrows M6.3a's "an answer is the world as the tick left it" to "…and as
    /// earlier requests in this drain left it".
    ///
    /// A request that cannot be answered produces [`Response::Error`] rather than
    /// stopping the run. A malformed name is the agent's mistake and the message
    /// is the whole of the feedback it gets; a simulation that stopped because
    /// somebody asked about an entity that had been despawned would be a worse
    /// answer to a smaller problem.
    ///
    /// **`band` is cut at the first accepted write** and not again. The
    /// guarantee D19 states is "reproducible up to the first write", so a second
    /// cut could only move the promise forwards past a write the band cannot
    /// account for. Since M6.4a a scene load and a replay start cut it too, for
    /// the identical reason and through the identical arm.
    ///
    /// **The return value is the one thing a drain cannot finish**, and
    /// [`After`] says why.
    pub fn answer_pending(&mut self, control: RunControl<'_>) -> After {
        let RunControl {
            at,
            simulation,
            mode,
            source,
            budget,
            band,
            cues,
        } = control;

        #[cfg(test)]
        if let Some(tick) = at.tick() {
            self.deliver_scheduled(tick);
        }

        let mut after = After::Carry;

        while let Some(request) = self.pending.pop_front() {
            let stage = Stage {
                at,
                simulation: &mut *simulation,
                mode: &mut *mode,
                source: &mut *source,
                budget: &mut *budget,
                cues: &mut *cues,
            };
            let (response, effect) = answer(&request, stage);
            self.answers.push(response);

            match effect {
                Effect::Nothing => {}
                Effect::Wrote | Effect::Restarted => {
                    if self.cut_at.is_none() {
                        self.cut_at = Some(at.ticks_run());
                        band.cut_to(at.ticks_run());
                    }
                    if effect == Effect::Restarted {
                        after = After::Restart;
                    }
                }
                // A cut band is not lengthened: it stopped describing this run at
                // the write, and how much further the run goes is no longer its
                // business.
                Effect::Granted => {
                    if self.cut_at.is_none() {
                        band.extend_to(*budget);
                    }
                }
            }
        }

        after
    }

    /// Enqueues `request` so the drain at `tick`, and no earlier one, answers it.
    ///
    /// See [`scheduled`](Self::scheduled)'s own documentation for why this is a
    /// test instrument rather than an offered capability.
    #[cfg(test)]
    pub fn push_at(&mut self, tick: u64, request: Request) {
        self.scheduled.push((tick, request));
    }

    /// Moves everything scheduled for `tick` into the queue, in schedule order.
    ///
    /// Exact equality on the tick, not "due by now". A schedule entry for a tick
    /// the run never reaches is therefore never delivered, and
    /// [`undelivered`](Self::undelivered) is what lets a test refuse to pass on
    /// a request that silently never arrived.
    #[cfg(test)]
    fn deliver_scheduled(&mut self, tick: u64) {
        let mut index = 0;
        while index < self.scheduled.len() {
            if self.scheduled[index].0 == tick {
                let (_, request) = self.scheduled.remove(index);
                self.pending.push_back(request);
            } else {
                index += 1;
            }
        }
    }

    /// How many scheduled requests never came due.
    #[cfg(test)]
    pub fn undelivered(&self) -> usize {
        self.scheduled.len()
    }

    /// Whether the band is still recording, which it is until a write lands.
    ///
    /// The runner asks before it appends: after the cut the band takes no more
    /// input, and `Recording::push` would refuse it anyway — its `tick <
    /// self.ticks` assertion is exactly the invariant the cut established, so a
    /// missing check here is a panic rather than a silently longer band.
    pub fn band_is_open(&self) -> bool {
        self.cut_at.is_none()
    }

    /// Takes the answers the ticks since the last call produced.
    ///
    /// Drained rather than borrowed, exactly as `SceneHost::take_cues` is and for
    /// the same reason: a caller cannot deliver the same answer twice. Its own
    /// `expect(dead_code)` is gone for the reason [`push`](Self::push) records —
    /// the runner hands what this returns to [`Channel::answered`].
    pub fn take_answers(&mut self) -> Vec<Response> {
        std::mem::take(&mut self.answers)
    }
}

/// Answers one request, and says what answering it did beyond that.
///
/// The second half of the return is what cuts or lengthens the band, and it is
/// decided in the exhaustive match below rather than by a second look at the
/// request — so a command added later cannot be classified by omission. The match
/// has no `_` arm for the same reason M6.1's own gates have none, and M6.3c
/// measured that the gate reaches it: adding one variant to `narvo-ipc` stopped
/// this function compiling.
fn answer(request: &Request, mut stage: Stage<'_>) -> (Response, Effect) {
    let (outcome, effect) = match request {
        // `Dump` joins the reads, and that is ADR-0032's criterion applied rather
        // than a new decision: "a run that is reproducing a recording answers
        // reads and refuses every command that would change what it reproduces".
        // A dump changes nothing, so it is answered during a replay like the
        // other three — and unlike them it is the one a replay is *for*.
        Request::ListEntities
        | Request::GetEntity { .. }
        | Request::GetComponent { .. }
        | Request::Dump => (
            read(
                request,
                &stage.simulation.world,
                &stage.simulation.registry,
                stage.at,
            ),
            Effect::Nothing,
        ),
        Request::SetComponent {
            entity,
            component,
            value,
        } => (set(*entity, component, value, &mut stage), Effect::Wrote),
        Request::Step { ticks } => (step(*ticks, &mut stage), Effect::Granted),
        Request::LoadScene { path } => (load_scene(path, &mut stage), Effect::Wrote),
        Request::Replay { path } => (start_replay(path, &mut stage), Effect::Restarted),
    };

    match outcome {
        Ok(response) => (response, effect),
        // A refused request is not an accepted one: the world is untouched and the
        // budget did not move, so neither the band's cut nor its length has
        // anything to follow. D19 cuts on acceptance, and a grant reads the same
        // way.
        Err(error) => (
            Response::Error {
                message: error.to_string(),
            },
            Effect::Nothing,
        ),
    }
}

/// Grants the run more ticks.
///
/// **Adds rather than sets**, so two commands of one tick each grant two ticks.
/// **Best rejected alternative: set the remaining budget.** Its strongest
/// argument is that setting is idempotent under retransmission, so a client that
/// resent a command it was unsure of could not overshoot. Against it: a stream
/// transport does not retransmit, and under "set" a client that sends `step 1`
/// twice gets one tick — which is not what stepping twice means.
///
/// **Refused during a replay**, and that is not a policy dressed as one:
/// `run_with` already refuses to be asked for more ticks than the recording
/// covers (`RunError::TooShort`), because past the end a replay runs on with no
/// input at all and reproduces nothing. A `step` that got through would do
/// exactly what that check exists to prevent, one grant at a time.
///
/// **Saturating**, so an enormous grant is a run that effectively never ends
/// rather than an overflow. That is left possible rather than capped: under
/// M6.3d's reading of an exhausted budget — the run waits — an unbounded grant is
/// "run freely", which is a sensible thing to ask for. It is named here because
/// it is also how a client hangs a headless run.
fn step(ticks: u64, stage: &mut Stage<'_>) -> Result<Response, RequestError> {
    refuse_while_replaying("step", STEP_STEERS, stage)?;

    *stage.budget = stage.budget.saturating_add(ticks);

    Ok(Response::Step {
        granted: *stage.budget,
        ticks_run: stage.at.ticks_run(),
    })
}

/// Constitutes the world afresh from a scene file (ADR-0022's reconstitution).
///
/// # It goes through the loader the other two callers use
///
/// `sim::scene_file::build`, the same function `headless::begin` calls for a
/// `--scene` run and `window.rs` calls for a hot reload. S1 asked whether the
/// existing reconstitution path carries a third caller before a second one is
/// built, and it does: everything specific to loading a scene — which components
/// are registered, which systems are wired, the input buffer that is appended
/// when the file names none — is in that function and none of it is repeated
/// here.
///
/// # What the run keeps, and what it does not
///
/// - **The tick counter runs on.** A load at tick 300 leaves the run at tick 300
///   with a new world. That is the opposite of the window's reload, which puts
///   its counter back to zero (`SceneHost::replace_world`), and the difference is
///   not a disagreement about meaning: here the counter is also the *budget's*
///   cursor, so resetting it would silently grant the run its whole `--ticks`
///   again — and again per load, which is a client hanging a run by asking a
///   reasonable question. The window's counter has no budget attached to it.
/// - **The band is cut** and never re-anchored. Cutting is D19 (see
///   [`Effect::Wrote`]); leaving `scene-sha256` alone is the other half of the
///   same honesty, because the band now describes only the ticks *before* the
///   load and those really were run against the file the anchor names. Rewriting
///   the anchor would make a faithful prefix claim a provenance it does not have.
/// - **The run's mode becomes [`Mode::SceneFile`]**, which is what the world now
///   is. Left alone it would be a lie with teeth: `sim::feed` matches on the mode
///   and `unreachable!`s for one that consumes no input, so a load into a
///   `--mode input` run would panic the moment the pilot drew its next event.
/// - **The cue baseline is reseeded**, exactly as the window's reload does it.
///
/// # Errors
///
/// [`RequestError::NotWhileReplaying`] during a replay,
/// [`RequestError::Scene`] if the file cannot be read or its path is absolute,
/// and [`RequestError::SceneStart`] if it is not a scene this build can load —
/// carrying the position M4.2 gives it.
fn load_scene(path: &str, stage: &mut Stage<'_>) -> Result<Response, RequestError> {
    refuse_while_replaying("load_scene", LOAD_SCENE_STEERS, stage)?;

    // One read, hashed from the same bytes the world is built from — the rule
    // ADR-0019 states and `Anchor::read` implements. Reading here rather than in
    // `headless` keeps that module's own no-I/O property intact.
    let (anchor, text) = Anchor::read(Path::new(path)).map_err(|source| RequestError::Scene {
        path: path.to_owned(),
        source,
    })?;

    let built = sim::scene_file::build(&text).map_err(|source| RequestError::SceneStart {
        path: anchor.path().to_owned(),
        source: Box::new(source),
    })?;

    // Nothing above this line has touched the run, so a scene that does not load
    // leaves the running world exactly as it was — the rule ADR-0022 states for
    // the window's reload, holding here because the build either produces a whole
    // simulation or produces nothing.
    let entities = built.world.len();
    *stage.cues = CueMemory::new(&built.world);
    *stage.simulation = built;
    *stage.mode = Mode::SceneFile;

    Ok(Response::LoadScene {
        path: anchor.path().to_owned(),
        digest: anchor.digest().to_owned(),
        entities,
        ticks_run: stage.at.ticks_run(),
    })
}

/// Makes the run a replay of a recording.
///
/// # It goes through `headless::begin`, which is the runner's own prologue
///
/// Everything a replay needs settled before its first tick — that the recording
/// covers the ticks asked for, that this build can consume its input
/// (`validate_recording`), that its world is constituted from the scene it names
/// or from code — is that function's, and it is the same call `run_with` makes.
/// The file half is `crate::load_recording` and `crate::scene_for`, which
/// `main.rs` calls for `--replay`. So a replay started over a socket and a replay
/// started on the command line differ in nothing but where the path came from,
/// and `a_replay_started_over_the_seam_reaches_the_command_lines_state` is what
/// says so.
///
/// # What it replaces
///
/// All five: the simulation, the mode, the input source, the budget — the one
/// command that can *lower* it — and the cue baseline. The sixth thing is the
/// tick counter, which the runner puts back to zero on [`After::Restart`],
/// because a recording's inputs are indexed from its own tick 0 and
/// `Source::take` matches the number exactly.
///
/// # Errors
///
/// [`RequestError::NotWhileReplaying`] during a replay,
/// [`RequestError::Recording`] if the file is not a recording this build can
/// read, and [`RequestError::Replay`] if the runner refuses to start it.
fn start_replay(path: &str, stage: &mut Stage<'_>) -> Result<Response, RequestError> {
    refuse_while_replaying("replay", REPLAY_STEERS, stage)?;

    let recording = crate::load_recording(Path::new(path))
        .map_err(|message| RequestError::Recording { message })?;
    let scene =
        crate::scene_for(&recording).map_err(|message| RequestError::Recording { message })?;

    let Begun {
        simulation,
        mode,
        source,
        budget,
        seed,
        anchor: _,
    } = headless::begin(Plan::Replay {
        recording,
        scene,
        ticks: None,
    })
    .map_err(|source| RequestError::Replay {
        source: Box::new(source),
    })?;

    // The anchor is dropped rather than written onto the band, for the reason
    // `load_scene` records: the band has just been cut and describes only what
    // came before, which was not run against this recording's scene.
    *stage.cues = CueMemory::new(&simulation.world);
    *stage.simulation = simulation;
    *stage.mode = mode;
    *stage.source = source;
    *stage.budget = budget;

    Ok(Response::Replay {
        path: path.to_owned(),
        mode: mode.to_string(),
        seed,
        ticks: budget,
        // The moment this answer was given, which belongs to the run that is
        // now over: a replay starts its own count again at zero, and the
        // runner does that one line after this returns.
        ticks_run: stage.at.ticks_run(),
    })
}

/// Refuses a command that would change what a replay is reproducing.
///
/// **The rule, in one place, since M6.4a: a replay answers questions and takes
/// no orders.** M6.3c wrote it for `step` alone and gave the reason in
/// `RunError::TooShort`'s own terms — past the end of a recording a run
/// continues with no input at all and reproduces nothing. The same reason covers
/// a write, a scene load and a second replay, so the check is one function with
/// the *consequence* as its parameter rather than four checks that could drift.
///
/// The reads are the other side of it and are deliberately untouched: looking at
/// a replayed world changes nothing about what it reproduces, which is the whole
/// point of being able to look.
fn refuse_while_replaying(
    command: &'static str,
    consequence: &'static str,
    stage: &Stage<'_>,
) -> Result<(), RequestError> {
    if stage.replaying() {
        return Err(RequestError::NotWhileReplaying {
            command,
            consequence,
        });
    }

    Ok(())
}

/// What a `step` would do to a replay. M6.3c's own sentence, kept word for word.
const STEP_STEERS: &str = "a replay's length is its recording's, and past the end of a recording a \
                           run continues with no input at all, which reproduces nothing";

/// What a `set_component` would do to one.
///
/// **Measured before it was refused.** M6.4a drove `--replay … --ipc …` — a
/// combination the command line has always allowed — and a write went through:
/// the replay reported a state hash that was not the recorded run's, while the
/// recording it produced stayed byte-identical to the one it had been given,
/// because the cut landed on a band that was already exactly that long. So the
/// run's own account said "this is that recording" about a world that had been
/// written to.
const SET_STEERS: &str = "a write leaves the world in a state the recording does not describe, and \
                          the band cut cannot say so — a band cut to the length it already had is \
                          byte-identical to one that was never cut";

/// What a scene load would do to one.
const LOAD_SCENE_STEERS: &str = "a world constituted from another file is not the one the recording was made against, and its \
     remaining inputs would arrive in it meaning nothing (ADR-0019)";

/// What a second replay would do to the first.
const REPLAY_STEERS: &str = "a second recording would abandon the one being reproduced part-way, and neither of the two \
     would then have been replayed";

/// Writes one component, and hands back what was there before.
///
/// The read comes first deliberately. It is the same
/// `ComponentRegistry::serialize_component` call `get_component` makes, one line
/// earlier, and it is what lets the answer distinguish a replacement from an
/// insertion — which the registry's writing path does not otherwise report, since
/// `World::insert` succeeds either way.
///
/// **The type reaching `insert` came out of the registry**, which is the exact
/// inverse of the trap ADR-0026 Decision 4 records: there, inserting into a world
/// that lacks a registration would succeed and fail later at a dump. Here the
/// erased path is the registry's own, so anything it can write is registered by
/// construction and a dump taken afterwards still works.
fn set(
    entity: EntityName,
    component: &str,
    value: &str,
    stage: &mut Stage<'_>,
) -> Result<Response, RequestError> {
    refuse_while_replaying("set_component", SET_STEERS, stage)?;

    let ticks_run = stage.at.ticks_run();
    let Simulation {
        world, registry, ..
    } = &mut *stage.simulation;

    write(entity, component, value, world, registry, ticks_run)
}

/// The write itself, against a world and the registry that names its components.
///
/// Split from [`set`] so that the refusal above it and the writing below it can
/// be read — and tested — apart. The write half still needs no *run* around it —
/// no budget, no source, no band — which is what kept every M6.3b test of it as
/// it was until M6.7b; since then it takes the moment as a number, because an
/// answer that came from a world says when it was true (ADR-0036) and a write's
/// answer is also where D19 cut the band.
fn write(
    entity: EntityName,
    component: &str,
    value: &str,
    world: &mut World,
    registry: &ComponentRegistry,
    ticks_run: u64,
) -> Result<Response, RequestError> {
    let id = resolve(entity, world)?;

    if !registry.contains(component) {
        return Err(RequestError::UnknownComponent {
            name: component.to_owned(),
            known: registry.iter().map(|info| info.name()).collect(),
        });
    }

    let previous = registry
        .serialize_component(component, world, id)
        .map_err(|source| RequestError::Engine {
            name: entity,
            source,
        })?;

    registry
        .deserialize_component(component, world, id, value)
        .map_err(|source| RequestError::Rejected {
            name: entity,
            component: component.to_owned(),
            source,
        })?;

    Ok(Response::SetComponent {
        entity,
        component: component.to_owned(),
        previous,
        // Also where D19 cut the band: `answer_pending` cuts at exactly this
        // number, so a client learns the cut point from the answer to the very
        // command that caused it.
        ticks_run,
    })
}

/// Answers one request, or says why it cannot be answered.
///
/// The taxonomy stays here rather than in the protocol: M6.1 left
/// [`Response::Error`] as free text and named M6.3 as the owner of the wording,
/// and a client that could branch on a variant is what would justify putting the
/// variants on the wire. There is no client yet (`ProjektPlan.md` §2), so the
/// structure is where the failures are and the wire carries the sentence.
fn read(
    request: &Request,
    world: &World,
    registry: &ComponentRegistry,
    at: Moment,
) -> Result<Response, RequestError> {
    let ticks_run = at.ticks_run();

    match request {
        // The one command that needs no registry: a name is a fact about the
        // world's slots, not about what is stored in them.
        Request::ListEntities => Ok(Response::ListEntities {
            entities: world.entity_ids().into_iter().map(name_of).collect(),
            ticks_run,
        }),

        // The whole world, in the engine's own text. **`canonical_dump` and
        // nothing else**: a walk written here would be a second definition of a
        // format that already has one, and the byte-identity `--expect` needs
        // would then rest on two implementations agreeing rather than on there
        // being one.
        Request::Dump => Ok(Response::Dump {
            state: narvo_ecs::canonical_dump(world, registry)
                .map_err(|source| RequestError::Dump { source })?,
            ticks_run,
        }),

        Request::GetEntity { entity } => {
            let id = resolve(*entity, world)?;

            // The registry iterates in stable-name order, so this comes out in
            // the canonical order without sorting anything — the same walk
            // `canonical_dump` makes over the same registry.
            let mut components = Vec::new();
            for info in registry.iter() {
                let serialized =
                    info.serialize(world, id)
                        .map_err(|source| RequestError::Engine {
                            name: *entity,
                            source,
                        })?;
                if let Some(text) = serialized {
                    components.push(ComponentValue::new(info.name(), text));
                }
            }

            Ok(Response::GetEntity {
                entity: *entity,
                components,
                ticks_run,
            })
        }

        Request::GetComponent { entity, component } => {
            let id = resolve(*entity, world)?;

            // Asked before the lookup, because the registry's own message for an
            // unknown name ends "register it before looking it up", which is
            // advice for whoever builds the world and not for whoever is holding
            // a socket. This one names what there is instead.
            if !registry.contains(component) {
                return Err(RequestError::UnknownComponent {
                    name: component.clone(),
                    known: registry.iter().map(|info| info.name()).collect(),
                });
            }

            let value = registry
                .serialize_component(component, world, id)
                .map_err(|source| RequestError::Engine {
                    name: *entity,
                    source,
                })?;

            Ok(Response::GetComponent {
                entity: *entity,
                component: component.clone(),
                value,
                ticks_run,
            })
        }

        // Unreachable by construction rather than by hope: `answer` dispatches on
        // the same enum with no `_` arm, and its write arm is the only route to
        // `set`. Loud rather than silently wrong if that construction ever
        // changes — and it takes `&World`, so this arm could not perform a write
        // even if it were reached.
        Request::SetComponent { .. } => unreachable!(
            "a write is answered by `set`, which takes `&mut World`; `answer` \
             dispatches on the request and never routes one here"
        ),

        // Unreachable for the same reason and a different one besides: a step
        // touches the run rather than the world, so there is nothing here it
        // could be answered against.
        Request::Step { .. } => unreachable!(
            "a step is answered by `step`, which takes the run's budget and no \
             world at all; `answer` dispatches on the request and never routes \
             one here"
        ),

        // Unreachable for the same reason, and a third one besides: both of
        // these replace the world rather than reading it, so `&World` is not an
        // access either of them could be performed through.
        Request::LoadScene { .. } | Request::Replay { .. } => unreachable!(
            "a scene load and a replay start are answered by `load_scene` and \
             `start_replay`, which replace the run's simulation; `answer` \
             dispatches on the request and never routes one here"
        ),
    }
}

/// Finds the entity a name addresses, or says why there is none.
///
/// **A lookup rather than a conversion**, and that is the whole of the answer to
/// M6.1's fabrication gap. `EntityId::from_parts` is `pub(crate)` and its own
/// documentation says why — a handle built from two numbers is a label, not a
/// handle — while `EntityId` derives `Deserialize` on a public type, so any crate
/// in this workspace *can* build one. This function never does: the only
/// `EntityId` it can return is one the world itself produced, so a name that
/// addresses nothing cannot become a handle that addresses something.
///
/// It walks `entity_ids`, which is the only handle-free route there is:
/// `World::contains` needs a handle to ask about, and `component_type_ids` is
/// `pub(crate)`. The walk costs one enumeration per request, which is the same
/// per-tick cost `audio::cues_of` already pays and is not a hot path.
fn resolve(name: EntityName, world: &World) -> Result<EntityId, RequestError> {
    let mut in_that_slot = None;

    for id in world.entity_ids() {
        if id.index() != name.index() {
            continue;
        }
        if id.generation() == name.generation() {
            return Ok(id);
        }
        // A slot holds at most one live entity, so this can be assigned at most
        // once. Written as a loop anyway, because the fallback below is about
        // what is in the slot rather than about how many things could be.
        in_that_slot = Some(name_of(id));
    }

    match in_that_slot {
        Some(current) => Err(RequestError::Recycled { name, current }),
        None => Err(RequestError::NoSuchEntity { name }),
    }
}

/// Names an entity the way the canonical dump spells it.
///
/// The conversion `narvo-ipc`'s own integration test does by hand, in the place
/// M6.1 said it would end up. One direction only: a name never becomes a handle
/// without [`resolve`] checking it against a world.
fn name_of(entity: EntityId) -> EntityName {
    EntityName::new(
        entity.index(),
        NonZeroU32::new(entity.generation()).expect("hecs generations count from one"),
    )
}

/// Why a request could not be answered.
///
/// # These messages are the product
///
/// A request is written by an agent and the message is the whole of the feedback
/// it gets, so every variant says **what** was wrong and **what to do about it**,
/// and every one is asserted on in a test, wording included — the standard
/// `narvo-scene` set in M4.2, `narvo-input` in M5.1 and `narvo-ipc` in M6.1
/// were all held to.
#[derive(Debug)]
enum RequestError {
    /// Nothing in this world has ever occupied that slot, or nothing does now.
    NoSuchEntity {
        /// The name that was asked about.
        name: EntityName,
    },
    /// The slot exists and holds a different entity than the one named.
    Recycled {
        /// The name that was asked about.
        name: EntityName,
        /// What that slot holds instead.
        current: EntityName,
    },
    /// Nothing is registered under that stable component name.
    UnknownComponent {
        /// The name that was asked about.
        name: String,
        /// The stable names this world's registry does hold, in canonical order.
        known: Vec<&'static str>,
    },
    /// The engine refused to read a component it was asked for.
    Engine {
        /// The entity that was being read.
        name: EntityName,
        /// What the engine said.
        source: EcsError,
    },
    /// The text offered for a component is not a value of that component.
    ///
    /// Separate from [`Engine`](Self::Engine) because the fault is the caller's
    /// rather than the world's, and the two want different sentences: this one
    /// says what was offered and where in it the reader stopped, and the engine
    /// one says what the world could not do.
    Rejected {
        /// The entity that would have been written to.
        name: EntityName,
        /// The component the value was offered for.
        component: String,
        /// What the registry's reader said, position included.
        source: EcsError,
    },
    /// A command that would change what a replay reproduces arrived during one.
    ///
    /// **One variant for four commands**, which is S2's question asked of the
    /// refusals rather than of the band: M6.3c had this for `step` alone, and
    /// M6.4a found that a write, a scene load and a second replay are refused for
    /// the same reason with a different consequence. Parameterising was the
    /// alternative to three more variants that could drift apart, and the
    /// `step` sentence is unchanged inside its own clause.
    NotWhileReplaying {
        /// The command's name on the wire, so the message names what an agent
        /// actually sent.
        command: &'static str,
        /// What that command would do to the reproduction, in one clause.
        consequence: &'static str,
    },
    /// The scene file named for a load could not be read.
    Scene {
        /// The path that was asked for, as it was asked for.
        path: String,
        /// What the anchor reader said.
        source: AnchorError,
    },
    /// The scene file was read and is not a scene this build can constitute a
    /// world from.
    SceneStart {
        /// The scene's path, in the anchor's normal form.
        path: String,
        /// What the loader said, position included (M4.2).
        ///
        /// Boxed, and not for style: `SceneStartError` carries
        /// `narvo_scene::SceneError`, which is large enough that inlining it
        /// here makes every `Result<Response, RequestError>` in this module —
        /// including `resolve`'s, which is on the path of every request — carry
        /// its width. Clippy's `result_large_err` is what says so.
        source: Box<SceneStartError>,
    },
    /// The recording named for a replay could not be read.
    ///
    /// Carries `crate::load_recording`'s or `crate::scene_for`'s own message,
    /// which already names the file and — for a parse failure — the line and what
    /// was wrong with it. Nothing is put in front of it, the same reading
    /// `SceneStartError::Scene` applies to `narvo-scene`'s messages.
    Recording {
        /// What the loader said.
        message: String,
    },
    /// The runner refused to start the replay that was asked for.
    Replay {
        /// What the runner said — a recording this build cannot consume, or a
        /// scene-file recording with no scene.
        ///
        /// Boxed for the reason [`SceneStart`](Self::SceneStart)'s source is:
        /// `RunError` carries a `SceneStartError` of its own.
        source: Box<RunError>,
    },
    /// The world could not be dumped.
    ///
    /// **Its own variant rather than [`Engine`](Self::Engine)'s, because there is
    /// no entity the caller asked about.** A dump is of the whole world; the
    /// entity in the engine's message is one this handler found, not one anybody
    /// named, and putting it in `Engine`'s "entity {name} could not be read"
    /// sentence would report the request as being about an entity it never
    /// mentioned.
    ///
    /// **What it means in practice, measured rather than guessed:**
    /// `canonical_dump` calls `reject_unregistered` for every entity
    /// (`crates/narvo-ecs/src/state.rs:72`) and fails on the first component
    /// type the registry does not know, while `get_entity` walks the registry and
    /// therefore never sees such a component at all. So this command is the
    /// stricter of the two, deliberately: a dump that quietly left a component
    /// out would not be byte-identical to what `--dump` writes — the command line
    /// fails in exactly the same case — and byte-identity is the whole of what
    /// makes a wire dump usable as `--expect`'s input.
    Dump {
        /// What the engine said.
        source: EcsError,
    },
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchEntity { name } => write!(
                f,
                "there is no entity {name} in this world, and slot {} holds nothing at all; \
                 ask list_entities for the entities there are",
                name.index()
            ),
            Self::Recycled { name, current } => write!(
                f,
                "there is no entity {name} in this world: slot {} holds {current} now, so \
                 {name} was despawned and its slot handed out again. Generations count up, \
                 so a name from before a despawn never addresses what took its place",
                name.index()
            ),
            Self::UnknownComponent { name, known } => write!(
                f,
                "no component is registered under the stable name \"{name}\"; this world \
                 registers {}",
                if known.is_empty() {
                    "no components at all".to_owned()
                } else {
                    known.join(", ")
                }
            ),
            Self::Engine { name, source } => {
                write!(f, "entity {name} could not be read: {source}")
            }
            Self::Rejected {
                name,
                component,
                source,
            } => write!(
                f,
                "the value offered for \"{component}\" of entity {name} is not one the \
                 registry can read: {source}"
            ),
            Self::NotWhileReplaying {
                command,
                consequence,
            } => write!(
                f,
                "{command} is refused during a replay: a replay reproduces the run its recording \
                 describes, and {consequence}. A replay answers questions and takes no orders — \
                 let it finish, or start a live run to steer"
            ),
            Self::Scene { path, source } => match source {
                // The anchor reader's own sentence for a missing file is about
                // the scene *a recording was made against*, and a load has no
                // recording in it, so this says the same thing in this command's
                // terms rather than quoting words that would be false here. The
                // other variants are about the path itself and are quoted as they
                // stand.
                AnchorError::Missing { source, .. } => write!(
                    f,
                    "the scene {path} could not be read: {source}. load_scene takes a path \
                     relative to the directory the run was started in"
                ),
                other => write!(f, "the scene {path} could not be loaded: {other}"),
            },
            Self::SceneStart { path, source } => {
                write!(
                    f,
                    "the scene {path} was read and did not load, so the running world is \
                     unchanged: {source}"
                )
            }
            Self::Recording { message } => f.write_str(message),
            Self::Replay { source } => {
                write!(f, "this recording cannot be replayed: {source}")
            }
            Self::Dump { source } => write!(
                f,
                "this world cannot be dumped, so there is no state to report: {source}. \
                 A dump is of the whole world and refuses to leave anything out, which is \
                 what makes it comparable with `narvo --dump`; get_entity is the reading \
                 that answers about one entity and skips what the registry does not know"
            ),
        }
    }
}

impl Error for RequestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchEntity { .. }
            | Self::Recycled { .. }
            | Self::UnknownComponent { .. }
            | Self::NotWhileReplaying { .. }
            // Its own message is the loader's, so the loader's error is already
            // reported rather than being underneath this one.
            | Self::Recording { .. } => None,
            Self::Engine { source, .. }
            | Self::Rejected { source, .. }
            | Self::Dump { source } => Some(source),
            Self::Scene { source, .. } => Some(source),
            Self::SceneStart { source, .. } => Some(source),
            Self::Replay { source } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        After, CueMemory, Effect, Inbox, Moment, Recording, RequestError, RunControl, Simulation,
        Source, Stage, answer, load_scene, name_of, read, resolve, start_replay, step, write,
    };
    use crate::sim::{self, Mode};
    use narvo_ecs::{ComponentRegistry, Rng, Scheduler, World, canonical_dump};
    use narvo_ipc::{Request, Response};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// A name, parsed the way one arrives over a wire.
    fn name(text: &str) -> narvo_ipc::EntityName {
        text.parse().expect("the tests write well-formed names")
    }

    /// The moment the tests that are not about the moment are answered at.
    ///
    /// One constant rather than a number written out fifteen times: these tests
    /// ask what an answer *says about a world*, and every one of them predates
    /// the moment being in the answer at all. Naming it here keeps that visible —
    /// a site using `AT` is a site the moment does not matter to, and the tests
    /// the moment *does* matter to name their own and are grouped under
    /// `an_answer_says_how_many_ticks_had_run_when_it_was_given` below.
    ///
    /// `Tick(0)` and not `Waiting`, because it is the ordinary case: one tick has
    /// run by the time the drain inside tick 0 happens, so `AT.ticks_run()` is 1.
    const AT: Moment = Moment::Tick(0);

    /// A run standing still, holding everything a drain borrows.
    ///
    /// One owner rather than five locals per test, so a test says which of the
    /// five it is varying and stays silent about the rest. It is **live** unless
    /// [`replaying`](Self::replaying) is asked for, because a live run is what
    /// every test written before M6.4a assumed without having to say so.
    struct Bench {
        simulation: Simulation,
        mode: Mode,
        source: Source,
        budget: u64,
        cues: CueMemory,
    }

    impl Bench {
        /// A live run around `simulation`, with a budget no test has to think
        /// about.
        fn live(simulation: Simulation) -> Self {
            let cues = CueMemory::new(&simulation.world);
            Self {
                simulation,
                mode: Mode::Scene,
                source: Source::Pilot(Rng::new(1)),
                budget: 1_000,
                cues,
            }
        }

        /// The same run, reproducing `recording` instead of making one.
        fn replaying(mut self, recording: Recording) -> Self {
            self.source = Source::Recorded { recording, next: 0 };
            self
        }

        /// What a handler is given, at a chosen moment.
        ///
        /// Separate from [`stage`](Self::stage) so that the tests which are about
        /// the moment say which one they mean, and every other test keeps saying
        /// nothing about it.
        fn stage_at(&mut self, at: Moment) -> Stage<'_> {
            Stage {
                at,
                simulation: &mut self.simulation,
                mode: &mut self.mode,
                source: &mut self.source,
                budget: &mut self.budget,
                cues: &mut self.cues,
            }
        }

        /// What a handler is given.
        fn stage(&mut self) -> Stage<'_> {
            Stage {
                at: AT,
                simulation: &mut self.simulation,
                mode: &mut self.mode,
                source: &mut self.source,
                budget: &mut self.budget,
                cues: &mut self.cues,
            }
        }

        /// What a drain is given, at `at` and against `band`.
        fn control<'a>(&'a mut self, at: Moment, band: &'a mut Recording) -> RunControl<'a> {
            RunControl {
                at,
                simulation: &mut self.simulation,
                mode: &mut self.mode,
                source: &mut self.source,
                budget: &mut self.budget,
                band,
                cues: &mut self.cues,
            }
        }

        /// The canonical dump of the world it is standing on.
        fn dump(&self) -> String {
            canonical_dump(&self.simulation.world, &self.simulation.registry)
                .expect("a bench world registers everything it carries")
        }
    }

    /// A built simulation whose world carries all seven of the scene's
    /// registered components.
    ///
    /// `Mode::Scene` rather than a hand-assembled world: the property under test
    /// is that an answer agrees with the canonical dump of a *real* simulation,
    /// and a two-component fixture would agree with a dump that had lost six of
    /// them.
    fn scene() -> Simulation {
        sim::build(Mode::Scene, 1).expect("the demo simulations always build")
    }

    /// The block `canonical_dump` writes for one entity, without its header.
    fn dump_block(dump: &str, entity: &str) -> Vec<String> {
        dump.lines()
            .skip_while(|line| *line != format!("entity {entity}"))
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .map(str::to_owned)
            .collect()
    }

    /// The components of a `get_entity` answer, rendered as dump lines.
    fn as_dump_lines(response: &Response) -> Vec<String> {
        match response {
            Response::GetEntity { components, .. } => components
                .iter()
                .map(|component| format!("  {} {}", component.name, component.value))
                .collect(),
            other => panic!("expected the entity answer, got {other:?}"),
        }
    }

    // ---- what an answer says -------------------------------------------

    /// **The oracle M6.1 built, now against a running simulation.**
    ///
    /// `narvo-ipc`'s own integration test compares a hand-built answer with
    /// `canonical_dump` over a two-component world; this is the same comparison
    /// through the production handler over a world with seven registered
    /// components in it. That is what makes "the protocol carries what the dump
    /// carries" (ADR-0030) a property of this seam rather than of a fixture.
    #[test]
    fn an_entity_answer_is_the_canonical_dumps_block_for_that_entity() {
        let Simulation {
            world, registry, ..
        } = scene();
        let dump = canonical_dump(&world, &registry).expect("everything is registered");

        for entity in world.entity_ids() {
            let spelled = name_of(entity).to_string();
            let response = read(
                &Request::GetEntity {
                    entity: name(&spelled),
                },
                &world,
                &registry,
                AT,
            )
            .expect("the entity is alive and everything it carries is registered");

            assert_eq!(
                as_dump_lines(&response),
                dump_block(&dump, &spelled),
                "the answer for {spelled} is not what the dump says about it"
            );
        }
    }

    /// One component crosses as the registry's own bytes, down to the line the
    /// dump writes.
    #[test]
    fn a_component_answer_carries_the_line_the_dump_writes() {
        let Simulation {
            world, registry, ..
        } = scene();
        let dump = canonical_dump(&world, &registry).expect("everything is registered");

        let camera = world
            .entity_ids()
            .into_iter()
            .find(|entity| world.get::<narvo_ecs::Camera>(*entity).is_ok())
            .expect("the scene has a camera");
        let spelled = name_of(camera).to_string();

        let response = read(
            &Request::GetComponent {
                entity: name(&spelled),
                component: "camera".to_owned(),
            },
            &world,
            &registry,
            AT,
        )
        .expect("the camera entity carries a camera");

        let value = match &response {
            Response::GetComponent {
                value: Some(value), ..
            } => value.clone(),
            other => panic!("expected a component answer with a value, got {other:?}"),
        };
        assert!(
            dump_block(&dump, &spelled).contains(&format!("  camera {value}")),
            "the answer's bytes are not the dump's bytes: {value}"
        );
    }

    /// A component the entity does not carry is an absence, not a failure.
    #[test]
    fn a_component_the_entity_does_not_carry_answers_null() {
        let Simulation {
            world, registry, ..
        } = scene();

        // Any entity that is not the camera, picked from the world rather than
        // by slot number, which keeps this test independent of the order the
        // scene spawns things in.
        let plain = world
            .entity_ids()
            .into_iter()
            .find(|entity| world.get::<narvo_ecs::Camera>(*entity).is_err())
            .expect("the scene has entities that are not the camera");

        let response = read(
            &Request::GetComponent {
                entity: name(&name_of(plain).to_string()),
                component: "camera".to_owned(),
            },
            &world,
            &registry,
            AT,
        )
        .expect("an absence is not an error");

        match response {
            Response::GetComponent { value, .. } => assert_eq!(value, None),
            other => panic!("expected a component answer, got {other:?}"),
        }
    }

    /// The entity list is the world's canonical order, name for name.
    #[test]
    fn the_entity_list_is_the_worlds_canonical_order() {
        let Simulation {
            world, registry, ..
        } = scene();

        let response =
            read(&Request::ListEntities, &world, &registry, AT).expect("a list never fails");
        let expected: Vec<String> = world
            .entity_ids()
            .into_iter()
            .map(|entity| name_of(entity).to_string())
            .collect();

        match response {
            Response::ListEntities { entities, .. } => {
                let spelled: Vec<String> = entities.iter().map(ToString::to_string).collect();
                assert_eq!(spelled, expected);
            }
            other => panic!("expected the list answer, got {other:?}"),
        }
    }

    // ---- names that address nothing ------------------------------------

    /// A name on a slot this world has never used.
    #[test]
    fn a_name_on_an_empty_slot_says_so_and_says_what_to_ask_instead() {
        let Simulation {
            world, registry, ..
        } = scene();

        let error = read(
            &Request::GetEntity {
                entity: name("4000000v1"),
            },
            &world,
            &registry,
            AT,
        )
        .expect_err("this world has no four-millionth slot");

        assert!(matches!(error, RequestError::NoSuchEntity { .. }));
        assert_eq!(
            error.to_string(),
            "there is no entity 4000000v1 in this world, and slot 4000000 holds nothing at \
             all; ask list_entities for the entities there are"
        );
    }

    /// **The recycled slot, measured rather than assumed.**
    ///
    /// Despawn and respawn puts a new entity in the old one's slot with the
    /// generation raised by one — `narvo-ecs`'s own
    /// `the_hecs_bit_layout_is_what_this_module_decodes` pins that, and the
    /// assertions below check it here rather than take it as read. What a name
    /// from before that despawn does is the finding: it addresses
    /// nothing, and the message names what took its place, because "no such
    /// entity" on a slot that visibly holds one is the least actionable answer
    /// there is.
    #[test]
    fn a_name_on_a_recycled_slot_names_what_holds_it_now() {
        let mut world = World::new();

        let first = world.spawn();
        assert_eq!(name_of(first).to_string(), "0v1");
        world.despawn(first).expect("it was just spawned");
        let recycled = world.spawn();
        assert_eq!(
            name_of(recycled).to_string(),
            "0v2",
            "hecs is expected to hand the freed slot straight back"
        );

        let error = resolve(name("0v1"), &world).expect_err("0v1 is gone");
        assert!(matches!(error, RequestError::Recycled { .. }));
        assert_eq!(
            error.to_string(),
            "there is no entity 0v1 in this world: slot 0 holds 0v2 now, so 0v1 was \
             despawned and its slot handed out again. Generations count up, so a name from \
             before a despawn never addresses what took its place"
        );

        // And the live name in the same slot resolves, so the refusal above is
        // about the generation rather than about the slot.
        assert_eq!(
            resolve(name("0v2"), &world).expect("0v2 is alive"),
            recycled
        );
    }

    /// A despawned entity whose slot nobody took back.
    #[test]
    fn a_name_on_a_despawned_entity_is_refused() {
        let mut world = World::new();

        let kept = world.spawn();
        let doomed = world.spawn();
        world.despawn(doomed).expect("it was just spawned");

        let error = resolve(name("1v1"), &world).expect_err("1v1 was despawned");
        assert!(matches!(error, RequestError::NoSuchEntity { .. }));
        assert!(error.to_string().contains("slot 1 holds nothing at all"));

        assert_eq!(resolve(name("0v1"), &world).expect("0v1 is alive"), kept);
    }

    /// **The hazard the name type exists to prevent, measured on a real stale
    /// handle.**
    ///
    /// No handle is fabricated here: `kept` is an `EntityId` the world itself
    /// produced, held across a despawn whose slot is then handed out again. That
    /// is exactly the shape a protocol that reused `EntityId` would produce, and
    /// what the engine does with it is the measurement — if a stale handle
    /// addressed its slot's new occupant, `resolve`'s enumeration would be the
    /// only thing standing between an agent and somebody else's component.
    #[test]
    fn a_stale_handle_is_refused_even_once_its_slot_has_been_recycled() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Mark>("mark")
            .expect("a fresh registry accepts it");

        let mut world = World::new();
        let kept = world.spawn();
        world.insert(kept, Mark { n: 1 }).expect("just spawned");
        world.despawn(kept).expect("it was just spawned");

        let recycled = world.spawn();
        world.insert(recycled, Mark { n: 2 }).expect("just spawned");
        assert_eq!(kept.index(), recycled.index(), "the slot came back");
        assert_ne!(kept.generation(), recycled.generation());

        let refused = registry
            .serialize_component("mark", &world, kept)
            .expect_err("the stale handle addresses nothing");
        assert!(
            matches!(refused, narvo_ecs::EcsError::NoSuchEntity { .. }),
            "a stale handle read a recycled slot: {refused}"
        );

        // And the live handle reads what is actually there.
        assert_eq!(
            registry
                .serialize_component("mark", &world, recycled)
                .expect("alive and registered"),
            Some("(n:2)".to_owned())
        );
    }

    /// A component with a stable name, for the tests that need a registry.
    #[derive(Serialize, Deserialize)]
    struct Mark {
        n: u64,
    }

    // ---- component names -----------------------------------------------

    /// An unknown component name is answered with the names there are.
    #[test]
    fn an_unknown_component_name_lists_the_ones_this_world_has() {
        let Simulation {
            world, registry, ..
        } = scene();
        let first = name_of(world.entity_ids()[0]).to_string();

        let error = read(
            &Request::GetComponent {
                entity: name(&first),
                component: "postion".to_owned(),
            },
            &world,
            &registry,
            AT,
        )
        .expect_err("`postion` is not a component");

        assert!(matches!(error, RequestError::UnknownComponent { .. }));
        assert_eq!(
            error.to_string(),
            "no component is registered under the stable name \"postion\"; this world \
             registers camera, follow, layer, sampling, shake, transform, wander"
        );
    }

    /// A world that registers nothing says that, rather than listing an empty
    /// list.
    #[test]
    fn a_world_with_an_empty_registry_says_it_registers_nothing() {
        let mut world = World::new();
        world.spawn();
        let registry = ComponentRegistry::new();

        let error = read(
            &Request::GetComponent {
                entity: name("0v1"),
                component: "transform".to_owned(),
            },
            &world,
            &registry,
            AT,
        )
        .expect_err("nothing is registered");

        assert_eq!(
            error.to_string(),
            "no component is registered under the stable name \"transform\"; this world \
             registers no components at all"
        );
    }

    /// A component whose own `Serialize` fails is reported with the engine's own
    /// words, under the name the protocol uses for the entity.
    #[test]
    fn a_component_that_will_not_serialize_is_reported_with_the_engines_words() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Unwritable>("unwritable")
            .expect("a fresh registry accepts it");

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Unwritable).expect("just spawned");

        let error = read(
            &Request::GetComponent {
                entity: name("0v1"),
                component: "unwritable".to_owned(),
            },
            &world,
            &registry,
            AT,
        )
        .expect_err("this component refuses to be written");

        assert!(matches!(error, RequestError::Engine { .. }));
        let message = error.to_string();
        assert!(
            message.starts_with("entity 0v1 could not be read: serializing the "),
            "{message}"
        );
        assert!(
            message.ends_with("this component refuses to be written"),
            "{message}"
        );
    }

    /// A component that cannot be written, so the engine's serialization failure
    /// has a way to happen.
    struct Unwritable;

    impl Serialize for Unwritable {
        fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "this component refuses to be written",
            ))
        }
    }

    impl<'de> Deserialize<'de> for Unwritable {
        /// Never reached. `register_component` requires `DeserializeOwned`, and
        /// this exists to satisfy that bound rather than to do anything.
        fn deserialize<D: Deserializer<'de>>(_deserializer: D) -> Result<Self, D::Error> {
            Err(serde::de::Error::custom(
                "this component is never read back",
            ))
        }
    }

    // ---- the named limit -----------------------------------------------

    /// **What a `get_entity` answer cannot see, pinned rather than assumed.**
    ///
    /// `canonical_dump` refuses a world whose entity carries a component the
    /// registry does not know (ADR-0008: a component outside the hash makes a
    /// divergence in it invisible). This handler walks the registry, so it cannot
    /// notice such a component at all — the answer is silently short.
    ///
    /// Closing the gap would need `World::component_type_ids`, which is
    /// `pub(crate)` on purpose ("answering it publicly would invite code that
    /// branches on component types at runtime", `world.rs:320-329`), so it is a
    /// change to `narvo-ecs` and therefore not this task's. Reported, and held
    /// here so the limit cannot quietly change.
    #[test]
    fn an_unregistered_component_is_invisible_to_an_answer_and_fatal_to_the_dump() {
        struct Hidden;

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Hidden).expect("just spawned");
        let registry = ComponentRegistry::new();

        assert!(
            canonical_dump(&world, &registry).is_err(),
            "the dump is expected to refuse an unregistered component"
        );

        let response = read(
            &Request::GetEntity {
                entity: name("0v1"),
            },
            &world,
            &registry,
            AT,
        )
        .expect("the entity is alive, so the answer is produced");

        match response {
            Response::GetEntity { components, .. } => assert!(
                components.is_empty(),
                "the answer is expected to be silently short: {components:?}"
            ),
            other => panic!("expected the entity answer, got {other:?}"),
        }
    }

    // ---- reading changes nothing ---------------------------------------

    /// **Twenty reads leave the world byte-identical.**
    ///
    /// The measurable half of "the read path only reads", in the shape M5.6c gave
    /// it. What it can and cannot see is worth stating: it compares *state*, so
    /// it catches any write that moves the dump — including one made through the
    /// `&mut` query ADR-0005 says is reachable behind a shared `&World` — and it
    /// is blind to a write that stores the value already there. Nothing in the
    /// type system closes that remainder, which is why ADR-0005 calls the
    /// read-only discipline a convention where queries are concerned.
    #[test]
    fn reading_a_world_never_changes_it() {
        let mut bench = Bench::live(scene());
        let before = bench.dump();

        let names: Vec<String> = bench
            .simulation
            .world
            .entity_ids()
            .into_iter()
            .map(|entity| name_of(entity).to_string())
            .collect();

        for round in 0..20 {
            let spelled = &names[round % names.len()];
            for request in [
                Request::ListEntities,
                Request::GetEntity {
                    entity: name(spelled),
                },
                Request::GetComponent {
                    entity: name(spelled),
                    component: "transform".to_owned(),
                },
            ] {
                let (_response, effect) = answer(&request, bench.stage());
                assert_eq!(
                    effect,
                    Effect::Nothing,
                    "a read classified itself as something that moves the run"
                );
                assert_eq!(bench.budget, 1_000, "a read moved the run's budget");
            }
        }

        let after = bench.dump();
        sim::assert_same_state("twenty reads", &before, &after);
    }

    // ---- the queue ------------------------------------------------------

    /// A band long enough for a test to cut wherever it likes.
    fn band() -> Recording {
        Recording::new(Mode::Scene, 1, 1_000)
    }

    /// Drains an inbox at tick 0 against a throwaway band, and hands the band
    /// back for the tests that do care what happened to it.
    fn drain(inbox: &mut Inbox, bench: &mut Bench) -> Recording {
        let mut band = band();
        drain_at(inbox, bench, &mut band, 0);
        band
    }

    /// Drains at `tick` against `band`, and **asserts that nothing restarted the
    /// run**.
    ///
    /// Every test written before M6.4a is a test of a run that carries straight
    /// on, and none of them said so because there was nothing else a drain could
    /// leave behind. Asserting it here rather than returning it says it for all
    /// of them at once, and is what keeps `After`'s `must_use` from becoming
    /// twelve `let _ =` lines that would each be a place the check went missing.
    /// The tests that do expect a restart use [`drain_expecting`].
    fn drain_at(inbox: &mut Inbox, bench: &mut Bench, band: &mut Recording, tick: u64) {
        assert_eq!(
            drain_expecting(inbox, bench, Moment::Tick(tick), band),
            After::Carry,
            "the drain at tick {tick} restarted the run"
        );
    }

    /// Drains at `at` against `band`, and hands back what the runner would do.
    fn drain_expecting(
        inbox: &mut Inbox,
        bench: &mut Bench,
        at: Moment,
        band: &mut Recording,
    ) -> After {
        inbox.answer_pending(bench.control(at, band))
    }

    /// An inbox answers each request once and hands each answer over once.
    #[test]
    fn a_drained_inbox_keeps_neither_the_request_nor_the_answer() {
        let mut bench = Bench::live(scene());
        let mut inbox = Inbox::new();

        inbox.push(Request::ListEntities);
        drain(&mut inbox, &mut bench);
        assert_eq!(inbox.take_answers().len(), 1);

        // Nothing is left on either side: a second drain produces nothing, and a
        // second take hands over nothing.
        drain(&mut inbox, &mut bench);
        assert!(inbox.take_answers().is_empty());
    }

    /// A request that cannot be answered becomes an error response rather than
    /// stopping anything.
    #[test]
    fn an_unanswerable_request_becomes_an_error_response() {
        let mut bench = Bench::live(scene());
        let mut inbox = Inbox::new();

        inbox.push(Request::GetEntity {
            entity: name("4000000v1"),
        });
        inbox.push(Request::ListEntities);
        drain(&mut inbox, &mut bench);

        let answers = inbox.take_answers();
        assert_eq!(answers.len(), 2, "the second request is still answered");
        match &answers[0] {
            Response::Error { message } => assert!(message.contains("4000000v1"), "{message}"),
            other => panic!("expected an error answer, got {other:?}"),
        }
        assert!(matches!(answers[1], Response::ListEntities { .. }));
    }

    /// Answers come back in the order the requests arrived.
    #[test]
    fn answers_come_back_in_the_order_the_requests_arrived() {
        let mut bench = Bench::live(scene());
        let first = name_of(bench.simulation.world.entity_ids()[0]).to_string();
        let mut inbox = Inbox::new();

        inbox.push(Request::GetEntity {
            entity: name(&first),
        });
        inbox.push(Request::ListEntities);
        inbox.push(Request::GetComponent {
            entity: name(&first),
            component: "transform".to_owned(),
        });
        drain(&mut inbox, &mut bench);

        let tags: Vec<String> = inbox
            .take_answers()
            .iter()
            .map(|answer| answer.to_json().split('"').nth(1).unwrap_or("").to_owned())
            .collect();
        assert_eq!(tags, vec!["get_entity", "list_entities", "get_component"]);
    }

    // ---- the write (M6.3b) ---------------------------------------------

    /// A component that is one float, so a round trip is about the float rather
    /// than about a struct shape. The same fixture M6.1's own path test uses.
    #[derive(Serialize, Deserialize)]
    struct Probe {
        x: f32,
    }

    /// A world holding one entity that carries a `Mark` and one that does not,
    /// with both test components registered.
    fn marked() -> Simulation {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Mark>("mark")
            .expect("a fresh registry accepts it");
        registry
            .register_component::<Probe>("probe")
            .expect("a fresh registry accepts it");

        let mut world = World::new();
        let carrying = world.spawn();
        world.insert(carrying, Mark { n: 1 }).expect("just spawned");
        world.spawn();

        // A `Simulation` rather than the pair it was until M6.4a: a drain now
        // takes the world and the registry together, because two of the commands
        // replace both at once. The scheduler is empty — nothing here runs a
        // tick, and a system would be state this fixture does not mean to have.
        Simulation {
            world,
            registry,
            scheduler: Scheduler::new(),
        }
    }

    /// A write replaces a value and hands back the one it replaced.
    #[test]
    fn a_write_replaces_a_value_and_reports_what_it_replaced() {
        let Simulation {
            mut world,
            registry,
            ..
        } = marked();

        let response = write(
            name("0v1"),
            "mark",
            "(n:42)",
            &mut world,
            &registry,
            AT.ticks_run(),
        )
        .expect("0v1 carries a mark and the value is one");

        match response {
            Response::SetComponent {
                previous,
                component,
                entity,
                ..
            } => {
                assert_eq!(previous, Some("(n:1)".to_owned()));
                assert_eq!(component, "mark");
                assert_eq!(entity.to_string(), "0v1");
            }
            other => panic!("expected a set answer, got {other:?}"),
        }

        assert_eq!(
            registry
                .serialize_component("mark", &world, resolve(name("0v1"), &world).expect("alive"))
                .expect("alive and registered"),
            Some("(n:42)".to_owned()),
            "the value the registry now holds is not the one that was written"
        );
    }

    /// **A write to a component the entity does not carry adds it.**
    ///
    /// Measured rather than reasoned about, because it is not what "set" implies:
    /// the registry's writing path ends in `World::insert`, which succeeds
    /// whether or not the entity already carried one. The consequence is bigger
    /// than a changed value — an entity's component set is in the canonical dump,
    /// so a write can change the *shape* of the state — and the answer is what
    /// makes it visible: `previous` is `None` exactly here.
    #[test]
    fn a_write_on_a_component_the_entity_lacks_adds_it_and_says_so() {
        let Simulation {
            mut world,
            registry,
            ..
        } = marked();
        let before = canonical_dump(&world, &registry).expect("everything is registered");
        assert!(
            !before.contains("1v1\n  mark"),
            "1v1 is supposed to start without a mark:\n{before}"
        );

        let response = write(
            name("1v1"),
            "mark",
            "(n:7)",
            &mut world,
            &registry,
            AT.ticks_run(),
        )
        .expect("the entity is alive");

        match response {
            Response::SetComponent { previous, .. } => assert_eq!(
                previous, None,
                "an added component is supposed to report no previous value"
            ),
            other => panic!("expected a set answer, got {other:?}"),
        }

        let after = canonical_dump(&world, &registry).expect("everything is registered");
        assert!(
            after.contains("entity 1v1\n  mark (n:7)"),
            "the dump did not gain the added component:\n{after}"
        );
        assert_ne!(before, after, "adding a component moved no state");
    }

    /// **Every value that survives the read path survives the write path too.**
    ///
    /// ADR-0014's round-trip consequence, collected on the side M6.1 could not
    /// reach: it measured that eleven `f32` stress values cross *out* through the
    /// registry's RON with their bits intact. This sends each of them back *in*
    /// through `set` and reads it out again, comparing `to_bits` — `==` would
    /// call `0.0` and `-0.0` equal and two `NaN`s unequal, so it cannot see what
    /// a state hash sees.
    #[test]
    fn every_stress_value_survives_a_set_and_a_get_with_its_bits_intact() {
        let stress: [(&str, f32); 11] = [
            ("0.1", 0.1_f32),
            ("one bit above 0.1", f32::from_bits(0.1_f32.to_bits() + 1)),
            ("+0.0", 0.0_f32),
            ("-0.0", -0.0_f32),
            ("smallest subnormal", f32::from_bits(1)),
            ("MIN_POSITIVE", f32::MIN_POSITIVE),
            ("MAX", f32::MAX),
            ("-MAX", -f32::MAX),
            ("infinity", f32::INFINITY),
            ("negative infinity", f32::NEG_INFINITY),
            ("NaN", f32::NAN),
        ];

        for (label, value) in stress {
            let Simulation {
                mut world,
                registry,
                ..
            } = marked();
            let entity = resolve(name("0v1"), &world).expect("alive");

            // Out through the registry, exactly as `get_component` would.
            world.insert(entity, Probe { x: value }).expect("alive");
            let text = registry
                .serialize_component("probe", &world, entity)
                .expect("alive and registered")
                .expect("just inserted");

            // Back in through the write path, onto an entity that has none.
            write(
                name("1v1"),
                "probe",
                &text,
                &mut world,
                &registry,
                AT.ticks_run(),
            )
            .unwrap_or_else(|error| panic!("{label}: the write was refused: {error}"));

            let back = world
                .get::<Probe>(resolve(name("1v1"), &world).expect("alive"))
                .expect("just written")
                .x;
            assert_eq!(
                back.to_bits(),
                value.to_bits(),
                "{label}: {:#010x} came back as {:#010x} (the text was {text})",
                value.to_bits(),
                back.to_bits()
            );
        }
    }

    /// A value that is not RON is refused, and the message says where the reader
    /// stopped.
    #[test]
    fn a_malformed_value_is_refused_and_the_message_carries_the_position() {
        let Simulation {
            mut world,
            registry,
            ..
        } = marked();

        let error = write(
            name("0v1"),
            "mark",
            "(n:",
            &mut world,
            &registry,
            AT.ticks_run(),
        )
        .expect_err("`(n:` is not a value");

        assert!(matches!(error, RequestError::Rejected { .. }));
        let message = error.to_string();
        // Printed as well as asserted on: the exact sentence an agent receives is
        // the product, and a maintainer reading `--nocapture` should not have to
        // reassemble it from three `contains` calls.
        println!("  malformed value: {message}");
        assert!(
            message.starts_with(
                "the value offered for \"mark\" of entity 0v1 is not one the registry can \
                 read: reading a "
            ),
            "{message}"
        );
        assert!(
            message.contains("1:4"),
            "the reader's own position is missing: {message}"
        );

        // And nothing was written: the value that was there is still there.
        assert_eq!(
            registry
                .serialize_component("mark", &world, resolve(name("0v1"), &world).expect("alive"))
                .expect("alive and registered"),
            Some("(n:1)".to_owned())
        );
    }

    /// A well-formed value of the wrong component's shape is refused too.
    #[test]
    fn a_value_of_another_components_shape_is_refused() {
        let Simulation {
            mut world,
            registry,
            ..
        } = marked();

        let error = write(
            name("0v1"),
            "mark",
            "(x:0.5)",
            &mut world,
            &registry,
            AT.ticks_run(),
        )
        .expect_err("a mark has no x");

        assert!(matches!(error, RequestError::Rejected { .. }));
        println!("  wrong shape: {error}");
        assert!(
            error.to_string().contains("\"mark\""),
            "{}",
            error.to_string()
        );
    }

    /// A write to a name that addresses nothing is refused before anything is
    /// read or written.
    #[test]
    fn a_write_to_a_name_that_addresses_nothing_is_refused() {
        let Simulation {
            mut world,
            registry,
            ..
        } = marked();
        let before = canonical_dump(&world, &registry).expect("everything is registered");

        let error = write(
            name("4000000v1"),
            "mark",
            "(n:9)",
            &mut world,
            &registry,
            AT.ticks_run(),
        )
        .expect_err("there is no such entity");

        assert!(matches!(error, RequestError::NoSuchEntity { .. }));
        assert_eq!(
            canonical_dump(&world, &registry).expect("everything is registered"),
            before
        );
    }

    /// An unknown component name is refused with the names there are.
    #[test]
    fn a_write_under_an_unknown_component_name_is_refused() {
        let Simulation {
            mut world,
            registry,
            ..
        } = marked();

        let error = write(
            name("0v1"),
            "marc",
            "(n:9)",
            &mut world,
            &registry,
            AT.ticks_run(),
        )
        .expect_err("`marc` is not registered");

        assert_eq!(
            error.to_string(),
            "no component is registered under the stable name \"marc\"; this world registers \
             mark, probe"
        );
    }

    // ---- the band cut (D19) ---------------------------------------------

    /// **An accepted write cuts the band at the tick it was accepted on.**
    ///
    /// The cut is `tick + 1` because the seam runs after the systems: everything
    /// up to and including tick `N` has happened, so a band covering `N + 1`
    /// ticks is exactly the run that reaches the state the write landed on.
    #[test]
    fn an_accepted_write_cuts_the_band_at_the_tick_it_landed_on() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        assert!(inbox.band_is_open());
        inbox.push(Request::SetComponent {
            entity: name("0v1"),
            component: "mark".to_owned(),
            value: "(n:5)".to_owned(),
        });
        drain_at(&mut inbox, &mut bench, &mut band, 42);

        assert_eq!(band.ticks(), 43, "the band covers ticks 0 through 42");
        assert!(!inbox.band_is_open());
        assert!(matches!(
            inbox.take_answers().as_slice(),
            [Response::SetComponent { .. }]
        ));
    }

    /// **A refused write does not cut.** D19 cuts on acceptance, and its own
    /// wording is "ein Lauf, der einen annimmt" — a run that *takes* one.
    #[test]
    fn a_refused_write_leaves_the_band_alone() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        for refused in [
            Request::SetComponent {
                entity: name("4000000v1"),
                component: "mark".to_owned(),
                value: "(n:5)".to_owned(),
            },
            Request::SetComponent {
                entity: name("0v1"),
                component: "marc".to_owned(),
                value: "(n:5)".to_owned(),
            },
            Request::SetComponent {
                entity: name("0v1"),
                component: "mark".to_owned(),
                value: "(n:".to_owned(),
            },
        ] {
            inbox.push(refused);
        }
        drain_at(&mut inbox, &mut bench, &mut band, 7);

        assert_eq!(band.ticks(), 1_000, "a refused write cut the band");
        assert!(inbox.band_is_open());
        assert_eq!(inbox.take_answers().len(), 3);
    }

    /// The band is cut once, at the first accepted write.
    ///
    /// A second cut could only move the promise forwards, past a write the band
    /// already cannot account for.
    #[test]
    fn the_band_is_cut_at_the_first_write_and_not_again() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        let write = |value: &str| Request::SetComponent {
            entity: name("0v1"),
            component: "mark".to_owned(),
            value: value.to_owned(),
        };

        inbox.push(write("(n:1)"));
        drain_at(&mut inbox, &mut bench, &mut band, 10);
        assert_eq!(band.ticks(), 11);

        inbox.push(write("(n:2)"));
        drain_at(&mut inbox, &mut bench, &mut band, 20);
        assert_eq!(band.ticks(), 11, "a second write moved the cut");
    }

    /// **A write that stores the value already there cuts the band like any
    /// other**, and this is where the reach of every instrument here is named.
    ///
    /// M6.3a measured that such a write is invisible to a state comparison. The
    /// mirror of that finding is this: a `set` that stored nothing new and a
    /// `set` that silently did nothing at all leave the world in the same state,
    /// return the same `previous`, and cannot be told apart by anything outside
    /// the world. **The band cut is not that instrument either** — it fires on
    /// acceptance, which is what D19 asks for and which both cases satisfy.
    ///
    /// What *is* distinguishable is a write that stores something different, and
    /// `a_write_replaces_a_value_and_reports_what_it_replaced` is what holds it.
    /// There is no test for the remaining pair, and this note stands in for one.
    #[test]
    fn a_write_that_changes_nothing_still_cuts_the_band() {
        let mut bench = Bench::live(marked());
        let before = bench.dump();
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push(Request::SetComponent {
            entity: name("0v1"),
            component: "mark".to_owned(),
            value: "(n:1)".to_owned(),
        });
        drain_at(&mut inbox, &mut bench, &mut band, 3);

        assert_eq!(
            bench.dump(),
            before,
            "writing the value that was already there moved the state"
        );
        assert_eq!(band.ticks(), 4, "the cut is on acceptance, not on effect");
    }

    // ---- the instrument (M6.3c) ----------------------------------------

    /// **The instrument shown in its failure state, not only its success one.**
    ///
    /// Everything M6.3c proves about a cut at N > 0 rests on this: that a request
    /// scheduled for tick N is answered at tick N **and at no earlier drain**. An
    /// instrument that quietly delivered at tick 0 would make every one of those
    /// proofs a tick-0 proof in costume, and the workspace already carries eight
    /// instances of the "green because it checks nothing" class.
    ///
    /// So the failure state is what is asserted first: five drains before the due
    /// tick produce **nothing**. The success state — one answer, at tick 5 — comes
    /// after, and would be satisfied by an instrument that ignored the tick
    /// entirely.
    #[test]
    fn the_instrument_delivers_at_the_tick_it_was_given_and_not_before() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push_at(5, Request::ListEntities);
        assert_eq!(inbox.undelivered(), 1);

        for tick in 0..5 {
            drain_at(&mut inbox, &mut bench, &mut band, tick);
            assert!(
                inbox.take_answers().is_empty(),
                "the request was delivered at tick {tick}, which is not the tick it was \
                 scheduled for"
            );
            assert_eq!(inbox.undelivered(), 1, "it left the schedule early");
        }

        drain_at(&mut inbox, &mut bench, &mut band, 5);
        assert_eq!(inbox.take_answers().len(), 1, "it never came due at all");
        assert_eq!(inbox.undelivered(), 0);

        // And it is delivered once: a sixth drain finds nothing left.
        drain_at(&mut inbox, &mut bench, &mut band, 6);
        assert!(inbox.take_answers().is_empty());
    }

    /// A request scheduled for a tick the run never reaches is never delivered,
    /// and the instrument says so rather than hiding it.
    ///
    /// The delivery test is exact equality on the tick, so a schedule entry for a
    /// tick that is skipped or never arrives simply stays put. That is a failure
    /// mode a test built on this instrument has to be able to notice, which is
    /// what `undelivered` is for — every end-to-end proof below asserts it is
    /// zero.
    #[test]
    fn a_request_scheduled_for_a_tick_that_never_comes_stays_undelivered() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push_at(900, Request::ListEntities);
        for tick in 0..20 {
            drain_at(&mut inbox, &mut bench, &mut band, tick);
        }

        assert!(inbox.take_answers().is_empty());
        assert_eq!(inbox.undelivered(), 1);
    }

    // ---- the run's budget (M6.3c) --------------------------------------

    /// A bench whose budget is `budget` and whose world is the smallest there is.
    ///
    /// The `step` tests are about arithmetic on one number and want nothing else
    /// in scope, which is what this keeps true now that a handler takes a run
    /// rather than a `&mut u64`.
    fn budgeted(budget: u64) -> Bench {
        let mut bench = Bench::live(marked());
        bench.budget = budget;
        bench
    }

    /// A step raises the budget and answers with the total.
    #[test]
    fn a_step_raises_the_budget_and_answers_with_the_total() {
        let mut bench = budgeted(4);
        let response = step(5, &mut bench.stage()).expect("a live run takes a step");

        assert_eq!(bench.budget, 9);
        assert_eq!(response.ticks_run(), Some(1));
        assert_eq!(
            response,
            Response::Step {
                granted: 9,
                ticks_run: 1,
            }
        );
    }

    /// Two steps add up, which is what makes stepping twice mean two ticks.
    #[test]
    fn two_steps_add_rather_than_the_second_replacing_the_first() {
        let mut bench = budgeted(0);
        step(1, &mut bench.stage()).expect("granted");
        step(1, &mut bench.stage()).expect("granted");

        assert_eq!(
            bench.budget, 2,
            "the second step replaced the first instead of adding"
        );
    }

    /// A step of nothing grants nothing, and is not an error.
    ///
    /// Measured rather than designed toward: there is no rule against asking for
    /// zero more ticks, and inventing one would be this seam having an opinion
    /// about arithmetic.
    #[test]
    fn a_step_of_zero_grants_nothing_and_is_not_refused() {
        let mut bench = budgeted(7);
        let response = step(0, &mut bench.stage()).expect("zero is a number of ticks");

        assert_eq!(bench.budget, 7);
        assert_eq!(
            response,
            Response::Step {
                granted: 7,
                ticks_run: 1,
            }
        );
    }

    /// An enormous step saturates rather than wrapping.
    ///
    /// **Measured at the seam and never run.** A budget of `u64::MAX` is a run
    /// that does not end in any time a test can wait for, which is exactly why
    /// this is asserted on the arithmetic instead of through `run_with`. The
    /// hazard is real and is named on `step` itself: an unbounded grant is how a
    /// client hangs a headless run, and under M6.3d's reading of an exhausted
    /// budget it is also how one asks a run to proceed freely.
    #[test]
    fn an_enormous_step_saturates_rather_than_wrapping() {
        let mut bench = budgeted(10);
        step(u64::MAX, &mut bench.stage()).expect("granted");
        assert_eq!(bench.budget, u64::MAX);

        // And again, so the saturating case itself is exercised rather than the
        // one addition that happens to fit.
        step(1_000, &mut bench.stage()).expect("granted");
        assert_eq!(bench.budget, u64::MAX, "the budget wrapped");
    }

    /// A step during a replay is refused, and the message says why.
    ///
    /// **The wording moved in M6.4a and lost nothing.** M6.3c's sentence was this
    /// variant's whole message; it is now the `consequence` clause inside a
    /// sentence four commands share, because the reason they are refused is one
    /// reason and four separate messages could drift apart. What was specific to
    /// `step` — that a replay's length is its recording's — is still here word
    /// for word.
    #[test]
    fn a_step_during_a_replay_is_refused_with_its_reason() {
        let mut bench = budgeted(4).replaying(band());
        let error = step(5, &mut bench.stage()).expect_err("a replay's length is its file's");

        assert!(matches!(error, RequestError::NotWhileReplaying { .. }));
        assert_eq!(
            error.to_string(),
            "step is refused during a replay: a replay reproduces the run its recording \
             describes, and a replay's length is its recording's, and past the end of a \
             recording a run continues with no input at all, which reproduces nothing. A replay \
             answers questions and takes no orders — let it finish, or start a live run to steer"
        );
        assert_eq!(bench.budget, 4, "a refused step moved the budget");
    }

    /// A step is not a write: it lengthens the band and does not cut it.
    #[test]
    fn a_step_lengthens_an_open_band_and_never_cuts_it() {
        let mut bench = budgeted(4);
        let mut inbox = Inbox::new();
        let mut band = Recording::new(Mode::Scene, 1, 4);

        inbox.push(Request::Step { ticks: 5 });
        let after = inbox.answer_pending(bench.control(Moment::Tick(3), &mut band));

        assert_eq!(after, After::Carry);
        assert_eq!(bench.budget, 9);
        assert_eq!(band.ticks(), 9, "the band did not follow the run");
        assert!(inbox.band_is_open(), "a step cut the band");
    }

    /// A cut band is not lengthened by a later step.
    ///
    /// The band stopped describing this run at the write; how much further the run
    /// goes is no longer its business, and a longer band would claim ticks it
    /// cannot reproduce.
    #[test]
    fn a_step_after_a_write_leaves_the_cut_band_where_it_is() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push(Request::SetComponent {
            entity: name("0v1"),
            component: "mark".to_owned(),
            value: "(n:3)".to_owned(),
        });
        inbox.push(Request::Step { ticks: 50 });
        drain_at(&mut inbox, &mut bench, &mut band, 6);

        assert_eq!(bench.budget, 1_050, "the step was refused as well");
        assert!(!inbox.band_is_open());
        assert_eq!(band.ticks(), 7, "the cut band followed the run after all");
    }

    /// A read behind a write in one drain sees what the write did.
    #[test]
    fn a_read_behind_a_write_in_one_drain_sees_what_the_write_wrote() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();

        inbox.push(Request::SetComponent {
            entity: name("0v1"),
            component: "mark".to_owned(),
            value: "(n:99)".to_owned(),
        });
        inbox.push(Request::GetComponent {
            entity: name("0v1"),
            component: "mark".to_owned(),
        });
        drain(&mut inbox, &mut bench);

        let answers = inbox.take_answers();
        match &answers[1] {
            Response::GetComponent { value, .. } => {
                assert_eq!(value.as_deref(), Some("(n:99)"));
            }
            other => panic!("expected a component answer, got {other:?}"),
        }
    }

    // ---- the scene load and the replay start (M6.4a) --------------------

    /// A scene that is committed, small and certainly loadable.
    ///
    /// **Named relative to the working directory a test runs in**, which is what
    /// the command needs: `Anchor::read` refuses an absolute path, so a scene
    /// load cannot be given one and a test cannot smuggle a temp directory in.
    const A_REAL_SCENE: &str = "scenes/determinism-case.ron";

    /// A committed file that exists and is certainly not a scene.
    const NOT_A_SCENE: &str = "Cargo.toml";

    /// **The assumption every relative path here rests on, checked rather than
    /// assumed.**
    ///
    /// Cargo runs a test binary with the working directory set to its package
    /// root; nothing in this repository had ever depended on that, and M6.4a's
    /// two commands do, because both take a path from an agent and resolve it
    /// against wherever the run was started. If it were ever false, the tests
    /// below would fail with "no such file" and say nothing about why — this one
    /// says why.
    #[test]
    fn a_test_runs_in_the_package_root_so_a_relative_scene_path_resolves() {
        assert!(
            std::path::Path::new(A_REAL_SCENE).is_file(),
            "{A_REAL_SCENE} is not readable from {:?}; every relative path in these \
             tests assumes the package root",
            std::env::current_dir()
        );
        assert!(std::path::Path::new(NOT_A_SCENE).is_file());
    }

    /// Writes `text` to a scratch file and hands back the **relative** path to it.
    ///
    /// Relative, and under the workspace's `target/`, for a reason that was
    /// measured rather than anticipated: `std::env::temp_dir()` is on `C:` while
    /// this working copy is on `D:`, so on Windows there is no relative spelling
    /// of a system temp path at all — and `Anchor::read` refuses an absolute one,
    /// so a scene load cannot be handed one. `target/` is beside the working copy
    /// on every platform, is already where a test may write, and is ignored by
    /// git.
    ///
    /// The two `..` steps are the package root's distance from the workspace
    /// root, which `a_test_runs_in_the_package_root_so_a_relative_scene_path_resolves`
    /// is what pins.
    fn scratch(name: &str, text: &str) -> String {
        let directory = std::path::Path::new("../../target");
        std::fs::create_dir_all(directory).expect("the target directory is writable");

        let path = format!("../../target/narvo-m64a-{}-{name}", std::process::id());
        std::fs::write(&path, text).expect("the target directory is writable");
        path
    }

    /// A recording of a `motion` run, which consumes no input and so needs none.
    fn a_recording(ticks: u64) -> Recording {
        Recording::new(Mode::Motion, 7, ticks)
    }

    /// **A scene load replaces the world, and says which bytes it took.**
    ///
    /// The three things the answer carries are asserted against three
    /// independent sources: the path against the anchor's normal form, the digest
    /// against a SHA-256 taken here over the file's own bytes, and the count
    /// against the world that is now running. A handler that reported its own
    /// inputs back would pass none of the three.
    #[test]
    fn a_scene_load_replaces_the_world_and_reports_the_bytes_it_took() {
        let mut bench = Bench::live(marked());
        let before = bench.dump();

        let answer = load_scene(A_REAL_SCENE, &mut bench.stage()).expect("a committed scene loads");

        let text = std::fs::read_to_string(A_REAL_SCENE).expect("just loaded it");
        let expected = narvo_core::sha256::hex(&narvo_core::sha256::sha256(text.as_bytes()));

        match answer {
            Response::LoadScene {
                path,
                digest,
                entities,
                ..
            } => {
                assert_eq!(path, A_REAL_SCENE, "the answer names another file");
                assert_eq!(digest, expected, "the digest is not the file's");
                assert_eq!(
                    entities,
                    bench.simulation.world.len(),
                    "the count is not the loaded world's"
                );
                assert!(entities > 0, "the scene constituted an empty world");
            }
            other => panic!("expected a scene answer, got {other:?}"),
        }

        assert_ne!(before, bench.dump(), "the world was not replaced");
        assert_eq!(
            bench.mode,
            Mode::SceneFile,
            "the run still claims to be the simulation it no longer is"
        );
    }

    /// The registry moves with the world, so the run can still be dumped.
    ///
    /// Not implied by the test above, which dumps through the bench's own pair.
    /// This is the failure `sim::Simulation` exists to prevent: a world swapped
    /// while its registry stayed would carry components nothing can name, and
    /// `canonical_dump` — which is the state hash, the determinism suite and the
    /// `--dump` report — would refuse it from that tick on.
    #[test]
    fn a_scene_load_brings_the_registry_that_names_the_new_worlds_components() {
        let mut bench = Bench::live(marked());
        load_scene(A_REAL_SCENE, &mut bench.stage()).expect("a committed scene loads");

        assert!(
            canonical_dump(&bench.simulation.world, &bench.simulation.registry).is_ok(),
            "the loaded world cannot be dumped, so the registry did not travel with it"
        );
        assert_eq!(bench.simulation.registry.len(), 17);
    }

    /// **A scene load cuts the band exactly where a `set` would**, which is S2's
    /// question answered by measurement.
    ///
    /// The same numbers as `an_accepted_write_cuts_the_band_at_the_tick_it_landed_on`
    /// against the same drain at the same tick, reached through the same
    /// `Effect::Wrote` arm and the same `cut_at`. No second cut mechanism was
    /// written for the second consumer, and this is what says so.
    #[test]
    fn a_scene_load_cuts_the_band_at_the_tick_it_landed_on() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        assert!(inbox.band_is_open());
        inbox.push(Request::LoadScene {
            path: A_REAL_SCENE.to_owned(),
        });
        drain_at(&mut inbox, &mut bench, &mut band, 42);

        assert_eq!(band.ticks(), 43, "the band covers ticks 0 through 42");
        assert!(!inbox.band_is_open());
        assert!(matches!(
            inbox.take_answers().as_slice(),
            [Response::LoadScene { .. }]
        ));
    }

    /// **A load while the run waits at tick 0 cuts the band to nothing**, which
    /// is the case `cut_after` could not express at all.
    ///
    /// M6.3b's spelling computed `tick + 1` and its smallest answer was one; a
    /// run that is *waiting* has run some number of ticks and that number may be
    /// zero. The scene load is the second command to reach it and the first for
    /// which it is the ordinary case rather than a corner — an agent connecting
    /// to `--ticks 0` and loading a scene before anything has happened is the
    /// workflow, not the edge.
    #[test]
    fn a_load_while_waiting_at_tick_zero_cuts_the_band_to_nothing() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push(Request::LoadScene {
            path: A_REAL_SCENE.to_owned(),
        });
        let after = drain_expecting(
            &mut inbox,
            &mut bench,
            Moment::Waiting { ticks_run: 0 },
            &mut band,
        );

        assert_eq!(after, After::Carry, "a scene load restarted the tick count");
        assert_eq!(band.ticks(), 0, "the band claims ticks that never ran");
        assert!(!inbox.band_is_open());
    }

    /// The band keeps the anchor it was made with, cut or not.
    ///
    /// A cut band describes only the ticks *before* the load, and those really
    /// were run against the scene the anchor names. Rewriting it to the newly
    /// loaded file would make a faithful prefix claim a provenance it does not
    /// have — and ADR-0019's whole purpose is that a recording refuses to replay
    /// against a file it was not made from.
    #[test]
    fn a_scene_load_leaves_the_bands_own_anchor_where_it_was() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();
        band.anchor_to(crate::scene_anchor::Anchor::from_parts(
            "scenes/elsewhere.ron".to_owned(),
            "0".repeat(64),
        ));

        inbox.push(Request::LoadScene {
            path: A_REAL_SCENE.to_owned(),
        });
        drain_at(&mut inbox, &mut bench, &mut band, 5);

        assert_eq!(
            band.scene().map(|anchor| anchor.path().to_owned()),
            Some("scenes/elsewhere.ron".to_owned()),
            "the cut band was re-anchored to a scene its ticks were not run against"
        );
    }

    /// **The cue baseline is reseeded, so a loaded counter is not heard as
    /// purchases.**
    ///
    /// `CueMemory::new`'s own documentation names the failure: a counter's value
    /// is state and its movement is the event, so a memory carried across a swap
    /// would read a scene authored with `count: 5` as five buys arriving at once.
    /// The window's reload reseeds for exactly this reason
    /// (`SceneHost::replace_world`), and this measures that the *seam* does too —
    /// `audio::tests::a_fresh_memory_does_not_reissue_past_clicks` already holds
    /// the memory's own half.
    ///
    /// **It is discriminating, which took arranging.** A counter on an entity the
    /// memory has never seen sounds nothing whatever the memory holds
    /// (`cues_of`'s `None => 0` arm), so the two worlds have to put their
    /// counters in the *same slot* with different values. They do: both scenes
    /// describe one entity, so both counters are `0v1`, and a memory carried
    /// across would see 0 become 5 and click five times.
    #[test]
    fn a_scene_load_reseeds_the_cue_baseline() {
        let counting = |count: i64| {
            format!(
                "Scene(entities: [(components: {{\n\
                 \x20   \"tally\": (action: \"buy\", count: {count}),\n\
                 }})])\n"
            )
        };
        let empty = scratch("counter-0.ron", &counting(0));
        let full = scratch("counter-5.ron", &counting(5));

        let mut bench = Bench::live(marked());
        load_scene(&empty, &mut bench.stage()).expect("a one-entity scene loads");
        let _ = crate::audio::cues_of(&bench.simulation.world, 0, &mut bench.cues);

        load_scene(&full, &mut bench.stage()).expect("a one-entity scene loads");
        let heard = crate::audio::cues_of(&bench.simulation.world, 1, &mut bench.cues);

        let clicks = heard
            .iter()
            .filter(|cue| {
                **cue
                    == narvo_audio::Cue::play(
                        1,
                        narvo_audio::Channel::Sfx,
                        narvo_audio::SoundLibrary::CLICK,
                    )
            })
            .count();
        assert_eq!(
            clicks, 0,
            "the counter the second scene was authored with was heard as purchases: {heard:?}"
        );

        let _ = std::fs::remove_file(&empty);
        let _ = std::fs::remove_file(&full);
    }

    /// A scene that is not there is refused, and nothing moves.
    #[test]
    fn a_missing_scene_is_refused_and_the_running_world_is_unchanged() {
        let mut bench = Bench::live(marked());
        let before = bench.dump();

        let error = load_scene("scenes/no-such-scene.ron", &mut bench.stage())
            .expect_err("that scene is not in the repository");

        assert!(matches!(error, RequestError::Scene { .. }));
        let message = error.to_string();
        println!("  missing scene: {message}");
        assert!(
            message.starts_with("the scene scenes/no-such-scene.ron could not be read: "),
            "{message}"
        );
        assert!(
            message.ends_with(
                ". load_scene takes a path relative to the directory the run was started in"
            ),
            "{message}"
        );

        assert_eq!(bench.dump(), before, "a refused load moved the world");
        assert_eq!(bench.mode, Mode::Scene, "a refused load moved the mode");
    }

    /// A file that is not a scene is refused with the position the loader found,
    /// and the running world keeps running.
    #[test]
    fn an_unloadable_scene_is_refused_with_its_position_and_changes_nothing() {
        let mut bench = Bench::live(marked());
        let before = bench.dump();

        let error =
            load_scene(NOT_A_SCENE, &mut bench.stage()).expect_err("a manifest is not a scene");

        assert!(matches!(error, RequestError::SceneStart { .. }));
        let message = error.to_string();
        println!("  unloadable scene: {message}");
        assert!(
            message.starts_with(
                "the scene Cargo.toml was read and did not load, so the running world is \
                 unchanged: "
            ),
            "{message}"
        );
        assert!(
            message.contains("1:"),
            "the loader's own position is missing: {message}"
        );

        assert_eq!(bench.dump(), before, "a refused load moved the world");
    }

    /// An absolute scene path is refused before the file is even read.
    #[test]
    fn an_absolute_scene_path_is_refused_with_the_reason_a_recording_needs_one() {
        let mut bench = Bench::live(marked());
        let absolute = std::env::temp_dir().join("wherever.ron");

        let error = load_scene(&absolute.display().to_string(), &mut bench.stage())
            .expect_err("an absolute path is refused");

        let message = error.to_string();
        println!("  absolute scene path: {message}");
        assert!(message.starts_with("the scene "), "{message}");
        assert!(message.contains("is an absolute path"), "{message}");
    }

    /// **A replay start replaces the run and asks the runner to restart it.**
    #[test]
    fn a_replay_start_replaces_the_run_and_asks_for_a_restart() {
        let path = scratch("start.rec", &a_recording(37).render());

        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push(Request::Replay { path: path.clone() });
        let after = drain_expecting(&mut inbox, &mut bench, Moment::Tick(11), &mut band);

        assert_eq!(
            after,
            After::Restart,
            "the tick count was left where it was"
        );
        assert_eq!(bench.budget, 37, "the budget is not the recording's length");
        assert_eq!(bench.mode, Mode::Motion, "the mode is not the recording's");
        assert!(
            matches!(bench.source, Source::Recorded { .. }),
            "the run is still generating its own input"
        );
        assert_eq!(band.ticks(), 12, "the band was not cut at the redirect");
        assert!(!inbox.band_is_open());

        match inbox.take_answers().as_slice() {
            [
                Response::Replay {
                    mode, seed, ticks, ..
                },
            ] => {
                assert_eq!(mode, "motion");
                assert_eq!(*seed, 7);
                assert_eq!(*ticks, 37);
            }
            other => panic!("expected one replay answer, got {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    /// A recording that is not there is refused in the loader's own words.
    #[test]
    fn a_missing_recording_is_refused_in_the_loaders_own_words() {
        let mut bench = Bench::live(marked());

        let error = start_replay("no-such-run.rec", &mut bench.stage())
            .expect_err("that recording does not exist");

        assert!(matches!(error, RequestError::Recording { .. }));
        let message = error.to_string();
        println!("  missing recording: {message}");
        assert!(
            message.starts_with("could not read the recording no-such-run.rec: "),
            "{message}"
        );
        assert!(
            matches!(bench.source, Source::Pilot(_)),
            "a refused replay redirected the run"
        );
    }

    /// A file that is not a recording is refused with the line the parser stopped
    /// on.
    #[test]
    fn an_unparsable_recording_is_refused_with_its_line() {
        let path = scratch("broken.rec", "narvo-recording 1\nmode nonsense\n");
        let mut bench = Bench::live(marked());

        let error = start_replay(&path, &mut bench.stage()).expect_err("`nonsense` is not a mode");

        let message = error.to_string();
        println!("  unparsable recording: {message}");
        assert!(message.contains("nonsense"), "{message}");
        assert!(message.contains(&path), "{message}");

        let _ = std::fs::remove_file(&path);
    }

    /// A recording this build cannot replay faithfully is refused by the runner's
    /// own check, before the run is touched.
    ///
    /// `validate_recording` is `headless::begin`'s and runs before the first tick
    /// of a `--replay` too. Reaching it from here is what says the seam goes
    /// through that prologue rather than around it.
    #[test]
    fn a_recording_whose_mode_cannot_consume_its_input_is_refused() {
        let path = scratch(
            "mismatch.rec",
            "narvo-recording 1\nmode motion\nseed 1\nticks 10\n\n3 turn 1\nend\n",
        );
        let mut bench = Bench::live(marked());

        let error = start_replay(&path, &mut bench.stage()).expect_err("motion consumes no input");

        assert!(matches!(error, RequestError::Replay { .. }));
        let message = error.to_string();
        println!("  unreplayable recording: {message}");
        assert!(
            message.starts_with("this recording cannot be replayed: "),
            "{message}"
        );
        assert!(message.contains("consumes no input at all"), "{message}");
        assert!(
            matches!(bench.source, Source::Pilot(_)),
            "a refused replay redirected the run"
        );

        let _ = std::fs::remove_file(&path);
    }

    // ---- what a replay refuses, and what it does not (S4) ---------------

    /// **A replay answers questions and takes no orders**, all four commands and
    /// both reads in one place.
    ///
    /// S4 asked where the two new commands collide with each other and with what
    /// was already there. The answer turned out to be one rule rather than a
    /// table of pairs, and this is it measured: during a replay every command
    /// that would change what is being reproduced is refused with the same
    /// sentence and a consequence of its own, and the reads are untouched —
    /// looking at a replayed world changes nothing about what it reproduces.
    ///
    /// **The `set` half is a behaviour change and was measured before it was
    /// made.** `--replay … --ipc …` has always been a legal command line, so a
    /// write during a replay was reachable over a real socket; M6.4a drove one
    /// and the replay reported a state hash that was not the recorded run's while
    /// the recording it produced stayed byte-identical to the one it was given.
    #[test]
    fn during_a_replay_every_steering_command_is_refused_and_the_reads_are_not() {
        let refusals = [
            (
                Request::Step { ticks: 1 },
                "step",
                "a replay's length is its recording's",
            ),
            (
                Request::SetComponent {
                    entity: name("0v1"),
                    component: "mark".to_owned(),
                    value: "(n:5)".to_owned(),
                },
                "set_component",
                "a write leaves the world in a state the recording does not describe",
            ),
            (
                Request::LoadScene {
                    path: A_REAL_SCENE.to_owned(),
                },
                "load_scene",
                "a world constituted from another file is not the one the recording was made \
                 against",
            ),
            (
                Request::Replay {
                    path: "elsewhere.rec".to_owned(),
                },
                "replay",
                "a second recording would abandon the one being reproduced part-way",
            ),
        ];

        for (request, command, clause) in refusals {
            let mut bench = Bench::live(marked()).replaying(a_recording(50));
            let before = bench.dump();
            let mut inbox = Inbox::new();
            let mut band = band();

            inbox.push(request);
            drain_at(&mut inbox, &mut bench, &mut band, 4);

            match inbox.take_answers().as_slice() {
                [Response::Error { message }] => {
                    println!("  {command} during a replay: {message}");
                    assert!(
                        message.starts_with(&format!("{command} is refused during a replay: ")),
                        "{message}"
                    );
                    assert!(message.contains(clause), "{message}");
                    assert!(
                        message.ends_with(
                            "A replay answers questions and takes no orders — let it finish, or \
                             start a live run to steer"
                        ),
                        "{message}"
                    );
                }
                other => panic!("{command} was not refused: {other:?}"),
            }

            // Nothing moved: not the world, not the band, not the budget.
            assert_eq!(bench.dump(), before, "{command} changed the world");
            assert_eq!(band.ticks(), 1_000, "{command} cut the band");
            assert!(inbox.band_is_open(), "{command} closed the band");
            assert_eq!(bench.budget, 1_000, "{command} moved the budget");
        }

        // And the reads are answered exactly as they are in a live run.
        //
        // **`Dump` joined this list in M6.7b**, and it was worth checking rather
        // than assuming: a red-flank injection that refused a dump during a
        // replay was caught only by the one test written for it, because this
        // list named two of the reads and there were three. It is the reading a
        // replay most exists to produce — a repro of what was reproduced — so a
        // refusal here would be the sharpest possible way to make a replay
        // useless while every other test stayed green.
        let mut bench = Bench::live(marked()).replaying(a_recording(50));
        let mut inbox = Inbox::new();
        let mut band = band();
        inbox.push(Request::ListEntities);
        inbox.push(Request::GetEntity {
            entity: name("0v1"),
        });
        inbox.push(Request::Dump);
        drain_at(&mut inbox, &mut bench, &mut band, 4);

        assert!(matches!(
            inbox.take_answers().as_slice(),
            [
                Response::ListEntities { .. },
                Response::GetEntity { .. },
                Response::Dump { .. }
            ]
        ));
    }

    /// **A replay start after a scene load is taken, and the band stays where the
    /// load cut it.**
    ///
    /// The third pair S4 named. A scene load leaves the run live, so the replay
    /// is not refused; the band was already closed by the load, so the cut does
    /// not move — which is `the_band_is_cut_at_the_first_write_and_not_again`
    /// holding across two different commands rather than two of one.
    #[test]
    fn a_replay_start_after_a_scene_load_is_taken_and_the_cut_does_not_move() {
        let path = scratch("after-load.rec", &a_recording(9).render());

        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push(Request::LoadScene {
            path: A_REAL_SCENE.to_owned(),
        });
        drain_at(&mut inbox, &mut bench, &mut band, 3);
        assert_eq!(band.ticks(), 4);
        assert_eq!(bench.mode, Mode::SceneFile);

        inbox.push(Request::Replay { path: path.clone() });
        let after = drain_expecting(&mut inbox, &mut bench, Moment::Tick(20), &mut band);

        assert_eq!(after, After::Restart);
        assert_eq!(band.ticks(), 4, "a second redirect moved the cut");
        assert_eq!(bench.mode, Mode::Motion, "the replay's mode did not take");
        assert_eq!(bench.budget, 9);

        let _ = std::fs::remove_file(&path);
    }

    /// A second scene load replaces the world again and leaves the cut alone.
    #[test]
    fn a_second_scene_load_replaces_the_world_again_and_leaves_the_cut_alone() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        for tick in [2, 30] {
            inbox.push(Request::LoadScene {
                path: A_REAL_SCENE.to_owned(),
            });
            drain_at(&mut inbox, &mut bench, &mut band, tick);
            assert!(matches!(
                inbox.take_answers().as_slice(),
                [Response::LoadScene { .. }]
            ));
        }

        assert_eq!(band.ticks(), 3, "the second load moved the cut");
    }

    // ---- the dump and the moment (M6.7b) --------------------------------

    /// The dump on the seam is `canonical_dump`'s own text and nothing beside it.
    ///
    /// Near-tautological on purpose, and it is the tautology that matters: the
    /// handler must **call** the one function that writes this format rather than
    /// walk the world itself. A second walk would produce something dump-shaped
    /// that `--expect` would then reject, which is the gap M6.7a measured, moved
    /// one layer in.
    #[test]
    fn a_dump_over_the_seam_is_the_canonical_dump_of_the_same_world() {
        let Simulation {
            world, registry, ..
        } = scene();
        let expected = canonical_dump(&world, &registry).expect("everything is registered");

        let response = read(&Request::Dump, &world, &registry, AT).expect("a scene world dumps");

        let Response::Dump { state, .. } = response else {
            panic!("a dump request is answered with a dump");
        };
        assert_eq!(state, expected);
        assert!(
            state.ends_with('\n'),
            "the trailing newline is part of the dump and part of what --expect compares"
        );
    }

    /// Every answer that came from a world says how many ticks had run.
    ///
    /// **`ticks_run`, not a tick index**, and the two differ by one inside a
    /// tick: the drain happens after that tick's systems, so a request answered
    /// inside tick 41 is answered against the world a run of **42** ticks ends
    /// in. That is the number `--ticks` takes, which is the whole reason the
    /// field is this quantity and not the other one.
    #[test]
    fn an_answer_says_how_many_ticks_had_run_when_it_was_given() {
        for (at, expected) in [
            (Moment::Tick(0), 1),
            (Moment::Tick(41), 42),
            (Moment::Waiting { ticks_run: 0 }, 0),
            (Moment::Waiting { ticks_run: 7 }, 7),
        ] {
            let mut bench = Bench::live(scene());

            for request in [Request::ListEntities, Request::Dump] {
                let (response, _) = answer(&request, bench.stage_at(at));
                assert_eq!(
                    response.ticks_run(),
                    Some(expected),
                    "{request:?} at {at:?} dated itself wrongly"
                );
            }
        }
    }

    /// A write's answer is dated too, and its date is where the band was cut.
    ///
    /// The two numbers come from one place — `Moment::ticks_run` — so this is
    /// less about arithmetic than about the client being able to *learn* the cut
    /// point at all. A cut band is byte-indistinguishable from an ordinary short
    /// one (ADR-0032), so the answer is the only place it can be said.
    #[test]
    fn a_writes_answer_is_dated_where_the_band_was_cut() {
        let mut bench = Bench::live(marked());
        let mut inbox = Inbox::new();
        let mut band = band();

        inbox.push(Request::SetComponent {
            entity: name("0v1"),
            component: "mark".to_owned(),
            value: "(n:9)".to_owned(),
        });
        drain_at(&mut inbox, &mut bench, &mut band, 12);

        let answers = inbox.take_answers();
        let [Response::SetComponent { ticks_run, .. }] = answers.as_slice() else {
            panic!("a write is answered with a write");
        };
        assert_eq!(*ticks_run, 13, "inside tick 12, thirteen ticks have run");
        assert_eq!(band.ticks(), 13, "the answer's date is not the band's cut");
    }

    /// A component the registry does not know makes the dump fail, not shrink.
    ///
    /// **The inherited hardness, checked rather than assumed.** `canonical_dump`
    /// rejects an unregistered component per entity while `get_entity` walks the
    /// registry and never sees one — so the two commands genuinely differ here,
    /// and the dump is the stricter of them on purpose: the command line fails in
    /// exactly this case, and a wire dump that quietly left a component out would
    /// stop being the thing `--expect` compares against.
    ///
    /// **This is not the same check as
    /// [`an_unregistered_component_is_invisible_to_an_answer_and_fatal_to_the_dump`],
    /// and the difference is the point.** That one calls `canonical_dump`
    /// directly, so it pins the *engine's* behaviour and the limit M6.3a reported
    /// — that `get_entity` cannot see such a component without
    /// `World::component_type_ids`, which is `pub(crate)` on purpose. This one
    /// pins the *handler*: that the engine's refusal reaches the wire as a
    /// refusal, rather than being swallowed into a shorter dump. A red-flank
    /// injection that made the handler swallow it left that older test green.
    ///
    /// So the dump command gives the protocol a way to *notice* an unregistered
    /// component that no other command has — through `canonical_dump`'s own
    /// check, without this crate ever naming the `pub(crate)` item.
    #[test]
    fn a_dump_of_a_world_holding_an_unregistered_component_fails_rather_than_leaving_it_out() {
        // A registry that knows `mark` and not `probe`, over a world that carries
        // both.
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Mark>("mark")
            .expect("a fresh registry accepts it");

        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Mark { n: 1 }).expect("alive");
        world.insert(entity, Probe { x: 0.5 }).expect("alive");

        // The reading that skips it, so the contrast is measured and not claimed.
        let seen = read(
            &Request::GetEntity {
                entity: name("0v1"),
            },
            &world,
            &registry,
            AT,
        )
        .expect("get_entity walks the registry and sees only what it knows");
        assert_eq!(as_dump_lines(&seen), vec!["  mark (n:1)".to_owned()]);

        let error = read(&Request::Dump, &world, &registry, AT)
            .expect_err("a dump refuses a world it cannot write whole");

        assert!(matches!(error, RequestError::Dump { .. }));
        let message = error.to_string();
        assert!(message.contains("this world cannot be dumped"), "{message}");
        // The engine names the Rust **type**, because a component nothing
        // registered has no stable name to be called by. Asserted as it is rather
        // than as one might wish it were.
        assert!(message.contains("Probe"), "{message}");
        assert!(message.contains("0v1"), "{message}");
        assert!(
            message.contains("get_entity"),
            "the message has to name the reading that does answer: {message}"
        );
    }

    /// A dump during a replay is answered, like every other read.
    ///
    /// ADR-0032's criterion applied rather than a new decision: *"a run that is
    /// reproducing a recording answers reads and refuses every command that would
    /// change what it reproduces"*. A dump changes nothing — and it is the one a
    /// replay exists to produce, so a refusal here would make a replay unable to
    /// say what it reproduced.
    #[test]
    fn a_dump_during_a_replay_is_answered() {
        let mut bench = Bench::live(scene()).replaying(band());

        let (response, effect) = answer(&Request::Dump, bench.stage());

        assert!(
            matches!(response, Response::Dump { .. }),
            "a dump is a read and a replay answers reads: {response:?}"
        );
        assert_eq!(effect, Effect::Nothing, "a dump must not cut the band");
    }
}
