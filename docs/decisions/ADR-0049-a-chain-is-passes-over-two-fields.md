# ADR-0049: A chain is n passes over two fields, and a merge may not depend on order

Status: accepted · Date: 2026-08 · Scope: `narvo-render2d` (`field.rs`,
`compute.rs`, `shaders/transport.wgsl`)

## Context

ADR-0039 fixed a frame at **two draw batches in one render pass**, and argued the
number: two, not `n`, because a list of batches would be stock with no consumer.
That decision is about *draws*, and it does not carry the lighting M8.3 to M8.6
are for. Jump flooding needs log2(n) passes; a probe cascade needs one per level,
plus a merge; composition needs its own. None of those is a draw.

M8.2's census, re-counted rather than inherited from M8.0:

| question | answer, and how |
|---|---|
| compute pipelines in the tree | **zero** — grep for `create_compute_pipeline`, `begin_compute_pass`, `ComputePipeline`, `@compute` and `workgroup_size` over every `.rs` and `.wgsl` outside `target/` returned nothing |
| what `request_device` asks for | **nothing** — `..Default::default()` at `gpu.rs:74`, and the probe confirmed it from the other side: `device.features()` came back empty on all eight adapter/backend pairs while `adapter.features()` listed 60-odd |
| a texture with `RENDER_ATTACHMENT` and `TEXTURE_BINDING` together | **still none.** The offscreen resolve target is `RENDER_ATTACHMENT \| COPY_SRC` (`offscreen.rs:384`), its multisample attachment `RENDER_ATTACHMENT` alone (`:402`), the quad's uploaded textures `TEXTURE_BINDING \| COPY_DST` (`quad.rs:546`), and a surface takes `RENDER_ATTACHMENT` plus `COPY_SRC` where offered (`window.rs:238`). M8.0's largest single gap is unmoved |
| `begin_render_pass` per draw path | **one.** `quad.rs:396` (`encode_pass_with`) and `quad.rs:460` (`encode_runs`) are the two entry points, and a draw goes through exactly one of them; `offscreen.rs:610` is the clear-only path |

## Decision

**A chain is `n` compute passes recorded into one encoder, alternating between
two textures of one format. There is no render graph.**

Four pieces, and every one is named beside the task that consumes it — a chain of
reasoning the M8.2 brief required and which is the whole of why this is small:

| piece | consumer |
|---|---|
| `Field` — one `Rgba32Float` texture, storage- and sample-bindable | M8.3's distance field, M8.4's march target, M8.5's cascades, M8.6's albedo |
| `FieldPair` — two of them, `read`/`write`, `swap` between passes | M8.3's log2(n) jump-flooding passes, M8.5's cascade levels |
| `FieldKernel` — one WGSL entry point compiled to a compute pipeline | all four |
| the per-pass `step` in a 16-byte uniform | M8.3's halving jump distance |

Nothing else was added. In particular there is no pass list type, no resource
handle table, no barrier derivation and no aliasing: wgpu already inserts the
barriers between passes that touch one resource, which is the half a graph would
have rebuilt by hand.

**The format is `Rgba32Float` and the usage set is four flags.** Both are
measurements, and they are recorded in `field.rs` beside the constants:

- `Rgba8UnormSrgb` — the offscreen target's own format — reports **no**
  `STORAGE_BINDING` on any of the eight pairs. The field was never going to be
  the format the scene is drawn in.
- `Rg32Float`, the obvious two-channel jump-flooding format, is **refused on both
  GL adapters** and accepted on Vulkan and DX12.
- `Rgba16Float` is accepted everywhere and is half the size; it is not used
  because a half-float is exact only to 2048 and M8.3 stores a pixel coordinate
  per texel.
- `RENDER_ATTACHMENT` is **absent from the usage set**, and that is not tidiness:
  adding it makes `Rgba32Float` *refused* under lavapipe's GL. No consumer needs
  it — M8.3 to M8.6 are compute — so the flag nobody wants is the flag that would
  have made the format machine-dependent.

**And the rule this ADR exists to state: a merge may not be written
order-dependently.**

## The measurement behind the rule

M8.0 excluded atomics, workgroup reductions and `workgroupBarrier` on purpose, so
its numbers measured arithmetic rather than scheduling. "Sum over light sources"
is exactly the excluded shape, and M8.5's cascade merge is too, so M8.2 measured
it: three reductions over 4096 f32, eight repeats each, on eight adapter/backend
pairs, in `dev` and `release` — **32 cells**.

| variant | within-run reproducible | value |
|---|---|---|
| atomic CAS accumulation | **0 of 32** | 4 to 8 distinct sums per 8 dispatches |
| workgroup shared memory + `workgroupBarrier` | **32 of 32** | `0x4b18ac7f` in every cell |
| order-independent control | **32 of 32** | `0x4b18ad59` in every cell |

The pairs were AMD RX 9070 XT and an integrated AMD part over Vulkan and DX12,
WARP over DX12, AMD's GL driver, and lavapipe under WSL over Vulkan and over GL.
The atomic variant failed on the CPU rasterisers too — a prediction registered
before the WSL run said lavapipe might walk invocations in index order and it does
not.

The two reproducible variants returned **one value each across every backend,
adapter, platform and profile**. They return *different* values from each other,
which is correct: different summation trees over non-associative f32.

