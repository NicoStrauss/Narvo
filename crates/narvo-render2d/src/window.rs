//! Presenting the render path in a window.
//!
//! Thin on purpose. Everything that decides what a frame looks like - the
//! pipeline, the shader, the vertex data, the sampler, the draw call - is the
//! same code the offscreen path uses and the golden-image tests cover. What is
//! added here is what a window needs and a texture does not: a surface, its
//! configuration, resizing, and presenting.
//!
//! Since M3.32 it also carries three things a window does not strictly need but
//! a *measurement* does, and they are named rather than blended in: the frame
//! split into separately callable steps ([`WindowTarget::begin_frame`],
//! [`WindowTarget::draw_sprites`], [`WindowTarget::present`]) so a clock can sit
//! between them, [`PresentPolicy`] so the display can be taken out of a number,
//! and [`WindowTarget::drain`], which is explicitly not part of an ordinary
//! frame.

use std::fmt;
use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::error::RenderError;
use crate::gpu;
use crate::offscreen::{Pixels, read_back_texture, rgba_from};
use crate::quad::QuadPipeline;
use crate::sprite::{
    BatchOf, CameraView, MAX_SPRITES_PER_BATCH, Projection, SpriteBatch, SpriteFilter,
    SpriteInstance, batch_plan, batch_vertices,
};

/// Picks the format the surface is configured with and the pipeline built for.
///
/// Prefers sRGB so the window sees the same colour handling as the offscreen
/// path and the golden images, and falls back to whatever is offered if a
/// platform has no sRGB format at all.
///
/// A free function rather than an inline `find` so it can be tested without a
/// GPU - see the tests at the bottom of this file.
pub(crate) fn choose_format(available: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    available
        .iter()
        .copied()
        .find(|format| format.is_srgb())
        .or_else(|| available.first().copied())
}

/// What a caller wants from the swapchain's pacing.
///
/// Two policies rather than an exposed `wgpu::PresentMode`, because the mode
/// itself is a `wgpu` type and this crate's API boundary keeps those out - a
/// caller still learns nothing about which graphics backend is in use.
///
/// The distinction is not cosmetic and it is why this type exists at all: under
/// [`Self::VSync`] the swapchain paces the loop to the display, so a frame-time
/// measurement taken through it measures the display. Under [`Self::Uncapped`]
/// nothing waits for a scan-out, so the same measurement is of the work rather
/// than of the pacing. A throughput figure needs the second; a question about
/// whether the loop *holds* a rate needs the first. Both are asked in
/// `docs/perf/BASELINE.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PresentPolicy {
    /// Wait for the display. Fifo when offered, which is vsync with no tearing.
    #[default]
    VSync,
    /// Do not wait for the display. Immediate first, then Mailbox.
    ///
    /// Immediate is preferred over Mailbox deliberately: Mailbox still paces
    /// *presentation* to the display and only avoids blocking the queue, while
    /// Immediate hands the frame over as soon as it is ready. For a measurement
    /// that wants no scan-out in the number at all, Immediate is the one that
    /// removes it.
    Uncapped,
}

/// Picks the present mode for `policy`.
///
/// Under [`PresentPolicy::VSync`], always Fifo when it is offered.
///
/// **"It always is" now cites something a reader can open.** Until M3.36 this
/// doc appealed to "the WebGPU and Vulkan specifications" and gave no location
/// for either, which is the shape `CLAUDE.md` rules out: the claim was as
/// unopenable as an assumption. The pinned dependency states it in two places —
/// `wgpu-types-30.0.0/src/surface.rs:929`, "Fifo is the only mode guaranteed to
/// be supported", and the variant's own doc at `:56`, "**Supported on**: All
/// platforms". What those guarantee is that Fifo can be *configured*; whether
/// `SurfaceCapabilities::present_modes` is obliged to *list* it is **uncertain**
/// and was not checked. Nothing here rests on the stronger reading — the chain
/// below falls back when Fifo is absent, and
/// `a_surface_without_fifo_at_all_still_yields_something` is that path's test.
///
/// Deliberately *not* `available[0]`: that is whatever the driver happened to
/// list first, which on
/// the machine this was written on is `Immediate`. Fifo is also the right
/// default for a window that draws on demand - vsync, no tearing, no spinning.
///
/// Under [`PresentPolicy::Uncapped`], Immediate then Mailbox, and then whatever
/// the [`PresentPolicy::VSync`] chain would have given - Fifo when it is
/// offered, otherwise the first mode on the list. A surface offering neither
/// Immediate nor Mailbox leaves nothing that does not wait, so the policy cannot
/// be honoured and something waiting is returned. **The caller is told which
/// mode was actually taken** by [`WindowTarget::present_mode`], so a measurement
/// is labelled with the fallback rather than with the request.
pub(crate) fn choose_present_mode(
    available: &[wgpu::PresentMode],
    policy: PresentPolicy,
) -> wgpu::PresentMode {
    let preference: &[wgpu::PresentMode] = match policy {
        PresentPolicy::VSync => &[wgpu::PresentMode::Fifo],
        PresentPolicy::Uncapped => &[wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox],
    };

    preference
        .iter()
        .copied()
        .find(|mode| available.contains(mode))
        .or_else(|| {
            available
                .contains(&wgpu::PresentMode::Fifo)
                .then_some(wgpu::PresentMode::Fifo)
        })
        .or_else(|| available.first().copied())
        .unwrap_or(wgpu::PresentMode::Fifo)
}

/// The name of a present mode, for a measurement that has to say which it used.
///
/// A number produced under `Immediate` and a number produced under `Fifo` are
/// answers to different questions, so no table carrying one may omit which.
pub(crate) fn present_mode_name(mode: wgpu::PresentMode) -> &'static str {
    match mode {
        wgpu::PresentMode::Fifo => "Fifo",
        wgpu::PresentMode::FifoRelaxed => "FifoRelaxed",
        wgpu::PresentMode::Immediate => "Immediate",
        wgpu::PresentMode::Mailbox => "Mailbox",
        wgpu::PresentMode::AutoVsync => "AutoVsync",
        wgpu::PresentMode::AutoNoVsync => "AutoNoVsync",
    }
}

