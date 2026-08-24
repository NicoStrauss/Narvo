# ADR-0038 — The glyph atlas and the layout move into `narvo-render2d`

Status: accepted · M6.6b · 14.08.2026

## Context

`glyph_atlas` and `text` lived in `narvo-testkit`. That crate is
`publish = false` and a **dev**-dependency of everything that uses it
(ADR-0016), so nothing reachable from a production build could draw a glyph.

The placement was deliberate and its reasoning was written into `text.rs`'s own
module header, together with the condition under which it would stop holding:

> "*Considered and rejected: `narvo-render2d`.* Its argument is that text
> rendering is a render capability and belongs with the renderer. Against it: the
> region table would have to move there too, and it carries `ab_glyph`. […] When
> text becomes a shipped capability rather than a golden-image subject, moving it
> is the right change, and it is a decision with an ADR rather than a side effect
> of this task."

**That condition occurred.** M6.6b surveyed the two ways to build the debug
overlay M6.6 calls for, and the human decided (v1.06) against egui and for the
native text path. The measurements behind that decision:

- egui is **not in the tree** — no manifest, no `Cargo.lock` entry, no Rust
  source; five mentions across all tracked files, every one of them prose.
- `egui` + `egui-wgpu` + `egui-winit` at 0.36.1 resolve this workspace's own
  wgpu 30.0.0 and winit 0.30.13, so the route was open — at **105 packages** the
  lock does not have (346 → 451). ADR-0034 rejected `rmcp` at 28.
- The native path already draws text over a scene through the ordinary sprite
  pipeline, and two blessed references pin it.

An overlay is therefore more `SpriteInstance`s in the buffer the frame already
draws — but only if the code that produces them can be reached from a production
build. It could not.

## Decision

**`glyph_atlas` and `text`'s layout half move to `narvo-render2d`**, together
with `DejaVuSansMono.ttf` and its licence file. `narvo-render2d` declares
`ab_glyph` and `serde`, both optional and inside its existing `gpu` feature.

`narvo-testkit` keeps the two modules as **re-exports**, so every caller in the
workspace keeps the path it already wrote.

**`model_image` and `over` do not move.** They are the CPU model a golden scene
is predicted against, they depend on `narvo_testkit::srgb`, and a *normal*
dependency from `narvo-render2d` back to `narvo-testkit` is the one direction
ADR-0016's cycle does not permit. The line is the one `narvo-testkit`'s own
header already draws for itself: "Fixture *data* … **Not the rules that check
them**." A model of what a render should produce is a rule that checks.

> **Marked by M7.0 — the middle clause's premise has moved, the decision has
> not.** `srgb` is `narvo_render2d::srgb` now, so depending on it no longer
> implies a normal edge back to `narvo-testkit`, and this sentence's second
> reason no longer carries weight. The first and third are untouched and are
> enough on their own: `model_image` and `over` stay. See the amendment below.

## Consequences

- **No package moves.** `Cargo.lock` gains and loses no `[[package]]` block:
  346 before, 346 after. Three dependency *names* move from `narvo-testkit`'s
  block to `narvo-render2d`'s. `ab_glyph` was already in the lock, and on Linux
  already in `narvo-app`'s production graph through
  `winit -> sctk-adwaita -> ab_glyph`.
- **The headless tree is byte-identical**, measured before and after:
  `cargo tree -p narvo-app --no-default-features --edges normal` produces the
  same 142 lines, and `ab_glyph` appears in it zero times. `narvo-render2d` is
  not in that graph, so nothing it declares can reach it.
- **`cargo deny list` is byte-identical** before and after, and already named
  `ab_glyph@0.2.32` — the licence check has been inspecting this crate all along,
  because `cargo deny` resolves dev-dependencies of workspace members. This move
  cannot introduce an uninspected dependency; there was none to introduce.
- **Two modules are public where every other module in this crate is private.**
  `pub mod glyph_atlas` and `pub mod text`, against a crate that re-exports
  everything flat at its root. Flattening would put `rasterize`, `FONT`,
  `layout_line` and `sprites_for` into `narvo_render2d`'s root namespace, where
  `FONT` and `rasterize` say nothing about what they are.
- **Two names for one thing**, which is the re-export's price. What it buys is
  that the move touched **no test file at all**: the three blessed references
  drawn through this path — `text_lines_ascii_192x80`,
  `text_over_scene_192x80` and `click_counter_state3_128x128` — and every test
  that draws them are an *unmoved* reference to moved code.