**The rule for M8.5, in one sentence:** a merge writes each result to a slot the
work item owns and folds nothing into a shared accumulator, so no value ever
depends on which invocation ran first — a barrier-and-shared-memory tree is
permitted because it is order-*independent* by construction, and an atomic
accumulation is not permitted at all.

## The oracle, and why it is shaped this way

A multi-pass path cannot be checked by looking at a picture, because a picture is
what a *wrong* path also produces. So the check is a pattern that no drawing could
have made: two channels holding exact integers of a thousand and two thousand plus
the texel's own coordinate, and two channels holding **negative** values.
`write_texture` puts it in; nothing on the draw path can — the targets are
`…UnormSrgb` and clamp, premultiplied `OVER` never leaves `0.0..=1.0` (ADR-0023),
and a nearest sampler returns a texel rather than inventing one.

The kernel then answers three separate questions in three channels:

- `x = x + x + step` — **order-sensitive**: after n passes it is
  `x0·2ⁿ + Σ sᵢ·2^(n−i)`, so two orderings of the same steps land on different
  numbers, which a sum could not distinguish.
- `y = y + y` — **count-sensitive**: `y0·2ⁿ`. A chain whose passes all read the
  original field rather than the previous pass's output gives `y0·2`.
- `z`, `w` — **must not move.** They catch a pass that invents a texel.

Only `+` appears in the shader — no `*`, no `/`, no `sqrt`, nothing
transcendental — because M8.0 measured `+`, `-` and `*` as the reproducible
subset and measured DX12 contracting 928 of 4096 `a*b+c` expressions inside one
run. The doubling is `value + value` rather than `value * 2.0` for that reason,
and the magnitudes are chosen so every intermediate is an exact integer below
2²⁴, where any contraction a translator might synthesise anyway is exact. A
comparison against a contractable expression measures the translator.

## What was shown red

Three injections, each written before the guard it is aimed at was trusted:

- **the ping-pong stops swapping** — `FieldPair::swap` made a no-op. Three guards
  went red, including `the_two_sides_of_a_pair_are_never_the_same_field` and the
  oracle, which reported `left: 1000.0, right: 8012.0`.
- **a chain one pass short that reports the full count** — the arithmetic guards
  went red (`4004` against `8012`) while **the count assertion stayed green**.
  That is the point of having both: the count alone is not the evidence.
- **the pass counter over-reports** — the count assertion itself went red,
  `left: 4, right: 3`, which is the counting tool shown falling.

## Alternatives

**(a) A general render graph** with declared resources, aliasing and derived
barriers. Best argument, and it is a real one: 3D will need one, and building the
same thing twice costs more than building it once. It falls on having no consumer
that would exercise its generality — every pass here reads one field and writes
one field — so it would be designed against a toy and measured against nothing.
**Reopens at a third consumer with a different pass structure, in practice the
first 3D slice.**

**(b) One field, read and written by the same pass**, with `StorageTextureAccess::ReadWrite`.
Best argument: half the memory, and every adapter measured reports
`STORAGE_READ_WRITE` for this format, so it would work. It falls on §4's
measurement: a pass reading texels its own workgroups may not have written yet is
order-dependent by construction, which is the thing this ADR forbids. **Reopens
never for a chained pass; a pass that only ever touches the texel it owns could
use it, and would have to say so.**

**(c) `Rgba16Float` for the field.** Best argument: half the bytes, filterable
without a device feature, accepted on all eight pairs — so M8.5 could interpolate
a cascade with a linear sampler where `Rgba32Float` needs `FLOAT32_FILTERABLE`.
It falls on M8.3 being next and needing exact coordinates above 2048. **Reopens
if M8.5's merge wants bilinear interpolation badly enough to carry two formats,
which is a decision with a measurement behind it rather than a preference.**

## Consequences for other decisions

**ADR-0039 is amended, not superseded, and the distinction is a measurement of
this tree rather than a reading of the brief.** §5(d) of the M8.2 brief says "a
frame no longer draws two batches; name what takes its place." After M8.2 a frame
still draws exactly two batches in exactly one render pass: nothing in
`SceneHost`, `Windowed` or `Offscreen` records a compute pass, and
`FieldKernel::run` has no production caller at all — the modules carry
`expect(dead_code)` naming M8.3 as the first one. What changed is that the crate
*can* record passes that are not draws. ADR-0039's "two, not `n`" is about the
batch list and is untouched.

The supersession, if it comes, belongs to the task that first puts a compute pass
inside a frame — M8.6 by the plan — and that task will also have to decide
whether the two batches and the chain share one encoder.

**ADR-0040 keeps its meaning and gained a better substrate.** Its axis is that a
screenshot is the presented frame rather than a second render. M8.2 moved the
offscreen read-back out of the draw (`OffscreenTarget::read_back`), so the
headless target is now shaped like the window — draw, then optionally copy — and
the guard that holds ADR-0040's axis was re-run against M8.1's plausible
injection and failed correctly, 1025 of 4096 pixels differing.

**ADR-0008 is untouched.** Nothing here stores an expected value: the oracle
computes its expectation from the seed it wrote, in the same process.

## What is not known

- Whether two *backends* compute the same float field on one machine. The
  reduction measurement says they agree on an order-independent sum of 4096
  values; it says nothing about a jump-flooding pass. ADR-0048 records the same
  gap from the adapter side.
- What a chain costs. Nothing here is benchmarked, because a transport kernel's
  cost is not a lighting pass's and a number measured on it would be quoted later
  as though it were.
