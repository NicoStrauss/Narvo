# ADR-0015: The renderer takes a buffer of scalars, not a world

Status: accepted · Date: 2026-08-07 · Scope: narvo-render2d and every caller
that draws simulation state

## Context

M3.4 drew one sprite whose placement came from a `Transform`, and the seam had
to be put somewhere: `narvo-render2d` does not depend on `narvo-ecs`, so it
cannot see the component. The task worked around it by taking five scalars, and
the agent reported the underlying question rather than deciding it — whether the
render crate should depend on the ECS is an architecture call, and CLAUDE.md
says a change that wants to span crates is a signal to stop and report.

M3.5 is where the maintainer decided it, as D12 in `ProjektPlan.md` §11. M3.6
is the first task that makes it load-bearing: a batch reads many entities, which
is exactly the case the decision was deferred until.

## Decision

**`narvo-render2d` stays free of the ECS. It takes placements as an explicit
buffer of scalars — a slice of `SpritePlacement` — and the extraction from a
`World` lives outside it.**

Concretely, as built in M3.6:

- `narvo-render2d` owns the projection, the per-sprite vertex generation, the
  batch geometry and the draw call. Its input is `&[SpritePlacement]`, five
  `f32` per sprite.
- `narvo-app` owns `placements_of(&World) -> Vec<SpritePlacement>`. It is the
  only crate that sees both sides, and it is where a runner already lives.

> **Amended by M6b.1 — the location above is superseded and left standing.**
> `placements_of` lives in `narvo-view2d` since `901493f`, and the atlas half
> followed in M6b.2a. The wording is deliberately unedited: a Decision section
> records what was decided on its date, and this ADR's own amendment below
> explains why the move fulfilled the decision rather than reversing it.

## Why

Two reasons, and the second is the one that decides it.

**A task on the renderer stays workable without ECS context.** The crate can be
read, changed and tested by someone holding only the render path in their head,
which is the property CLAUDE.md's one-crate-per-task rule exists to protect. It
also keeps the seam of ADR-0005 intact: the one place hecs is visible in a
public API stays the query facade, and does not acquire a second exit through
the renderer.

**The throughput criterion is cleaner with an explicit buffer.**
`ProjektPlan.md` §6/M3 already splits the 50 000-sprite target into parts, and
the part this engine controls is the CPU-side batch preparation. Measuring that
is straightforward when the renderer's input is a buffer someone handed it, and
muddled when the renderer's input is a world it queries itself: the measurement
would then include query iteration, borrow checking and archetype traversal, and
attributing a regression would mean separating them again afterwards.

## The alternative that was rejected, and its argument

**A dependency `narvo-render2d → narvo-ecs`, with the renderer querying the
world directly.**

Its argument is real and was not dismissed: every renderer that ships in a real
engine reads the world, and the indirection chosen here copies data every frame.
At 50 000 sprites that copy is 50 000 `SpritePlacement` values — twenty bytes
each — built, moved and dropped once per frame, which is precisely the
performance question §6/M3 asks. A renderer that iterated the world in place
would not pay it.

What decides against it is that the payment buys a measurement. §6/M3's
criterion is about the CPU-side preparation, and an explicit buffer makes that
preparation a thing with a boundary — something a benchmark can time without
timing the ECS as well. **The price is real and is knowingly paid.** If the
throughput task shows the copy to be the dominant cost rather than a component
of it, that is evidence for a new ADR superseding this one, not a reason to
reinterpret this one.

## What building it added to the decision

Three things the decision text could not have known, recorded because the next
reader will otherwise rediscover them:

- **The seam turned out to be the natural place for draw order.**
  `placements_of` iterates `World::entity_ids`, the canonical ascending
  enumeration, rather than a query — because query iteration is archetype order,
  which `narvo-ecs` documents as explicitly unstable. Draw order is the order
  of the buffer and there is no depth buffer, so with a query the same world
  could have drawn the same sprites in two orders and produced two images
  wherever they overlap. Had the renderer queried the world itself, this choice
  would have been made inside the render crate, where nothing about entity
  identity is visible.
- **The copy is where a filter will go.** Only entities carrying a `Transform`
  become placements; culling, layer selection and visibility flags all belong at
  the same point. That work has to happen somewhere on the CPU regardless, so
  the copy is less purely additional than the rejected alternative's argument
  suggests. This is reasoning, not a measurement — no benchmark has run.
