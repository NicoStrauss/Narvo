# ADR-0010: Seeded randomness is world state, and the generator is written out

Status: accepted · Date: 2026-08 · Scope: narvo-ecs (`Rng`), and every
simulation that draws a random number

## Context

M2 needs randomness that two runs can agree on. That is three separate
questions, and answering only the first one is how a determinism guarantee ends
up with a hole in it:

1. Where do the numbers come from — a crate, or code in this repository?
2. Where does the generator's state live?
3. Which crate owns the type?

ADR-0008 already answered the first question for the state hash, for reasons
that transfer unchanged. The second question is the one this milestone actually
turns on, and it has a wrong answer that looks entirely reasonable.

## Decision

**PCG-XSH-RR 64/32, written out in `crates/narvo-ecs/src/rng.rs`. The
generator's state is an ordinary component, registered like any other, and
therefore inside the canonical dump and the state hash.**

A simulation that wants randomness puts an `Rng` on an entity and registers it.
There is no global generator, no thread-local, no `Rng` handed to systems out of
band.

## Rationale

1. **A generator outside the dump makes divergence invisible.** This is the
   whole decision; the rest is detail. Held in a local, a closure capture or a
   `static`, the generator's state is something two runs can disagree about
   while every component still matches — so the hash reports agreement at the
   exact moment the runs have diverged, and reports it again for every tick
   until the difference finally reaches a component. By then the dump names a
   position, not a cause. It is the same defect as a component the registry does
   not know about, which `canonical_dump` already refuses to tolerate, and it
   deserves the same answer. Put the state in the world and a diff of two dumps
   points at `rng (state:…)` on the first tick that differs.

2. **Written out rather than pulled in, for ADR-0008's reason.** The promise
   wanted from this type is stability: the same seed produces the same sequence
   next year. That promise comes from the constants being frozen in a file in
   this repository, where changing them is a visible diff next to the argument
   for them. A dependency puts it one version bump away — and `rand` in
   particular has changed its generator defaults across releases, which would
   silently move every recorded state without a line of engine code changing.
   The code is about twenty lines of wrapping integer arithmetic.

3. **PCG because its correctness is checkable against something outside this
   repository.** The reference implementation publishes an exact seed and the
   output it produces, so `rng.rs` can assert that seeding with `(42, 54)`
   yields the six documented values. That is not a nicety: the two constants in
   this file are one transposed digit away from a generator that would still be
   deterministic, still pass every two-run comparison in the workspace, and
   still be wrong — because a two-run comparison compares two runs of the same
   wrong code. Only a value from outside catches it. Algorithms without a
   published expected output were rejected for that reason alone, not on
   statistical grounds.

4. **`narvo-ecs` rather than `narvo-core`, on three counts.** The state is a
   *component*, and components are `narvo-ecs`' subject — `narvo-core` owns
   the primitives that live *outside* the world, which is exactly what
   `FixedTimestep` is and exactly what this is not. A registered component must
   be serializable, and `narvo-core` is deliberately dependency-free
   (`crates/narvo-core/Cargo.toml`), so putting it there would mean adding
   serde to the foundation crate to serve a type that only makes sense one layer
   up. And it keeps this decision next to its sibling in ADR-0011: an event
   buffer needs `World`, which `narvo-core` cannot see, so the two would
   otherwise be split across crates for no reason.

   This contradicts the crate table in `CLAUDE.md`, which listed "events, seeded
   RNG" under `narvo-core`. That table was written in M0 as a plan, before
   either existed and before the requirement that both be visible to the state
   hash. The table has been corrected to match; this ADR is the record of why.

## Consequences

- **Integer generation only.** `next_u32` and `next_u32_below` exist; there is
  no float generation, deliberately. A uniform `f64` in `0..1` *is* derivable
  exactly — `(x >> 11) as f64 * 2f64.powi(-53)` is a 53-bit integer scaled by a
  power of two, so every step is exact and platform-independent — and it will be
  added when something needs it. It is not added now because the demo does not
  use it, and because keeping the `chance` simulation integer-only means a
  cross-platform divergence there cannot be blamed on float rounding. The
  floating-point question is asked once, by `motion`, on purpose.
- **The seeding procedure is load bearing.** `with_stream` reproduces the
  reference implementation's `pcg32_srandom_r` step for step, including the two
  discarded draws. Simplifying it would change every sequence and break the
  published test vector, which is the point of having one.
- **Every draw is a state change that has to be written back.** A system draws
  through `&mut Rng` obtained from the world; there is no way to draw without
  the world seeing it. That is more verbose than a global and it is the property
  being bought.
- **Two subsystems that must not couple need two streams**, not two seeds. Same
  seed, different stream is the supported way to get independent sequences;
  sharing one generator means the order in which two systems draw becomes part
  of both their behaviours.
- The generator's serialized form — `(state:…,increment:…)` — is observable
  surface, like every other component's. Changing the field names or their order
  changes every state hash of every simulation that uses one.

## Revision condition

Reopen if a simulation needs a generator whose period or dimensionality this one
cannot serve — PCG32 has a 2⁶⁴ period per stream, which is not a constraint any
milestone through M7 approaches — or if float generation turns out to need a
construction that is not exactly specified, which would be a finding worth an
ADR of its own rather than a quiet helper.

Not a reason to reopen: wanting the convenience of a global generator.
