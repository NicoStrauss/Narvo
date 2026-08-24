# ADR-0017: The camera has one composition point

Status: accepted · Date: 2026-08 · Scope: narvo-ecs (`Camera`, `Follow`,
`Shake` and the system that writes them) and every scenario that drives a camera

## Context

M3.12 made the camera a component rather than a render setting, which turned
"what the camera is" into world state, so it enters the canonical dump — and
therefore the state hash — **of any scenario that registers it**. That qualifier
is load-bearing and this repository has recorded getting it wrong twice: being in
the hash is a property of a scenario, not of a type. M3.30 added following: one
system, one writer, no question about ownership. M3.31 added a second
contributor — a shake — and with it the question M3.30 had left open: **who owns
the camera when two behaviours both want to move it.**

That question was decided and built in M3.31; the M3.31 report proposed an ADR
for it and did not write one, for the reason it records — *"Proposed, not
written (the task forbids writing one)"*. This ADR is written in M3.36 and
**records what is in the tree**, not a plan. Every location it names was checked
against the working copy it was written from, and then checked again by an
adversarial pass that found four of them wrong.

## Decision

**Contributors own their state as components; exactly one system writes the
camera; and it writes `camera = base + Σ offsets` from the state the tick has
already left behind.**

Three parts, and all three carry weight:

1. **Each contributor owns its own state.** `Follow` keeps the smoothed point it
   has reached in `Follow::x`/`Follow::y`; `Shake` keeps `amplitude` and `phase`.
   Neither reads the camera back, so neither can be fed its own previous output.
2. **One system writes the camera.** `compose_camera`
   (`crates/narvo-ecs/src/follow.rs:267`, exported at
   `crates/narvo-ecs/src/lib.rs:98`) advances every contributor on an entity
   that carries a `Camera`, then composes: the base is the live `Follow`'s
   smoothed point, or `Shake::base_x`/`base_y` when there is no follow, and the
   only offset today is `Shake::offset` (`crates/narvo-ecs/src/shake.rs:212`).
   The write is `follow.rs:329-331` and it is the only `get_mut::<Camera>` in the
   workspace.
3. **Advance first, compose second.** `Shake::advance`
   (`crates/narvo-ecs/src/shake.rs:187`) runs before the sum is taken, so
   **wherever the camera is written it is a pure function of the components as
   the tick leaves them**. That is what converts the rule from a convention a
   reader has to keep into a property a test recomputes. The qualifier is not
   decoration: two rows of the table write **no camera** — a lost `Follow`, and
   a `Camera` with no contributor — and they reach that outcome by different
   routes. A lost follow reaches the base `match`, gets `None`, and the
   `continue` holds the camera (`crates/narvo-ecs/src/follow.rs:318-326`); a
   contributor-less camera never reaches the match at all, because the first
   pass drops it (`follow.rs:296-298`), which also makes the `(None, None)` arm
   at `:322` unreachable. In both the camera keeps whatever it was last given,
   and keeping is not a function of the current components —
   `a_lost_follow_holds_the_camera_through_a_shake` (`follow.rs:1101`) pins that
   while the `Shake` on the same entity goes on advancing and being written back.

"The only writer" is a claim about *mutation*, and it is worth being exact:
`Camera` values are still constructed and inserted elsewhere — a scene places one
at `crates/narvo-app/src/sim/scene.rs:330`. Placing a camera is not writing one
each tick, and this decision governs the second.

## Why

- **Replay safety (ADR-0008, ADR-0010).** All contributor state lives in
  components, so it is registerable — and dumped and hashed in a scenario that
  registers it, which is the only form that claim may take. The composed camera
  is derived, never a second source of truth, so there is no copy that can
  disagree with the state it came from. No generator is involved anywhere in this
  path, so ADR-0010 has nothing to bind here.
- **Single-writer clarity.** A reader who asks "what moved the camera" has one
  place to look, and a `grep` for `get_mut::<Camera>` answers it.
- **Hash coverage without a second mechanism.** Where a scenario registers both
  contributors, their state is in the hash and the composed camera is a function
  of it, so a divergence between the two is impossible rather than merely
  unlikely.
- **A third behaviour costs one term.** Bounds clamping, a zoom effect, a recoil:
  each adds a component for its own state and one term at the composition. It
  does not need to know what else contributes, and — the load-bearing half — it
  **cannot be forgotten by a caller**, because no caller composes.

## The rejected candidates, with the arguments that lost

**Last writer, ordered by system registration.** Follow writes the camera; a
shake system then adds to it. Rejected on three counts. It works exactly while
both contributors run *and* both write, and fails quietly when one does not — a
lost follow stops writing, so the shake's offset accumulates onto its own
previous output instead of onto a base. Nothing in the resulting value says who
produced it, so a reader recovers the answer only from registration order. And a
third behaviour has to be inserted into an order that is implied rather than
written down.

