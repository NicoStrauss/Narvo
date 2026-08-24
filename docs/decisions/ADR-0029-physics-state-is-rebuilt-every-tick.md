# ADR-0029: The physics world is rebuilt from registered scalars every tick

Status: accepted · Date: 2026-08-12 · Scope: narvo-physics2d, and every
consumer that puts physics state into the canonical dump

## Context

`narvo-physics2d` puts rapier behind an engine-owned facade in the shape
ADR-0002 uses for hecs. The facade's signature was never the hard part;
ADR-0015 already settled that a subsystem takes scalars rather than a `World`,
and this crate is that decision pointed at physics instead of at the renderer.

The hard part is where the *state* boundary runs, and it is hard because rapier
keeps a great deal between one `PhysicsPipeline::step` and the next. Anything
that survives a step **and** changes what the next step computes is
behaviour-carrying state. If it lives inside rapier rather than inside a
registered component, it is outside the canonical dump — and `ProjektPlan.md`
§5.2 states the consequence without hedging:

> Nicht registrierte Komponenten sind ein harter Fehler — was außerhalb des
> Dumps liegt, liegt außerhalb des Hashes, und eine Abweichung dort wäre
> unsichtbar.

ADR-0008 says the same from the other end: "Anything that is not in the registry
is not in the hash. A component the hash cannot see makes a divergence
invisible."

So the question this ADR answers is not "is rapier deterministic" — M5b.1
measured that, 16 of 16 byte-identical across Windows, WSL, `dev` and `release`.
It is: **what may the engine keep between ticks, and where does it have to
live?**

## What rapier retains, named rather than gestured at

Surveyed from the pinned source of the exact configuration this crate builds
(`rapier2d 0.35.1`, `parry2d 0.30.2`, features `dim2, f32, std, block-solver,
alloc, enhanced-determinism`). The list is not exhaustive and does not need to
be — one entry that carries behaviour and cannot be reconstructed already
decides the question — but it is specific:

| State | Where | Carries behaviour into the next step? |
|---|---|---|
| Per-contact-point warm-start normal impulse | `rapier2d-0.35.1/src/geometry/contact_pair.rs:63`, `pub warmstart_impulse: Real` | **yes** — read back at `src/dynamics/solver/contact_constraint/contact_with_coulomb_friction.rs:171` |
| Per-contact-point warm-start friction impulse | `contact_pair.rs:65`, `pub warmstart_tangent_impulse: TangentImpulse<Real>` | **yes** — read at `contact_with_coulomb_friction.rs:175` |
| Previous step's total normal impulse | `contact_pair.rs:58`, `pub impulse: Real` | **yes**, and not merely as a report: `contact_with_coulomb_friction.rs:187` derives `is_new` from `pt_data(ii).impulse == 0.0`, which selects whether restitution applies to that point |
| The contact manifolds themselves | `contact_pair.rs:221`, `pub manifolds: Vec<ContactManifold>` — passed as `&mut` into the generator each step, so the previous step's contents are this step's input | **yes** |
| Contact and intersection graphs | `src/geometry/narrow_phase/mod.rs:332-334` — `contact_graph`, `intersection_graph`, `graph_indices` | **yes** |
| Incremental solver graph colouring | `narrow_phase/mod.rs:343`, `body_solver_color_masks`, whose own doc says it is "maintained incrementally on contact start/stop, so the solver never recolors its constraint graph from scratch" | **yes** |
| Island assignment and its epoch, BVH optimisation bookkeeping, pending wake-up and join queues | `IslandManager`, `BroadPhaseBvh`, `ImpulseJointSet` | **yes** |

None of it is reconstructible from a body's position, rotation, velocities,
shape and material. That is not this repository's inference — rapier says it
itself, in the comment explaining why solver contacts are serialised even though
they look derivable (`contact_pair.rs:553-560`):

> the solver contacts won't be updated for sleeping bodies. So it means that for
> one frame, we won't have any solver contacts when waking up an island after a
> deserialization. Not only does this break post-snapshot determinism, but it
> will also skip constraint resolution for these contacts during one frame.

And rapier's own snapshot test names the size of the problem: a snapshot has
**nine** parts (`tests/snapshot_roundtrip.rs:28-32`) — gravity, integration
parameters, islands, broad phase, narrow phase, bodies, colliders, impulse
joints, multibody joints. Bodies and colliders are two of the nine.

