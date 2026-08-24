# ADR-0023: One pipeline, and it blends premultiplied

Status: accepted · Date: 2026-08 · Scope: `narvo-render2d` (the one pipeline),
every blessed reference, and the meaning of draw order

`ProjektPlan.md` §6/M4 books blending as a renderer task and names what it is
for — "HUD und Text-über-Szene brauchen es" — and says in the same line that
`blend: None` is **blessed semantics**, so changing it is a deliberate
re-blessing. This is that decision.

## Context

Until M4.7 the pipeline was created with `blend: None`, and there was exactly one
pipeline. A fragment therefore *replaced* whatever the target held, across the
whole of the quad it belonged to. For opaque content that is invisible. For a
glyph it is not: M3.35's blessed `text_lines_ascii_192x80` carries **1 217
transparent pixels**, one for every texel inside a glyph's rectangle whose
coverage is zero, each of them `[0, 0, 0, 0]` written over an opaque clear. That
report calls them "by design", and they were — of a design that could not put
text over anything.

## Decision 1 — premultiplied alpha, and straight alpha rejected

The source is already multiplied by its own coverage, so the colour blend is
`src + dst * (1 - src.a)` with **no factor on the source term**.

The representation was not invented here. `narvo_testkit::glyph_atlas` has
written coverage into all four channels since M3.34, with the comment that this
is "white with the coverage as its alpha, **already premultiplied**". So the
content the engine has was premultiplied before the pipeline that consumes it
existed.

**Rejected: straight alpha** (`SrcAlpha, OneMinusSrcAlpha`). Its best argument is
real and worth stating: it is what an author expects when they type a colour and
an opacity, it is what most image files store, and it needs no discipline of the
content pipeline to stay correct.

Against it, in the order that decided:

1. **It is wrong under filtering, and this renderer filters.** D13 chose `Linear`
   for sprite atlases. Interpolating straight-alpha texels drags colour toward a
   transparent neighbour's colour — which is arbitrary, since a fully transparent
   texel has no meaningful colour — and produces dark or coloured fringes.
   Premultiplied interpolation cannot: the colour is already weighted by the
   coverage it belongs to. The glyph atlas's own doc records exactly this as its
   reason.
2. **The existing content would have to be converted**, and the conversion is
   lossy in eight bits.
3. **It does not compose.** A target blended into twice must be a valid source
   for the third draw. Premultiplied `OVER` is closed under that; straight alpha
   is not without a divide.

## Decision 2 — the alpha channel is `OVER` too

`a_out = a_src + a_dst * (1 - a_src)`, the same component on both halves. This
one was left to this task to decide and justify.

Two alternatives were available:

- **`One, Zero`** — the source's alpha replaces the target's. A half-transparent
  sprite would then punch a half-transparent hole through an opaque scene, and
  the read-back would carry an alpha nobody composited for. **A picture does not
  show this**: every colour channel still looks right, which is why
  `compositing_over_an_opaque_ground_leaves_nothing_transparent` asserts the
  alpha plane image-wide rather than trusting the eye.
- **`Zero, One`** — keep the target's alpha. Indistinguishable from the above on
  an opaque background, wrong the moment anything is drawn onto a transparent
  one.

`OVER` is the only one of the three that **keeps the result premultiplied**. That
invariant — `rgb <= a` out whenever `rgb <= a` in — is what makes Decision 3
possible at all, because it is what lets a target that has been drawn into serve
as the source-side representation for the next draw.

The spelled-out state is asserted to equal `wgpu`'s
`PREMULTIPLIED_ALPHA_BLENDING`, so the account above and the arithmetic that runs
cannot drift apart.

## Decision 3 — one pipeline, and no opaque second one

`blend: None` is gone as a configuration. There is one pipeline and it blends.

**Rejected: a second, non-blending pipeline for opaque content.** Its best
argument is performance — a blend is a read-modify-write of the target, an opaque
write is not, and a renderer that knows a sprite is opaque could skip it.

Against it: **the saving is unmeasured and the cost is a fork in the render
path.** Every question this project asks about a picture would have to be asked
twice, and D15's per-run sampler already showed how quickly "which pipeline drew
this" becomes the first question of every investigation. The measurement below
also removes the urgency: the blend pipeline reproduces the unblended one **bit
for bit** on five of six blessed references across three rasterisers, so nothing
about the picture argues for keeping both. Reopen when a frame-time measurement —
not an argument — shows the blend costing something the budget notices.

## Decision 4 — alpha comes from the texture, and only from there

No per-sprite alpha, no colour modulation. A sprite's transparency is what its
texels say.

