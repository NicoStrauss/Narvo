//! Temporal accumulation: the previous frame's field, resampled into this
//! frame's grid and mixed with the new answer.
//!
//! **M8.7's capability.** A cascade answers "how much light reaches this probe"
//! for one frame. An accumulator answers it for a *run of* frames, by carrying
//! the previous answer forward and letting each new frame move it by a fixed
//! share. The trade is the usual one: noise and cost per frame fall, and the
//! answer lags a change by however many frames the share implies.
//!
//! # The recurrence
//!
//! ```text
//! history_0     = the first frame's fresh field, exactly
//! reprojected_n = resample(history_n, motion)      -- fresh where there is no history
//! history_n+1   = reprojected_n + (fresh_n - reprojected_n) / divisor
//! ```
//!
//! `divisor` is a power of two, and that is the type ([`Blend`]) rather than a
//! check. `shaders/accumulate.wgsl` writes the second line as a **division** and
//! contains no float multiply at all, so ADR-0051's rule is not merely obeyed but
//! inapplicable — there is no fused divide-add on any backend to contract into.
//!
//! # Why the reprojection is a gather and not an interpolation
//!
//! [`Resample::Nearest`] performs no arithmetic on a radiance value: an integer
//! index goes in and a texel comes out. That is what keeps a whole run of frames
//! byte-identical across adapters, which M8.7 measured rather than assumed.
//!
//! [`Resample::Bilinear`] is the other arm and is **inexact by construction** —
//! its weights are the fractional part of a continuous motion, so they cannot be
//! powers of two. It is kept because M8.7's §2 asks how steppy nearest is, and a
//! measurement with one arm deleted is an assertion. `MergeForm::Aggregate` is
//! kept for the same reason and M8.5b's report says why.
//!
//! # Three assurances, and which of them are equalities
//!
//! - **A motion of zero reprojects exactly.** Both arms: the fixed-point offset
//!   is zero, so nearest reads the probe it is writing and bilinear collapses its
//!   four taps onto that same probe. An **equality**, byte for byte.
//! - **Accumulating a converged field changes nothing.** With `fresh` equal to
//!   the reprojected history, the blend is `h + (h - h) / d`, which is `h + 0`.
//!   An **equality**, and it holds for every finite `h` that is not a negative
//!   zero — a value this path cannot produce, because the only zeros in a
//!   radiance field come from a store of `+0.0`.
//! - **A static scene stops changing.** A **bound** and not an equality: the gap
//!   `fresh - history` shrinks by the exact factor `1 - 1/divisor` each frame
//!   until `(fresh - history) / divisor` is below half an ulp of `history`, at
//!   which point the addition returns `history` and the field is a fixed point of
//!   the float recurrence. **So it stops near the fresh field rather than at it**,
//!   and the residue is bounded by roughly `divisor / 2` ulp. `tests/temporal_accumulation.rs`
//!   measures both halves.
//!
//! # What is not world state
//!
//! All of it. An accumulated field is picture light: it decides nothing, no
//! system reads it, and no component holds it. That is why nothing here is
//! registered, serialized or hashed, and why M8.7's nine state hashes are
//! unmoved.

use crate::cascade::{CascadeStage, RadianceField};
use crate::error::RenderError;
use crate::field::{Field, FieldPair};

/// The exact arm's source: the nearest-neighbour reprojection and the blend.
pub(crate) const ACCUMULATE_WGSL: &str = include_str!("shaders/accumulate.wgsl");

/// The inexact arm's source: the bilinear reprojection, and nothing else.
pub(crate) const ACCUMULATE_BILINEAR_WGSL: &str = include_str!("shaders/accumulate_bilinear.wgsl");

/// The entry point that resamples the previous field into this frame's grid.
///
/// One name for both arms: they are two modules with one signature, so the
/// pipeline that is bound decides which is run.
pub(crate) const REPROJECT_ENTRY: &str = "reproject";

/// The entry point that mixes the reprojected history with the fresh field.
pub(crate) const BLEND_ENTRY: &str = "blend";

/// Invocations per workgroup, both axes. A probe grid is walked in two
/// dimensions, exactly as `bounce.wgsl` walks a field.
pub(crate) const ACCUMULATE_WORKGROUP: u32 = 8;

/// Fixed-point bits of fraction in a reprojection offset.
///
/// Sixteen. `shaders/accumulate.wgsl` declares the same number as `SHIFT` and a
/// test holds the two together.
pub(crate) const REPROJECT_SHIFT: u32 = 16;

/// Fixed-point units to one probe.
pub(crate) const REPROJECT_UNIT: i32 = 1 << REPROJECT_SHIFT;

