# ADR-0004: Orientation conventions for the render path

Status: accepted · Date: 2026-08 · Scope: narvo-render2d, and everything that
draws or compares images; since the M3.4 amendment also the world-space
conventions of registered components, wherever they live

## Context

Three coordinate systems meet in the render path and they do not agree about
which way y points:

- **Normalised device coordinates** put y up. `y = +1` is the top of the
  viewport. This is the WebGPU rule, not a choice Narvo gets to make.
- **The framebuffer** puts y down. Row 0 is the top row, and the fixed-function
  viewport transform is what turns the one into the other.
- **Texture coordinates** put their origin at the top left, so `v` grows
  downward — the opposite of NDC y, and the same direction as framebuffer rows.

M1.2 wired all three together and the result is right way up, but the convention
existed only as a table of numbers and a few comments. That is enough for one
quad and not nearly enough for sprites, a camera, or a golden-image suite, each
of which will silently depend on it. This ADR records what the code already
does; it changes nothing.

## Decision

The convention, as implemented:

| Stage | Origin | y direction |
| --- | --- | --- |
| Normalised device coordinates | centre | up (`+1` is the top) |
| Framebuffer / render target | top left | down (row 0 is the top) |
| Texture coordinates | top left | down (`v = 1` is the bottom) |
| `Pixels` buffer, `Pixels::pixel`, PNG | top left | down (`y = 0` is the top) |

The two opposing conventions are reconciled **exactly once**, in the vertex
table `VERTICES` in `crates/narvo-render2d/src/quad.rs`, which pairs NDC
`y = +1.0` with `v = 0.0` and NDC `y = -1.0` with `v = 1.0`. Nothing else in the
path flips anything:

- `src/shaders/quad.wgsl` passes both attributes straight through.
  `vs_main` writes `vec4<f32>(input.position, 0.0, 1.0)` and `output.uv =
  input.uv`; there is no negation in the shader.
- The NDC-to-framebuffer flip itself happens in the fixed-function viewport
  transform, inside the GPU. It is not our code and cannot be configured away.
- `OffscreenTarget::finish_and_read_back` copies with `origin: Origin3d::ZERO`,
  so `copy_texture_to_buffer` emits framebuffer rows in order, top row first.
  The padding-stripping loop iterates `chunks_exact` over that buffer and
  preserves the order.
- `Pixels::pixel(x, y)` indexes `(y * width + x) * 4`, so `y = 0` is the
  framebuffer's top row.
- `Pixels::save_png` hands the buffer to `image::save_buffer` as `Rgba8`, which
  reads it row-major from the top, so PNG row 0 is the top row too.

So a texel at the top left of an input texture ends up at the top left of the
output image, of `Pixels::pixel(0, 0)`, and of the written PNG.

## Rationale

1. **The flip is not ours to make, only ours to absorb.** NDC y-up against
   framebuffer y-down is fixed by the graphics API. The only open question was
   where to compensate, and the answer had to be somewhere.
2. **Compensating once, in data, beats compensating in code.** A `1.0 - uv.y` in
   the fragment shader or a reversed loop in the read-back would put the same
   decision somewhere that runs per pixel and reads as an implementation detail.
   In the vertex table it is four lines that can be read side by side and
   checked against each other.
3. **Top-left origin all the way out matches every consumer.** PNG, the `image`
   crate, image diffing tools and most 2D content pipelines already address rows
   from the top. Ending anywhere else would mean every consumer flips, and the
   one that forgets produces an upside-down reference image that looks plausible.

## Consequences

- Every later piece of render code presupposes this. Sprite placement, camera
  and projection matrices, texture atlas coordinates, tilemap indexing and
  glyph layout all inherit it; none of them may introduce a second, silent
  reconciliation. A y-flip anywhere downstream has to be an explicit and
  documented flip, applied on top of this convention rather than instead of it.
- Changing the convention invalidates **every** golden image at once. Reference
  images encode the orientation as surely as they encode the colours, and a
  flipped renderer against unflipped references fails every comparison with no
  hint as to why.
- The quadrant test in `offscreen.rs` is the executable form of this ADR. It
  renders an asymmetric four-quadrant texture and asserts the corners land where
  this document says they land, so a change to the convention shows up as a test
  failure rather than as a subtly wrong screenshot.
