//! The frame loop: the order a frame's work runs in, and what each part cost.
//!
//! [`FixedTimestep`](crate::FixedTimestep) answers *how many* simulation ticks
//! are due. This module answers *when they run relative to everything else*, and
//! it is the piece `ProjektPlan.md` §6/M3 needs before the 50 000-sprite figure
//! can be measured at all: a frame loop with presentation, and a timing
//! measurement that lives outside the renderer.
//!
//! # Why it is here rather than in the runner
//!
//! This crate's own boundary put it here. `lib.rs` names "Clocks and frame
//! timing, the fixed-timestep scheduler that decouples simulation rate from
//! frame rate" as the first of its responsibilities, and a loop that turns
//! elapsed time into ticks is that sentence's subject.
//!
//! The same boundary decides what may *not* be here: "Anything that needs an
//! operating-system resource ... belongs in a sibling crate." A monotonic clock
//! is an operating-system resource. So [`Clock`] is a trait here and its real
//! implementation is not — it lives in `narvo-app`, beside the window whose
//! frames it times. Nothing in this module reads a *system* clock — it reads
//! only the [`Clock`] it is handed, once at the start of a frame and once after
//! each phase — the same arrangement and for the same reason as
//! [`FixedTimestep::advance`](crate::FixedTimestep::advance) being handed an
//! elapsed duration rather than measuring one.
//!
//! That is also what makes the loop testable without a GPU, a window or a
//! wall clock: a staged [`Clock`] makes every duration in a [`FrameRecord`] an
//! exact, chosen number, so a test can assert the arithmetic instead of
//! sampling it.
//!
//! # What the loop does not decide
//!
//! [`FrameHost`]'s five methods are opaque to this module. It knows the order
//! they run in and what each one cost; it does not know that one of them touches
//! a world and another a swapchain, and it cannot, because this crate knows
//! nothing about either. **A test here can therefore assert order, count and
//! timing, and not that the right data flowed** — that is covered where the data
//! is, in the runner's own tests.
//!
//! # What is drawn between two ticks
//!
//! The state the last tick left. Nothing is interpolated, and one frame draws at
//! most once — exactly once whenever there is somewhere to draw — whether it ran
//! no ticks, one, or the whole cap's worth.
//!
//! Interpolating would need two simulation states to sit between, and a `World`
//! holds one; keeping a second is a copy of every drawn component every tick,
//! which at 50 000 entities is the per-frame cost the throughput criterion is
//! asking about in the first place.
//! [`FixedTimestep::interpolation`](crate::FixedTimestep::interpolation) is
//! still there and still documents itself as an input to rendering. **Nothing
//! outside `time.rs`'s own doctest and unit tests calls it**, here or anywhere
//! in the workspace, and adopting it is a change with its own design question
//! rather than a detail of this one.

use std::time::Duration;

use crate::time::FixedTimestep;

/// A monotonic time source, read between the phases of a frame.
///
/// `&mut self` rather than `&self` so that a test can hand out a scripted
/// sequence: staging the answers is the whole point of the trait existing, and a
/// clock that could only be read immutably would push every test towards
/// interior mutability for no gain.
///
/// The contract is **monotonically non-decreasing**. A source that can go
/// backwards would make a phase appear to take a negative duration, and
/// [`FrameLoop`] handles that by saturating to zero rather than panicking — a
/// frame loop is the wrong place to take a process down, and a zero is visible
/// in the table as an impossibility.
pub trait Clock {
    /// Time elapsed since an arbitrary fixed origin.
    ///
    /// Only differences between two readings are ever used, so the origin may be
    /// anything as long as it does not move during a run.
    fn now(&mut self) -> Duration;
}

/// The parts of a frame, in the order [`FrameLoop::step`] runs them.
///
/// Five rather than the four a frame is usually described with, and the extra
/// one is [`Self::Acquire`]. Asking the swapchain for an image is the *candidate
/// home* of the display wait: under a Fifo surface an image is free only once
/// the display has scanned one out, and folding that wait into the drawing would
/// put the monitor's refresh rate inside a number that is supposed to be about
/// the work. Which call actually blocks is a property of the driver and the
/// mode; `docs/perf/BASELINE.md` records that on this machine it is this one.
/// Separating it is what lets a measurement subtract it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FramePhase {
    /// Every simulation tick that came due this frame, together.
    ///
    /// One entry for the whole batch rather than one per tick: the loop's
    /// question is what a *frame* cost, and a frame that ran three ticks spent
    /// the sum of them here.
    Tick,
    /// Turning the simulation's state into what the renderer takes.
    Extract,
    /// Obtaining somewhere to draw. **The candidate home of the display wait.**
    Acquire,
    /// Recording the drawing commands and handing them to the driver.
    ///
    /// Whether the GPU's own execution is inside this number is the host's
    /// business rather than the loop's, and this crate cannot see which. For the
    /// windowed host it is not: submission there is asynchronous and returns
    /// before the work runs. A host that blocks on the GPU inside this phase
    /// would put it in.
    Encode,
    /// Handing the finished image over to be shown.
    Present,
}