The reason is the same one ADR-0015 gives for the renderer taking scalars: a
per-sprite alpha would be simulation state, therefore a registered component,
therefore in the state hash and in every scene file — a decision with reach far
past this pipeline. Nothing asks for it yet. Reopen when a consumer does: a fade,
a hit flash, a HUD that dims.

## The survey, and its three answers

Taken before anything was designed, because two of the three could have stopped
the task.

**Glyph coverage is already premultiplied.** `glyph_atlas` writes `[c, c, c, c]`.
No collision, and no conversion needed — the halt condition did not fire.

**Draw order is back-to-front.** ADR-0004's M3.11 amendment settles it: "larger
depth is drawn later and therefore ends up in front", and `placements_of` sorts
ascending by depth and then by entity id. So the far thing is drawn first, which
is the precondition alpha compositing needs. The halt condition did not fire, and
nothing was turned around.

**And that gives draw order a load it did not carry.** Under `blend: None` order
decided only *which* sprite won a pixel. Under blending, order is **composition**:
the same sprites in a different order are a different picture wherever they
overlap. `layer_order_regions_128x128` and the depth rule in ADR-0004 were
already the place that order is decided; what changes is the cost of getting it
wrong, and that a sorted-by-depth list is now part of the arithmetic rather than
a tie-break. There is still no depth buffer (`depth_stencil: None`), and adding
one would now change results rather than only performance — ADR-0004's own note
about a future depth buffer should be read with that in mind.

## The measurement, which was the point of the task

**[Referenced by ADR-0046, M7.1d.]** The table below is the closest thing this
repository has to prior evidence that the blessed stock survives a change of
rasteriser — it already spans AMD Vulkan, WARP over Dx12 and lavapipe. M7.1d
moved the *window* to DX12 on Windows and left the offscreen instance, which
draws every reference, exactly where it was; so this table is prior evidence and
not that task's own measurement, and the numbers below are unmoved.

The pipeline was flipped and then the **whole blessed stock** was measured, on
all three locally available rasterisers: RX 9070 XT (Vulkan, discrete), WARP
(Dx12, software), lavapipe (Vulkan, software, under WSL). Differing pixels over
the 4-count floor, then worst channel deviation.

| Reference | class before | AMD | WARP | lavapipe |
|---|---|---|---|---|
| `textured_quad_quadrants_64x64` | byte-exact anchor | 0 / **0** | 0 / **0** | 0 / **0** |
| `sprite_atlas_regions_128x128` | tolerance | 0 / 0 | 0 / 0 | 0 / 1 |
| `placed_sprite_quadrants_128x128` | tolerance | 0 / 0 | 0 / 2 | 0 / 1 |
| `layer_order_regions_128x128` | tolerance | 0 / 0 | 0 / 0 | 0 / 0 |
| `camera_regions_128x128` | tolerance | 0 / 0 | 0 / 1 | 0 / 2 |
| `text_lines_ascii_192x80` | byte-exact anchor | **2 527 / 255** | **2 527 / 255** | **2 527 / 255** |

**Five of six did not move at all**, and every one of those numbers is
byte-for-byte the number the same run produced before the flip. Opaque
mathematics — `src * 1 + dst * 0` at `a = 1` — makes identity *possible*; three
rasterisers reproducing it exactly is what makes it a fact rather than an
expectation.

**The sixth moved, and only in one plane.** A channel-wise comparison of the
pre-blend reference against the post-blend render, identical on all three
rasterisers:

- 2 591 pixels differ at all (2 527 of them by more than the 4-count floor);
- **0 pixels differ in R, G or B** — the colour image is byte-identical;
- 2 591 differ in alpha, every one of them ending at 255;
- of those, 1 217 came from alpha 0 — exactly the count M3.35's model committed
  as the transparent-inside-a-box class, counted by a second instrument.

So the change did precisely one thing to the blessed stock: it removed the holes
from the text image. That is the announced semantic change, and nothing else
came with it.

## The two new references, and their classes by finding

Both were rendered on the RX 9070 XT and then held against the two software
rasterisers, the same way M3.5a established the practice.

**`blend_proof_steps_128x128` — byte-exact anchor, the third.** Eight 32 × 32
panels of four alpha steps over an opaque red ground and an opaque black one,
drawn `Nearest` at four pixels per texel with every edge on a whole pixel.
WARP and lavapipe both report *"none, the images are identical"*. It earns
byte-exactness the way `textured_quad` does: uniform content on the pixel grid
gives two implementations nothing to round differently.

