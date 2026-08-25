# ADR-0052 — A reprojection over continuous motion is a gather and not an interpolation, and the arrears are what make that affordable

- **Status:** accepted
- **Date:** 25.08.2026
- **Milestone:** decided and measured in M8.7; written in M8.8, which §4 of its
  brief made responsible for it
- **Supersedes:** nothing. **Amends:** nothing. ADR-0051 is untouched — see
  "What this does *not* decide".

## Why this is written a task later than it was decided

M8.7's brief told it to report an ADR rather than write one, and it did: its
report's "The ADR case" section names this decision, its three candidates, its
measurement and its revision condition, and says in as many words that one is
due. M8.8 §4 made writing it this task's job. **The decision and every number
below are M8.7's**; nothing here was measured afterwards, and where a number is
quoted it is quoted from the run that produced it.

This is the same arrangement ADR-0050 had — M8.3a measured the decision and
reported it, M8.3b wrote it — and the date above is the writing's, not the
measurement's.

## Context

M8.7 added temporal accumulation: a radiance field is carried from one frame
into the next, reprojected to follow the camera, and blended with the frame just
computed. The reprojection has to answer one question per probe — *where was
this probe last frame?* — and the camera does not move in whole probes. So the
answer is a fractional position, and something has to be done about the
fraction.

Three things could be done, and the choice is not obviously one-sided:

- **(a) nearest neighbour.** Round to a whole probe and read it. A gather: an
  integer index in, a texel out, no arithmetic performed on a radiance value.
- **(b) bilinear.** Read the four probes around the fractional position and
  weight them. Smoother, and the obvious choice if the goal is "follow the
  camera accurately".
- **(c) don't reproject.** Blend against the stored field where it stands. Free,
  and wrong the moment the camera moves at all.

The stake is larger than image quality, because of what M8.5a, M8.5b and
ADR-0051 had already established about this pipeline: a radiance field in this
engine is compared **byte for byte** across machines, and that regime survives
only while no `f32` multiply feeds an `f32` add. A bilinear reprojection is four
fetches and three weighted sums; a temporal blend is `h + (f - h) * a`, which is
that forbidden shape written out. If reprojection killed the exact regime, every
later slice would have to be checked with an image tolerance instead of an
equality.

## Decision

**Nearest neighbour, and `Resample::default()` is `Nearest`. The bilinear arm
is built beside it and kept.**

**And the accumulator carries its arrears**: each frame the offset handed to the
kernel is *this frame's motion plus what the last whole-probe shift could not
take*, `applied_offset` recomputes what the kernel did with it from the same
expression the kernel evaluates, and the remainder is carried into the next
frame.

### The steppiness table, which is the decider

§2 of M8.7's brief asked *how* steppy nearest is, and the arrangement supplies a
ground truth that needs no arguing for: the world is static and only the camera
moves, so a perfect reprojection would return the current frame's field exactly
at every speed. The error against the fresh field is therefore **entirely**
resampling error, with no temporal-lag component to subtract. 96 × 72 probes,
spacing 4, a closed camera loop, errors as a multiple of one probe of spatial
gradient; `sharp` is how much of the true field's own spatial detail survived.

| probes/frame | one in | arm | rms/grad | worst/grad | sharp |
|---|---|---|---|---|---|
| **0.00** | 4 | nearest | **0.0000** | **0.0000** | **1.0000** |
| **0.00** | 4 | bilinear | **0.0000** | **0.0000** | **1.0000** |
| 0.05 | 4 | nearest | 0.0465 | 0.79 | 0.9982 |
| 0.05 | 4 | bilinear | 0.1015 | 3.25 | 0.8792 |
| 0.10 | 16 | nearest | 0.2766 | 11.81 | 0.8254 |
| 0.10 | 16 | bilinear | 0.4247 | 16.20 | **0.5504** |
| 0.50 | 16 | nearest | 0.2713 | 14.92 | 0.8518 |
| 0.50 | 16 | bilinear | 0.6202 | 16.68 | **0.4018** |
| 2.00 | 16 | nearest | 0.5493 | 25.85 | 0.8407 |
| 2.00 | 16 | bilinear | 0.5622 | 15.25 | **0.4419** |