impl FramePhase {
    /// Every phase, in the order they run.
    pub const ALL: [Self; 5] = [
        Self::Tick,
        Self::Extract,
        Self::Acquire,
        Self::Encode,
        Self::Present,
    ];

    /// The phases whose sum is the frame's own work.
    ///
    /// [`Self::Acquire`] and [`Self::Present`] are excluded because both are
    /// handovers to something outside the process — a swapchain and a compositor
    /// — and a run paced by the display spends most of a frame in the first of
    /// them doing nothing. `ProjektPlan.md` §6/M3 words its criterion as 50 000
    /// sprites at a stable 60 fps; a display-paced interval answers that
    /// trivially and says nothing about the engine, so the budget is asked of
    /// this sum and the interval is reported beside it rather than instead.
    pub const WORK: [Self; 3] = [Self::Tick, Self::Extract, Self::Encode];

    /// The phase's name in a report or a table.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Tick => "tick",
            Self::Extract => "extract",
            Self::Acquire => "acquire",
            Self::Encode => "encode+submit",
            Self::Present => "present",
        }
    }

    /// The phase's index into a [`FrameRecord`]'s durations.
    const fn index(self) -> usize {
        match self {
            Self::Tick => 0,
            Self::Extract => 1,
            Self::Acquire => 2,
            Self::Encode => 3,
            Self::Present => 4,
        }
    }
}

/// Whether a frame has somewhere to draw.
///
/// A window that is occluded, whose swapchain timed out, or whose swapchain has
/// just been rebuilt has nowhere, and that is an ordinary state rather than an
/// error. The
/// loop skips the drawing for that frame and says so in the [`FrameRecord`],
/// because a run that counted such frames as delivered would be reporting a rate
/// it did not achieve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquisition {
    /// There is somewhere to draw. [`FrameHost::encode`] and
    /// [`FrameHost::present`] will both run.
    Ready,
    /// There is not. Neither will run this frame.
    Unavailable,
}

/// The work a frame is made of, as five steps the loop calls in order.
///
/// Deliberately opaque. None of these methods says what it touches, because this
/// crate may not know: the runner's implementation ticks a world, reads it into
/// a buffer of scalars and hands that to a renderer, and none of those three
/// types can be named here. What the trait fixes is that they happen, in this
/// order, once per frame.
pub trait FrameHost {
    /// Whatever any of the five steps can fail with.
    type Error;

    /// Advances the simulation by exactly one tick.
    ///
    /// Called once per due tick, so a frame that owes three ticks calls this
    /// three times before anything else runs. It is never called with a
    /// fractional or variable step: that is the whole point of the accumulator
    /// upstream of it.
    fn tick(&mut self) -> Result<(), Self::Error>;

    /// Reads the state the last tick left into whatever the drawing needs.
    fn extract(&mut self) -> Result<(), Self::Error>;

    /// Obtains somewhere to draw, if there is one.
    fn acquire(&mut self) -> Result<Acquisition, Self::Error>;

    /// Records the drawing and hands it to the driver.
    ///
    /// Only called after [`Self::acquire`] answered [`Acquisition::Ready`].
    fn encode(&mut self) -> Result<(), Self::Error>;

    /// Hands the finished frame over to be shown.
    ///
    /// Only called after [`Self::encode`] returned successfully.
    fn present(&mut self) -> Result<(), Self::Error>;
}

/// What one frame did and what each part of it cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameRecord {
    /// Wall time from the previous frame's start to this one's.
    ///
    /// This is what was handed to the accumulator, and it is the whole interval
    /// including any waiting — not the sum of the phases below. A loop that fed
    /// the accumulator its *work* time instead would run the simulation slower
    /// than wall time for as long as it drew anything at all, and nothing in the
    /// state would show it.
    pub frame_time: Duration,
    /// How many simulation ticks ran, after the accumulator's cap.
    pub ticks: u32,
    /// Whether there was somewhere to draw.
    pub drawn: bool,
    /// Each phase's duration, indexed by [`FramePhase`].
    ///
    /// The two drawing phases of a frame with nowhere to draw are
    /// [`Duration::ZERO`], because neither was ever written. [`FramePhase::Tick`]
    /// is not zero by *construction* even when no tick ran: its two readings
    /// bracket the accumulator's own bookkeeping as well as the ticks, so a
    /// frame that owed none still charges whatever the clock resolved in
    /// between — which is very often nothing at all. That bookkeeping is
    /// shorter than a real clock's granularity, so both readings land on one
    /// tick of the counter and differ by zero.
    ///
    /// **A zero here therefore means "below what the clock could resolve", not
    /// "the phase did not run".** `ticks` and `drawn` are what answer that.
    durations: [Duration; 5],
}