**`text_over_scene_192x80` — tolerance class.** The same layout and conditions as
`text_lines`, over a solid red ground. 0 pixels over the floor on both software
rasterisers, worst channel deviation **1 count**, at **(129, 14)** on WARP and
**(126, 14)** on lavapipe — different pixels, same one count, which is M3.27's
"worst places are not rasteriser-stable" showing up again.

It is the **first reference whose content depends on the blend arithmetic**, and
therefore the first that cannot be byte-exact: the one channel that needs the
sRGB transfer function is the one that spreads. Green, blue and alpha are exact
everywhere, on every rasteriser, because the red ground contributes nothing to
them.

## Where the arithmetic happens — a premise that did not hold

The task was set with the blend written as `(dst * (255 - a)) / 255`, and steps
chosen so that it divides without a remainder. **That is not the arithmetic this
pipeline performs**, and the difference is not a detail.

The target is `Rgba8UnormSrgb`. The hardware decodes the target's stored bytes to
linear light, blends there, and re-encodes on write. `blend_proof` was built to
settle it by measurement rather than by assertion: the fixture's steps make
**both** readings whole numbers over both grounds, so no rounding enters, and the
test computes both predictions and reports which one the render matches.

```
step  85 over red:  measured [226, 85, 85, 255]   linear-light [226, ...]  stored-bytes [255, ...]
step 170 over red:  measured [223, 170, 170, 255] linear-light [223, ...]  stored-bytes [255, ...]
```

**Linear light matches 8 of 8 probes; stored bytes 6 of 8** — agreeing only where
the two readings coincide. The prediction was computed by hand before the render
and hit both discriminating values on the counter.

The consequence for anyone predicting a future image: over a black ground the
composite is exact and needs no transfer function, and over anything else it does.
The CPU model in `narvo_testkit::text` now carries that arithmetic, and
`text_over_scene`'s model test measures where the CPU and the GPU disagree rather
than pretending they cannot: **83 of 15 360 pixels, all by 1 count, all in red.**

## Blessing procedure

One re-blessing and two first blessings, and M4.7 ends **without a commit** so
that all three can be one.

The choreography §12 records — "Liste + Datei **im selben Commit**" — was not
what M3.35 actually did: `7f87500` added the list entries and left the tree red,
and `c63d146` added the PNG. That worked because the agent's commit was allowed
to be red. Ending this task uncommitted achieves the rule as written instead: the
list entries, the scenes, the tests and the three PNGs are all in the working
tree together, the tree is **green**, and the human's single commit satisfies
both directions of `every_blessed_reference_has_a_scene_here` at once.

What the human is blessing is therefore not "a file that appeared" but a tree in
which every instrument already agrees with the candidate. The M4.7 report carries
the panel layout, the expected values at named points, and the exact `git add`
list.

## Consequences

- **Draw order is composition.** See Decision 4's neighbour above. The five
  scenes blessed before this one were order-sensitive only where they overlapped;
  every scene from here on is order-sensitive wherever anything is not opaque.
- **The replica pipelines had to follow.** Five test files build their own
  pipeline as a control against production; all six sites now carry the same
  blend state, named rather than spelled out so that the two statements stay
  independent. Three of them showed nothing when the pipeline changed, because
  their scenes are opaque — they were updated anyway, since a control that
  replicates a pipeline nobody has is a trap waiting for the scene that would
  reveal it.
- **A fourth copy of the sRGB transfer function now exists** (`narvo-testkit`,
  `blend_proof.rs`, `camera_motion.rs`, `camera_pan_steps.rs`). Reported rather
  than folded: three of them are deliberate independent statements inside
  comparisons that would otherwise move both sides together. Deciding which are
  redundant is not this task's, and §6.10's third-`sha256` case is the reason it
  is written down rather than left to be discovered.
- **`Tolerance::default`'s trigger has now fired twice.** Its doc named "glyph
  coverage" as the content that would make the numbers worth arguing about;
  `text_lines` was that image and did not need them, and `text_over_scene` is the
  first that does — 1 count, against a floor of 4. Still no argument for changing
  them, now with a second measurement behind that.

## Revision condition

Reopen when a frame-time measurement shows the blend costing something the budget
notices, which is the only argument the rejected second pipeline has left. Reopen
when a consumer asks for per-sprite alpha or colour modulation. Reopen if a depth
buffer is ever proposed, because with blending in place it changes results and
not only cost. And reopen if a reference is ever blessed whose content is
partially transparent over a partially transparent ground — nothing in the stock
composites onto a non-opaque target, so the alpha half of `OVER` is exercised
only in its `a_dst = 255` case.
