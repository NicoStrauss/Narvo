# ADR-0039 — A frame draws two batches, and an empty second one costs nothing

Status: accepted · M6.6c · 14.08.2026

## Context

M6.6 wants a debug overlay: glyphs over a running scene. M6.6c measured that
"append the glyphs to the sprite buffer" cannot work, and the reason is one
sentence already in this repository, in `sprite_batch.rs`:

> "One texture, because a draw call binds one."

A `SpriteInstance` carries a `TextureRegion`, and that region is **normalised
into one texture**. Glyphs live in the glyph atlas; the scene's sprites live in
the scene atlas. Appended glyph sprites therefore draw cutouts of the *scene*
atlas at glyph positions.

Two ways out were measured and rejected before this one:

- **A second `draw` call.** Every render pass in `quad.rs` carries
  `LoadOp::Clear(BLACK)` (`:392`, `:441`) and there is no `Load` variant, so the
  second call would wipe the scene.
- **Merging the two textures into one.** `TextureRegion::WHOLE_TEXTURE` is
  `(0,0,1,1)` and `from_texels` divides by the texture's dimensions, so growing
  the atlas changes what every existing region samples. That moves blessed
  references to obtain a debug feature.

What was actually missing is small, and the code already provided for it:
`QuadPipeline::encode_runs` takes `runs: &[(Range<usize>, &wgpu::BindGroup)]` and
calls `set_bind_group` **inside** the loop — its own comment says the bind group
"is the one thing that is *not* hoisted". A run pointing at a second texture's
bind group was structurally accepted and simply had no caller.

## Decision

**A frame draws two batches, in one pass.** The renderer's entry points take an
optional second `SpriteBatch { image, sprites }`, drawn after the first and
therefore over it. `narvo_render2d::batch_plan` decides which runs exist and
which batch each belongs to; the GPU-side code is a mapping over that plan.

**Two, not `n`.** A list of batches would be more general and would be stock
(§2): two is what has a consumer. `BatchOf` is where the compiler starts asking
the day a third is wanted.

**An empty second batch produces nothing at all** — no bind group, no run, the
same command sequence the target emitted before the parameter existed. Not
"draws nothing": *produces* nothing.

Nothing in the pass changes: `LoadOp::Clear` stays, the blend state stays
(ADR-0023 untouched), `encode_runs` is unmodified, `quad.rs` is unmodified.

## Consequences

- **The regression evidence for the ten blessed references stays the strong
  one.** Because an empty batch produces no run, "the shared code emits the same
  command sequence" carries — rather than the weaker "the overlay happens to be
  off", which is what M6.6c's S5 warned would otherwise be all that was left.
  Three things hold it: `an_empty_second_batch_adds_nothing_to_the_plan` (no
  device), `an_empty_second_batch_renders_the_same_bytes_as_no_second_batch`
  (rendered bytes), and `the_host_asks_for_no_overlay` (`SceneHost` sends none).
- **The batch limit is on the sum**, because both batches share one pass, one
  vertex buffer and one index buffer. `RenderError::BatchTooLarge` reports the
  total; its wording is unchanged.
- **Two public doors on the offscreen side, one implementation.**
  `render_sprites_viewed_by` keeps its signature — it has thirteen callers, nine
  of them in tests that draw blessed references, and changing it would have moved
  those files in the same commit as the seam. `render_sprites_over` is the form
  that carries a second batch. Both end in `render_batches`. On the window side
  `draw_sprites` has one caller, so it took the parameter directly rather than
  growing a second door.
- **`FrameTarget::draw` gains the parameter**, and all three implementations
  pass it through. `SceneHost`'s signature is untouched (M6.6a paid ten sites for
  it); only the trait's contract moved.
- **This task builds the seam and does not use it.** `SceneHost::encode` passes
  `None`. That is §2's no-stockpiling rule stretched a third time, deliberately
  and by the v1.08 cut — M6.6d is the named consumer.
- **One guard gap, measured rather than assumed.** Removing the emptiness filter
  so that an empty batch still uploads a texture and builds a bind group is
  caught by **nothing**: 1121 tests, 0 failures. No run changes and no byte
  changes, so neither instrument can see it. What it costs is a per-frame texture
  upload for nothing. The property is upheld by construction — one `filter` line
  in each entry point, commented as load-bearing — and is *not* guarded. Closing
  it would need a counter inside `QuadPipeline`, which is production
  instrumentation existing only for a test, and that trade was not taken.
