//! The error type this crate returns.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;

/// Something that went wrong setting up or driving the GPU.
///
/// The underlying `wgpu` or `image` error is kept, but type-erased behind
/// [`Error`]: [`RenderError::source`] hands out the original message and lets a
/// caller walk the chain, while no graphics-API type appears anywhere in this
/// crate's public API. The boundary in the crate docs - no `wgpu` type crosses
/// back out - therefore holds without throwing the cause away.
///
/// [`Display`](fmt::Display) repeats the source's message rather than only
/// naming the context. That duplicates a little when somebody walks the chain,
/// and it is the deliberate trade: a single `{error}` in a test failure or a log
/// line has to be diagnosable on its own.
#[derive(Debug)]
#[non_exhaustive]
pub enum RenderError {
    /// The requested render target size cannot be used.
    InvalidSize {
        /// Width that was requested.
        width: u32,
        /// Height that was requested.
        height: u32,
        /// Largest dimension this crate accepts.
        max: u32,
    },
    /// More sprites were handed to one batch than it will draw.
    BatchTooLarge {
        /// Sprites the caller passed.
        requested: usize,
        /// Most this crate draws in one batch.
        limit: usize,
    },
    /// A pixel buffer does not hold as many bytes as its dimensions claim.
    PixelBufferSize {
        /// Width the buffer was said to have.
        width: u32,
        /// Height the buffer was said to have.
        height: u32,
        /// Bytes those dimensions require.
        expected: usize,
        /// Bytes actually supplied.
        actual: usize,
    },
    /// No GPU adapter could be obtained, not even a software one.
    ///
    /// This is the expected outcome on a machine with neither a GPU nor a
    /// software rasteriser. Callers that can do without rendering should read it
    /// as "not available here" rather than as a defect, and tests should skip
    /// rather than fail.
    NoAdapter {
        /// Every adapter request that was made, in order, with its rejection.
        attempts: String,
    },
    /// An adapter was found, but it would not create a device.
    NoDevice {
        /// Which adapter refused.
        adapter: String,
        /// The driver's own error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// No drawing surface could be created for a window.
    NoSurface {
        /// The graphics API's error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// The window's swapchain would not hand out a frame to draw into.
    ///
    /// Usually means the surface needs reconfiguring, which is what a resize
    /// does. A caller that sees this repeatedly has a surface out of step with
    /// its window.
    NoFrame {
        /// What the surface reported.
        source: Box<dyn Error + Send + Sync>,
    },
    /// Waiting for the GPU to finish the work already submitted failed.
    ///
    /// Not a lost device and not a swapchain problem. `wgpu`'s `PollError` has
    /// exactly two variants — a timeout, and a submission index from another
    /// device — so this is what one of those looks like coming out.
    DeviceWait {
        /// The driver's own error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// Copying the rendered texture back to the CPU failed.
    Readback {
        /// Which step of the read-back failed.
        step: &'static str,
        /// The underlying error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// The window's surface was configured without the usage a copy needs.
    ///
    /// `SurfaceCapabilities::usages` guarantees `RENDER_ATTACHMENT` and nothing
    /// else (`wgpu-types-30.0.0/src/surface.rs:530-533`), so a platform is
    /// entitled to offer a surface that can be drawn into and presented but not
    /// copied out of. The surface is then configured without `COPY_SRC` — asking
    /// for a usage that is not offered would be a validation error at
    /// configuration time, which would cost the window rather than the
    /// screenshot — and a read-back reports this instead.
    SurfaceNotReadable,
    /// A buffer of field texels is not as long as the field's size requires.
    ///
    /// Separate from [`RenderError::PixelBufferSize`] rather than folded into
    /// it: that one counts RGBA8 *bytes* and this one counts `f32` *channels*,
    /// so one message would have to say "expected 64" about two different units.
    /// Error messages are agent feedback (CLAUDE.md), and a unit a reader has to
    /// infer is the part that costs a session.
    FieldTexelCount {
        /// Width of the field the buffer was offered to.
        width: u32,
        /// Height of that field.
        height: u32,
        /// Floats those dimensions require, four per texel.
        expected: usize,
        /// Floats actually supplied.
        actual: usize,
    },
    /// A seed was placed outside the field it belongs to.
    ///
    /// Separate from [`RenderError::InvalidSize`], which is about the field's
    /// own dimensions rather than about a point inside it. The two would
    /// otherwise share a message that could not say which of the numbers in it
    /// was the offending one.
    SeedOutsideField {
        /// Column the seed was placed at.
        x: u32,
        /// Row the seed was placed at.
        y: u32,
        /// Width of the field it was placed in.
        width: u32,
        /// Height of that field.
        height: u32,
    },
    /// A ray endpoint is outside the field it would be marched against.
    ///
    /// Refused rather than clamped. `march.wgsl` clamps a position to the field
    /// as defence, and M8.3b measured that a clamp at a field edge can mask an
    /// off-by-one — so the refusal here is the guard and the clamp is not asked
    /// to be one.
    RayOutsideField {
        /// Column the endpoint sits at, in field texels.
        x: f32,
        /// Row the endpoint sits at, in field texels.
        y: f32,
        /// Width of the field it would be marched against.
        width: u32,
        /// Height of that field.
        height: u32,
    },
    /// A ray endpoint is not a finite number.
    ///
    /// Separate from [`RenderError::RayOutsideField`], which can name a
    /// coordinate and a bound. A `NaN` compares false against every bound, so
    /// folding the two would produce a message claiming `NaN` is outside a range
    /// it is merely incomparable with.
    RayNotFinite,
    /// A cascade stage was given a number it cannot be built from.
    ///
    /// One variant for the scalar faults rather than one each, because they
    /// share a shape: a named quantity, the value it was given, and the
    /// requirement it broke. That is the whole of the feedback, and splitting it
    /// six ways would repeat the same sentence six times with a different noun.
    /// The two faults that are *not* here — the angular resolution and the
    /// direction count — are separate because their requirement is a derivation
    /// rather than a comparison, and a caller has to be told the number.
    StageParameter {
        /// What was wrong, as a noun phrase that fits the message.
        name: &'static str,
        /// The value that was given. Zero where the fault is a count.
        value: f32,
        /// What it had to satisfy, as a verb phrase that fits the message.
        requirement: &'static str,
    },
    /// A stage's direction count is zero or not a power of two.
    ///
    /// Not a tidiness rule. It is what makes the kernel's normalisation an exact
    /// division and therefore lets the kernel hold no `f32` multiply at all —
    /// which M8.5a measured to be the difference between one radiance field
    /// across eight adapter/backend pairs and two.
    DirectionsNotPowerOfTwo {
        /// The count that was asked for.
        directions: u32,
    },
    /// A stage's directions separate by more than a texel at its far end.
    ///
    /// Two adjacent directions are `far * 2 * pi / directions` apart along the
    /// arc where they finish. Let that exceed one texel and an occluder can fall
    /// between them, which shows up as a stripe where a soft shadow belongs.
    /// The bound is the texel size and nothing else.
    PenumbraUnderresolved {
        /// The count that was asked for.
        directions: u32,
        /// The far end that outruns it.
        far: f32,
        /// `ceil(2 * pi * far)`, the smallest count that would do.
        required: u32,
    },
    /// A stage asks for more rays than one compute dispatch can hold.
    StageTooLarge {
        /// Rays the stage would march: probes times directions.
        rays: u64,
        /// The most one dispatch reaches.
        limit: u32,
    },
    /// An emission map is not the same size as the seed set beside it.
    EmissionSizeMismatch {
        /// Width of the seed set.
        seed_width: u32,
        /// Height of the seed set.
        seed_height: u32,
        /// Width of the emission map.
        emission_width: u32,
        /// Height of the emission map.
        emission_height: u32,
    },
    /// A probe stands closer to an edge than its own near end.
    ProbeOutsideField {
        /// Column the offending probe sits at, in field texels.
        x: f32,
        /// Row the offending probe sits at, in field texels.
        y: f32,
        /// The near end it has to stand clear of every edge by.
        near: f32,
        /// Width of the field it was placed in.
        width: u32,
        /// Height of that field.
        height: u32,
    },
    /// An emission value was placed outside the map it belongs to.
    ///
    /// Separate from [`RenderError::SeedOutsideField`] rather than sharing it:
    /// the two are the same arithmetic about two different things, and a message
    /// saying "a seed" about an emission value would send a reader to the wrong
    /// call.
    EmissionOutsideField {
        /// Column the value was placed at.
        x: u32,
        /// Row the value was placed at.
        y: u32,
        /// Width of the map it was placed in.
        width: u32,
        /// Height of that map.
        height: u32,
    },
    /// A cascade level's per-direction radiance is more than one storage buffer
    /// binding can hold.
    ///
    /// Only the **directional** merge can meet this: it keeps one entry per
    /// probe per direction in a single buffer, where the aggregate merge keeps
    /// one texel per probe in a field and is bounded by the texture dimension
    /// instead. So this is the ceiling that separates the two forms rather than
    /// one they share.
    CascadeLevelTooLarge {
        /// Which level, counting from zero at the bottom.
        level: u32,
        /// Bytes that level's entries would occupy.
        bytes: u64,
        /// The most one binding holds.
        limit: u64,
    },
    /// An albedo map is not the same size as the seed set beside it.
    ///
    /// Separate from [`RenderError::EmissionSizeMismatch`] rather than sharing
    /// it, for [`RenderError::EmissionOutsideField`]'s reason: the two are the
    /// same arithmetic about two different maps, and a surface cache takes both,
    /// so a message naming the wrong one would send a reader to the wrong
    /// argument of the same call.
    AlbedoSizeMismatch {
        /// Width of the seed set.
        seed_width: u32,
        /// Height of the seed set.
        seed_height: u32,
        /// Width of the albedo map.
        albedo_width: u32,
        /// Height of the albedo map.
        albedo_height: u32,
    },
    /// An albedo value was placed outside the map it belongs to.
    AlbedoOutsideField {
        /// Column the value was placed at.
        x: u32,
        /// Row the value was placed at.
        y: u32,
        /// Width of the map it was placed in.
        width: u32,
        /// Height of that map.
        height: u32,
    },
    /// A surface cache was asked for a probe grid it cannot index in integers.
    ///
    /// The write-back turns a texel coordinate into the index of the probe
    /// nearest it, and ADR-0050 says a comparison over coordinates is written in
    /// integers. That reasoning reaches an *index* over coordinates as well, so
    /// the level-zero origin and spacing have to be whole texels — which is a
    /// restriction on the cascade rather than on the cache, and is refused here
    /// because this is the first place that needs it.
    ProbeGridNotIntegral {
        /// Which number is unusable, spelled as the caller wrote it.
        name: &'static str,
        /// The value that was offered.
        value: f32,
    },
    /// A texture's format is not one whose bytes are four RGBA8 channels.
    UnreadableFormat {
        /// The format that was found, as `wgpu` spells it.
        format: String,
    },
    /// Encoding or writing a PNG failed.
    PngWrite {
        /// File that was being written.
        path: PathBuf,
        /// The encoder's or the filesystem's error.
        source: Box<dyn Error + Send + Sync>,
    },
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize { width, height, max } => write!(
                f,
                "render target size {width}x{height} is unusable: \
                 both dimensions must be between 1 and {max}"
            ),
            Self::BatchTooLarge { requested, limit } => write!(
                f,
                "a sprite batch of {requested} exceeds the limit of {limit}: \
                 split it across several calls. Drawing the first {limit} instead \
                 is not offered, because a batch that silently loses sprites looks \
                 like a rendering fault everywhere except where it is"
            ),
            Self::PixelBufferSize {
                width,
                height,
                expected,
                actual,
            } => write!(
                f,
                "a {width}x{height} RGBA8 image needs {expected} bytes but {actual} were given"
            ),
            Self::FieldTexelCount {
                width,
                height,
                expected,
                actual,
            } => write!(
                f,
                "a {width}x{height} field needs {expected} floats, four per texel, but {actual} were given"
            ),
            Self::SeedOutsideField {
                x,
                y,
                width,
                height,
            } => write!(
                f,
                "a seed at ({x}, {y}) is outside a {width}x{height} field: \
                 columns run 0..{width} and rows 0..{height}"
            ),
            Self::RayOutsideField {
                x,
                y,
                width,
                height,
            } => write!(
                f,
                "a ray endpoint at ({x}, {y}) is outside a {width}x{height} field: \
                 both ends of a march must lie within 0..={width} by 0..={height}, \
                 because the field says nothing about what is beyond its edge"
            ),
            Self::RayNotFinite => write!(
                f,
                "a ray endpoint is not a finite number, so it has no position in \
                 the field at all"
            ),
            Self::StageParameter {
                name,
                value,
                requirement,
            } => write!(
                f,
                "a cascade stage cannot be built: {name} is {value}, and it has to {requirement}"
            ),
            Self::DirectionsNotPowerOfTwo { directions } => write!(
                f,
                "a cascade stage of {directions} directions is not buildable: the count \
                 must be a power of two, so that dividing a sum by it is exact. That \
                 exactness is what keeps the kernel free of any float multiplication, \
                 and a float multiply feeding an add is the one thing measured to make \
                 two backends compute two different radiance fields"
            ),
            Self::PenumbraUnderresolved {
                directions,
                far,
                required,
            } => write!(
                f,
                "{directions} directions do not resolve an interval reaching {far} texels: \
                 two adjacent directions finish {:.3} texels apart, so an occluder one \
                 texel wide can fall between them and the soft shadow becomes a stripe. \
                 Use at least {required} directions, or shorten the interval",
                f64::from(*far) * std::f64::consts::TAU / f64::from(*directions)
            ),
            Self::StageTooLarge { rays, limit } => write!(
                f,
                "a cascade stage of {rays} rays is beyond one dispatch, which reaches \
                 {limit}: split the probe grid, or use fewer directions. At 48 bytes a \
                 ray the limit is already 201 MB of GPU buffer"
            ),
            Self::EmissionSizeMismatch {
                seed_width,
                seed_height,
                emission_width,
                emission_height,
            } => write!(
                f,
                "an emission map of {emission_width}x{emission_height} does not go with a \
                 seed set of {seed_width}x{seed_height}: a stage reads emission at the \
                 seed a direction stopped on, so the two are indexed by one coordinate"
            ),
            Self::ProbeOutsideField {
                x,
                y,
                near,
                width,
                height,
            } => write!(
                f,
                "a probe at ({x}, {y}) with a near end of {near} does not fit a \
                 {width}x{height} field: every probe must stand at least its near end \
                 clear of every edge, because a direction that starts outside the field \
                 has no position in it. The far end needs no such room - a direction \
                 that runs off the edge is clipped there and counts as having met nothing"
            ),
            Self::EmissionOutsideField {
                x,
                y,
                width,
                height,
            } => write!(
                f,
                "an emission value at ({x}, {y}) is outside a {width}x{height} map: \
                 columns run 0..{width} and rows 0..{height}"
            ),
            Self::CascadeLevelTooLarge {
                level,
                bytes,
                limit,
            } => write!(
                f,
                "level {level} of a directional cascade needs {bytes} bytes of radiance, \
                 and one storage buffer binding holds {limit}: use a coarser probe \
                 spacing, fewer levels, or the aggregate merge, which keeps one texel \
                 per probe instead of one per direction"
            ),
            Self::AlbedoSizeMismatch {
                seed_width,
                seed_height,
                albedo_width,
                albedo_height,
            } => write!(
                f,
                "an albedo map of {albedo_width}x{albedo_height} does not fit the \
                 seed set of {seed_width}x{seed_height} beside it: the two are \
                 indexed by one coordinate, so a map of another size is a confusion \
                 rather than a shortfall and is refused rather than padded"
            ),
            Self::AlbedoOutsideField {
                x,
                y,
                width,
                height,
            } => write!(
                f,
                "an albedo value at ({x}, {y}) lies outside the {width}x{height} map \
                 it was placed in"
            ),
            Self::ProbeGridNotIntegral { name, value } => write!(
                f,
                "a surface cache needs {name} to be a whole number of texels, and \
                 {value} is not: the write-back finds the probe nearest a texel by \
                 integer arithmetic, which ADR-0050 is the reason for. Give the \
                 cascade an integral origin and spacing"
            ),
            Self::NoAdapter { attempts } => write!(
                f,
                "no GPU adapter available; tried {attempts}. On a headless machine, \
                 install a software rasteriser such as Mesa lavapipe and make sure the \
                 Vulkan loader can find it"
            ),
            Self::NoDevice { adapter, source } => {
                write!(f, "adapter {adapter} would not create a device: {source}")
            }
            Self::NoSurface { source } => {
                write!(
                    f,
                    "no drawing surface could be created for the window: {source}"
                )
            }
            Self::NoFrame { source } => write!(
                f,
                "the window's swapchain would not hand out a frame: {source}. \
                 The surface usually needs reconfiguring - resize it and try again"
            ),
            Self::DeviceWait { source } => write!(
                f,
                "waiting for the GPU to finish the submitted work failed: {source}"
            ),
            Self::Readback { step, source } => {
                write!(
                    f,
                    "reading the rendered texture back to the CPU failed while {step}: {source}"
                )
            }
            Self::SurfaceNotReadable => write!(
                f,
                "this window's surface cannot be read back: the adapter offers no \
                 COPY_SRC usage for it, so a frame can be drawn and presented but \
                 not copied to the CPU. Render the scene offscreen instead - \
                 `narvo --screenshot` needs no window"
            ),
            Self::UnreadableFormat { format } => write!(
                f,
                "a frame in {format} cannot be read back: this crate turns only \
                 Rgba8Unorm, Rgba8UnormSrgb, Bgra8Unorm and Bgra8UnormSrgb into \
                 RGBA8 bytes, and reinterpreting anything else would produce a \
                 plausible picture with the wrong colours in it"
            ),
            Self::PngWrite { path, source } => {
                write!(f, "writing the PNG to {} failed: {source}", path.display())
            }
        }
    }
}

impl Error for RenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidSize { .. }
            | Self::BatchTooLarge { .. }
            | Self::PixelBufferSize { .. }
            | Self::FieldTexelCount { .. }
            | Self::SeedOutsideField { .. }
            | Self::RayOutsideField { .. }
            | Self::RayNotFinite
            | Self::StageParameter { .. }
            | Self::DirectionsNotPowerOfTwo { .. }
            | Self::PenumbraUnderresolved { .. }
            | Self::StageTooLarge { .. }
            | Self::EmissionSizeMismatch { .. }
            | Self::ProbeOutsideField { .. }
            | Self::EmissionOutsideField { .. }
            | Self::AlbedoSizeMismatch { .. }
            | Self::AlbedoOutsideField { .. }
            | Self::ProbeGridNotIntegral { .. }
            | Self::CascadeLevelTooLarge { .. }
            | Self::SurfaceNotReadable
            | Self::UnreadableFormat { .. }
            | Self::NoAdapter { .. } => None,
            Self::NoDevice { source, .. }
            | Self::NoSurface { source }
            | Self::NoFrame { source }
            | Self::DeviceWait { source }
            | Self::Readback { source, .. }
            | Self::PngWrite { source, .. } => Some(source.as_ref()),
        }
    }
}