/// The backends the **window's** instance asks for, before `WGPU_BACKEND` speaks.
///
/// On Windows: DX12 and nothing else. Everywhere else: wgpu's default set,
/// unchanged, because DX12 does not exist there.
///
/// # Why the window and not the workspace
///
/// This is a per-instance choice and it can be, because Narvo builds **two**
/// instances and they were already separate: this one, with a display handle,
/// and `OffscreenTarget`'s at `offscreen.rs`, without. Every blessed reference
/// in this repository is produced through the second one — nothing in the
/// workspace builds a [`WindowTarget`] outside production code — so moving this
/// set moves no image that anything compares. That was measured before it was
/// relied on (M7.1d/S1), and it is the whole reason the change is this small.
///
/// # What it buys, measured
///
/// A surface reconfigure on Windows over Vulkan blocks the next few frames for
/// about two seconds each inside [`WindowTarget::begin_frame`]. Over DX12 it does
/// not. On one machine, one binary, `narvo --frames 900 --resize-probe`:
/// **fifteen blocked frames and an `acquire` maximum of 2014.806 ms over Vulkan,
/// against zero blocked frames and 20.4 ms over DX12** — five resizes either way,
/// each one confirmed to have reached the surface.
///
/// # What it costs, also measured
///
/// DX12 has a failure mode Vulkan did not show here. Of **24** DX12 runs of that
/// command, **two** ended in a wgpu validation panic — "Surface is not configured
/// for presentation", from `get_current_texture` after a resize. Both fell in one
/// early batch and twenty-two later runs were clean, including four deliberately
/// alternated with Vulkan runs to test whether switching backends was the
/// trigger. It was not, and **what the trigger is stays unknown**. A plausible
/// reading that was *not* confirmed: [`WindowTarget::resize`] calls
/// `surface.configure` and never asks whether it worked, so a rejected
/// configuration would leave exactly this state behind.
///
/// The escape from both is the same and needs no rebuild: `WGPU_BACKEND=vulkan`.
///
/// # No silent fallback, deliberately
///
/// A Windows machine with no DX12 adapter gets [`RenderError::NoAdapter`] naming
/// the three rungs it tried, not a quiet Vulkan window. This whole task exists
/// because a backend nobody had named was doing something nobody had measured;
/// a fallback that picks a different one without saying so would rebuild that.
#[must_use]
pub(crate) fn window_backends() -> wgpu::Backends {
    if cfg!(target_os = "windows") {
        wgpu::Backends::DX12
    } else {
        wgpu::Backends::default()
    }
}

/// Picks how the window is composited with what is behind it.
///
/// Opaque: nothing in this renderer wants to blend with the desktop, and a
/// premultiplied mode would make a translucent window out of an image whose
/// alpha happens not to be 1.
pub(crate) fn choose_alpha_mode(
    available: &[wgpu::CompositeAlphaMode],
) -> wgpu::CompositeAlphaMode {
    if available.contains(&wgpu::CompositeAlphaMode::Opaque) {
        wgpu::CompositeAlphaMode::Opaque
    } else {
        available
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto)
    }
}

/// Picks what the swapchain images may be used for.
///
/// `RENDER_ATTACHMENT` always, because a swapchain image that cannot be drawn
/// into is of no use to anybody, and `COPY_SRC` beside it **when the surface
/// offers it** — that is the usage
/// [`WindowTarget::read_back`] needs to copy a drawn frame to the CPU.
///
/// **Asked rather than assumed, and the asking is the whole point of this
/// function.** `SurfaceCapabilities::usages` documents exactly one guarantee —
/// "The usage [`TextureUsages::RENDER_ATTACHMENT`] is guaranteed"
/// (`wgpu-types-30.0.0/src/surface.rs:530-533`) — so a surface is entitled to
/// offer nothing else. Configuring one with a usage it does not support is a
/// validation error, and a window that failed to open because somebody wanted a
/// screenshot would be the feature costing the thing it is a convenience for.
/// So the flag is dropped when it is not on offer and
/// [`RenderError::SurfaceNotReadable`] is what a caller gets later.
///
/// Both platforms this project verifies on **do** offer it — measured in M6.4b,
/// AMD Vulkan on Windows and lavapipe under WSL both reporting
/// `COPY_SRC | COPY_DST | TEXTURE_BINDING | STORAGE_BINDING | RENDER_ATTACHMENT
/// | STORAGE_ATOMIC` — so the fallback arm is unreached on either. It is here
/// because the guarantee, not the measurement, is what a third platform will be
/// held to.
///
/// A free function rather than an inline `if`, for the reason its three
/// neighbours are: this way it is tested without a GPU.
pub(crate) fn choose_usage(available: wgpu::TextureUsages) -> wgpu::TextureUsages {
    let wanted = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;

    if available.contains(wanted) {
        wanted
    } else {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    }
}

/// A render target that presents to a window.
///
/// Created from anything that can hand out a window and a display handle, which
/// in practice means a windowing library's window type. The bounds are
/// `raw-window-handle` traits rather than `wgpu` ones, so the crate's API
/// boundary holds: a caller still learns nothing about which graphics backend is
/// in use.
#[derive(Debug)]
pub struct WindowTarget {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    quad: QuadPipeline,
    /// The multisampled attachment every frame draws into, resolved into the
    /// swapchain texture. Sized to the surface, so [`Self::resize`] rebuilds it.
    ///
    /// The view alone, because it owns what it views: `wgpu::TextureView` holds
    /// its parent `Texture` by value (`wgpu-30.0.0/src/api/texture_view.rs:18`)
    /// and `TextureView::texture` records that all wgpu resources are refcounted
    /// (`:26-32`). A second field for the texture would keep nothing alive that
    /// this one does not.
    multisampled_view: wgpu::TextureView,
    adapter_summary: String,
}