- **`narvo-app` is a binary crate with no library target**, so `placements_of`
  is reachable from unit tests inside `src/` and from nothing else. That is
  adequate today and will not stay so: the moment a second consumer needs the
  extraction, either that crate grows a `[lib]` or the function moves. Named
  here so the move is a decision rather than a surprise.

## Consequences

- **No type from `narvo-ecs` appears in `narvo-render2d`'s API, and no
  `wgpu` type appears in `narvo-app`'s.** The two crates meet at
  `SpritePlacement`, which is five `f32` and belongs to the renderer.
- **This is ADR-0014 one level up.** That ADR keeps foreign types out of
  serialized components so a dependency cannot govern the state hash; this one
  keeps the ECS out of the renderer so a dependency cannot govern the render
  path. Both are the same shape of boundary and both are paid for in
  hand-written conversion.
- **A renderer that needs more per sprite widens `SpritePlacement`**, it does
  not reach for the component. A tint, a texture region, a layer index are all
  more scalars.
- **The extraction is not covered by an integration test**, only by unit tests
  inside the binary crate, for the reason above. The end-to-end path from a
  `Transform` to pixels is covered instead in
  `crates/narvo-app/tests/transform_to_sprite.rs`, which duplicates the five
  field reads rather than calling the function.

## Revision condition

The throughput task of §6/M3, when the CPU-side batch preparation is actually
measured. If the per-frame copy dominates rather than contributes, this decision
is taken again on that evidence in a new ADR. Reopen also if a second crate
needs the extraction, since the binary-only home stops working at that point.

## Amendment, M6b.1 (2026-08-15): the second condition occurred, and this ADR was
## fulfilled rather than superseded

**The second half of the revision condition has happened.** M6b.0 built an
external consumer of the public surface, it reached six of seven goals, and the
seventh stopped exactly here: the probe rewrote `hit_test` and `depth_order` by
hand and could not reach `placements_of` at all. That is the "second crate needs
the extraction" case, arriving as a probe rather than as a shipped crate, and it
is the case this ADR said the binary-only home would not survive.

**The first half did not occur, and it was checked rather than assumed.** This
ADR makes the rejected alternative's return conditional on a number — whether the
per-frame copy *dominates* rather than contributes. It does not:

- the copy is **14.6 µs of 1 025 µs** at 50 000 sprites (M3.7, in
  `placements_of`'s own documentation);
- the extraction phase is **3.76 ms of a 6.74 ms frame** (M3.32), and the cause
  named in the same paragraph is the *double enumeration* — `camera_of` and
  `placements_of` each call `World::entity_ids`, which allocates and sorts a
  vector of every live id — rather than the copy.

So `narvo-render2d → narvo-ecs` stays rejected, on the evidence this ADR asked
for. The Decision and the Consequences above are unchanged and still hold: no
`narvo-ecs` type appears in `narvo-render2d`'s API, the two crates still meet at
`SpritePlacement`, and the renderer still sorts nothing.

### Which of the two named answers was taken

This ADR offered two, and M6b.1 took the second:

> the moment a second consumer needs the extraction, **either that crate grows a
> `[lib]` or the function moves.**

**The function moved.** ADR-0041 records where and why: a new crate,
`narvo-view2d`, carrying the extraction and the hit test together, with
`narvo-app` as its consumer. The `[lib]` answer was measured and rejected — 327
packages against 147, and verification step 4 breaking with 141 dead-code errors
in the form that was measured.

This is therefore **not** a superseding decision. ADR-0041 does not overturn
anything here; it carries out an alternative this ADR named in advance, which is
what the sentence was written for.

### What in this document is now out of date

One line, and it is in the Decision:

> `narvo-app` owns `placements_of(&World) -> Vec<SpritePlacement>`. It is the
> only crate that sees both sides, and it is where a runner already lives.

**`narvo-view2d` owns it now**, and is the crate that sees both sides. The
signature in that line was already stale before the move for a second reason —
it has returned `Vec<Drawn>` since `Sampling` arrived in M3.23.

The Consequence about integration tests also needs reading with a date on it:

> **The extraction is not covered by an integration test**, only by unit tests
> inside the binary crate, for the reason above.

The reason is gone — `narvo-view2d` is a library and its tests are ordinary unit
tests that any consumer could also write from outside. What remains true is the
sentence after it: `crates/narvo-app/tests/transform_to_sprite.rs` still
duplicates the field reads rather than calling the function, and it still uses a
query rather than `entity_ids`, so it pins single-entity placement and not draw
order.

The rest of this document — the Why, the rejected alternative and its argument,
and all three findings under *What building it added to the decision* — is
unamended and still describes what is built.