/// The largest reprojection offset a shader may be handed, in fixed point.
///
/// **Derived, not chosen.** The kernel computes `(probe_index << SHIFT) - offset`
/// in `i32`. A probe index is below
/// [`OffscreenTarget::MAX_DIMENSION`](crate::OffscreenTarget::MAX_DIMENSION) =
/// 8192, so the first term is below `2^29`; bounding the offset by the same
/// `2^29` keeps the difference inside `i32` with a whole bit to spare. It is a
/// motion of 8192 probes in one frame, which is the entire field, so nothing a
/// camera does reaches it.
pub(crate) const MAX_REPROJECT_OFFSET: i32 = 1 << 29;

/// The share of a frame's fresh answer that reaches the accumulated field.
///
/// **A reciprocal power of two, and the type says so rather than checking it at
/// the last moment.** `Blend::one_in(8)` lets a frame move the field by an
/// eighth of the way to its own answer.
///
/// The restriction is ADR-0051's and it is what makes the accumulation exact:
/// `shaders/accumulate.wgsl` divides by this number, a division by a power of two
/// is exact, and there is no fused divide-add on any backend for a compiler to
/// contract the surrounding addition into. A general weight would be an `f32`
/// multiply feeding an `f32` add, which ADR-0051 measured returning two radiance
/// fields where the exact form returns one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blend {
    divisor: u32,
}

impl Blend {
    /// The largest divisor, `2^23`.
    ///
    /// Above it `f32(divisor)` in the kernel would stop being exact, which is the
    /// one property the whole design rests on. It is 8 388 608 frames of history
    /// and nothing wants more.
    pub const MAX_DIVISOR: u32 = 1 << 23;

    /// No history at all: every frame replaces the field with its own answer.
    ///
    /// The control, and the thing an oracle compares against.
    ///
    /// **It hands back the frame it was given byte for byte, and that is
    /// measured rather than promised.** `h + (f - h) / 1` is `f` only where
    /// `f - h` is exact, which is not every pair of floats; on a real lit field it
    /// was identical in 50 of 50 frames, and `tests/temporal_accumulation.rs`
    /// asserts it over the synthetic one. **The limit is in the tree beside it**:
    /// the same test asserts that a history nine orders of magnitude above the
    /// fresh field does *not* come back exact, so the qualification is a fact
    /// rather than a sentence in a doc comment.
    pub const NONE: Self = Self { divisor: 1 };

    /// A blend that gives a fresh frame `1 / divisor` of the answer.
    ///
    /// # Errors
    ///
    /// [`RenderError::BlendNotPowerOfTwo`] if `divisor` is zero, above
    /// [`Self::MAX_DIVISOR`], or not a power of two.
    pub fn one_in(divisor: u32) -> Result<Self, RenderError> {
        if divisor == 0 || divisor > Self::MAX_DIVISOR || !divisor.is_power_of_two() {
            return Err(RenderError::BlendNotPowerOfTwo { divisor });
        }
        Ok(Self { divisor })
    }

    /// The divisor, as the kernel receives it.
    #[must_use]
    pub fn divisor(self) -> u32 {
        self.divisor
    }

    /// The fresh frame's share, as a fraction.
    ///
    /// Exact for every value this type can hold, because the divisor is a power
    /// of two below `2^23`.
    #[must_use]
    pub fn alpha(self) -> f32 {
        1.0 / self.divisor as f32
    }
}

/// Which of the two reprojections runs.
///
/// **Both are offered and the default is the exact one.** That is not the same
/// arrangement as [`MergeForm`](crate::MergeForm), where M8.6 recorded a decision;
/// here the decision is M8.7's own and its argument is a measurement, written up
/// in that task's report: the nearest arm returns one field on eight
/// adapter/backend pairs and the bilinear arm does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resample {
    /// The probe the source landed nearest to.
    ///
    /// A gather: an integer index in, a texel out, no arithmetic on a radiance
    /// value anywhere. Exact on every adapter, and the default.
    #[default]
    Nearest,
    /// The four probes around the source, weighted by where between them it fell.
    ///
    /// **Inexact by construction** — the weights are the fractional part of a
    /// continuous motion. `shaders/accumulate_bilinear.wgsl`'s header carries the
    /// argument for keeping it anyway.
    Bilinear,
}

/// How far the content moved across the field between two frames, in texels.
///
/// **Named fields rather than a `[f32; 2]`**, for ADR-0039's reason: two adjacent
/// same-typed numbers are a swap nothing catches, and a reprojection that
/// transposes its axes produces a plausible picture over a wrong field.
///
/// The sign convention is stated once, here: what is now at field position `p`
/// was at `p - motion` in the previous frame. A camera panning right moves the
/// content left, so `dx` is negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// Texels the content moved along x since the previous frame.
    pub dx: f32,
    /// Texels the content moved along y since the previous frame.
    pub dy: f32,
}

impl Motion {
    /// Nothing moved. The case in which the reprojection is the identity.
    pub const STILL: Self = Self { dx: 0.0, dy: 0.0 };
}