/// Builds the multisampled colour attachment for a surface of this size.
///
/// Its own function because the constructor and every resize need exactly the
/// same texture, and a window that resized into a mismatched attachment would
/// fail at the next `begin_render_pass` rather than here.
fn multisample_attachment(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("narvo window multisample attachment"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: crate::SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

impl WindowTarget {
    /// Creates a surface on `window` and the pipeline that draws into it.
    ///
    /// The window is taken as an [`Arc`] because the surface borrows it for as
    /// long as it lives, and the instance needs the same handle to pick a
    /// backend. Sharing one allocation is cheaper and clearer than asking the
    /// caller for the handle twice.
    ///
    /// # Where a caller gets the traits, and why this crate re-exports nothing
    ///
    /// `W`'s bounds are `raw-window-handle`'s, and this crate takes that crate as
    /// an optional dependency without re-exporting it. **A caller holding a winit
    /// window needs nothing from here**: cargo unifies the two requirements onto
    /// one `raw-window-handle` 0.6, so `WindowTarget::new(Arc<winit::window::Window>, …)`
    /// compiles with no mention of the crate at all. A caller that wants to write
    /// the bound down itself takes the trait names from
    /// `winit::raw_window_handle`, which winit already re-exports at exactly that
    /// major (`winit-0.30.13/src/lib.rs:196`, `pub use rwh_06 as raw_window_handle`).
    ///
    /// M6b.1's survey measured all four cases and found a re-export here would
    /// serve only a caller whose window does *not* come from winit and which
    /// picks its own `raw-window-handle` version — and picks a different major,
    /// which is the one case that fails, with two copies of the trait in the
    /// graph. A re-export was therefore not added: it would be public surface for
    /// a case nothing in this repository has, and adding one later costs nothing
    /// that not adding it now does.
    ///
    /// # Errors
    ///
    /// - [`RenderError::InvalidSize`] if the window reports a zero dimension,
    ///   which happens while a window is minimised.
    /// - [`RenderError::NoSurface`] if no surface can be created on it, or the
    ///   surface offers no usable format.
    /// - [`RenderError::NoAdapter`] or [`RenderError::NoDevice`] as for the
    ///   offscreen path, except that the adapter must also be able to present to
    ///   this surface.
    pub fn new<W>(window: Arc<W>, width: u32, height: u32) -> Result<Self, RenderError>
    where
        W: HasWindowHandle + HasDisplayHandle + fmt::Debug + Send + Sync + 'static,
    {
        Self::with_present_policy(window, width, height, PresentPolicy::VSync)
    }

    /// [`Self::new`], with the swapchain paced by `policy`.
    ///
    /// `new` is this with [`PresentPolicy::VSync`] and has no code of its own, so
    /// the ordinary window keeps exactly the configuration it had before the
    /// parameter existed.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_present_policy<W>(
        window: Arc<W>,
        width: u32,
        height: u32,
        policy: PresentPolicy,
    ) -> Result<Self, RenderError>
    where
        W: HasWindowHandle + HasDisplayHandle + fmt::Debug + Send + Sync + 'static,
    {
        if width == 0 || height == 0 {
            return Err(RenderError::InvalidSize {
                width,
                height,
                max: u32::MAX,
            });
        }

        // The display handle lets wgpu pick a backend that can actually talk to
        // this windowing system. The backend set is this crate's own choice
        // (see `window_backends`), and `with_env` runs *after* it, so
        // `WGPU_BACKEND` still overrides — which is what keeps the comparison
        // between the two backends measurable from a command line.
        let mut descriptor =
            wgpu::InstanceDescriptor::new_with_display_handle(Box::new(Arc::clone(&window)));
        descriptor.backends = window_backends();
        let instance = wgpu::Instance::new(descriptor.with_env());

        let surface = instance
            .create_surface(window)
            .map_err(|error| RenderError::NoSurface {
                source: Box::new(error),
            })?;

        // Unlike the offscreen path this one must present, so the adapter search
        // is constrained to adapters that can.
        let (selection, adapter) = gpu::select_adapter(&instance, Some(&surface))?;
        let adapter_summary = gpu::summarize(&adapter, selection);
        let (device, queue) = gpu::create_device(&adapter)?;

        let capabilities = surface.get_capabilities(&adapter);
        let format =
            choose_format(&capabilities.formats).ok_or_else(|| RenderError::NoSurface {
                source: "the surface offers no texture format at all".into(),
            })?;

        let config = wgpu::SurfaceConfiguration {
            usage: choose_usage(capabilities.usages),
            format,
            width,
            height,
            present_mode: choose_present_mode(&capabilities.present_modes, policy),
            alpha_mode: choose_alpha_mode(&capabilities.alpha_modes),
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Built for the surface's format rather than the offscreen one, but from
        // the same shader and the same vertex data.
        let quad = QuadPipeline::new(&device, format);
        let multisampled_view = multisample_attachment(&device, &config);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            quad,
            multisampled_view,
            adapter_summary,
        })
    }

    /// Human-readable description of the adapter in use and how it was picked.
    #[must_use]
    pub fn adapter_summary(&self) -> &str {
        &self.adapter_summary
    }

    /// Current surface size in pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// The present mode the surface was actually configured with.
    ///
    /// The mode that was *taken*, not the policy that was asked for: a surface
    /// offering neither Immediate nor Mailbox falls back to a waiting mode under
    /// [`PresentPolicy::Uncapped`], and a measurement that printed the requested
    /// policy would then be labelled with a wait it did not exclude.
    #[must_use]
    pub fn present_mode(&self) -> &'static str {
        present_mode_name(self.config.present_mode)
    }

    /// Reconfigures the surface after the window changed size.
    ///
    /// A zero dimension is ignored rather than treated as an error: a minimised
    /// window reports one, and reconfiguring to it would be rejected by the
    /// driver. The next non-zero resize puts things right.
    ///
    /// # What this costs, measured
    ///
    /// **Superseded by M7.1d, and kept rather than rewritten** — the section
    /// below describes what a resize cost while the window ran on Vulkan, which
    /// on Windows it no longer does. See [`window_backends`] for what replaced
    /// it and what that is worth: on the same machine and the same probe, fifteen
    /// blocked frames became zero, and the worst `acquire` in a 1 400-frame game
    /// run with seven resizes became 33.107 ms — one missed vsync interval.
    /// Everything below still holds for `WGPU_BACKEND=vulkan`, which is exactly
    /// how it was re-measured in M7.1d, and it is still the whole truth on any
    /// platform without DX12.
    ///
    /// One sentence of it was wrong when it was written and is corrected here
    /// rather than silently: M7.1c recorded that a four-pixel resize costs what a
    /// maximise costs. That came from a run whose window was already maximised
    /// when it opened. M7.1d measured the two apart and they are not always
    /// alike — in one run only the maximise blocked, and every resize blocked
    /// thereafter, in that process and in the next.
    ///
    /// **On Windows over the Vulkan backend, roughly two seconds per swapchain
    /// image, charged to the next few calls to [`Self::begin_frame`]** — not to
    /// this function, which returns promptly. With
    /// `desired_maximum_frame_latency: 2` the swapchain holds three images and
    /// three consecutive frames each block for about 2 s, so one resize costs
    /// about six seconds of frozen window (M7.1c).
    ///
    /// Reproduce it with `narvo --frames 700 --resize-probe`, and read the
    /// `acquire` row: 6.067 ms max with nothing resizing, 2015.478 ms with four
    /// resizes, on one machine and one binary. The same command over the DX12
    /// backend (`WGPU_BACKEND=dx12`) gives 22.035 ms, and under WSL's lavapipe
    /// the question could not be asked at all — the compositor refused every
    /// size the probe requested, so no surface was ever reconfigured there.
    ///
    /// **What is not known** is which of the three waits inside `wgpu-hal`'s
    /// `Swapchain::acquire` is the one. Two measurements narrow it:
    /// `--drain` leaves the block intact, which rules out the wait on the
    /// submission fence (`wgpu-hal-30.0.0/src/vulkan/swapchain/native.rs:449`);
    /// and `--uncapped` leaves it intact while the ordinary-frame median falls
    /// from 3.5 ms to 0.3 ms, which makes `vkAcquireNextImageKHR` (`:461`) an
    /// unlikely place for a wait that survives having no scan-out to wait for.
    /// What remains is the Windows-only fence wait at `:494`, whose own comment
    /// names the arrangement this machine is in — "the Vulkan driver is using a
    /// DXGI swapchain". That is consistent with every measurement and was **not**
    /// confirmed by instrumenting `wgpu-hal`, so it stays uncertain.
    ///
    /// Nothing here is a defect in this function, and no fix is attempted in it:
    /// the cost is recorded so the next reader does not have to measure it again.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 || (width, height) == self.size() {
            return;
        }

        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        // The old attachment is the wrong size now. Rebuilding it here rather
        // than per frame keeps the allocation off the frame path, at the cost
        // of one texture per resize.
        self.multisampled_view = multisample_attachment(&self.device, &self.config);
    }

    /// Draws `image` across the window and presents the frame.
    ///
    /// The same quad, shader and sampler the offscreen path uses, so what the
    /// window shows is what a golden-image test would compare - modulo the
    /// window's size and surface format.
    ///
    /// Not every call draws. A minimised or occluded window and a timed-out
    /// swapchain both mean "there is nothing useful to do right now", and an
    /// out-of-date surface is reconfigured here so the next call succeeds. Those
    /// are ordinary states of a window, not failures, and the return value says
    /// which one happened rather than hiding it in an error.
    ///
    /// # Errors
    ///
    /// [`RenderError::NoFrame`] for the two states the triage above does not
    /// handle: the surface was lost, or the acquire raised a validation error.
    ///
    /// **They are not one case, and this doc said "neither is recoverable
    /// without building the target again" until M3.36.** `wgpu` documents them
    /// separately and does not give them one answer. `Lost`: the surface "has
    /// been lost and needs to be recreated" — recreate it, configure it, try
    /// again (`wgpu-30.0.0/src/api/surface_texture.rs:67-72`), which from out
    /// here is building the target again. `Validation`: applications "should
    /// attend to the validation error and try again" (`:74-78`), which is not
    /// the same instruction.
    ///
    /// **What `Validation` actually carries is not settled here: uncertain, and
    /// deliberately left so.** It is produced from an error the frontend routes
    /// through the surface's error sink
    /// (`wgpu-30.0.0/src/backend/wgpu_core.rs:4023-4032`), and the underlying
    /// `wgpu_core::present::SurfaceError`
    /// (`wgpu-core-30.0.0/src/present.rs:43-56`) holds cases as different as
    /// `AlreadyAcquired` and `Device(DeviceError)`. Which of them reach a caller
    /// as `Validation` rather than ending the process depends on the error's
    /// class and on whether an error scope is pushed — and this crate pushes
    /// none. Establishing that needs a GPU and a test, not a doc comment; two
    /// earlier drafts of this paragraph asserted an answer and both were wrong.
    /// What survives is the negative claim the correction was for: no single
    /// sentence about recovery covers both variants.
    ///
    /// Those are the only two variants the arm can see in `wgpu` 30.0.0, whose
    /// `CurrentSurfaceTexture` has seven and is not `#[non_exhaustive]`
    /// (`wgpu-30.0.0/src/api/surface_texture.rs:44-80`). The arm is still a
    /// catch-all, so a variant added by a later `wgpu` would arrive here too and
    /// this paragraph would owe an update.
    pub fn present_textured_quad(&mut self, image: &Pixels) -> Result<FrameOutcome, RenderError> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            // Suboptimal still draws correctly; it only asks to be reconfigured
            // eventually, and the next resize does that anyway.
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,

            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(FrameOutcome::Skipped);
            }

            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(FrameOutcome::Reconfigured);
            }

            other => {
                return Err(RenderError::NoFrame {
                    source: format!("the surface reported {other:?}").into(),
                });
            }
        };

        // The window path draws the single quad, not a sprite batch: `Nearest`,
        // named rather than defaulted.
        let bindings = self.quad.bind_texture(
            &self.device,
            &self.queue,
            image.width(),
            image.height(),
            image.rgba(),
        );
        let bind_group = bindings.for_filter(SpriteFilter::Nearest);

        {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("narvo window frame"),
                });

            self.quad
                .encode_pass(&mut encoder, &self.multisampled_view, &view, bind_group);
            self.queue.submit(std::iter::once(encoder.finish()));
        }

        // Presenting is an explicit call in wgpu 30, on the queue and not on the
        // surface texture. Dropping the texture instead does not present it, it
        // throws the frame away - silently, with no error anywhere, which is
        // exactly how this window came to be blank while every other diagnostic
        // said the frame had been drawn correctly.
        self.queue.present(frame);

        Ok(FrameOutcome::Presented)
    }

    /// Acquires the next swapchain image.
    ///
    /// The first of the three steps [`SurfaceFrame`] exists to separate, and the
    /// **candidate home of the display wait**: under a Fifo surface the
    /// swapchain has no image free until the display has scanned one out, so
    /// either this call or [`Self::present`] must absorb that time. Which one
    /// does is a property of the driver and the present mode, and this doc does
    /// not settle it in general.
    ///
    /// **On the machine `docs/perf/BASELINE.md` describes, it is this one**, and
    /// that was measured rather than assumed: as the frame's work grows, the
    /// time spent here shrinks by nearly the same amount while the frame
    /// interval stays pinned to the display's period. That is why a frame loop
    /// times this step on its own - folding it into the drawing would put the
    /// display's pacing inside a number that is supposed to be about the work.
    ///
    /// The triage matches [`Self::present_textured_quad`]'s exactly, because it
    /// is the same surface in the same states; an out-of-date surface is
    /// reconfigured here so the next call succeeds.
    ///
    /// # Errors
    ///
    /// [`RenderError::NoFrame`] in exactly the two states
    /// [`Self::present_textured_quad`] returns it in — a lost surface and a
    /// validation error — which one sentence about recovery cannot cover. The
    /// argument, including what is left uncertain about `Validation`, is
    /// spelled out there rather than repeated here: the triage is the same
    /// match, so the two must not drift apart, and one copy of the argument is
    /// how that is kept true.
    pub fn begin_frame(&mut self) -> Result<FrameStart, RenderError> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                Ok(FrameStart::Ready(SurfaceFrame { texture }))
            }

            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                Ok(FrameStart::Skipped)
            }

            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                Ok(FrameStart::Reconfigured)
            }

            other => Err(RenderError::NoFrame {
                source: format!("the surface reported {other:?}").into(),
            }),
        }
    }

    /// Draws `sprites` into an acquired frame and submits the work.
    ///
    /// The second of the three steps. This is the windowed twin of
    /// [`OffscreenTarget::render_sprites_viewed_by`][offscreen] and goes through
    /// the *same* `batch_runs` decomposition, the same `encode_runs`, the same
    /// pipeline and the same multisample attachment - so what a window shows is
    /// what a golden image would compare, modulo the surface's size and format.
    /// The D15 rule holds here as it does offscreen: the drawing order is the
    /// order of `sprites`, runs are cut where the sampler wish changes, and one
    /// draw call is issued per run.
    ///
    /// **What this does not do is wait for the GPU.** `queue.submit` hands the
    /// work over and returns; the drawing happens afterwards. A caller timing
    /// this call is therefore measuring CPU-side preparation and submission, not
    /// GPU execution, and a frame-time table built only from it would be
    /// answering a narrower question than it appears to. That limitation is
    /// stated where the numbers are, in `docs/perf/BASELINE.md`.
    ///
    /// # Errors
    ///
    /// [`RenderError::BatchTooLarge`] if `sprites` holds more than
    /// [`MAX_SPRITES_PER_BATCH`](crate::MAX_SPRITES_PER_BATCH).
    ///
    /// [offscreen]: crate::OffscreenTarget::render_sprites_viewed_by
    /// `overlay` is drawn **after** `sprites`, from its own texture.
    ///
    /// The window half of M6.6c's seam. `None` — and equally a batch with no
    /// sprites in it — produces **nothing at all**: no second bind group, no
    /// extra run, the same command sequence this method emitted before the
    /// parameter existed. That is the property `batch_plan` is written around,
    /// and it is what makes "the shared code is unchanged" the regression
    /// evidence for the blessed references rather than "the overlay is off".
    ///
    /// Both batches go into the *same* pass. The blend state, the pass and its
    /// `LoadOp::Clear` are untouched (ADR-0023): `encode_runs` binds per run, so
    /// a second texture is a second group of runs and not a second pass.
    ///
    /// Since M6b.4 the overlay is seen through its own
    /// [`SpriteBatch::camera`](crate::SpriteBatch::camera) rather than through
    /// `camera` — [`CameraView::IDENTITY`] is the screen-fixed layer, and
    /// passing `camera` itself reproduces the pre-M6b.4 frame bit for bit. The
    /// offscreen twin says the same in more detail.
    pub fn draw_sprites(
        &mut self,
        frame: &SurfaceFrame,
        image: &Pixels,
        sprites: &[SpriteInstance],
        overlay: Option<SpriteBatch<'_>>,
        camera: CameraView,
    ) -> Result<(), RenderError> {
        // Emptiness rather than `Option`, exactly as the offscreen path does it.
        let overlay = overlay.filter(|batch| !batch.sprites.is_empty());
        let overlay_len = overlay.map_or(0, |batch| batch.sprites.len());

        if sprites.len() + overlay_len > MAX_SPRITES_PER_BATCH {
            return Err(RenderError::BatchTooLarge {
                requested: sprites.len() + overlay_len,
                limit: MAX_SPRITES_PER_BATCH,
            });
        }

        let bindings = self.quad.bind_texture(
            &self.device,
            &self.queue,
            image.width(),
            image.height(),
            image.rgba(),
        );
        // Built only when there is something to draw from it.
        let overlay_bindings = overlay.map(|batch| {
            self.quad.bind_texture(
                &self.device,
                &self.queue,
                batch.image.width(),
                batch.image.height(),
                batch.image.rgba(),
            )
        });

        // D15, exactly as the offscreen path does it: cut the drawing order into
        // runs of equal filter, one draw call each, and touch the order not at
        // all. The run's first sprite names the sampler for all of them, because
        // that is what `batch_runs` cut on; since M6.6c the batch it fell in
        // names the texture, which is what `batch_plan` carries.
        let overlay_sprites: &[SpriteInstance] = overlay.map_or(&[], |batch| batch.sprites);
        let runs: Vec<(std::ops::Range<usize>, &wgpu::BindGroup)> =
            batch_plan(sprites, overlay_sprites)
                .into_iter()
                .map(|(run, batch)| match batch {
                    BatchOf::First => {
                        let filter = sprites[run.start].filter;
                        (run, bindings.for_filter(filter))
                    }
                    BatchOf::Second => {
                        let filter = overlay_sprites[run.start - sprites.len()].filter;
                        let bindings = overlay_bindings
                            .as_ref()
                            .expect("a Second run exists only when the overlay was bound");
                        (run, bindings.for_filter(filter))
                    }
                })
                .collect();

        // M6b.4, exactly as the offscreen path does it: the overlay's projection
        // is this one with its camera field replaced, so the two share the
        // target's half-extents and an equal camera is a bit-for-bit identity.
        let projection =
            Projection::for_target(self.config.width, self.config.height).viewed_by(camera);
        let mut corners = batch_vertices(sprites, projection);
        let overlay_projection = projection.viewed_by(overlay.map_or(camera, |batch| batch.camera));
        corners.extend(batch_vertices(overlay_sprites, overlay_projection));
        let vertices = self.quad.corner_buffer(&self.device, &corners);
        let indices = self
            .quad
            .batch_index_buffer(&self.device, sprites.len() + overlay_len);

        let view = frame
            .texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo window sprite batch"),
            });

        self.quad.encode_runs(
            &mut encoder,
            &self.multisampled_view,
            &view,
            &vertices,
            &indices,
            &runs,
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        Ok(())
    }

    /// Blocks until the GPU has finished everything submitted so far.
    ///
    /// **This exists to make GPU execution measurable, and it is not part of an
    /// ordinary frame.** `queue.submit` returns as soon as the work is handed
    /// over, so a clock around [`Self::draw_sprites`] times CPU-side preparation
    /// and submission and nothing else: a frame with vast GPU headroom and a
    /// frame that is GPU-bound produce the same number. Nothing else in this
    /// crate can tell them apart either - timestamp queries would need
    /// `Features::TIMESTAMP_QUERY`, and `gpu::create_device` requests no features
    /// at all.
    ///
    /// So the honest way to price the GPU with what is available is to stop and
    /// wait for it, which is what this does. **Calling it changes what is being
    /// measured**: it serialises CPU and GPU instead of letting them overlap, so
    /// a loop that drains every frame is slower than the same loop that does
    /// not, and the drained figure is an *attribution* of GPU cost rather than
    /// the frame time of a pipelined loop. A measurement using it has to say so.
    ///
    /// # Errors
    ///
    /// [`RenderError::DeviceWait`] if the poll failed. That is **not** a lost
    /// device: `wgpu`'s `PollError` is a timeout or a submission index belonging
    /// to another device, and a lost device arrives by a different route
    /// entirely. An earlier version of this doc said "the device is lost" and
    /// mapped the failure to [`RenderError::NoFrame`], which named a swapchain
    /// problem that had not happened.
    pub fn drain(&self) -> Result<(), RenderError> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| RenderError::DeviceWait {
                source: Box::new(error),
            })?;

        Ok(())
    }

    /// Hands a drawn frame to the compositor.
    ///
    /// The third step, and the one that must not be skipped: dropping a
    /// [`SurfaceFrame`] instead of presenting it throws the frame away silently,
    /// with no error anywhere - the failure that left this window blank in M1
    /// while every other diagnostic said the frame had been drawn correctly.
    ///
    /// Taking the frame by value **does not prevent that**, and it would be
    /// comfortable to claim it does: Rust has no linear types, so dropping a
    /// value is always legal and a caller that acquires and never presents still
    /// compiles. What by-value buys is narrower and worth having anyway — a
    /// frame cannot be presented twice, and it cannot be used after presenting.
    pub fn present(&mut self, frame: SurfaceFrame) {
        self.queue.present(frame.texture);
    }

    /// Copies a drawn frame back to the CPU as RGBA8.
    ///
    /// **Between [`Self::draw_sprites`] and [`Self::present`], and it reads the
    /// image that is about to be presented** rather than rendering the scene a
    /// second time. That distinction is the reason this exists in this shape: a
    /// second render would agree with the window only by an argument about the
    /// inputs being equal, while a copy of the swapchain image is the frame
    /// itself. Whatever the window shows — the overlay's presence, the draw
    /// order, the sampler each run took — is in these bytes because it is in
    /// that texture.
    ///
    /// It does **not** consume the frame. Presenting still has to happen and is
    /// still the caller's, so a screenshot costs the frame nothing but the wait.
    ///
    /// **It waits for the GPU**, unavoidably: the copy is queued behind the draw
    /// and the mapping callback only runs while the device is polled. A frame
    /// this is called on is therefore not a frame anybody may time, for exactly
    /// the reason [`Self::drain`] carries.
    ///
    /// Two steps, both shared rather than written again here.
    /// [`read_back_texture`] is the offscreen path's own copy-and-unpad, which
    /// is what makes an offscreen test of row padding a test of this too;
    /// [`rgba_from`] is what turns the surface's channel order into the RGBA8
    /// [`Pixels`] promises, and it is not optional — `choose_format` returns a
    /// BGRA surface on any platform whose list has no RGBA format in it, WSL's
    /// lavapipe among them.
    ///
    /// # Errors
    ///
    /// - [`RenderError::SurfaceNotReadable`] if the surface was configured
    ///   without `COPY_SRC` because the adapter did not offer it. Checked here
    ///   rather than left to the driver, so the answer is a sentence instead of
    ///   a validation error from inside the copy.
    /// - [`RenderError::UnreadableFormat`] if the surface's format is not one of
    ///   the four 8-bit RGBA or BGRA formats.
    /// - [`RenderError::Readback`] if the copy, the poll or the mapping failed.
    pub fn read_back(&self, frame: &SurfaceFrame) -> Result<Pixels, RenderError> {
        if !self.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            return Err(RenderError::SurfaceNotReadable);
        }

        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo window read-back"),
            });

        // The surface texture, not the multisample attachment: the pass
        // resolves into the former, and a multisampled texture is not a legal
        // copy source anyway — `wgpu-core-30.0.0/src/command/transfer.rs:428`
        // states it ("The texture must not be multisampled") and `:458-459`
        // enforces it with `TransferError::InvalidSampleCount`.
        // `self.config` rather than the texture's own size,
        // because the two are the same by construction and this is the pair the
        // pipeline's projection was built from.
        let raw = read_back_texture(
            &self.device,
            &self.queue,
            encoder,
            &frame.texture.texture,
            self.config.width,
            self.config.height,
        )?;

        rgba_from(self.config.format, raw)
    }
}