- **Two batches naming the same texture is legal and silent**, measured: it
  returns `Ok`, draws both, and costs a redundant upload. Rejecting it would need
  an identity comparison between two `&Pixels` that means nothing useful.

## What this does not decide

Nothing about text, glyphs, an overlay or an inspector — that is M6.6d. It does
not add a render path: there is one pass, one pipeline, one blend state, and the
two-texture case is covered offscreen by `tests/two_textures.rs`, which is what
`lib.rs`'s own warning about "two render paths where the golden images pin one
and nothing checks the other" asks for.

## Rejected alternatives

**A list of batches, `&[SpriteBatch]`.** Argument: it is the general shape, and a
third batch would need no further change. Against it: nothing wants a third, and
§2's rule against building for an absent consumer applies to a parameter type as
much as to a feature. Two is also what makes `BatchOf` an enum a reader can hold
in their head. Revisit when something wants three.

**A `LoadOp::Load` pass variant and a second `draw` call.** Argument: the batches
stay independent, and a caller composes as many as it likes. Against it: it is a
second pass per overlay, it makes the clear conditional — which is exactly the
"two render paths" `lib.rs` warns about — and it buys nothing the per-run bind
group did not already provide.

**Keep `render_sprites_viewed_by`'s signature everywhere by adding a door on the
window side too.** Argument: symmetry between the two targets. Against it:
`draw_sprites` has exactly one caller, which must pass the overlay through, so
the wrapper would have had no callers at all — a dead function kept for the shape
of the file.

---

## Amendment, 2026-08-15 (M6b.4, written in M6b.5): a batch carries its own camera

*Added, not rewritten. Everything above stands word for word and this ADR stays
`accepted`; the decision it records — two batches, one pass, an empty second one
producing nothing — is unchanged and none of its consequences is withdrawn.*

*Written one task late, and that is worth saying plainly rather than
backdating: the decision was taken and built in M6b.4, whose prompt withheld the
authority to open or amend a decision record. M6b.4's report booked the debt as
its first named surface with the address `M6b.5`, and this discharges it. The
material it is written from is M6b.4's report and the doc comment on
`SpriteBatch::camera`, both of which predate this text.*

**One claim above is overtaken by it, and is marked here rather than edited.**
The Decision section reads *"an optional second `SpriteBatch { image, sprites }`"*.
That struct has carried a third field, `camera`, since M6b.4. The sentence is
left character for character as it was, because what it was deciding — that the
second batch arrives as one named borrowed struct rather than a tuple or a pair
of parameters — is exactly what the third field then relied on. Nothing else
above names the field list.

### The decision

**Every batch carries its own `CameraView`, as a field on `SpriteBatch`.** The
first batch is still viewed through the camera the entry point is given; the
second is viewed through its own. That is what lets a HUD stand still while the
scene moves under it, and the debug inspector in the shipped binary is the first
consumer.

**A field, not a fifth parameter, and the argument is the compiler's rather than
taste:**

- A fifth parameter would sit next to the fourth **with the same type** —
  `(…, overlay, camera, overlay_camera)`, two adjacent `CameraView`s. Swapping
  them compiles, renders, and puts the HUD into world space; no test, no type and
  no lint sees it. A named field cannot be swapped.
- The overlay is an `Option` at every entry point, so a separate parameter would
  admit the state *"an overlay camera and no overlay"* — a value describing a
  batch that is not there. As a field the camera cannot outlive what it
  describes.

The cost was five construction sites, all of them compile errors, which is the
shape ADR-0036 already priced.

**Passing the scene's camera reproduces the previous behaviour bit for bit.**
The overlay projection is the scene projection with its camera field replaced
(`projection.viewed_by(…)`, not a second `Projection::for_target(…)`), so an
equal camera replaces it with itself and no arithmetic runs. That spelling is
load-bearing, and it is why no blessed reference moved.

### "Screen-fixed" in numbers, and the half of it that is a limit

**`CameraView::IDENTITY` under `Projection::for_target(w, h)`** — origin at the
centre of the target, one world unit per target pixel, y up (ADR-0004, without a
negation). It is a consequence of how the projection is built from the target's
own dimensions, not a convention invented for the overlay.

