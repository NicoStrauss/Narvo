# ADR-0051: A radiance field is compared byte for byte, and no float multiply feeds an add

Status: accepted · Date: 2026-08 · Scope: `narvo-render2d`
(`shaders/cascade.wgsl`, `cascade.rs`)

## Context

ADR-0050 bought the exact reference regime for the distance field by making it
**integral**, and said so in its own revision condition: it reopens *"at a value
that cannot be an integer, which is M8.5's radiance"*. M8.5a is that value, so
the condition has fired and this ADR is the answer.

M8.3a was explicit that its result did not generalise: *"an integer field agrees
almost for free, that is why integer was chosen; M8.5's radiance is not an
integer, and this task says nothing about it."* The one data point it had on the
general question was its `f32` control, and that control said **no** — one input,
two fields, split by rasteriser family.

So the question M8.5a had to answer, before designing anything around it, is
whether two backends agree on a field of `f32` that nobody could make integral.

## Decision

**A radiance field is compared byte for byte, and the rule that keeps that
possible is: no `f32` multiply may feed an `f32` add.**

In practice `shaders/cascade.wgsl` contains **no `f32` multiplication at all**.
The integration is a sum; the normalisation is a *division* by the direction
count, and there is no fused divide-add for a translator to reach for. That is
why `CascadeStage::new` requires the direction count to be a power of two: it
makes the division exact, so the rule costs nothing in accuracy.

The tolerance regime v1.52 wrote for images — `channel: 4`,
`max_channel_deviation: 24`, `max_differing_ratio: 0.001` — is **not** adopted
here, and the reason is a measurement rather than a preference (below).

## The measurement behind the rule

Five variants of one kernel, otherwise character-identical, on **eight
adapter/backend pairs** in **two profiles**, each cell run **twice** — 160 chain
executions. The chain is the shipping one: `jump_flood.wgsl` and `march.wgsl`
copied verbatim from the tree and `cascade.wgsl` byte-identical to it
(`sha256 5d645529…`). Each run is digested over the whole radiance field, and
each is also compared against an **unfused CPU** computing the same thing in
Rust `f32`, which never fuses on its own.

The pairs are M8.2's eight: AMD RX 9070 XT / Vulkan · AMD integrated / Vulkan ·
AMD RX 9070 XT / DX12 · AMD integrated / DX12 · WARP / DX12 · AMD RX 9070 XT /
GL · llvmpipe / Vulkan (WSL) · llvmpipe / GL (WSL).

| variant | how the sum is written | distinct fields over the eight pairs |
|---|---|---|
| **`add`** — what ships | `sum = sum + r`, then `sum / D` | **1** |
| `mul-pow2` | `sum = sum + r * (1/D)`, no divide | **1**, and the *same* field as `add` |
| `mul-decay` | `sum = sum + r * 0.7` | **2** |
| `fma-decay` | `sum = fma(r, 0.7, sum)` | **2** |
| `add-2p20` | `add`, every emission scaled by 2^20 | **1** |

Windows `dev` and Windows `release` are line-for-line identical over the whole
output; so are WSL `dev` and WSL `release`.

### What each row establishes

- **The shipping form agrees everywhere**, and it agrees with the unfused CPU as
  well — so the agreement is not eight backends making one shared mistake.
- **The contractible form does not.** `mul-decay` returns `65e30818fe8dd514` on
  the five AMD driver paths and `8c02cdb619f6f379` on the three software
  rasterisers (WARP, llvmpipe on Vulkan, llvmpipe on GL). The AMD digest matches
  the CPU's `mul_add`; the software digest matches the CPU's unfused `*` then
  `+`. **The five fuse and the three do not**, which is M8.3a's split by
  rasteriser family arriving in float arithmetic instead of in a comparison.
- **Contractible is not enough — the product has to be inexact.** `mul-pow2` has
  a multiply feeding an add and still returns one field, identical to `add`'s,
  because scaling by a power of two commutes with rounding. That corrects the
  obvious guess and is why the rule is written about the *product*, not about the
  syntax.
- **Magnitude is not the discriminator.** §1 of M8.5a's brief offered "radiance
  lives in a small range where `f32` is exact" as a hypothesis and marked it one.
  It is **refuted as an explanation**: multiplying every emission by 2^20 leaves
  one field. What M8.3a measured was a *comparison* whose branch amplified a
  last-bit difference into a whole coordinate; a sum has no branch, so a last-bit
  difference stays a last-bit difference at any magnitude.
- **`fma()` is not an escape either.** WARP and both llvmpipe backends compute
  WGSL's `fma` builtin **unfused**, disagreeing with the CPU's `mul_add`. Asking
  for the fusion out loud does not get it, so "fuse everywhere, deliberately" is
  not an available option (alternative (c) below).

### How far apart the disagreeing form is

Because the question "how far?" is only answerable where something disagrees,
it is answered on `mul-decay`, comparing the fused and unfused readings of one
expression over 4096 probes:

| unit | worst |
|---|---|
| components differing | **574 of 4096** (14.0 %) |
| ULP distance | **28** |
| relative error | **1.900e-6** |
| absolute error | **1.013e-6** |
| components whose 8-bit quantisation differs | **0** |

The last row is the one that decides a regime and it is computed rather than
estimated: a contracted radiance field, quantised to a 0..=255 channel, is
**identical**. Not one pixel tips.

## The rejected alternatives, at full strength

