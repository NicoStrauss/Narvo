# Build and test time baseline

How long the Narvo workspace takes to build and to test, and the exact procedure
that produced those numbers.

Since M3.1 it carries a second kind of baseline that is not a time: how far each
rasteriser's render sits from its blessed reference image. It lives here because
it is measured the same way — one machine section, one procedure, one dated row
per environment — and because M3 reads it the way the build times are read, as
the figure a later measurement is compared against.

**The absolute values only became meaningful at M2.** At M0 the workspace was four
empty crates with one smoke test each and every command finished in well under a
second; those rows are kept because the *series* is the artifact, not any single
row. From M2 onwards the budgets from §8.1 of the project document apply:
incremental rebuild < 5 s, per-crate test run < 5 s.

**Two platforms since M2.1e.** The local reference machine has carried both
Windows and Linux (WSL 2) since ADR-0007, and the two have measurably different
times on the same hardware. Every row is therefore labelled with its platform.
Per §8.1 the budgets are stated for **Windows**, the slower of the two; the Linux
numbers are carried because the simulation work iterates there.

It is re-measured with the commands below at the end of every milestone. That
promise was not kept between M0 and M2.3 — see the gap in the history table,
which is left visible rather than back-filled with guesses.

## History

One row per milestone and platform. Wall-clock times, measured as described under
[Procedure](#procedure). `Commit` is the tree the numbers were taken on: the last
commit affecting compiled code at the time of measurement.

| Milestone | Platform | Date | Commit | Clean build | Incremental rebuild | Test run (warm) | Doctests |
| --- | --- | --- | --- | --- | --- | --- | --- |
| M0 - workspace scaffold | Windows | 2026-08-06 | `10111dd` | 0.35 s | 0.31 s | 0.17 s | 0.12 s |
| M0 close - first feature | Windows | 2026-08-06 | `e2e8571` | 0.37 s | 0.28 s | 0.20 s | 0.40 s |
| M1 close - window and first pixels | Windows | — | `9ff0543` | *not measured* | *not measured* | *not measured* | *not measured* |
| M2.3 close - recording and replay | Windows | 2026-08-07 | `33f8a5b` | 59.57 s | 1.55 s | 1.76 s | 2.89 s |
| M2.3 close - recording and replay | Linux (WSL 2) | 2026-08-07 | `33f8a5b` | 45.27 s | 1.19 s | 0.65 s | 2.38 s |

**The M1 gap is real and is not reconstructed.** No measurement was taken when M1
closed, and none is recorded in any report. Building that tree today would produce
a number about today's machine and today's toolchain, which is not what the column
means, so the row says what happened instead. Everything between M1 and M2.3 is
likewise absent; the M2.3 row is the first two-platform entry.

### Notes that belong to the numbers

- **The incremental probe changed between M2.2 and M2.2b.** It used to be a
  comment inserted into a source file and removed again; it is now an mtime touch
  on the same file. Cargo decides freshness by mtime either way, so the work done
  is the same, but the two are not the same measurement to the tenth of a second
  and should not be read as one series without this note.
- **The rebuild figure is a span, not a point.** 2.03 s (M2.2) → 1.34 s (M2.2b) →
  1.42 s (M2.3) → 1.55 s here, across that method change. Read it as *around
  1.4–2 s*; a swing inside that range is noise, not a regression. What was real
  was the earlier jump from 0.57 s (M2.1) to about 2 s: `narvo-app` gained a
  dependency on `narvo-ecs` at M2.2 and is rebuilt with it. ADR-0009 rests on
  these figures and is unaffected — the budget is 5 s.
- **Windows and Linux compile different numbers of units** from the same source:
  113 against 143 in the clean build. The platform-specific halves of the
  dependency tree differ — `winit` pulls the wayland and x11 crates on Linux and
  the `windows-sys` family on Windows — so the clean-build figures compare two
  workloads, not one workload on two systems. The incremental figures do compare
  like with like.
- **The full verification run is measured separately** because it is its own
  budget (§8.1, < 5 min from M3): warm it is 6.15 s on Windows and 5.13 s on
  Linux. With clippy's own fingerprints cold it was 32.61 s on Windows — clippy
  walks the dependency tree a second time under its own driver, which §8.2 of the
  project document records as known and unoptimised.

## Golden-image margin

How far each rasteriser's render sits from the blessed reference of
`the_textured_quad_matches_its_golden_reference`. §6/M1 recorded "0 of 4096
pixels differing" for Linux and, for Windows, only "passed", which left it
unknown whether the second platform had a comfortable margin or a one-count one.
This is that gap closed, and it is the reference point the further images of M3
are read against.

The thresholds are `Tolerance::default()` in
`crates/narvo-render2d/src/golden.rs`; the numbers come from the comparison the
golden test already performs, printed on success rather than only on failure.
Measured 2026-08-07 on commit `2244ff3`, one reference image, 64 x 64 = 4096
pixels.

| Rasteriser | Adapter string, as the run reported it | Differing pixels, floor 4 counts | Ratio, budget 0.1000 % | Worst channel deviation, limit 24 |
| --- | --- | --- | --- | --- |
| llvmpipe (ubuntu-latest) | `llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu] chosen by: high-performance` | 0 of 4096 | 0.0000 % | 0 |
| WARP (windows-latest) | `Microsoft Basic Render Driver [Dx12, Cpu] chosen by: high-performance` | 0 of 4096 | 0.0000 % | 0 |
| AMD RX 9070 XT (local, Windows) | `AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu] chosen by: high-performance` | 0 of 4096 | 0.0000 % | 0 |

All three are byte-identical to the reference, not merely inside tolerance: the
unused margin is the whole of every threshold. Two CPU rasterisers on different
APIs and one discrete GPU agree on this frame to the last count.

**These are moment values, not fixed points**, and the row above names what they
are a moment of. The CI figures were produced by run `31191569988` on runner
images `ubuntu-24.04` and `windows-2025-vs2026`, with the Linux side on
`Mesa 25.2.8-0ubuntu0.24.04.2 (LLVM 20.1.2)`, Vulkan `apiVersion 1.4.318`,
pinned to the lavapipe ICD by `ci.yml`. A Mesa bump in the runner image, a
different WARP with a Windows image bump, or a GPU driver update locally can each
move a number here without anything in this repository changing. That is why the
adapter string is a column and not a footnote.

**What this does not say.** It is one image, of one textured quad, at one size,
with axis-aligned geometry and nearest-neighbour-scale colour blocks. It says
nothing yet about filtered sampling at non-integer positions, about a camera
transform that puts vertices between pixel centres, or about glyph coverage — the
three things M3 adds. `every_threshold_decides_on_its_own_at_its_boundary` in
`golden.rs` is what keeps these numbers decomposable when they stop being zero:
it drives each threshold one count either side of where it is set, so a future
non-zero figure can be read as *which* threshold and *how far*, not as an
undifferentiated "still passes".

## Off-grid margin: what a camera costs in rasteriser agreement

The section above ends by naming what it does not cover, and "a camera transform
that puts vertices between pixel centres" is the first of those three to arrive.
M3.12 measures it before any reference image depends on it, because a reference
CI cannot reproduce is worse than no reference.

One scene — three 32 x 32 sprites overlapping in a chain, A with B and B with C
while A and C are disjoint, one atlas region each — rendered through six cameras
on a 128 x 128 target. An edge at world `w` lands at image coordinate
`X = 64 + (w - cam) * zoom` across and `Y = 64 - (w - cam) * zoom` down — the two
differ in sign because image rows run against world y (ADR-0004) — and every
sprite coordinate in the scene is a whole number, so the camera alone decides
whether an edge falls on a pixel boundary. `the_cases_are_on_and_off_the_grid_where_the_table_says` in
`crates/narvo-render2d/tests/camera_margin.rs` asserts that split rather than
assuming it.

Measured 2026-08-08 on the M3.12 working tree: `0446641`, plus M3.12 itself,
plus the maintainer's `ProjektPlan.md` v0.16 — uncommitted at the time, since
committed as `2705aff`, and read by no test. That tree is `d6d31ea`. 128 x 128 = 16 384 pixels per case. The comparison is
`Golden::verify` — the same code and the same three numbers the section above
reports — with each rasteriser compared against the AMD run.

| Case | Camera | Zoom | Vertical edges land at | On the grid? |
| --- | --- | --- | --- | --- |
| `identity` | (0, 0) | 1 | 28, 60, 48, 80, 68, 100 | yes — today's regime |
| `zoom_2` | (0, 0) | 2 | -8, 56, 32, 96, 72, 136 | yes |
| `offset_half_x` | (0.5, 0) | 1 | 27.5, 59.5, 47.5, 79.5, 67.5, 99.5 | no — every **vertical** edge on a pixel centre; the horizontal ones stay on boundaries, since an x-only offset does not move them |
| `offset_half_xy` | (0.5, 0.5) | 1 | as above across; 28.5, 48.5, 60.5, 68.5, 80.5, 100.5 down | no — the sharpest case, and the only one on centres in **both** axes |
| `zoom_1_5_quarter` | (0.25, 0.25) | 1.5 | 9.625, 57.625, 39.625, 87.625, 69.625, 117.625 | no |
| `offset_and_zoom` | (0.3, -0.7) | 1.5 | 9.55, 57.55, 39.55, 87.55, 69.55, 117.55 | no |

| Rasteriser | Adapter string, as the run reported it | Differing pixels, floor 4 counts | Ratio, budget 0.1000 % | Worst channel deviation, limit 24 |
| --- | --- | --- | --- | --- |
| AMD RX 9070 XT (local, Windows) | `AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu] chosen by: high-performance` | reference run | — | — |
| WARP (local, Windows) | `Microsoft Basic Render Driver [Dx12, Cpu] chosen by: forced software fallback` | 0 of 16 384, **every case** | 0.0000 % | 0 |
| llvmpipe (local, WSL 2) | `llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu] chosen by: high-performance` | 0 of 16 384, **every case** | 0.0000 % | 0 |

**All three rasterisers are byte-identical in every case, including the one that
puts every edge exactly on a pixel centre, the one that does so for every
vertical edge, and the two with a non-integer zoom.**
That is the whole result. The margin the M1 quad had is the margin a camera has.

WARP was reached by temporarily putting the adapter ladder's software-fallback
rung first in `gpu.rs`; the change was reverted and the file checked byte-for-byte
against its snapshot afterwards.

**The instrument was shown to move.** Fed two of the six PNGs under each other's
names, the same comparison reported `8016 of 16384 pixels (48.9258%) differ …
the worst pixel is off by 255 counts` for both and left the other four at zero.
A row of zeroes here is a measurement, not a comparison that never ran.

### What the measurement also showed, and did not set out to

A **sub-pixel camera offset does not move the picture by a sub-pixel amount.**
`offset_half_x` is `identity` translated by exactly one whole column — 16 256 of
16 256 comparable pixels equal under that shift — and `offset_half_xy` is
byte-identical to `offset_half_x`, so the half-pixel offset along y moved
nothing at all. The pipeline takes one sample per pixel
(`multisample: MultisampleState::default()`), so coverage is never partial: an
edge lands on one side of a pixel centre or the other, and the tie is broken by
the fill rule. The two axes tie in opposite directions because image y runs
against world y (ADR-0004).

The consequence is a property of the camera, not of any rasteriser: **camera
motion is quantised to whole pixels, and a smooth pan will step rather than
glide.** Nothing in M3.12 addresses that; it is reported in that task's report as
a named surface.

## Soft-edge margin: what a *mixed* pixel costs in rasteriser agreement

The two sections above measure a renderer that cannot produce a mixed pixel.
M3.13 established why: one sample per pixel and `Nearest` filtering, so every
pixel is exactly one texel and the three rasterisers only ever had to agree on
which side of a pixel centre an edge falls. This section measures the regime
that starts where that one stops.

**It does not measure the shipped renderer, and no row here should be read as if
it did.** `crates/narvo-render2d/tests/soft_edge_margin.rs` builds a second,
test-only pipeline — same target format, same clear, a shader of the same shape —
because the production one has no configuration in which a pixel carries a blend.
Nothing in `src/` changed for this.

Two regimes, because they mix in different places and neither produces the
other's pixels:

- **Coverage (`msaa`)** — four samples per pixel, resolved. Mixes only at
  geometry edges. Scene: one white triangle on black, no edge axis-aligned and
  none at 45°, slopes −0.170 / −2.25 / +1.552, one vertex on a pixel centre.
- **Filtering (`linear`)** — `FilterMode::Linear`, one sample per pixel. Mixes
  only inside a primitive. Scene: an 8 × 8 black-and-white checkerboard
  magnified by 12.6625 pixels per texel, a deliberately non-integer factor.

**The scenes produce the case, checked before the rasterisers were compared.**
A scene with no mixed pixel measures zero whatever the rasterisers do, and the
test refuses to report one:

| Regime | Pixels carrying a mixed value | Share of 16 384 |
| --- | --- | --- |
| `msaa` | 225 | 1.3733 % |
| `linear` | 10 134 | 61.8530 % |

The count is per run, not a property of the scene alone, and all three runs
reported these same two numbers — which is itself a small piece of the result.

Measured 2026-08-08 on the M3.14 working tree — `f03df1f` plus the one new test
file, plus this section; nothing under `src/` differs, and the maintainer's
`crates/narvo-render2d/tests/golden/README.md` was dirty in the tree throughout
and is read by no test. 128 × 128 = 16 384 pixels per case. Each pair compared
directly, not against a reference; the three numbers are `Golden::verify`'s, the
histogram is the test's own.

### Coverage regime (`msaa`)

| Pair | Differing pixels, floor 4 counts | Ratio, budget 0.1000 % | Worst channel deviation, limit 24 |
| --- | --- | --- | --- |
| AMD ↔ WARP | 0 of 16 384 | 0.0000 % | **0** |
| AMD ↔ llvmpipe | 0 of 16 384 | 0.0000 % | 1 |
| WARP ↔ llvmpipe | 0 of 16 384 | 0.0000 % | 1 |

Distribution: AMD and WARP are byte-identical. Against llvmpipe, 93 pixels of
16 384 differ, every one of them by exactly 1 count; 16 291 are identical.

### Filtering regime (`linear`)

| Pair | Differing pixels, floor 4 counts | Ratio, budget 0.1000 % | Worst channel deviation, limit 24 |
| --- | --- | --- | --- |
| AMD ↔ WARP | 29 of 16 384 | **0.1770 %** | 8 |
| AMD ↔ llvmpipe | 0 of 16 384 | 0.0000 % | 3 |
| WARP ↔ llvmpipe | 14 of 16 384 | 0.0854 % | 6 |

Distribution, counts → pixels:

| Pair | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 8 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| AMD ↔ WARP | 13 090 | 3 014 | 197 | 47 | 7 | 2 | 26 | 1 |
| AMD ↔ llvmpipe | 14 495 | 1 853 | 27 | 9 | — | — | — | — |
| WARP ↔ llvmpipe | 13 263 | 2 939 | 119 | 38 | 11 | 6 | 8 | — |

**One threshold breaks, and it is the pixel budget.** AMD against WARP puts
0.1770 % of the frame past the noise floor where the budget allows 0.1000 % —
1.77 times over. The other two thresholds hold with room: the worst single pixel
is 8 counts against a limit of 24, and the noise floor of 4 is what the 29
pixels exceeded rather than a threshold that failed. WARP against llvmpipe sits
at 0.0854 %, inside the budget but not by much.

The shape matters as much as the maximum: the difference is not a handful of bad
pixels but a broad haze of one-count roundings — about 3 000 of them in the worst
pair — with a small tail reaching 6 and 8. A budget written for a few displaced
pixels is not the instrument for that.

**These are moment values**, for the same reason the section above says so: a
Mesa bump, a Windows image bump or a driver update moves them without a line of
this repository changing. The adapter strings:

| Rasteriser | Adapter string, as the run reported it |
| --- | --- |
| AMD RX 9070 XT (local, Windows) | `AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu] chosen by: high-performance` |
| WARP (local, Windows) | `Microsoft Basic Render Driver [Dx12, Cpu] chosen by: forced software fallback` |
| llvmpipe (local, WSL 2) | `llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu] chosen by: high-performance` |

WARP was reached by temporarily putting the software-fallback rung first in the
test file's own adapter ladder; the file was checked byte-for-byte against its
snapshot afterwards. **No threshold was changed** — M3.14 measures and records.

What to do about the breach belongs to **D13** (`ProjektPlan.md` §11, the sampler
filter), and to whatever decision is eventually opened about antialiasing; the
thing that would have to move is `Tolerance::default()` in
`crates/narvo-render2d/src/golden.rs`. M3.14's task description called that
second decision "D14", but **§11 has no D14** — the table runs D2, D3, D4, D5,
D7, D8, D10, D13 — so this section does not cite one.

## MSAA in the production path: what four samples cost, and what they buy

The three sections above measure a renderer with one sample per pixel. M3.15
gave it four, in `src/`, for every render pass the crate records — `SAMPLE_COUNT`
in `crates/narvo-render2d/src/lib.rs`, with no way to switch it off. These rows
are therefore about the shipped renderer, unlike the soft-edge section, which
built its own pipeline because the shipped one had no such configuration.

**Four and not another number.** `TextureFormat::guaranteed_format_features`
grants `MULTISAMPLE_X4 | MULTISAMPLE_RESOLVE` to both formats this crate renders
into, `Rgba8UnormSrgb` and `Bgra8UnormSrgb`
(`wgpu-types-30.0.0/src/texture/format.rs:976` and `:981`). `MULTISAMPLE_X2`,
`_X8` and `_X16` are separate flags in the same bitset and are guaranteed for
neither. Any other count would mean a capability query, a fallback path, and
golden images that depend on which machine rendered them.

### Memory

| Target | Single-sample target | Multisample attachment, 4 samples |
| --- | --- | --- |
| 128 × 128 | 65 536 B | 262 144 B |
| 1920 × 1080 | 8 294 400 B | 33 177 600 B (31.6 MiB) |

Four times the colour attachment, exactly, and it is an addition rather than a
replacement: the single-sample texture stays, because it is what the resolve
writes into and what the read-back copies from.
`OffscreenTarget::multisample_bytes` reports it, and
`the_multisample_attachment_costs_what_the_sample_count_implies` in
`golden_image.rs` asserts the arithmetic. That assertion earned its place
immediately: the method first used `TextureFormat::target_pixel_byte_cost`,
which is WebGPU's *attachment budget* figure and charges an 8-bit-per-channel
format 8 bytes rather than 4 (`format.rs:1312` against `:1215`), and it reported
63.3 MiB — double the truth — until the test caught it.

The window path pays the same multiple, sized to the surface and rebuilt on
every resize.

### Time

**No cost measurable at any sprite count**, which is not what the first attempt
at this table said, and the difference between the two is worth more than the
result.

The "before" column has to be `c43a2e1`, the commit this work sits on. The
table under "Offscreen round trip with N sprites" below was measured on
`c949c2f`, eighteen commits earlier — before each sprite got its own texture
region and before the camera projection entered `sprite.rs` and `offscreen.rs`,
both of which are on the path this test times, and one of which rewrote the
test's own workload. Against that column the 50 000 row read `2.84 → 3.89 ms`
and was written up here as "+37 %, and that shape is what four samples should
cost". It was measured correctly and attributed to the wrong thing.

Re-measured at `c43a2e1` in a throwaway worktree, same machine, same `release`
profile, same best-of-three, same adapter. Three runs of each column:

| Sprites | `c43a2e1`, one sample | Working tree, four samples |
| --- | --- | --- |
| 1 | 1.28 / 1.25 / 1.18 ms | 1.27 / 1.19 / 1.30 ms |
| 1 000 | 1.28 / 1.17 / 1.15 ms | 1.24 / 1.17 / 1.23 ms |
| 10 000 | 1.59 / 1.43 / 1.59 ms | 1.58 / 1.51 / 1.60 ms |
| 50 000 | 3.77 / 3.82 / 3.65 ms | 3.89 / 3.76 / 3.75 ms |

Every row's two ranges overlap, including the last: 3.65–3.82 against
3.75–3.89. **Four samples cost nothing this test can see**, and the 2.84 of the
older table is the renderer of eighteen commits ago, not the renderer without
MSAA.

Read the rows as ranges. The first three are dominated by the fixed ~1.2 ms of
read-back, which MSAA does not touch: the resolve happens before the copy and
the copy is the same 8.3 MB either way. The 50 000 row is where fill would show
if it were going to, and at 16 × 16 sprites over 1920 × 1080 it does not — this
workload is not fill-bound enough to price four samples. **A denser scene could
still cost, and this table does not say otherwise.**

### The blessed images: one of five moved

> **The `camera_regions_128x128` row has changed meaning, 2026-08-09 (M3.26).
> Nothing here was re-measured and no number in it was edited.** Under D17's
> `center` the same instrument now prints **48 of 16 384 (0.2930 %), worst 30**
> where it printed 87 / 0.5310 % / 137 below. Nothing went red, because this
> test prints its margins and asserts only the one-sample control — which is
> why the change is recorded here rather than left to be noticed.
>
> **The cause is this file's fixture, not the renderer.** `msaa_blessed_margin.rs`
> renders **unpadded** copies of the blessed scenes (`AtlasLayout::UNPADDED`),
> the same private-copy situation the `Linear` section already carries an M3.21
> note about. On unpadded content `center` extrapolates a partly covered pixel's
> uv into the neighbouring region, so the antialiased pixel comes to resemble its
> neighbour and the distance to the one-sample render *shrinks*. On the padded
> content the blessed scenes actually use, that extrapolation reads a duplicate
> of the region's own edge texel and no such thing happens: the blessed
> `camera_regions_128x128` is byte-identical under both qualifiers — 0 of 16 384,
> worst 0, measured in M3.24a and again in M3.26.
>
> So the figures below remain the honest record of what four samples bought under
> `centroid` on unpadded fixtures. What they no longer are is a measurement of
> the shipped scenes under the shipped qualifier. **The number to trust for that
> is the golden test's**, and it says the image did not move.

> **The sentence below no longer ties this table to the blessed PNGs at all,
> 2026-08-09 (M3.29). Nothing here was re-measured and no number in it was
> edited.** M3.28's note said the count was the thing to keep true and that the
> fourth conversion should end the annotating and force a rewrite. This is that
> rewrite, and the count is now **one of five**.
>
> This file's replicas sample `Nearest` (`msaa_blessed_margin.rs:418-420`), and
> after D13's round **four of the five blessed references are `Linear` renders** —
> `sprite_atlas_regions_128x128` (M3.24/M3.26), `placed_sprite_quadrants_128x128`
> (M3.27), `layer_order_regions_128x128` (M3.28) and `camera_regions_128x128`
> once M3.29's blessing is given. The only one the sentence below still describes
> is `textured_quad_quadrants_64x64`, which stays `Nearest` deliberately as the
> byte-exact three-rasteriser anchor.
>
> **So read the sentence below as history, not as a property.** It was true when
> it was written, of a repository in which every blessed image was `Nearest`;
> what makes it false is the conversion, not a defect. For
> `camera_regions_128x128` it had in fact already stopped being true at **M3.15**,
> one blessing earlier and for an unrelated reason — that row's 87 / 0.5310 % /
> 137 *is* the gap between the one-sample render and the blessed PNG, which is
> what the M3.26 note above this one is about.
>
> **For `placed_sprite_quadrants_128x128` the gap follows from this table
> without a run**, and is stated rather than left vague: its row below is
> `0 of 16 384 … 0` — worst channel 0, so the one-sample render *is* that PNG
> — and M3.27's golden test measured the `Linear` render against the same PNG at
> 360 of 16 384 (2.1973 %), worst 173. Those two compose exactly, so the
> one-sample replica will sit 360 / 173 from the new reference. The step that
> carries it is a pre-D17 measurement nobody has re-taken, which is the one
> reason it is written as following from the table rather than as observed. For
> `sprite_atlas_regions_128x128` no such composition is available: its replica is
> an unpadded private copy, so its row is not a byte-identity to compose through.
>
> **For `layer_order_regions_128x128` the gap is deliberately not stated, and the
> reason is worth more than the number would be.** Its replica here is unpadded
> too (`msaa_blessed_margin.rs:159`), which is the disqualifier the sentence above
> applies to `sprite_atlas_regions_128x128`. What is different is that for *this*
> scene the padding provably does not matter under `Nearest` — the border reads
> nothing, which is why its blessed reference did not move when M3.22 added one
> (`sprite_batch.rs`, `atlas()`'s doc). So the composition is probably available
> where the atlas scene's is not. **Probably is not a measurement**, it would take
> a run, and this task collected no number.
>
> **What the rows still measure is unchanged**, because the comparison behind
> them never reads a reference: both sides are live renders of this file's own
> scene table, one sample against four (the only thing that touches the
> reference directory compares file *names*). So each row remains an honest
> statement about MSAA on that geometry. What it has stopped being is a
> statement about the *blessed image* of the same name. The `Linear` counterpart
> — what four samples cost these scenes under the sampler they now ship — has
> never been measured by anything, and is named here rather than collected.

Measured before anything was switched on, by
`crates/narvo-render2d/tests/msaa_blessed_margin.rs`, which replicates each
blessed scene's geometry and renders it at one sample and at four. **The
one-sample render is byte-identical to the blessed PNG in all five cases**,
which is what makes the four-sample number a statement about MSAA rather than
about the replication.

| Reference | Differing pixels, floor 4 | Ratio, budget 0.1000 % | Worst channel deviation, limit 24 |
| --- | --- | --- | --- |
| `textured_quad_quadrants_64x64` | 0 of 4 096 | 0.0000 % | 0 |
| `placed_sprite_quadrants_128x128` | 0 of 16 384 | 0.0000 % | 0 |
| `sprite_atlas_regions_128x128` | 0 of 16 384 | 0.0000 % | 0 |
| `layer_order_regions_128x128` | 0 of 16 384 | 0.0000 % | 0 |
| `camera_regions_128x128` | 87 of 16 384 | 0.5310 % | **137** |

Four of the five are not merely inside the budget, they are byte-identical: zero
pixels differ at all, not zero pixels past the floor. Their edges lie on whole
pixel boundaries, so every sample of a boundary pixel falls on the same side and
the resolve averages four identical values. **AMD, WARP and llvmpipe returned
these same numbers and the same distribution**, to the pixel — adapter strings
`AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu]`, `Microsoft Basic Render Driver
[Dx12, Cpu]` and `llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu]`. The test
prints these numbers and asserts only the one-sample control, so the two
non-local rows are a record of runs rather than something the suite re-checks.

The fifth is the one D14 already named as a known cost (`ProjektPlan.md` §11:
an edge at 52.75, will change, needs a fresh blessing), and whether any of the
other four moved it recorded as unmeasured. This is that measurement.
Distribution, since the maximum hides the shape — all 87 pixels sit in two
columns, and every one of them is a coverage fraction rather than noise:

| Deviation | Pixels | What it is |
| --- | --- | --- |
| 16 | 24 | column 100, B's dark green at three quarters coverage |
| 30 | 24 | column 100, B's green at three quarters |
| 66 | 15 | column 52, a quarter of B's dark green over A's dark red |
| 137 | 24 | column 52, a quarter of B's green over A's red |

Column 52 and column 100 are B's two vertical edges, at `X = 52.75` and
`X = 100.75`. Column 52 is 39 rows rather than 48 because sprite C covers the
lower nine.

**137 counts against a limit of 24 is not a tolerance problem.** The reference
says this pixel is pure red and the renderer now says it is three quarters red
and one quarter green, which is a different picture and not a noisier one. It
needs a new blessing, not a wider threshold, and no threshold was changed.

### What a pan actually costs in step size

> **Re-anchored, 2026-08-09 (M3.25). Nothing in this section was re-measured and
> no number in it changed.** The silhouette's four quarter-pixel steps are still
> asserted, but on a **single-colour** copy of this sprite rather than on the
> striped atlas the table below was measured with. The reason is the fourth
> bullet: on striped content the step list depends on the uv interpolation
> qualifier — `[2, 6, 10, 14]` under `centroid` and `[2, 6, 8, 10, 14]` under
> `center` — because column 87 is a silhouette column that *also* sits on the
> region's outer texel boundary, and at `k = 8` the camera sits at exactly half a
> pixel and every texel boundary lands exactly on a pixel centre. Under `center`
> that column's sample steps out of the region with the interior; under
> `centroid` it does not. The extra entry is therefore a change of colour under
> unchanged coverage, not a fifth coverage step — down column 87 the four
> coverage steps are −60, −74, −102, −274 and the `k = 8` drop is −188, larger
> than any of them, while both `Nearest` columns sum to the same −510.
>
> On one colour there is no texel boundary to cross and no neighbouring colour to
> reach, so nothing but coverage can change a pixel. `camera_pan_steps.rs` now
> carries both: the assertion on the uniform fixture, the numbers below printed
> and labelled on the striped one. **New instrument, new number** (AMD, `centroid`
> as committed): the uniform sweep moves the silhouette at `[2, 6, 10, 14]`,
> changes no interior column at any step, and takes column 39 through
> `0 → 137 → 188 → 225 → 255` — the five coverage levels of a single colour.

`crates/narvo-render2d/tests/camera_pan_steps.rs` sweeps the camera across
exactly one pixel in sixteenths and asks at which sixteenths the image changes.
`ProjektPlan.md` §12 asked M3.15 whether MSAA makes the movement smooth "or
whether the quantisation from M3.13 survives somewhere else". Both, in different
places:

| | silhouette moves at | interior flips at | diagonal pixel blends |
| --- | --- | --- | --- |
| AMD Radeon RX 9070 XT, Vulkan | 2, 6, 10, 14 | `k = 9` | 6 of 17 |
| Microsoft Basic Render Driver, Dx12 | 2, 6, 10, 14 | `k = 8` | 4 of 17 |
| llvmpipe (LLVM 20.1.2), Vulkan | 2, 6, 10, 14 | `k = 8` | 4 of 17 |

- **The silhouette's step size is a quarter of a pixel, not a pixel**, and all
  three rasterisers put the four steps in the same four places. That is what
  D14 bought, and it is portable.
- **The interior's is not touched.** `Nearest` still samples once per pixel, so
  a texel boundary still moves the interior only when it crosses a pixel centre
  — once per pixel of travel, the pattern shifting a whole column at a time,
  exactly as before MSAA. The 6 pixels per texel are the spacing of the
  interior's grid, not the distance between its steps: the boundaries sit 6
  pixels apart and all at the same sub-pixel phase, so they cross together
  rather than in turn.
- **`k = 8` is the one step the three disagree about.** There the camera sits at
  exactly `0.5` and the pixel centre lands exactly on a texel boundary, so
  `floor` of an exactly-integral interpolated value decides the texel, and the
  last bit decides `floor`. This is **not MSAA's**: with the shader temporarily
  switched back to `center` interpolation the same disagreement appeared at the
  same step on both AMD and llvmpipe.

**A hazard that follows, stated once:** a scene that puts a pixel centre exactly
on a texel boundary cannot be blessed, because two rasterisers render it
differently by a whole texel — 255 counts, not 1. None of the five blessed
images does, and it takes both halves of the reason. Every blessed sprite spans
a whole number of pixels per texel — 8, 6, 4, 3 and 5 across the five — and every
edge is on a whole pixel except sprite B of the camera scene, whose edges are at
52.75 and 100.75. A whole-pixel edge makes `px + 0.5 - X0` a half-integer, which
is never a whole multiple of a whole number of pixels; B's quarter-pixel edge
makes it `px - 52.25`, which is not a multiple of 6 either. **The
pixels-per-texel half is not decoration**: at 2.5 pixels per texel — a 20-pixel
sprite over 8 texels — an offset of 2.5 pixels is exactly one texel, and a
whole-pixel edge puts a pixel centre on a boundary after all. A sprite whose
pixel width is not a whole multiple of its texel count has to be checked by
hand.

### `centroid`, and what it cost to avoid a worse thing

> **Superseded as a description of the renderer, 2026-08-09 (M3.26). Nothing in
> this section was re-measured and no number in it changed.** The shader
> qualifies its uv varying `@interpolate(perspective, center)` since D17 was
> carried out; every measurement below describes the `centroid` regime that
> preceded it, and each is still the measurement that decided the change.
>
> Three of them need a pointer rather than a correction:
>
> - The `[137, 0, 0]` bleed was measured on an **unpadded** atlas, in M3.15,
>   before `camera_scene.rs` got its border in M3.21. Re-measured on padded
>   content in M3.24a, the same class of pixel reads `[0, 225, 0]` — its own
>   green against black, zero red — because the texel beyond a padded region is
>   a copy of the region's own edge texel. That is why the protection `centroid`
>   provided could be given up: it did not become unnecessary, it became the
>   content pipeline's job. D17 records that this binds D10's glyph atlases to
>   padding every cell.
> - "`centroid` has a cost of its own, and it is smaller" was the trade as it
>   stood before padding. D17 reversed it: on **padded** content the bleed does
>   not occur and the diagonal seam does, and under `Linear` that seam carried
>   `sprite_atlas_regions_128x128`'s worst deviation. The seam is gone under
>   `center` — 320 of 16384 against 323, worst 64 against 71.
> - "the repository holds no test in that configuration" stopped being true in
>   M3.25: `camera_pan_steps.rs` prints the striped step list every run, so
>   `[2, 6, 8, 10, 14]` is now the output of a committed test rather than a
>   number recorded here. What it is **not** is an assurance — that moved to a
>   single-colour fixture, for the reason the pan-step section above gives.

The shader qualifies its uv varying `@interpolate(perspective, centroid)`. That
is not decoration. Under MSAA the fragment shader still runs once per pixel, and
at the default `center` that one invocation reads the varying at the pixel
centre — which for a partly covered pixel lies *outside* the sprite. The uv is
then extrapolated past the sprite's own atlas region and `Nearest` fetches a
neighbouring sprite's texel.

Measured rather than reasoned: sprite B of the camera scene drawn alone against
black, at four samples with `center`, put `[137, 0, 0]` into pixel 52 — a
quarter of *red*, the texel column left of its region — where its own region is
green. With `centroid` the same pixel reads green. This is the atlas bleeding
D13 records `Nearest` as not having, arriving through MSAA rather than through
`Linear`.

`centroid` has a cost of its own, and it is smaller. A quad is two triangles
split along its diagonal, and a pixel *on* that diagonal is partly covered by
each, so the two get two sample points and can land in two texels; the resolve
then averages two texel colours. It shows only where a quad's internal diagonal
crosses a texel boundary — seven pixels of a 48 × 48 sprite in the pan sweep —
against a coloured fringe on every off-grid silhouette pixel of every sprite.
Switching to `center` also gave the silhouette *five* steps per pixel instead of
four — `2, 6, 8, 10, 14` against `2, 6, 10, 14` — the extra one at `k = 8` being
the bleed changing texel. Measured by re-running the sweep with the qualifier
removed; the repository holds no test in that configuration, so the step list is
recorded here rather than cited.

Two ways out would have neither artefact and neither was taken: a per-vertex
region clamp in the fragment shader, which widens the vertex format, and
per-sample shading, which is four times the fragment cost and the "no SSAA" this
task was given. Both are reported, neither is decided.

## Camera motion: how far the picture moves when the camera does

> **The centroid and mass columns stopped being asserted, 2026-08-09 (M3.25).
> Nothing in this section was re-measured and no number in it changed.** They are
> printed by `camera_motion.rs` and labelled there. The reason is the fixture:
> this scene's atlas is **deliberately unpadded**, so under the default `center`
> interpolation a partly covered pixel's uv extrapolates past its own region and
> `Nearest` fetches the *neighbouring* sprite's texel — the mechanism
> `src/shaders/quad.wgsl` records, measured on this very scene. An edge pixel
> that borrows a neighbour's colour changes the frame's mass rather than moving
> it, and the centroid is only a translation measure while the mass holds still.
> M3.24c measured `offset_half_x` at a centroid dx of −0.31 with a mass gain of
> ~9.95 under `center`, against −0.4999 and +0.19 in the table below.
>
> **The edge column is unchanged and still asserted**: at a half-pixel camera the
> pixel centre lands exactly on the sprite's edge, so the fetch stays inside the
> region whichever point the varying is read at, and M3.24c measured 47.4971
> under both qualifiers.
>
> The assurance moved rather than went away — see "The qualifier-independent
> carriers" at the end of this section.

The MSAA section above prices four samples and measures which blessed images
they move. This one answers the question D14 was decided on and M3.15 did not
reach: **whether the movement became smooth.** Measured by
`crates/narvo-render2d/tests/camera_motion.rs` on the scene
`camera_margin.rs` uses — three overlapping 32 × 32 sprites, one atlas region
each, 4 pixels per texel — at five of the cameras M3.12 and M3.13 used, plus
`(−0.5, −0.5)`, which is in neither and is here to check the two axes together.

**A count of differing pixels cannot answer this.** M3.13's sharpest number on
this scene was 16 256 of 16 256 comparable pixels *matching* once one image was
shifted a whole column — a statement about whether a whole-pixel shift fits, and
one that cannot distinguish a picture that slid half a pixel from one that
jumped a whole one. That is why M3.13 could locate the quantisation but not
price it. The
measurement has to yield a displacement, in pixels, on a scale finer than a
pixel. Two measures, because they fail differently:

- **The sub-pixel edge position.** Under MSAA the column a silhouette edge falls
  in is partly covered, and the covered fraction *is* the sub-pixel position: a
  column covered `f` of the way puts its edge at `column + (1 - f)`. Read in
  **linear** light — the resolve averages before the format encodes, so a
  half-covered pixel stores `sRGB(0.5) = 188`, not `128`.
- **The intensity centroid of the frame.** A rigid translation of the content
  moves the centroid by exactly the translation, which is what makes it a
  displacement rather than a difference. Reported with the frame's total
  intensity beside it, because it is only a translation measure while that total
  holds still.

Measured 2026-08-08 on the M3.16 working tree — `3ea1377` plus one new test file
and this section, nothing under `src/`. Adapters
`AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu]` and
`llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu]`, both at
`chosen by: high-performance`.

**The two agree on every structural number** — five edge positions, four steps,
one interior step — and on the edge displacements to within one count of the
stored half-coverage byte: AMD's half-covered column reads back 0.5029 and
llvmpipe's 0.4969, which are 188 and 187 of 255 and bracket 0.5 to within a
rounding step (+0.0029 above, +0.0031 below). The same one-count difference the
soft-edge section already records between these two.

**That agreement is on the edge measure.** The centroid and mass columns below
are AMD's alone and do not reproduce: on llvmpipe the x cameras' mass change is
−0.15 where AMD gives +0.19, and `offset_half_xy`'s centroid dy is +0.4018
against +0.3939.

### The cameras

Displacements are from the identity camera. Sprite B's left edge is at exactly
`X = 48` there, asserted before any displacement is derived. AMD's column below;
llvmpipe's edge displacements are −0.4969 / −0.4969 / +0.5031 / 0.0000 / +0.5031
in the same order.

| Camera | Edge x | Edge moved | Centroid dx | Centroid dy | Mass change |
| --- | --- | --- | --- | --- | --- |
| `identity` | 48.0000 | — | — | — | 1805.39 total |
| `offset_half_x` (0.5, 0) | 47.4971 | **−0.5029** | −0.4999 | −0.0011 | +0.19 |
| `offset_half_xy` (0.5, 0.5) | 47.4971 | **−0.5029** | −0.4999 | +0.3939 | −33.71 |
| `minus_half_x` (−0.5, 0) | 48.4971 | **+0.4971** | +0.5001 | −0.0011 | +0.19 |
| `minus_half_y` (0, −0.5) | 48.0000 | 0.0000 | −0.0007 | −0.6090 | −35.10 |
| `minus_half_xy` (−0.5, −0.5) | 48.4971 | **+0.4971** | +0.5001 | −0.6061 | −33.71 |

**Half a pixel of camera now buys half a pixel of silhouette, in both
directions.** Pre-MSAA, M3.13 measured this same scene and found `offset_half_x`
to be `identity` shifted by **one whole column** — which the "Off-grid margin"
section above also records — while `(−0.5, 0)` came out **byte-identical to
identity**: one pixel of movement in one direction and none in the other. That
sign asymmetry is gone.

The second half of that citation rests on `target/reports/M3.13.md`, which is
git-ignored and does not survive `cargo clean`. It is named as history rather
than cited as a durable source; the first half has a committed home in the
section above.

The residue of 0.0029 is this measurement's, not the renderer's: coverage is
recovered from a stored byte, `sRGB(0.5)` rounds to 188 of 255, and 188 decodes
to **0.5029** — not to 0.4971, which is `1 - 0.5029` and the *fractional part of
the edge position* rather than the coverage. The edge is then reported at
`47 + (1 - 0.5029) = 47.4971`.

**The y column is where it stops being tidy.** The three cameras that move in y
lose about 1.9 % of the frame's intensity and miss the expected centroid by about
0.11 pixels — **all three in the same direction**, which is the informative part.
`offset_half_xy` asked for +0.5 and got +0.3939, so it undershot; the two
negative-y cameras asked for −0.5 and got −0.609 and −0.606, so they overshot.
Measured minus expected is −0.106, −0.109, −0.106 on AMD and −0.098, −0.103,
−0.098 on llvmpipe: the centroid sits about a tenth of a pixel above where a
rigid translation would put it, whichever way the camera went. A shift, not a
scale error. Nothing left the frame — that is asserted separately — so the
mass did not move, it *changed value*: a half-pixel offset in y puts pixel
centres exactly on texel row boundaries, `Nearest` picks a side, and this
atlas's regions differ between their upper and lower halves (255 against 128)
while being uniform across x. The x cameras leave the mass essentially
untouched — +0.19 on AMD, −0.15 on llvmpipe, a hundredth of a percent either way
and of no fixed sign, so nothing here explains its direction and nothing rests
on it.

### The step size, which is the answer

> **Asserted on uniform content since 2026-08-09 (M3.25), and the numbers did not
> move.** `the_edge_advances_in_quarter_pixel_steps_and_there_are_four_of_them`
> now sweeps a single-colour copy of this scene and prints the striped sweep
> beside it. Under `centroid` the two produce the **same nine positions** — the
> coverage is identical and only the sampled colour ever differed — so the table
> below is the uniform fixture's as well; it was measured again as such and came
> back position for position. Under `center` only the uniform one keeps them: on
> the striped atlas a quarter-covered column's uv extrapolates into sprite A's
> red region, the coverage read looks at green and sees nothing, and the scan
> reports the next column instead.

One pixel of camera travel in eighths, at zoom 1:

| Camera x | 0.000 | 0.125 | 0.250 | 0.375 | 0.500 | 0.625 | 0.750 | 0.875 | 1.000 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Edge x | 48.0000 | 47.7498 | 47.7498 | 47.4971 | 47.4971 | 47.2471 | 47.2471 | 47.0000 | 47.0000 |

**Five distinct positions, four steps, evenly spaced by a quarter of a pixel.**
Recomputed from the printed levels rather than printed themselves, in ascending
order: 0.2471, 0.2501, 0.2527, 0.2502 — quarters seen through eight bits. The
motion is not continuous and it is not whole-pixel: it is quantised at
`1 / SAMPLE_COUNT` of a pixel.

**On "at most four coverage levels per edge":** four is the number of *steps*.
The number of *levels* is five, counting both ends. Recorded because it arrived
as a guess and is now measured.

### And the half MSAA did not reach

The same sweep, reading an **interior** texel boundary instead of the silhouette
— the row where sprite B's upper half meets its lower half. Both sides are opaque
sprite, so nothing is partly covered and MSAA has no coverage to grade:

| Camera y | 0.000 | 0.125 | 0.250 | 0.375 | 0.500 | 0.625 | 0.750 | 0.875 | 1.000 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Boundary row | 64 | 64 | 64 | 64 | 64 | 65 | 65 | 65 | 65 |

**Two positions, one step, a whole pixel.** `Nearest` still takes one sample per
pixel and MSAA does not multiply that, so an interior texel boundary can only
ever land on a whole pixel — the quantisation M3.13 described, unchanged.

The two numbers side by side are the result:

| What moves | Quantum | Steps per pixel of travel |
| --- | --- | --- |
| Silhouette (partly covered edge pixels) | 0.25 px | 4 |
| Interior (texel boundaries under `Nearest`) | 1.00 px | 1 |

`camera_pan_steps.rs` reached the same figure independently in M3.15, on a
different scene with a different atlas and panning the other axis: its control
pixel — an interior pixel deliberately clear of the quad's diagonal — changes
**exactly once** across one pixel of travel, and that is what the file asserts.
Read its printed line with care: it reports interior movement at five steps, of
which one is the flip above the quad's diagonal, one the flip below it, and three
are a single seam pixel flickering. The whole-pixel quantum is the control
pixel's, not the count of printed steps.

**How much of the picture each quantum governs.** Only *partly covered* pixels
are graded, so the quarter-pixel quantum reaches the silhouette and nothing else.
For a pan along one axis that is two edge columns per sprite — 2 × 32 × 3 = 192
of 3 072 sprite pixels, a sixteenth. For a diagonal pan it is the whole
perimeter, 3 × (4 × 32 − 4) = 372, about an eighth.

The 3 072 counts the overlaps twice — the sprites overlap in a chain, 12 × 12
each way, so the drawn area is 2 784 and part of each perimeter is occluded
rather than silhouetted. 372 of 2 784 is still about an eighth.

The rest is interior, and its visible structure is its texel boundaries, which
step a whole pixel. **How many of those there are depends on the texture, and
this scene is a weak case for the argument rather than a strong one**: `atlas()`
paints each 8 × 8 region in two colours split at texel row 4 and uniform across
x, so sprite B has exactly one visible internal boundary along y and none along
x. A sprite whose texture changed colour at every texel boundary would carry 7
per axis against 2 silhouette edges; that ratio is the general case and is **not
measured here**.

What is measured here is the area: roughly a sixteenth of the moving content
takes the finer quantum on an axis pan, and the rest takes the coarser one.
Whether that is what a pan *looks* like is not measured — no animation was
viewed and no perceptual judgement was taken.

### The qualifier-independent carriers (M3.25)

**New instruments, not a re-measurement.** Everything above was measured on
content whose colour changes from texel to texel, and on such content what a
partly covered pixel *shows* depends on where the varying is interpolated —
which is what the uv qualifier decides. D14's two purchased numbers are
therefore asserted on a fixture the qualifier cannot reach: an atlas holding one
colour in every texel. Every sample point then returns the same value, inside
the primitive or outside it, so a pixel is `coverage x colour` and coverage is
the rasteriser's.

Measured 2026-08-09 on `AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu]`, with the
shader as committed (`centroid`).

| Instrument | Fixture | Measured |
| --- | --- | --- |
| `camera_pan_steps`, silhouette steps | one colour, 48 px sprite over 8 texels | `[2, 6, 10, 14]` of 16 — 4 steps, evenly spaced by 4 sixteenths |
| same, interior steps | | none, at any of the 16 |
| same, column 39 across the sweep | | `0, 137, 188, 225, 255` |
| `camera_motion`, identity | one colour, M3.12's three sprites | edge x `48.0000`, centroid `(63.5000, 63.5000)`, mass `2784.00` |
| same, `offset_half_x` | | edge `−0.5029`, centroid `(−0.5000, −0.0000)`, mass `+0.4156` |
| same, `offset_half_xy` | | edge `−0.5029`, centroid `(−0.5000, +0.5000)`, mass `+0.8097` |
| same, `minus_half_x` | | edge `+0.4971`, centroid `(+0.5000, −0.0000)`, mass `+0.4156` |
| same, `minus_half_y` | | edge `+0.0000`, centroid `(−0.0000, −0.5000)`, mass `+0.4156` |
| same, `minus_half_xy` | | edge `+0.4971`, centroid `(+0.5000, −0.5000)`, mass `+0.8097` |

**The y column is the one that changed character.** On the striped scene the
three y cameras lose about 1.9 % of the frame's intensity and miss the centroid
by about 0.11 px, which is why the assertion there had to allow 0.15. On one
colour there is no upper/lower half to re-sample: the miss is 0.0000 and the
mass moves by 0.4156 of 2 784, so both axes are held to the same 0.01 and the
mass to half a percent. That is a **stronger** statement than the one it
replaces, on a weaker fixture — 2 784 is the union area of three 32 × 32 sprites
overlapping 12 × 12 twice, and at the identity camera every edge is on a whole
pixel, so the identity frame has no partly covered pixel at all.

The premises are checked rather than assumed. The step instrument asserts that
**no interior column changes at any step** and that no pixel anywhere takes a
green byte outside the five coverage levels `0, 137, 188, 225, 255` (one count
of slack, the same one-count read-back difference between AMD and llvmpipe this
document records elsewhere) with red and blue exactly 0.

**That one count of slack is not speculative**: on `llvmpipe (LLVM 20.1.2,
256 bits) [Vulkan, Cpu]` the same column 39 reads
`0, 137, 187, 225, 255` — 187 where AMD reads 188, which is `sRGB(0.5)` at
187.52 of 255 rounding the other way. Every other level agrees exactly. The rigid instrument
asserts red and blue exactly 0 over every pixel of every frame. A fixture that
stopped being uniform would redden those before it could quietly weaken the
assurance resting on it.

## What `Linear` would do to the interior quantum, and what its antidote costs

The section above measured that the picture moves in two quanta — the silhouette
in quarter pixels, the interior in whole ones — and that MSAA reaches only the
first. D13 is the decision about the second. **This section collects the numbers
that decision hangs on and takes no position on it.**

Measured 2026-08-08 on commit `79cbbcd` plus one new test file and this section,
nothing under `src/`, by
`crates/narvo-render2d/tests/linear_motion.rs`. Adapters
`AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu]` and
`llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu]`, both `chosen by:
high-performance`. Same scene and same cameras as the M3.16 section.

**Both measures changed**, each for a reason the test file records. The
silhouette moves from sprite B's left edge to sprite A's, because B's bleeds
under `Linear`. The interior becomes a sub-pixel midpoint crossing, because under
`Linear` there is no first row that is not the upper colour. What makes the two
sections comparable is not that the instruments are identical but that the
`Nearest` columns reproduce M3.16's answer: five silhouette positions a quarter
pixel apart, two interior positions a whole pixel apart.

**The two adapters agree on every structural number below** — 5, 2, 5 and 9
distinct positions; padding byte-identical under `Nearest`; the bleed at 48.3760
and 48.0000 padded.

**They do not agree cell for cell**, and the difference is the one the M3.16
section already records: one count of the stored coverage byte where a column is
half covered. The table below is AMD's; llvmpipe reads 27.5031 for the A edge at
cameras 0.375 and 0.500 where AMD reads 27.4971, and 63.8756 for the `Linear`
interior at 0.375 against 63.8804. The sampler disturbance's worst channel
deviation is 177 on AMD and 176 on llvmpipe.

**The production sampler is untouched.** The pipeline is test-only, and it is
checked live against `OffscreenTarget::render_sprites_viewed_by` at `Nearest`,
byte for byte, before any `Linear` number is read — and because nextest gives each
test its own process, it runs in **each** of the three tests that publish a
figure here rather than once for the file — the control shape M3.15a settled on.

### Does `Linear` fix the interior quantum?

One pixel of camera travel in eighths. The silhouette is read on **sprite A's**
left edge, not B's: A's region starts at atlas texel 0, so `ClampToEdge` makes
the bilinear blend partner the edge texel itself and the column measures geometry
alone at either filter.

| Camera | `Nearest` edge | `Nearest` interior | `Linear` edge | `Linear` interior |
| --- | --- | --- | --- | --- |
| 0.000 | 28.0000 | 63.5000 | 28.0000 | 63.5035 |
| 0.125 | 27.7498 | 63.5000 | 27.7498 | 63.6214 |
| 0.250 | 27.7498 | 63.5000 | 27.7498 | 63.7452 |
| 0.375 | 27.4971 | 63.5000 | 27.4971 | 63.8804 |
| 0.500 | 27.4971 | 63.5000 | 27.4971 | 64.0130 |
| 0.625 | 27.2471 | 64.5000 | 27.2471 | 64.1161 |
| 0.750 | 27.2471 | 64.5000 | 27.2471 | 64.2570 |
| 0.875 | 27.0000 | 64.5000 | 27.0000 | 64.3684 |
| 1.000 | 27.0000 | 64.5000 | 27.0000 | 64.5035 |

| | Quantum | Distinct positions per pixel |
| --- | --- | --- |
| Silhouette, `Nearest` | 0.25 px | 5 |
| Silhouette, `Linear` | 0.25 px | 5 |
| Interior, `Nearest` | 1.00 px | 2 |
| **Interior, `Linear`** | **below this sweep's resolution** | **9 of 9** |

**Finding: `Linear` removes the interior's pixel quantum.** Every one of the nine
sampled camera positions gives a different interior position, and the total
travel is 1.0000 px against a camera that moved 1.0. The step is no longer a
property of the pixel grid.

**It is not infinitely fine, and what bounds it is content.** The ramp spans one
texel, so a camera shift of `d` pixels moves the blend by `d / M` where `M` is
the magnification — 4 pixels per texel here. With a contrast of 127 counts across
the boundary that is about 32 counts of output per pixel of shift, so one count
of the 8-bit store is about 1/32 px. The measured positions wander from an exact
eighth by at most **0.0124 px** — recomputed from the table rather than printed,
and the same maximum on both adapters — which is inside that resolution and not a
step. **Coarser
magnification or lower contrast makes it worse, finer magnification better; the
pixel grid does not enter.** That is the substantive difference from `Nearest`,
whose quantum is one pixel whatever the content does.

The silhouette is unchanged at 5 positions and 4 steps, which is what it should
be: MSAA grades geometry coverage and the sampler does not touch it.

### The bleed, in this scene's numbers

Read on sprite B's left edge instead of A's, the `Linear` silhouette measure
returns 48.3760 with the camera at rest, where the geometry is at 48. B's region
begins at atlas texel 8, its leftmost column samples texel coordinate 8.125, and
bilinear puts weight 0.375 on texel 7 — which belongs to another sprite's region.
The column reads 62.5 % of its own colour.

This **corroborates M3.9 rather than repeating it.** M3.9 computed 42 % on a
scene magnifying 6 pixels to the texel across x — its sprite is 48 x 24 over an
8 x 8 region, so 6 across and 3 down — while this one magnifies 4 on both axes
and gives 37.5 %. Same rule, two magnifications, and the difference is exactly
the magnification.

### The three antidotes

Area is computed for a concrete packing — the four 8 × 8 regions of this scene's
16 × 16 atlas — and for the general `n × n` region beside it.

| | Atlas area, this packing | General | Removes the bleed? |
| --- | --- | --- | --- |
| **Padding**, 1 texel of duplicated border | 256 → 400 texels, **+56.2 %** | `((n + 2) / n)²`: +26.6 % at 16, +12.9 % at 32, +6.3 % at 64 | **Yes, measured** |

**Both padding figures are for a border shared with nobody**, and no reliance on
`ClampToEdge` at the atlas rim — which is what `padded_atlas()` builds. A packer
that shares one texel between neighbours is nearer `((n + 1) / n)²`: **+12.9 %**
rather than +56.2 % at `n = 8`, a factor of four less. Not measured here, and the
column headed "general" is general over region size and not over packing.
| **Gutter**, 1 texel of empty space | 256 → 400 texels, **+56.2 %** | same geometry, same figure | No — it changes *which* wrong colour |
| **Half-texel inset** | **no area cost** | none | Yes, in principle |

**Padding is measured on both counts**, and both halves matter:

- **It costs nothing under `Nearest`.** A padded atlas with the regions moved to
  match renders **byte-identical** through the unchanged production path. The
  sampler reads one texel and padding does not change which. Measured on this
  scene through production; no blessed image was rendered, so "costs no
  re-blessing" is the inference that follows and not the measurement itself.
- **It removes the bleed under `Linear`.** B's left edge goes from 48.3760 to
  **48.0000** — the blend partner becomes the region's own duplicated edge texel,
  so the column reads full strength.

**The gutter is the same area for a different result.** An empty texel between
regions makes the blend partner the gutter's own texel rather than a neighbouring
sprite's. With `blend: None` in the pipeline it is the gutter's **RGB** that
lands in the fringe whatever its alpha, so this trades a wrong-coloured fringe
for a gutter-coloured one; it does not remove the fringe. And a gutter is
precisely the case where sharing one texel between two regions is natural, so its
area is the shared figure rather than +56.2 %. Not measured — it needs no measurement to see that the
blend still has a partner.

**The half-texel inset costs no atlas area and still cannot be expressed.** It
shrinks the sampled rectangle by half a texel on each side so the blend never
reaches past the region. M3.9 already named both the obstacle and the way out —
`from_texels` takes whole texels, and a second constructor taking `f32` would do
it — and recorded that the design does not preclude it, only that it costs API.
(That report lives under `target/`, which is git-ignored and does not survive
`cargo clean`, so it is quoted here rather than cited as a durable source.)

The code behind it: `from_texels(left: u32, top: u32, width: u32, height: u32,
...)` at `sprite.rs:184`, against four `f32` fields at `sprite.rs:140-146`. **The
constructor is the constraint, not the representation.**

What each demands beyond the atlas:

- **Padding and gutter** demand no engine change at all. They are content: a
  different atlas and different `from_texels` arguments. What they demand is a
  **tool** — every region's texels must be copied into a larger grid with its
  border replicated, and every region's origin recomputed. Nothing in the
  repository does that, and nothing decides the atlas layout: D3 (scene format,
  due M4) is where content tooling is still open, and an atlas packer is not
  named anywhere in `ProjektPlan.md`.
- **The inset** demands one new constructor on `TextureRegion` and no tool at
  all, which is the opposite trade: engine change, no content change.

### Two samplers side by side

Feasible, and cheaper than it looks. Read from the code rather than argued:

- **The filter is per sampler, not per pipeline.** `mag_filter` and `min_filter`
  are fields of `wgpu::SamplerDescriptor`, and the sampler reaches the shader as
  a *bind group entry* (`quad.rs`'s binding 1), not as pipeline state. Two
  samplers therefore need two bind groups and **one pipeline**.
- **The existing bind group layout already admits both.** It declares
  `SamplerBindingType::Filtering`, and this section's measurements bound a
  `Nearest` sampler and a `Linear` sampler to a **replica** of exactly that shape
  in one run. What was measured is the replica; that "one pipeline" follows is
  read from the API rather than demonstrated, because this file builds a fresh
  pipeline per render and so shows nothing about reuse.
- **The cost lands on batching.** `render_sprites_viewed_by(&self, image, sprites,
  camera)` takes one image for the whole batch, so the rule today is one batch,
  one texture. A per-sprite filter makes it **one batch, one (texture, sampler)
  pair** — a batch would split whenever the filter changes, exactly as it splits
  when the texture changes. For a scene mixing pixel-art and smooth sprites that
  is at most a doubling of draw calls, not a new pipeline per sprite.
- **What it demands of the type.** `Sprite` today is a `SpritePlacement` and a
  `TextureRegion`. It would need a third field naming the filter, or the choice
  would have to move up to whatever owns the texture — and `QuadPipeline` holds
  its `sampler` as a single field, so that field becomes a pair or a small map.

Nothing above is built and nothing is recommended.

## What `Linear` would cost the five blessed images

> **Superseded as an input, 2026-08-08 (M3.21).** Every number in this section
> was measured on **unpadded** copies of the three atlas scenes. Two of those
> three scenes now carry a one-texel border of duplicated edge texels, and the
> `linear_blessed_margin.rs` fixtures that produced these figures are private
> copies that did not move with them. The figures below are therefore an **upper
> bound** for the padded scenes, not a measurement of them: the section "What
> `Linear` would do to the interior quantum" measured that padding removes the
> bleed at a region edge (48.3760 to exactly 48.0000), and it is that bleed the
> margins here are largest at. Nothing was re-measured for M3.21, which was a
> build task; the correction is a bound, not a new number.

The section above measures what `Linear` solves. This one measures what it costs
the **verification strategy** — the part D13 has not been priced on. Two facts
set it up, both already recorded here and neither re-derived: the "Soft-edge
margin" section measured that under `Linear` two rasterisers break the pixel
budget by a factor of 1.77 on a scene built to be hard, and the section "The
blessed images: one of five moved" records all five as **byte-identical** across
three rasterisers, which is what lets one reference serve all three. (The
"Golden-image margin" section is about one image and says so; the five-image
figure is the later one.)

> **Three of the five rows moved slightly under D17, 2026-08-09 (M3.26).
> Nothing here was re-measured and no number in it was edited.** With the shader
> at `center` the same instrument prints `sprite_atlas_regions_128x128` at
> **629 / 3.8391 % / 173** against 635 / 3.8757 % / 180 below,
> `layer_order_regions_128x128` at **1 456 / 8.8867 % / 177** against 1 455 /
> 8.8806 % / 183, and `camera_regions_128x128` at **1 329 / 8.1116 % / 173**
> against 1 327 / 8.0994 % / 173. The other two are unchanged to the pixel. The
> moved pixels are the seam class — where a quad's internal diagonal crosses a
> texel boundary — which is exactly what `center` removes. As with the M3.21 note
> above, these are printed figures and no assertion moved.

> **Four of the five rows have become their own answer, and this section has
> stopped asking a question — 2026-08-09, M3.27 / M3.28 / M3.29. Nothing here was
> re-measured and no number in it was edited.** The section asks what `Linear`
> *would* cost each blessed image, and answers it as the distance from a `Linear`
> render to a `Nearest` reference. For a scene that has since been blessed at
> `Linear` the question no longer applies: the row becomes the distance to a
> reference that is no longer on disk. That has happened to
> `sprite_atlas_regions_128x128` (M3.24/M3.26), to
> `placed_sprite_quadrants_128x128` (M3.27), to
> `layer_order_regions_128x128` (M3.28), and to `camera_regions_128x128` once
> M3.29's blessing is given — **all four of D13's round, which closes there**.
> Only `textured_quad_quadrants_64x64` still has the counterfactual this heading
> promises, and it is the one scene that will never take the offer.
>
> **For `camera_regions_128x128` the bound holds, as it did for layer_order.**
> The same question — a `center` `Linear` render against the same blessed
> `Nearest` PNG — reads **1 329 / 8.1116 % / 173** on this file's unpadded copy
> (M3.26's note above) and **816 / 4.9805 % / 67** on the padded scene the golden
> test renders. That is 0.614 of the bound on pixels and 0.387 on the worst
> channel; the pixel ratio sits at the top of the band the other converted scenes
> show (`sprite_atlas` 0.509, `layer_order` 0.527, `camera` 0.614). Why this one
> sits highest is **not** established here and is not worth a guess.
>
> **`layer_order_regions_128x128` does not collapse to zero, and this is where
> the M3.21 note above pays for this scene.** That note calls these figures an
> upper bound for the padded scenes, because this file's copies are unpadded;
> layer_order is one of the three it names. The bound can now be checked against
> the thing it bounds, and **it holds with room to spare**: the
> same question — a `center` `Linear` render against the same blessed `Nearest`
> PNG — reads **1 456 / 8.8867 % / 177** on the unpadded copy (M3.26's note above)
> and **768 / 4.6875 % / 69** on the padded scene the golden test actually renders.
> That is 0.527 of the bound, which sits inside the band the other two padded
> atlas scenes show for the same padded-against-unpadded pair — `sprite_atlas` 323 of
> 635 (0.509) and `camera` 814 of 1327 (0.613), the figures v0.23 recorded as
> padding roughly halving the blessing price. So after the blessing this row keeps
> measuring
> something real — the padding bleed of an unpadded replica against a padded
> reference — and stops measuring what its heading says.
>
> **For `placed_sprite_quadrants_128x128` the row collapses to zero, and unlike
> the atlas rows it collapses honestly.** The M3.21 note above exempts it —
> that note is scoped to the three *atlas* scenes, and this scene's fixture is
> `quadrant_texture(8)`, a whole texture with no atlas and no padding, so this
> file's replica is not a private unpadded copy but the same fixture production
> uses. With the reference itself a `Linear` render, `margin(reference, linear)`
> compares two renders of the same thing: 360 / 2.1973 % / 173 below becomes
> 0 / 0.0000 % / 0. Predicted from two committed guards
> (`production_at_linear_is_byte_identical_to_this_file_s_linear` and the golden
> test), not measured — this task collected nothing new. **Both guards are
> same-adapter**, so the prediction is for the adapter that produced the blessed
> PNG; on another rasteriser the row would keep the sub-count spread the
> between-rasteriser table records, and worst 0 would become worst 1 or 2. The
> table this note sits above is attributed to the AMD run.
>
> **The rows that take over the load are the between-rasteriser ones** — for
> `placed_sprite_quadrants_128x128` 166 / 0 / 1 against llvmpipe and 149 / 0 / 2
> against WARP, for `layer_order_regions_128x128` 198 / 0 / 1 and 7 / 0 / 1, and
> for `camera_regions_128x128` **518 / 0 / 2 and 330 / 0 / 1 — the widest of the
> four, and now the shipped one**. Until now they priced a hypothetical; from each
> blessing they are the margin a shipped reference actually depends on. Their
> shared caveat, pre-existing and not these tasks' to fix: they were measured on
> 2026-08-08, before D17, and the M3.26 note above covers only the vs-reference
> table. **layer_order's and camera's carry one caveat more than placed_sprite's**:
> placed_sprite's was measured on the same fixture production uses, theirs on the
> unpadded private copy, so they are proxies for a render the shipped scene does
> not perform. Named, not re-collected — and for camera the shipped figure does
> exist in another form: its golden test now reports the whole-image comparison on
> three rasterisers at once, which is what the M3.29 CI run records.

Measured 2026-08-08 on commit `c75550c` plus one new test file and this section,
nothing under `src/`, by
`crates/narvo-render2d/tests/linear_blessed_margin.rs`. Adapters
`AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu] chosen by: high-performance`,
`llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu] chosen by: high-performance`, and
`Microsoft Basic Render Driver [Dx12, Cpu] chosen by: forced software fallback`.

The pipeline is test-only and checked live against
`OffscreenTarget::render_sprites_viewed_by` at `Nearest`, byte for byte, over all
five scenes, in the one test that publishes a measured GPU figure.

**WARP was reached by temporarily putting the software-fallback rung first in two
adapter ladders** — the test file's and `gpu.rs`'s. Injecting only the first is
not enough and does not silently produce a wrong number: the adapter guard fails
the run naming both adapters, which is what happened. Both were reverted and
checked byte-for-byte against SHA-256 snapshots taken beforehand, with a positive
check that the work survived.

### Against the blessed reference: all five move, and not slightly

**AMD's run.** The differing counts and ratios reproduce exactly on llvmpipe; two
of the worst values do not — `textured_quad_quadrants_64x64` reads 176 there
against 177, and `layer_order_regions_128x128` reads 182 against 183. That is the
same one-count spread this document already records between these two adapters,
and it is why this table is attributed rather than presented as all three.

| Reference | Differing, floor 4 | Ratio, budget 0.1000 % | Worst, limit 24 |
| --- | --- | --- | --- |
| `textured_quad_quadrants_64x64` | 960 of 4 096 | **23.4375 %** | **177** |
| `placed_sprite_quadrants_128x128` | 360 of 16 384 | **2.1973 %** | **173** |
| `sprite_atlas_regions_128x128` | 635 of 16 384 | **3.8757 %** | **180** |
| `layer_order_regions_128x128` | 1 455 of 16 384 | **8.8806 %** | **183** |
| `camera_regions_128x128` | 1 327 of 16 384 | **8.0994 %** | **173** |

Both thresholds break in every image, the budget by factors of 22 to 234 and the
cap by about seven. The distributions are long and flat rather than spiky — the
64 × 64 quad alone spreads 960 differing pixels across 52 distinct deviations
from 29 to 177 — which is what a texture being resampled everywhere looks like,
as against a geometry edge moving.

**All five would need a fresh blessing.** That is not a tolerance question and no
threshold could absorb it: a `Linear` render of these scenes is a different
picture, not a noisier one.

### Between rasterisers: byte-identity is lost, agreement is not

Same renders, compared against another rasteriser's run of the same test rather
than against a reference.

| Reference | AMD ↔ llvmpipe | AMD ↔ WARP |
| --- | --- | --- |
| | differ at all / past floor / worst | differ at all / past floor / worst |
| `textured_quad_quadrants_64x64` | 359 / **0** / 1 | 38 / **0** / 1 |
| `placed_sprite_quadrants_128x128` | 166 / **0** / 1 | 149 / **0** / 2 |
| `sprite_atlas_regions_128x128` | 229 / **0** / 1 | 181 / **0** / 3 |
| `layer_order_regions_128x128` | 198 / **0** / 1 | 7 / **0** / 1 |
| `camera_regions_128x128` | 518 / **0** / 2 | 330 / **0** / 1 |

Distribution: almost every differing pixel is off by exactly 1 count. The
exceptions are 2 pixels at 2 counts in the camera scene against llvmpipe; and
against WARP, 28 at 2 counts in `placed_sprite_quadrants_128x128`, and 11 at 2
with 2 at 3 in `sprite_atlas_regions_128x128`. **Nothing anywhere reaches 4**,
which is the floor `Tolerance::default()` counts from.

**Zero pixels past the floor in all ten pairs**, against a budget of 0.1000 %,
and a worst channel deviation of 3 against a limit of 24.

### The three questions

**Does one reference still serve three rasterisers?** *Finding: yes, with the
margin nearly intact.* **Both measured pairs** report 0.0000 % against a
0.1000 % budget and at most 3 counts of 24.

**llvmpipe ↔ WARP was not measured**, and naming that matters: in the "Soft-edge
margin" section it is *not* the benign pair. The conclusion survives without it
by the triangle inequality — per scene the two measured pairs are at most 1 and 3
counts, so llvmpipe against WARP is at most 4, which is *at* the floor and not
past it — but that is a bound and not a measurement.

**Does a threshold break, and by how much?** *Finding: not between rasterisers —
none of the three is approached.* Against the *references* every threshold breaks
by large factors, but that is the images being outdated rather than the
thresholds being wrong, and re-blessing fixes it where a wider threshold would
only hide it. **No threshold was changed.** A reasoned new number would need what
this measurement does not supply: the same comparison over many scenes rather
than five, including at least one that is not axis-aligned and one magnified by a
non-integer factor, and a statement of which of the three thresholds is meant to
catch what. The soft-edge section's 1.77 factor came from a scene built to be
hard; these five are not, and averaging the two would be worse than either.

**Do any stay byte-identical, and is that a property or a coincidence?**
*Finding: none.* All five differ from at least one other rasteriser, by 7 to 518
pixels. There is nothing to explain.

The **shape** of the loss is worth stating, since it is what changed. Under
`Nearest` the byte-identity was not luck: the sampler returns one texel unaltered
and every rasteriser agrees on which, so the result is exact and there is nothing
to round. Under `Linear` every interior pixel is a weighted sum, the weights come
from an interpolated coordinate, and the last bit of that coordinate is not
specified to agree across implementations. The disagreement is therefore expected
everywhere the blend is non-trivial, and it is bounded by the 8-bit store rather
than by any property of these scenes. **The 1-count spread is the same one the
"Soft-edge margin" section records between these adapters**, arriving in a place
that used to be exact.

### What a second sampler costs the batch

D13 keeps the second sampler in view from the start, which touches the throughput
strand `ProjektPlan.md` §6/M3 records as closed. Reported, not built.

**Draw calls: one more per additional (texture, sampler) pair.** Today the rule is
one batch, one texture — `render_sprites_viewed_by` takes one image for the whole
batch. A per-sprite filter makes it one batch per *(texture, sampler)* pair, so a
scene mixing pixel-art and smooth sprites over one atlas goes from one draw call
to two. **Not measured**: the offscreen round-trip table below times whole calls
including read-back and does not isolate a draw call, so nothing here prices one.

**Preparation: bounded by a figure that is measured.** Splitting a batch by
sampler adds one pass over the sprite list to partition it; it does not change
the vertex arithmetic, since the same sprites still produce the same vertices.
That pass is the same shape of work as the "pure copy of the placements" column
in the preparation table below — one traversal writing into another buffer — which
is **14.6 µs at 50 000 sprites, 1.4 % of preparation**. So a second pair costs at
most about 1.4 % more preparation. *Derived from a measured figure, not itself
measured.* Whether a partition is *cheaper* than a copy is *not* claimed here: a
stable partition generally needs a scratch buffer or two outputs, and this
document already records one intuition about this same function measuring
backwards, under "Preallocating `placements_of` was measured and is slower".

**§6/M3's "about 6 % of a 16.7 ms frame" is untouched.** Preparation at 50 000
sprites is about 1.0 ms; adding 1.4 % makes it about 1.01 ms, which is 6.1 %
rather than 6.0 %. The claim is stated to one significant figure and survives.

**What the type would need.** `Sprite` is a `SpritePlacement` and a
`TextureRegion`. A per-sprite filter needs a third field, or the choice moves up
to whatever owns the texture. `QuadPipeline` holds its `sampler` as one field,
which becomes a pair; the bind group layout already declares
`SamplerBindingType::Filtering` and needs no change.

**And there is a conflict here larger than any of that.** Batching by sampler and
depth order do not compose. `narvo-app`'s `sprite_batch.rs` sorts by
`(depth, EntityId)` and states that draw order *is* the order of that vector,
because there is no depth buffer — `depth_stencil: None`, so a later sprite
overwrites an earlier one where they overlap. **Two draw calls therefore mean
everything in the second lands on top of everything in the first, whatever its
depth.** Splitting a batch by sampler reorders sprites across depths by
construction.

Stability is not the issue, and an earlier draft of this section said it was.
That sort's key is a total order with ties broken by `EntityId`, so no two
sprites compare equal, and the file records that the result therefore does not
depend on the sort being stable. The issue is that painter's-algorithm depth
ordering assumes one draw call across the whole batch. A second sampler would
cost a draw call, a partition, **and** an answer to that — which this section does
not have, because nothing here builds it.

## Two draw calls and the drawing order (D15)

M3.18's self-audit found that batching by sampler and z-ordering do not compose:
there is no depth buffer, so the drawing order *is* the painter's algorithm and
everything in a second draw call lands on top of everything in the first. D13 is
decided and needs a second sampler; **D15 is its precondition and is open**. This
section holds what M3.19 measured. The calculations are not here — they belong to a report, and
`target/` is git-ignored, so they are named rather than cited: the worst case for
the multi-batch way is one draw call per sprite, and the naive non-overlap check
is quadratic in the sprite count.

Measured 2026-08-08 on commit `6e36140` plus one new test file, this section and
an uncommitted `ProjektPlan.md`, nothing under `src/`, by `crates/narvo-render2d/tests/draw_order_margin.rs`.
Adapter `AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu] chosen by:
high-performance`. Test-only pipeline throughout; `quad.rs` is untouched.

The pipeline is checked live against `OffscreenTarget::render_sprites_viewed_by`
over all five blessed scenes, byte for byte, in each of the four tests that
publish a measured GPU figure.

### The incompatibility, demonstrated

Two 32 × 32 sprites with uniform-coloured regions. `Nearest` and `Linear` agree
everywhere except within one texel of a region edge, where `Linear` blends into
the neighbouring region — the bleed the padding section above prices. The probe
at (64, 64) is eight pixels from the nearest region edge, so the sampler cannot
be what changes *it*; columns 70 and 71 carry that bleed as well as the
draw-order swap, and both lie inside the overlap and count as differing either
way.
`BACK` covers image columns 40..72, `FRONT` covers 56..88, both cover rows 48..80.
Ranges here are half-open, as Rust writes them; the bounding box below is given
by its inclusive corners.

| Rendering | Probe (64, 64) |
| --- | --- |
| One draw call, in depth order | `[0, 255, 0]` — `FRONT`, correct |
| Two draw calls, split by sampler | `[255, 0, 0]` — **`BACK`, wrong** |

**512 pixels differ, and the differing region is exactly the overlap**: bounding
box (56, 48) to (71, 79), which is 16 columns by 32 rows. Nothing outside it
moves. The sprite that should be behind owns the overlap because it is in the
second draw call.

### A depth buffer

| Question | Measured |
| --- | --- |
| Does it restore the order across two draw calls? | **Yes — 0 differing pixels** against the single-draw-call render, with distinct depths and `Less` |
| Does it move the five blessed images? | **No — 0 differing, worst 0**, all five, with z decreasing by draw index against the production path — though only `layer_order_regions_128x128` and `camera_regions_128x128` contain overlapping sprites and can discriminate; the other three are single-sprite or non-overlapping and return 0 under any ordering |
| Memory, 128 × 128 | 262 144 B |
| Memory, 1920 × 1080 | 33 177 600 B (31.6 MiB) |

`Depth32Float` is 4 bytes per sample and the attachment is multisampled like the
colour one, so at `SAMPLE_COUNT` = 4 it costs the same as the multisample colour
attachment the MSAA section already prices — the two are the same arithmetic.

**Equal depths are decided by the compare function, not by the sort.** The
equal-depth case is not what the blessed scenes do — four of them are `Sprite`
arrays handed straight to the renderer with no depth at all, and
`layer_order_regions_128x128` is built from a world whose three entities carry
*distinct* `Layer` depths. It is what a world *without* `Layer` components does:
every entity sits at `Layer::DEFAULT`'s depth and `placements_of` breaks the tie
on `EntityId`, so the later sprite wins by being drawn later. Under a depth
buffer, with both sprites at the same z:

| Compare function | Probe (64, 64) |
| --- | --- |
| `Less` | `[255, 0, 0]` — the **first** sprite keeps the overlap; the second's fragments fail wherever the first already wrote, though it draws normally outside the overlap |
| `LessEqual` | `[0, 255, 0]` — the later sprite wins, which is today's behaviour |

So a depth buffer does not by itself preserve what the renderer does now. It
preserves it under `LessEqual` and inverts it under `Less`, and that choice is
not currently made anywhere because there is nothing to make it in.

## The multi-batch decomposition (D15), and what it costs

D15 is decided: multi-batch sorting. The depth-ordered sequence is cut into runs
wherever the sampler wish changes, one draw call per run, and depth stays an
ordering rather than simulation state. M3.20 built the decomposition and nothing
else — there is still one sampler, so it yields exactly one run and **no blessed
image moves**.

Measured 2026-08-08 on commit `2da86f1` plus this change, `release` profile,
best of five after one unmeasured warm-up — the shape the preparation table below
uses. Machine as described there.

### What the decomposition costs

| | 50 000 sprites, one filter |
| --- | --- |
| M3.19's calculation, as a bound | ≤ 14.6 µs, the "pure copy of the placements" column |
| **Measured** | **21.7–27.1 µs**, nine runs across two working copies |

**The calculation was optimistic by 1.49 to 1.86 times**, and that is the
finding rather than something to fix. As a share of the 1 025.5 µs preparation
total it is **2.1 % to 2.6 %**, against the 1.4 % M3.19 predicted. Reported as a
band because it is one: the spread between runs is wider than the gap to the
bound, so a point value would be false precision.

The cause is not chased — `ProjektPlan.md` §7's rule is that spread is the
finding. Worth recording only that the two are not the same traversal: the copy
column moves `SpritePlacement`, five `f32`, while the decomposition walks
`Sprite`, which carries a region and a filter as well.

**Both M3.7 guards survive the change.** `extracting_placements_stays_linear_in_the_entity_count`
reports a ratio in the 11–14 band against a linear 10 — nine runs across two
working copies, a band for the same reason — and `the_cost_of_extracting_placements_is_recorded`
still prints its capacity column. Neither was touched.

### What it does not cost

Nothing in the picture. With one filter in the world the decomposition returns a
single run covering every sprite, the render path issues one `draw_indexed` over
the whole index range, and all five blessed images stay byte-identical in their
regular tests — worst channel deviation 0, five of five.

Four of the five go through the new path and are proved to: with `batch_runs`
forced to return no runs, `placed_sprite_quadrants_128x128`,
`sprite_atlas_regions_128x128`, `layer_order_regions_128x128` and
`camera_regions_128x128` all fail. `textured_quad_quadrants_64x64` passes, because
it is drawn by `render_textured_quad` through the single-quad path and never
reaches the batch — the same distinction M3.15a recorded.

## Frame preparation for a sprite batch

What it costs, on the CPU, to turn a world into the vertex data for one batch.
ADR-0015 rests on this being measurable: the decision to hand the renderer an
explicit buffer instead of letting it query the world was argued on the grounds
that the copy buys a measurement. Until M3.7 that measurement did not exist, so
the load-bearing sentence of an architecture decision was an assertion.

Measured 2026-08-07 on commit `c949c2f`, `release` profile, best of 25 rounds
after one unmeasured warm-up. Machine as described below.

| Sprites | `placements_of` (world → placements) | `batch_vertices` (placements → vertices) | pure copy of the placements | total preparation |
| --- | --- | --- | --- | --- |
| 100 | 1.2 µs | 0.4 µs | 0.1 µs | 1.6 µs |
| 1 000 | 9.0 µs | 8.0 µs | 1.9 µs | 17.0 µs |
| 10 000 | 87.3 µs | 35.9 µs | 2.6 µs | 123.2 µs |
| 50 000 | 599.7 µs | 425.8 µs | 14.6 µs | 1 025.5 µs |

**The 1 000 and 10 000 rows are not monotonic per sprite** — 8.0 ns and 3.6 ns
respectively for `batch_vertices` — and that is left visible rather than
smoothed. The cause is not established. The 50 000 row is the one the decision
turns on and it is stable across runs.

**The copy is 1.4 % of the preparation at 50 000 sprites.** That figure is what
ADR-0015's revision condition asks for, and it is a lower bound on what the
rejected alternative would have saved: a renderer iterating the world in place
would avoid materialising the vector, but not the per-entity lookup, not the
sort in `World::entity_ids`, and not the vertex arithmetic.

**Allocation structure, as counted rather than timed:**

| | allocations | grows? |
| --- | --- | --- |
| `batch_vertices` | one, `with_capacity(4n)` | no — capacity equals length exactly |
| `World::entity_ids` | one for the vector, plus a `sort_unstable` over it | — |
| `placements_of`'s `collect` | about `log2(n)` | yes — `filter_map`'s size hint has a lower bound of zero, so the vector doubles: 100 → 128, 1 000 → 1 024, 10 000 → 16 384, 50 000 → 65 536 |

The last row is a finding, not a fault that was fixed here. M3.7 measures.

### Amended 2026-08-07 (M3.8): the `placements_of` figures above are not reproducible

*The table stands as the record of what was measured on `c949c2f`. This says
what happened when M3.8 tried to reproduce it, because a figure that cannot be
reproduced has to be labelled rather than quietly replaced.*

`batch_vertices` reproduces exactly — 35.8 against 35.9 µs at 10 000, 425.1
against 425.8 µs at 50 000, on unchanged code. **`placements_of` does not.**
Eight runs on the same commit, same procedure, gave:

| Sprites | recorded M3.7 | M3.8, eight runs (min – max) | median |
| --- | --- | --- | --- |
| 10 000 | 87.3 µs | 106.4 – 197.3 µs | ~186 µs |
| 50 000 | 599.7 µs | 636.6 – 928.8 µs | ~773 µs |

Both recorded values sit **below the minimum of eight later runs**. The M3.7
figures were single best-of-25 values from one process each, and this function's
run-to-run spread is 20 to 45 per cent — so they were the bottom of a
distribution nobody had characterised, recorded as if they were a point. The
recorded numbers are not wrong about what that run measured; they are wrong as a
baseline for a before-and-after.

**Consequence for how this file is read:** a single figure for `placements_of`
means little. `batch_vertices` and the golden-image margins are stable to the
last digit and can be compared across commits; this one needs a distribution.

### Preallocating `placements_of` was measured and is slower

M3.7 reported the growing `collect` as a finding to be fixed. M3.8 built the fix
— `Vec::with_capacity(entities.len())` plus a push loop — and measured it
order-balanced in one process, alternating which shape ran first so that neither
inherits the block the other just freed:

| Entities | growing `collect` | preallocated | difference | capacity |
| --- | --- | --- | --- | --- |
| 1 000 | 9.1 – 11.4 µs | 16.5 – 21.0 µs | **+7.3 to +9.7 µs** | 1 024 → 1 000 |
| 10 000 | 159.9 – 178.3 µs | 186.9 – 220.5 µs | **+8.6 to +58.6 µs** | 16 384 → 10 000 |
| 50 000 | 732.5 – 755.7 µs | 812.4 – 880.7 µs | **+79 to +139 µs** | 65 536 → 50 000 |

Five runs, three sizes, the same sign every time, and two independent eight-run
standalone distributions agree. **The change was not kept.** Why it is slower is
not established; a fresh megabyte faulted in page by page against growth that
`realloc` can extend in place is a hypothesis and is recorded as one.

A first, order-*unbalanced* pairing had reported the opposite — preallocation 44
per cent faster — because the growing shape always ran first and freed a hot
block the preallocated one then reused. That measurement was wrong and is
recorded here so the next person recognises the shape of the error.

### What the sort in `entity_ids` costs

Isolated without instrumenting `entity_ids`: a query yields entities in
archetype order, which is the order `entity_ids` collects before sorting, so a
clone of that vector sorted with `sort_unstable` is the same work on the same
data. This times an equivalent sort, not the call itself.

Measured 2026-08-07, `release`, best of 25, 50 000 entities:

| | ns | share of `entity_ids` | share of `placements_of` | share of the whole preparation |
| --- | --- | --- | --- | --- |
| sort of the archetype order | 28.7 µs | 47 % | 3.6 % | ~2.4 % |
| `World::entity_ids` in full | 61.3 µs | — | 7.7 % | ~5 % |
| `placements_of` in full | 795.1 µs | — | — | ~65 % |

**The sort is not where the time goes.** M3.7 called it "plausibly a noticeable
share of the 600 µs"; it is 3.6 per cent of the extraction. Nor is `entity_ids`
as a whole: at 7.7 per cent, the remaining 734 µs are the 50 000 per-entity
`world.get::<Transform>` lookups, about 14.7 ns each.

**A caveat that belongs to the number.** The test world has one archetype, and
its archetype order is therefore *already ascending* — the measurement printed
`archetype order already ascending: true`, and sorting sorted input is the best
case for a pattern-defeating sort. Sorting the same data already in order took
32.3 µs, indistinguishable from the 28.7 µs above, which confirms the sort is
getting easy input rather than being fast. A world with several archetypes would
present entities grouped by archetype, and the sort would cost more; how much
more is unmeasured.

## Offscreen round trip with N sprites

**Not the 60 fps figure of `ProjektPlan.md` §6/M3**, and it cannot become it.
This times a whole `render_sprites` call including the blocking read-back of the
1920 × 1080 target — 8.3 MB copied back to the CPU that a windowed frame never
copies. It is an upper bound on GPU-side cost, recorded because it is what can
be measured without a frame loop, which is a capability and not a measurement.

Measured 2026-08-07 on commit `c949c2f`, `release`, best of three, adapter
`AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu] chosen by: high-performance`.

**These four rows are the renderer before MSAA and are kept as the "before"
column of the M3.15 section above.** The current figures are there. The 50 000
row is the only one that moved.

| Sprites | Offscreen round trip |
| --- | --- |
| 1 | 1.31 ms |
| 1 000 | 1.25 ms |
| 10 000 | 1.61 ms |
| 50 000 | 2.84 ms |

About 1.3 ms of that is fixed cost that does not vary with the sprite count; the
marginal cost of 50 000 sprites over one is roughly 1.5 ms.

**What would move any of these numbers:** a different CPU or memory
configuration for the preparation table, and the GPU driver version for the
round trip — so both are moment values. The preparation figures also depend on
the profile: they are `release`, and the same code under the `dev` profile that
`cargo xtask ci` uses is slower by a factor that is not constant across the
rows. The regression guards in `sprite.rs` and `sprite_batch.rs` deliberately
gate on a ratio and on an allocation count, neither of which moves with any of
the above.

## The frame loop at 50 000 sprites: the 60 fps figure of §6/M3

**This is the figure `ProjektPlan.md` §6/M3 asks for, and it is met.** At 50 000
sprites the frame's own work costs **6.74 ms at p50** and 8.64 ms at p99,
against a 16.67 ms budget — about 40 per cent of it. Not one interval in a
1 000-frame run exceeded the budget.

**Hardware-bound, and no CI guard watches it.** §6/M3 splits the throughput
criterion deliberately: the 50 000 sprites at 60 fps are "eine protokollierte
Messung auf der Referenz-GPU in `BASELINE.md`, **ohne Gate**", and what CI
guards instead is the GPU-free half. Nothing in `.github/workflows/ci.yml` runs
this measurement and nothing there can — no job opens a window — and the figure
is a property of this CPU, this adapter, this driver and this display. A green
CI run says nothing about the numbers below, and a regression in them cannot
turn CI red. Re-measure by hand.

Measured 2026-08-10 on commit `6cccea4` plus this change, `release` profile, on
the machine described below. Produced by
`narvo --frames 1000 --sprites 50000 [--uncapped]`, which is a command rather
than a test for the reason above: 1 000 frames after
60 discarded as warm-up, in a window whose inner size is requested as
1280 × 720 *logical* pixels; this machine runs at 96 DPI (100 per cent scaling),
so the framebuffer is 1280 × 720 too, and on a scaled display it would not be.
Every **duration** below is a **nearest-rank** quantile over those frames — the
value reported is one some frame actually took, never an interpolation between
two. The counts and rates beside them are not quantiles.

### What the phases are, and what each one does not contain

Five, not four. The extra one is `acquire`, and separating it is what makes the
rest of the table mean anything.

| phase | what it is | what it does **not** contain |
| --- | --- | --- |
| `tick` | every simulation tick the frame owed, summed | — |
| `extract` | `placements_of` and `camera_of`, world → scalars | — |
| `acquire` | asking the swapchain for an image | — this is **where the display wait lands**; it is pacing, not renderer cost |
| `encode+submit` | vertex and index buffers, atlas upload, command recording, `queue.submit` | **the GPU executing any of it.** `queue.submit` returns as soon as the work is handed over |
| `present` | `queue.present` | — |

`work` is `tick + extract + encode+submit`. The two handovers are excluded
because both wait on something outside the process. `interval` is the whole
frame, start to start, waiting included — it is what the accumulator is fed, and
it is the series to read if only one is read.

### (a) The work, with the display taken out of the number

Present mode **Immediate**, taken by asking for `PresentPolicy::Uncapped`.
`WindowTarget::present_mode` reports the mode actually taken, and it is carried
here rather than assumed. Nothing waits for a scan-out, so these are the work's
own figures.

| phase | p50 | p99 | max |
| --- | --- | --- | --- |
| tick | 0.000 ms | 1.750 ms | 2.378 ms |
| extract | 3.758 ms | 3.915 ms | 4.370 ms |
| acquire | 0.147 ms | 0.289 ms | 0.351 ms |
| encode+submit | 2.512 ms | 3.234 ms | 3.425 ms |
| present | 0.061 ms | 0.160 ms | 0.201 ms |
| **work** | **6.738 ms** | **8.644 ms** | **9.183 ms** |
| interval | 7.020 ms | 9.029 ms | 9.481 ms |

**0 of 1 000 intervals over the 16.67 ms budget. 1 000 of 1 000 frames drawn.**

**`extract` is the largest phase — 56 per cent of the work.** It is
`placements_of` plus `camera_of`, and the two enumerate the world separately, so
all 50 002 entity ids — the 50 000 sprites, the wander target and the camera —
are collected and sorted twice per frame. `sprite_batch.rs`
documents that double enumeration and says no benchmark had priced it; this is
the price. M3.7's own figure for `placements_of` alone at 50 000 was 0.60 ms and
M3.8 re-measured it at 0.64–0.93 ms, so most of the 3.76 ms here is not that
function getting slower — it is the second enumeration plus a world of 50 002
entities rather than the bare-`Transform` fixture those numbers came from.

**`tick` is zero at p50 and that is arithmetic, not an idle simulation.** The
frame takes ~7 ms and the step is 16.67 ms, so most frames owe no tick at all
and some owe one: 442 ticks over 1 000 frames. The column is bimodal and its p50
is the lower mode. A frame's tick cost is better read off the p99.

### How the cost scales

Same command, `--uncapped`, 400 frames per row.

| sprites | tick p50 | extract p50 | encode+submit p50 | work p50 | work p99 | interval p50 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 000 | 0.000 ms | 0.074 ms | 0.130 ms | 0.205 ms | 0.301 ms | 0.379 ms |
| 5 000 | 0.000 ms | 0.339 ms | 0.260 ms | 0.618 ms | 0.813 ms | 0.823 ms |
| 10 000 | 0.000 ms | 0.720 ms | 0.594 ms | 1.412 ms | 1.950 ms | 1.644 ms |
| 20 000 | 0.000 ms | 1.450 ms | 1.050 ms | 2.551 ms | 3.751 ms | 2.791 ms |
| 30 000 | 0.000 ms | 2.168 ms | 1.479 ms | 4.020 ms | 5.278 ms | 4.315 ms |
| 50 000 | 0.000 ms | 3.773 ms | 2.526 ms | 6.864 ms | 8.661 ms | 7.158 ms |

**Linear in the sprite count**, about 0.137 µs of work per sprite — 0.075 µs of
it `extract` and 0.051 µs `encode+submit`. Extrapolating the p99, the budget
would be reached somewhere near 95 000 sprites; that is an extrapolation and was
not measured, and `MAX_SPRITES_PER_BATCH` is 65 536 anyway.

### (b) The Fifo run: does the loop hold a rate?

Present mode **Fifo**. **The display on this machine runs at 200 Hz**, measured
rather than assumed (`Win32_VideoController.CurrentRefreshRate` = 200 at
2560 × 1440), so a vsync-paced run here is paced to 5.0 ms and not to 16.67 ms.
That is an input to this half of the measurement and the rows below cannot be
read without it.

| sprites | interval p50 | interval p99 | achieved rate | frames drawn | intervals over 16.67 ms |
| --- | --- | --- | --- | --- | --- |
| 50 000 | 7.090 ms | 9.033 ms | 141 /s | 1 000 of 1 000 | 0 of 1 000 |

**At 50 000 sprites the loop holds 141 frames a second** — above the 60 the
criterion asks for and below the 200 the display offers, because the work
(6.80 ms) is longer than the display's 5.0 ms period. Every frame was drawn.

What cannot be said from these numbers is that no *vertical blank* was missed:
wgpu 30 exposes no presented-frame statistics, so a missed blank is not
observable here. "Intervals over budget" counts intervals and is a **proxy**,
not a driver statistic.

**The display wait really does land in `acquire`, and this is the evidence.**
Under Fifo, as the work grows the acquire phase shrinks by very nearly the same
amount while the interval stays pinned to the display's period:

| sprites | work p50 | acquire p50 | interval p50 |
| --- | --- | --- | --- |
| 1 000 | 0.300 ms | 4.512 ms | 4.998 ms |
| 10 000 | 1.589 ms | 3.238 ms | 4.979 ms |
| 20 000 | 2.982 ms | 1.900 ms | 4.992 ms |
| 30 000 | 4.334 ms | 0.337 ms | 4.966 ms |

That is why `acquire` is a phase of its own and why it is excluded from `work`.
It also means **the per-phase quantiles are not independent under Fifo**:
`acquire` is slack there, so the sum of the phase medians is not the median
frame.

### What the GPU costs, as an attribution rather than a frame time

There is no timestamp query in this engine — `gpu::create_device` requests no
device features — so GPU execution cannot be read directly. What can be done is
to stop and wait for it, which `--drain` does.

| run (50 000, Immediate) | work p50 | interval p50 | interval p99 |
| --- | --- | --- | --- |
| pipelined | 6.738 ms | 7.020 ms | 9.029 ms |
| drained every frame | 6.806 ms | 7.173 ms | 9.356 ms |

**About 0.15 ms of GPU work was still outstanding** when the CPU finished a
frame. The GPU is nowhere near the constraint here. This is an attribution and
not a frame time: draining serialises CPU and GPU instead of letting them
overlap, so the drained run is slower *because* it was measured that way.

### What the D15 decomposition is worth, measured by getting it wrong

The scene asks half its sprites for `Nearest` and half for `Linear`, as **two
contiguous blocks** in the drawing order, so `batch_runs` cuts the batch into
three runs — the two blocks plus the wander target, which carries no `Sampling`
and is drawn last — and the frame issues three draw calls.

An earlier revision of the scene alternated the two samplers per sprite instead.
`batch_runs` cuts wherever the wish changes, so that is one run *per sprite*:
**50 001 draw calls per frame**, the worst case M3.20 had measured GPU-free.
Both configurations were measured, on the same build save that one function:

| sampler layout | draw calls | encode+submit p50 | work p50 | intervals over budget |
| --- | --- | --- | --- | --- |
| two blocks | 3 | 2.512 ms | 6.738 ms | 0 of 1 000 |
| alternating | 50 001 | 16.985 ms | 22.945 ms | 1 000 of 1 000 |

**A degenerate sampler layout costs 6.8× the encode time and 3.4× the frame's
work, and it is the difference between meeting the criterion and missing it by
40 per cent.** The batching is not a micro-optimisation; it is the whole
difference. Recorded here because the number only exists by accident — the
alternating layout was a defect in the scene, caught by the M3.32 self-audit
before the commit, and its measurement was kept rather than discarded.

**What would move any of these numbers:** the CPU for all three work phases,
since all three are CPU-side; the GPU driver for the drain row; and **the
display's refresh rate for every Fifo row**, which is why it is stated above
rather than assumed to be 60. The profile matters too — these are `release`, and
the `dev` profile `cargo xtask ci` uses is slower by a factor that is not
constant across the phases. Sprite size, window size and the sampler layout are
fixed by the scene: 16 × 16 world units, 1280 × 720, two contiguous sampler
blocks.

## Machine

Every row of the history table has to come from this machine, or from a machine
documented in its own section below it. Comparing across hardware is not a trend,
it is noise.

| | |
| --- | --- |
| CPU | AMD Ryzen 7 7700X, 8 cores / 16 threads, 4.5 GHz max |
| RAM | 32 GB DDR5-5600 (2 x 16 GB) |
| Storage | WD_BLACK SN850X 2 TB, NVMe SSD - workspace on `D:` |
| GPU | AMD Radeon RX 9070 XT |
| Measured | 2026-08-06 (M0 rows), 2026-08-07 (M2.3 rows) |

### Windows

| | |
| --- | --- |
| OS | Windows 11 Pro, build 26200, x86_64 |
| Toolchain | rustc 1.97.1 (8bab26f4f 2026-07-14), cargo 1.97.1 (c980f4866 2026-06-30) |
| Target | `x86_64-pc-windows-msvc` |
| Linker | `rust-lld` (see `.cargo/config.toml`) |
| Test runner | cargo-nextest 0.9.143 |
| Profile | `dev` - workspace crates at opt-level 1, dependencies at opt-level 3 |
| Target directory | `D:\Narvo\target` |

### Linux (WSL 2), from M2.3

The same physical machine and the same working copy, read through drvfs from
`/mnt/d/Narvo`. ADR-0007 records what this can and cannot tell us; the short
form is that it is a pre-check and the CI runner remains the authority.

| | |
| --- | --- |
| OS | Ubuntu 24.04.4 LTS on WSL 2 |
| Kernel | 6.18.33.1-microsoft-standard-WSL2 |
| libc | GNU libc 2.39 (Ubuntu GLIBC 2.39-0ubuntu8.7) |
| Toolchain | rustc 1.97.1 (8bab26f4f 2026-07-14), cargo 1.97.1 (c980f4866 2026-06-30) |
| Target | `x86_64-unknown-linux-gnu` |
| Linker | `clang` with `-fuse-ld=lld` (see `.cargo/config.toml`) |
| Test runner | cargo-nextest 0.9.143 |
| Target directory | `$HOME/.cache/narvo/target`, inside the distro |

Both platforms resolve to the same rustc *commit hash*, not merely the same
version string. That is what `rust-toolchain.toml` is for, and it is checked by
reading `rustc --version` on both rather than assumed.

**The two target-directory rows carry post-rename paths (U3, 24.08.2026).**
Every measurement in the history table predates the rename and was taken
under the old directory names, which ADR-0047 item 2 spells. The two rows got
there differently, and the difference is worth one sentence: the Windows row
moved because the working directory really was renamed, while the Linux row
states the directory this repository expects — the distro's own
`~/.cargo/config.toml` had not been edited when this was written, and
`CLAUDE.md`'s WSL section carries that open gap. Both rows are written in
today's form because they describe the machine a re-run happens on, and a
re-run happens now; the path is not a variable any of these numbers depend
on, since it names where artifacts land and not what is compiled.

## Procedure

Run with the workspace quiesced: no editor indexing, no other build in flight.
Times are wall clock as measured around the command. Clean builds are measured
once, because each one costs a minute and the figure is not precise enough for
repetition to help; every other figure is the **best of three**, which is what
the M2 budget checks quote.

### a) Clean debug build

```
cargo clean
cargo build --workspace
```

| Metric | M0 | M0 close | M2.3 (Windows) | M2.3 (Linux) |
| --- | --- | --- | --- | --- |
| Wall clock | 0.35 s | 0.37 s | 59.57 s | 45.27 s |
| cargo "Finished in" | 0.29 s | 0.32 s | 59.48 s | 45.23 s |
| Units compiled | 4 | 4 | 113 | 143 |

### b) Incremental rebuild

An mtime touch on one crate's `src/lib.rs`, then:

```
cargo build --workspace
```

Two probes, because they answer different questions. `narvo-core` sits at the
bottom of the dependency graph, so touching it rebuilds everything above — the
worst case a developer hits routinely. `narvo-ecs` is where the simulation work
actually happens, and it is the figure §8.1 and ADR-0009 quote.

| Metric | M0 | M0 close | M2.3 (Windows) | M2.3 (Linux) |
| --- | --- | --- | --- | --- |
| Touch `narvo-core` | 0.31 s | 0.28 s | 1.55 s | 1.19 s |
| Units recompiled | 4 | 4 | 5 | 5 |
| Touch `narvo-ecs` | — | — | 1.40 s | 1.12 s |
| Units recompiled | — | — | 3 | 3 |

The M0 columns were taken with the older probe — a comment inserted and removed
again — and `narvo-ecs` did not exist yet.

### c) Test run

```
cargo nextest run --workspace
```

Measured twice, because the two numbers answer different questions. *Cold* still
has to compile the test harnesses, which `cargo build` does not produce; *warm*
is the tight edit-test loop and is the figure carried into the history table.

| Metric | M0 | M0 close | M2.3 (Windows) | M2.3 (Linux) |
| --- | --- | --- | --- | --- |
| Wall clock, cold | 0.63 s | 0.67 s | 7.04 s | 3.74 s |
| Wall clock, warm | 0.17 s | 0.20 s | 1.76 s | 0.65 s |
| Tests | 4 | 13 | 223 | 223 |

A single crate is the more common inner loop and has its own budget (< 5 s):
`cargo nextest run -p narvo-ecs` is 0.48 s on Windows and 0.43 s on Linux.

### d) Doctests

nextest does not run doctests, so they are measured and run separately:

```
cargo test --doc --workspace
```

| Metric | M0 | M0 close | M2.3 (Windows) | M2.3 (Linux) |
| --- | --- | --- | --- | --- |
| Wall clock | 0.12 s | 0.40 s | 2.89 s | 2.38 s |

The jump from 0.12 s to 0.40 s between the first two columns is the whole story of
that row: at M0 the command had nothing to compile and returned almost
immediately, and at M0 close it builds and runs a real doctest. It is the
retroactive proof that the step was wired up correctly all along - it reported
zero for exactly as long as there was nothing to report. The command stays in the
verification set so that the next doctest is executed on the day it lands rather
than sitting unrun until somebody notices.

### e) Full verification set

```
cargo xtask ci
```

Its own budget (§8.1, < 5 min from M3), and worth measuring separately because it
is the command an agent actually runs.

| Metric | M2.3 (Windows) | M2.3 (Linux) |
| --- | --- | --- |
| Incremental, everything warm | 6.15 s | 5.13 s |
| With clippy's fingerprints cold | 32.61 s | not measured |

## Per-commit detail

`--timings` writes an HTML report per build to `target/cargo-timings/`, breaking
the wall clock down per crate and showing where parallelism stalls. The CI
workflow builds the Linux job with `--timings` and uploads that directory as a
`cargo-timings-<sha>` artifact, so the timeline behind any individual commit can
be pulled up without reproducing it locally. Those artifacts expire; this file is
the record that does not.

## The border a padded atlas owes each region (D13, M3.21)

Not a timing. A property, recorded here because the fixtures it constrains are
the ones every other section in this file measures against.

`check_region_padding` in `narvo-render2d` compares every texel of a region's
border against the content texel nearest to it, all four edges and all four
corners, and names the offending texel, its side and both colours. **Three**
atlases carry a border today: `linear_motion.rs`'s `padded_atlas()`, which has
carried one since M3.17 and produced the 48.3760 → 48.0000 figure above, and now
`texture_region.rs`'s and `camera_scene.rs`'s — the fixtures behind
`sprite_atlas_regions_128x128` and `camera_regions_128x128`.

**Both blessed images are byte-identical after the change** — the golden
machinery reports "worst pixel: none, the images are identical" for each, not
merely a deviation inside the tolerance. **M3.17 had already measured this**, not
argued it: `linear_motion.rs`'s `padding_is_free_under_nearest_and_removes_the_bleed_under_linear`
compares the two framebuffers with `assert_eq!(before.rgba(), after.rgba())`
through the production path. What it measured was its own scene; this is the
first time it is measured on a **blessed** one, which is the only part M3.17 had
to leave as an inference.

**Why it cannot move the image, in one line:** a region's edges go from `k / 16`
to `(k + 1 + 2c) / 20` while its span stays 8 texels of the atlas, so `u * 20` is
`u * 16` plus an integer — 1 for the first cell on an axis, 3 for the second, on u and v alike.
`Nearest` rounds down and the fractional part decides where; the fractional part
is untouched. Every probe margin in `texture_region.rs`'s derivation is M3.9's
number unchanged for that reason, the tightest still a twelfth of a texel.

**And the guard is not redundant with the reference image.** Demonstrated: with
one border texel of the top-left cell replaced, `every_region_of_the_atlas_carries_its_border`
fails naming texel (4, 0), the top edge and the source it must copy, while
`the_atlas_scene_matches_its_golden_reference` **passes**. `Nearest` does not
read the border, so the blessed image is blind to it, and it would first appear
as a colour fringe under `Linear`.