impl FrameRecord {
    /// What `phase` cost in this frame.
    #[must_use]
    pub fn phase(&self, phase: FramePhase) -> Duration {
        self.durations[phase.index()]
    }

    /// The sum of [`FramePhase::WORK`]: what the engine itself spent.
    #[must_use]
    pub fn work(&self) -> Duration {
        FramePhase::WORK
            .into_iter()
            .map(|phase| self.phase(phase))
            .sum()
    }
}

/// Runs the frame loop, one frame per call.
///
/// One frame per call rather than an owned `while`, because the caller that
/// matters is a windowing library: `winit` runs the loop and hands control back
/// through callbacks, so a loop that owned its own would have nowhere to live.
/// [`Self::run_frames`] adds the `while` back for callers that do not have one.
#[derive(Debug, Clone)]
pub struct FrameLoop {
    timestep: FixedTimestep,
    /// When the previous frame started, or `None` before the first one.
    previous: Option<Duration>,
}

impl FrameLoop {
    /// A loop driven by `timestep`.
    #[must_use]
    pub fn new(timestep: FixedTimestep) -> Self {
        Self {
            timestep,
            previous: None,
        }
    }

    /// The accumulator this loop drives, for a caller that needs its step.
    #[must_use]
    pub fn timestep(&self) -> &FixedTimestep {
        &self.timestep
    }

    /// Runs one frame: every due tick, then the extraction, then the drawing.
    ///
    /// # The order, which is the point
    ///
    /// Ticks first, all of them, then exactly one extraction and at most one
    /// drawing. Two properties follow, and what this module's tests assert of
    /// them is order and count — that the right *data* flowed is the runner's to
    /// check, for the reason the module doc gives:
    ///
    /// - **Nothing is drawn before it is simulated.** The extraction sees the
    ///   state the last tick of *this* frame left, never a state from between
    ///   two ticks.
    /// - **A frame draws once.** Whether the frame owed no ticks, one, or the
    ///   accumulator's whole cap, [`FrameHost::extract`] runs exactly once. A
    ///   frame that owed nothing still draws, and draws the state the previous
    ///   tick left.
    ///
    /// # The first frame
    ///
    /// It has no predecessor to measure against, so its `frame_time` is zero,
    /// and no tick comes due from an accumulator that is still empty — which is
    /// what every caller here hands over. It draws, which is what puts something
    /// on screen before the first tick rather than a frame of black.
    ///
    /// # Errors
    ///
    /// Whatever `host` returns. A failed phase stops the frame where it failed:
    /// the phases after it do not run, and the frame produces no record. An
    /// acquired frame that fails to encode is therefore dropped rather than
    /// presented, which is the correct handling — presenting an unfinished frame
    /// would show it.
    pub fn step<H, C>(&mut self, clock: &mut C, host: &mut H) -> Result<FrameRecord, H::Error>
    where
        H: FrameHost,
        C: Clock + ?Sized,
    {
        let started = clock.now();
        // Saturating rather than panicking: a clock that went backwards is a
        // broken clock, and taking the process down in the frame loop is worse
        // than a zero that shows up in the table as an impossible duration.
        let frame_time = match self.previous {
            Some(previous) => started.saturating_sub(previous),
            None => Duration::ZERO,
        };
        self.previous = Some(started);

        let ticks = self.timestep.advance(frame_time);

        let mut durations = [Duration::ZERO; 5];
        let mut mark = started;
        // Closure-free so that `host` stays borrowed once: each phase runs, then
        // the clock is read and the gap since the last reading is banked.
        macro_rules! finish {
            ($phase:expr) => {{
                let now = clock.now();
                durations[$phase.index()] = now.saturating_sub(mark);
                mark = now;
            }};
        }

        for _ in 0..ticks {
            host.tick()?;
        }
        finish!(FramePhase::Tick);

        host.extract()?;
        finish!(FramePhase::Extract);

        let acquisition = host.acquire()?;
        finish!(FramePhase::Acquire);

        let drawn = match acquisition {
            Acquisition::Ready => {
                host.encode()?;
                finish!(FramePhase::Encode);

                host.present()?;
                finish!(FramePhase::Present);
                true
            }
            Acquisition::Unavailable => false,
        };

        // The last phase to run assigns `mark` and nothing reads it afterwards.
        // That is the macro being uniform rather than a lost reading.
        let _ = mark;

        Ok(FrameRecord {
            frame_time,
            ticks,
            drawn,
            durations,
        })
    }