/// A swapchain image that has been acquired and not yet presented.
///
/// Opaque on purpose. It wraps the surface texture so that the three steps of a
/// frame - acquire, draw, present - can be *separate calls* without a `wgpu`
/// type crossing this crate's API boundary, which is the same rule
/// [`WindowTarget::new`] follows for the window handle.
///
/// The separation exists for one reason: **the frame-time measurement lives
/// outside the renderer** (`ProjektPlan.md` §6/M3, and the M3.7 list of what was
/// missing). A caller timing a single all-in-one call learns the total and
/// nothing else, and under a waiting present mode the total is dominated by a
/// wait it cannot see. Three calls let the loop put a clock between them and
/// find out *which* step the time went to. Nothing here reads a clock.
///
/// Not `Clone` or `Copy`: a swapchain image is presented exactly once, and
/// [`WindowTarget::present`] consumes it to say so in the type system.
#[derive(Debug)]
pub struct SurfaceFrame {
    texture: wgpu::SurfaceTexture,
}

/// What acquiring a swapchain image produced.
///
/// The two non-`Ready` arms are ordinary states of a window rather than
/// failures. **No caller distinguishes them today** - the frame loop's host maps
/// both to "nowhere to draw" - and they are kept apart anyway because they are
/// different events: a skipped frame is one the compositor did not want, and a
/// reconfigured one is a frame the surface spent recovering. Merging them in
/// this type would make that distinction unrecoverable for a caller that later
/// wants to report them separately; merging them at the call site, which is what
/// happens now, does not.
#[derive(Debug)]
pub enum FrameStart {
    /// An image was acquired. It must be presented or dropped.
    Ready(SurfaceFrame),
    /// Nothing to draw into: the window is occluded or minimised, or the
    /// swapchain timed out.
    Skipped,
    /// The surface was out of date and has been reconfigured. Nothing was
    /// acquired; the next call should succeed.
    Reconfigured,
}

