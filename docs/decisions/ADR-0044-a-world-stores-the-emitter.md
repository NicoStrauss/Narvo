# ADR-0044: A world stores the emitter, not the particles

Status: accepted · Date: 2026-08 · Scope: `narvo-ecs` (the `Burst` component
and the closed form over it) and `narvo-view2d` (the extraction that draws it)

## Context

`ProjektPlan.md` §6/M6b's seventh post asks one question and reserves a number
for it. **D22: are particles simulation state?** The plan states both sides and
says the answer belongs before the build, with a measurement.

*For:* `Layer`'s rejected alternative — "render state outside the World" — fell
because a replay does not reproduce it and ADR-0008's hash cannot notice it. The
same argument applies here word for word.

*Against, and it is an arithmetic rather than an opinion:*

> zweitausend Partikel sind zweitausend Entities im kanonischen Dump, und der
> Dump ist das Orakel des Repro-Läufers (ADR-0035) und wird über den Draht
> getragen (ADR-0036) — **ein Dump, der um zwei Größenordnungen wächst, macht
> das Instrument langsam, das M6 gerade gebaut hat.**

The measurement was made first, against a rule registered before the numbers
existed (the D2 / ADR-0028 precedent). It is in `target/reports/M6b.7.md` and in
the sealed `target/reports/M6b.7-predictions.md`; the figures this ADR rests on
are quoted where they are used.

## Decision 1 — particles are simulation state, and the state is the emitter

**Yes, and less than the question assumed.** A world stores a [`Burst`]: how
many particles, which directions they leave in, how old the burst is, how long
it lives, how fast and how wide. Six bare scalars, one registered component,
stable name `burst`, the fourteenth in `register_engine_components`.

Everything the *for* side wants follows from that and needs no further argument:
the emitter is in the canonical dump, therefore in ADR-0008's state hash,
therefore in an ADR-0043 save, therefore readable and writable over ADR-0030's
protocol, therefore replayed by any recording that replays the run.

**The particles themselves are not stored, anywhere, ever.**
`Burst::particle(index)` computes where one is and how bright, from the
component alone.

### The halt branch did not fire

§6/M6b halts the task if the answer needs **a third state class** beside "World"
and "outside". It does not. The emitter is world state; the particles are a
*function* of world state, which is not a class of state at all. The repository
already carries the pattern one level down: `Wander` is a registered component
and `wander_at(wander, tick)` is a closed form over it
(`crates/narvo-app/src/sim/scene.rs:162`). This applies it to drawables rather
than to a component value.

## Decision 2 — the emitter carries its **age**, and the tick is not a parameter

The obvious spelling of a derived particle is a **birth tick** on the component,
with the position derived from `tick - born`. **It was considered and rejected
during design, and the reason is a measurement M6b.6 already made: the tick is
not world state.** It is a local in `headless::run` and a field on
`SceneHost`.

A picture derived from `tick - born` would therefore depend on something the
canonical dump does not contain. Two worlds with identical dumps would draw
differently, and ADR-0008's hash could not see it — which is precisely the
objection that sank "render state outside the World" when `Layer` was decided,
wearing a different hat. Choosing the birth tick would have been rejecting
candidate (d) and then building it.

So `Burst::age` is a field, `advance_bursts` is the one system that moves it, and
**the extraction is a pure function of the world** — no tick parameter, and no
change to the shape of the seam ADR-0041 cut. `Shake::arm` / `Shake::at_rest` is
the pattern; `Burst::arm` / `Burst::spent` is the same pair.

**The age saturates at `life`** rather than running on. That is not a detail: a
counter that keeps moving while the picture is frozen moves the canonical dump
for ever and proves nothing, which is exactly what M6b.5 measured about an
animation hold counter. `a_finished_burst_stops_moving_the_state_hash` holds
one half and `a_running_burst_moves_the_state_hash` holds the other, because
without the second the first passes for a burst that never ages at all.

## Decision 3 — the particles are drawn where draw order is already decided

