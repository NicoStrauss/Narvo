# ADR-0002: ECS is hecs behind an engine-owned facade

Status: accepted · Date: 2026-08 · Scope: narvo-core / narvo-ecs

## Context

The engine is data-oriented by principle (flat component schemas, systems as
functions over queries), so the ECS is the architectural center. Options:
build a custom ECS, or adopt an existing crate. A custom ECS is a classic
time sink for solo engine projects; an external one risks leaking a foreign
API into every system an agent will ever touch.

## Decision

Use `hecs` as the ECS storage/query engine, wrapped in an engine-owned
facade. Scheduling (explicit system order), events, and the fixed-timestep
loop are engine code on top — hecs provides storage and queries only.

## Rationale

1. **Avoids the build-your-own trap.** Archetype storage, query iteration and
   borrow-safe access are solved problems; rebuilding them delays every
   milestone that actually validates the AI-first thesis.
2. **The facade keeps the agent-facing API ours.** Systems are written against
   Narvo types and idioms. That keeps documentation, error messages, and
   conventions consistent — and keeps hecs swappable.
3. **hecs is small and unopinionated.** No runtime, no scheduler, no app
   framework — it does not fight the engine's own architecture, and it is
   feasible to hold its entire API in one agent context window.

## Consequences

- Facade discipline: no `hecs` types in public APIs of other crates. Systems,
  components and queries go through the facade. Violations are review
  blockers.
- Determinism requirements (stable iteration order for anything hashed or
  serialized) must be guaranteed at the facade layer, not assumed from hecs
  internals.
- The facade starts inside `narvo-core` and moves to a dedicated
  `narvo-ecs` crate once its API boundary justifies the split (M2).

## Revision condition

A custom ECS may only be considered after M7 (vertical slice) and requires a
new ADR with measured evidence — e.g. the facade provably blocking a needed
capability or a performance budget — not preference.
