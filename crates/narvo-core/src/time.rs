//! Fixed-timestep scheduling: turning variable frame durations into a whole
//! number of fixed-length simulation ticks.

use std::time::Duration;

/// Accumulator that converts variable frame durations into a whole number of
/// fixed-length simulation ticks.
///
/// A renderer produces frames at whatever rate the machine manages; a
/// deterministic simulation has to advance in identical steps regardless. This
/// type sits between the two: [`advance`](Self::advance) is told how long the
/// last frame took and answers how many ticks are now due, banking whatever is
/// left over for the next call.
///
/// It never reads a clock itself. Elapsed time is passed in, which is what makes
/// a headless run reproducible - a caller can feed a recorded or synthetic
/// sequence of frame durations and the simulation takes exactly the same path it
/// took when those durations came from a real frame loop.
///
/// # Examples
///
/// ```
/// use narvo_core::FixedTimestep;
/// use std::time::Duration;
///
/// // 60 Hz by default, so one step is roughly 16.7 ms.
/// let mut timestep = FixedTimestep::default();
///
/// // A frame that took a little over two steps: two ticks come due, and the
/// // remaining time stays banked for the next call.
/// let due = timestep.advance(Duration::from_millis(35));
/// assert_eq!(due, 2);
/// for _ in 0..due {
///     // advance the simulation by exactly `timestep.step()` here
/// }
///
/// // The banked remainder places the render path between the tick that just
/// // ran and the one coming up.
/// assert!(timestep.interpolation() > 0.0);
/// assert!(timestep.interpolation() < 1.0);
/// ```
#[derive(Debug, Clone)]
pub struct FixedTimestep {
    /// Length of one simulation tick.
    step: Duration,
    /// Elapsed time not yet consumed by a tick. Always shorter than `step`.
    accumulator: Duration,
    /// Upper bound on the ticks a single `advance` call may return.
    max_ticks_per_advance: u32,
}

impl FixedTimestep {
    /// Tick rate used by [`FixedTimestep::default`].
    pub const DEFAULT_TICK_RATE_HZ: u32 = 60;

    /// Tick cap used unless [`with_max_ticks_per_advance`] overrides it.
    ///
    /// Eight ticks is about 133 ms of simulation at 60 Hz: long enough to absorb
    /// an ordinary hitch without visibly dropping simulation time, short enough
    /// that a real stall is cut off instead of compounding.
    ///
    /// [`with_max_ticks_per_advance`]: Self::with_max_ticks_per_advance
    pub const DEFAULT_MAX_TICKS_PER_ADVANCE: u32 = 8;

    /// Creates an accumulator running at `tick_rate_hz` ticks per second.
    ///
    /// The step length is one second divided by `tick_rate_hz`, truncated to
    /// whole nanoseconds, so rates that do not divide a second evenly - 60 Hz
    /// among them - run a hair fast. That error is under one nanosecond per tick
    /// and, more importantly, is identical on every machine.
    ///
    /// # Panics
    ///
    /// Panics if `tick_rate_hz` is zero. A simulation that can never tick is a
    /// configuration mistake, and failing here names it directly instead of
    /// leaving a silently frozen engine to be diagnosed from its symptoms.
    #[must_use]
    pub fn new(tick_rate_hz: u32) -> Self {
        assert!(
            tick_rate_hz > 0,
            "tick rate must be at least 1 Hz, got 0 - a zero rate would never tick"
        );

        Self {
            step: Duration::from_secs(1) / tick_rate_hz,
            accumulator: Duration::ZERO,
            max_ticks_per_advance: Self::DEFAULT_MAX_TICKS_PER_ADVANCE,
        }
    }

    /// Sets how many ticks a single [`advance`](Self::advance) call may return.
    ///
    /// # Panics
    ///
    /// Panics if `max_ticks` is zero, which would stop the simulation entirely
    /// while leaving the frame loop running.
    #[must_use]
    pub fn with_max_ticks_per_advance(mut self, max_ticks: u32) -> Self {
        assert!(
            max_ticks > 0,
            "max ticks per advance must be at least 1, got 0 - a zero cap would never tick"
        );

        self.max_ticks_per_advance = max_ticks;
        self
    }