/// What a request to draw a frame actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The frame was drawn and handed to the compositor.
    Presented,
    /// Nothing was drawn: the window is occluded or minimised, or the swapchain
    /// timed out. Try again on the next redraw.
    Skipped,
    /// The surface was out of date and has been reconfigured. Nothing was drawn
    /// this time; the next call should present.
    Reconfigured,
}

#[cfg(test)]
mod tests {
    use super::{
        PresentPolicy, choose_alpha_mode, choose_format, choose_present_mode, choose_usage,
        present_mode_name, window_backends,
    };
    use crate::offscreen::offscreen_backends;
    use wgpu::{Backends, CompositeAlphaMode, PresentMode, TextureFormat, TextureUsages};

    /// On Windows the window asks for DX12 and for nothing else.
    ///
    /// The number this defends is in `window_backends`' own doc: fifteen blocked
    /// frames over Vulkan against zero over DX12. Widening the set would let
    /// wgpu's adapter ladder land on Vulkan again on a machine that offers both,
    /// and the window would go back to freezing for six seconds a resize without
    /// one line of this file looking wrong.
    #[test]
    #[cfg(target_os = "windows")]
    fn the_window_asks_for_dx12_and_for_nothing_else() {
        assert_eq!(
            window_backends(),
            Backends::DX12,
            "the window's backend set is no longer DX12 alone, so wgpu may pick \
             Vulkan again and a resize costs about six seconds"
        );
    }