**(a) A tolerance regime for float fields, with numbers derived from the table
above.** Its best argument is real and is the reason it is written down rather
than dismissed: M8.5b's merge and M8.6's write-back may need a weight that is not
a power of two, and then the rule above cannot be kept. It falls on §2's rule
against building on spec — there is no such consumer today, and a regime built
for a failure that does not occur would be measured against nothing. What the
measurement leaves behind for whoever needs it is the number: a contraction moves
a radiance by at most 28 ULP and 1.9e-6 relative, and by **zero** 8-bit channel
steps. **Reopens** at the first kernel that cannot avoid a float multiply feeding
a float add.

**(b) Reuse v1.52's image regime as it stands.** Best argument: it exists, it is
already trusted, and a second regime is a second thing to maintain. It falls on
the numbers, and it falls in *both* directions at once — which is what makes it
unusable rather than merely imperfect. `max_channel_deviation: 24` is 24/255 =
9.4e-2, roughly **five orders of magnitude** looser than the worst deviation this
field can produce; it would accept almost any wrong answer. And
`max_differing_ratio: 0.001` is **140 times tighter** than the 0.140 the
contracted field actually shows, so it would reject the one case it exists to
tolerate. M8.3a already noted in passing that these numbers were built for 8-bit
channels; this is that note with the measurement behind it.

**(c) Force fusion everywhere by writing `fma()` explicitly**, so that every
backend performs the same single-rounding operation. Best argument: it makes the
operation explicit instead of leaving it to a translator, which is the same
instinct that put the layout of every bind group in this crate in writing rather
than deriving it. **It falls on a measurement, not on taste**: WARP and both
llvmpipe backends compute `fma()` unfused. Asking for one rounding gets two on
three of the eight pairs, and the field splits exactly as it does under
`mul-decay`.

**(d) Store radiance in `Rgba16Float` and halve the memory.** Not a regime
decision and out of scope here, but named because §6's budget makes it tempting:
a half-float rounds at 11 bits instead of 24, so every number in this ADR would
have to be re-measured before it could be believed. **Reopens** with M8.5b's
memory budget, and only together with a repeat of the measurement above.

## The guard, and why its shape follows from the measurement

**A source read, for the same reason ADR-0050's is.** A contracted expression
produces a *plausible* number; no comparison of outputs can report it, because
the two fields differ by 1.9e-6 and agree in every 8-bit channel. The guard is
`cascade::tests::the_kernel_holds_no_float_multiplication`, and it is a literal
against a literal: it holds the exact set of lines in `cascade.wgsl` containing a
`*`, all three of which are integer arithmetic, and additionally refuses `fma(`
and pins the four divisions.

**It has been seen to fall.** Five injections were pre-registered by name before
any was applied, and each fell exactly the tests registered for it. The one that
matters here is J3 — ADR-0049's forbidden shape, an atomic and a barrier added to
the kernel without changing what it computes: **one test failed, and it was the
source guard**, while all eleven GPU oracles and all fourteen of M8.4's march
tests stayed green. That is the demonstration that the guard cannot be replaced
by an output comparison.

## Consequences

- A blessed reference over a radiance field may be compared **byte for byte**,
  across every adapter, backend, platform and profile measured. The exact regime
  M8.3a bought for the distance field survives into M8.5.
- §4(d)'s two readings resolve to the first: the field is deterministic **and
  hashable across machines**, not only within one. A test can only ever say the
  second — it sees one platform — so the cross-machine half rests on the probe
  above and is stated as such wherever it is claimed.
- **The price is named:** a direction count must be a power of two, and a weight
  that is not a power of two cannot be folded into the accumulation. Where such a
  weight is genuinely needed it has to be applied *after* the sum, or (a) has to
  be reopened.
- ADR-0049 is unaffected and unamended: it forbids an order-dependent merge, and
  this kernel obeys it by construction. The two rules are independent — J3
  breaks ADR-0049's while leaving this one intact, and `mul-decay` breaks this
  one while leaving ADR-0049's intact.

## Revision condition

The first kernel in this crate that cannot be written without a float multiply
feeding a float add. M8.5b's merge is the named candidate: if merging a level
into the one below needs an interpolation weight, that weight is a multiply and
its product feeds an add. When that happens, this ADR does not simply relax —
alternative (a) is taken up with the numbers above as its starting point, and the
five-variant measurement is repeated on the kernel that needs it.

A `wgpu` bump is the second condition, for the narrower reason that the split
measured here is a property of naga's output and three translators' treatment of
it, not of the WGSL specification.

## What is not known

- **Why WARP and llvmpipe decline to fuse, and whether they always will.** The
  split is measured on eight pairs on one machine plus one WSL distro; it is not
  a promise about a ninth. The decision does not rest on the split — it rests on
  the shipping form having no expression to fuse — so a fourth backend behaving
  differently would not move it.
- **Whether the agreement survives a much longer sum.** The measurement uses 64
  directions. A level with 32 768 directions performs 512 times as many additions
  and the accumulated rounding grows with it; every operation is still correctly
  rounded and still identical across backends, so byte equality should hold, but
  it is an argument here and not a measurement. M8.5b runs those levels.
- **Whether `f32` is the right precision at all.** Nothing here measures the
  accuracy of the answer, only its reproducibility. A sum of 64 terms loses up to
  63 ULP relative to an exact sum, which was measured (worst 16 ULP over 200 000
  random values at D = 64) and is recorded in the white-chamber oracle's own
  derivation. That is an accuracy question and it is open.
