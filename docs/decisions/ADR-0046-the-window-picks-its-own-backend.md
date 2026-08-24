# ADR-0046: The window picks its own backend, and the references keep theirs

Status: accepted · Date: 2026-08 · Scope: `narvo-render2d` (`window.rs`,
`offscreen.rs`), Windows only

## Context

M7.1c localised a defect and did not fix it: on Windows over the Vulkan backend,
reconfiguring the swapchain after a window resize blocks the next few calls to
`WindowTarget::begin_frame` for about two seconds each. With
`desired_maximum_frame_latency: 2` the swapchain holds three images, so one
resize costs about six seconds of frozen window. It is B-5 on the human's
finding list and it is the reason `games/forge-loop` is unpleasant to drag.

M7.1c weighed a fix — "Windows default to DX12" — and rejected it with a
sentence that this ADR exists because it was **wrong**:

> Er verlegt jedes Bild, das dieses Repository je vergleicht, auf ein anderes
> Substrat — die zwölf gesegneten Referenzen eingeschlossen.

That was an assumption about the tree, stated in the tone of a measurement, and
it is the fourth instance of the class `CLAUDE.md` opens with. Narvo does not
have one `wgpu::Instance`. It has **two**, and they were already separate before
this task:

| Instance | Site | Display handle | Draws |
|---|---|---|---|
| offscreen | `offscreen.rs:333` | no | every blessed reference |
| window | `window.rs:325` | yes | the window, and nothing a test compares |

Five further instances exist in test files, all without a display handle, all of
the offscreen kind. **Nothing in the workspace builds a `WindowTarget`** — M7.1c
established that separately, and it is why the two sides never met.

So the choice was never global. It is per instance, and the instance that has the
defect is not the instance that produces the images.

## Decision

**The window's instance asks for DX12 on Windows and for wgpu's default set
everywhere else. The offscreen instance is left exactly as it was.**

Both are named functions — `window::window_backends` and
`offscreen::offscreen_backends` — so that the two choices are two things a test
can hold apart, and both descriptors still run through `InstanceDescriptor::with_env`
**after** the set is chosen. `Backends::with_env` replaces the set outright when
`WGPU_BACKEND` is present (`wgpu-types-30.0.0/src/backend.rs:161-168`), so the
environment still wins, `WGPU_BACKEND=vulkan` remains the escape, and the
comparison between the two backends stays measurable from a command line without
a rebuild. That last property is not a convenience: it is how every number below
was taken.

**No silent fallback.** A Windows machine with no DX12 adapter gets
`RenderError::NoAdapter` naming the three rungs it tried, not a quiet Vulkan
window. This whole line of work exists because a backend nobody had named was
doing something nobody had measured; a fallback that picks a different one
without saying so would rebuild exactly that.

## What it is worth, measured

`narvo --frames 900 --resize-probe`, one machine (RX 9070 XT, driver
32.0.22042.14002, display 2560×1440 at 60 Hz), one binary, five resizes each
confirmed to have reached the surface:

| | blocked frames | `acquire` p50 | max |
|---|---|---|---|
| `WGPU_BACKEND=vulkan` | **15** | 3.255 ms | **2014.806 ms** |
| DX12 (the new default) | **0** | 3.4 ms | **20.4 ms** |

And on the game, through M7.1b's own external driver, seven resizes including a
maximise to 2560×1369:

| | blocked frames | worst `acquire` in 1 400 frames |
|---|---|---|
| before | 3 | **2012.763 ms** (6030.4 ms on one resize) |
| after | **0** | **33.107 ms** |

The residual is 33.107 ms, which is two vsync intervals at 60 Hz — one dropped
frame. Seven frames of 1 400 exceeded 20 ms. **Not "fixed": from about six
seconds to about one dropped frame.**

## What it costs, also measured

DX12 has a failure mode Vulkan did not show here. Of **24** DX12 runs of that
command, **two** ended in a wgpu validation panic:

```
wgpu error: Validation Error
Caused by:
  In Surface::get_current_texture_view
    Surface is not configured for presentation
```

Both fell in one early batch; twenty-two later runs were clean, including four
deliberately alternated with Vulkan runs to test whether switching backends was
the trigger. **It was not, and what the trigger is stays unknown.** A plausible
reading that was *not* confirmed: `WindowTarget::resize` calls `surface.configure`
and never asks whether it worked, so a rejected configuration would leave exactly
this state behind for the next `get_current_texture`.

> **Amended in M7.9 — see *The reading, traced* below.** The sentence above is
> half right and half impossible, and the half that is impossible is the word
> "asks": `Surface::configure` returns `()`. The paragraph is left standing as
> it was written, because what it got right is the part that matters.

This is a trade and it is written down as one: a certain six-second freeze on
every resize, against a crash observed twice in twenty-four runs of a probe that
resizes five times in fifteen seconds — far harder than a hand. The escape from
either is `WGPU_BACKEND`.

## The reading, traced (M7.9)