    /// Length of a single simulation tick.
    #[must_use]
    pub fn step(&self) -> Duration {
        self.step
    }

    /// Largest number of ticks one [`advance`](Self::advance) call can return.
    #[must_use]
    pub fn max_ticks_per_advance(&self) -> u32 {
        self.max_ticks_per_advance
    }

    /// Adds `frame_time` to the accumulator and reports how many simulation
    /// ticks are now due.
    ///
    /// Time below one full step is banked and carries over, so nothing is lost
    /// between calls: the running total of ticks stays pinned to the total
    /// elapsed time no matter how unevenly that time arrives.
    ///
    /// A single call never returns more than
    /// [`max_ticks_per_advance`](Self::max_ticks_per_advance). When the cap is
    /// reached the surplus is *discarded* rather than deferred. Catching it up
    /// on a later frame would make that frame longer, which makes the frame
    /// after it longer again - the death spiral this cap exists to prevent.
    /// Simulated time falls behind wall time by the discarded amount and stays
    /// there, which is the intended trade: a simulation that runs slow is
    /// recoverable, one that never catches up is not.
    #[must_use = "the returned count is the simulation work that is due; \
                  discarding it silently skips those ticks"]
    pub fn advance(&mut self, frame_time: Duration) -> u32 {
        self.accumulator = self.accumulator.saturating_add(frame_time);

        let step_nanos = self.step.as_nanos();
        let due = self.accumulator.as_nanos() / step_nanos;

        // The sub-step remainder always carries over - that is what keeps the
        // accumulator free of drift. Whole steps beyond the cap are dropped here
        // rather than left banked, because banking them is the spiral.
        let carried_over = u64::try_from(self.accumulator.as_nanos() % step_nanos)
            .expect("a remainder shorter than one step always fits in u64 nanoseconds");
        self.accumulator = Duration::from_nanos(carried_over);

        u32::try_from(due)
            .unwrap_or(u32::MAX)
            .min(self.max_ticks_per_advance)
    }

    /// How far the accumulator currently sits between the tick that last ran and
    /// the next one, as a fraction in `0.0..1.0`.
    ///
    /// This exists for the render path, which draws between two simulation
    /// states: `0.0` means a tick just landed, values approaching `1.0` mean the
    /// next one is imminent. It is an input to rendering only. Feeding it back
    /// into simulation logic would make the simulation depend on frame rate and
    /// break the determinism the fixed timestep is there to provide.
    #[must_use]
    pub fn interpolation(&self) -> f32 {
        // The accumulator is always shorter than one step, so the ratio is in
        // range by construction; the clamp only absorbs f32 rounding at the top.
        (self.accumulator.as_secs_f32() / self.step.as_secs_f32()).clamp(0.0, 1.0)
    }
}

impl Default for FixedTimestep {
    fn default() -> Self {
        Self::new(Self::DEFAULT_TICK_RATE_HZ)
    }
}

#[cfg(test)]
mod tests {
    use super::FixedTimestep;
    use std::time::Duration;

