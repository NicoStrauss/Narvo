# ADR-0027: A click is world state answered in draw order, and a scene world gets its input buffer from the runner

Status: accepted · Date: 2026-08 · Scope: `narvo-ecs` (`HitRect`, the engine
set), `narvo-app` (`hit`, `sim::scene_file`, the window's click path),
`narvo-render2d` (`Projection::screen_to_world`)

## Context

M5.3 gave the window a keyboard. M5.4 gives it a mouse — one click, no hover, no
drag — and that needs three things the repository did not have: a component
saying what a click on an entity means, an inverse of the render transform, and a
scene world that can actually receive an input event.

## Decision 1 — `HitRect` is a registered component, and it holds a string

An axis-parallel rectangle in world units, centred on the entity's `Transform`,
plus the action name and magnitude a click on it sends. Registered, so it is in
the canonical dump and the state hash: a click that changes the world has to be
reproducible, and an area that lived outside the world would not be.

**The string is permitted by ADR-0014's own words**, not by analogy. Its
consequences say a registered type *"holds only scalars, `String`, and other
registered-component types"*, and `Sprite` (M4.8) is the precedent that exercised
it. The ADR's concern is a dependency's serde format entering ADR-0008's
stability domain; `String` has no such owner.

**It ignores `rotation` and `scale`, as one rule rather than two.** Axis-parallel
is the decision, and an axis-parallel rectangle cannot follow a rotation at all.
Honouring `scale` but not `rotation` would be the half-rule that is worse than
either — it would look like it tracked the sprite and would stop the moment
anything turned.

**It does not validate its action name**, exactly as `Sprite` does not reject an
unresolvable region. A component is storage, and this crate *cannot* reach the
rule: it lives in `narvo-input`, which `narvo-ecs` does not depend on
(ADR-0025). The check happens where a world and the rule are both in scope —
`sim::scene_file` — at load, naming the entity index, which `narvo-scene` calls
the stronger of its two spellings of *where* because it is also the world slot.

## Decision 2 — the ordering rule has one copy, and hit testing is its mirror

`depth_order` **moved** from `sprite_batch` to `crate::hit` and gained a second
caller rather than a second copy. `placements_of` imports it.

That matters for one reason with a name: `f32::total_cmp` puts `-0.0` strictly
below `+0.0`, and content does not mean that. A hit test that normalised zero
differently from the draw order would put a click on the sprite that is *not* in
front, and only for depths of `-0.0`.

`hit_test` returns the greatest `(depth_order(depth), EntityId)` among the
rectangles containing the point — the mirror of a sort that draws in ascending
order of the same pair, so the last drawn is on top. Stating it as one comparison
over a pair rather than as two rules is what keeps it *the same* comparison.

**`hit_test` lives in `narvo-app`, not `narvo-ecs`**, and `Layer`'s own
documentation is why: the ordering rule *"lives with the extraction in
`narvo-app`"*, where ADR-0015 put the draw-order decision. The component is
state; the order is render-adjacent. The module is gated
`#[cfg(any(feature = "render", test))]` like `input` and `watch`, so it is fully
tested headless.

## Decision 3 — the screen-to-world inverse lives beside the forward transform

`Projection::screen_to_world` sits in `narvo-render2d`, in the same struct as
`world_to_ndc`, reading the same three fields. A conversion written anywhere else
would be a second piece of camera mathematics: correct on the day it was written
and free afterwards to disagree about a zoom, a half-extent or a sign.
`a_world_point_survives_the_round_trip_through_the_screen` composes the two and
asserts identity.

**The y flip, and why it is not the second reconciliation ADR-0004 forbids.**
Pixel rows run down and NDC y runs up, so the inverse negates y where
`world_to_ndc` does not. On the way *out* that meeting is the GPU's viewport
transform, which is not this project's code; on the way *back* there is no GPU,
so the inverse of that transform has to be written down, and this is the only
place it is. It is the same single reconciliation read backwards. If a second
flip were ever added, the round trip would stop being the identity.

## Decision 4 — the input buffer for scene worlds is registered by the runner, not the engine

**The task that produced this ADR asked for `register_engine_components` to grow
an `"input"` entry. It cannot, and that is recorded rather than worked around.**

`Events<InputEvent>`'s payload lives in `narvo-input`, and ADR-0025 Decision 1
says: *"`narvo-app` depends on `narvo-ecs` and on `narvo-input`, and neither
of those two knows about the other."* Registering the buffer in the engine set
would mean `narvo-ecs` depending on `narvo-input`, reversing that layering and
closing a dependency cycle against `narvo-input`'s dev-dependency
(`crates/narvo-ecs/Cargo.toml` has no workspace dependency at all;
`crates/narvo-input/Cargo.toml:26-27` is the dev edge).

So `sim::scene_file::build` registers it, exactly as `sim::scene` registers its
own `Wander`, and exactly as the registry's own documentation describes: a caller
registers the engine set *"and then whatever else it has"*.

**Everything the milestone needed survives the change of address.** A scene file
may name `"input"` — the component-open consequence of registration, and not a
special case ADR-0018 would have to carve out. The buffer is in the dump and the
hash. `rotate_events` is wired.

**The cost is named rather than hidden:** `tools/narvo-cli` validates against
the engine set alone, so it reports a scene naming `"input"` as carrying an
unknown component. That is already true of `wander`, and it is the same trade —
the validator knows the engine, a runner may know more.

**Rejected: make `narvo-ecs` depend on `narvo-input`.** Best argument: the
buffer would then be engine vocabulary everywhere, the CLI could validate a scene
that names it, and one registration would serve every runner. Against it: it
inverts ADR-0025's layering four milestones after that ADR made the leaf-crate
property the reason the split was safe, and it would need a superseding ADR with
an argument this task has no evidence for.

**Rejected: move `InputEvent` back to `narvo-ecs`.** Best argument: the cleanest
resolution of the same tension, and it would put the buffer in the engine set
without a cycle. Against it: it undoes M5.1 and ADR-0025 wholesale to buy one
registration.

## Decision 5 — the insertion rule

`scene_file::build` gives the world exactly one input buffer:

- **If any entity already carries one, nothing is inserted.** The scene said
  where the buffer goes. A second would give the world two, and `rotate_events`
  rotates every buffer it finds while a feeder writes to the first — a silent
  half-delivery rather than an error.
- **Otherwise one entity is spawned to carry it, after every entity the file
  describes.** That position is what makes it deterministic: ids are handed out
  in spawn order, the scene's entities are spawned in file order (ADR-0018), and
  appending leaves every one of their ids where it was.

**It moves the dump of every scene-file world, deliberately and measurably.** The
suite's scene-file case went from `entities 3` to `entities 4` with one appended
line and no other change; the other 21 artifacts are byte-identical, and the
recording did not move because a scene-file run feeds no input. Both sides of
every comparison come from one build (ADR-0008) and no expected hash is stored
anywhere to disagree.

## Decision 6 — a click is an action and never a position

The window remembers the cursor and acts only on a left press. The path is:
remembered position → `screen_to_world` through the *last extraction's* camera →
`hit_test` → the rectangle's own action and value → the M5.3 queue.

**The click never reaches a recording; only the action does.** That is ADR-0012's
M5.2 amendment applied to a second device: a repro says `buy 1`, not "the left
button went down at (412, 233)", and it therefore survives a window of a
different size. Nothing in this task gives a cursor position a serialized form.

Physical pixels on both sides — winit reports the cursor in them and the
projection is built from the target's physical extent — so no scale factor has to
be guessed at.

## Consequences

- **The engine set is nine types.** Adding one turned **seven** tests red across
  four crates, then two more when the census grew, then two more when the
  specimen grew, then one more: four stages, nine sites. The M4.8 cascade
  protocol is what made that a procedure rather than a surprise.
- **A ninth census copy was found unguarded.** `tools/narvo-cli/tests/cli.rs`
  carries *two* transcribed engine sets and M4.9's agreement guard covers only
  one; the other was caught by a downstream round-trip failing. Reported.
- **`SceneHost` gained `world()` and `camera()` as production accessors.** The
  render path still only reads.
- **Hit testing is not switched on by `--mapping`**, but the queue that carries
  its result is. A window with no mapping has no feed, so a click produces
  nothing.

## Revision condition

Reopen when a second pointer gesture is wanted — hover, drag, double click —
because each needs state between events and this decision deliberately keeps the
path stateless apart from the remembered position.

Reopen if a rotated or scaled entity ever needs a hit area that follows it, which
is an oriented rectangle and a different type rather than a widening of this one.

Reopen if `narvo-ecs` ever gains a legitimate reason to depend on
`narvo-input`, since Decision 4 is the only thing standing between the buffer
and the engine set.