M7.9 was asked to check the plausible reading above rather than act on it. It
was traced through the pinned sources — `wgpu` 30.0.0 and `wgpu-core` 30.0.0,
the versions `Cargo.lock` names — and it comes apart into three findings.

**The state it describes is real, and the mechanism is exactly the one it
guesses.** `Device::configure_surface` takes the surface's presentation state
*before* it attempts the reconfigure and only puts it back on success:

- `wgpu-core-30.0.0/src/device/resource.rs:5368` — `surface.presentation.lock().take()`
- `…:5381` — `surface_raw.configure(...)`, and every error arm `break 'error`
- `…:5399-5404` — `*presentation = Some(...)`, reached only when no arm broke

So a rejected configuration does leave the surface with `presentation == None`,
and `Surface::get_current_texture` answers exactly one way to that:
`return Err(SurfaceError::NotConfigured)` (`wgpu-core-30.0.0/src/present.rs:172`),
whose `Display` is the string in the panic above
(`…/src/present.rs:46`). That much of the reading holds.

**"Never asks whether it worked" is not a thing `resize` could do differently.**
`Surface::configure` returns `()` — `wgpu-30.0.0/src/api/surface.rs:119`. There
is no result to inspect, so the omission the sentence names is not an omission.
What the backend does instead is route the error to the device's error sink
(`wgpu-30.0.0/src/backend/wgpu_core.rs:3978-3985`), and with no error scope
pushed and no custom handler installed that sink **panics**:
`panic!("wgpu error: {err}
")` in `default_error_handler`
(`…/backend/wgpu_core.rs:693-694`).

**And that is what refutes the reading as an explanation of these two crashes.**
Had the `configure` inside `resize` failed, the process would have panicked *at
the configure*, in `Surface::configure`. The panic that was actually observed
names `Surface::get_current_texture_view`. Every `ConfigureSurfaceError` variant
maps to `ErrorType::Validation` (`…/present.rs:134-152`) and therefore to that
panic — every variant but one. `ConfigureSurfaceError::Device(DeviceError::Lost)`
maps to `ErrorType::DeviceLost`, and `DeviceLost` is the single arm of the error
sink that **returns without panicking**, deferring to the device-lost callback
(`…/backend/wgpu_core.rs:304`).

So the reading survives in a much narrower form than it was written: **a device
loss during a reconfigure is the one path that fails silently and leaves the
state the panic reports.** A validation-shaped rejection is not, because it
would have said so, loudly, one call earlier.

### No repair was built, deliberately

The trigger is still unknown and the crash is still unreproduced — twice in
twenty-four runs, never since, never in a human's hands. A change made on the
strength of a trace is a guess with a diff attached, and the trace here argues
*against* the fix the sentence implies rather than for it: there is no return
value to check, and the one silent path is a device loss, which an error scope
around `configure` would not catch either.

What would make it measurable later, in the order it should be tried:

1. Install a `Device::on_uncaptured_error` handler in `WindowTarget::new`. That
   turns every non-device-lost surface error into something the process can log
   and name instead of a panic message the reader has to parse. Cheap, and it
   costs no behaviour when nothing fails.
2. Read the device-lost callback. It is the one channel the silent path uses,
   and this repository installs nothing on it today.
3. Only then, and only with a reproduction, consider anything in `resize`.

One reachable validation variant *is* worth naming while this is open, because
it is checkable rather than speculative: `ConfigureSurfaceError::TooLarge`
(`…/present.rs:88`) fires when a requested size exceeds
`max_texture_dimension_2d`. `WindowTarget::resize` guards zero
(`crates/narvo-render2d/src/window.rs:503`) and does not guard the maximum, so
a sufficiently large window panics. That is a finding, not a fix, and it is
reported rather than taken for the same reason as the rest.

## What the guards are, and what they are not

Three tests, and the third exists because the first two are not enough:

1. `window_backends()` is `Backends::DX12` on Windows, the default elsewhere.
2. `offscreen_backends()` is wgpu's default and, on Windows, differs from the
   window's.