Three things it says, and only the first was expected:

1. **At a motion of nothing both arms are exact** — `0.000e0`, sharpness
   `1.0000`, at both divisors.
2. **Neither arm wins on error.** Nearest costs 0.16–0.67 of a probe-gradient,
   bilinear 0.29–0.62, and the ratio between them swings from 0.53 to 2.49
   across the sweep. Bilinear's *worst* error is consistently the lower of the
   two, because it smooths and an edge misregisters more gently.
3. **Bilinear destroys the field.** After `8 × divisor` frames of resampling its
   own output it retains **0.40 to 0.62** of the true field's spatial detail
   against nearest's **0.75 to 1.00**. A gather cannot blur; an interpolation
   applied to its own output every frame blurs cumulatively.

So the arm that was supposed to be the accurate one is not more accurate, and it
is the one that loses half the picture.

### And it is exact where bilinear is not

Measured over the same eight adapter/backend pairs ADR-0051 used, in both
profiles, at three blend weights:

| divisor | arm | distinct fields over 8 pairs |
|---|---|---|
| 1 | nearest | **1** |
| 1 | bilinear | **1** |
| 4 | nearest | **1** |
| 4 | bilinear | **3** |
| 64 | nearest | **1** |
| 64 | bilinear | **3** |

**The nearest arm returns one field on all eight pairs, at every divisor, in
both profiles.** The divisor-of-one row is the control that isolates the cause:
at `Blend::NONE` the reprojected history is discarded by the blend, and there
bilinear returns one field too — so the divergence is in the **resample**, not
in the blend.

Bilinear's three groups are finer than ADR-0051's two-way split by rasteriser
family: group B is exactly the three software rasterisers, but group C is a
single adapter/backend pair — the discrete AMD part on Vulkan, standing alone
against the same part on DX12 and on GL. That is reported rather than explained.

### The arrears, and the defect that produced them

**The first steppiness run said something worse than "steppy", and reading it is
what produced the design's last piece.** At 0.10 probes a frame with divisor 16
the error was 0.3284 of the field's own RMS; at 1.00 and 2.00 probes a frame it
collapsed to about 1e-5. The collapse at *whole* speeds is the tell: a nearest
reprojection shifts by a whole probe, so a per-frame offset of 0.10 probes
rounds to **nothing, every frame**, and the stored field never moves at all
while the scene slides underneath it. That is not a stair-step; it is a
misregistration that grows without bound.

Carrying the remainder bounds it at **half a probe forever** — the grid snaps by
one probe whenever the arrears reach that, instead of standing still. The same
row falls to 0.1408, and nearest then *beats* bilinear there. `Accumulator::unapplied`
is the public half: a caller can read how far the accumulated field is
misregistered against the frame it is being blended with, in the units it handed
the motion in.

This was a change against M8.7's own pre-registered design note and was marked
as one there. The note said the offset is converted to fixed point on the CPU
and reaches the GPU as an `i32`; that is unchanged. What was added is that the
accumulator *remembers* the sub-probe part, and it was added because a
measurement said so.

## The rejected arm stays in the tree, and says why

`Resample::Bilinear` is kept, the way `MergeForm::Aggregate` is. **A measurement
with one arm deleted is an assertion**: the table above is only checkable while
both arms exist, and "how steppy is nearest" is only answerable against
something that is not. Its own header says it is inexact on purpose and that it
is not the default, and `the_two_arms_agree_on_whole_probes_and_not_on_fractions`
is the in-tree guard that it has not quietly become a copy of the exact one.

Candidate (c), not reprojecting, is **not** kept in any form. It is not a
different trade-off, it is the defect the arrears exist to fix, generalised to
every speed.

