# ADR-0024: PNG files under the asset contract, and a component that names a region

Status: accepted · Date: 2026-08 · Scope: `narvo-assets` (the decoder),
`narvo-ecs` (the eighth registered component), `narvo-app` (resolution and the
scene-file window mode)

`ProjektPlan.md` §6/M4 chose the **Contract-Weg** in v0.41: the pipeline defines
an asset contract, the packer is the tool that produces it, and *"die Quelle ist
unter dem Contract austauschbar — code-generiert oder Datei"*. M4.4 built the
contract with a code source. This is the file source, and the sentence above is
the thing it has to make true.

## Decision 1 — the decoder is `png`, on three measurements

Candidates: `png`, `image`, and writing one out.

| | new crates in the lock file | clean build | headless guard |
|---|---|---|---|
| **`png`** | **0** | **3.60 s** | passes |
| `image` | 0 | 19.02 s | **fails** |

**Every crate either one needs was already in the lock file**, because
`narvo-render2d` has carried `image` (with the `png` feature) since M3 to read
and write golden images. `git diff Cargo.lock` for this change adds no
`[[package]]` block at all — one line, naming the new dependency edge.

What decided it against `image` was not the build time but the guard.
`.github/workflows/ci.yml` fails if
`cargo tree -p narvo-app --no-default-features --edges normal` contains
`image`, and `narvo-assets` is in that tree. Adding `image` here would trip a
guard that exists to keep the graphics stack out of the headless build — and
what `image` adds over `png` is `moxcms`, a colour-management stack, for a task
whose whole point is that no colour management happens.

**Rejected: writing a decoder out**, and this is the one that deserved a real
argument rather than a dismissal, because this project has written out SHA-256
and the sRGB transfer function on exactly that reasoning.

Its best argument: a decoder that never changes cannot surprise a blessed
artefact, and eighty lines of frozen standard have twice been worth more here
than a dependency.

**Why it does not carry, and the difference is the whole point:** SHA-256 is a
fixed, self-contained, eighty-line function published in FIPS 180-4, and this
project's copy is checkable against published vectors. PNG is a container format
whose payload is **zlib** — Huffman coding, LZ77 windows, adaptive per-scanline
filters, a CRC per chunk, interlacing. Writing that out is not transcribing a
standard, it is implementing a compression library, and the failure mode is not
"a wrong hash" but "an image that decodes subtly wrong on some input nobody
tested". The sha256 precedent argues *against* this, not for it: it earned its
place by being small enough to check completely, and inflate is not.

So the honest form of the rule is: **write out what can be checked completely,
depend on what cannot.**

## Decision 2 — PNG only, with exact conversions and one refusal

Accepted, because every one of these expands to RGBA8 **exactly**: 8-bit RGBA;
8-bit RGB (alpha becomes 255); grayscale at 1, 2, 4 or 8 bits (expanded, then
`r = g = b`); grayscale with alpha; palette, including a `tRNS` chunk becoming
real alpha.

**Refused: 16 bits per sample.** `png` would strip it happily
(`Transformations::STRIP_16`) and the result would look right. It is refused
because discarding half of every sample in a file somebody deliberately authored
at 16 bits is a loss nobody discovers. The message names the file and says what
to do:

> `deep.png` has 16 bits per sample, and expanding it to 8 would discard half of
> every sample. Re-export it at 8 bits per sample; this decoder refuses a lossy
> conversion rather than making one quietly

Other formats are a non-goal, not an oversight — see the list at the end.

## Decision 3 — premultiply at load, in byte space, with the rounding named

```text
out = (colour * alpha + 127) / 255        // integer division
```

ADR-0023 made the pipeline consume premultiplied colour; an image editor writes
straight alpha; somebody has to multiply. Doing it **at load** is what keeps
every later stage — packer, padding guard, anchor, upload, blend — looking at one
representation.

**Byte space, integer arithmetic, no floating point.** An atlas built from files
is hashed into ADR-0020's anchor, so the arithmetic that produces its pixels must
give the same answer on every machine, and integer arithmetic is the only kind
this project is willing to promise that about.

**The `+ 127` is round-to-nearest**, checked exhaustively rather than asserted:
`the_rounding_is_round_to_nearest_on_all_65536_pairs` compares every one of the
65 536 possible inputs against `f64` rounding. Truncation would darken every
partially transparent pixel by up to one count — small, systematic, and invisible
until an edge looks dirty.

