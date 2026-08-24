//! Rendering into a texture with no window attached, and getting the result
//! back as pixels.
//!
//! This is the foundation every later golden-image test stands on, and it was
//! built before there was any geometry to draw: a render path that cannot be
//! read back cannot be verified, and an unverifiable renderer is exactly what
//! this project is trying not to build.

use std::path::Path;

use crate::cascade::{CascadeKernel, CascadeStage, Emission, RadianceField};
use crate::compute::{self, FieldKernel};
use crate::error::RenderError;
use crate::field::{Field, FieldPair};
use crate::gpu;
use crate::hierarchy::{self, Cascade, DirectionalKernel, MergeForm};
use crate::march::{MarchHit, MarchKernel, Ray, derived_budget};
use crate::quad::QuadPipeline;
use crate::sdf::{self, SeedMap, Seeds};
use crate::sprite::{
    BatchOf, CameraView, MAX_SPRITES_PER_BATCH, Projection, SpriteBatch, SpriteFilter,
    SpriteInstance, SpritePlacement, batch_plan, batch_vertices,
};

/// Format of every render target this crate creates.
///
/// sRGB on purpose. Pairing it with an sRGB texture means a sampled byte is
/// decoded to linear and re-encoded on write, so it comes back out unchanged -
/// which is what lets a test assert on exact colours instead of on colour
/// management.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// A colour to clear the render target with.
///
/// Components are **linear** and run `0.0..=1.0`, matching what the GPU expects.
/// The render target is `Rgba8UnormSrgb`, so the GPU re-encodes these values on
/// write: `0.0` and `1.0` come back out as `0` and `255` exactly, while values
/// in between land wherever the sRGB transfer function puts them. Predicting
/// those on the CPU is not worth doing - render and read back instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClearColor {
    /// Red, linear.
    pub r: f64,
    /// Green, linear.
    pub g: f64,
    /// Blue, linear.
    pub b: f64,
    /// Alpha. Not sRGB-encoded, so it passes through unchanged.
    pub a: f64,
}

impl ClearColor {
    /// Opaque black, `[0, 0, 0, 255]` after read-back.
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    /// Opaque white, `[255, 255, 255, 255]` after read-back.
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    /// Opaque red, `[255, 0, 0, 255]` after read-back.
    pub const RED: Self = Self::new(1.0, 0.0, 0.0, 1.0);
    /// Opaque green, `[0, 255, 0, 255]` after read-back.
    pub const GREEN: Self = Self::new(0.0, 1.0, 0.0, 1.0);
    /// Opaque blue, `[0, 0, 255, 255]` after read-back.
    pub const BLUE: Self = Self::new(0.0, 0.0, 1.0, 1.0);

    /// Creates a clear colour from linear components.
    #[must_use]
    pub const fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }
}

/// An RGBA8 image that has been read back from the GPU.
///
/// Rows are tightly packed: `rgba().len()` is always `width * height * 4`, with
/// no padding left over from the transfer.
#[derive(Debug, Clone)]
pub struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Pixels {
    /// Builds an image from raw RGBA8 bytes, row-major from the top left.
    ///
    /// This is how a texture gets into the renderer without touching a file:
    /// generate or decode the bytes elsewhere, hand them over here. Loading from
    /// disk is `narvo-assets`' job, not this crate's.
    ///
    /// # Errors
    ///
    /// - [`RenderError::InvalidSize`] if a dimension is zero or above
    ///   [`OffscreenTarget::MAX_DIMENSION`].
    /// - [`RenderError::PixelBufferSize`] if `rgba` is not exactly
    ///   `width * height * 4` bytes long.
    ///
    /// # Examples
    ///
    /// ```
    /// use narvo_render2d::Pixels;
    ///
    /// // Two by two: red and green on the top row, blue and white below.
    /// let image = Pixels::from_rgba8(
    ///     2,
    ///     2,
    ///     vec![
    ///         255, 0, 0, 255, 0, 255, 0, 255, //
    ///         0, 0, 255, 255, 255, 255, 255, 255,
    ///     ],
    /// )?;
    ///
    /// assert_eq!(image.pixel(0, 0), Some([255, 0, 0, 255]));
    /// assert_eq!(image.pixel(1, 0), Some([0, 255, 0, 255]));
    /// assert_eq!(image.pixel(0, 1), Some([0, 0, 255, 255]));
    /// # Ok::<(), narvo_render2d::RenderError>(())
    /// ```
    pub fn from_rgba8(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, RenderError> {
        if width == 0
            || height == 0
            || width > OffscreenTarget::MAX_DIMENSION
            || height > OffscreenTarget::MAX_DIMENSION
        {
            return Err(RenderError::InvalidSize {
                width,
                height,
                max: OffscreenTarget::MAX_DIMENSION,
            });
        }

        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(RenderError::PixelBufferSize {
                width,
                height,
                expected,
                actual: rgba.len(),
            });
        }

        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The tightly packed RGBA8 bytes, row-major from the top left.
    #[must_use]
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// The pixel at `(x, y)`, or `None` if it lies outside the image.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }

        let start = (y as usize * self.width as usize + x as usize) * 4;
        Some([
            self.rgba[start],
            self.rgba[start + 1],
            self.rgba[start + 2],
            self.rgba[start + 3],
        ])
    }

    /// Writes the image to `path` as a PNG, creating or truncating the file.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::PngWrite`] if the file cannot be created or the
    /// encoder rejects the buffer.
    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<(), RenderError> {
        let path = path.as_ref();

        image::save_buffer(
            path,
            &self.rgba,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| RenderError::PngWrite {
            path: path.to_path_buf(),
            source: Box::new(error),
        })
    }
}

/// The backends the **offscreen** instance asks for, before `WGPU_BACKEND` speaks.
///
/// wgpu's **primary** set, and deliberately *not*
/// [`crate::window::window_backends`]. Every blessed reference in this repository
/// is produced through this instance, so this is the substrate all twelve of them
/// live on; M7.1d moved the window off Vulkan on Windows and left this exactly
/// where it was, which is the entire reason not one of the twelve moved.
///
/// **It said `Backends::default()` from M1 until M8.2** — that is, `all()`, which
/// carries wgpu's second tier as well. M8.2 narrowed it to `PRIMARY` and measured
/// first that the narrowing moves nothing: the adapter chosen on either platform
/// is unchanged, so the twelve did not move for this either. ADR-0048 carries the
/// decision; the body below carries the numbers.
///
/// A function rather than an inline default so that the two choices are two named
/// things a test can hold apart. What such a test cannot do is stop somebody
/// editing the call site, so it is joined by one that reads this file's source.
#[must_use]
pub(crate) fn offscreen_backends() -> wgpu::Backends {
    // `PRIMARY`, not `default()`. The two are not the same and the difference is
    // GL: `impl Default for Backends` returns `Self::all()`
    // (`wgpu-types-30.0.0/src/backend.rs:140-144`), and `Backends::SECONDARY` —
    // which `all()` includes and `PRIMARY` does not — is exactly `GL`, described
    // there as "the apis that wgpu offers second tier of support for. These may
    // be unsupported/still experimental" (`:132-136`).
    //
    // **Measured, not inferred.** The M8.2 probe found the GL backend answering
    // on both platforms this project verifies on — AMD's OpenGL driver on
    // Windows, llvmpipe under WSL — and behaving differently from the others on
    // the same machine: it refuses `Rg32Float` as a storage texture where Vulkan
    // and DX12 accept it, and lavapipe's GL refuses `Rgba32Float` as a render
    // attachment where its own Vulkan accepts it. A reference produced on GL
    // would be a reference produced by a rasteriser nothing else here checks
    // against.
    //
    // **It changes nothing today, which was measured before the change**: the
    // ladder's first rung asks for a high-performance adapter and got
    // `AMD Radeon RX 9070 XT [Vulkan, DiscreteGpu]` on Windows and
    // `llvmpipe [Vulkan, Cpu]` under WSL. GL was never being chosen; it was
    // merely reachable, and "reachable by nobody's decision" is what ADR-0048 is
    // about.
    //
    // `WGPU_BACKEND` still overrides this, through `with_env` at the call site.
    wgpu::Backends::PRIMARY
}

/// A GPU render target with no window or surface behind it.
///
/// Owns its own device and queue, so a process can hold one without having
/// opened a window - which is what makes rendering testable in CI and scriptable
/// in a headless run.
///
/// # Examples
///
/// ```
/// use narvo_render2d::{ClearColor, OffscreenTarget, RenderError};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let target = match OffscreenTarget::new(64, 64) {
///     Ok(target) => target,
///     // A machine with neither a GPU nor a software rasteriser cannot run
///     // this. Reporting that beats failing over something the caller did not
///     // do wrong.
///     Err(RenderError::NoAdapter { .. }) => return Ok(()),
///     Err(other) => return Err(other.into()),
/// };
///
/// let pixels = target.render_clear(ClearColor::RED)?;
///
/// assert_eq!(pixels.width(), 64);
/// assert_eq!(pixels.rgba().len(), 64 * 64 * 4);
/// assert_eq!(pixels.pixel(0, 0), Some([255, 0, 0, 255]));
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct OffscreenTarget {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// The multisampled attachment every pass draws into, resolved into
    /// [`Self::view`] when the pass ends.
    ///
    /// Held rather than built per pass: it is the largest allocation this type
    /// makes — [`crate::SAMPLE_COUNT`] times the target — and rebuilding it on
    /// every frame would put that allocation on the frame path. Its view is
    /// kept beside it for the same reason.
    multisampled: wgpu::Texture,
    multisampled_view: wgpu::TextureView,
    width: u32,
    height: u32,
    adapter_summary: String,
    /// Built eagerly so a broken shader fails here rather than on the first
    /// draw. Compiling it costs a few milliseconds once per target.
    quad: QuadPipeline,
}