## Consequences

### The one this decision exists for: an accumulated field can be compared byte for byte

M8.7's §3 assurances, with which are equalities and which are bounds:

- **(a) A motion of nothing is the identity — an equality.** Derived: at a
  fixed-point offset of zero, `ix = ((x << SHIFT) + HALF) >> SHIFT = x`, so the
  nearest arm reads the probe it is writing. Asserted on the GPU with **no
  tolerance**, on both arms.
- **(b) Accumulating a converged field changes nothing — an equality.** With
  `fresh == reprojected history` the difference is `+0.0`, the division is
  `+0.0`, and `h + 0.0` is `h` for every finite `h` that is not `-0.0`. Eight
  rounds at four divisors, compared against the **starting** field rather than
  the previous round, and the negative-zero edge is asserted rather than argued
  away.
- **(c) A static scene stops changing — a bound, and the residue is exactly
  `divisor / 2` ULP.** In all six measured rows, not approximately: 1, 2, 4, 8,
  32, 128 ULP at divisors 2, 4, 8, 16, 64, 256. It stops *near* the target and
  not at it, because the addition returns `h` once `(f - h) / d` falls below
  half an ulp.

**M8.8 leans on the first of those directly.** Its seam test compares the image
light against the game light through the cascade's own output; that comparison
is only repeatable because an unmoving scene produces an unmoving field.

### What the blend does, and why it is not the problem

`shaders/accumulate.wgsl` contains **no float multiply anywhere**, and
`no_float_multiplication_in_the_exact_arm_at_all` is the literal form of that.
Three things remove ADR-0051's forbidden shape rather than tolerating it: the
blend weight is a reciprocal power of two *by type* (`Blend`) rather than by a
check at the last moment; it is written as a **division**, and no backend has a
fused divide-add; and the nearest reprojection performs no arithmetic on a
radiance value at all.

### The price, named

- **Sub-probe registration is given up.** The field snaps by whole probes and is
  misregistered by up to half a probe at all times. M8.8 measured what that costs
  a consumer: at a probe spacing of 4 texels it is 2.0 texels, which becomes
  **0.309930** of visibility at that arrangement's gradient and **dominates**
  the composed bound at 55.6 % once a camera moves.
- **A moving camera therefore has a wider error budget than a still one**, and a
  test that holds the camera still is not measuring the shipping case. M8.8's
  seam test says so in its own header rather than quietly including the term.

## What this does *not* decide

**ADR-0051 is untouched, and its amendment is still owed elsewhere.** M8.7's §2
anticipated that if reprojection killed the exact regime, ADR-0051's pending
amendment would get the justification M8.6 could not give it. **It does not kill
it** — the exact arm survives on all eight pairs — so this decision supplies
that amendment with nothing and makes it smaller for the second task running.
The amendment remains due for M8.5b's composition split, and M8.7 adds one thing
to it: the three-way bilinear split above, where a single adapter/backend pair
stands alone.

**Nothing here decides how many frames a consumer should accumulate over**, or
what blend weight it should pick. `15 × divisor` frames to convergence is an
empirical law across six divisors (14.5, 16, 16.25, 16, 14.9, 13.6) and is
recorded as one rather than derived.

## Revision condition

**A consumer that needs sub-probe registration more than it needs sharpness.**

At that point the bilinear arm is already in the tree and the change is one
default; what dies with it is the exact regime for whatever uses it, so the
consumer would have to bring an image tolerance of its own and ADR-0051's
byte-for-byte comparison would stop applying to that path. The two halves of
the trade are both measured above — 0.40–0.62 of the field's detail against
half a probe of registration — so the decision can be re-taken on numbers rather
than on preference.

A second, narrower trigger: **a probe spacing small enough that half a probe
stops mattering.** Half a probe is a fixed fraction of the spacing, so a cascade
laid out at spacing 1 has a registration bound of 0.5 texels, below M8.4's `q`,
and the argument above changes on its own without anything being decided.