**`alpha == 0` gives `rgb == 0`, and that falls out of the formula rather than
being a special case:** `colour * 0 + 127` is 127, and `127 / 255` is 0 for every
colour. This is the "dirty transparent" case — a bright colour left under a fully
erased pixel, which every editor produces and which bleeds into its neighbours
under `Linear`. M4.8 probes it through a real file and a real render.

**Rejected: premultiplying in linear light.** Its best argument is real and will
come back: the blend that consumes these pixels happens in linear light
(ADR-0023's measurement), so multiplying coverage into *encoded* bytes is not the
same operation as multiplying it into light, and text-like content shows the
difference at its edges.

Against it, for now: the glyph atlas has premultiplied in byte space since M3.34,
`blend_proof` and `text_over_scene` are blessed against that arithmetic, and
changing this would move both. Doing it in linear light also means an encoded →
linear → encoded round trip per channel, which either introduces floating point
into the anchor's input or needs a fixed-point table that is its own decision.
**Revision condition:** a measured case where byte-space premultiplication is
visibly wrong — most likely file-loaded text or a soft edge over a bright
ground — at which point this is reopened together with the atlas's own
convention, because the two have to agree.

> **Amended by M6b.3 — the open question above is answered for a *second*
> premultiplication this decision does not govern, and the revision condition
> for this one did not occur.** The tint premultiplies in `f32` at draw time and
> in linear light; this decision's arithmetic on stored bytes is untouched. The
> wording here is deliberately unedited — see the amendment below for the
> measurement, and for why the workspace now premultiplies in two spaces on
> purpose rather than by oversight.

## Decision 4 — `Sprite { region: String }`, the eighth registered component

A scene names a region; it does not carry a rectangle.

**Rejected: storing the four texel coordinates.** A rectangle is a fact about one
*packing* — where the packer happened to put a region on the day it ran — and
ADR-0020 makes the packing an output. Add an asset and every rectangle moves, so
a scene holding rectangles would need rewriting by a tool whenever the atlas was
rebuilt, and a recording made against it would describe a world nobody could
reconstruct. A name survives all of it, and it is what the contract already
speaks.

**The first string in a registered component**, so ADR-0014 has to be answered
rather than waved at. That ADR's reasoning is about a *dependency's formatting
decisions* entering ADR-0008's stability domain: a maths library's vector type or
an enum's RON representation is serde's to render. A `String` is not that case —
RON writes a string as a quoted string and no representation attribute changes
it. What it does bring is **escaping**, which is a real surface and is checked
directly, on names carrying quotes, backslashes, newlines, unicode and the empty
string.

**An unresolvable name is not the component's problem.** It stores; resolution
happens where the world and the atlas are both in scope. Same construction as
`Sampling` not rejecting an unknown filter code and `Layer` not rejecting `NaN`.

## Decision 5 — `assets/` beside the scene, stem is the name, everything loads

A scene at `levels/one.ron` loads `levels/assets/`. Nothing in the scene format
names the directory, which keeps ADR-0018's rule that asset paths stay out of the
format: a convention needs no syntax, no validation rule and no escaping
question.

**Every file is loaded, and an unused region is legal.** M4.2 decided there are
exactly two warning classes and this adds none — "you have an asset you are not
using" is a lint about content taste. The reverse is a **load error**: a name no
region carries is a scene that cannot be drawn as written, and the message names
the ones that exist, in the M4.2 measure.

**Case:** the region name is the stem **verbatim**, so `Hero.png` is `Hero`.
Duplicate *detection* ignores case, so `Hero.png` and `hero.png` in one directory
are refused on both platforms — they can coexist on Linux and cannot on Windows,
so a directory holding both describes a project that loads on one machine and not
the other. **Rejected: lower-casing the name**, which would make the name in a
scene something other than the name of the file — the indirection this design
exists to avoid.

**An empty directory is not an error.** It packs to nothing, and a scene that
then names a region gets the unknown-region message, which says "there are none".
That is a more useful place to be told than earlier.

## Decision 6 — `QuadrantBySlot` is retired

M4.6 needed a picture before a scene could say what an entity looked like, so it
derived one from draw-order position, and its own documentation called that
transitional. This is its replacement, and the old rule is **removed** rather
than kept beside the new one.

**Its one visible consequence: an entity with no `Sprite` no longer draws in the
scene-file mode.** Under the old rule every transform drew something whether or
not the content asked for it. What a thing looks like is now content.

`placements_of` is **untouched**, and that is measured rather than claimed: all
eight blessed references compare at 0 differing pixels and worst deviation 0
after this change. The new extraction is a second function, `regions_of`, which
duplicates the sort rather than sharing it — because sharing would mean editing
the function six blessed images are drawn through. The duplication is paid for by
`the_two_extractions_agree_on_order_when_every_entity_has_a_sprite`, which renders
one world through both and compares: they agree **by measurement** rather than by
construction, which is the stronger statement.

## Decision 7 — assets are outside the anchor's domain, and what that costs

ADR-0019's anchor is the scene file's bytes. It is **not** extended to cover
`assets/`, and the honest consequence has to be written down rather than left
implied:

> **A replay guarantees simulation fidelity, not image fidelity.**

Replace an icon's PNG between a recording and its replay and the replay is
byte-identical in every dump and every state hash — the region *name* is what the
world holds, and the name did not change — while the picture is different. That
is a named surface, not an oversight, and it is the right one for this ADR:
extending the anchor over a directory means deciding what happens when an unused
asset changes, and that question belongs with the multi-file initial state
ADR-0019 already defers.

**Revision condition:** the first time an image difference has to be reproducible
from a recording — a visual regression suite driven by replays would be the
trigger.

## Non-goals, each with the condition that would reopen it

- **Asset hot reload.** Scene hot reload exists (ADR-0022); the assets are read
  once, when the scene loads. Reopens on the same front as external prefab files
  and the multi-file anchor — a scene whose initial state depends on more than
  one file is one question, not three, and ADR-0019 owns it.
- **Formats other than PNG.** Reopens when a content set exists that cannot be
  authored as PNG.
- **Mips and compression.** Reopens when a measurement shows texture bandwidth or
  minification quality costing something.
- **Streaming and refcounting.** Formally cancelled in v0.47: loading at scene
  change is enough for the M7 slice, and streaming would be a feature with no
  consumer (§2).
- **A committed binary.** No PNG is committed, here or anywhere. Test images are
  encoded by the tests that read them, and the runnable demo under
  `target/m48-demo/` is written by a test rather than checked in.

  > **Corrected in M7.9 — see the amendment at the end of this file.** The
  > absolute is false, and was false on the day it was written: twelve PNGs are
  > tracked. It is left standing rather than edited, because what it got wrong
  > is more useful than a quiet fix would be.

## Revision condition

Reopen Decision 3 on a measured case where byte-space premultiplication is
visibly wrong. Reopen Decision 7 when image fidelity has to survive a replay.
Reopen Decision 1 if `png` ever stops being maintained or if the workspace stops
carrying it for other reasons — the measurement that chose it was partly "it is
already here", and that half can expire.

## Amendment, M6b.3 (2026-08-15): the tint multiplies in linear light, and Decision 3's condition did not occur

**The plan asked this ADR a question it turns out not to own.** `ProjektPlan.md`
§6/M6b lists the colour modulation's largest unknown as *"linear oder encoded"*,
and the natural place to look was Decision 3, which is the workspace's only
written position on that axis. M6b.3's survey was told to read the condition
verbatim and answer it. Reading it verbatim is what settles it:

> Reopen Decision 3 on a measured case where byte-space premultiplication is
> visibly wrong.

**That case has not been produced, and this task did not look for one.** Decision
3 governs turning an image file's straight-alpha *bytes* into premultiplied
bytes, once, at load, in integer arithmetic, because the result is hashed into
ADR-0020's anchor. The tint is a different operation on the other side of the
upload: a per-draw multiply in `f32`, on a value that is already premultiplied
and already decoded, which never touches a stored byte and never reaches the
anchor. Neither the atlas's pixels nor the arithmetic that produced them moved.

So the honest answer is two answers, and they are not the same answer:

- **Decision 3 stands, its revision condition untriggered.** This amendment
  records that the condition was examined rather than assumed, because the
  prompt that opened this task asserted the condition *had* occurred and that
  assertion is a claim like any other.
- **The plan's open question is closed by measurement, not by decision.**

## What was measured

The multiplication happens in `quad.wgsl`'s fragment shader, on what
`textureSample` returned. Both ends of that are sRGB formats:

- the atlas texture is `Rgba8UnormSrgb` — `quad.rs`'s `bind_texture` is the
  only upload path in the crate;
- the offscreen render target is `Rgba8UnormSrgb` — `offscreen.rs`'s
  `TARGET_FORMAT`.

wgpu's own documentation of that format is the citation: *"Srgb-color [0, 255]
converted to/from linear-color float [0, 1] in shader"*
(`wgpu-types-30.0.0/src/texture/format.rs:186`). The shader therefore multiplies
linear light, and re-encoding happens on write. **Nobody chooses this. The
formats choose it**, and they were chosen in M1 and M4.4 for other reasons.

Because a citation is still only a citation, the two readings were separated on
a rendered pixel. A white texel under a tint of `0.5` reads back:

| reading | predicted byte | measured |
|---|---|---|
| linear light | 188 | **188** |
| stored byte | 128 | — |

`the_half_tint_lands_where_a_linear_multiply_puts_it` in
`narvo-render2d/tests/tint.rs` is that measurement, and it names both readings
in its failure message: 60 counts apart, so no rounding and no rasteriser can
carry one into the other.

**What this costs a content author, stated rather than left to be discovered:**
`0.5` is half the *light*, not half the stored byte. A sprite tinted `0.5` does
not look "half as bright" to the eye that judges bytes. That is the price of the
formats being what they are, and it is written into `Tint`'s own documentation
where an author will meet it.

## Two premultiplications in two spaces, on purpose

After this task the workspace premultiplies in two places:

- **the atlas, at load, in byte space** — this ADR's Decision 3;
- **the tint, at draw, in linear `f32`** — `SpriteTint::premultiplied`.

That looks like the inconsistency Decision 3's own condition warns about
("reopened together with the atlas's own convention, because the two have to
agree"), and it is worth saying exactly why it is not.

The two never meet as alternatives. The atlas's premultiplication produces the
texel; the tint's produces a factor; the shader multiplies one by the other.
And the invariant survives the crossing with room to spare: byte-space
premultiplication guarantees `rgb <= a` on *stored* bytes, sRGB decoding is
monotonic with `decode(x) <= x` on `0..=1`, and alpha is not sRGB-encoded — so
`rgb_linear <= rgb_encoded <= a` still holds after the decode. Byte-space
premultiplication is **conservative** with respect to the linear-space
invariant, not merely compatible with it.

What a tint cannot do is make Decision 3's known artefact worse. The tint scales
every texel of a sprite by one constant factor, so it multiplies the artefact
and the signal by the same number and leaves their ratio where it was.

## The tint's own arithmetic, and the one condition it rests on

A premultiplied source `(C, A)` is the straight colour `C / A` at coverage `A`.
Tinting is `(C / A) * t_rgb` at coverage `A * t_a`, which back in premultiplied
form is

```text
out_rgb = C * t_rgb * t_a
out_a   = A * t_a
```

— so the factor is the tint *in premultiplied form*, and the shader is one
component-wise product. The alpha of a tint must therefore reach the colour
channels, or a half-transparent tint leaves a fragment brighter than its own
coverage and ADR-0023's invariant breaks at exactly the edges it protects.

`out_rgb <= out_a` holds whenever `C <= A` and `t_rgb <= 1`. The first is what
the pipeline already guarantees. **The second is a condition on the tint**, and
a channel above one breaks it. That is recorded as a named limit rather than
clamped away: clamping would silently change a caller's value, and rejecting
would make a colour fallible to hold. A test measures the limit instead of
asserting it is fine.

## Rejected: packing colour variants into the atlas

Its best argument in full strength: **no render path is touched at all.** No
vertex format changes, no shader changes, no blessed reference is even
theoretically at risk, and for a handful of rarity tiers — a common sword, a
rare sword, a legendary sword — an artist producing three images is a smaller
change than a component, a seam field and a vertex attribute.

What defeats it is that the atlas area grows multiplicatively with every colour,
while the cases the plan actually names — damage numbers, hit flashes, greyed-out
states — are not a handful. They are a **continuous** axis: a hit flash is a
value between white and the sprite's own colour, and no finite set of packed
variants is that. The variant approach answers "a few named colours" and the
requirement is "any colour", so it is not a cheaper version of the same
capability.

**Reopen it** if the tint turns out to be wanted only at a small fixed set of
values *and* the vertex cost is measured to matter — the tint doubled the vertex
from 16 to 32 bytes, which is 8 MB rather than 4 MB at
`MAX_SPRITES_PER_BATCH`. Neither half of that condition is met today: nothing has
measured vertex bandwidth, and M6b.5's animation post wants a tint that moves
over time, which is the continuous case again.

## Revision condition for this amendment

Reopen the space question if either sRGB format is ever exchanged for a linear
one — the answer is a consequence of the formats, so it expires with them, and
`the_half_tint_lands_where_a_linear_multiply_puts_it` is the test that would say
so. Reopen the clamping question if a measured case shows a tint above one
producing a visible defect rather than a documented one.

## Amendment, M6b.6 (2026-08-15): the frame convention belongs beside Decision 5

*Booked in M6b.5 and discharged here. M6b.5 built the clip declaration and wrote
its rule into the module header of `crates/narvo-assets/src/clip.rs`; its own
report named the debt — the rule is a statement about **file stems**, which is
Decision 5's subject, and a naming convention that lives only in a source comment
is one the next reader of this ADR cannot find. The material below is that
module header and M6b.5's report, both of which predate this text.*

**Nothing above is overtaken by it, and that is worth saying rather than
leaving to inference.** Decision 5 says the region name is the file stem
**verbatim**, and it still is. Recognition is additive: no region is renamed,
none is removed, the packed table is untouched, and a region belonging to no clip
is a region exactly as it was. `Atlas::region` answers for `hero_run_0` under
that name, before and after. The check was made before the code was written —
M6b.5 scanned every `*.rs` and `*.ron` in the workspace for a string literal
shaped like a numbered region name and found one hit, `"zoom_2"`, which is a
camera-margin test case rather than a region, and no tracked image's stem matches
the shape at all. So this amendment carries no mark on any sentence above,
because there is no sentence above it contradicts.

### The rule

A region name is a **frame** when everything after its **last** `_` is a
non-empty run of ASCII digits that fits in a `u32`. What precedes that separator
is the **clip's** name. Anything else belongs to no clip.

Three details of it are decisions rather than consequences, and each has a reason
Decision 5's stem rule makes necessary:

- **The digit test is explicit rather than `str::parse::<u32>`.** `parse` accepts
  a leading `+`, so `run_+1` would become frame 1 of `run`. No file stem produces
  that shape by numbering, so it is refused by an `is_ascii_digit` pass, which
  refuses non-ASCII digits on the same ground.
- **Frames are answered as names, never as a count to spell back.** Padding is
  the author's — `run_00.png` through `run_11.png` is ordinary — and no speller
  can know whether to emit `run_0` or `run_00`. A gap in the numbering makes it
  worse: frames 0, 1 and 3 are three frames, and a consumer handed the count `3`
  would ask for `run_2`, which no file carries. Handing over the names is what
  keeps every answer a region the atlas actually has.
- **The order is *(number, name)*, from a `BTreeSet`.** Numeric rather than
  lexical, so `run_9` precedes `run_10`; and two stems that differ only in
  padding — `run_0` and `run_00` — are two frames at one number, ordered by name,
  which is total and reproducible rather than arbitrary.

### Why it belongs here

Decision 5 is where this repository decides what a file on disk is called and
what that name means. The frame convention is a second sentence about the same
subject: it says which stems, among those Decision 5 already turned into region
names, additionally form a sequence. Read apart, the two invite the question of
whether a numbered stem is still a region name — read together, the answer is
plainly yes.

### Revision condition for this amendment

Reopen if the separator or the digit run ever has to vary per project, which
would make the convention configuration rather than a convention; or if a source
other than a directory of files starts producing regions, since the rule is
stated over stems and a source without stems would need its own sentence.


## Amendment, M7.9 (2026-08-20): what "no PNG is committed" was worth

M7.9 went looking for this repository's rule on binary files, to answer a
question about five untracked target images, and found the non-goal above
contradicted by `git ls-files`:

```
git ls-files -- 'crates/**/tests/golden/*.png' | wc -l
12
```

Nine under `crates/narvo-render2d/tests/golden/` and three under
`crates/narvo-app/tests/golden/` — the blessed references. They are not an
oversight and not a later drift: the first was committed on **2026-08-06**
(`65a6542`, "Bless the golden reference for the textured quad"), and this ADR
was written on **2026-08-11**, five days afterwards. The claim was never true.

**What the tree does instead is a real rule, and it is written down — just not
here.** `crates/narvo-app/tests/golden/README.md` and its twin in
`narvo-render2d` carry it: these files belong to the maintainer, no agent
writes them, and a reference is only ever updated in a separate,
human-authored commit after a human has looked at the image. That is a policy
about *who may write a binary and when*, which is a more useful thing to have
than a prohibition the repository does not keep.

The rest of the bullet is accurate and stays: test *inputs* are encoded by the
tests that read them, and `target/m48-demo/` is written rather than checked in.
The false half is the absolute — "here or anywhere".

**Nothing about the five target images is decided here.** They sit untracked in
`docs/zielbilder/`, and what to do with them belongs to a human; M7.9's report
sets out the options with their costs and deliberately takes none. What this
amendment settles is only the premise that decision was about to be argued
from: this repository already commits binaries, under a named rule, so the open
question is which rule concept art falls under — not whether a binary may be
committed at all.
