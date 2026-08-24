# ADR-0045: Widgets are not an engine concept, and edge anchoring is one function

Status: accepted · Date: 2026-08 · Scope: `narvo-render2d` (`Projection`), and
every consumer that draws a HUD

## Context

v1.06 decided against egui with the sentence that whoever takes the native path
"die Widget-Arbeit einkauft" — buys the widget work — and noted the overlap with
M7's HUD as *partial*. ADR-0038 later priced egui at **105 packages** the lock
does not have. What that decision actually cost in engine code had never been
measured, and `ProjektPlan.md` §6/M6b's last item, M6b.8, is where the bill came
due.

Two questions were open, and both had been carried since v1.12:

1. Does a HUD need a widget **system** — layout, anchors, event forwarding — or a
   set of **building blocks** a game assembles? §2 says the second until a second
   consumer appears.
2. ADR-0039's M6b.5 amendment booked **edge anchoring** as a named limit of the
   screen-fixed batch and addressed it here.

## The method: build first, then count

The same inversion that made M6b.5 tractable. Before anything was designed, a
probe outside the tracked tree built the HUD the task asks for — a button that
looks different on hover and on press, a panel with a border, a progress bar and
a number that can get big — out of what existed at `ede2740` and **nothing else**.

The result decided everything below, and it was not the expected one.

## Decision 1 — widgets are not an engine concept

**Nothing was blocked.** The probe built all four widgets, with hover, press,
text, tint and a screen-fixed layer, with **zero engine changes**. Its
hover/press oracle already passed in both directions: the idle frame and the
"pointer elsewhere" frame were **byte-identical**, and the hover and press frames
differed from both.

Measured, in code lines, by category:

| Category | lines | what it was |
|---|---:|---|
| policy | ~108 | the art, the abbreviation, the tint values, what the HUD contains |
| mechanism | ~53 | merging one texture; turning the world into sprites |
| guessed | ~27 | hover and press as the consumer's own state |
| **blocked** | **0** | — |

So no `Button`, no `Panel`, no `ProgressBar`, no widget tree and no event
forwarding is added. `hit_test` already answers "what is under this point", and
that is the whole of what hover and press need; the *look* a button takes is the
game's, and an engine that chose it would be choosing a visual style.

**§S3's warning did not fire.** The probe needed no event forwarding at all,
which is the finding that keeps §2's direction rather than overturning it. A
forwarding mechanism would have been the start of a system, and nothing asked for
one.

### Rejected: a `Hover` / `Press` component pair

Its best argument is real: hover state is per-entity, it changes over time, and
that is what a component is for. Against it: the probe measured the 27 lines it
would replace to be **mostly the tint values**, which are policy — and a
component the engine writes would need the engine to decide when a pointer
"enters" a widget, which is a frame-to-frame comparison and therefore state, and
therefore a system. That is the thing §2 forbids without a measurement demanding
it. **Reopen when a second consumer writes the same hover bookkeeping**, because
two copies of one mechanism is exactly the signal §2 waits for.

### Rejected: a number formatter in the engine

`abbreviate` was measured at 20 lines using **no engine type whatsoever** —
integer arithmetic and `format!`. A game that wants `1.2K`, `1,234` or a locale's
own separators wants a different function, and the engine has no basis to choose.
It stays with the game.

## Decision 2 — edge anchoring is `Projection::anchor`, and it is not layout

ADR-0039 booked it as layout. **That word was wrong**, and the correction is the
substance of this decision: what a HUD needed is one pure function that turns a
corner and an inset into a world point. No state, no tree, no knowledge of what
is placed there.

```rust
pub fn anchor(&self, anchor: ScreenAnchor, inset_x: f32, inset_y: f32) -> [f32; 2]
```

`ScreenAnchor` is a closed nine-variant enum — three positions per axis, so a
tenth variant could name nothing. A positive inset always points **inward**, on
both axes and from every anchor, which is what lets a HUD's four corners spell
alike instead of each carrying its own sign.

**Measured, before and after.** A HUD authored against 192 × 128 and rendered at
256 × 160 drifted by exactly half the size difference: a panel 2 px from the left
edge became 34 px from it, a button 14 px from the right edge became 134 px from
it. The same HUD built through `anchor` measures **left gap 2, right gap 14,
bottom gap 4 at both sizes.**