    /// Runs `frames` frames, collecting every record.
    ///
    /// # Errors
    ///
    /// As [`Self::step`], at the frame that failed. Records from the frames
    /// before it are dropped with it; a partial run is not a measurement.
    pub fn run_frames<H, C>(
        &mut self,
        clock: &mut C,
        host: &mut H,
        frames: usize,
    ) -> Result<FrameLog, H::Error>
    where
        H: FrameHost,
        C: Clock + ?Sized,
    {
        let mut log = FrameLog::with_capacity(frames);
        for _ in 0..frames {
            log.push(self.step(clock, host)?);
        }
        Ok(log)
    }
}

/// Every frame of a run, and the distribution questions asked of them.
///
/// A distribution rather than a mean, because the criterion in
/// `ProjektPlan.md` §6/M3 is about frames that arrive on time: an average hides
/// exactly the frames that do not, and a run whose mean is comfortable can still
/// miss one frame in fifty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameLog {
    records: Vec<FrameRecord>,
}

impl FrameLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty log sized for `frames`.
    #[must_use]
    pub fn with_capacity(frames: usize) -> Self {
        Self {
            records: Vec::with_capacity(frames),
        }
    }

    /// Adds one frame.
    pub fn push(&mut self, record: FrameRecord) {
        self.records.push(record);
    }

    /// Every frame, in the order they ran.
    #[must_use]
    pub fn records(&self) -> &[FrameRecord] {
        &self.records
    }

    /// How many frames the log holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no frame has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// How many frames had somewhere to draw.
    #[must_use]
    pub fn drawn(&self) -> usize {
        self.records.iter().filter(|record| record.drawn).count()
    }

    /// The total number of simulation ticks across every frame.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.records
            .iter()
            .map(|record| u64::from(record.ticks))
            .sum()
    }

    /// Drops the first `frames` records, which is how a warm-up is discarded.
    ///
    /// The first frames of a run are not like the others — buffers are being
    /// allocated for the first time, caches are cold, and a swapchain is filling
    /// — so a distribution that includes them describes the start rather than
    /// the run. Discarding is explicit and counted rather than silent: the
    /// number of frames dropped belongs in the report beside the table.
    pub fn discard_warmup(&mut self, frames: usize) {
        let dropped = frames.min(self.records.len());
        self.records.drain(..dropped);
    }

    /// The `quantile` of `phase` across every frame, by nearest rank.
    ///
    /// **Nearest rank on the sorted sample, not an interpolating definition.**
    /// The value returned is therefore always one that was actually measured,
    /// which is what a reader checking a table against a raw dump expects; an
    /// interpolated p99 is a number no frame took. `quantile` is clamped to
    /// `0.0..=1.0`, and `1.0` is the maximum.
    ///
    /// `None` for an empty log — a percentile of nothing is not zero.
    #[must_use]
    pub fn quantile(&self, phase: FramePhase, quantile: f64) -> Option<Duration> {
        self.quantile_of(quantile, |record| record.phase(phase))
    }

    /// The `quantile` of the per-frame work sum, by the same definition.
    #[must_use]
    pub fn work_quantile(&self, quantile: f64) -> Option<Duration> {
        self.quantile_of(quantile, FrameRecord::work)
    }

    /// The `quantile` of the frame-to-frame interval, by the same definition.
    #[must_use]
    pub fn frame_time_quantile(&self, quantile: f64) -> Option<Duration> {
        self.quantile_of(quantile, |record| record.frame_time)
    }

    /// Shared body of the three quantile questions.
    fn quantile_of(
        &self,
        quantile: f64,
        of: impl Fn(&FrameRecord) -> Duration,
    ) -> Option<Duration> {
        if self.records.is_empty() {
            return None;
        }

        let mut values: Vec<Duration> = self.records.iter().map(&of).collect();
        values.sort_unstable();

        let quantile = quantile.clamp(0.0, 1.0);
        // Nearest rank: the smallest index whose position covers the quantile.
        // `ceil(q * n)` counts from one, so one is taken off for the index. The
        // clamp runs *before* that subtraction and is what keeps q = 0 in range:
        // its rank is 0, the clamp lifts it to 1, and the index is 0.
        #[expect(
            clippy::cast_precision_loss,
            reason = "the count is a frame count; f64 is exact to 2^53 frames"
        )]
        let rank = (quantile * values.len() as f64).ceil();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rank is in 0..=len by construction, having been clamped"
        )]
        let index = (rank as usize).clamp(1, values.len()) - 1;

        Some(values[index])
    }

    /// The largest value `phase` took.
    #[must_use]
    pub fn max(&self, phase: FramePhase) -> Option<Duration> {
        self.quantile(phase, 1.0)
    }

    /// How many frames took longer than `budget` from start to start.
    ///
    /// **A count of intervals, not a driver statistic.** Nothing in the graphics
    /// API this engine uses reports a missed vertical blank, so a frame that
    /// arrived late is recognised here by having taken longer than it should
    /// have — which is a proxy, and a report using it has to say so.
    #[must_use]
    pub fn frames_longer_than(&self, budget: Duration) -> usize {
        self.records
            .iter()
            .filter(|record| record.frame_time > budget)
            .count()
    }

    /// Total wall time across every frame's interval.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.records.iter().map(|record| record.frame_time).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::{Acquisition, Clock, FrameHost, FrameLoop, FramePhase};
    use crate::time::FixedTimestep;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    /// One shared instant, readable as a [`Clock`] and advanceable by the host.
    ///
    /// The sharing is the point. A clock a test merely scripts can stage what a
    /// frame *cost*, but it cannot stage a frame whose cost depends on the work
    /// the loop decided to do — and that dependency is the whole of the death
    /// spiral. Here the host charges its own time to the same instant the loop
    /// reads, so more ticks really do make a longer frame, which really does
    /// make more ticks come due. The feedback loop is closed rather than
    /// imitated.
    #[derive(Debug, Clone, Default)]
    struct SharedTime(Rc<RefCell<Duration>>);

    impl SharedTime {
        fn advance(&self, by: Duration) {
            *self.0.borrow_mut() += by;
        }
    }

    impl Clock for SharedTime {
        fn now(&mut self) -> Duration {
            *self.0.borrow()
        }
    }

    /// What a host was asked to do, in order.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Call {
        Tick,
        Extract,
        Acquire,
        Encode,
        Present,
    }

    /// A host that records every call and charges a staged cost for it.
    #[derive(Debug)]
    struct Recorder {
        time: SharedTime,
        calls: Vec<Call>,
        available: Acquisition,
        /// What each phase costs, indexed by [`FramePhase`].
        cost: [Duration; 5],
        /// A phase that fails, and the call count at which it starts failing.
        fail_at: Option<Call>,
    }

    impl Recorder {
        fn new(time: &SharedTime) -> Self {
            Self {
                time: time.clone(),
                calls: Vec::new(),
                available: Acquisition::Ready,
                cost: [Duration::ZERO; 5],
                fail_at: None,
            }
        }

        /// Sets what one call of `phase` costs.
        fn costing(mut self, phase: FramePhase, cost: Duration) -> Self {
            self.cost[phase.index()] = cost;
            self
        }

        /// Records the call, charges its cost, and fails if told to.
        fn enter(&mut self, call: Call, phase: FramePhase) -> Result<(), Broken> {
            self.calls.push(call);
            self.time.advance(self.cost[phase.index()]);
            if self.fail_at == Some(call) {
                return Err(Broken);
            }
            Ok(())
        }

        /// How many times `call` was made.
        fn count(&self, call: Call) -> usize {
            self.calls.iter().filter(|made| **made == call).count()
        }
    }

    /// The only way a test host fails.
    #[derive(Debug, PartialEq, Eq)]
    struct Broken;

    impl FrameHost for Recorder {
        type Error = Broken;

        fn tick(&mut self) -> Result<(), Self::Error> {
            self.enter(Call::Tick, FramePhase::Tick)
        }

        fn extract(&mut self) -> Result<(), Self::Error> {
            self.enter(Call::Extract, FramePhase::Extract)
        }

        fn acquire(&mut self) -> Result<Acquisition, Self::Error> {
            self.enter(Call::Acquire, FramePhase::Acquire)?;
            Ok(self.available)
        }

        fn encode(&mut self) -> Result<(), Self::Error> {
            self.enter(Call::Encode, FramePhase::Encode)
        }

        fn present(&mut self) -> Result<(), Self::Error> {
            self.enter(Call::Present, FramePhase::Present)
        }
    }

    /// A loop at 100 Hz, whose step is a round 10 ms.
    fn hundred_hertz() -> FrameLoop {
        FrameLoop::new(FixedTimestep::new(100))
    }

    #[test]
    fn the_first_frame_draws_without_ticking() {
        // Nothing has elapsed before the first frame, so nothing is due - and it
        // still draws, which is what puts an image on screen rather than a frame
        // of black while the accumulator fills.
        let time = SharedTime::default();
        let mut host = Recorder::new(&time);
        let mut clock = time.clone();

        let record = hundred_hertz()
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");

        assert_eq!(record.ticks, 0);
        assert_eq!(record.frame_time, Duration::ZERO);
        assert!(record.drawn);
        assert_eq!(
            host.calls,
            [Call::Extract, Call::Acquire, Call::Encode, Call::Present]
        );
    }

    #[test]
    fn every_due_tick_runs_before_the_extraction() {
        // The order this whole module exists to fix: a frame owing three ticks
        // runs all three, and only then reads the state out. An extraction that
        // ran first would draw a state one tick stale, and an extraction between
        // two ticks would draw a state no tick ever produced.
        let time = SharedTime::default();
        let mut host = Recorder::new(&time);
        let mut clock = time.clone();
        let mut frames = hundred_hertz();

        frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");
        host.calls.clear();

        // 35 ms at a 10 ms step: three ticks due, 5 ms banked.
        time.advance(Duration::from_millis(35));
        let record = frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");

        assert_eq!(record.ticks, 3);
        assert_eq!(
            host.calls,
            [
                Call::Tick,
                Call::Tick,
                Call::Tick,
                Call::Extract,
                Call::Acquire,
                Call::Encode,
                Call::Present
            ],
            "three ticks, then one extraction, then one drawing"
        );
    }

    #[test]
    fn a_frame_that_owes_no_tick_still_draws_the_last_tick_state() {
        // The answer to "what is rendered when no tick is due": the same thing
        // as always, once. Nothing is interpolated and nothing is skipped.
        let time = SharedTime::default();
        let mut host = Recorder::new(&time);
        let mut clock = time.clone();
        let mut frames = hundred_hertz();

        frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");
        host.calls.clear();

        // Four frames of 2 ms each: none reaches the 10 ms step on its own, and
        // the fifth 2 ms crosses it.
        for _ in 0..4 {
            time.advance(Duration::from_millis(2));
            let record = frames
                .step(&mut clock, &mut host)
                .expect("the host was not told to fail");
            assert_eq!(record.ticks, 0, "2 ms cannot fill a 10 ms step");
            assert!(record.drawn, "a frame with no tick due still draws");
        }

        assert_eq!(host.count(Call::Tick), 0);
        assert_eq!(host.count(Call::Extract), 4, "one extraction per frame");
        assert_eq!(host.count(Call::Present), 4, "one presentation per frame");
    }

    #[test]
    fn a_frame_with_nowhere_to_draw_neither_encodes_nor_presents() {
        let time = SharedTime::default();
        let mut host = Recorder::new(&time);
        host.available = Acquisition::Unavailable;
        let mut clock = time.clone();

        let record = hundred_hertz()
            .step(&mut clock, &mut host)
            .expect("an unavailable frame is not an error");

        assert!(!record.drawn);
        assert_eq!(host.calls, [Call::Extract, Call::Acquire]);
        assert_eq!(
            record.phase(FramePhase::Encode),
            Duration::ZERO,
            "a phase that did not run costs nothing"
        );
        assert_eq!(record.phase(FramePhase::Present), Duration::ZERO);
    }

    #[test]
    fn each_phase_is_charged_exactly_what_it_spent() {
        // The decomposition itself, asserted as equalities rather than sampled:
        // every phase costs a different, staged amount, so a duration landing in
        // the wrong slot cannot pass.
        let time = SharedTime::default();
        let mut host = Recorder::new(&time)
            .costing(FramePhase::Tick, Duration::from_millis(1))
            .costing(FramePhase::Extract, Duration::from_millis(2))
            .costing(FramePhase::Acquire, Duration::from_millis(4))
            .costing(FramePhase::Encode, Duration::from_millis(8))
            .costing(FramePhase::Present, Duration::from_millis(16));
        let mut clock = time.clone();
        let mut frames = hundred_hertz();

        frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");

        // A second frame, so that ticks are due and the tick phase is not empty.
        // The first frame ran no ticks, so it spent 2 + 4 + 8 + 16 = 30 ms and
        // the clock stands there: exactly three 10 ms steps are due, and the
        // tick phase costs the three milliseconds those ticks charge.
        let record = frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");

        assert_eq!(record.ticks, 3);
        assert_eq!(record.phase(FramePhase::Tick), Duration::from_millis(3));
        assert_eq!(record.phase(FramePhase::Extract), Duration::from_millis(2));
        assert_eq!(record.phase(FramePhase::Acquire), Duration::from_millis(4));
        assert_eq!(record.phase(FramePhase::Encode), Duration::from_millis(8));
        assert_eq!(record.phase(FramePhase::Present), Duration::from_millis(16));

        assert_eq!(
            record.work(),
            Duration::from_millis(13),
            "work is tick plus extract plus encode - the two handovers are excluded"
        );
        assert_eq!(
            record.frame_time,
            Duration::from_millis(30),
            "the interval is the whole previous frame, waiting included"
        );
    }

    #[test]
    fn a_sustained_overload_stays_at_the_cap_instead_of_spiralling() {
        // The death spiral, with the feedback loop closed and under the only
        // condition that can actually produce one: **a tick that costs more
        // than it simulates.** Each tick charges 15 ms to the same clock the
        // loop measures frames with, against a 10 ms step - so running ticks
        // makes the next frame owe more ticks than the one before. A tick
        // cheaper than its step catches up on its own and proves nothing, which
        // is what a first version of this test measured.
        let time = SharedTime::default();
        let mut host = Recorder::new(&time).costing(FramePhase::Tick, Duration::from_millis(15));
        let mut clock = time.clone();
        let mut frames = FrameLoop::new(FixedTimestep::new(100).with_max_ticks_per_advance(8));

        // The first frame establishes the origin and owes nothing; the backlog
        // is opened after it.
        frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");
        time.advance(Duration::from_millis(100));

        let mut counts = Vec::new();
        let mut lengths = Vec::new();
        for _ in 0..20 {
            let record = frames
                .step(&mut clock, &mut host)
                .expect("the host was not told to fail");
            counts.push(record.ticks);
            lengths.push(record.frame_time);
        }

        assert!(
            counts.iter().all(|ticks| *ticks <= 8),
            "no frame may exceed the cap: {counts:?}"
        );

        // The signature of *not* spiralling: both the tick count and the frame
        // length settle instead of climbing. Eight ticks cost 120 ms and 120 ms
        // owes twelve, which the cap cuts back to eight - a fixed point.
        assert_eq!(
            counts[10..],
            [8; 10],
            "the tick count did not settle at the cap: {counts:?}"
        );
        assert!(
            lengths[10..].iter().all(|length| *length == lengths[10]),
            "the frame length kept moving after it should have settled: {lengths:?}"
        );
    }

    #[test]
    fn without_the_cap_the_same_overload_runs_away() {
        // The negative control, and what makes the test above mean anything.
        // Its fixed point at eight is also what an `advance` returning a
        // constant eight would produce - that passes every assertion there,
        // interval or no interval. So the identical staged scenario runs again
        // with the cap lifted - `with_max_ticks_per_advance` rejects only zero,
        // so `u32::MAX` is a constructible "no cap" - and it has to diverge.
        // The cap is what bounds the run, not arithmetic that never looked at
        // the interval.
        fn overload(cap: u32) -> Vec<u32> {
            let time = SharedTime::default();
            let mut host =
                Recorder::new(&time).costing(FramePhase::Tick, Duration::from_millis(15));
            let mut clock = time.clone();
            let mut frames =
                FrameLoop::new(FixedTimestep::new(100).with_max_ticks_per_advance(cap));

            frames
                .step(&mut clock, &mut host)
                .expect("the host was not told to fail");
            time.advance(Duration::from_millis(100));

            (0..12)
                .map(|_| {
                    frames
                        .step(&mut clock, &mut host)
                        .expect("the host was not told to fail")
                        .ticks
                })
                .collect()
        }

        let capped = overload(8);
        let uncapped = overload(u32::MAX);

        assert!(
            capped.iter().all(|ticks| *ticks <= 8),
            "the capped run exceeded its cap: {capped:?}"
        );

        // Uncapped, every frame owes half again what the last one did: ten ticks
        // cost 150 ms, 150 ms owes fifteen, fifteen cost 225. Nothing bounds it.
        assert!(
            uncapped.windows(2).all(|pair| pair[1] > pair[0]),
            "an uncapped timestep must climb every frame, got {uncapped:?}"
        );
        assert!(
            *uncapped.last().expect("twelve frames") > 100,
            "twelve frames of a 1.5x climb should be far past a hundred ticks, \
             got {uncapped:?}"
        );
        assert!(
            uncapped.iter().sum::<u32>() > capped.iter().sum::<u32>() * 10,
            "the two runs did not diverge, so the cap is not what is being tested: \
             capped {capped:?} against uncapped {uncapped:?}"
        );
    }

    #[test]
    fn the_accumulator_is_fed_the_whole_interval_exactly_once_a_frame() {
        // Two things nothing else pins. First, that `advance` is called once per
        // frame rather than once per phase: at 100 Hz a 25 ms frame owes two
        // ticks, and a loop calling `advance` five times with the same interval
        // would owe ten. Second, that what it is fed is the *interval* and not
        // the work - a loop feeding its own work time would run the simulation
        // slower than wall time for as long as it drew anything, and no state
        // would show it.
        let time = SharedTime::default();
        // The frame spends 20 ms of its 25 in a phase that is not work, which is
        // what a display wait looks like. Feeding work time would owe zero ticks
        // here; feeding the interval owes two.
        let mut host = Recorder::new(&time).costing(FramePhase::Acquire, Duration::from_millis(20));
        let mut clock = time.clone();
        let mut frames = hundred_hertz();

        frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");

        // The first frame cost 20 ms of acquire; add 5 to make the interval 25.
        time.advance(Duration::from_millis(5));
        let record = frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");

        assert_eq!(
            record.frame_time,
            Duration::from_millis(25),
            "the interval is the whole frame, including the waiting"
        );
        assert_eq!(
            record.ticks, 2,
            "25 ms at a 10 ms step is two ticks; more would mean `advance` ran \
             more than once, fewer would mean it was fed the work time"
        );
        assert_eq!(
            record.work(),
            Duration::ZERO,
            "nothing in this frame did any work, and the wait is not work"
        );
    }

    #[test]
    fn an_unrecoverable_stall_discards_its_backlog_rather_than_banking_it() {
        // One enormous frame, then ordinary ones. The stall runs the cap and the
        // rest of its backlog is dropped, so the frame after it is ordinary
        // again. Banking would make every later frame owe the leftovers.
        let time = SharedTime::default();
        let mut host = Recorder::new(&time);
        let mut clock = time.clone();
        let mut frames = FrameLoop::new(FixedTimestep::new(100).with_max_ticks_per_advance(8));

        frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");

        time.advance(Duration::from_secs(1));
        let stall = frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");
        assert_eq!(stall.ticks, 8, "a whole second runs the cap, not a hundred");

        time.advance(Duration::from_millis(10));
        let after = frames
            .step(&mut clock, &mut host)
            .expect("the host was not told to fail");
        assert_eq!(
            after.ticks, 1,
            "the 92 skipped ticks are gone; the next frame is ordinary"
        );
    }

    #[test]
    fn two_runs_of_the_same_script_produce_the_same_log() {
        // The loop itself is deterministic: same staged clock, same host, same
        // record, field for field. Nothing in it reads a clock of its own.
        fn run() -> Vec<super::FrameRecord> {
            let time = SharedTime::default();
            let mut host = Recorder::new(&time)
                .costing(FramePhase::Tick, Duration::from_micros(250))
                .costing(FramePhase::Extract, Duration::from_micros(700))
                .costing(FramePhase::Encode, Duration::from_micros(1_100));
            let mut clock = time.clone();
            let mut frames = hundred_hertz();

            frames
                .run_frames(&mut clock, &mut host, 200)
                .expect("the host was not told to fail")
                .records()
                .to_vec()
        }

        assert_eq!(run(), run(), "two runs of the loop disagreed");
    }

    #[test]
    fn a_failing_phase_stops_the_frame_where_it_failed() {
        // An acquired frame that cannot be encoded must not be presented:
        // presenting it would show whatever the swapchain image happened to
        // hold.
        let time = SharedTime::default();
        let mut host = Recorder::new(&time);
        host.fail_at = Some(Call::Encode);
        let mut clock = time.clone();

        let outcome = hundred_hertz().step(&mut clock, &mut host);

        assert_eq!(outcome.err(), Some(Broken));
        assert_eq!(host.count(Call::Present), 0, "a failed frame is not shown");
    }

    #[test]
    fn quantiles_are_values_that_were_actually_measured() {
        // Nearest rank, so every answer is a sample. Ten frames whose extract
        // phase costs 1..=10 ms make the ranks checkable by hand.
        let mut log = super::FrameLog::new();
        for milliseconds in 1..=10 {
            let mut durations = [Duration::ZERO; 5];
            durations[FramePhase::Extract.index()] = Duration::from_millis(milliseconds);
            log.push(super::FrameRecord {
                frame_time: Duration::from_millis(milliseconds),
                ticks: 0,
                drawn: true,
                durations,
            });
        }

        assert_eq!(
            log.quantile(FramePhase::Extract, 0.5),
            Some(Duration::from_millis(5)),
            "p50 of 1..=10 by nearest rank is the fifth value"
        );
        assert_eq!(
            log.quantile(FramePhase::Extract, 0.99),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            log.max(FramePhase::Extract),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            log.quantile(FramePhase::Extract, 0.0),
            Some(Duration::from_millis(1)),
            "a zero quantile is the smallest sample rather than an index of -1"
        );
        assert_eq!(
            super::FrameLog::new().quantile(FramePhase::Extract, 0.5),
            None,
            "a percentile of no frames is not zero"
        );
    }

    #[test]
    fn a_warmup_is_dropped_by_count_and_late_frames_are_counted() {
        let mut log = super::FrameLog::new();
        for milliseconds in [50_u64, 40, 1, 2, 3] {
            log.push(super::FrameRecord {
                frame_time: Duration::from_millis(milliseconds),
                ticks: 0,
                drawn: true,
                durations: [Duration::ZERO; 5],
            });
        }

        assert_eq!(log.frames_longer_than(Duration::from_millis(10)), 2);

        log.discard_warmup(2);
        assert_eq!(log.len(), 3);
        assert_eq!(
            log.frames_longer_than(Duration::from_millis(10)),
            0,
            "the two long frames were the warm-up"
        );

        log.discard_warmup(99);
        assert!(log.is_empty(), "discarding more than there are empties it");
    }
}
