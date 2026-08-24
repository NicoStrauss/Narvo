# ADR-0050: A comparison over coordinates is written in integer arithmetic

Status: accepted · Date: 2026-08 · Scope: `narvo-render2d`
(`shaders/jump_flood.wgsl`, `sdf.rs`)

## Context

M8.3a needed a rule for how jump flooding compares two candidate seeds. The
obvious expression is `dx*dx + dy*dy`, and it is exactly the contractable shape
M8.0 measured: WGSL cannot forbid contraction, and DX12 fused **928 of 4096**
such expressions inside one run.

M8.2 had already met this and defended `f32` with **magnitude** rather than
syntax — every value it put through the transport kernel is an exact integer far
below 2^24, where a doubling and an addition are exact in either form, so
contraction is unobservable by construction. That defence is correct, and M8.2's
own sizes are inside it.

The question M8.3a had to answer is whether it stays correct at the sizes a
*field* can be. A `Field` is capped by `OffscreenTarget::MAX_DIMENSION`, which is
8192, and a squared distance across such a field reaches `2 · 8191²` ≈ 1.34e8 —
far outside the range where an `f32` holds every integer.

**M8.3a reported this decision rather than writing it**, on its brief's
instruction to report when an ADR would be due. This ADR holds the decision; it
does not take it. Everything below was measured before M8.3a landed and is
recorded in `target/reports/M8.3a-Distanzfeld.md`.

## Decision

**A comparison over coordinates is written in integer arithmetic.**

Concretely, in `shaders/jump_flood.wgsl`: seed coordinates are loaded from the
field and converted to `i32`, the squared distance is `dx * dx + dy * dy` over
`i32`, and the accumulator is `var best_d: i32`. The tie-break is a total order
over `(d, seed_row, seed_column)`, all integers.

The scope is the comparison, not the storage. The field stays `Rgba32Float`
(ADR-0049 fixed that format on eight adapter/backend pairs), and the coordinates
ride in `f32` channels, where every integer below 2^24 is exact and 8192 is a
long way below it. **What is forbidden is doing arithmetic on them as floats.**

`*` appears in the kernel and that is not a contradiction of ADR-0049's rule for
the transport kernel. M8.0 measured contraction of `a*b + c` in **`f32`**; there
is no fused multiply-add over integers and no rounding to fuse.

## The measurement behind the rule

Two kernels were built. They differ in three lines — `f32(sx - x)` instead of
`sx - x`, and an `f32` accumulator — and are otherwise character-identical, so a
difference in their outputs is a difference in the arithmetic and nothing else.
Seven seed arrangements × 8 adapter/backend pairs × 2 profiles × 2 runs =
**448 chain executions**.

**Six of seven arrangements are byte-identical under both variants.** At 128 and
1024 texels a side every squared distance is far below 2^24, and the two forms
cannot be told apart. M8.2's magnitude defence holds there, which is the point of
saying it.

The seventh is a field 8192 texels wide — precisely `MAX_DIMENSION` — with two
seeds one row apart at (0, 0) and (0, 1). For a texel (x, 1) the distance to
(0, 1) is `x²` and to (0, 0) is `x² + 1`, so the nearer seed is the one the
tie-break disfavours. As soon as `f32` cannot tell `x²` from `x² + 1`, the wrong
seed wins.

| variant | texels naming the farther seed, of 16 384 | first wrong x |
|---|---|---|
| **`i32`, all eight pairs** | **0** | none |
| `f32`, AMD RX 9070 XT / Vulkan | 3 248 | 4096 |
| `f32`, AMD integrated / Vulkan | 3 248 | 4096 |
| `f32`, AMD RX 9070 XT / DX12 | 3 248 | 4096 |
| `f32`, AMD integrated / DX12 | 3 248 | 4096 |
| `f32`, AMD RX 9070 XT / GL | 3 248 | 4096 |
| `f32`, WARP / DX12 | **4 096** | 4096 |
| `f32`, llvmpipe / Vulkan (WSL) | **4 096** | 4096 |
| `f32`, llvmpipe / GL (WSL) | **4 096** | 4096 |

Two things fall out, and **the second is the one that decides**:

1. `f32` is simply wrong over half the widths a field can legally have. The first
   wrong texel is at x = 4096, which is where `x²` reaches 2^24 — a derived bound
   and then a measured one, agreeing.