3. `offscreen.rs`'s **call site** still reads `descriptor.backends =
   offscreen_backends();` and does not name `window_backends` in code.

(3) is the one that matters. Comparing two constants is blind to the leak that
actually threatens the references — one edited line in another file — and that
leak is invisible to every image comparison in the repository, because both sides
of a golden comparison would move together. Measured: routing the offscreen call
site through `window_backends()` turns **11 of 1371** tests red, and the
call-site guard is among them by name.

The ten others are worth naming, because they correct a fear rather than confirm
it: they are the **margin** tests (`draw_order_margin`, `linear_blessed_margin`,
`msaa_blessed_margin`, `linear_motion`, `linear_production`), which hold
production output against a CPU model at a tighter tolerance than the golden
comparison uses. Not one of the twelve `matches_its_golden_reference` tests
failed. So the blessed stock alone would **not** have caught a substrate move,
and the margin tests would — which is consistent with ADR-0023's own
cross-rasteriser table, where the stock holds across AMD Vulkan, WARP Dx12 and
lavapipe.

**What nothing guards is the runtime choice.** With `WGPU_BACKEND=vulkan` the
window measurably goes back to blocking 2005 ms per resize and the whole set
reports **1371 of 1371 green**. The backend is *reported* — every windowed run
prints `adapter: … [Dx12, DiscreteGpu]` — and it is checked by nobody. That is
recorded here rather than fixed: a test that asserts which backend a window got
needs a window, and nothing in this workspace has one.

## Alternatives

- **(b) Switch the default backend globally.** M7.1c's candidate. Rejected
  because it is strictly larger than (f) for the same benefit: it moves the
  offscreen instance too, and that instance is the substrate of all twelve
  references. Its best argument is simplicity — one choice instead of two named
  ones — and it is not worth the reach. **Reopen if** the two instances ever
  merge, at which point this decision becomes (b) whether or not anybody says so.
- **(a) `desired_maximum_frame_latency: 1`.** Measured in M7.1c: three blocked
  frames become two, so six seconds become four. Rejected: it pays a frame of
  pipelining in *every* run, including every run that never resizes, to remove a
  third of a defect. **Reopen if** the DX12 route is withdrawn.
- **(c) Raise `wgpu`.** The Windows-only wait was *added* deliberately
  (`wgpu-hal-30.0.0/src/vulkan/swapchain/native.rs:494`, citing wgpu#8310 and
  #8354), so there is no reason to expect a later version to drop it. It also
  moves `Cargo.lock`, which fires `release-determinism.yml`. **Reopen if** an
  upstream issue reports the block.
- **(d) Debounce resizes.** Reduces how often the cost is paid and never removes
  it. Rejected.
- **A silent DX12→Vulkan fallback.** Rejected above, and rejected for the reason
  the task exists.

## Consequences for other decisions

- **ADR-0040** — a screenshot is the presented frame. Its surface-capability and
  format-list measurements were taken on **Windows/AMD Vulkan**, which is no
  longer what a Windows window runs on. Nothing in its *decision* moves — the
  copy still sits between encode and hand-over, and `rgba_from` still normalises
  whatever format the surface offers, which is exactly the mechanism that makes a
  format-list change a non-event. What is now **unmeasured** is the DX12 format
  list on this machine. Marked there.
- **ADR-0004** — orientation. Untouched in substance: no y-flip moves, and the
  conventions are stated in terms of NDC and image row order rather than of a
  backend. But it has never been checked *on this backend for the window*, and
  the window is the one path with no automated coverage. Marked there.
- **ADR-0023** — premultiplied blending. Its measurement already spans three
  rasterisers including WARP over Dx12, and the blessed stock held on all three,
  which is the closest thing to prior evidence that this change is safe for
  colour. It is prior evidence and not this task's measurement. Marked there.
- **ADR-0013** — cross-platform determinism. **Not** affected, and this was
  checked rather than assumed: the cross-platform comparison is over `.dump`
  files, which are canonical dumps of simulation state produced by
  `narvo --headless --dump` (`xtask/src/determinism.rs`). No pixel and no GPU
  enters it, and no window is opened. `Cargo.toml` and `Cargo.lock` are unmoved,
  so `release-determinism.yml` does not fire.

## What is not known

- **The trigger for the two DX12 panics.** Twenty-two clean runs afterwards.
- **Whether the Vulkan block is bistable.** It is: within one hour on one
  machine, six consecutive runs showed none of it and every run after a maximise
  showed all of it, in that process and in later ones. What flips it is unknown;
  no reboot and no driver change happened in between. This also corrects M7.1c's
  claim that a four-pixel resize costs what a maximise costs — the two are not
  always alike.
- **Other GPUs, other drivers, other Windows versions.** The sample is one card.
- **What CI sees.** Nothing: `ci.yml`'s Windows job sets no `WGPU_BACKEND` and
  builds no window, so this decision is exercised by no automated run anywhere.

  **M7.9 measured whether that has to stay true, and on Linux it does not.**
  `narvo --frames 30` was run under `xvfb-run -s "-screen 0 1280x720x24"` on a
  machine with no GPU, with the loader pinned to the same lavapipe ICD `ci.yml`
  already installs: a real window opened, the adapter came back as
  `llvmpipe (LLVM 20.1.2, 256 bits) [Vulkan, Cpu]`, **thirty of thirty frames
  were drawn** and the process exited 0. So the premise D29 was blocked on —
  that a GPU-less runner cannot present through a window — is false for the
  Linux job, and the missing ingredient is one apt package.

  Two things that measurement does **not** say, and both matter. It was taken
  under WSL rather than on an `ubuntu-latest` runner, so it is a proxy; and it
  says nothing at all about the Windows job, which is the one this ADR is about.
  A guard built on it would exercise ADR-0040's copy path, not this decision's
  backend choice — those are different claims on different platforms.