/// The thirty-two bytes both accumulation kernels' uniform holds.
///
/// A struct rather than a `[u8; 32]` built at the call site, for
/// [`BounceParams`](crate::surface::BounceParams)' reason and
/// [`StageLayout`](crate::StageLayout)'s: four of the six numbers are adjacent
/// same-typed integers, and a swap between `probes_x` and `probes_y` or between
/// `offset_x` and `offset_y` is a defect nothing else would catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccumulateParams {
    pub(crate) probes_x: u32,
    pub(crate) probes_y: u32,
    offset_x: i32,
    offset_y: i32,
    divisor: u32,
    has_history: u32,
}

impl AccumulateParams {
    /// The uniform's bytes, in the order `struct Accumulate` declares them.
    fn bytes(self) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        bytes[0..4].copy_from_slice(&self.probes_x.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.probes_y.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.offset_x.to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.offset_y.to_ne_bytes());
        bytes[16..20].copy_from_slice(&self.divisor.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.has_history.to_ne_bytes());
        bytes
    }
}

/// One axis of a [`Motion`], as the fixed-point offset the kernel reads.
///
/// **The whole of ADR-0050's reasoning applied to a camera.** The division and
/// the rounding happen here, once, in `f64`, on the CPU; a shader is handed an
/// `i32` and never sees a float motion. A `f64` division is correctly rounded by
/// IEEE 754 and `round` is exact, so two machines that agree on the inputs agree
/// on this number.
///
/// # Errors
///
/// [`RenderError::MotionOutOfRange`] if the motion is not finite or is so large
/// that the offset would leave [`MAX_REPROJECT_OFFSET`].
pub(crate) fn offset_of(name: &'static str, motion: f32, spacing: f64) -> Result<i32, RenderError> {
    let limit = f64::from(MAX_REPROJECT_OFFSET);
    if !motion.is_finite() {
        return Err(RenderError::MotionOutOfRange {
            name,
            value: motion,
            probes: f64::NAN,
        });
    }
    let probes = f64::from(motion) / spacing;
    let fixed = (probes * f64::from(REPROJECT_UNIT)).round();
    if !(-limit..=limit).contains(&fixed) {
        return Err(RenderError::MotionOutOfRange {
            name,
            value: motion,
            probes,
        });
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value was just bounded by MAX_REPROJECT_OFFSET, which is 2^29"
    )]
    let whole = fixed as i32;
    Ok(whole)
}

/// What the nearest arm actually applies, given a fixed-point offset.
///
/// **The CPU has to know this, and that is the whole reason the residual below
/// exists.** `accumulate.wgsl` computes `ix = (x * UNIT - offset + HALF) >> SHIFT`,
/// which is `x` plus `floor((HALF - offset) / UNIT)`, so the shift it performs is
/// a whole number of probes and the fraction is discarded. Rediscovering that
/// number here rather than storing it is deliberate: it is derived from the same
/// expression the kernel evaluates, in the same integer arithmetic, so the two
/// cannot drift apart without a test noticing.
///
/// Floor division on both sides of zero, which is what `>>` does on a signed
/// value and what `div_euclid` does here.
pub(crate) fn applied_offset(offset: i32) -> i32 {
    let half = REPROJECT_UNIT / 2;
    -((half - offset).div_euclid(REPROJECT_UNIT)) * REPROJECT_UNIT
}

/// A probe grid's lighting carried across frames.
///
/// Holds the accumulated field, the scratch the reprojection writes into, and a
/// field to upload each frame's fresh answer through.
/// [`OffscreenTarget::accumulate`](crate::OffscreenTarget::accumulate) advances it
/// by one frame.
///
/// **Its own type rather than a field on
/// [`SurfaceCache`](crate::SurfaceCache)**, and that is a decision rather than an
/// accident of layering: an accumulator takes a [`RadianceField`] per frame, so an
/// oracle can feed it a sequence it computed itself instead of dragging a whole
/// cascade behind every assertion. M8.6's named limit 6 is what that avoids — a
/// test that compares a step against a model built from the same machinery is
/// blind to a defect the two share.
///
/// No `wgpu` type appears in its public API, so it is an opaque handle in the
/// sense the crate header means.
#[derive(Debug)]
pub struct Accumulator {
    /// The accumulated field. `read()` is always the current one; a step writes
    /// the next into `write()` and swaps.
    pub(crate) history: FieldPair,
    /// What the reprojection wrote, and what the blend reads as history.
    pub(crate) reprojected: Field,
    /// This frame's fresh radiance, uploaded from the caller's field.
    pub(crate) fresh: Field,
    probes: [u32; 2],
    /// Texels between adjacent probes, in `f64` because the offset conversion is.
    pub(crate) spacing: f64,
    blend: Blend,
    resample: Resample,
    frames: u32,
    /// False until something has been accumulated or set. The first frame has
    /// nothing to reproject, and this is what says so.
    pub(crate) has_history: bool,
    /// The part of the motion the grid could not take, in fixed point.
    ///
    /// **Measured into existence rather than designed in.** A nearest
    /// reprojection shifts by a whole probe, so a camera moving a tenth of a probe
    /// per frame rounds to nothing *every frame* and the stored field never moves
    /// at all while the scene slides underneath it — a misregistration that grows
    /// without bound rather than a stair-step. M8.7's probe measured that at 0.10
    /// probes a frame it costs an error of 0.12 to 0.49 of the field's own RMS,
    /// against about 1e-5 at a whole probe a frame, and the collapse at whole
    /// speeds is what named the cause.
    ///
    /// Carrying what was not applied bounds the error at **half a probe** forever:
    /// the grid snaps by one probe whenever the arrears reach that, instead of
    /// standing still. It is integer arithmetic and costs nothing.
    ///
    /// Zero for [`Resample::Bilinear`], which applies the offset in full and has
    /// no arrears to keep.
    residual: [i32; 2],
}