    /// Interpolation is consumed by the render path, so tests compare it with a
    /// tolerance rather than bit-exactly.
    fn assert_close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected interpolation {expected}, got {actual}"
        );
    }

    #[test]
    fn the_default_runs_at_sixty_hertz() {
        let timestep = FixedTimestep::default();

        // One second divided by 60, truncated to whole nanoseconds.
        assert_eq!(timestep.step(), Duration::from_nanos(16_666_666));
        assert_eq!(
            timestep.max_ticks_per_advance(),
            FixedTimestep::DEFAULT_MAX_TICKS_PER_ADVANCE
        );
        assert_close(timestep.interpolation(), 0.0);
    }

    #[test]
    fn an_exact_multiple_of_the_step_produces_exactly_that_many_ticks() {
        let mut timestep = FixedTimestep::new(100); // 10 ms per tick

        assert_eq!(timestep.advance(Duration::from_millis(30)), 3);

        // Nothing is banked, so the render path sits exactly on the tick.
        assert_close(timestep.interpolation(), 0.0);
    }

    #[test]
    fn a_frame_shorter_than_one_step_produces_no_tick() {
        let mut timestep = FixedTimestep::new(100); // 10 ms per tick

        assert_eq!(timestep.advance(Duration::from_millis(9)), 0);
    }

    #[test]
    fn leftover_time_carries_over_between_calls() {
        let mut timestep = FixedTimestep::new(100); // 10 ms per tick

        // Six-millisecond frames: every second call crosses a tick boundary,
        // but only because the remainder from the previous one is still banked.
        assert_eq!(timestep.advance(Duration::from_millis(6)), 0); // banked 6 ms
        assert_eq!(timestep.advance(Duration::from_millis(6)), 1); // 12 ms -> 1, banked 2
        assert_eq!(timestep.advance(Duration::from_millis(6)), 0); // banked 8 ms
        assert_eq!(timestep.advance(Duration::from_millis(6)), 1); // 14 ms -> 1, banked 4
    }

    #[test]
    fn a_long_frame_is_capped_and_the_backlog_is_discarded() {
        let mut timestep = FixedTimestep::new(100).with_max_ticks_per_advance(4);

        // A full second is a hundred ticks' worth of work; only the cap runs.
        assert_eq!(timestep.advance(Duration::from_secs(1)), 4);

        // The 96 skipped ticks are gone rather than deferred: the next ordinary
        // frame behaves as though the stall had never happened. Deferring them
        // is precisely the death spiral the cap exists to prevent.
        assert_eq!(timestep.advance(Duration::from_millis(10)), 1);
    }

    #[test]
    fn interpolation_spans_the_gap_between_two_ticks() {
        let mut timestep = FixedTimestep::new(100); // 10 ms per tick

        // Nothing consumed yet: exactly on a tick boundary.
        assert_close(timestep.interpolation(), 0.0);

        assert_eq!(timestep.advance(Duration::from_millis(5)), 0);
        assert_close(timestep.interpolation(), 0.5);

        // Just short of the next tick.
        assert_eq!(timestep.advance(Duration::from_millis(4)), 0);
        assert_close(timestep.interpolation(), 0.9);

        // Crossing the boundary drops it back down: 1 ms banked of a 10 ms step.
        assert_eq!(timestep.advance(Duration::from_millis(2)), 1);
        assert_close(timestep.interpolation(), 0.1);
    }

    #[test]
    fn uneven_frames_do_not_drift_over_a_long_run() {
        let mut timestep = FixedTimestep::new(60);

        // A deliberately ragged pattern: none of these divides evenly into the
        // 60 Hz step, so every call leaves a different remainder behind. None is
        // long enough to reach the tick cap either, so nothing is legitimately
        // discarded and the totals have to match exactly.
        let frame_times_us = [16_700_u64, 9_100, 33_400, 4_250, 21_900, 12_000];

        let mut elapsed = Duration::ZERO;
        let mut ticks: u64 = 0;
        for frame_us in frame_times_us.iter().copied().cycle().take(6_000) {
            let frame = Duration::from_micros(frame_us);
            elapsed += frame;
            ticks += u64::from(timestep.advance(frame));
        }

        // Whatever the accumulator holds back is always shorter than one step,
        // so the tick total is pinned to the elapsed total - independent of how
        // that time was sliced into frames. Any drift shows up here.
        let expected = u64::try_from(elapsed.as_nanos() / timestep.step().as_nanos())
            .expect("the tick count of a 97-second run fits in u64");
        assert_eq!(ticks, expected);
    }

    #[test]
    #[should_panic(expected = "tick rate must be at least 1 Hz")]
    fn a_zero_tick_rate_is_rejected() {
        let _ = FixedTimestep::new(0);
    }

    #[test]
    #[should_panic(expected = "max ticks per advance must be at least 1")]
    fn a_zero_tick_cap_is_rejected() {
        let _ = FixedTimestep::default().with_max_ticks_per_advance(0);
    }
}
