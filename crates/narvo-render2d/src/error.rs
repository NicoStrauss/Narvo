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