impl OffscreenTarget {
    /// Largest width or height this crate will create.
    ///
    /// Matches `max_texture_dimension_2d` in wgpu's default limits, which is
    /// what [`OffscreenTarget::new`] asks the device for. Requesting more would
    /// be rejected by the driver rather than by us, and much later.
    ///
    /// Checked against the pinned version rather than remembered (F5 in
    /// `ProjektPlan.md` §12, which named this claim as unsourced):
    /// `wgpu-types-30.0.0/src/limits.rs:424` sets `max_texture_dimension_2d:
    /// 8192` inside `Limits::defaults()`, and `Cargo.lock` pins `wgpu-types` at
    /// `30.0.0`. A version bump can move it, and then this constant and that
    /// line have to be compared again.
    pub const MAX_DIMENSION: u32 = 8192;

    /// Creates a `width` x `height` `Rgba8UnormSrgb` target on the best adapter
    /// this machine offers.
    ///
    /// Adapter selection deliberately does not insist on a discrete GPU. It asks
    /// for a high-performance adapter, then for any adapter at all, then forces
    /// a software fallback, and only gives up if all three fail. Insisting on
    /// real hardware would make every machine without it - CI runners first
    /// among them - fail outright, and the point of this type is to be usable
    /// exactly there.
    ///
    /// # Errors
    ///
    /// - [`RenderError::InvalidSize`] if a dimension is zero or above
    ///   [`MAX_DIMENSION`](Self::MAX_DIMENSION). This is checked before any GPU
    ///   work, so it costs nothing on a machine with no adapter.
    /// - [`RenderError::NoAdapter`] if none of the three requests found one.
    /// - [`RenderError::NoDevice`] if an adapter was found but refused a device.
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        Self::with_format(width, height, TARGET_FORMAT)
    }

    /// The same target, built for an arbitrary colour format.
    ///
    /// Internal, and it exists for one reason: the window path builds its
    /// pipeline for whatever format the surface offers, which on many machines
    /// is `Bgra8UnormSrgb` rather than the `Rgba8UnormSrgb` used here. Being
    /// able to render through that format without a window is what lets a test
    /// catch a channel swap that would otherwise only ever appear on somebody
    /// else's display.
    pub(crate) fn with_format(
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self, RenderError> {
        if width == 0 || height == 0 || width > Self::MAX_DIMENSION || height > Self::MAX_DIMENSION
        {
            return Err(RenderError::InvalidSize {
                width,
                height,
                max: Self::MAX_DIMENSION,
            });
        }

        // `with_env` honours WGPU_BACKEND and friends, so a headless runner can
        // be pointed at a specific backend (Vulkan for lavapipe, say) without a
        // code change. No display handle: there is no window to attach to.
        //
        // The backend set is spelled out through `offscreen_backends` rather than
        // taken implicitly, so that the *offscreen* choice is a named thing since
        // M7.1d — the window's is now different (`window::window_backends`), and
        // every blessed reference in this repository is drawn through this
        // instance and not that one.
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = offscreen_backends();
        let instance = wgpu::Instance::new(descriptor.with_env());

        // No compatible surface to satisfy: this target never presents.
        let (selection, adapter) = gpu::select_adapter(&instance, None)?;
        let adapter_summary = gpu::summarize(&adapter, selection);
        let (device, queue) = gpu::create_device(&adapter)?;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("narvo offscreen target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // No COPY_SRC: nothing ever reads the samples themselves, only the
        // resolve. RENDER_ATTACHMENT is the whole usage.
        let multisampled = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("narvo offscreen multisample attachment"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: crate::SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let multisampled_view = multisampled.create_view(&wgpu::TextureViewDescriptor::default());

        let quad = QuadPipeline::new(&device, format);

        Ok(Self {
            device,
            queue,
            texture,
            view,
            multisampled,
            multisampled_view,
            width,
            height,
            adapter_summary,
            quad,
        })
    }

    /// Nominal size of the multisample attachment: width times height times
    /// sample count times bytes per texel, read off the descriptor.
    ///
    /// The price of [`crate::SAMPLE_COUNT`], stated in the one unit anybody
    /// budgets in. Reported rather than derived by the caller so that a change
    /// to the sample count moves the number without anybody remembering to.
    ///
    /// **Nominal, not observed.** What a driver actually allocates for a
    /// multisampled texture is implementation-defined, and nothing in this crate
    /// asks it.
    #[must_use]
    pub fn multisample_bytes(&self) -> u64 {
        // `block_copy_size`, not `target_pixel_byte_cost`. The latter is
        // WebGPU's *attachment budget* figure and charges an 8-bit-per-channel
        // format 8 bytes rather than 4 - wgpu notes the discrepancy itself,
        // "Despite being 4 bytes per pixel, these are 8 bytes per pixel in the
        // table" (`wgpu-types-30.0.0/src/texture/format.rs:1334`), and gives no
        // reason; why the WebGPU table does is uncertain. This method reports a
        // size in bytes, so it wants the copy size (`:1215`, against `:1312`).
        let bytes_per_sample: u64 = self
            .multisampled
            .format()
            .block_copy_size(None)
            .expect("every format this crate renders into has a block copy size")
            .into();
        u64::from(self.multisampled.width())
            * u64::from(self.multisampled.height())
            * u64::from(self.multisampled.sample_count())
            * bytes_per_sample
    }

    /// Width of the target in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of the target in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Human-readable description of the adapter in use and how it was picked.
    ///
    /// Worth logging from a test or a headless run: "the image differs" and "the
    /// image was produced by a completely different rasteriser" look identical
    /// until somebody prints this.
    #[must_use]
    pub fn adapter_summary(&self) -> &str {
        &self.adapter_summary
    }

    /// A `width` x `height` [`Field`] on this target's device.
    ///
    /// The one way a field comes into being, because a field needs a device and
    /// this type is what owns one. Nothing about the target's own texture is
    /// involved: a field is its own allocation, in its own format, and drawing
    /// into it is not possible on purpose (`field.rs`'s `FIELD_USAGE`).
    ///
    /// **Carried an `expect(dead_code)` from M8.2 until M8.5a, and the compiler
    /// took it away.** M8.3a's reason was that a chain needs the *pair*, so
    /// [`Self::distance_field`] goes through [`Self::field_pair`] and never
    /// through here — true, and it stayed true. What arrived instead is a caller
    /// that wants **one** field and never alternates: [`Self::cascade_stage`]
    /// uploads an emission map, which is read by every pass and written by none,
    /// so a ping-pong would be a second texture nothing ever writes.
    ///
    /// Worth keeping in view, because it is the second time this crate's
    /// `expect`-not-`allow` habit has paid: the day M8.5a landed, the compiler
    /// had an opinion about which of these attributes were still true, and this
    /// one was not.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidSize`] as for [`Self::new`].
    pub(crate) fn field(&self, width: u32, height: u32, label: &str) -> Result<Field, RenderError> {
        Field::new(&self.device, &self.queue, width, height, label)
    }

    /// Two fields to alternate between, for a chain of passes.
    ///
    /// **Carried an `expect(dead_code)` from M8.2 until M8.3a, and the compiler
    /// took it away**: [`Self::distance_field`] is the production caller M8.2
    /// named, and it wants the pair rather than a single field.
    ///
    /// # Errors
    ///
    /// As [`Self::field`].
    pub(crate) fn field_pair(
        &self,
        width: u32,
        height: u32,
        label: &str,
    ) -> Result<FieldPair, RenderError> {
        FieldPair::new(&self.device, &self.queue, width, height, label)
    }

    /// The transport kernel, compiled on this target's device.
    ///
    /// M8.2 ships exactly one compute kernel and this is it; M8.3a's jump
    /// flooding is the second, and it arrived beside this one exactly as
    /// predicted — [`Self::distance_field`] builds its own `FieldKernel` from
    /// `sdf::JUMP_FLOOD_WGSL` rather than taking a `&str` of WGSL through a
    /// parameter, because that parameter would be a knob with one setting today
    /// and a way to compile arbitrary shaders tomorrow.
    ///
    /// **Which is why this one is still dead, and will stay dead.** M8.2's
    /// reason read "the multi-pass machinery precedes its first caller: M8.3's
    /// jump flooding" — but jump flooding compiles its own kernel, so no caller
    /// of *this* one was ever going to arrive from that direction. It is the
    /// transport oracle, its callers are `compute.rs`'s tests, and the corrected
    /// reason below says so instead of naming a caller that cannot come.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the transport kernel is M8.2's oracle: jump flooding compiles its own, so only tests want this one"
        )
    )]
    pub(crate) fn transport_kernel(&self) -> FieldKernel {
        FieldKernel::new(
            &self.device,
            &self.queue,
            "narvo transport",
            compute::TRANSPORT_WGSL,
            compute::TRANSPORT_ENTRY,
        )
    }

    /// For every texel of `seeds`, the nearest seeded texel.
    ///
    /// **The first production caller of M8.2's multi-pass machinery**, and the
    /// whole of what M8.3a ships: a seed set goes in, a [`SeedMap`] comes out,
    /// and no `wgpu` type is involved on either side.
    ///
    /// # What it does
    ///
    /// Seeds the read half of a `FieldPair` with `seeds`, runs one pass per entry
    /// of `sdf::jump_flood_steps` — descending powers of two from the largest
    /// below the longer side down to one — and reads the result back, turning it
    /// into a `SeedMap`. The comparison inside the kernel is in integer
    /// arithmetic, which M8.3a decided by measurement rather than by preference;
    /// `shaders/jump_flood.wgsl`'s header carries the numbers.
    ///
    /// Jump flooding is an **approximation**, and `sdf.rs`'s header carries how
    /// far off it was measured to be: exact on four of five arrangements, and 27
    /// of 16 384 texels on a rasterised ring, none of them naming a seed more
    /// than 0.2425 texels farther than the nearest.
    ///
    /// # What it costs
    ///
    /// A compute pipeline is compiled and a field pair is allocated **on every
    /// call**. At 1920 x 1080 that pair is 66.4 MB (`field.rs`'s `FIELD_FORMAT`).
    /// Right for a caller that computes a field once; wrong for one that computes
    /// it every frame — and M8.3b is the task that will know which it is. The
    /// reopening is a compiled-pass object holding both, which is a larger public
    /// surface and so is not built on a guess.
    ///
    /// # Errors
    ///
    /// [`RenderError::Readback`] if the copy back to the CPU fails. That is the
    /// only one of the three this function threads that can actually occur, and
    /// the other two are named rather than unwrapped:
    ///
    /// - [`RenderError::InvalidSize`] **cannot** occur. [`Seeds::new`] rejects a
    ///   dimension outside `1..=MAX_DIMENSION`, which is the identical condition
    ///   `Field::new` applies, and `Seeds`' fields are private — so a `Seeds` a
    ///   caller can hold is always a size a field can be.
    /// - [`RenderError::FieldTexelCount`] **cannot** occur either: the buffer and
    ///   the pair are built from the same two numbers.
    ///
    /// Both are threaded rather than unwrapped because each is an invariant of
    /// *this function's* two call sites rather than of the types, so an `expect`
    /// here would be a claim that the next edit could quietly falsify.
    pub fn distance_field(&self, seeds: &Seeds) -> Result<SeedMap, RenderError> {
        let pair = self.flood(seeds)?;
        Ok(SeedMap::from_texels(
            seeds.width(),
            seeds.height(),
            &pair.read().read_back()?,
        ))
    }

    /// Seeds a field pair and floods it, leaving the answer on the GPU.
    ///
    /// The half [`Self::distance_field`] and [`Self::march`] share. It is
    /// private because a `FieldPair` is a `wgpu` resource and this crate's
    /// boundary says none crosses out — which is also why `march` exists at all
    /// rather than a caller being handed a field to march itself.
    fn flood(&self, seeds: &Seeds) -> Result<FieldPair, RenderError> {
        let width = seeds.width();
        let height = seeds.height();
        let mut pair = self.field_pair(width, height, "narvo distance field")?;
        pair.read().write(&seeds.texels())?;

        let kernel = FieldKernel::new(
            &self.device,
            &self.queue,
            "narvo jump flood",
            sdf::JUMP_FLOOD_WGSL,
            sdf::JUMP_FLOOD_ENTRY,
        );
        let steps = sdf::jump_flood_steps(width, height);
        kernel.run(&mut pair, &steps)?;
        Ok(pair)
    }

    /// Marches every ray in `rays` against the field `seeds` describes.
    ///
    /// **The field never comes to the CPU.** M8.4 measured `distance_field`'s
    /// phases and found the eleven flooding passes to be 0.29 ms of 21.2 ms at
    /// 1920 x 1080 while uploading, reading back and rebuilding the field cost
    /// 16.9 ms — so a march that runs where the field already is pays none of
    /// that, and what crosses back is sixteen bytes a ray. `march.rs`'s header
    /// carries the table.
    ///
    /// The step budget is [derived](Ray::derived_budget) from the rays: a step is
    /// either zero, which ends the march, or at least one fixed-point unit, so no
    /// ray can take more steps than its own length in those units. A caller that
    /// wants to spend less asks [`Self::march_within`].
    ///
    /// # Errors
    ///
    /// As [`Self::march_within`].
    pub fn march(&self, seeds: &Seeds, rays: &[Ray]) -> Result<Vec<MarchHit>, RenderError> {
        self.march_within(seeds, rays, derived_budget(rays))
    }

    /// [`Self::march`] with a step budget of the caller's choosing.
    ///
    /// A ray that runs out reports [`MarchVerdict::Exhausted`](crate::MarchVerdict::Exhausted),
    /// which is **not** visible: a march that stopped early has not established a
    /// line of sight, and saying otherwise would be claiming something it never
    /// checked.
    ///
    /// The consumer is M8.5's cascade, which marches a probe's rays by the
    /// million and will want to bound the work; the budget is not a knob with one
    /// setting, because M8.4's own exhaustion oracle is the other caller.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidSize`] if the seed set's dimensions are ones a field
    /// cannot have, and [`RenderError::Readback`] if the answers cannot be copied
    /// back. A ray is validated when it is built, not here.
    pub fn march_within(
        &self,
        seeds: &Seeds,
        rays: &[Ray],
        budget: u32,
    ) -> Result<Vec<MarchHit>, RenderError> {
        if rays.is_empty() {
            return Ok(Vec::new());
        }
        let pair = self.flood(seeds)?;
        let kernel = MarchKernel::new(&self.device, &self.queue);
        kernel.run(pair.read(), rays, budget)
    }

    /// Runs one cascade stage: marches every probe's directions and integrates.
    ///
    /// **M8.5a's capability, and two compute passes in one queue.** The first is
    /// M8.4's march, unchanged; the second is `cascade.wgsl`'s integration, which
    /// reads the hits where the march left them. The hits never come to the CPU,
    /// so the stage pays the marshalling `march.rs`'s header measured away.
    ///
    /// # What a probe answers with
    ///
    /// The mean over its directions of what each one found: the emission of the
    /// occluder it stopped on, or [`StageLayout::far_radiance`] if it reached the
    /// far end of the interval having met nothing. A direction that ran out of
    /// steps contributes nothing and is counted as neither.
    ///
    /// The emission is read **at the seed**, not at the point the march stopped:
    /// a march stops up to a texel short of what stopped it, so the stopping
    /// texel is often the empty one in front of the lamp. `cascade.wgsl` carries
    /// the derivation.
    ///
    /// # What it costs
    ///
    /// A field pair, an emission field, a radiance field, three compiled
    /// pipelines and a ray buffer, **on every call** — the same shape
    /// [`Self::distance_field`] has and for the same reason. A ray is 32 bytes
    /// and a hit is 16, so the buffers are 48 bytes times
    /// [`CascadeStage::ray_count`]; the ceiling is
    /// [`CascadeStage::MAX_RAYS`], which is derived from what one dispatch
    /// reaches rather than chosen.
    ///
    /// # Errors
    ///
    /// - [`RenderError::EmissionSizeMismatch`] if the emission map is not the
    ///   seed set's size. The two are indexed by one coordinate, so this is a
    ///   confusion rather than a shortfall and is refused rather than padded.
    /// - [`RenderError::ProbeOutsideField`] if a probe stands closer to an edge
    ///   than its own near end.
    /// - [`RenderError::Readback`] if the radiance cannot be copied back.
    /// - [`RenderError::InvalidSize`] and [`RenderError::FieldTexelCount`] are
    ///   threaded and cannot occur: every size involved comes from a `Seeds`, an
    ///   `Emission` or a `CascadeStage`, each of which refused an impossible one
    ///   when it was built. They are threaded rather than unwrapped for
    ///   `distance_field`'s reason — the invariant belongs to this function's
    ///   call sites rather than to the types.
    pub fn cascade_stage(
        &self,
        seeds: &Seeds,
        emission: &Emission,
        stage: &CascadeStage,
    ) -> Result<RadianceField, RenderError> {
        let (width, height) = (seeds.width(), seeds.height());
        if emission.width() != width || emission.height() != height {
            return Err(RenderError::EmissionSizeMismatch {
                seed_width: width,
                seed_height: height,
                emission_width: emission.width(),
                emission_height: emission.height(),
            });
        }
        stage.check_fits(width, height)?;

        let rays = stage.rays(width, height)?;
        let pair = self.flood(seeds)?;

        let emitter = self.field(width, height, "narvo cascade emission")?;
        emitter.write(emission.texels())?;

        let march = MarchKernel::new(&self.device, &self.queue);
        let (ray_buffer, hit_buffer) = march.dispatch(pair.read(), &rays, derived_budget(&rays));

        // A stage that stands alone has no level above it, so an escaping
        // direction carries `far_radiance` and the upper binding is a texel
        // nothing reads. It is bound rather than made optional because a bind
        // group layout is one shape.
        let no_upper = self.field(1, 1, "narvo cascade no upper")?;
        CascadeKernel::new(&self.device, &self.queue).run(
            pair.read(),
            &emitter,
            &ray_buffer,
            &hit_buffer,
            stage,
            &no_upper,
            false,
        )
    }

    /// Runs a whole cascade and returns level zero's composed radiance.
    ///
    /// **M8.5b's capability.** Levels run **top down**: the top takes
    /// [`CascadeLayout::sky`](crate::CascadeLayout::sky), and every level below
    /// takes the composed radiance of the level above as what an escaping
    /// direction carries. That ordering is not a preference — it is what lets the
    /// composition be written without `escaped * upper`, whose product is
    /// inexact and would fire ADR-0051's reopening. `hierarchy.rs`'s header
    /// carries the argument.
    ///
    /// The distance field is flooded **once** and every level marches against
    /// it, which is the one cost a cascade does not pay per level.
    ///
    /// # The two forms
    ///
    /// [`MergeForm::Aggregate`](crate::MergeForm::Aggregate) keeps one radiance
    /// per probe and applies it to every escaping direction;
    /// [`MergeForm::Directional`](crate::MergeForm::Directional) keeps one per
    /// probe per direction and gives each escaping direction the four upper
    /// directions covering its own arc. **Both are offered and neither is
    /// preferred** — [`Cascade::budget`](crate::Cascade::budget) is what they
    /// cost and M8.5b's report is what they differ by.
    ///
    /// # Errors
    ///
    /// - [`RenderError::EmissionSizeMismatch`] if the emission map is not the
    ///   seed set's size.
    /// - [`RenderError::InvalidSize`] if the cascade was validated against a
    ///   field of another size than the one handed in here.
    /// - [`RenderError::CascadeLevelTooLarge`] if a directional level exceeds
    ///   one storage buffer binding.
    /// - [`RenderError::Readback`] if the radiance cannot be copied back.
    pub fn cascade(
        &self,
        seeds: &Seeds,
        emission: &Emission,
        cascade: &Cascade,
        form: MergeForm,
    ) -> Result<RadianceField, RenderError> {
        let (width, height) = (seeds.width(), seeds.height());
        if emission.width() != width || emission.height() != height {
            return Err(RenderError::EmissionSizeMismatch {
                seed_width: width,
                seed_height: height,
                emission_width: emission.width(),
                emission_height: emission.height(),
            });
        }
        if cascade.field() != [width, height] {
            return Err(RenderError::InvalidSize {
                width,
                height,
                max: Self::MAX_DIMENSION,
            });
        }
        cascade.check_form(form)?;

        let pair = self.flood(seeds)?;
        let emitter = self.field(width, height, "narvo cascade emission")?;
        emitter.write(emission.texels())?;
        let march = MarchKernel::new(&self.device, &self.queue);

        match form {
            MergeForm::Aggregate => {
                self.cascade_aggregate(cascade, pair.read(), &emitter, &march, width, height)
            }
            MergeForm::Directional => {
                self.cascade_directional(cascade, pair.read(), &emitter, &march, width, height)
            }
        }
    }

    /// The aggregate merge: one radiance per probe, carried down as one value.
    fn cascade_aggregate(
        &self,
        cascade: &Cascade,
        field: &Field,
        emitter: &Field,
        march: &MarchKernel,
        width: u32,
        height: u32,
    ) -> Result<RadianceField, RenderError> {
        let kernel = CascadeKernel::new(&self.device, &self.queue);
        let mut upper: Option<Field> = None;
        for level in (0..cascade.level_count()).rev() {
            let stage = cascade.level(level).expect("a level below the count");
            let rays = stage.rays(width, height)?;
            let (ray_buffer, hit_buffer) = march.dispatch(field, &rays, derived_budget(&rays));
            let placeholder;
            let bound = match upper.as_ref() {
                Some(field) => field,
                None => {
                    placeholder = self.field(1, 1, "narvo cascade no upper")?;
                    &placeholder
                }
            };
            let composed = kernel.run_into_field(
                field,
                emitter,
                &ray_buffer,
                &hit_buffer,
                stage,
                bound,
                upper.is_some(),
            )?;
            let layout = stage.layout();
            if level == 0 {
                // Only level zero crosses to the CPU. Every level above it is
                // read by the level below, on the GPU, which is the whole reason
                // `run_into_field` exists.
                return Ok(RadianceField::from_texels(
                    layout.probes[0],
                    layout.probes[1],
                    composed.read_back()?,
                ));
            }
            upper = Some(composed);
        }
        unreachable!("a cascade has at least one level, so level zero was reached")
    }

    /// The directional merge: one radiance per probe per direction.
    fn cascade_directional(
        &self,
        cascade: &Cascade,
        field: &Field,
        emitter: &Field,
        march: &MarchKernel,
        width: u32,
        height: u32,
    ) -> Result<RadianceField, RenderError> {
        let kernel = DirectionalKernel::new(&self.device, &self.queue);
        let mut upper: Option<wgpu::Buffer> = None;
        for level in (0..cascade.level_count()).rev() {
            let stage = cascade.level(level).expect("a level below the count");
            let layout = stage.layout();
            let rays = stage.rays(width, height)?;
            let (ray_buffer, hit_buffer) = march.dispatch(field, &rays, derived_budget(&rays));

            let entries = u64::from(stage.ray_count());
            let outgoing = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("narvo cascade directional radiance"),
                size: entries * hierarchy::ENTRY_BYTES,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let placeholder;
            let bound = match upper.as_ref() {
                Some(buffer) => buffer,
                None => {
                    placeholder = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("narvo cascade no upper"),
                        size: hierarchy::ENTRY_BYTES,
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    });
                    &placeholder
                }
            };
            let mean = Field::new(
                &self.device,
                &self.queue,
                layout.probes[0],
                layout.probes[1],
                "narvo cascade directional mean",
            )?;
            let upper_grid = upper.as_ref().map(|_| {
                let above = cascade
                    .level(level + 1)
                    .expect("a level above exists whenever an upper buffer does")
                    .layout();
                above.probes
            });
            kernel.run(
                field,
                emitter,
                &ray_buffer,
                &hit_buffer,
                &mean,
                bound,
                &outgoing,
                &stage.params(width, height, upper_grid),
                stage.probe_count(),
            );
            if level == 0 {
                return Ok(RadianceField::from_texels(
                    layout.probes[0],
                    layout.probes[1],
                    mean.read_back()?,
                ));
            }
            upper = Some(outgoing);
        }
        unreachable!("a cascade has at least one level, so level zero was reached")
    }

    /// Copies whatever the target currently holds back to the CPU.
    ///
    /// **The counterpart to [`WindowTarget::read_back`](crate::WindowTarget::read_back),
    /// and the point is that the two are now the same shape.** A windowed frame
    /// is drawn, then optionally copied, then handed over; before M8.2 the
    /// offscreen path had no such seam — every `render_*` method drew *and*
    /// copied in one call, so there was no moment between the two in which
    /// anything else could be encoded. A lighting chain needs exactly that
    /// moment.
    ///
    /// It reads the resolve target, which is the same texture every `render_*`
    /// method returns pixels from, so calling this straight after one of them
    /// hands back the identical image.
    ///
    /// # Errors
    ///
    /// [`RenderError::Readback`] if the GPU never finishes the copy or the
    /// transfer buffer cannot be mapped.
    pub fn read_back(&self) -> Result<Pixels, RenderError> {
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo offscreen read-back"),
            });
        self.finish_and_read_back(encoder)
    }

    /// Draws exactly what [`Self::render_sprites_over`] draws, and reads nothing
    /// back.
    ///
    /// The draw half of the seam above. `render_sprites_over` is this followed by
    /// [`Self::read_back`] — literally, since M8.2: both go through
    /// `render_batches`, which now returns after the submit and leaves the copy
    /// to its caller.
    ///
    /// # Errors
    ///
    /// [`RenderError::BatchTooLarge`] if the **sum** of both batches exceeds
    /// [`MAX_SPRITES_PER_BATCH`].
    pub fn draw_sprites_over(
        &self,
        image: &Pixels,
        sprites: &[SpriteInstance],
        overlay: Option<SpriteBatch<'_>>,
        camera: CameraView,
    ) -> Result<(), RenderError> {
        self.render_batches(image, sprites, overlay, camera)
    }

    /// Clears the target to `color` and reads the result back as pixels.
    ///
    /// Draws nothing. Useful as a background on its own, and as the simplest
    /// possible check that a device is alive and the read-back path works -
    /// which is what it was built for before there was any geometry.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Readback`] if the GPU never finishes the copy or
    /// the transfer buffer cannot be mapped.
    pub fn render_clear(&self, color: ClearColor) -> Result<Pixels, RenderError> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo offscreen clear"),
            });

        // The pass does nothing but clear and store. Dropping it immediately is
        // what ends the pass and commits that store.
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("narvo clear pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                // Multisampled attachment, resolved into the readable texture.
                // A clear writes the same colour to every sample, so a
                // clear-only pass resolves to that colour. Byte-for-byte is
                // checked only for channel values 0.0 and 1.0, by
                // `every_pixel_of_a_cleared_target_carries_the_clear_colour`;
                // for other values the sRGB round trip through the resolve is
                // unchecked.
                view: &self.multisampled_view,
                depth_slice: None,
                resolve_target: Some(&self.view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                        a: color.a,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        }));

        self.finish_and_read_back(encoder)
    }

    /// Draws `image` across the whole target and reads the result back.
    ///
    /// The quad fills the target, so every output pixel is a sample of `image`.
    /// Placing or transforming the quad is a later milestone; what this pins
    /// down now is the orientation convention - texture origin top left, output
    /// origin top left - and that a texture survives the trip through the
    /// pipeline unchanged.
    ///
    /// The sampler is nearest-neighbour, so no colour is ever a blend of two
    /// texels and a given output pixel maps to exactly one texel.
    ///
    /// # Errors
    ///
    /// Returns [`RenderError::Readback`] if the GPU never finishes the copy or
    /// the transfer buffer cannot be mapped.
    ///
    /// # Examples
    ///
    /// ```
    /// use narvo_render2d::{OffscreenTarget, Pixels, RenderError};
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let target = match OffscreenTarget::new(64, 64) {
    ///     Ok(target) => target,
    ///     // No GPU and no software rasteriser on this machine.
    ///     Err(RenderError::NoAdapter { .. }) => return Ok(()),
    ///     Err(other) => return Err(other.into()),
    /// };
    ///
    /// // A two by two texture: red top left, green top right, blue bottom
    /// // left, white bottom right.
    /// let texture = Pixels::from_rgba8(
    ///     2,
    ///     2,
    ///     vec![
    ///         255, 0, 0, 255, 0, 255, 0, 255, //
    ///         0, 0, 255, 255, 255, 255, 255, 255,
    ///     ],
    /// )?;
    ///
    /// let output = target.render_textured_quad(&texture)?;
    ///
    /// // Each texel covers one quarter of the output, corner for corner.
    /// assert_eq!(output.pixel(0, 0), Some([255, 0, 0, 255]));
    /// assert_eq!(output.pixel(63, 0), Some([0, 255, 0, 255]));
    /// assert_eq!(output.pixel(0, 63), Some([0, 0, 255, 255]));
    /// # Ok(())
    /// # }
    /// ```
    pub fn render_textured_quad(&self, image: &Pixels) -> Result<Pixels, RenderError> {
        // The M1 single-quad path draws one screen-filling quad and has no sprite,
        // so no wish: `Nearest`, named rather than defaulted.
        let bindings = self.quad.bind_texture(
            &self.device,
            &self.queue,
            image.width(),
            image.height(),
            image.rgba(),
        );
        let bind_group = bindings.for_filter(SpriteFilter::Nearest);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo offscreen quad"),
            });

        self.quad.encode_pass(
            &mut encoder,
            &self.multisampled_view,
            &self.view,
            bind_group,
        );

        self.finish_and_read_back(encoder)
    }

    /// Draws `image` as one sprite placed by `placement`, on black.
    ///
    /// Exactly one sprite. Drawing several is batching, which is a later
    /// milestone and has to leave this image unchanged to be believed — so this
    /// method deliberately offers no way to pass a second one.
    ///
    /// The placement is in world units, which under [`Projection`] are the
    /// target's pixels with the origin at its centre. A sprite scaled to
    /// `(32.0, 32.0)` on a 64 x 64 target therefore covers the middle half of
    /// it, and which texel lands on which pixel can be worked out before
    /// anything runs.
    ///
    /// The whole texture, not a region of it: this is
    /// [`SpriteInstance::whole_texture`], which since M3.9 is an ordinary
    /// [`TextureRegion`](crate::TextureRegion) whose edges are the texture's
    /// own rather than a second code path. A caller that wants a region uses
    /// [`Self::render_sprites`].
    ///
    /// # Errors
    ///
    /// [`RenderError::Readback`] if the copy back from the GPU fails, as for
    /// every other `render_*` method.
    pub fn render_sprite(
        &self,
        image: &Pixels,
        placement: SpritePlacement,
    ) -> Result<Pixels, RenderError> {
        self.render_sprites(image, &[SpriteInstance::whole_texture(placement)])
    }

    /// Draws every sprite in `sprites` from `image`, in one render pass and
    /// one draw call per run of equal sampler wish (`batch_runs`).
    ///
    /// This is the only path that draws a sprite; [`Self::render_sprite`] is a
    /// call to it with a slice of one and has no drawing code of its own. That
    /// matters more than the ergonomics: the blessed golden image of M3.5,
    /// `placed_sprite_quadrants_128x128`, is rendered through this, so breaking
    /// it turns the image red rather than leaving it green over an unused path.
    ///
    /// **Through this and not through [`Self::render_sprite`], since M3.27.**
    /// Until then that image was the only *blessed reference* reached through
    /// `render_sprite` — never its only caller, of which there are seven others
    /// in `sprite_placement.rs` and `narvo-app/tests/transform_to_sprite.rs`.
    /// Converting the scene to `Linear` (D13) moved its call here, because the
    /// wish is a field of [`SpriteInstance`] and `render_sprite` builds its own with
    /// [`SpriteInstance::whole_texture`], which is `Nearest`. So `render_sprite` keeps
    /// those derived pixel probes and no longer has a blessed image behind it.
    /// Nothing about the drawing changed with the move: `render_sprite` has no
    /// drawing code, it is the one-line call below, so both entry points always
    /// produced their vertices from the same `batch_vertices` call in this
    /// function.
    ///
    /// **The other blessed image does not come through here.**
    /// `textured_quad_quadrants_64x64` is rendered by
    /// [`Self::render_textured_quad`], which binds the pipeline's own
    /// screen-filling quad and never calls `batch_vertices`. Until M3.9 this
    /// paragraph claimed both images, which made the two-path argument of M3.6
    /// — one image moves, the other stays green *because* it is a different
    /// path — read as a one-path one (`ProjektPlan.md` §12).
    ///
    /// Each sprite carries its own [`TextureRegion`](crate::TextureRegion), so
    /// one bound texture can serve sprites that look nothing alike. The
    /// full-texture case is the region
    /// [`TextureRegion::WHOLE_TEXTURE`](crate::TextureRegion::WHOLE_TEXTURE)
    /// and takes the same route through `batch_vertices` as any other.
    ///
    /// # Draw order
    ///
    /// The order of `sprites`, exactly. Nothing sorts, and there is no depth
    /// buffer — `depth_stencil: None` in the pipeline — so a later sprite
    /// overwrites an earlier one where they overlap. Depth ordering is a
    /// separate capability in `ProjektPlan.md` §6/M3 and is not decided here.
    ///
    /// # Errors
    ///
    /// - [`RenderError::BatchTooLarge`] if `sprites` holds more than
    ///   [`MAX_SPRITES_PER_BATCH`].
    /// - [`RenderError::Readback`] if the copy back from the GPU fails.
    pub fn render_sprites(
        &self,
        image: &Pixels,
        sprites: &[SpriteInstance],
    ) -> Result<Pixels, RenderError> {
        self.render_sprites_viewed_by(image, sprites, CameraView::IDENTITY)
    }

    /// [`Self::render_sprites`], seen from `camera`.
    ///
    /// The only **offscreen** path that draws a sprite in this crate;
    /// `render_sprites` is a call to it with [`CameraView::IDENTITY`] and has no
    /// code of its own. Both qualifiers are load bearing. *Sprite*, because
    /// `render_textured_quad` here and `WindowTarget::present_textured_quad`
    /// both draw as well, from the pipeline's own screen-filling quad, and
    /// `ProjektPlan.md` §12 records a committed claim that lost exactly that
    /// word. *Offscreen*, since M3.32: this was "the only path that draws a
    /// sprite in this crate" from M3.12 until
    /// [`WindowTarget::draw_sprites`](crate::WindowTarget::draw_sprites) became
    /// the second, through the same `batch_runs`, the same `batch_vertices` and
    /// the same `encode_runs`.
    ///
    /// The camera path is the regression argument it always was: the three
    /// blessed images that reach this crate through a projection —
    /// `placed_sprite_quadrants_128x128`, `sprite_atlas_regions_128x128` and
    /// `layer_order_regions_128x128` — go through the camera path on every run,
    /// with the identity view, rather than past it.
    ///
    /// The fourth, `textured_quad_quadrants_64x64`, does **not**: it is drawn by
    /// [`Self::render_textured_quad`] from the pipeline's own screen-filling
    /// quad, whose corners are NDC literals in `quad.rs` and never meet a
    /// `Projection`. A camera cannot move it, and M3.12 reports that rather than
    /// changing it.
    ///
    /// # Errors
    ///
    /// As [`Self::render_sprites`].
    pub fn render_sprites_viewed_by(
        &self,
        image: &Pixels,
        sprites: &[SpriteInstance],
        camera: CameraView,
    ) -> Result<Pixels, RenderError> {
        self.render_batches(image, sprites, None, camera)?;
        self.read_back()
    }

    /// The same render with a **second texture's** sprites drawn after the
    /// first's.
    ///
    /// The offscreen half of M6.6c's seam, and it exists so the two-texture path
    /// has coverage that does not need a window. `lib.rs` says why that is a
    /// condition rather than a nicety: a second path "where the golden images pin
    /// one and nothing checks the other" is the thing this crate keeps out.
    ///
    /// `overlay` is drawn **after** `sprites` and therefore over them — draw
    /// order is composition (ADR-0023), and this method adds an ordering rule
    /// rather than a blending one. The blend state, the pass and its
    /// `LoadOp::Clear` are untouched: both batches go into the *same* pass, as
    /// two groups of runs, because `QuadPipeline::encode_runs` binds per run.
    ///
    /// # Two cameras
    ///
    /// `camera` is the **scene's**; the overlay is seen through
    /// [`SpriteBatch::camera`], which is a field of the batch rather than a
    /// second parameter here for the reasons recorded on that type. An overlay
    /// carrying [`CameraView::IDENTITY`] is screen-fixed: it stays where it is
    /// while `camera` pans and zooms. An overlay carrying `camera` itself
    /// reproduces what this method drew before M6b.4, bit for bit, because the
    /// overlay's projection is this one with its camera field replaced.
    ///
    /// # Errors
    ///
    /// [`RenderError::BatchTooLarge`] if the **sum** of both batches exceeds
    /// [`MAX_SPRITES_PER_BATCH`]. One pass, one vertex buffer, one index buffer —
    /// so the limit is on the total rather than on each batch.
    pub fn render_sprites_over(
        &self,
        image: &Pixels,
        sprites: &[SpriteInstance],
        overlay: SpriteBatch<'_>,
        camera: CameraView,
    ) -> Result<Pixels, RenderError> {
        self.render_batches(image, sprites, Some(overlay), camera)?;
        self.read_back()
    }

    /// One implementation for both entry points above.
    ///
    /// **There is one render path, not two.** The public surface has two doors
    /// because `render_sprites_viewed_by` has many callers — most of them in
    /// tests that draw blessed references — and changing its signature would
    /// have moved those files in the same commit as the seam, which is exactly
    /// the evidence M6.6b's re-export was kept to protect.
    ///
    /// **This said "thirteen callers — nine of them in tests" and the number had
    /// gone stale, which M8.2 found by counting.** At M8.2's gate there were
    /// **fourteen**; the sentence had been written when there were thirteen and
    /// nothing recounted it when the fourteenth arrived. M8.2 then removed one —
    /// `narvo-app`'s `Offscreen::draw` calls [`Self::draw_sprites_over`] now —
    /// so the count is thirteen again **by coincidence**, which is a worse state
    /// than being wrong: a number that is right by luck reads exactly like one
    /// that is right by construction.
    ///
    /// So it is a quantifier now rather than a number. Nothing counts these
    /// callers, and a count nothing checks is a claim waiting to go stale — the
    /// argument the sentence is making does not need the figure, only that there
    /// are enough of them for a signature change to be expensive.
    ///
    /// **It draws and does not read back, since M8.2.** The copy moved out to
    /// [`Self::read_back`] so that a caller can encode something between the two
    /// — which is the whole of what a multi-pass frame is. The two public
    /// `render_*` doors above call this and then that, in one statement each, so
    /// what they return is unchanged down to the byte.
    fn render_batches(
        &self,
        image: &Pixels,
        sprites: &[SpriteInstance],
        overlay: Option<SpriteBatch<'_>>,
        camera: CameraView,
    ) -> Result<(), RenderError> {
        // Emptiness rather than `Option`: a batch with no sprites must be
        // indistinguishable from no batch at all, which is the property
        // `batch_plan` is written around.
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
        // Built only when there is something to draw from it. This is the line
        // that makes an empty second batch a no-op rather than a cheap one.
        let overlay_bindings = overlay.map(|batch| {
            self.quad.bind_texture(
                &self.device,
                &self.queue,
                batch.image.width(),
                batch.image.height(),
                batch.image.rgba(),
            )
        });

        // D15: cut the drawing order into runs of equal filter, one draw call
        // each. The order is not touched — `batch_runs` only cuts, and
        // `batch_plan` only concatenates two of its results.
        //
        // Each run's sampler is the filter every sprite in it shares — that is
        // what `batch_runs` cut on, so the run's first sprite names it for all
        // of them. **The decomposition chooses the sampler here; `encode_runs`
        // only binds what it is handed** — it takes bind groups, never filters,
        // so the run loop has no sampler policy in it. Since M6.6c it chooses
        // the *texture* here too, for the same reason.
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

        // M6b.4: the two batches are two coordinate spaces, not one. The
        // overlay's projection is **this** projection with its camera field
        // replaced — `viewed_by` is `Self { camera, ..self }`, a move rather
        // than arithmetic — so the two share the target's half-extents by
        // construction and an overlay handed the scene's camera is handed back
        // the very same `Projection`, bit for bit. That is what makes "existing
        // callers reproduce today's image" an identity instead of a claim about
        // floating point.
        let projection = Projection::for_target(self.width, self.height).viewed_by(camera);
        let mut corners = batch_vertices(sprites, projection);
        let overlay_projection = projection.viewed_by(overlay.map_or(camera, |batch| batch.camera));
        corners.extend(batch_vertices(overlay_sprites, overlay_projection));
        let vertices = self.quad.corner_buffer(&self.device, &corners);
        let indices = self
            .quad
            .batch_index_buffer(&self.device, sprites.len() + overlay_len);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo offscreen sprite batch"),
            });

        self.quad.encode_runs(
            &mut encoder,
            &self.multisampled_view,
            &self.view,
            &vertices,
            &indices,
            &runs,
        );

        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(())
    }

    /// Submits `encoder`, copies the target into a transfer buffer and reads it
    /// back as tightly packed RGBA8.
    ///
    /// Shared by every `render_*` method: whatever was drawn, this is how it
    /// leaves the GPU.
    ///
    /// A one-line delegation since M6.4b. The body moved out to
    /// [`read_back_texture`] unchanged so that the window path can call the same
    /// code; what an offscreen test proves about padding and row order therefore
    /// covers a windowed read-back too.
    fn finish_and_read_back(&self, encoder: wgpu::CommandEncoder) -> Result<Pixels, RenderError> {
        read_back_texture(
            &self.device,
            &self.queue,
            encoder,
            &self.texture,
            self.width,
            self.height,
        )
    }
}

