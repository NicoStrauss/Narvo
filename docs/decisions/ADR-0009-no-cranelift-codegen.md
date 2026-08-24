# ADR-0009: No Cranelift codegen backend for debug builds

Status: accepted · Date: 2026-08 · Scope: workspace build configuration
(`rust-toolchain.toml`, `[profile.dev]`)

A record of a decision taken. It closes D6 in `ProjektPlan.md` §11, which was
due at M2.

## Context

D6 asked whether debug builds should use the Cranelift codegen backend instead
of LLVM. It was never an open preference: `ProjektPlan.md` §8.2 attached a
condition to it from the start — try it, and *keep* it only if the rebuild
budget of §8.1 would otherwise be missed. The budget is an incremental debug
rebuild after a change in one crate, under five seconds, in force since M2.

## Decision

Cranelift is not introduced. Debug builds keep the default LLVM backend.

## Rationale

1. **The condition did not occur.** Measured on the reference machine: 0.57 s
   for a rebuild after a change in `narvo-ecs` at M2.1, and 2.03 s at M2.2 —
   the latter being the higher figure only because `narvo-app` now depends on
   `narvo-ecs` and rebuilds with it, which is the honest worst case. Both are
   far inside the five-second budget. The other measures in §8.2 are what got it
   there: dependencies compiled once at `opt-level = 3` while workspace crates
   stay at 1, LLD on both hosts, and a headless build that carries three crates
   instead of a hundred and twenty-one.
2. **The toolchain pin stands in the way, and it outranks this.**
   `rust-toolchain.toml` pins an exact stable version so that both platforms
   compile with the same compiler; Cranelift needs nightly. Having both would
   mean a second toolchain or an override on top of the pin. Either one puts a
   compiler difference back into an experiment whose entire purpose is to hold
   the compiler still: M2.4 compares Windows against Linux, and a divergence
   that could be either the platform or the backend answers nothing. The pin is
   load-bearing for the milestone; Cranelift is an optimisation for a budget
   that is not under pressure.

## Consequences

- One fewer moving part in the build. No nightly toolchain, no backend-specific
  configuration, and no second thing to keep in step between the two platforms
  and CI.
- The rebuild budget rests entirely on the §8.2 measures. If they stop being
  enough, that is a real signal rather than a nuisance, and it lands in the
  revision condition below rather than being absorbed silently.
- D6 is closed. `ProjektPlan.md` §11 records it as open until a human carries
  this ADR back into that table.

## Revision condition

Reopen when the incremental rebuild actually breaks the five-second budget
**and** the measures in §8.2 are exhausted — not when it merely grows.

If that happens, the first decision is about the toolchain pin, not about
Cranelift. The pin exists for the cross-platform determinism experiment, so
whether it can be relaxed depends on where that experiment stands; only once
that is settled does a nightly-only backend become a question that can be
answered at all.
