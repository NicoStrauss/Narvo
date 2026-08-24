# ADR-0020: The asset contract, and the packer that produces it

Status: accepted · Date: 2026-08 · Scope: `narvo-assets` (the contract types and
the packer), the SHA-256 that anchors an atlas, and every future source of asset
regions

`ProjektPlan.md` §6/M4 decided the *shape* of the answer — "die Pipeline
definiert einen **Asset-Contract** … der D3-Packer ist das Werkzeug, das den
Contract herstellt; **die Quelle ist unter dem Contract austauschbar**" — and
left what the contract actually says to the task that builds it. This is that
decision.

## Context

M3.34's glyph generator already produced an atlas the renderer's guard accepts:
rasterised glyphs, shelf-packed, one-texel padded, anchored by a SHA-256 over
pixels and table. It is a *precedent* and not a contract, because everything
about it is specific to glyphs — its sizes are uniform, its order is ASCII, and
its consumer is one test.

M4's pixel-art slice needs item icons in quantity, of different sizes, from a
source that may eventually be a file. The contract is what makes that a change of
*source* rather than a change of everything downstream.

## Decision 1 — what the contract is

A consumer hands in **named source regions** (RGBA8 pixels, a width, a height)
and gets back **one atlas**: a texture, and a table from name to rectangle. It
may rely on exactly this:

1. **The atlas is a function of the region set alone.** Not of the order they
   were given in, not of the machine, not of the run. Same set, same bytes,
   always.
2. **Every region is padded to the rule the renderer checks.** One texel of
   edge-extension on every side, which `narvo_render2d::check_region_padding`
   verifies texel by texel. The packer satisfies it by construction; the test
   proves it by calling the guard.
3. **No two regions overlap, padding included**, and every region is inside the
   atlas.
4. **A region's pixels arrive verbatim.** No resampling, no premultiplication,
   no colour conversion.
5. **The table is what the renderer samples with.** The four numbers go to
   `TextureRegion::from_texels` and come out as the coordinates the shader uses.
6. **A SHA-256 anchor** moves whenever a pixel or a placement does.

What a consumer may **not** rely on: which rectangle any given region lands in.
That is the packer's, and it is free to change — with the consequence Decision 5
governs.

## Decision 2 — shelves, sorted by name

The packer sorts by name, then places each region to the right of the last on the
current shelf, starting a new shelf whose height is its own when it does not fit.
The atlas is square, a power of two, grown by doubling until everything fits or
[`MAX_ATLAS_SIDE`] is passed.

**Why the simplest algorithm that carries the consumers.** §6/M4's guardrail, and
it is the right one: optimal area is not a goal, determinism and guard conformance
are. Shelves are what M3.34 already uses, so the packer inherits a shape whose
behaviour is understood in this repository rather than introducing a second one.

**Rejected: MAXRECTS, skyline, or any other bin-packing heuristic.** Best
argument: markedly better occupancy on heterogeneous sets, which is exactly the
set M4 is about — a shelf packer wastes the height difference between the tallest
region on a shelf and every other one, and icon sets are heterogeneous by nature.
Against it: it is several hundred lines of placement heuristics whose value is
measured in wasted texels, and nothing in this project has yet been slowed by an
atlas being larger than it needed to be. The revision condition is a *measurement*
— an atlas that does not fit `MAX_ATLAS_SIDE` under shelves and would under a
better packer — and not a feeling that shelves are unsophisticated.

**Rejected: a packing dependency.** No measurement was taken because none was
needed to decide: the clap and sha2 precedents (M4.2, M4.3) both weighed a
dependency against a small amount of written-out code, and a shelf packer is
forty lines. A dependency would have to be measured before it could be taken;
that is the standing rule and it is why this line exists rather than a crate.

### Sorting by name, and what it costs

The key is the region's name, which is total because names are unique within a
pack — a duplicate is an error before any placement happens.

**Rejected: input-stable order** (place them as given). Best argument: the caller
controls the layout, which is real — a caller that knows its access patterns
could group related regions and it cannot now. Against it: an anchor that depends
on call order makes every harmless refactor of the call site an anchor change. A
reordered `vec![]` in a consumer would break a committed constant and produce a
diagnostic about atlas contents for a change that touched none. The property is
asserted over every rotation of the input plus its reverse.

## Decision 3 — padding, taken from the existing rule rather than invented

**Border width: one texel.** Not decided here. `ceil((k+1)/2)` texels for `k`
texels per pixel is derived in `narvo-testkit`'s glyph atlas module for M3.34,
and at `k ≤ 1` — magnification and 1:1, every sampler configuration this engine
ships — that is 1. `narvo_render2d::REGION_PADDING_TEXELS` is the same number,
and a test asserts the two constants have not drifted, because this crate cannot
depend on that one and a silently different border would produce atlases the
renderer's own guard rejects.

**Padding content: edge extension.** Every border texel is a copy of the content
texel nearest to it, clamped on each axis independently — the region's corner
texel for a corner, the nearest edge texel otherwise. That is not a choice made
here either: it is precisely what `check_region_padding` verifies, and the packer
exists to satisfy that guard.

**What the guard checks is content, not spacing.** Worth stating because the two
are easy to conflate: it does not measure gaps, it compares each border texel
against the content texel it should be a copy of, and reports the first
mismatch by edge and coordinate. A packer that left the right *gaps* and did not
fill them would fail it — which is the M4.4 report's first red demonstration, and
it did fail, naming the texel.

