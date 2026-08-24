# ADR-0048: The offscreen path names its adapter, and stops at wgpu's first tier

Status: accepted · Date: 2026-08 · Scope: `narvo-render2d` (`offscreen.rs`,
`gpu.rs`)

## Context

ADR-0046 settled the **window's** backend and said so in its title: "the window
picks its own backend, and the references keep theirs." It deliberately left the
offscreen instance untouched, because that instance draws all twelve blessed
references and moving them was the cost it was avoiding.

That left the offscreen side decided by nobody. Two facts compose:

- **`select_adapter` compares nothing.** `LADDER` (`gpu.rs:16-28`) tries three
  rungs — high performance, no preference, forced software fallback — and
  `return`s the first that answers. No vendor id, no device id and no adapter
  name appears anywhere in this crate outside a `format!`. Whichever adapter the
  driver stack offers first for a given power preference is the one that draws.
- **`offscreen_backends` returned `Backends::default()`**, which is
  `Backends::all()` — `impl Default for Backends` is one line and returns exactly
  that (`wgpu-types-30.0.0/src/backend.rs:140-144`). `all()` contains
  `Backends::SECONDARY`, and `SECONDARY` is precisely `GL`, described in that
  same file as "the apis that wgpu offers second tier of support for. These may
  be unsupported/still experimental" (`:132-136`).

Neither has cost anything so far. Every reference this repository compares is an
8-bit-per-channel image and `docs/perf/BASELINE.md` records three rasterisers
producing the M1 reference with `worst channel deviation 0`. A picture two
rasterisers agree on to zero counts does not care much which one drew it.

**M8.3 changes that.** A global-illumination reference is not a byte image; it is
a field of `f32`. M8.2's own measurement is what makes the concern concrete
rather than anticipated — see below.

## Decision

Two changes, and deliberately not a third.

**1. `offscreen_backends()` returns `wgpu::Backends::PRIMARY`.**

GL leaves the set. `PRIMARY` is `VULKAN | METAL | DX12 | BROWSER_WEBGPU`;
`default()` was `NOOP | VULKAN | GL | METAL | DX12 | BROWSER_WEBGPU`, printed by
the guard that had to be updated. `WGPU_BACKEND` still overrides it, through
`InstanceDescriptor::with_env` at the call site, which is the escape hatch
ADR-0046 named for the window and which now serves both sides.

**2. `gpu::summarize` carries the adapter's numeric identity.**

    AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu, 0x1002:0x7550] chosen by: high-performance

was

    AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu] chosen by: high-performance

`vendor` and `device` are the identifiers `AdapterInfo` reports. A name is a
string a driver may localise or reword between releases; a vendor/device pair is
not. **Twenty-eight call sites read this summary already** — counted, not
estimated: `grep -rn "adapter_summary()"` over `crates/` and `tools/`, minus the
definition. Four of them are production (`screenshot.rs:97`,
`blessed_scenes.rs:409`, `window.rs:409`, and `window.rs:887`, which puts it in a
`RunReport`), one is a pass-through accessor (`frame.rs:399`), and the rest print
it from a test. Every one gained the identity without a line changing.

Five of those twenty-eight do more than print it: the margin test files compare
it against a summary they build themselves, to prove their own pipeline and the
production one landed on the same adapter. That comparison is what caught this
change — ten tests went red on the format alone — and those five files were
brought back into step with production, in the format *and* in the backend set,
since a wider set there would let them land where production cannot go.

**3. What was deliberately not done: no selection by name or id.**

The ladder still takes the first rung that answers. Choosing an adapter by
comparing vendor and device against a list would be a policy this repository
cannot state — the CI runners, the two local platforms and any future machine
have different hardware, and a list that has to be right on all of them is a list
that will be wrong on the next one. What §5(a) of the M8.2 brief actually calls
an error from the first GI reference onward is a run that **cannot say** what
produced it. Point 2 fixes that; point 1 removes the one alternative measured to
behave differently. Selecting *for* the user is a third thing and has no consumer.

## What it is worth, measured

The M8.2 probe built a `texture_storage_2d<…, write>` compute pipeline and a
texture for twelve candidate formats, on **eight adapter/backend pairs**: AMD RX
9070 XT and an integrated AMD part over Vulkan and DX12, WARP over DX12, AMD's
OpenGL driver on Windows, and lavapipe under WSL over Vulkan and over GL.

GL was the only backend that disagreed with the others **on the same machine**:

| format | Vulkan / DX12 | GL (Windows, AMD) | GL (WSL, lavapipe) |
|---|---|---|---|
| `Rg32Float` storage | accepted | **refused** | **refused** |
| `Rgba32Float` storage | accepted | accepted | accepted |
| `Rgba32Float` + `RENDER_ATTACHMENT` | accepted | accepted | **refused** |

The refusals are validation errors at pipeline or texture creation, not silent
differences — so today they would be a crash rather than a wrong picture. That is
the better failure of the two, and it is still a failure that depends on which
adapter answered first.

## What it costs, also measured

**Nothing today, and that was measured before the change rather than argued
after it.** `narvo --screenshot` prints the adapter it used:

| | before | after |
|---|---|---|
| Windows | `AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu]` | same adapter, plus `0x1002:0x7550` |
| WSL | `llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu]` | same adapter |

The 256 x 256 screenshot is **byte-identical across the change**,
`sha256 f598a3a2…`. GL was reachable, not chosen; the first rung asks for a
high-performance adapter and Vulkan answered on both platforms.

The real cost is at the edge: a machine that offers **only** GL now gets
`RenderError::NoAdapter` naming the three rungs it tried, where before it would
have rendered. That is the same trade ADR-0046 made for the window and for the
same reason — a loud absence beats a quiet substitution.

## Alternatives

**(a) Leave it alone.** Best argument: ADR-0046 explicitly chose not to touch
this instance, and the twelve references are the most expensive thing in the
repository to move. It falls on the measurement above: the reason ADR-0046 gave
was that moving the window's backend would move the references, and this change
was measured *not* to move them — same adapter, byte-identical screenshot.
Reopens if a supported platform turns out to offer GL and nothing else.

**(b) Restrict harder — Vulkan only, or DX12 only, per platform.** Best argument:
it would make the offscreen substrate one named thing per platform instead of a
race between two first-tier backends. It falls on CI: the Windows runner's
adapter mix is not this machine's, and pinning a backend that a runner may not
offer trades a measured problem for an unmeasured one. Reopens if a GI reference
turns out to differ between Vulkan and DX12 on one machine — which is a
measurement M8.3 can make and this task did not.

**(c) Select the adapter by vendor and device id.** Best argument: it is the only
thing that literally pins *the adapter* rather than the set it is drawn from. It
falls on having no correct list, per Decision 3.

## Consequences for other decisions

ADR-0046 stands unchanged. Its table of two instances is still the fact this
rests on; what changes is only that both entries are now a named choice instead
of one named and one inherited. The sentence in `offscreen_backends`' doc — "M7.1d
moved the window off Vulkan on Windows and left this exactly where it was, which
is the entire reason not one of the twelve moved" — is still true and is now
history rather than the current state, and it says so.

## What is not known

Whether Vulkan and DX12 compute the same float field on one machine. Nothing here
measures it, because nothing here produces a float field a reference is taken
from. M8.3 is the first task that can, and the release-determinism suite's shape
— two runs from one commit, no stored expected value (ADR-0008) — is the shape
that answer wants.