`narvo_view2d::regions_of` emits an entity's own sprite **and one copy of it per
live particle**. Not a second function whose output a caller concatenates:
appending a separate list draws it last, and "last" is a new ordering rule in the
one place ADR-0015 and ADR-0041 both put draw order on purpose. A burst behind a
wall has to be behind the wall.

**A world with no live burst comes out exactly as it did**, and that is a
property of the sort key rather than of a branch. The key is *(depth, entity,
slot)*, where slot is `0` for an entity's own sprite and `index + 1` for its
particles; an entity without a burst has slot `0` alone, so the key degenerates
to the *(depth, entity)* it always was.
`a_world_without_a_burst_draws_exactly_what_it_drew` measures that against a
world holding a spent burst, and
`physics_drop_tick45_128x128` — the blessed reference drawn through this
function — did not move.

`placements_of` is **not** touched and does not see a burst
(`placements_of_does_not_see_a_burst`). That is M4.8's constraint, unchanged: the
whole-texture arm of `SceneHost::extract` emits what it always did.

A particle inherits the emitter's `Tint` with the fade multiplied into **the
alpha alone**. `SpriteTint::premultiplied` is `[r·a, g·a, b·a, a]`, so a fade in
`0.0 ..= 1.0` leaves the colour channels where the content put them and keeps
ADR-0023's invariant `rgb ≤ a` intact at every step. **A fade is not a glow**:
this reaches nothing above 1.0 and leaves that named limit exactly where
`Tint`'s own doc comment and
`a_tint_above_one_is_the_named_limit_and_not_a_promise` leave it.

## Decision 4 — what a computed particle cannot do, named before it is discovered

| mechanism | needs | a computed particle |
|---|---|---|
| `hit_test` (ADR-0027) | an entity with `HitRect` and `Transform` | **cannot be clicked** |
| `GetComponent` / `SetComponent` (ADR-0030) | an `EntityName` and a registered component | **cannot be addressed**; the emitter can |
| `ListEntities` | an entity | **does not appear** |
| physics (ADR-0029) | an entity with `RigidBody` | **cannot collide** |
| individual death | per-particle state | **impossible** |

The sharpest of these, stated rather than left to be met: **a spark that dies
because something hit it is ruled out.** Nothing can shorten one particle's life,
because there is no one particle to shorten. A burst as a whole can be armed,
aged, ended, saved, inspected and written over the wire like any other component;
a single spark meeting a wall cannot.

That price is the decision, not an oversight. `ProjektPlan.md` §2 forbids
building on spec, and every capability in the table above is one **no named
consumer has asked for**.

## The rejected candidates, each with its best argument

### (a) One entity per particle

*Its best argument, and it is strong.* It needs **no new mechanism in any
machinery this engine has.** A particle is `Transform` + `Layer` + `Sprite` +
`Tint` and a lifetime; `regions_of` already draws it, the existing total order
*(depth, then `EntityId`)* already covers it with no new rule, `hit_test` already
sees it, ADR-0043 already saves it, ADR-0030 already addresses it — and the repro
oracle's answer to one wrong particle in two thousand is **65 bytes naming the
entity and the component**, the best diagnosis of any candidate measured. It is
also the only candidate whose cost could be measured end to end without building
anything.

*Where it falls.* On §2, and on the arithmetic. **177.5 B of dump per particle**,
measured — so the registered size cut-off is crossed at **N ≈ 3 600**, and at
N = 10 000 the dump is 27.8 × the largest this repository produces. Every burst
puts thousands of lines into every dump, every save, every determinism artefact
and every wire answer; and because entity ids are slots, a change to one
particle's lifetime shifts every later entity's slot and generation, so an
unrelated edit reads as a diff of thousands of lines.

**Reopening condition:** the first named consumer that needs a particle to
collide, to be addressed individually over the protocol, or to die because
something hit it. At that point (a) is not a concession but the right answer, and
`target/reports/M6b.7.md` §4 says what it costs.

### (b) A component per emitter carrying a field of stored particles

*Its best argument.* It is genuinely the middle, and the numbers back it: at
N = 2 000 its dump is **1.28 ×** the largest this repository produces against
(a)'s **5.55 ×**, its per-tick cost is 28 × cheaper than (a)'s, it puts one dump
*line* per emitter instead of one per particle, and — unlike (c) — it keeps every
particle individually addressable in principle.

