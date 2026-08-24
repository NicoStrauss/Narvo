# ADR-0003: The fixed timestep discards catch-up ticks

Status: accepted · Date: 2026-08 · Scope: narvo-core (`FixedTimestep`)

## Context

The simulation advances in fixed steps while frames arrive at whatever rate the
machine manages, so `FixedTimestep` banks elapsed time and hands out whole ticks.
A long frame — a stall, a breakpoint, a window drag, a loading hitch — leaves the
accumulator holding many steps' worth of work at once, and something has to bound
how much of that a single call runs. What the bound does with the surplus is the
actual decision: defer it to later frames, or drop it.

## Decision

Past `max_ticks_per_advance` (8 by default, roughly 133 ms of simulation at
60 Hz) the surplus is discarded. Only the sub-step remainder carries over.

## Rationale

1. **Deferring relocates the spiral instead of preventing it.** Ticks carried
   into the next frame make that frame longer, which leaves more backlog, which
   makes the frame after it longer again. A cap that defers is a cap in name
   only: it bounds a single call while letting the accumulator grow without
   bound.
2. **The resulting failure mode is recoverable.** Discarding makes the simulation
   run slow for the length of the stall and then continue normally. Deferring
   makes it run away and never return — the engine becomes unresponsive exactly
   when the machine is already struggling, which is the worst moment to lose it.
3. **The lost work is not worth replaying.** Time is only dropped after a stall
   long enough to exceed the cap, and a frame loop that stalls that long has a
   problem the timestep cannot fix. Replaying the missed ticks trades one
   visible hitch for an invisible cascade.

## Consequences

- Simulated time can fall permanently behind wall time, by exactly the discarded
  amount. Nothing may derive a wall-clock timestamp from accumulated ticks;
  anything needing real elapsed time has to measure it separately.
- Systems must tolerate a discontinuity after a stall: a body that would have
  travelled a metre simply does not travel it. That is what "the simulation ran
  slow" looks like from the inside, and it is the intended behaviour.
- The accumulator works in `Duration` — whole nanoseconds — rather than floating
  point. Integer arithmetic keeps the carry-over exact, which is what lets the
  drift test assert `total_ticks == elapsed / step` as an exact equality instead
  of a tolerance. A float accumulator would make that property only
  approximately checkable, and the checkability is the point.
- Step lengths truncate to whole nanoseconds, so 60 Hz is 16_666_666 ns rather
  than 16_666_666.67. The step runs a hair short and the rate a hair fast: under
  one nanosecond per tick, and identical on every machine.
- The cap is configurable, so a caller that must not drop ticks can raise it.
  That is a decision at the call site, not a property of the type.

## Revision condition

Reopen if the replay and determinism requirements arriving in M2 need different
semantics — most plausibly a headless mode running a fixed tick budget with no
relation to wall time at all, where "falling behind" stops being a meaningful
notion and discarding is simply wrong. That is a new ADR, not an edit to this
one.