impl Accumulator {
    /// How many frames have been accumulated.
    #[must_use]
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// The share a fresh frame is given.
    #[must_use]
    pub fn blend(&self) -> Blend {
        self.blend
    }

    /// Which reprojection it runs.
    #[must_use]
    pub fn resample(&self) -> Resample {
        self.resample
    }

    /// Probes across and down.
    #[must_use]
    pub fn probes(&self) -> [u32; 2] {
        self.probes
    }

    /// Whether the next frame has anything to reproject.
    ///
    /// False before the first [`accumulate`](crate::OffscreenTarget::accumulate)
    /// and after [`Self::forget`].
    #[must_use]
    pub fn has_history(&self) -> bool {
        self.has_history
    }

    /// The accumulated field.
    ///
    /// # Errors
    ///
    /// [`RenderError::Readback`] if the copy to the CPU fails.
    pub fn field(&self) -> Result<RadianceField, RenderError> {
        Ok(RadianceField::from_texels(
            self.probes[0],
            self.probes[1],
            self.history.read().read_back()?,
        ))
    }

    /// Replaces the accumulated field, so the next frame reprojects one of the
    /// caller's choosing.
    ///
    /// **Its consumers are the oracles** — an idempotence check needs to start
    /// from a field it knows, and a convergence check needs to restart from one.
    /// State that cannot be set is state that cannot be tested, which is the
    /// argument [`SurfaceCache::set_bounced`](crate::SurfaceCache::set_bounced)
    /// makes for the same shape.
    ///
    /// A field set this way **is** history: the next frame reprojects it.
    ///
    /// # Errors
    ///
    /// [`RenderError::RadianceGridMismatch`] if the field is not the
    /// accumulator's grid, and [`RenderError::FieldTexelCount`] if the upload is
    /// rejected, which cannot happen once the grid matches.
    pub fn set_field(&mut self, field: &RadianceField) -> Result<(), RenderError> {
        self.check_grid(field)?;
        self.history.read().write(field.texels())?;
        self.has_history = true;
        // A field a caller sets is history **on the current grid**, so nothing is
        // owed against it.
        self.residual = [0, 0];
        Ok(())
    }

    /// Drops the history without touching the frame count.
    ///
    /// The next frame is treated as a first one and returns its fresh field
    /// exactly. What a caller reaches for when a cut, a teleport or a scene change
    /// makes the previous frame meaningless — the case reprojection cannot answer
    /// and should not pretend to.
    pub fn forget(&mut self) {
        self.has_history = false;
        self.residual = [0, 0];
    }

    /// Refuses a field that is not this accumulator's grid.
    ///
    /// # Errors
    ///
    /// [`RenderError::RadianceGridMismatch`], naming both shapes.
    pub(crate) fn check_grid(&self, field: &RadianceField) -> Result<(), RenderError> {
        if field.width() != self.probes[0] || field.height() != self.probes[1] {
            return Err(RenderError::RadianceGridMismatch {
                grid_width: self.probes[0],
                grid_height: self.probes[1],
                field_width: field.width(),
                field_height: field.height(),
            });
        }
        Ok(())
    }