/// Submits `encoder`, copies `texture` into a transfer buffer and reads it back
/// as tightly packed bytes, four per texel.
///
/// **In the texture's own channel order, not necessarily RGBA.** A
/// `Bgra8UnormSrgb` texture reads back blue-green-red-alpha, which is what
/// `the_quad_keeps_its_colours_in_a_surface_typical_bgra_format` asserts and why
/// that test swaps its expectations rather than the bytes. [`rgba_from`] is the
/// step that turns those bytes into what [`Pixels::rgba`] promises, and it is
/// deliberately *not* folded in here: every caller inside this file renders
/// through [`TARGET_FORMAT`], where it would be the identity, and the ten
/// blessed references are read back through exactly this function.
///
/// A free function since M6.4b, taking its device, queue and texture rather than
/// reading them off an [`OffscreenTarget`]. That is the whole of the change —
/// the body below is [`OffscreenTarget::finish_and_read_back`]'s, moved — and it
/// is what lets [`WindowTarget::read_back`](crate::WindowTarget::read_back) copy
/// a swapchain image with the same code an offscreen test already exercises.
///
/// `texture` must have been created with `COPY_SRC` and must not be
/// multisampled; both hold for every caller in this crate.
///
/// # Errors
///
/// [`RenderError::Readback`] if the device could not be polled, the transfer
/// buffer could not be mapped, or the mapping could not be read.
pub(crate) fn read_back_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Pixels, RenderError> {
    // `copy_texture_to_buffer` requires `bytes_per_row` to be a multiple of
    // COPY_BYTES_PER_ROW_ALIGNMENT (256 bytes), which for RGBA8 only happens
    // when the width is a multiple of 64. Every other width gets padding at
    // the end of each row, and forgetting to strip it later shifts every row
    // against the one above it - an image that is subtly, confusingly wrong
    // rather than obviously broken.
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

    let transfer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("narvo read-back transfer"),
        size: u64::from(padded_bytes_per_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &transfer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    // Map, then poll. The mapping callback only runs while the device is
    // polled, so without the poll below the receive would wait forever -
    // the classic wgpu read-back deadlock.
    let slice = transfer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        // The receiver is gone only if this call already unwound, in which
        // case there is nobody left to tell.
        let _ = sender.send(result);
    });

    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map_err(|error| RenderError::Readback {
            step: "waiting for the GPU to finish the copy",
            source: Box::new(error),
        })?;

    match receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            return Err(RenderError::Readback {
                step: "mapping the transfer buffer",
                source: Box::new(error),
            });
        }
        Err(error) => {
            return Err(RenderError::Readback {
                step: "waiting for the mapping callback after a blocking poll",
                source: Box::new(error),
            });
        }
    }

    let unpadded = unpadded_bytes_per_row as usize;
    let padded = padded_bytes_per_row as usize;

    let mapped = slice
        .get_mapped_range()
        .map_err(|error| RenderError::Readback {
            step: "reading the mapped transfer buffer",
            source: Box::new(error),
        })?;
    let mut rgba = Vec::with_capacity(unpadded * height as usize);
    for row in mapped.chunks_exact(padded) {
        rgba.extend_from_slice(&row[..unpadded]);
    }
    drop(mapped);
    transfer.unmap();

    Ok(Pixels {
        width,
        height,
        rgba,
    })
}