**And it is centre-anchored.** An element keeps its pixel offset from the
**centre** of the target across target sizes; it does not keep its offset from an
**edge**. A bar authored 48 px below the centre is 48 px below the centre at
1280 × 720 and at 640 × 360 alike — which is what makes it a HUD at all — but it
is not 16 px above the *bottom* in both.

**Booked as a named limit of the capability, not as a defect.** Edge anchoring is
layout, and layout is `ProjektPlan.md` §6/M6b's last item, M6b.8.

> **The booking was redeemed in M6b.8, and one word of it was wrong.** The
> sentence above is left as written. `Projection::anchor` closes the limit —
> `ADR-0045`. What M6b.8 measured is that edge anchoring is **not** layout: it is
> one pure function returning one point, with no state, no tree and no knowledge
> of what is placed there. Calling it layout is what had made it look like a
> bigger thing than it is. Nothing in this ADR's own subject — the two batches,
> their order, the blend state or the batch limit — moved.

### Rejected: a camera component on each sprite

The alternative that is not about spelling. Each sprite would carry which of two
projections it is drawn through, and the batch line would stop being the
world/screen line.

**Its best argument, stated at full strength:** it is strictly more expressive.
It would allow damage numbers pinned to enemies and a status bar pinned to the
screen **in one frame and one batch**, which the batch-level camera cannot
express at all; and it would decouple "which texture" from "which space", which
are genuinely independent questions that this decision deliberately fuses.

**Against it, and the price is concrete rather than rhetorical:**

- It needs either a **further vertex attribute**, on a vertex M6b.3 had just
  grown from 16 to 32 bytes, or
- a **second cutting criterion in `batch_runs`** — which is precisely what M6b.3
  declined to do to D15's sampler cut, so taking it here would overturn that
  decision as a side effect of an overlay.
- ADR-0039 had already drawn a scene/overlay line through the frame. Putting
  "world" and "screen" onto the line that exists costs nothing; a per-sprite
  space adds a **third axis** to a frame that has two.

**The reopening condition is sharp, and it is the alternative's own best
argument turned into a test:** the first consumer that needs **two overlays with
different cameras** in one frame — damage numbers pinned to enemies and a bar
pinned to the screen, together. Until such a consumer exists, a world-fixed
overlay belongs in the scene batch, where it already works.

### What this amendment does not change

`LoadOp::Clear` stays, the blend state stays (ADR-0023 untouched), `encode_runs`
is unmodified, `quad.rs` is unmodified, and the batch limit is still on the sum.
It is still two batches and not `n`; `BatchOf` is still where the compiler starts
asking. ADR-0017's single composition point is untouched — this path **reads** a
camera and writes none — and ADR-0018 is not involved at all.

## Amendment (M8.2): a second kind of pass exists, and a frame still draws two batches

M8.2 built the multi-pass compute path the M8 lighting slices consume, and its
brief asked this decision to be either amended or superseded, on the premise that
"a frame no longer draws two batches."

**It is an amendment, because the premise did not hold in this tree, and that was
checked rather than assumed.** After M8.2 a frame still draws exactly two batches
in exactly one render pass. Nothing in `SceneHost`, `Windowed` or `Offscreen`
records a compute pass; `FieldKernel::run` has no production caller at all, and
the two new modules carry `expect(dead_code)` naming M8.3's jump flooding as the
first one. What changed is only that `narvo-render2d` now *can* record work that
is not a draw.

So every sentence above stands. "Two, not `n`" is a statement about the batch
list, and the batch list is untouched: `encode_runs` is unmodified, `quad.rs` is
unmodified, the blend state and `LoadOp::Clear` are unmodified, the limit is still
on the sum, and an empty second batch still *produces* nothing rather than drawing
nothing — which is still the regression evidence for the blessed references.

ADR-0049 records what was built, including the format and usage measurements
behind it and the rule that a merge may not depend on the order invocations run
in.

**Where the supersession would come from, named so it is not a surprise.** The
task that first records a compute pass *inside* a frame — M8.6 by the plan — has
to answer two questions this decision cannot: whether the draw and the chain
share one command encoder, and whether "a frame" then means the raster pass, the
whole sequence, or both. That is a decision about the frame's structure rather
than about the batch list, so it may well need to replace this ADR rather than
extend it. It is not being pre-empted here, because nothing yet consumes the
answer.