    /// What the grid still owes: the part of every motion so far that a whole
    /// probe of shift could not carry, in field texels.
    ///
    /// Bounded by half a probe on each axis for [`Resample::Nearest`], and always
    /// zero for [`Resample::Bilinear`], which applies an offset in full. It is
    /// what a caller reads to know how far the accumulated field is misregistered
    /// against the frame it is being blended with — the price of an exact
    /// reprojection, in the units the caller handed the motion in.
    #[must_use]
    pub fn unapplied(&self) -> Motion {
        let texels = |fixed: i32| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a residual is below half a probe, far inside f32's exact range"
            )]
            let out = (f64::from(fixed) / f64::from(REPROJECT_UNIT) * self.spacing) as f32;
            out
        };
        Motion {
            dx: texels(self.residual[0]),
            dy: texels(self.residual[1]),
        }
    }

    /// The uniform for one step, with the offset already in fixed point and the
    /// arrears from previous frames folded in.
    ///
    /// **The offset handed to the kernel is the whole of what is owed**, not this
    /// frame's motion alone. The kernel rounds it (nearest) or interpolates it
    /// (bilinear); what it did with it is recomputed here by [`applied_offset`],
    /// and the remainder is carried to the next frame. The `residual` field says
    /// what that is for and what it was measured to be worth.
    ///
    /// # Errors
    ///
    /// [`RenderError::MotionOutOfRange`] as [`offset_of`], and the same error if
    /// the arrears push the total past the bound. Both are raised before anything
    /// is written, so a refused motion leaves the arrears exactly as they were.
    pub(crate) fn params(&mut self, motion: Motion) -> Result<AccumulateParams, RenderError> {
        let owed = |name, value: f32, residual: i32| -> Result<i32, RenderError> {
            let offset = offset_of(name, value, self.spacing)?;
            offset
                .checked_add(residual)
                .filter(|total| (-MAX_REPROJECT_OFFSET..=MAX_REPROJECT_OFFSET).contains(total))
                .ok_or(RenderError::MotionOutOfRange {
                    name,
                    value,
                    probes: f64::from(offset) / f64::from(REPROJECT_UNIT),
                })
        };
        let total_x = owed("the motion's x", motion.dx, self.residual[0])?;
        let total_y = owed("the motion's y", motion.dy, self.residual[1])?;

        let applied = match self.resample {
            Resample::Nearest => [applied_offset(total_x), applied_offset(total_y)],
            Resample::Bilinear => [total_x, total_y],
        };
        self.residual = if self.has_history {
            [total_x - applied[0], total_y - applied[1]]
        } else {
            // Nothing was reprojected, so nothing is owed: the next frame starts
            // on this frame's own grid.
            [0, 0]
        };

        Ok(AccumulateParams {
            probes_x: self.probes[0],
            probes_y: self.probes[1],
            offset_x: total_x,
            offset_y: total_y,
            divisor: self.blend.divisor(),
            has_history: u32::from(self.has_history),
        })
    }

    /// Records that a frame has been accumulated.
    pub(crate) fn advanced(&mut self) {
        self.frames = self.frames.saturating_add(1);
        self.has_history = true;
        self.history.swap();
    }

    /// Level zero's probe grid as a [`RadianceField`], from texels just read back.
    pub(crate) fn field_of(&self, texels: Vec<f32>) -> RadianceField {
        RadianceField::from_texels(self.probes[0], self.probes[1], texels)
    }
}

/// The half of an accumulator's construction that needs no GPU.
///
/// Separate so that every refusal a caller can trigger is reachable from a
/// machine with no adapter, which is the same reason
/// [`surface::plan`](crate::surface::plan) is.
///
/// # Errors
///
/// [`RenderError::StageParameter`] if the stage's spacing is not a positive
/// finite number. Every other property of a stage was checked when it was built.
pub(crate) fn plan(stage: &CascadeStage) -> Result<([u32; 2], f64), RenderError> {
    let layout = stage.layout();
    if !layout.spacing.is_finite() || layout.spacing <= 0.0 {
        return Err(RenderError::StageParameter {
            name: "a probe spacing",
            value: layout.spacing,
            requirement: "be a positive finite number",
        });
    }
    Ok((layout.probes, f64::from(layout.spacing)))
}

/// Assembles an accumulator from its fields. The GPU half of the constructor.
pub(crate) fn assemble(
    history: FieldPair,
    reprojected: Field,
    fresh: Field,
    probes: [u32; 2],
    spacing: f64,
    blend: Blend,
    resample: Resample,
) -> Accumulator {
    Accumulator {
        history,
        reprojected,
        fresh,
        probes,
        spacing,
        blend,
        resample,
        frames: 0,
        has_history: false,
        residual: [0, 0],
    }
}