No deviation from the glyph semantics was needed, so none was taken.

## Decision 4 — the anchor runs over pixels *and* table

`anchor_bytes` is the atlas dimensions, then the pixels, then the region count,
then each `(name length, name, left, top, width, height)` in name order. SHA-256
over that.

**Both halves, for M3.34's reason:** either alone misses what the other is for. A
moved region changes no pixel value if the pixels are identical; a recoloured
region moves no placement. An anchor over one of them would sleep through the
other, and a test asserts both directions.

Two details that are not decoration. The **count goes in before the entries**, so
a table that lost a row cannot hash like a shorter one written on purpose — the
"count before hash" lesson M3.33 and M3.34 both record. And each **name is
preceded by its length**, because without it `"ab" + "c"` and `"a" + "bc"` would
produce the same blob.

### The re-anchor procedure

The anchor is a **content anchor over a generated artefact** — ADR-0008's third
kind of literal, permitted where a *state* hash is forbidden. The distinction is
that nothing outside this repository can move it: the regions are written in the
test, the packer is in this workspace, and SHA-256 is frozen by FIPS 180-4. No
dependency bump changes it, so a break is a finding rather than noise.

**When it breaks:**

1. Work out which half moved — a placement or a pixel. The two other anchor tests
   narrow it in one run.
2. Decide whether the move was intended. A packer change, a padding change or an
   encoding change all move it legitimately.
3. Put the new value in, and **report the reason in the task's report**.

An anchor change needs **no blessing**, and that is the difference from a golden
image: the value is fully derivable from code in this repository, so a reviewer
can recompute it rather than having to look at it. What it does need is a stated
reason. The failure message says so, because the one thing that must not happen
is a new value pasted in to turn a red run green.

## Decision 5 — where SHA-256 lives

`narvo-core`. It arrived in `narvo-app` with M4.3's scene anchor and could not
stay there: `narvo-app` is a binary with no library target, so `narvo-assets`
cannot see it. `narvo-core` is where both consumers can, and it is already the
crate that holds what has no dependencies of its own.

**Rejected: a deliberate second copy, on the xtask precedent.** Best argument is
real and is recorded in `ProjektPlan.md` §5.1: `xtask` keeps a full copy of the
dump localiser rather than depending on `narvo-ecs`, because the dependency
would cost a full ECS build on every `cargo xtask ci` and the §8.1 budget is
tight. Against it here: `narvo-assets` already depends on `narvo-core`, so the
edge exists and costs nothing — the xtask argument is about a dependency that
would be *new*, and this one is not.

**A third copy exists and is deliberately untouched.** `narvo-testkit` has its
own, written for M3.34's glyph anchor. Folding it in would be right and is not
M4.4's: the glyph atlas's committed anchor and its blessing are outside this
task's scope, and the fold is one import when somebody takes it. The M4.4 report
records it as a finding, including that M4.3 introduced its copy without noticing
this one.

## Non-goals, each with the condition that would reopen it

- **A file source, an image decoder, and any on-disk atlas format.** The contract
  is what makes these a later step: a decoder becomes one more way to produce a
  `SourceRegion`, and nothing downstream of the packer learns that a file
  existed. Reopens when the pixel-art slice needs its first icon from disk.
- **Rotation.** Placing a tall region on its side packs some sets much better and
  costs every consumer a flag in the table plus a swap in the shader's
  coordinates. Reopens with a set that does not fit without it.
- **Trimming** — cropping a region's transparent margin and recording the offset.
  Same shape of cost: the table grows a field that every consumer must apply.
  Reopens when a real icon set is measured to be mostly empty.
- **Mip levels.** The border formula is `ceil((k+1)/2)` and this packer implements
  the `k ≤ 1` case. Minification needs more border *and* mipmaps, and D17 already
  carries that as an open front where mipmaps change the question rather than the
  border. Reopens with a consumer that samples an atlas minified.
- **More than one atlas.** `pack` produces one, and a set too large for
  `MAX_ATLAS_SIDE` is an error naming the numbers rather than a silent split.
  Distributing across atlases is a policy — which regions belong together — and
  policy needs a consumer with an opinion. Reopens when one exists.

## Consequences

- **`narvo-assets` stops being empty.** Its manifest reserved `narvo-core` for
  this since M2.8; the reservation is now spent.
- **A dev-dependency on `narvo-render2d`, never a normal one.** The guard and
  the end-to-end proof both need it, and `narvo-app`'s headless job asserts with
  `cargo tree --edges normal` that no graphics crate is reachable — through
  `narvo-assets`, which it depends on. A dev edge does not appear there. Same
  shape as ADR-0016's `narvo-testkit`, and there is no cycle in either
  direction.
- **The padding constant is duplicated across two crates that cannot see each
  other**, and a test is the only thing holding them together. That is the cost
  of keeping graphics out of the asset crate's production tree, and it is cheaper
  than the alternative.
- **The atlas is square and a power of two even when a rectangle would fit
  better.** Deliberate: it is one number instead of two to reason about, and
  every adapter samples it. A set that fits in 64 × 32 gets 64 × 64.
- **An empty region set is an empty atlas, not an error.** A consumer that
  packs a configurable set should not need a special case for the empty one.
