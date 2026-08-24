# ADR-0040 — A screenshot is the frame that was presented, and the surface says whether it may be copied

Status: accepted · M6.4b · 14.08.2026

## Context

M6.4b was written as "screenshot over MCP" and could not be built that way: the
windowed runner has no IPC seam, and v1.04 booked that adding one added nothing
and cost a second observation moment. What M6.6 unlocked was the *window* path —
the registry, the text path, the second batch, the inspector — so the task was
re-cut as **a human-side screenshot behind a key**, the same shift M6.6 took.
The survey did not refute that re-cut; §7 of the task forbids the IPC half
outright, and nothing measured here argues for it.

That leaves one real question, and the task hung on it: **a window draws into a
surface texture, and a texture is only copyable if its usage flags allow it.**

Measured, not assumed:

- `window.rs` configured the surface with `usage:
  wgpu::TextureUsages::RENDER_ATTACHMENT` and nothing else, so **no frame could
  be copied out of a window at all**.
- Within `crates/*/src/`, `copy_texture_to_buffer` and `map_async` occur only in
  `narvo-render2d/src/offscreen.rs`. M6.6a's finding holds for the source path;
  five files under `crates/narvo-render2d/tests/` have their own copies, which
  that phrasing did not cover.
- `SurfaceCapabilities::usages` guarantees exactly one flag: "The usage
  [`TextureUsages::RENDER_ATTACHMENT`] is guaranteed"
  (`wgpu-types-30.0.0/src/surface.rs:530-533`). `COPY_SRC` is **not**
  guaranteed, and asking for an unoffered usage is a validation error at
  `surface.configure` — which would cost the window, not the screenshot.
- Both platforms this project verifies on do offer it. A probe in
  `with_present_policy`, run once per platform and reverted from a SHA-checked
  copy, printed on each of them:
  `COPY_SRC | COPY_DST | TEXTURE_BINDING | STORAGE_BINDING | RENDER_ATTACHMENT |
  STORAGE_ATOMIC`.
- **[Amended by ADR-0046, M7.1d.]** Both bullets above and the one below were
  measured on **Windows/AMD Vulkan**, which since M7.1d is no longer the backend
  a Windows *window* runs on. The decision does not move: the copy still sits
  between the encode and the hand-over, and `rgba_from` still normalises whatever
  format the surface offers — which is the mechanism that makes a changed format
  list a non-event rather than a problem. What is now **unmeasured** is the DX12
  format list and usage set on that machine.
- The same probe measured something the task had not asked about and that
  decides the shape of the read-back: the surface's **format list differs
  between the two platforms**. Windows/AMD Vulkan offers
  `[Rgba8UnormSrgb, Bgra8UnormSrgb, Rgba8Unorm, Bgra8Unorm, Rgba16Float,
  Rgb10a2Unorm]`; WSL/lavapipe offers `[Bgra8UnormSrgb, Bgra8Unorm]` and no RGBA
  format at all. `choose_format` therefore lands on RGBA here and on BGRA there.

Three alternatives were weighed against adding the flag:

- **Render the frame a second time into an offscreen texture.** No surface
  change, but the picture would then agree with the window only by an argument
  about the inputs being equal, never by being the same bytes. It also needs a
  second target inside `WindowTarget` and a second encode of the same draw —
  closer to what `lib.rs` warns about ("the window cannot become a second
  renderer that drifts") than a copy is.
- **Always render offscreen and blit into the window.** Rebuilds the window path
  around a convenience and puts every blessed reference downstream of the
  rebuild.
- **Refuse, and leave the window uncapturable.** Defensible — a screenshot is in
  no completion criterion — and rejected because the cost measured out at one
  conditional bitflag rather than a rebuild, which is the condition the task's
  halt branch was written around.

## Decision

**A screenshot is a copy of the frame that is about to be presented.** It is
taken between the encode and the handover, from the swapchain image itself. It
is not a second render, and nothing is drawn differently because a capture was
asked for.

**The surface asks for `COPY_SRC` only when it is offered.**
`window::choose_usage` is the fourth member of the family `choose_format`,
`choose_present_mode` and `choose_alpha_mode` already form: a pure function over
what the adapter reported, testable without a GPU. Where the flag is absent the
surface is configured exactly as before and `WindowTarget::read_back` answers
`RenderError::SurfaceNotReadable`, naming the missing usage and pointing at
`--screenshot`, which needs no window.

**A read-back yields genuine RGBA8, whatever the surface's channel order.**
`offscreen::rgba_from` is the step that makes `Pixels::rgba`'s promise true for a
BGRA surface. It accepts the four 8-bit RGBA/BGRA formats and refuses everything
else with `RenderError::UnreadableFormat` rather than reinterpreting it — a
`Rgb10a2Unorm` texel is four bytes and is not four RGBA8 channels, and this
machine's surface offers that format.

**The copy itself is the offscreen path's own code.**
`offscreen::read_back_texture` is `finish_and_read_back`'s body, moved out
unchanged and parameterised by device, queue and texture. Nothing about padding,
row order or mapping is written twice, so what an offscreen test proves about
those covers a windowed read-back too.

**The request is world-adjacent state and the file is not.** `SceneHost` carries
a three-state `Capture` — idle, wanted, taken — and hands the bytes to the
runner; the runner names the file, writes it, and reports **the path the write
returned**. A render path does not acquire a filesystem, and a message is not
computed beside the effect it describes.

## Consequences

- **The ten blessed references cannot move, and the derivation is the strong
  one.** Nothing in the shared draw path changed: `quad.rs`, `encode_runs`,
  `LoadOp`, the blend state and the drawing order are untouched, and
  `read_back_texture` is a move rather than an edit. The surface configuration
  *did* change, which weakens the derivation by one step — but no reference is
  drawn through a `WindowTarget`; all ten go through `OffscreenTarget`, whose
  texture already carried `COPY_SRC`. Measured: `git status` over both
  `tests/golden` trees is empty and each of the ten blobs is hash-identical with
  its HEAD blob.
- **A captured frame is not a frame anybody may time.** The copy waits for the
  GPU, for the reason `WindowTarget::drain` already carries. `SceneHost` serves
  at most one capture per frame and only when asked, so an ordinary frame emits
  what it always did — the same "off is free" property ADR-0039 gives the
  overlay, and `a_frame_nobody_asked_about_is_never_read_back` is what holds it.
- **The oracle needs no window.** The same content rendered through
  `Rgba8UnormSrgb` and through `Bgra8UnormSrgb`, both read back with
  `read_back_texture` and the second normalised, comes out byte for byte
  identical on both platforms. That is what a windowed read-back rests on and it
  is checked in the ordinary verification set. Its limit is measured and stated:
  a defect **symmetric in both formats** — a lost row, say — is invisible to it,
  and is caught instead by the offscreen read-back's own tests, which is the
  second reason the copy is shared rather than copied.
- **What only a human can check stays small and named.** That the copied bytes
  are the bytes the compositor put on the screen is below the level anything
  here can observe, which is what CLAUDE.md already records for the present
  path. Everything above it — the usage decision, the copy, the row unpadding,
  the channel order, the moment, the file — is machine-checked.
- **Two costs are named rather than measured.** Whether `COPY_SRC` on a
  swapchain image changes how a driver allocates or compresses it, and what that
  does to frame time, is **uncertain**: nothing here measures a window with the
  flag against a window without it, and the window path has no automated
  coverage by design. And a surface offering neither an RGBA nor a BGRA 8-bit
  format would make screenshots unavailable on that platform; neither of the two
  measured is such a surface.
- **This decides nothing about an agent-facing screenshot.** ADR-0031's two
  answering moments and D20's gated socket are untouched, and so is v1.04's
  booking that the windowed runner gets no IPC seam. If a screenshot ever has to
  cross the protocol, it starts from that booking and not from this file.