/// The three compiled accumulation pipelines, with the bindings they share.
///
/// One bind group layout and three pipelines, for
/// [`BounceKernel`](crate::surface::BounceKernel)'s reason: every entry point
/// binds the same *shapes* — two textures to read, one to write, one uniform — and
/// what changes is which resource goes where.
///
/// **Both reprojection arms are compiled every time**, even though a given
/// accumulator runs one. That is deliberate: a shader that is never compiled is a
/// shader nothing validates, and the cost is one module on a path that already
/// builds three pipelines.
#[derive(Debug)]
pub(crate) struct AccumulateKernel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    nearest: wgpu::ComputePipeline,
    bilinear: wgpu::ComputePipeline,
    blend: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl AccumulateKernel {
    /// Compiles both arms and the blend against the four bindings they share.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let read_texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("narvo accumulate bindings"),
            entries: &[
                read_texture(0),
                read_texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: crate::field::FIELD_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("narvo accumulate layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let module = |label: &str, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let exact = module("narvo accumulate", ACCUMULATE_WGSL);
        let inexact = module("narvo accumulate bilinear", ACCUMULATE_BILINEAR_WGSL);
        let pipeline = |label: &str, module: &wgpu::ShaderModule, entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                module,
                layout: Some(&pipeline_layout),
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };

        Self {
            device: device.clone(),
            queue: queue.clone(),
            nearest: pipeline("narvo reproject nearest", &exact, REPROJECT_ENTRY),
            bilinear: pipeline("narvo reproject bilinear", &inexact, REPROJECT_ENTRY),
            blend: pipeline("narvo blend", &exact, BLEND_ENTRY),
            layout,
        }
    }

    /// `target = resample(history, offset)`, fresh where there is no history.
    pub(crate) fn reproject(
        &self,
        resample: Resample,
        history: &Field,
        fresh: &Field,
        target: &Field,
        params: &AccumulateParams,
    ) {
        let (pipeline, label) = match resample {
            Resample::Nearest => (&self.nearest, "narvo reproject nearest"),
            Resample::Bilinear => (&self.bilinear, "narvo reproject bilinear"),
        };
        self.run(pipeline, history, fresh, target, params, label);
    }

    /// `target = history + (fresh - history) / divisor`, one probe per invocation.
    pub(crate) fn blend(
        &self,
        history: &Field,
        fresh: &Field,
        target: &Field,
        params: &AccumulateParams,
    ) {
        self.run(&self.blend, history, fresh, target, params, "narvo blend");
    }

    /// The half every entry point shares: bind four resources and dispatch over
    /// the probe grid.
    fn run(
        &self,
        pipeline: &wgpu::ComputePipeline,
        a: &Field,
        b: &Field,
        target: &Field,
        params: &AccumulateParams,
        label: &str,
    ) {
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uniform, 0, &params.bytes());

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(a.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(b.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(target.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                params.probes_x.div_ceil(ACCUMULATE_WORKGROUP),
                params.probes_y.div_ceil(ACCUMULATE_WORKGROUP),
                1,
            );
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ACCUMULATE_BILINEAR_WGSL, ACCUMULATE_WGSL, ACCUMULATE_WORKGROUP, BLEND_ENTRY, Blend,
        MAX_REPROJECT_OFFSET, Motion, REPROJECT_ENTRY, REPROJECT_SHIFT, REPROJECT_UNIT, Resample,
        offset_of,
    };
    use crate::RenderError;

    fn uncommented(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -- the source guards --------------------------------------------------

    /// **ADR-0049's guard, for both arms.**
    ///
    /// The same reasoning as `surface.rs`'s, `cascade.rs`'s and `hierarchy.rs`'s:
    /// an order-dependent write produces a plausible number that no output
    /// comparison can report, so the only check there is is that the machinery is
    /// absent.
    #[test]
    fn the_accumulation_is_not_written_order_dependently() {
        for (name, source) in [
            ("the accumulation", ACCUMULATE_WGSL),
            ("the bilinear arm", ACCUMULATE_BILINEAR_WGSL),
        ] {
            let body = uncommented(source);
            for forbidden in [
                "atomic",
                "workgroupBarrier",
                "storageBarrier",
                "workgroupUniformLoad",
                "var<workgroup>",
            ] {
                assert!(
                    !body.contains(forbidden),
                    "{name} contains `{forbidden}`, which makes it depend on the order \
                     invocations run in"
                );
            }
        }
    }

    /// **ADR-0051's guard, and the strongest form of it this crate has.**
    ///
    /// `cascade.wgsl` says it has no float multiply because its normalisation is a
    /// division by a power of two; `bounce.wgsl` says its three multiplies feed no
    /// add because a texel store separates them. This file says the **first**
    /// thing, for a blend — which is normally the archetype of a multiply feeding
    /// an add.
    ///
    /// Two properties are read, and the second is the one that would be lost by an
    /// innocent-looking edit:
    ///
    /// 1. **there is no `*` outside a comment anywhere in the exact arm**, so
    ///    there is nothing for a backend to contract; and
    /// 2. **the blend divides**, which is what makes (1) possible at all. A
    ///    session that writes `* alpha` instead has to change this line, and then
    ///    has to say which eight adapter/backend pairs it re-measured.
    #[test]
    fn no_float_multiplication_in_the_exact_arm_at_all() {
        let body = uncommented(ACCUMULATE_WGSL);
        let starred: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|line| line.contains('*'))
            .collect();
        assert!(
            starred.is_empty(),
            "the exact arm multiplies, in {starred:?}. A float multiply feeding a float \
             add is the one shape ADR-0051 measured returning two radiance fields; the \
             blend divides by a power of two so that there is nothing to contract"
        );
        assert!(
            body.contains("/ divisor"),
            "the blend does not divide by the divisor, so the guard above proves nothing"
        );
        assert!(
            !body.contains("fma("),
            "the exact arm asks for a fused multiply-add"
        );
    }

    /// **The inexact arm is inexact, and that is asserted rather than assumed.**
    ///
    /// A guard in the other direction, and it earns its place twice over: it names
    /// the six products so that a session cannot quietly change what the
    /// measurement's second arm measures, and it fails if somebody "fixes" the
    /// bilinear resample into the exact arm's shape — at which point the two arms
    /// would return the same field and M8.7's §2 measurement would silently
    /// compare a thing with itself.
    #[test]
    fn the_bilinear_arm_carries_the_products_it_exists_to_carry() {
        let body = uncommented(ACCUMULATE_BILINEAR_WGSL);
        let starred: Vec<String> = body
            .lines()
            .map(str::trim)
            .filter(|line| line.contains('*'))
            .map(str::to_owned)
            .collect();
        assert_eq!(
            starred,
            vec![
                "let top = v00 * (1.0 - wx) + v10 * wx;".to_owned(),
                "let bottom = v01 * (1.0 - wx) + v11 * wx;".to_owned(),
                "let value = top * (1.0 - wy) + bottom * wy;".to_owned(),
            ],
            "the bilinear arm's weighting is not the three registered lines. It is the \
             second arm of M8.7's measurement and it has to keep being the inexact one"
        );
    }

    /// **ADR-0050's guard: the reprojection's index arithmetic is integer.**
    ///
    /// The shape ADR-0050 requires and the shape no output comparison can see —
    /// which is exactly why it has to be a source read. A float index would agree
    /// with an integer one at every grid this repository's tests use and part
    /// company at the sizes `MAX_DIMENSION` allows, and it would part company
    /// *differently per rasteriser family*.
    ///
    /// Both arms are read: the bilinear one is inexact in its *weights* and must
    /// still be exact in its *indices*, because a tap that lands on the wrong probe
    /// is not a rounding, it is a wrong answer.
    #[test]
    fn the_reprojection_indexes_in_integers() {
        for (name, source) in [
            ("the accumulation", ACCUMULATE_WGSL),
            ("the bilinear arm", ACCUMULATE_BILINEAR_WGSL),
        ] {
            let body = uncommented(source);
            for line in body.lines().map(str::trim) {
                let is_index = line.starts_with("let sx")
                    || line.starts_with("let sy")
                    || line.starts_with("let ix")
                    || line.starts_with("let iy")
                    || line.starts_with("let x0")
                    || line.starts_with("let y0")
                    || line.starts_with("let x1")
                    || line.starts_with("let y1")
                    || line.starts_with("let fx")
                    || line.starts_with("let fy");
                if is_index {
                    assert!(
                        !line.contains("f32"),
                        "{name} computes `{line}` through an f32. ADR-0050 requires a \
                         comparison over coordinates to be written in integers"
                    );
                }
            }
        }
    }

    /// The dispatch and every entry point agree on the workgroup, and the two
    /// files agree on the fixed-point shift.
    #[test]
    fn the_dispatch_and_the_accumulation_agree_on_their_constants() {
        let declared = format!("@workgroup_size({ACCUMULATE_WORKGROUP}, {ACCUMULATE_WORKGROUP})");
        let shift = format!("const SHIFT: u32 = {REPROJECT_SHIFT}u;");
        for (name, source) in [
            ("the accumulation", ACCUMULATE_WGSL),
            ("the bilinear arm", ACCUMULATE_BILINEAR_WGSL),
        ] {
            assert!(
                source.contains(&declared),
                "{name} does not declare `{declared}`"
            );
            assert!(
                source.contains(&shift),
                "{name} does not declare `{shift}`, so the CPU and the kernel disagree \
                 about what a fixed-point offset means"
            );
            assert!(
                source.contains(&format!("fn {REPROJECT_ENTRY}(")),
                "{name} has no `{REPROJECT_ENTRY}` entry point"
            );
        }
        assert!(
            ACCUMULATE_WGSL.contains(&format!("fn {BLEND_ENTRY}(")),
            "the accumulation has no `{BLEND_ENTRY}` entry point"
        );
        assert!(
            ACCUMULATE_BILINEAR_WGSL.contains(&format!("const UNIT: i32 = {REPROJECT_UNIT};")),
            "the bilinear arm's UNIT is not `1 << {REPROJECT_SHIFT}`"
        );
    }

    // -- the blend weight ---------------------------------------------------

    /// A divisor that is not a power of two is refused, and that is ADR-0051's
    /// exactness rather than a taste in numbers.
    #[test]
    fn a_blend_that_is_not_a_power_of_two_is_refused() {
        for divisor in [0, 3, 6, 100, Blend::MAX_DIVISOR + 1, u32::MAX] {
            match Blend::one_in(divisor) {
                Err(RenderError::BlendNotPowerOfTwo { divisor: named }) => {
                    assert_eq!(named, divisor, "the refusal names the wrong divisor");
                }
                other => panic!("a divisor of {divisor} was not refused: {other:?}"),
            }
        }
    }

    /// Every power of two up to the ceiling is accepted, and its share is exact.
    #[test]
    fn every_power_of_two_share_is_exact() {
        let mut divisor = 1_u32;
        while divisor <= Blend::MAX_DIVISOR {
            let blend = Blend::one_in(divisor).expect("a power of two");
            assert_eq!(blend.divisor(), divisor);
            // Exact for a power of two: the reciprocal only negates an exponent.
            assert_eq!(
                blend.alpha() * divisor as f32,
                1.0,
                "the share of one in {divisor} does not multiply back to one"
            );
            divisor <<= 1;
        }
    }

    /// The control keeps no history at all.
    #[test]
    fn a_blend_of_one_in_one_keeps_no_history() {
        assert_eq!(Blend::NONE.divisor(), 1);
        assert_eq!(Blend::NONE.alpha(), 1.0);
        assert_eq!(
            Blend::one_in(1).expect("one is a power of two"),
            Blend::NONE
        );
    }

    // -- the fixed-point offset ---------------------------------------------

    /// The offset, or a panic naming the refusal. `RenderError` carries a boxed
    /// source and therefore no `PartialEq`, so the tests below compare the value
    /// rather than the `Result`.
    fn offset(motion: f32, spacing: f64) -> i32 {
        offset_of("the motion's x", motion, spacing)
            .unwrap_or_else(|error| panic!("a motion of {motion} at spacing {spacing}: {error}"))
    }

    /// Zero motion is zero offset, on both axes and at every spacing — which is
    /// the arithmetic half of §3's identity assurance. The GPU half is in
    /// `tests/temporal_accumulation.rs`, and it is the one that counts.
    #[test]
    fn no_motion_is_no_offset() {
        for spacing in [1.0, 2.0, 4.0, 7.5, 1024.0] {
            assert_eq!(offset(Motion::STILL.dx, spacing), 0);
            assert_eq!(offset(Motion::STILL.dy, spacing), 0);
        }
    }

    /// A whole probe of motion is a whole `REPROJECT_UNIT` of offset, and the
    /// division is exact where the spacing divides the motion.
    #[test]
    fn a_whole_probe_of_motion_is_a_whole_unit_of_offset() {
        assert_eq!(offset(4.0, 4.0), REPROJECT_UNIT);
        assert_eq!(offset(-4.0, 4.0), -REPROJECT_UNIT);
        assert_eq!(offset(12.0, 4.0), 3 * REPROJECT_UNIT);
        assert_eq!(offset(2.0, 4.0), REPROJECT_UNIT / 2);
        assert_eq!(offset(1.0, 4.0), REPROJECT_UNIT / 4);
    }

    /// A motion that cannot be expressed is refused rather than wrapped.
    #[test]
    fn a_motion_that_cannot_be_expressed_in_fixed_point_is_refused() {
        // 2^29 fixed-point units is 8192 probes; one probe more overflows the
        // bound MAX_REPROJECT_OFFSET names.
        let probes = f64::from(MAX_REPROJECT_OFFSET) / f64::from(REPROJECT_UNIT);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "8192 is exactly representable and the test is about the bound, not the cast"
        )]
        let over = (probes + 1.0) as f32;
        for motion in [over, -over, f32::INFINITY, f32::NAN, f32::MIN, f32::MAX] {
            assert!(
                matches!(
                    offset_of("the motion's x", motion, 1.0),
                    Err(RenderError::MotionOutOfRange { .. })
                ),
                "a motion of {motion} was not refused"
            );
        }
        // And the bound itself is inside.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "8192 is exactly representable in f32"
        )]
        let edge = probes as f32;
        assert_eq!(offset(edge, 1.0), MAX_REPROJECT_OFFSET);
    }

    /// The default reprojection is the exact one.
    ///
    /// **A test rather than a comment**, because the choice is M8.7's decision and
    /// a decision that nothing reads is a preference. The argument is the
    /// measurement in that task's report: the nearest arm returns one field on
    /// eight adapter/backend pairs.
    #[test]
    fn the_default_reprojection_is_the_exact_one() {
        assert_eq!(Resample::default(), Resample::Nearest);
    }

    /// A motion is two named numbers, and swapping them is a different motion.
    ///
    /// ADR-0039's reasoning, checked rather than asserted in a doc comment.
    #[test]
    fn a_motion_names_its_axes() {
        let along_x = Motion { dx: 3.0, dy: 0.0 };
        let along_y = Motion { dx: 0.0, dy: 3.0 };
        assert_ne!(along_x, along_y);
        assert_eq!(offset(along_x.dx, 1.0), offset(along_y.dy, 1.0));
        assert_eq!(offset(along_x.dy, 1.0), 0);
    }
}