*Where it falls.* On a measurement, not an opinion. **One wrong particle out of
two thousand is reported as 81 723 bytes on a single line.** ADR-0035 chose a
dump over a hash precisely so that a repro does not "throw away the entity and
the component before the reader ever sees them"; under (b) the entity and the
component are named and then 81 kB is handed over, which is the same loss wearing
a better label. And a particle inside a `Vec` is reachable by *nothing* this
engine has — not `hit_test`, not `GetComponent`, not `placements_of` — so the
addressability it keeps "in principle" has no mechanism in practice.

**Reopening condition:** a measured need for stored per-particle state at a count
where (a)'s dump is refused. It should then be built writing **one dump line per
particle** rather than a `Vec` on one line, which removes the objection that
kills it here.

### (d) Outside the world

*Its best argument, and it is the cheapest thing on every axis measured.* 54 B of
dump, zero repro cost, zero hash cost, no registry entry, no scene syntax, no
save format, no ordering rule — and a defensible claim that where a spark sits is
not something a loot-incremental ever reasons about.

*Where it falls.* On the `Layer` argument, word for word: a replay does not
reproduce it and ADR-0008's hash cannot notice it. §6/M6b's own oracle for this
post requires that a replay reproduce *the burst*, and outside the world it would
do so only by the accident of two runs computing the same thing from something
that is not state, with nothing checking that they did.

**Reopening condition:** an effect provably invisible to every oracle this
project has *and* provably unable to affect anything a player or an agent
observes. Particles do not meet it.

## What this does **not** decide

- **A particle budget.** N = 2 000 was the decision point, not a limit. A burst
  is constant in N in the dump; what a frame can *draw* is a throughput question
  this did not ask.
- **Whether particles can ever be entities.** (a)'s reopening condition is a
  real door.
- **Anything about a tint above 1.0.** The named limit is untouched, neither
  clamped nor rejected, and this decision reaches nothing above one.
- **Anything in ADR-0008, ADR-0018 or `Sprite`.** None was opened.

## Named limits

- **The state hash cannot see where a particle is.** It sees the emitter — count,
  seed, age, life, speed, spread — and everything the positions are computed
  from, but not the positions. A defect in `Burst::particle` moves no hash. That
  blind spot is real, is why `tests/burst_is_seen.rs` reads **pixels** in both
  directions, and is the reason that file exists rather than a hash comparison.
- **A burst is the emitter's own sprite, repeated.** It carries no region of its
  own, so an emitter that should not be visible has no way to say so today.
- **Two integer constants decide what a burst looks like.** `MIX_SEED` and
  `MIX_INDEX` are written out in `burst.rs` for the reason `state_hash` writes
  out FNV-1a. They carry no cryptographic claim; what is required of them is
  that neighbouring indices land in unrelated directions, and
  `neighbouring_particles_do_not_leave_together` is what requires it.
- **The window path is untouched and uncovered**, as always.

## Consequences

- `Burst` is the fourteenth registered engine component. Registering it changes
  nothing for a world that has none: the seven pre-change dumps of every mode and
  every committed scene came back **byte-identical**, and the cross-platform
  determinism artefacts compared **26 files identical** either side.
- **`Cargo.lock` did not move by one line**, and the external consumer that sees
  a burst drawn builds against **99 packages** — the same number
  `target/reports/M6b.5.md:175` records for a consumer that needs the world and
  the renderer. A burst costs no package.
- Five transcriptions of the engine component set had to be widened, four of them
  found by their own guards and one by a guard over the specimen scene. That
  cascade is M5b.3a's, met a third time; `crates/narvo-scene/scenes/example.ron`
  carries a `burst` because a coverage guard demands it, not because the format
  does — the same sentence `tint` carries two lines above it.

## Revision condition

Any of the three reopening conditions above. Additionally: if a future
measurement shows the extraction cost of computing particles per frame to
dominate a frame at a count the game needs, the arithmetic that put (c) ahead of
(a) is the arithmetic that would have to be re-run — this decision rests on a
dump that is constant in N, not on the computation being free.