- A caller that wants an image the other way up flips it deliberately, on the
  way out. The renderer does not offer an orientation option, because a
  configurable convention is not a convention.

## Amendment, 2026-08-07 (M3.4): world space

*Added, not rewritten. Everything above stands word for word, this ADR stays
`accepted`, and the decision it records is unchanged — what follows extends it
to a fourth coordinate system that did not exist when it was written.*

*The occasion, because a rule without one reads like bureaucracy at the next
tidy-up: M3.3 added `Transform` as a registered component and was told to
inherit its orientation from this ADR. The agent doing it reported that it
could not — the scope line above named only the render path, so this document
had nothing to say about a world. The convention was therefore written into a
doc comment in `narvo-ecs`, which is exactly the place a later reader would not
look. This closes that gap; the alternative, a second ADR for world space, was
rejected because two documents about which way y points is how the two of them
come to disagree.*

**World space puts x to the right and y up, with positive rotation
counter-clockwise.** The origin is where the projection says it is, which for
the fixed projection of M3.4 is the centre of the render target.

| Stage | Origin | y direction |
| --- | --- | --- |
| World space (`Transform`, and every component that carries a position) | projection-defined | up |

The direction is inherited rather than chosen, and the reason is the rule this
ADR already states: the two opposing y directions are reconciled **exactly
once**. World space agreeing with NDC is what keeps that true. A world with y
down would need a sign flip somewhere between a component and the vertex table,
and a second flip is what this document exists to prevent — the more so because
it is invisible in any test whose image is symmetric.

Counter-clockwise follows from x-right with y-up; it is a consequence of the two
axis directions, not an independent choice.

**Where the reconciliation happens, by file.** Three places, and none of them is
the projection:

- `crates/narvo-render2d/src/quad.rs`, `VERTICES` — NDC `y = +1.0` paired with
  `v = 0.0`, for the screen-filling quad. Unchanged since M1.2.
- `crates/narvo-render2d/src/sprite.rs`, `SPRITE_CORNERS` — the same pairing
  for a placed sprite: the corner at local `y = +0.5` carries `v = 0.0`.
- The NDC-to-framebuffer flip, in the fixed-function viewport transform inside
  the GPU. Not our code, as before.

`Projection::world_to_ndc` in `sprite.rs` is a scale on each axis with **no
negation**, and that is the property that keeps the list at three. It is
asserted directly by `no_axis_is_negated_on_the_way_to_ndc`, without a GPU and
without an image, because an image can only show the flip against a texture
asymmetric in the right axis and a test that depends on that is a test that can
be weakened by changing a fixture.

## Amendment, 2026-08-08 (M3.11): depth order

*Added, not rewritten. Everything above stands word for word — the original text
and the M3.4 amendment alike — this ADR stays `accepted`, and the decisions they
record are unchanged. What follows extends this document to an ordering rather
than to a coordinate system, which is a widening the scope line above does not
name; that line is left untouched deliberately, and a replacement wording for it
was proposed to the maintainer rather than applied here.*

*The occasion, because it is also the argument: M3.10 introduced the direction
and, in a first draft, justified it by analogy: larger coordinates are nearer the
viewer, as larger y is nearer the top. The agent writing it struck that sentence
before committing, on the ground that this document says nothing about depth, so
it never entered the repository under its own commit; the wording quoted here
comes from M3.10's report, which lives under the ignored `target/` and is not
part of the repository either. The convention survived, its anchor did not, and
since then it has lived in doc comments — on `narvo-ecs`'s `Layer`, on that
type's `depth` field, on `Layer::DEFAULT` and on `placements_of` in
`narvo-app` — and in a bullet of `ProjektPlan.md` §6/M3, but in no decision
record. That is the same kind of place, for the same reason, that the M3.4
amendment relieved of the decision: `Transform`'s doc comment still states the
convention and now cites this document for it.*

**Larger depth is drawn later and therefore ends up in front.** A sprite at
`depth = 1.0` covers one at `depth = 0.0` wherever the two overlap.

| Stage | Zero | direction |
| --- | --- | --- |
| Draw order (`Layer::depth`, and anything else that carries a depth) | `Layer::DEFAULT`, which is `0.0` | larger is nearer the viewer, and is drawn later |

