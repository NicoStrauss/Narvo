# ADR-0031: A run answers an agent at two moments, and the socket is off by default

Status: accepted · Date: 2026-08-13 · Scope: `narvo-app`'s headless runner, its
`ipc` and `transport` modules, `narvo-ipc`'s vocabulary, and anything that later
drives a run from outside it

## Context

M6.1 wrote the agent protocol as data — request and response types, their JSON
spelling, their parse errors — and ADR-0030 records that decision. M6.3a gave it
a **read seam**: one point per tick where a request from that vocabulary meets a
running world. M6.3b added the **write path** and D19's band cut. M6.3c added
**run control**: `Request::Step`, a budget the run reads instead of a constant,
and the delivery instrument that makes any of it observable. M6.3d added the
**transport**.

Two of those additions are not covered by any existing document, and one of them
changes ground another ADR reasons over.

**The transport is the easy half.** D20 was decided by measurement, not taste:
localhost TCP is the only transport `std` offers on both platforms on the pinned
toolchain, because `std::os::windows::net::UnixStream` is behind the unstable
feature `windows_unix_domain_sockets` (rust-lang/rust#150487) and ADR-0009's
reasoning excludes a nightly feature. That decision is already recorded, and
applying it is not an architectural decision of its own.

**The second answering moment is the hard half, and it is why this ADR exists.**
M6.3a chose *one* observation point per tick and argued it from ADR-0011's
Context — the claim that a visibility rule has to be a property that fits in one
sentence and in one test, rather than a consequence of where something sits in a
schedule. M6.3d then added a second observation point, at the wait, and defended
it in a source comment for five sessions. That is exactly the shape ADR-0011
warns about: a delivery rule that grew a second case without the second case
being written down anywhere a reader would look.

## Decision

**A run answers an agent at two moments and no others.**

```rust
pub enum Moment {
    Tick(u64),
    Waiting { ticks_run: u64 },
}
```

`crates/narvo-app/src/ipc.rs:160`. `Moment::Tick(n)` is the drain after tick
*n*'s systems have run (`headless.rs:459`); `Moment::Waiting { ticks_run }` is a
run that has reached its budget and is blocked on its client
(`headless.rs:370-391`). `Moment::ticks_run()` (`ipc.rs:178`) maps both onto one
number, and `Moment::Tick(4).ticks_run() == 5 == Moment::Waiting { ticks_run: 5
}` — the same instant, spelled twice.

**The socket is behind `ipc`, a feature that is off by default**
(`crates/narvo-app/Cargo.toml:49`). It activates no dependency; `Endpoint` is
`std::net` and nothing else. A release binary does not contain it.

**A run's recording is cut to the number of ticks that actually ran**, by
`Recording::cut_to(ticks)` (`recording.rs:168`), not to a tick index.

## Why a wait cannot merely queue

This is the load-bearing part, and it is not a matter of convenience.

The obvious design is one answering moment: a request that arrives while the run
is waiting goes on a queue, and the queue is drained at the next
`Moment::Tick`. It does not work, and the reason is specific rather than
general. **The command that ends a wait is `Request::Step`, and `Step` takes
effect by being answered.** `step` raises the budget (`ipc.rs:509`, `:547`) and
reports what it granted; the runner's wait loop re-tests `tick >= budget`
immediately afterwards. So a `Step` that is merely queued is a `Step` that is
never executed: the run stays below its budget, stays in the wait, and waits for
a message that will only be processed once it has stopped waiting. One
answering moment and a queue is a deadlock, not a slower design.

The alternative — special-casing `Step` so that only *it* is answered during a
wait — was rejected. It would make the protocol's meaning depend on which
variant a request is, and a client could then observe that `GetComponent` and
`Step` are delivered under different rules. The two moments are one rule
applied twice; the special case is two rules that have to be kept in step by
hand.

**A wait is not a tick.** No system runs, no time accumulates, nothing is
appended to the band. That is what makes the second moment cheap to reason
about: the world at `Moment::Waiting { ticks_run: n }` *is* the world at
`Moment::Tick(n - 1)`, byte for byte.

## The measurement each claim rests on

**That the two moments see the same world** was a sentence for five sessions and
is now a test:
`headless::tests::the_wait_answers_against_the_same_world_the_last_tick_did`
(`headless.rs:1936`). The same request is answered twice in two otherwise
identical runs — once at tick 4's drain, once at the wait after the fifth tick —
and the two answers must be the same bytes. Its third assertion is what stops it
being vacuous: the same request answered at tick 3 gives a **different** answer,
so the comparison is between two real observations of a moving simulation.

Red edge (b) measured what happens when the wait is wrong. A wait that does not
wait turns **9 tests of 375** red across two layers. A wait that engages one tick
early turns exactly **1** red — `a_write_during_a_wait_cuts_the_band_at_the_ticks_that_ran`
(`headless.rs:1844`) — which is the honest bound on how well an off-by-one here
is watched.

**That a wait is invisible in everything a run produces** is
`how_long_a_run_waited_is_visible_in_nothing_it_produces` (`headless.rs:2029`):
the same script run twice, once against a client that takes 250 ms per wait and
once against one that takes none, comparing ticks, entities, canonical dump and
recording as a whole. Red edge (c) — leaking the wait's own elapsed time into
the budget — turns that one test red and **no other test in the workspace**.

**That the transport needed a gate** is measured rather than assumed. A loopback
listener has no access check: in the D20 survey a second, unrelated process
connected with no prompt and no credential on both platforms, and under WSL a
*different user* connected to a root-owned listener with nothing in the way. An
AF_UNIX socket at mode 0755 refused that same user with `EACCES`. The gate is the
answer to that measurement, and it follows the form M4.6 and M5.3 already use.

## `cut_to` rather than `cut_after`

A write during a wait invalidates the recording from that point on — D19's band
cut, amended into ADR-0012 by M6.3b. The first spelling of that cut took a tick
*index* and computed `index + 1` internally.

It could not express one case: a write during the wait of a run that had executed
**zero** ticks. There is no tick index for "before the first tick", so the cut
had no way to say "keep nothing". `cut_to(ticks)` takes the number of ticks to
keep, as given, and zero is an ordinary value of it.

The conversion moved code and tests together, which is the failure class where a
sign error hides in both halves, so the guard was checked against something that
did not move: `a_cut_bands_header_says_the_number_it_was_cut_to`
(`recording.rs:850`) asserts against the **rendered header text** — `ticks 4`,
which the format has emitted since ADR-0012 — and a literal, plus a round trip
through the parser. No expectation in it is computed the way `cut_to` computes.

## The wait's liveness signal, and the question left open

A wait ends when a request arrives or when there is nobody left to wait for. Both
come out of one blocking read (`transport.rs:297-358`). There is deliberately no
timeout constant anywhere in that file: a duration chosen there would be a number
that is right on this machine and wrong on a slower runner, and the project has a
standing rule against exactly that. The operating system does the waiting.

**Open, and not decided here: whether a signal that can be missed is a signal.**
A client that vanishes without closing — a killed process on some paths, a
network that is not there on a hypothetical remote transport — produces no read
event at all, and the run waits for ever. Today that is acceptable because the
only clients are on the same machine and `--ipc` is an explicit request to serve
one. It stops being acceptable the moment a transport can lose a peer silently,
and the answer then is a liveness check, which is a timeout by another name and
therefore a decision this ADR does not get to make cheaply. M6.4 is where it
belongs.

## What this does *not* do

**It does not supersede ADR-0003.** A wait accumulates no simulated time, so it
cannot trip the catch-up cap. Headless time comes from a fixed table,
`FRAME_TIMES_US` (`headless.rs:189`), advanced once per frame *inside* the tick
path — a run that waits an hour and a run that waits a microsecond hand the
timestep the same durations in the same order.

**It does not supersede ADR-0011.** ADR-0011 governs events between systems
within the tick loop and is untouched: `Events<E>` still rotates once per tick
and is still in the state hash. What this adds is a second point at which
something *outside* the loop observes the world, which is a case ADR-0011 does
not cover, and it is written down here rather than left as the comment it was.

**It does not supersede ADR-0012.** The recording format is unchanged — the
header still says `ticks N`, the body still holds only the ticks that carry
input. `cut_to` changes the runner's arithmetic, not the bytes.

**It does not decide what a write during a *replay* means.** That surface was
opened by M6.3b and is deliberately still open.

## Consequences

- **A second answering moment is a second thing to keep honest.** Any future
  change to when a run answers has to satisfy
  `the_wait_answers_against_the_same_world_the_last_tick_did`, and that test is
  the reason the claim is checkable at all.

- **The deterministic fence is guarded where the wait meets it, and the guard has
  a measured reach.** Red edge (c)'s second stage raised the injected leak's
  threshold above the guard's own 250 ms delay, and **all 375 tests went green**.
  So the guard catches duration leaks coarser than a quarter of a second and is
  blind to finer ones: a leak of a few milliseconds per wait would pass. This is
  stated here rather than only in a report because it is a known limit of the
  evidence, and a limit of the evidence belongs beside the decision it supports.

- **`--ipc` with nobody attached waits for ever, by design.** That is what asking
  to serve means. A run *without* the flag has no listener at all, whatever the
  feature says, and ends as it always did — the property every existing
  invocation depends on, and one that has its own test.

- **The verification set grows a tenth step**, `cargo nextest run -p narvo-app
  --features ipc`, because a feature nothing builds is a feature nothing checks.
  It is added in `CLAUDE.md`, `xtask/src/main.rs` and `.github/workflows/ci.yml`
  in one commit, as that section requires, and the existing drift guards hold the
  three together.

- **The transport is cheaply reversible, which is the condition D20 attached to
  choosing TCP.** Nothing in `headless` or `ipc` names `std::net`; the seam is
  `Channel`, four methods wide, and swapping the transport is writing a second
  implementation of it.

- **A socket test is a different kind of test and is treated as one.** These are
  the first tests in the project that drive a second process over a real
  connection, they have flaked twice, and D21 landed them with the flake named
  rather than hidden. The standing instruction — that a red is a first-class
  finding, that the rerun and a bigger timeout are both the wrong move — lives in
  `tests/agent_socket.rs`'s own module documentation.

## Revision condition

Re-examine when any of the following happens:

- **A second client becomes necessary.** One at a time is a deliberate
  simplification; two would need an order between their messages, and that order
  is the operating system's.
- **A transport can lose a peer silently.** Anything beyond this machine reopens
  the liveness question above, and with it the timeout this design does not have.
- **A third answering moment is proposed.** Two are defensible because they are
  one rule applied twice. Three would need an argument that this ADR does not
  contain.
- **The duration-leak guard's reach stops being enough.** It is bounded by its
  own 250 ms delay; a finer leak would need a different instrument, not a smaller
  number.