## Decision

**The facade rebuilds the entire rapier world from the caller's scalars on every
`step`, and keeps nothing between calls.** `Body` — position, rotation as a
`(cos, sin)` pair, linear and angular velocity, shape, material — is the whole
of the state that crosses a tick boundary.

Equivalently and more usefully stated: **there is no physics state outside the
registry.** What the dump cannot see, the simulation does not have.

## The measurement the decision rests on

A throwaway probe outside the repository built both worlds from one scene — 32
bodies at the size of this engine's demo worlds, at `narvo-core`'s own
16 666 666 ns tick — and compared the dumps bit for bit. Full figures in
`target/reports/M5b_2.md`.

- **A rebuilt world and a retained one compute different trajectories.**
  Identical through tick 19, which is free fall; they part company at **tick 20**,
  the first tick that resolves a contact, in one field of one body; 30 of 32
  bodies and 206 of 224 scalars differ by tick 100. The retained state is
  therefore behaviour-carrying in fact, not only on paper.
- **Rebuilding costs about 4.9×.** 10 000 ticks in `release` on the reference
  machine: 141 ms retained against 690 ms rebuilt, five runs each, ranges not
  overlapping. Per tick at 32 bodies that is 0.014 ms against 0.069 ms — 0.4 %
  of a 16.67 ms frame.
- **Losing warm-starting costs settling, not stability.** Neither world explodes
  or tunnels; both keep the stack above the ground. But at tick 10 000 the
  retained world has a total speed of 0.0020 across all bodies and the rebuilt
  one 0.2120 — about a hundred times more residual motion. A rebuilt stack
  jitters where a retained one comes to rest. **This is the real price and it is
  paid knowingly.**
- **The rotation has to be a `(cos, sin)` pair, for two reasons and the second
  is the sharper one.** A world whose rotation is round-tripped through a scalar
  angle every tick leaves the exact one at **tick 2** — before any contact —
  because an angle is `atan2` out and `sin`/`cos` back, and the pair does not
  survive them. That much is a fidelity cost, and it is cross-platform
  consistent: through glam's own `angle`/`from_angle`, which
  `enhanced-determinism` routes via `libm`, all three probe modes were
  byte-identical between Windows and WSL.

  The second reason was found the hard way and is a determinism cost rather than
  a fidelity one. **The standard library's trigonometry is outside the domain
  `enhanced-determinism` establishes.** An earlier draft of this crate offered
  `Body::angle()` written as `f32::atan2`, with a test asserting the round trip
  was lossy; the test **passed on Windows and failed on Linux**, where the same
  round trip came back exact. So the crate offers no angle accessor at all: a
  one-line convenience built on `std` maths would be a platform-dependent
  function inside a crate whose subject is determinism, and no consumer needs it
  yet. Two bare `f32` stay inside ADR-0014 and cost nothing but being written
  down.

  Two things are worth keeping from how that was found. It was the WSL
  pre-check of ADR-0007 that caught it, before CI — which is what that decision
  is for. And the failing assertion was not a defect in the code under test but
  a **test asserting something that was never a property**; a platform-dependent
  claim written as an invariant is the same failure class this project keeps
  finding, in test clothing.

## The rejected paths, with their best argument

**Retained pipeline with a snapshot component.** Its argument is the strongest of
the three and is the measurement above read the other way: it keeps
warm-starting, so stacks settle and the per-tick cost stays at 0.014 ms — the
behaviour a physics engine is supposed to have. Two things defeat it, and the
first alone would.

*It needs a foreign format inside a registered component, which is exactly what
ADR-0014 forbids.* A rapier snapshot is rapier's serde representation of nine
structures; putting it in the dump would make rapier's serialisation format
govern every state hash in the project, which is the hazard ADR-0014 exists to
bound, one level worse than the `glam::Vec2` it was written about. Admitting it
needs an ADR **superseding ADR-0014**, and that is the maintainer's decision, not
a by-product of building a facade. **This is where the M5b.2 prompt's halt branch
fires: the path is reported, not built.**

*And it is not available in this configuration anyway.* rapier's snapshot
facility is plain serde derives behind the opt-in `serde-serialize` feature,
which is not in rapier's `default` (`rapier2d-0.35.1/Cargo.toml:66-71`) and not
in the feature set M5b.1 measured. Under the pinned build every one of those
derives is compiled out. Choosing this path would mean changing the dependency
configuration that M5b.1's evidence was gathered under, and re-measuring it.