    /// Off Windows the window keeps exactly the set it had before M7.1d.
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn elsewhere_the_window_keeps_the_default_set() {
        assert_eq!(
            window_backends(),
            Backends::default(),
            "a platform without DX12 must be left exactly as it was"
        );
    }

    /// **The offscreen path does not inherit the window's choice.**
    ///
    /// This is the guard the split actually needs, and it is the one that can
    /// fail on both platforms. Every blessed reference is drawn through
    /// `OffscreenTarget`'s instance; if somebody ever routes that instance
    /// through `window_backends` — a plausible tidying, since the two lines look
    /// alike — then the twelve references would start being produced on whatever
    /// backend the *window* wants, and no image comparison could notice, because
    /// both sides of every comparison would move together.
    #[test]
    fn the_offscreen_path_keeps_wgpus_own_backend_set() {
        assert_eq!(
            offscreen_backends(),
            Backends::default(),
            "the offscreen path no longer asks for wgpu's default backends, so \
             the blessed references are drawn on a substrate somebody chose"
        );

        if cfg!(target_os = "windows") {
            assert_ne!(
                offscreen_backends(),
                window_backends(),
                "the window's backend choice has reached the offscreen path, \
                 which is where every blessed reference is produced"
            );
        }
    }

    /// And the call site is held too, because the test above cannot hold it.
    ///
    /// `offscreen_backends` being right proves nothing if `OffscreenTarget::new`
    /// stops calling it — the leak this exists to stop is one edited line in
    /// another file, and a test that compares two constants is blind to it. So
    /// this reads that file's own source, which is the same technique `xtask`
    /// uses to keep the verification set from drifting away from `CLAUDE.md`.
    ///
    /// It is deliberately a *source* check and not a rendering one: telling the
    /// two backends apart from their pixels is exactly what the golden tolerance
    /// (4 counts, 0.1 % of pixels, cap 24) is not able to promise.
    ///
    /// Comments are stripped before looking, and that is not tidiness — the first
    /// version of this test failed on `offscreen.rs`'s own doc comment, which
    /// names `window_backends` in order to say it is *not* used. A guard that
    /// cannot tell a mention from a call would have to be either disabled or
    /// obeyed by never writing the word, and both are worse than stripping.
    #[test]
    fn the_offscreen_call_site_does_not_reach_for_the_windows_backend_set() {
        let code: String = include_str!("offscreen.rs")
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            code.contains("descriptor.backends = offscreen_backends();"),
            "OffscreenTarget::new no longer sets its backend set through \
             offscreen_backends, so what it asks for is unknown to this guard"
        );
        assert!(
            !code.contains("window_backends"),
            "offscreen.rs reaches for the window's backend set, so the blessed \
             references would be produced on the backend the window wants"
        );
    }

    #[test]
    fn an_srgb_format_is_preferred_even_when_a_linear_one_is_listed_first() {
        let offered = [
            TextureFormat::Rgba8Unorm,
            TextureFormat::Bgra8Unorm,
            TextureFormat::Bgra8UnormSrgb,
        ];

        assert_eq!(
            choose_format(&offered),
            Some(TextureFormat::Bgra8UnormSrgb),
            "a linear surface would make the window disagree with the offscreen \
             path and with every golden image"
        );
    }

    #[test]
    fn without_an_srgb_format_the_first_one_offered_is_taken() {
        let offered = [TextureFormat::Rgba16Float, TextureFormat::Rgba8Unorm];

        assert_eq!(choose_format(&offered), Some(TextureFormat::Rgba16Float));
    }

    #[test]
    fn a_surface_offering_nothing_yields_nothing_rather_than_panicking() {
        assert_eq!(choose_format(&[]), None);
    }

    #[test]
    fn fifo_is_chosen_even_when_the_driver_lists_immediate_first() {
        // Exactly what this machine's Vulkan driver offers. Taking entry zero
        // would pick Immediate, which is a choice nobody made.
        let offered = [
            PresentMode::Immediate,
            PresentMode::Fifo,
            PresentMode::FifoRelaxed,
        ];

        assert_eq!(
            choose_present_mode(&offered, PresentPolicy::VSync),
            PresentMode::Fifo
        );
    }

    #[test]
    fn an_uncapped_policy_takes_immediate_over_the_fifo_that_is_always_offered() {
        // The same list. The default policy picks Fifo from it; this one must
        // not, or a measurement that exists to exclude the display would be
        // taken through the display.
        let offered = [
            PresentMode::Immediate,
            PresentMode::Fifo,
            PresentMode::FifoRelaxed,
        ];

        assert_eq!(
            choose_present_mode(&offered, PresentPolicy::Uncapped),
            PresentMode::Immediate
        );
    }

    #[test]
    fn an_uncapped_policy_falls_back_to_mailbox_before_fifo() {
        let offered = [PresentMode::Fifo, PresentMode::Mailbox];

        assert_eq!(
            choose_present_mode(&offered, PresentPolicy::Uncapped),
            PresentMode::Mailbox,
            "Mailbox does not block the queue; Fifo does, and was asked not to"
        );
    }

    #[test]
    fn an_uncapped_policy_still_lands_on_fifo_when_nothing_else_is_offered() {
        // The honest fallback: there is no non-waiting mode here, so the surface
        // gets the one that always exists. `WindowTarget::present_mode` is what
        // tells a measurement it did not get what it asked for.
        let offered = [PresentMode::Fifo];

        assert_eq!(
            choose_present_mode(&offered, PresentPolicy::Uncapped),
            PresentMode::Fifo
        );
    }

    #[test]
    fn a_surface_without_fifo_at_all_still_yields_something() {
        // Neither policy's preference is on offer and neither is Fifo. Picking
        // the first is the only remaining choice, and it must not panic.
        let offered = [PresentMode::FifoRelaxed];

        assert_eq!(
            choose_present_mode(&offered, PresentPolicy::VSync),
            PresentMode::FifoRelaxed
        );
        assert_eq!(
            choose_present_mode(&offered, PresentPolicy::Uncapped),
            PresentMode::FifoRelaxed
        );
    }

    #[test]
    fn no_two_present_modes_share_a_name() {
        // That every mode *has* a name is the compiler's job, not this test's:
        // `present_mode_name` matches exhaustively with no wildcard arm, so a
        // mode without an arm is a build failure. What is checked here is the
        // half rustc cannot see - that no two of them collide. A measurement is
        // labelled with this string, and two modes sharing one would label a
        // table with a pacing it did not run under.
        let all = [
            PresentMode::Fifo,
            PresentMode::FifoRelaxed,
            PresentMode::Immediate,
            PresentMode::Mailbox,
            PresentMode::AutoVsync,
            PresentMode::AutoNoVsync,
        ];

        let mut names: Vec<&str> = all.iter().copied().map(present_mode_name).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();

        assert_eq!(names.len(), count, "two present modes share a name");
    }

    #[test]
    fn a_surface_that_offers_copy_src_is_configured_to_allow_reading_a_frame_back() {
        // What both platforms this project verifies on actually report, measured
        // in M6.4b rather than composed here.
        let offered = TextureUsages::COPY_SRC
            | TextureUsages::COPY_DST
            | TextureUsages::TEXTURE_BINDING
            | TextureUsages::STORAGE_BINDING
            | TextureUsages::RENDER_ATTACHMENT;

        assert_eq!(
            choose_usage(offered),
            TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC
        );
    }

    #[test]
    fn a_surface_offering_only_the_guaranteed_usage_is_configured_with_only_that() {
        // The one usage `wgpu` guarantees, and nothing else. Asking for COPY_SRC
        // here would be a validation error at configuration time, which would
        // cost the window rather than the screenshot.
        assert_eq!(
            choose_usage(TextureUsages::RENDER_ATTACHMENT),
            TextureUsages::RENDER_ATTACHMENT,
            "a screenshot is a convenience; a window that will not open is not"
        );
    }

    #[test]
    fn copy_src_without_render_attachment_is_not_taken_as_an_invitation() {
        // Nonsense a surface will not report, and the test is about the shape of
        // the check rather than the case: `contains` is asked for both flags at
        // once, so a partial offer cannot be read as a whole one. A check
        // written as `contains(COPY_SRC)` alone would return a usage this
        // surface never offered.
        assert_eq!(
            choose_usage(TextureUsages::COPY_SRC | TextureUsages::TEXTURE_BINDING),
            TextureUsages::RENDER_ATTACHMENT
        );
    }

    #[test]
    fn a_surface_that_cannot_be_read_says_so_and_says_what_to_do_instead() {
        // The one new message with no other test to carry its wording, because
        // reaching it needs a surface that offers only `RENDER_ATTACHMENT` and
        // neither platform this project verifies on has one. Constructed rather
        // than provoked, which is honest about what is being checked: the
        // sentence, not the path to it.
        let message = crate::RenderError::SurfaceNotReadable.to_string();

        assert!(
            message.contains("COPY_SRC"),
            "it must name the usage that is missing, or nobody can act on it: \
             {message}"
        );
        assert!(
            message.contains("--screenshot"),
            "and the thing to do instead, which needs no window: {message}"
        );
    }

    #[test]
    fn opaque_is_chosen_even_when_it_is_not_listed_first() {
        let offered = [
            CompositeAlphaMode::PreMultiplied,
            CompositeAlphaMode::Opaque,
        ];

        assert_eq!(choose_alpha_mode(&offered), CompositeAlphaMode::Opaque);
    }
}