**Composing at view-build time**, with `Camera` staying follow-pure and
`camera_of` (`crates/narvo-app/src/sprite_batch.rs:234`) adding the offset while
building the view. **Half of this was adopted and it is the better half**: its
ADR-0008 story is the prettiest of the three — the offset is state, the effective
view is a derived function, so nothing composed is ever stored and there is no
second copy to disagree with the first. What the adopted half buys is narrower
than that, and the difference is worth stating rather than borrowing the
candidate's own praise: the shake's own **oscillator** state — `amplitude` and
`phase` — stays on `Shake` and reaches the camera only through `Shake::offset`,
which is a pure function of those fields rather than a stored one. Its captured
base is a different matter and is the sum's base term in the shake-only row. And
the composed value *is* stored: `compose_camera` writes `base + offset` onto
`Camera` on every tick that composes one — three of the five rows
(`crates/narvo-ecs/src/follow.rs:329-331`) — so in a scenario that registers
`Camera` it reaches the dump. In those three rows it cannot disagree with the
components, because the guard recomputes it there; in the two holding rows the
stored value is a stale one the current components do not reproduce, and that is
the price of storing it. "Nothing composed is ever stored" is true of the
rejected candidate, not of this one. The
*composing* is rejected on reach: `camera_of` lives in `narvo-app`, so the
camera's meaning would straddle two crates and **every other view-builder would
silently lose the shake** — the same shape of silent, correct-looking wrongness
that M3.30 excluded for mis-following. It fails the reach test, not the replay
test, and that distinction is why only one half of it survives.

## The guard, and what it does not catch

`the_camera_is_exactly_the_base_plus_the_offset`
(`crates/narvo-ecs/src/follow.rs:766`) recomputes the camera from the components
the tick left behind and compares bit for bit, across follow-only,
follow-with-shake and shake-only, over eight ticks each. `ProjektPlan.md` §4/P6
(`:110`) is what asks for this rather than a comment — *"Kein Feature ohne
maschinellen Verifikationspfad"* — and `CLAUDE.md`'s Definition of Done says the
same in English (`:271-273`, *"Looks right" is not acceptance*).

It catches a second writer, a dropped offset and a flipped order. **Two of those
three were demonstrated red**, in M3.31 §5(a): `camera.x += 1.0` after the
composition, and composing from the shake's pre-`advance` state. A dropped offset
was never injected — it follows from the assertion's shape rather than from a
run, and saying "all three were demonstrated" would promote a by-construction
property to a measured one.

**It cannot catch a wrong base selection**, and that is structural rather than an
oversight: its `match` is the implementation's `match`, so a changed precedence
moves both sides together. The rows are covered instead by tests that fix their
expectation independently of that `match`: two name literals —
`a_shake_runs_its_trail_and_then_rests` (`:821`) and
`a_live_follow_ignores_the_shakes_own_base` (`:1039`) — and one pins a value
captured before the losing tick,
`a_lost_follow_holds_the_camera_through_a_shake` (`:1101`). A guard that
re-implements what it guards is worth having anyway, but only if it says so in
its own doc, which it does.

## Consequences

- **A new camera behaviour adds a component and a term, never a second writer.**
  A change that adds a second `get_mut::<Camera>` anywhere is a change against
  this ADR, not a refactor within it.
- **The registration duty grows with every contributor.** `ProjektPlan.md` §12's
  obligation is now `Transform`, `Layer`, `Sampling`, `Camera`, `Follow` and
  `Shake` together, discharged for the first time by `narvo-app`'s `scene` mode
  — six of `build_registry`'s seven `register_component` calls
  (`crates/narvo-app/src/sim/scene.rs:382-387`; `:388` adds that module's own
  `Wander`), not the system registration at `:406-408`, which is a different
  list. A scenario that draws and registers only some of them has a replay that
  is not one.
- **The composition assigns rather than adds, so a base-less contributor is a
  trap.** A `Shake::new` on a camera with no follow carries a base of `(0, 0)`
  and pulls the camera to the origin on its first tick;
  `a_bare_new_shake_pulls_a_standalone_camera_to_the_origin`
  (`crates/narvo-ecs/src/follow.rs:1073`) pins that so it is recorded rather
  than lurking, and `Shake::around` is the constructor that avoids it. Making the
  composition add instead would be a behaviour change and needs its own task.
- **The system's name says the job.** It was `follow_camera` from M3.30, when
  following was all it did, and M3.36 renamed it to `compose_camera` — following
  is one contributor of two and the composing is what the function is for.
  The module stays `follow.rs`, because a follow is still where the base normally
  comes from and moving it would touch more than a name.
- **A contributor that needs the composed camera as its input does not fit this
  shape.** Nothing needs that today; if something does, see below.

## Revision condition

Reopen if a contributor has to read the composed camera back — a bounds clamp
that must see the shake before deciding, for instance. Such a behaviour is not a
term in a sum, and ordering *inside* the one system would become load-bearing in
a way `camera = base + Σ offsets` does not express; that is a different decision
and deserves to be taken as one rather than smuggled in as one more term.

Reopen also if a camera ever has to be written from outside the tick — a cutscene
or an editor placing it directly. Placement at scene-build time is already
outside this rule and is fine; a *repeated* write from a second place is exactly
what this ADR forbids, and if it becomes necessary the single-writer property has
to be restated rather than quietly broken.