/// Turns bytes read back from a texture of `format` into genuine RGBA8.
///
/// [`Pixels::rgba`] promises "tightly packed RGBA8 bytes" and
/// [`Pixels::save_png`] hands them to the encoder as `Rgba8`, so a caller that
/// read a `Bgra8UnormSrgb` texture back and skipped this step would write a PNG
/// with red and blue swapped. That is not hypothetical here and it is not
/// symmetric between the two platforms this project verifies on: the offscreen
/// path pins [`TARGET_FORMAT`], and `choose_format` takes the first sRGB format
/// the *surface* offers, which was measured in M6.4b as `Rgba8UnormSrgb` on this
/// machine's AMD Vulkan surface and as `Bgra8UnormSrgb` under WSL's lavapipe —
/// the surface there offers no RGBA format at all. A format-blind read-back is
/// therefore correct on one of the two and wrong on the other.
///
/// Only the four 8-bit-per-channel formats a surface realistically hands out are
/// accepted. Anything else is refused rather than reinterpreted, because there
/// is no byte order in which a `Rgb10a2Unorm` texel is four RGBA8 bytes and
/// pretending otherwise would produce a plausible, wrong picture.
///
/// # Errors
///
/// [`RenderError::UnreadableFormat`] if `format` is not one of the four.
pub(crate) fn rgba_from(
    format: wgpu::TextureFormat,
    mut pixels: Pixels,
) -> Result<Pixels, RenderError> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => Ok(pixels),
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
            for texel in pixels.rgba.chunks_exact_mut(4) {
                texel.swap(0, 2);
            }
            Ok(pixels)
        }
        other => Err(RenderError::UnreadableFormat {
            format: format!("{other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{ClearColor, OffscreenTarget, Pixels};
    use crate::RenderError;

    /// How far a channel may drift before a test complains.
    ///
    /// Headroom, not a fitted difference — and the reason first given for it
    /// has since been measured and did not occur. That reason was that the CPU
    /// rasterisers CI uses, llvmpipe on Linux and WARP on Windows, need not
    /// convert sRGB bit-identically to a GPU's. `docs/perf/BASELINE.md`
    /// §"Golden-image margin" records all three rasterisers rendering the M1
    /// reference image with `worst channel deviation 0`. The margin is kept
    /// anyway, because that is a moment value of three drivers and these probes
    /// are not the place to meet a fourth.
    ///
    /// Positions get no tolerance. Four counts out of 255 stays far below what
    /// either diagnostic failure produces, and the two are not the same size: a
    /// swapped channel on this fixture moves a channel by up to 255, while the
    /// "sixty-odd counts" once quoted for a wrong colour space is an estimate
    /// nobody has measured. F1 and F2 in `ProjektPlan.md` §12 named both
    /// problems with the old wording; the value itself is unchanged.
    const COLOUR_TOLERANCE: u8 = 4;

    /// A `size` x `size` texture split into four differently coloured quadrants.
    ///
    /// The shared four-quadrant fixture (D16, ADR-0016), wrapped here.
    ///
    /// The `_rgba` form rather than `narvo_testkit::quadrant_texture`, and
    /// this is the one place in the workspace where that is forced rather than
    /// chosen: these tests are compiled *inside* `narvo-render2d`, so their
    /// `Pixels` is the `cfg(test)` build's, while the testkit's is the ordinary
    /// build's. Measured in M3.22, not assumed —
    /// `error[E0308]: expected `offscreen::Pixels`, found
    /// `narvo_render2d::offscreen::Pixels``. `Vec<u8>` is `std` and crosses
    /// unchanged, so the pixel truth is still the testkit's single definition.
    fn quadrant_texture(size: u32) -> Pixels {
        Pixels::from_rgba8(size, size, narvo_testkit::quadrant_rgba(size))
            .expect("the generated buffer matches its dimensions")
    }

    /// Asserts the pixel at an exact position is a colour, within tolerance.
    fn assert_pixel_close(pixels: &Pixels, x: u32, y: u32, expected: [u8; 4]) {
        let actual = pixels.pixel(x, y).unwrap_or_else(|| {
            panic!(
                "pixel ({x}, {y}) lies outside the {}x{} output",
                pixels.width(),
                pixels.height()
            )
        });

        for (channel, (got, want)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                got.abs_diff(*want) <= COLOUR_TOLERANCE,
                "pixel ({x}, {y}) channel {channel}: got {got}, expected {want} \
                 (tolerance {COLOUR_TOLERANCE}); whole pixel {actual:?} against {expected:?}"
            );
        }
    }

    /// Printed when a test cannot run for lack of an adapter. Distinctive on
    /// purpose: it is what a CI log gets searched for to tell "the renderer was
    /// verified" apart from "the renderer was never exercised".
    const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

    /// Set this, to anything, and a missing adapter fails instead of skipping.
    ///
    /// Skipping is right on a developer machine: not every one has a working
    /// driver, and the rest of the suite should still run. It is wrong in CI,
    /// where a skip is indistinguishable from a pass while verifying nothing -
    /// which is exactly how the Linux job looked before lavapipe was installed.
    /// An environment variable keeps that difference in the workflow file, where
    /// it is visible, instead of hiding it behind a build configuration.
    const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

    /// Builds a target, or reports that this machine cannot host one.
    ///
    /// Only a missing adapter skips, and only when [`REQUIRE_GPU_VAR`] is unset.
    /// Every other error is a real failure and is raised as one - a test that
    /// swallowed those would be green for the worst possible reason.
    fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
        match OffscreenTarget::new(width, height) {
            Ok(target) => {
                println!("adapter in use: {}", target.adapter_summary());
                Some(target)
            }
            Err(error @ RenderError::NoAdapter { .. }) => {
                assert!(
                    std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                    "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a failure \
                     rather than a skip. Either the software rasteriser stopped being \
                     installed or the Vulkan loader can no longer find it: {error}"
                );

                println!("{SKIP_MARKER}: {error}");
                None
            }
            Err(other) => panic!(
                "creating the offscreen target failed for a reason other than a \
                 missing adapter: {other}"
            ),
        }
    }

    #[test]
    fn a_failure_with_a_cause_keeps_it_reachable_through_source() {
        use std::error::Error as _;

        // No GPU involved: a one-pixel image and a directory that is not there.
        let image =
            Pixels::from_rgba8(1, 1, vec![1, 2, 3, 4]).expect("a 1x1 RGBA8 buffer is four bytes");
        let unreachable = std::env::temp_dir()
            .join(format!("narvo-render2d-absent-{}", std::process::id()))
            .join("nowhere.png");

        let error = image
            .save_png(&unreachable)
            .expect_err("writing into a directory that does not exist must fail");

        assert!(matches!(error, RenderError::PngWrite { .. }), "got {error}");

        let source = error.source().expect(
            "the underlying error must survive as source() instead of being \
             flattened into a string",
        );
        assert!(
            !source.to_string().is_empty(),
            "source() must carry the original message, not an empty placeholder"
        );

        println!("cause preserved: {source}");
    }

    #[test]
    fn a_pixel_buffer_that_contradicts_its_dimensions_is_rejected() {
        let error =
            Pixels::from_rgba8(4, 4, vec![0; 10]).expect_err("a 4x4 image needs 64 bytes, not 10");

        assert!(
            matches!(
                error,
                RenderError::PixelBufferSize {
                    expected: 64,
                    actual: 10,
                    ..
                }
            ),
            "got {error}"
        );
    }

    #[test]
    fn the_texture_quadrants_land_in_the_matching_corners_of_the_output() {
        let Some(target) = target_or_skip(64, 64) else {
            return;
        };

        let output = target
            .render_textured_quad(&quadrant_texture(8))
            .expect("drawing a textured quad must succeed once a device exists");

        assert_eq!(output.width(), 64);
        assert_eq!(output.height(), 64);

        // The extreme corners pin the orientation: a vertical flip swaps the
        // first pair, a horizontal flip the second, a transpose both.
        assert_pixel_close(&output, 0, 0, narvo_testkit::QUADRANT_TOP_LEFT);
        assert_pixel_close(&output, 63, 0, narvo_testkit::QUADRANT_TOP_RIGHT);
        assert_pixel_close(&output, 0, 63, narvo_testkit::QUADRANT_BOTTOM_LEFT);
        assert_pixel_close(&output, 63, 63, narvo_testkit::QUADRANT_BOTTOM_RIGHT);

        // Well inside each quadrant, where nearest sampling cannot land on a
        // boundary texel and an off-by-one in the UV mapping stays invisible.
        assert_pixel_close(&output, 16, 16, narvo_testkit::QUADRANT_TOP_LEFT);
        assert_pixel_close(&output, 48, 16, narvo_testkit::QUADRANT_TOP_RIGHT);
        assert_pixel_close(&output, 16, 48, narvo_testkit::QUADRANT_BOTTOM_LEFT);
        assert_pixel_close(&output, 48, 48, narvo_testkit::QUADRANT_BOTTOM_RIGHT);
    }

    #[test]
    fn the_quad_keeps_its_colours_in_a_surface_typical_bgra_format() {
        // The window path builds its pipeline for whatever format the surface
        // offers. Here that is Rgba8UnormSrgb, but Bgra8UnormSrgb is at least as
        // common elsewhere, and until now nothing rendered through it - a
        // channel swap would have shown up only on somebody else's display, in
        // a window no test looks at.
        let target = match OffscreenTarget::with_format(64, 64, wgpu::TextureFormat::Bgra8UnormSrgb)
        {
            Ok(target) => target,
            Err(error @ RenderError::NoAdapter { .. }) => {
                assert!(
                    std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                    "{REQUIRE_GPU_VAR} is set: {error}"
                );
                println!("{SKIP_MARKER}: {error}");
                return;
            }
            Err(other) => panic!("creating a BGRA target failed: {other}"),
        };

        let output = target
            .render_textured_quad(&quadrant_texture(8))
            .expect("drawing into a BGRA target must succeed once a device exists");

        // The read-back is raw texture bytes, so a BGRA texture reads back
        // blue-green-red-alpha. Expecting the swapped order here is the whole
        // point: if the pipeline wrote channels in the wrong order, these would
        // come out as the unswapped RGBA values.
        let swapped = |[r, g, b, a]: [u8; 4]| [b, g, r, a];

        assert_pixel_close(&output, 0, 0, swapped(narvo_testkit::QUADRANT_TOP_LEFT));
        assert_pixel_close(&output, 63, 0, swapped(narvo_testkit::QUADRANT_TOP_RIGHT));
        assert_pixel_close(&output, 0, 63, swapped(narvo_testkit::QUADRANT_BOTTOM_LEFT));
        assert_pixel_close(
            &output,
            63,
            63,
            swapped(narvo_testkit::QUADRANT_BOTTOM_RIGHT),
        );
    }

    #[test]
    fn a_textured_quad_reads_back_undistorted_at_an_unaligned_width() {
        // 100 px of RGBA8 is 400 bytes per row, which the copy pads to 512. Had
        // that padding survived into the result, the quadrant boundary would
        // drift further right on every row, and the bottom-row assertions below
        // would land in the wrong quadrant.
        let Some(target) = target_or_skip(100, 100) else {
            return;
        };

        let output = target
            .render_textured_quad(&quadrant_texture(8))
            .expect("drawing a textured quad must succeed once a device exists");

        assert_eq!(output.rgba().len(), 100 * 100 * 4);

        assert_pixel_close(&output, 25, 25, narvo_testkit::QUADRANT_TOP_LEFT);
        assert_pixel_close(&output, 75, 25, narvo_testkit::QUADRANT_TOP_RIGHT);
        assert_pixel_close(&output, 25, 75, narvo_testkit::QUADRANT_BOTTOM_LEFT);
        assert_pixel_close(&output, 75, 75, narvo_testkit::QUADRANT_BOTTOM_RIGHT);

        // The last row is where accumulated drift would be largest.
        assert_pixel_close(&output, 0, 99, narvo_testkit::QUADRANT_BOTTOM_LEFT);
        assert_pixel_close(&output, 99, 99, narvo_testkit::QUADRANT_BOTTOM_RIGHT);
    }

    #[test]
    fn a_zero_or_oversized_target_is_rejected_before_any_gpu_work() {
        // Deliberately checkable without an adapter, so at least one test of
        // this module does real work on every machine.
        for (width, height) in [(0, 64), (64, 0), (OffscreenTarget::MAX_DIMENSION + 1, 64)] {
            let error =
                OffscreenTarget::new(width, height).expect_err("an unusable size must be rejected");

            assert!(
                matches!(error, RenderError::InvalidSize { .. }),
                "expected InvalidSize for {width}x{height}, got {error}"
            );
            assert!(
                error.to_string().contains("unusable"),
                "the message should say what is wrong: {error}"
            );
        }
    }

    #[test]
    fn every_pixel_of_a_cleared_target_carries_the_clear_colour() {
        let Some(target) = target_or_skip(64, 64) else {
            return;
        };

        let pixels = target
            .render_clear(ClearColor::RED)
            .expect("clearing a 64x64 target must succeed once a device exists");

        assert_eq!(pixels.width(), 64);
        assert_eq!(pixels.height(), 64);
        assert_eq!(pixels.rgba().len(), 64 * 64 * 4);

        // 1.0 and 0.0 survive the linear-to-sRGB conversion exactly, so this is
        // an exact comparison rather than a tolerance.
        for chunk in pixels.rgba().chunks_exact(4) {
            assert_eq!(chunk, [255, 0, 0, 255], "every pixel must be opaque red");
        }
    }

    #[test]
    fn a_width_that_is_not_row_aligned_reads_back_without_padding() {
        // 100 px of RGBA8 is 400 bytes per row, which the 256-byte copy
        // alignment pads to 512. Every row therefore carries 112 bytes that are
        // not image data. If they were not stripped, they would show up here as
        // pixels that are not the clear colour.
        let Some(target) = target_or_skip(100, 100) else {
            return;
        };

        let pixels = target
            .render_clear(ClearColor::new(0.0, 1.0, 1.0, 1.0))
            .expect("clearing a 100x100 target must succeed once a device exists");

        assert_eq!(
            pixels.rgba().len(),
            100 * 100 * 4,
            "the buffer must be tightly packed, with no row padding left in it"
        );

        for (index, chunk) in pixels.rgba().chunks_exact(4).enumerate() {
            assert_eq!(
                chunk,
                [0, 255, 255, 255],
                "pixel {index} ({}, {}) is not the clear colour, which is what row \
                 padding left in the buffer looks like",
                index as u32 % 100,
                index as u32 / 100
            );
        }
    }

    #[test]
    fn a_written_png_reads_back_with_the_same_size_and_pixels() {
        let Some(target) = target_or_skip(32, 48) else {
            return;
        };

        let pixels = target
            .render_clear(ClearColor::BLUE)
            .expect("clearing a 32x48 target must succeed once a device exists");

        // Temp directory, never the repository: a test that leaves files in the
        // working tree turns `git status` into noise.
        let directory = std::env::temp_dir().join(format!(
            "narvo-render2d-png-{}",
            // nextest runs every test in its own process, so this is unique.
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).expect("the temp directory must be creatable");
        let path = directory.join("clear.png");

        pixels
            .save_png(&path)
            .expect("writing the PNG must succeed");

        let decoded = image::open(&path)
            .expect("the PNG that was just written must be readable")
            .to_rgba8();

        assert_eq!(decoded.width(), 32);
        assert_eq!(decoded.height(), 48);
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(decoded.get_pixel(31, 47).0, [0, 0, 255, 255]);
        assert_eq!(decoded.get_pixel(16, 24).0, [0, 0, 255, 255]);

        std::fs::remove_dir_all(&directory).expect("the temp directory must be removable");
    }

    #[test]
    fn sampling_outside_the_image_yields_nothing() {
        let Some(target) = target_or_skip(8, 8) else {
            return;
        };

        let pixels = target
            .render_clear(ClearColor::WHITE)
            .expect("clearing an 8x8 target must succeed once a device exists");

        assert_eq!(pixels.pixel(7, 7), Some([255, 255, 255, 255]));
        assert_eq!(pixels.pixel(8, 0), None);
        assert_eq!(pixels.pixel(0, 8), None);
    }

    #[test]
    fn an_rgba_read_back_is_left_alone_and_a_bgra_one_has_its_ends_swapped() {
        // One texel per channel plus one that is symmetric, so a swap that ran
        // twice, ran on the wrong pair, or did not run at all each show up
        // somewhere different.
        let bytes = vec![
            10, 20, 30, 40, // an asymmetric texel
            0, 99, 0, 255, // green: green and alpha must not move
            7, 7, 7, 7, // symmetric: unchanged either way
        ];
        let source = Pixels::from_rgba8(3, 1, bytes.clone()).expect("3x1 is a valid image");

        let untouched = super::rgba_from(wgpu::TextureFormat::Rgba8UnormSrgb, source)
            .expect("an RGBA format must be accepted");
        assert_eq!(untouched.rgba(), bytes, "an RGBA texture is already RGBA");

        let swapped = super::rgba_from(
            wgpu::TextureFormat::Bgra8UnormSrgb,
            Pixels::from_rgba8(3, 1, bytes).expect("3x1 is a valid image"),
        )
        .expect("a BGRA format must be accepted");

        assert_eq!(
            swapped.rgba(),
            [
                30, 20, 10, 40, //
                0, 99, 0, 255, //
                7, 7, 7, 7,
            ],
            "only the first and third byte of each texel may move"
        );
    }

    #[test]
    fn the_linear_twins_of_both_orders_are_accepted_too() {
        // `choose_format` prefers sRGB but falls back to whatever is first when a
        // surface offers none, so the linear pair is reachable and must not be
        // refused as an unknown format.
        for (format, expected) in [
            (wgpu::TextureFormat::Rgba8Unorm, [1, 2, 3, 4]),
            (wgpu::TextureFormat::Bgra8Unorm, [3, 2, 1, 4]),
        ] {
            let pixels = super::rgba_from(
                format,
                Pixels::from_rgba8(1, 1, vec![1, 2, 3, 4]).expect("1x1 is a valid image"),
            )
            .unwrap_or_else(|error| panic!("{format:?} must be accepted: {error}"));

            assert_eq!(pixels.rgba(), expected, "{format:?}");
        }
    }

    #[test]
    fn a_format_that_is_not_four_eight_bit_channels_is_refused_rather_than_reinterpreted() {
        // Offered by this machine's surface, measured in M6.4b, and four bytes
        // wide - so a read-back that only checked the byte count would sail
        // straight past it and write a PNG of the wrong colours.
        let error = super::rgba_from(
            wgpu::TextureFormat::Rgb10a2Unorm,
            Pixels::from_rgba8(1, 1, vec![1, 2, 3, 4]).expect("1x1 is a valid image"),
        )
        .expect_err("a ten-bit format has no RGBA8 reading");

        let message = error.to_string();
        assert!(
            message.contains("Rgb10a2Unorm"),
            "the message must name the format it found: {message}"
        );
        assert!(
            message.contains("Bgra8UnormSrgb"),
            "and the ones it would have taken: {message}"
        );
    }

    #[test]
    fn a_bgra_frame_normalises_to_exactly_what_the_rgba_path_renders() {
        // **The oracle for the windowed read-back**, and it needs no window.
        //
        // A window's surface is whatever `choose_format` found, which was
        // measured in M6.4b as `Rgba8UnormSrgb` on this machine and
        // `Bgra8UnormSrgb` under WSL - so the read-back's normalisation is the
        // identity on one platform and a real swap on the other, and only one of
        // those two can be checked by looking at a picture on this desk.
        //
        // What is checked instead is that the two agree: the same content
        // through the same shared render path, once in each channel order, both
        // read back with `read_back_texture` and the BGRA one put through
        // `rgba_from`, must come out **byte for byte identical**. A normalisation
        // that swapped the wrong pair, ran twice, or did not run fails here; so
        // does a copy that lost a row to padding in one format and not the other.
        //
        // Equality rather than `COLOUR_TOLERANCE`: this is not two rasterisers
        // being compared, it is one device rendering the same arithmetic twice
        // with the bytes landing in a different order.
        let rgba = match OffscreenTarget::with_format(
            // 100 is deliberately not a multiple of 64, so both read-backs go
            // through the row-padding path rather than around it.
            100,
            64,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ) {
            Ok(target) => target,
            Err(error @ RenderError::NoAdapter { .. }) => {
                assert!(
                    std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                    "{REQUIRE_GPU_VAR} is set: {error}"
                );
                println!("{SKIP_MARKER}: {error}");
                return;
            }
            Err(other) => panic!("creating an RGBA target failed: {other}"),
        };
        let bgra = OffscreenTarget::with_format(100, 64, wgpu::TextureFormat::Bgra8UnormSrgb)
            .expect("a BGRA target must be creatable once an adapter exists");

        let texture = quadrant_texture(8);
        let from_rgba = rgba
            .render_textured_quad(&texture)
            .expect("drawing into an RGBA target must succeed");
        let from_bgra = super::rgba_from(
            wgpu::TextureFormat::Bgra8UnormSrgb,
            bgra.render_textured_quad(&texture)
                .expect("drawing into a BGRA target must succeed"),
        )
        .expect("BGRA is one of the four formats");

        assert_eq!(from_bgra.width(), from_rgba.width());
        assert_eq!(from_bgra.height(), from_rgba.height());
        assert_eq!(
            from_bgra.rgba(),
            from_rgba.rgba(),
            "a normalised BGRA read-back must be the RGBA one, byte for byte"
        );

        // And not vacuously: an all-black or all-white frame would satisfy the
        // equality above whatever the swap did, because grey is its own mirror.
        assert_ne!(
            from_rgba.pixel(25, 16),
            from_rgba.pixel(75, 16),
            "the fixture must put different colours in different quadrants, or \
             the comparison above proves nothing about channel order"
        );
    }
}