**This does not follow from the axis directions, and claiming that it does is a
mistake that has already been made once.** The three y directions above are
facts about coordinate systems that a graphics API forces to disagree, and the
whole point of this document is where that disagreement is absorbed. A depth is
not a coordinate here: nothing projects it, nothing compares it against a
fragment, and no arithmetic anywhere turns it into a position. It is a sort key,
and the direction of a sort key is free. Where the M3.4 amendment could write
"the direction is inherited rather than chosen" and then give the inheritance,
this one cannot, and says so instead.

**Why it belongs in this document rather than in one of its own.** The criterion
is the one this ADR already applies to a y-flip: *a convention a symmetric test
image cannot see has to be written down, because nothing else will catch it.*
Reverse the depth order in a scene whose sprites all show the same texture and
the image stays entirely plausible — the same rectangles in the same colours,
a different one on top — exactly as a y-flip stays plausible against a texture
symmetric in y. M3.10 supplied the demonstration by accident: its reference image
was correct, and the human blessing it read it as wrong, because its three
sprites were not distinguishable from one another (`ProjektPlan.md` §10). The
alternative, a second ADR for depth, is rejected for the reason M3.4 rejected a
second ADR for world space: two documents about which way something points is
how the two of them come to disagree.

**Order is the only depth information there is.** The render pipeline is created
with `depth_stencil: None` (`crates/narvo-render2d/src/quad.rs:157`), so there
is no depth buffer and no depth test, and a later sprite simply overwrites an
earlier one where they overlap. Two consequences worth having in writing:

- The rule above is not a rendering feature; it is what a depth *means* today.
  Everything it promises is delivered by the sort in `placements_of`
  (`narvo-app`), where ADR-0015 already puts draw order, and by nothing on the
  GPU. `narvo-render2d` draws the slice it is given, in the order it is given,
  and sorts nothing.
- **If a depth buffer is ever added, its comparison function and its clear value
  are derived from the direction above, not the other way round.** Choosing them
  to match some other convention would reverse this one for every sprite that
  goes through the new path while leaving every sprite that does not exactly as
  it was — a divergence no single image can show, since each half of it looks
  correct. That is a new decision and a new ADR, and it starts from the sentence
  in bold.

**Where the order is not total, and what breaks the tie.** Two sprites at the
same depth are ordered by ascending `EntityId`, in `placements_of`. An
`EntityId` is a slot index plus a generation and its `Ord` compares the slot
index first, which gives the tie-break a named consequence: **despawning an
entity and spawning another reuses the slot, so the new entity takes the old
one's place in the tie-break rather than joining the end.** That is
reproducible, which is what the order is for, but it is not "later spawns draw
later", and it is the surface content authors will meet first (`ProjektPlan.md`
§12 carries it). Two float cases are settled with it: `-0.0` is mapped to `+0.0`
before the comparison so that the two tie, and a `NaN` depth sorts to an end via
`f32::total_cmp` instead of panicking — defined and reproducible rather than
correct.

## Amendment, 2026-08-17 (M7.1d): the window's backend moved, the conventions did not

**[See ADR-0046.]** On Windows the window now runs on DX12 rather than Vulkan.
Nothing in this ADR is rewritten, and nothing in it is overtaken: every rule
above is stated in terms of NDC, framebuffer and image row order, which are the
graphics API's own conventions and not one backend's, and no y-flip anywhere in
the workspace moved. All twelve blessed references were re-measured after the
change and every one reports `0 pixels differ, worst channel deviation 0 counts`
— but that says nothing about the backend, because they are drawn through the
*offscreen* instance, which did not move.

What is therefore **unmeasured** is orientation in the window on DX12. That is
not a new gap so much as the old one under a new name: `CLAUDE.md` already
records that the window and present path has no automated coverage, deliberately,
because the hand-over to the compositor is not observable below a compositor. A
y-flip that existed only on the window's backend would be caught by a human
looking at the window and by nothing else — before this change as well as after
it, on whichever backend the window happened to be using.

## Revision condition

The convention may only be changed together with a regeneration of every golden
image in the repository, in one change, and with a new ADR superseding this one.
Flipping the renderer while leaving reference images in place produces a suite
that fails everywhere and explains nothing, which is worse than either
orientation.