- **20 unit tests changed crate**, 13 with `glyph_atlas` and 7 with the layout
  half of `text`. `narvo-testkit` 43 → 23, `narvo-render2d` 70 → 90, total
  unchanged.
- **The font travels with its licence file**, as that licence requires. Both are
  byte-identical to the blobs they were at `a418286`, and ADR-0037's guard is
  unaffected: no crate was added, so its "at least fifteen manifests" floor still
  sits on fifteen.

## What this does not decide

It does not build an overlay, an inspector, or any new drawing capability — that
is M6.6c. It does not widen what the text path can do: it is still one line of
ASCII 32–126 at 16 px or 32 px, left to right, no shaping, no kerning, no line
breaking, no colour (D10). Moving a capability is not extending it.

## Rejected alternatives

**A new `narvo-text` crate.** Argument: it keeps `narvo-render2d` free of a
font rasteriser and gives text its own home if it grows. Against it: layout
produces `SpriteInstance` and the atlas produces `Pixels` and `TextureRegion`,
all three `narvo-render2d`'s types, so the new crate would depend on it and add
an edge without removing one — a fourth crate in the graph to hold two modules
that only speak to the renderer. Rejected on cost, and it is the alternative to
revisit if text ever grows a second consumer that is not the renderer.

**Move `model_image` and `over` too.** Argument: keeping half of one module in
each crate is a split nobody would design from scratch. Against it: they need
`srgb`, which would drag a third module across, and then `narvo-render2d` would
own the model its own golden tests are checked against — a renderer grading its
own homework. The split follows a boundary that already existed.

**No re-export; update every caller.** Argument: one name for one thing, and the
`narvo_testkit::text` path stops lying about where the code lives. Against it:
six test files would change in the same commit as the implementation, and the
evidence that the move changed no behaviour is exactly that those files did not
change. The re-export can be removed later in a commit whose evidence does not
depend on it.

## Amendment, M7.0 (16.08.2026): `srgb` moved on its own, and this decision stands

*M7.0 moved `narvo_testkit::srgb` to `narvo_render2d::srgb`, under the same
rule and by the same mechanism this ADR used: the module stays where its callers
already wrote it, as a re-export, so no test file changed. This section exists
because the rejected alternative above **names `srgb` by name**, and a later
reader acting on that sentence would be acting on a premise that has moved.*

**What moved the premise.** The rejected alternative's case against moving
`model_image` and `over` had two parts: they need `srgb`, "which would drag a
third module across", and a renderer would then own the model its own golden
tests are checked against. The first part is spent — `srgb` is already across,
moved for its own reasons rather than as anybody's dependency. **The second part
is untouched and is the whole of what keeps `model_image` and `over` in
`narvo-testkit`.** It was enough on its own then and it is enough on its own
now; nothing in this ADR's Decision is reversed, weakened or superseded.

**Why `srgb` could move when `model_image` could not** — the distinction this
ADR did not need to draw and M7.0 does. A CPU model of a render is a prediction
of *this renderer's* output, so a renderer that owned it would be grading its own
homework. A transfer function is not a prediction of anything this crate
computes: **no production code in `narvo-render2d` performs an sRGB conversion,
in Rust or in WGSL** — the transfer on the render path is the hardware's, done by
the `…UnormSrgb` format. Measured rather than argued: `grep -rn -i "srgb"` over
`crates/narvo-render2d/src/` returns format names and prose and no arithmetic,
and the shaders contain the string once, in a comment. So a prediction built from
`srgb` and the frame it is compared against remain on opposite sides of the GPU
boundary, which is the property that keeps the golden scenes evidence.

That was checked by injection rather than left to the argument: falsifying the
encode exponent turned **three** golden comparisons red — `text_lines`,
`text_over_scene` and `tint` — which is exactly what a model and a render on
opposite sides of the GPU are supposed to do, and could not happen if the render
path were reading the same constant.

**Why this is an amendment and not a new ADR.** No architectural commitment was
made: no crate, no dependency, no feature gate, no changed signature.
`Cargo.lock` is byte-identical, and the boundary that decided the destination is
the one `narvo-testkit`'s own header already draws and that this ADR already
applied twice. The reasoning that had to outlive the conversation is in the moved
module's header, in the tracked tree.

### Revision condition for this amendment

If `narvo-render2d` ever performs an sRGB conversion in its own code — a
readback normalisation that decodes, a linear render target with an encode step,
a CPU-side composite — then the measurement above stops holding and the golden
comparisons that predict through `srgb` become a renderer checking itself. At
that point the test-side copies stop being redundant and this placement is
reopened.