2. `f32` is wrong **differently on different machines**: 3 248 on the three AMD
   paths against 4 096 on the three software rasterisers. One input, two fields,
   and the split is by rasteriser family rather than by backend — AMD's GL agrees
   with AMD's Vulkan and DX12, not with llvmpipe's GL.

The `i32` variant returned **one field in 16 of 16 cells** on all seven
arrangements. Point 2 is what makes this a rule rather than a preference: `f32`
does not merely round, it rounds differently per translator, which is M8.0's
contraction finding arriving in a second place. Integer arithmetic has no such
freedom to exercise.

**The obvious explanation is contraction and it was not tested.** `fma(dx, dx,
dy*dy)` rounds once where the unfused form rounds twice, which would explain the
hardware/software split. What was measured is that the answers differ, not why.

## The rejected alternative, at full strength

**`f32` with M8.2's magnitude argument.** Its case is stronger than a summary
suggests, and it is not the case that lost:

- It is **correct** at every size M8.2 used, and correct at every size M8.3a's
  own tests use. Six of seven arrangements proved it by producing identical
  bytes.
- It costs nothing to write and reads more naturally than an integer round trip
  through `i32(probe.x)`.
- The bound is computable in advance, so a project that knew its field sizes
  could adopt it deliberately.

**What it fails is the bound, not the reasoning.** The disagreement is about
where the magnitude argument stops being true, and the answer is `MAX_DIMENSION`
— a limit this crate publishes and a caller may use. A rule that holds for the
sizes we happen to test and breaks at the size we advertise is not a rule.

Two things it is worth being clear about, because they cut *for* the rejected
alternative:

- M8.2's use of it was never wrong and is not retracted. `transport.wgsl` still
  computes in `f32`, and this ADR does not ask it to change: its values are
  bounded by its tests, and it is an oracle rather than a production path.
- The failure is **representability**, not contraction. The magnitude defence
  would have failed here even on a translator that fused nothing.

## The guard, and why its shape follows from a measurement

**The guard has to be a source read**, and that is forced rather than chosen: a
comparison of outputs cannot see the difference below the magnitude at which
`f32` stops being exact, and *six of the seven arrangements measured exactly
that* — byte-identical fields under both variants. The one arrangement that
separates them needs a field 8192 texels wide, which no unit test allocates.

So `sdf.rs`'s `the_kernel_compares_in_integer_arithmetic` reads the shader's
source and asserts that `var best_d: i32 = 0;` is declared, that the squared
distance is computed from integer coordinates in one expression, that
`f32(dx)` and `f32(dy)` appear nowhere, and that no root, power or transcendental
is present. It is a literal against a literal, worth exactly one thing: a session
that moves this arithmetic has to move that line too, and then has to say in the
commit message which adapters it re-measured.

It was **shown red in M8.3b** against an injection that turns the comparison back
into `f32` — the arrangement this ADR rejects. The result is recorded in
`target/reports/M8.3b-Verdecker-als-Weltzustand.md`.

## Consequences

- **The reference regime for M8.3 through M8.6 stays exact for this field.** A
  blessed reference over an integer jump-flooding field is byte-comparable across
  every adapter, backend, platform and profile measured, so v1.52's tolerance
  regime is not needed for it.
- **That result is largely what this decision bought**, and it should not be read
  as evidence about floats in general. The field is exact integers by design; the
  one data point about arbitrary floats is the rejected `f32` variant, and it says
  backends disagree.
- ADR-0049's rule (a merge may not be written order-dependently) is unaffected and
  independent: this is about *what* is compared, that one is about *who* compares
  it in what order.

## Revision condition

**A value that cannot be an integer.** M8.5's radiance is the named case: a
cascade stores light, light is not a coordinate, and no amount of care makes it
integral. At that point this rule has nothing to say and the question M8.3a's §4
left open becomes the live one — whether two backends agree on a float field, and
if not, what tolerance regime covers it. M8.5 owns that, and this ADR should be
read then rather than extended by analogy.

A second, weaker condition: if `MAX_DIMENSION` ever fell below 4096, the
magnitude argument would cover the whole legal range again and the rule would be
carrying nothing. That is not expected and is written down so the reasoning stays
checkable rather than becoming a habit.

## What is not known

- **Why the hardware and software rasterisers differ.** Contraction is the
  obvious explanation and was not tested; only the disagreement was measured.
- **Whether the 3 248 / 4 096 split is stable across driver versions.** It was
  measured once, on one machine, on 2026-08-24.
- **Whether an integer field is byte-identical on adapters this project has never
  seen.** Eight adapter/backend pairs agreed. That is eight, not all.