**Mixed: retain the pipeline, mirror every behaviour-carrying quantity as scalars
in registered components.** Its argument is that it would get both — full
warm-starting and a complete hash — and the fields are not even private:
`warmstart_impulse` and its friends are `pub`. What defeats it is shape rather
than access. Those impulses are keyed by *contact point within a manifold within
a collider pair*; the contact graph is a graph; the colouring masks are a
per-body incremental structure that exists precisely so it is never rebuilt.
None of that is entity-shaped, so mirroring it means inventing an
entity-component encoding of a pairwise relation and a graph, keeping it exact
against a solver that maintains them incrementally, and writing it back through
an API that offers no path for reconstructing matching contact features. The
honest summary is that the cost was not measured because the construction was
not credible enough to build; that is a judgement and it is marked as one.

**Do nothing and let the pipeline be retained inside the facade, unhashed.** Its
argument is real and is the one that needed the measurement to answer: a replay
under ADR-0012 starts at tick 0 and feeds the same inputs, so a retained pipeline
constructed identically and stepped identically *would* reproduce, and the hole
would never show. What defeats it is that two other things in this project do not
start at tick 0. ADR-0022 reconstitutes the world from the scene file at a tick
boundary on hot reload, and `ProjektPlan.md` §6/M7 requires Save/Load. Both
resume from state, and the probe measured what resuming from state costs when
something is missing: a different trajectory from the first contact onwards.

## Consequences

- **`Body` is the contract.** A quantity that must survive a tick is a field of
  `Body` or it does not exist. Adding one is a deliberate widening, in the same
  way ADR-0015 says a renderer that needs more per sprite widens
  `SpritePlacement`.
- **No body sleeps.** `can_sleep(false)` on every body, because sleeping is
  bookkeeping that would survive a tick and `Body` does not record it. In a world
  rebuilt every tick sleeping could not persist anyway; setting it explicitly
  makes the reason visible at the line rather than implicit in the architecture.
- **`narvo-physics2d` depends on nothing in this workspace.** It follows from
  the decision rather than being chosen beside it: if physics owns no state, it
  needs no `World`, so it is a leaf crate like `narvo-input` and
  `narvo-audio`. The mapping from components to `Body` belongs to whichever
  crate sees both sides, which is ADR-0015's arrangement exactly, and it arrives
  with the first consumer (M5b.4).
- **Two tests carry this ADR, and they catch different things.**
  `a_world_reconstituted_from_its_scalars_continues_identically` is the property
  itself, and it is the one that fires: a demonstration that retained only the
  `NarrowPhase` — the plausible optimisation — produced no panic from rapier and
  was caught by this test alone.
  `the_facade_does_not_retain_solver_state` asserts the facade still *differs*
  from a fully retained pipeline, so a wholesale reversal is noticed; it did
  **not** notice the partial one, and that limit is recorded here rather than
  left for the next reader to discover.
- **The performance figure is a budget item, not a footnote.** 0.069 ms per tick
  at 32 bodies is comfortable; nothing here says what it is at 500. The first
  consumer with a real scene is where that gets measured.
- **`enhanced-determinism` has an enforcement point at last.** It is not in
  rapier's defaults, so dropping it from the manifest would be silent;
  `the_manifest_still_asks_for_enhanced_determinism` reads the manifest at run
  time and fails if the string leaves. What it cannot do is check the flag's
  *effect* — M5b.1 measured that the default configuration produced byte-identical
  output in that scene, so there is no behavioural difference for a test to see.
  The guard is over the declaration, and saying so is part of it.

## Revision condition

**A measurement, not a mood.** Any of:

- the per-tick rebuild cost measured against a real consumer scene exceeds the
  frame budget in `ProjektPlan.md` §8 — the figure above is for 32 bodies and
  says nothing about a scene ten times that;
- the residual motion this decision accepts is shown to be a defect in a
  consumer rather than a property of a toy scene, which is a question for
  M5b.4's scene and not for this one;
- a rapier release exposes a documented way to restore contact and island state
  from values a registered component could hold, which would make the mixed path
  constructible and worth measuring for the first time.

Any of those is an argument for a new ADR with its own evidence. Superseding
ADR-0014 to admit a snapshot component is a separate decision and stays the
maintainer's.