### It shares `screen_to_world`, and pays for it

The body picks a pixel and calls `Projection::screen_to_world`. A second piece of
camera mathematics here would be free to disagree with that one about a zoom, a
half-extent or a sign — the failure `screen_to_world`'s own documentation
describes.

**The price is exactness and it is paid knowingly.** `screen_to_world` goes
through NDC, so `anchor(Centre, 8.0, 8.0)` on a 192-wide target returns
`8.000004` rather than `8.0`. It is measured in
`an_anchor_carries_the_rounding_of_the_conversion_it_shares` rather than
tolerated silently, and a change that made `anchor` exact would fail that test
and have to argue with this paragraph.

### The camera is part of the answer, and a HUD wants none

`anchor` returns the world point that *currently appears* at that screen
position, so a panned camera moves it. That is the honest general answer and the
wrong one for a HUD, which is drawn through `CameraView::IDENTITY`. The trap is
named in the doc comment and asserted by `an_anchor_moves_with_the_camera`, so
the warning is a measurement rather than an opinion.

### Rejected: an `Anchor` component resolved during extraction

Its best argument: an author would write the anchor in a scene file and never do
arithmetic. Against it: `regions_of` is handed a `World` and nothing else — it
does not know the target size, and giving it one would change the signature every
blessed reference is drawn through. It would also make anchoring a property of
*extraction*, so a world would mean different things at different target sizes,
which is a much larger claim than this task measured a need for. **Reopen when a
scene file needs to author a HUD**, which is the first thing that cannot be done
by putting `anchor`'s result into a `Transform`.

## Decision 3 — the location is a consequence of M6b.9's criterion

`anchor` lives on `Projection` in `narvo-render2d`, which is the type built from
the target's dimensions and the one that already owns the conversion.

It is **not** in `narvo-app`. M6b.9's criterion requires an external consumer
outside the tree to use all eight capabilities in one run, and a binary crate has
no lib target — the same constraint ADR-0041 records. Anything a HUD needs that
landed in `narvo-app` would be a self-inflicted failure of the closing probe.

The rendered oracles live in `narvo-view2d/tests/widgets.rs`, because a HUD
button is a world plus a hit test plus an extraction and that crate is the one
that sees all three.

## What this cost, against what v1.06 expected

**50 production lines** — one method, one enum and two small helpers — measured
as added non-blank, non-comment lines under `crates/*/src/` outside `#[cfg(test)]`
modules. The estimate registered before the survey was **220**, with a band of
100–400. It missed low, below its own band.

**And the reason matters more than the number.** The bill is small *because it was
paid in instalments*: M6.6b built the text path, M6b.1 made `hit_test` reachable
from outside, M6b.3 added the tint, M6b.4 built the screen-fixed batch. Read
against M6b as a whole, v1.06's "buys the widget work" was right that there was
work; read against the widget task itself, it overestimated what was left.

**No package was added**, measured: an external consumer of the HUD path carries
**99 packages besides itself**, the same figure `target/reports/M6b.5.md:175`
records for a world-and-renderer consumer and `target/reports/M6b.7.md:554`
records after the burst.

## Consequences

- `ScreenAnchor` is a new public name in `narvo-render2d`'s flat root namespace.
  It is named `ScreenAnchor` rather than `Anchor` because a bare `Anchor` there
  would not say whether it meant a sprite, a world or the target.
- **Nothing in the render path moved.** `quad.rs`, the blend state, the `LoadOp`,
  the batch encoding, `world_to_ndc` and `screen_to_world` are untouched, and the
  twelve blessed references did not move.
- A consumer that wants text and art in one HUD batch must still merge them into
  one texture itself, because a draw call binds one texture and ADR-0039 fixes
  the batch count at **two, not `n`**. That is a named limit, not a defect, and
  it is M6b.9's to live with.

## Revision condition

Reopen when a second consumer writes the same hover or press bookkeeping, which
is §2's own signal that a mechanism has two copies.

Reopen when a scene file has to author a HUD, because that is the first thing
`anchor` cannot serve — Decision 2's rejected component comes back at that point,
and with it the question of what a world means at two target sizes.

Reopen if a HUD ever needs more than one texture in its own batch, which is
ADR-0039's "two, not `n`" seen from the other side.
