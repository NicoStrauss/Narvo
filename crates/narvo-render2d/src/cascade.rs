//! One cascade stage: a grid of probes, each integrating over its own interval.
//!
//! **M8.5a's capability, and the first thing in this crate that produces a field
//! of `f32` nobody drew.** A probe has a position, a distance interval and a set
//! of directions. It marches every direction over its own interval — *not*
//! beyond it — and integrates what it finds into one radiance per probe.
//!
//! # What it consumes, and what it does not rebuild
//!
//! The march is M8.4's, unchanged: the same `march.wgsl`, the same
//! [`MarchKernel`](crate::march::MarchKernel), the same three verdicts. A stage
//! is two compute passes in one encoder — M8.4's march, then this module's
//! integration — with the hits never leaving the GPU in between. That is
//! ADR-0049's chain doing exactly what it was built for.
//!
//! **One thing about M8.4 did not fit and is reported rather than changed.** Its
//! public shape is a *list* of rays: `Ray` is 32 bytes of CPU-built fixed point,
//! and a cascade's rays are not a list, they are a function of a probe index and
//! a direction index. At this stage's scale that costs nothing; at a level-zero
//! covering 1920 x 1080 at one probe per texel, with the eight directions such a
//! level needs, it is **796 MB of ray and hit buffer** — and four times over the
//! ceiling one dispatch reaches ([`CascadeStage::MAX_RAYS`]) — for arithmetic the
//! kernel could do in two multiplies. The report carries the budget. Nothing in `march.rs` was changed to avoid it, because
//! the march *works* — what does not scale is the way a caller has to say what
//! it wants marched, and that is M8.5b's decision to make rather than this
//! task's.
//!
//! # The rule this file is written under
//!
//! ADR-0049 forbids an order-dependent merge. An integration over directions is
//! a sum, which is exactly the shape that becomes order-dependent the moment it
//! falls into a shared accumulator. It does not here: one invocation owns one
//! probe and walks that probe's directions in a loop the shader writes down, so
//! the summation order is fixed by the source. No atomic, no barrier, no
//! workgroup storage — and `cascade.wgsl`'s guard is a **source read**, because
//! a wrong summation order produces a plausible number that no output comparison
//! can report.
//!
//! ADR-0049 permits a barrier-and-shared-memory tree, and this does not need
//! one: the parallelism is the probe grid. **That is a property of a level with
//! many probes, and the top of a cascade is the opposite** — few probes, many
//! directions — so the reopening condition is named rather than left to be
//! discovered: a level whose probe count falls below one workgroup per compute
//! unit wants the tree, and M8.5b is where that is measured.
//!
//! # Why there is no `f32` multiplication in the kernel
//!
//! `shaders/cascade.wgsl`'s header carries the measurement; the short form is
//! that a radiance field is byte-identical across all eight adapter/backend
//! pairs when the integration contains no float multiply, and splits into two
//! fields along rasteriser families when it contains one inexact product feeding
//! an add. The direction count is required to be a power of two so the
//! normalisation can be a division that is exact.

use crate::error::RenderError;
use crate::field::{FIELD_CHANNELS, Field};
use crate::march::Ray;

/// The integration kernel's source.
pub(crate) const CASCADE_WGSL: &str = include_str!("shaders/cascade.wgsl");

/// The entry point in [`CASCADE_WGSL`].
pub(crate) const CASCADE_ENTRY: &str = "integrate";

/// Invocations per workgroup. One dimension: a probe list is a list.
pub(crate) const CASCADE_WORKGROUP: u32 = 64;

/// How much light every texel of the field gives off.
///
/// Plain data, like [`Seeds`](crate::Seeds), and for the same reason: building
/// one needs no GPU, so a stage's inputs can be constructed and checked on a
/// machine with no adapter at all.
///
/// **Only seeded texels are ever read.** A direction that stops does so because
/// the distance field named a seed, and the emission is read *at that seed*
/// (`cascade.wgsl` says why). Emission at an unseeded texel is therefore
/// inert — it is stored rather than refused, because refusing it would make
/// "which texels are occluders" a fact this type had to know, and it does not.
#[derive(Debug, Clone, PartialEq)]
pub struct Emission {
    width: u32,
    height: u32,
    /// Four floats a texel, so it can be uploaded as a field without a repack.
    /// The fourth is unused and stays zero.
    texels: Vec<f32>,
}

impl Emission {
    /// A `width` x `height` map that emits nothing.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidSize`] if a dimension is zero or above
    /// [`OffscreenTarget::MAX_DIMENSION`](crate::OffscreenTarget::MAX_DIMENSION).
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let max = crate::OffscreenTarget::MAX_DIMENSION;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(RenderError::InvalidSize { width, height, max });
        }
        Ok(Self {
            width,
            height,
            texels: vec![0.0; width as usize * height as usize * FIELD_CHANNELS],
        })
    }

    /// Sets the texel at `(x, y)` to emit `rgb`.
    ///
    /// # Errors
    ///
    /// [`RenderError::EmissionOutsideField`] if the point is outside the map,
    /// and [`RenderError::StageParameter`] if a channel is not a finite,
    /// non-negative number — a radiance below zero is not a dim light, it is a
    /// defect, and it would travel silently into a sum.
    pub fn set(&mut self, x: u32, y: u32, rgb: [f32; 3]) -> Result<(), RenderError> {
        if x >= self.width || y >= self.height {
            return Err(RenderError::EmissionOutsideField {
                x,
                y,
                width: self.width,
                height: self.height,
            });
        }
        for value in rgb {
            if !value.is_finite() || value < 0.0 {
                return Err(RenderError::StageParameter {
                    name: "an emission channel",
                    value,
                    requirement: "be finite and not negative",
                });
            }
        }
        let base = (y as usize * self.width as usize + x as usize) * FIELD_CHANNELS;
        self.texels[base] = rgb[0];
        self.texels[base + 1] = rgb[1];
        self.texels[base + 2] = rgb[2];
        Ok(())
    }

    /// What `(x, y)` emits, or `None` outside the map.
    #[must_use]
    pub fn get(&self, x: u32, y: u32) -> Option<[f32; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let base = (y as usize * self.width as usize + x as usize) * FIELD_CHANNELS;
        Some([
            self.texels[base],
            self.texels[base + 1],
            self.texels[base + 2],
        ])
    }

    /// Width in texels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in texels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The field texels this map uploads as.
    pub(crate) fn texels(&self) -> &[f32] {
        &self.texels
    }
}

/// Where a stage's probes are, how far they look, and in how many directions.
///
/// **Named fields rather than positional arguments**, and that is ADR-0039's
/// reasoning applied one level up: `origin`, `near` and `far` would otherwise be
/// three adjacent `f32` that nothing catches a caller for swapping. A stage is
/// six numbers and a colour; six numbers in a row is a defect waiting to be
/// written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageLayout {
    /// Where probe `(0, 0)` sits, in field texels.
    pub origin: [f32; 2],
    /// Texels between adjacent probes, both axes.
    pub spacing: f32,
    /// Probes across and down.
    pub probes: [u32; 2],
    /// Where each direction starts, as a distance from the probe.
    pub near: f32,
    /// Where each direction stops. **Not marched beyond**, which is the whole
    /// of what makes a level of a cascade a level.
    pub far: f32,
    /// Directions per probe, uniformly spaced over the circle. A power of two.
    pub directions: u32,
    /// What a direction that reaches `far` without meeting anything carries.
    pub far_radiance: [f32; 3],
}

/// A validated [`StageLayout`].
///
/// The validation is the point: [`CascadeStage::new`] is where §3's penumbra
/// inequality is a **checked assurance beside the parameters** rather than a
/// comment somebody has to have read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CascadeStage {
    layout: StageLayout,
}

impl CascadeStage {
    /// The most rays one stage may ask for.
    ///
    /// **Derived, not chosen.** The march dispatches one invocation per ray in
    /// workgroups of [`crate::march::MARCH_WORKGROUP`] = 64, along one
    /// dimension, and `Limits::defaults()` sets
    /// `max_compute_workgroups_per_dimension: 65535`
    /// (`wgpu-types-30.0.0/src/limits.rs:456`) — which is what
    /// [`OffscreenTarget::new`](crate::OffscreenTarget::new) asks the device
    /// for. So one dispatch reaches 65 535 x 64 rays and no more.
    ///
    /// **`MarchKernel::run` does not check this and M8.5a did not make it**, on
    /// §2's rule that a march which does not fit is a finding rather than a
    /// licence: a caller handing `march` five million rays today gets a wgpu
    /// validation failure rather than a `RenderError`, and that gap is M8.4's to
    /// close. The check is here because a stage's ray count is derived from its
    /// own parameters, so this is where a caller can be told what to change.
    ///
    /// At 48 bytes a ray — 32 in the ray buffer, 16 in the hit buffer — the
    /// ceiling is 201 MB of GPU buffer, which is the other reason not to raise
    /// it without a measurement.
    pub const MAX_RAYS: u32 = 65_535 * crate::march::MARCH_WORKGROUP;

    /// Validates a layout.
    ///
    /// # The penumbra inequality, derived
    ///
    /// Two adjacent directions separate as they travel. At the **far** end of
    /// the interval they are `far * (2 * pi / directions)` apart, measured along
    /// the arc. An occluder narrower than that gap falls between them and is
    /// missed, and the picture gets a stripe where it should have a soft edge.
    ///
    /// The narrowest occluder this engine can represent is **one field texel**,
    /// because a seed is a texel ([`Seeds::set`](crate::Seeds::set)). So the
    /// bound comes from the texel size and from nothing else:
    ///
    /// ```text
    ///     far * 2 * pi / directions  <=  1 texel
    ///     directions  >=  ceil(2 * pi * far)
    /// ```
    ///
    /// The arc is used rather than the chord, which is the conservative choice:
    /// the chord `2 * far * sin(pi / directions)` is never longer, so a stage
    /// that satisfies this satisfies the chord form too.
    ///
    /// It is checkable on a single stage even though its purpose is the
    /// hierarchy, because `far` and `directions` both stand right here. In a
    /// cascade it is what forces directions to grow as intervals do — the
    /// classical `interval x 4, directions x 4, probes / 4` scheme is this
    /// inequality held at equality.
    ///
    /// # Errors
    ///
    /// - [`RenderError::StageParameter`] if a number is not finite, the spacing
    ///   is not positive, the grid is empty, the interval runs backwards or
    ///   starts below zero, or a far-radiance channel is negative.
    /// - [`RenderError::DirectionsNotPowerOfTwo`] if `directions` is zero or not
    ///   a power of two. That is not tidiness: it is what makes the kernel's
    ///   normalisation an exact division and lets the kernel hold no float
    ///   multiply at all.
    /// - [`RenderError::PenumbraUnderresolved`] if the inequality above fails.
    /// - [`RenderError::StageTooLarge`] if the stage asks for more than
    ///   [`Self::MAX_RAYS`].
    pub fn new(layout: StageLayout) -> Result<Self, RenderError> {
        let scalars = [
            ("a probe grid origin", layout.origin[0]),
            ("a probe grid origin", layout.origin[1]),
            ("a probe spacing", layout.spacing),
            ("a stage's near end", layout.near),
            ("a stage's far end", layout.far),
        ];
        for (name, value) in scalars {
            if !value.is_finite() {
                return Err(RenderError::StageParameter {
                    name,
                    value,
                    requirement: "be a finite number",
                });
            }
        }
        for value in layout.far_radiance {
            if !value.is_finite() || value < 0.0 {
                return Err(RenderError::StageParameter {
                    name: "a far-radiance channel",
                    value,
                    requirement: "be finite and not negative",
                });
            }
        }
        if layout.spacing <= 0.0 {
            return Err(RenderError::StageParameter {
                name: "a probe spacing",
                value: layout.spacing,
                requirement: "be greater than zero",
            });
        }
        if layout.near < 0.0 {
            return Err(RenderError::StageParameter {
                name: "a stage's near end",
                value: layout.near,
                requirement: "be at least zero",
            });
        }
        if layout.far < layout.near {
            return Err(RenderError::StageParameter {
                name: "a stage's far end",
                value: layout.far,
                requirement: "be at least the near end",
            });
        }
        if layout.probes[0] == 0 || layout.probes[1] == 0 {
            return Err(RenderError::StageParameter {
                name: "a probe grid side",
                value: 0.0,
                requirement: "hold at least one probe on each axis",
            });
        }
        if layout.directions == 0 || !layout.directions.is_power_of_two() {
            return Err(RenderError::DirectionsNotPowerOfTwo {
                directions: layout.directions,
            });
        }

        let required = Self::directions_required(layout.far);
        if layout.directions < required {
            return Err(RenderError::PenumbraUnderresolved {
                directions: layout.directions,
                far: layout.far,
                required,
            });
        }

        let rays = u64::from(layout.probes[0])
            * u64::from(layout.probes[1])
            * u64::from(layout.directions);
        if rays > u64::from(Self::MAX_RAYS) {
            return Err(RenderError::StageTooLarge {
                rays,
                limit: Self::MAX_RAYS,
            });
        }

        Ok(Self { layout })
    }

    /// The smallest direction count the penumbra inequality allows for `far`.
    ///
    /// `ceil(2 * pi * far)`, and never below one: a stage whose far end is zero
    /// marches degenerate rays, which is legal and answers about the probe's own
    /// texel.
    #[must_use]
    pub fn directions_required(far: f32) -> u32 {
        let arc = std::f64::consts::TAU * f64::from(far);
        if !arc.is_finite() || arc <= 1.0 {
            return 1;
        }
        // `arc` is at most TAU * 8192, far inside u32.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the value is positive and bounded by TAU * MAX_DIMENSION"
        )]
        let required = arc.ceil() as u32;
        required
    }

    /// The layout this stage was built from.
    #[must_use]
    pub fn layout(&self) -> StageLayout {
        self.layout
    }

    /// How many probes the grid holds.
    #[must_use]
    pub fn probe_count(&self) -> u32 {
        self.layout.probes[0] * self.layout.probes[1]
    }

    /// How many rays a run of this stage marches.
    ///
    /// `probes * directions`, and the number a caller sizing a budget wants: at
    /// 48 bytes a ray it is also the stage's GPU buffer cost.
    #[must_use]
    pub fn ray_count(&self) -> u32 {
        self.probe_count() * self.layout.directions
    }

    /// Where probe `(x, y)` sits, or `None` outside the grid.
    #[must_use]
    pub fn probe_position(&self, x: u32, y: u32) -> Option<[f32; 2]> {
        if x >= self.layout.probes[0] || y >= self.layout.probes[1] {
            return None;
        }
        let [px, py] = self.position(x, y);
        // Exact for every position this crate accepts: a coordinate is below
        // 8192 and an f32 holds every value a f64 sum of two such produces to
        // well inside its precision.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a field coordinate is far inside f32's exact range"
        )]
        let out = [px as f32, py as f32];
        Some(out)
    }

    /// Probe `(x, y)`'s position, in `f64` for the ray arithmetic.
    fn position(&self, x: u32, y: u32) -> [f64; 2] {
        [
            f64::from(self.layout.origin[0]) + f64::from(x) * f64::from(self.layout.spacing),
            f64::from(self.layout.origin[1]) + f64::from(y) * f64::from(self.layout.spacing),
        ]
    }

    /// Checks that this stage fits inside a `width` x `height` field.
    ///
    /// Every probe must lie at least `near` from every edge. That is exactly the
    /// condition for the disc of radius `near` around a probe to be inside an
    /// axis-parallel rectangle, so it is one comparison per side rather than one
    /// per direction — and with `near` at zero it is simply "inside the field",
    /// which is no restriction at all.
    ///
    /// The **far** end needs no such condition: a direction that would leave the
    /// field is clipped at the boundary, and that is not an approximation. The
    /// field says nothing about what is outside it, so a direction that leaves
    /// has met nothing, which is what [`StageLayout::far_radiance`] is for.
    ///
    /// # Errors
    ///
    /// [`RenderError::ProbeOutsideField`] naming the offending corner.
    pub(crate) fn check_fits(&self, width: u32, height: u32) -> Result<(), RenderError> {
        let near = f64::from(self.layout.near);
        let last = self.position(self.layout.probes[0] - 1, self.layout.probes[1] - 1);
        let first = self.position(0, 0);
        let limits = [f64::from(width), f64::from(height)];
        for axis in 0..2 {
            let low = first[axis] - near;
            let high = last[axis] + near;
            if low < 0.0 || high > limits[axis] {
                let offending = if low < 0.0 { first } else { last };
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a probe position is bounded by the field, far inside f32"
                )]
                let (x, y) = (offending[0] as f32, offending[1] as f32);
                return Err(RenderError::ProbeOutsideField {
                    x,
                    y,
                    near: self.layout.near,
                    width,
                    height,
                });
            }
        }
        Ok(())
    }

    /// Every probe's every direction, probe-major, clipped to the field.
    ///
    /// The order is the one `cascade.wgsl` indexes with: probe `p`'s direction
    /// `k` is ray `p * directions + k`, and probes run row-major from
    /// `(0, 0)`. Nothing else knows that layout, which is why it is stated here
    /// and read there rather than being a convention two files share.
    ///
    /// # Errors
    ///
    /// As [`Ray::new`], which cannot in fact fail here — every endpoint is
    /// clipped into the field before it is offered — and is threaded rather than
    /// unwrapped because that is an invariant of *this function* rather than of
    /// the types.
    pub(crate) fn rays(&self, width: u32, height: u32) -> Result<Vec<Ray>, RenderError> {
        let mut rays = Vec::with_capacity(self.ray_count() as usize);
        let limits = [f64::from(width), f64::from(height)];
        let directions = self.layout.directions;
        for y in 0..self.layout.probes[1] {
            for x in 0..self.layout.probes[0] {
                let p = self.position(x, y);
                for k in 0..directions {
                    let theta = std::f64::consts::TAU * f64::from(k) / f64::from(directions);
                    let d = [theta.cos(), theta.sin()];
                    let exit = exit_distance(p, d, limits);
                    let t0 = f64::from(self.layout.near).min(exit);
                    let t1 = f64::from(self.layout.far).min(exit);
                    let from = clip(p, d, t0, limits);
                    let to = clip(p, d, t1, limits);
                    rays.push(Ray::new(from[0], from[1], to[0], to[1], width, height)?);
                }
            }
        }
        Ok(rays)
    }

    /// The forty-eight bytes the kernel's uniform holds.
    fn params(&self, width: u32, height: u32) -> [u8; 48] {
        let mut bytes = [0_u8; 48];
        bytes[0..4].copy_from_slice(&self.probe_count().to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.layout.directions.to_ne_bytes());
        bytes[8..12].copy_from_slice(&width.to_ne_bytes());
        bytes[12..16].copy_from_slice(&height.to_ne_bytes());
        bytes[16..20].copy_from_slice(&self.layout.probes[0].to_ne_bytes());
        // Words five to seven are the shader's named padding and stay zero.
        bytes[32..36].copy_from_slice(&self.layout.far_radiance[0].to_ne_bytes());
        bytes[36..40].copy_from_slice(&self.layout.far_radiance[1].to_ne_bytes());
        bytes[40..44].copy_from_slice(&self.layout.far_radiance[2].to_ne_bytes());
        bytes
    }
}

/// How far a ray from `p` in direction `d` gets before it leaves the rectangle.
///
/// The slab form, in `f64`. `p` is inside the rectangle — [`CascadeStage::check_fits`]
/// has said so — so the answer is never negative, and a direction parallel to an
/// axis contributes no bound on that axis rather than an infinite one.
fn exit_distance(p: [f64; 2], d: [f64; 2], limits: [f64; 2]) -> f64 {
    let mut t = f64::INFINITY;
    for axis in 0..2 {
        if d[axis] > 0.0 {
            t = t.min((limits[axis] - p[axis]) / d[axis]);
        } else if d[axis] < 0.0 {
            t = t.min(-p[axis] / d[axis]);
        }
    }
    if t.is_finite() { t.max(0.0) } else { 0.0 }
}

/// `p + d * t`, as `f32` inside the field.
///
/// The clamp is against `f64`-to-`f32` rounding pushing a point that is exactly
/// on the boundary a hair past it, which [`Ray::new`] would refuse. It is not
/// doing the clipping — [`exit_distance`] already did that, in `f64`.
fn clip(p: [f64; 2], d: [f64; 2], t: f64, limits: [f64; 2]) -> [f32; 2] {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value is inside the field, far inside f32's exact range"
    )]
    let out = [
        ((p[0] + d[0] * t) as f32).clamp(0.0, limits[0] as f32),
        ((p[1] + d[1] * t) as f32).clamp(0.0, limits[1] as f32),
    ];
    out
}

/// What one stage computed: a radiance per probe, and how much it still owes.
#[derive(Debug, Clone, PartialEq)]
pub struct RadianceField {
    width: u32,
    height: u32,
    texels: Vec<f32>,
}

impl RadianceField {
    /// The mean radiance at probe `(x, y)`, or `None` outside the grid.
    ///
    /// A **mean over directions**, not a flux: the sum of what the directions
    /// found, divided by how many there were. Multiplying by `2 * pi` gives the
    /// irradiance in the plane, and the report's §4(c) derives which of the two
    /// the 1/r law belongs to.
    #[must_use]
    pub fn radiance(&self, x: u32, y: u32) -> Option<[f32; 3]> {
        let base = self.base(x, y)?;
        Some([
            self.texels[base],
            self.texels[base + 1],
            self.texels[base + 2],
        ])
    }

    /// The share of directions that left the interval without meeting anything.
    ///
    /// **What this stage still owes the level above.** A stage is one annulus of
    /// a cascade; a direction that reached `far` has not been answered by this
    /// stage at all, and carried [`StageLayout::far_radiance`] as a placeholder.
    /// With that placeholder at zero, `radiance + escaped * above` is the
    /// composition M8.5b performs, which is why the two are stored beside each
    /// other rather than one being derivable from the other.
    ///
    /// A direction that ran out of steps is **not** counted here: it established
    /// nothing, so it is neither answered nor owed, and lumping it in would let
    /// an exhausted march be paid for twice.
    #[must_use]
    pub fn escaped(&self, x: u32, y: u32) -> Option<f32> {
        let base = self.base(x, y)?;
        Some(self.texels[base + 3])
    }

    /// Probes across.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Probes down.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Every value, four per probe, row-major from `(0, 0)`.
    ///
    /// The raw form, for a caller comparing two fields byte for byte — which
    /// M8.5a measured to be a comparison that holds across every adapter,
    /// backend, platform and profile.
    #[must_use]
    pub fn texels(&self) -> &[f32] {
        &self.texels
    }

    fn base(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y as usize * self.width as usize + x as usize) * FIELD_CHANNELS)
    }

    /// Wraps what the kernel wrote.
    pub(crate) fn from_texels(width: u32, height: u32, texels: Vec<f32>) -> Self {
        Self {
            width,
            height,
            texels,
        }
    }
}

/// The compiled integration kernel, with the bindings it needs.
#[derive(Debug)]
pub(crate) struct CascadeKernel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl CascadeKernel {
    /// Compiles the kernel against its six bindings.
    ///
    /// Spelled out rather than derived from the module, for the reason
    /// `FieldKernel::new` gives: a derived layout is a layout nothing in this
    /// crate states, so a kernel that quietly stopped binding the emission map
    /// would still compile.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let read_texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                // Not filterable, for `FieldKernel`'s measured reason: nothing
                // samples, `textureLoad` fetches a texel and interpolates
                // nothing, and `gpu::create_device` requests no features.
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let storage_buffer = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("narvo cascade bindings"),
            entries: &[
                read_texture(0),
                read_texture(1),
                storage_buffer(2),
                storage_buffer(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: crate::field::FIELD_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
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

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("narvo cascade"),
            source: wgpu::ShaderSource::Wgsl(CASCADE_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("narvo cascade layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("narvo cascade"),
            module: &module,
            layout: Some(&pipeline_layout),
            entry_point: Some(CASCADE_ENTRY),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Self {
            device: device.clone(),
            queue: queue.clone(),
            pipeline,
            layout,
        }
    }

    /// Integrates the hits M8.4's march left on the GPU, one probe per
    /// invocation, and reads the radiance back.
    ///
    /// # Errors
    ///
    /// [`RenderError::InvalidSize`] if the probe grid cannot be a field, and
    /// [`RenderError::Readback`] if the copy back to the CPU fails.
    pub(crate) fn run(
        &self,
        field: &Field,
        emission: &Field,
        rays: &wgpu::Buffer,
        hits: &wgpu::Buffer,
        stage: &CascadeStage,
    ) -> Result<RadianceField, RenderError> {
        let layout = stage.layout();
        let out = Field::new(
            &self.device,
            &self.queue,
            layout.probes[0],
            layout.probes[1],
            "narvo cascade radiance",
        )?;

        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("narvo cascade params"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&uniform, 0, &stage.params(field.width(), field.height()));

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("narvo cascade"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(field.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(emission.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: rays.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: hits.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(out.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo cascade"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("narvo cascade"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(stage.probe_count().div_ceil(CASCADE_WORKGROUP), 1, 1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        Ok(RadianceField::from_texels(
            layout.probes[0],
            layout.probes[1],
            out.read_back()?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CASCADE_ENTRY, CASCADE_WGSL, CASCADE_WORKGROUP, CascadeStage, Emission, StageLayout,
    };
    use crate::RenderError;

    /// A layout that passes every check, for a test to spoil one field of.
    fn sound() -> StageLayout {
        StageLayout {
            origin: [2.0, 2.0],
            spacing: 4.0,
            probes: [8, 8],
            near: 0.0,
            far: 4.0,
            directions: 32,
            far_radiance: [0.0, 0.0, 0.0],
        }
    }

    // -- the source guards --------------------------------------------------

    /// **ADR-0049's guard, and it has to be a source read.**
    ///
    /// A merge may not be written order-dependently. An output comparison cannot
    /// report that it was: a sum folded into a shared accumulator produces a
    /// *plausible* number, and M8.2 measured such a sum returning four to eight
    /// distinct answers from eight identical dispatches while looking like an
    /// ordinary picture every time. So the only check there is for this is that
    /// the kernel does not contain the machinery.
    #[test]
    fn the_integration_is_not_written_order_dependently() {
        let body = uncommented(CASCADE_WGSL);
        for forbidden in [
            "atomic",
            "workgroupBarrier",
            "storageBarrier",
            "workgroupUniformLoad",
            "var<workgroup>",
        ] {
            assert!(
                !body.contains(forbidden),
                "the cascade kernel contains `{forbidden}`, which makes its sum \
                 depend on the order invocations run in — ADR-0049 measured that \
                 form as irreproducible in 32 of 32 cells"
            );
        }
        assert_eq!(
            body.matches("textureStore(").count(),
            1,
            "the cascade kernel stores more than once per invocation, so a probe's \
             result is being built up in the output rather than in a local"
        );
    }

    /// **The contraction guard, and it is a literal against a literal.**
    ///
    /// M8.5a measured the whole of §1 through this property: with no `f32`
    /// multiply the radiance field is one field on all eight adapter/backend
    /// pairs, and with one inexact product feeding an add it is two — the AMD
    /// paths fuse and the software rasterisers do not. A contracted expression
    /// produces a plausible number, so no output comparison can report it.
    ///
    /// The check is the set of lines that contain a `*`. Every one of them must
    /// be integer arithmetic, and the list is written out so that a session
    /// adding a float multiply has to change this line and then has to say which
    /// eight adapters it re-measured.
    #[test]
    fn the_kernel_holds_no_float_multiplication() {
        let starred: Vec<String> = uncommented(CASCADE_WGSL)
            .lines()
            .map(str::trim)
            .filter(|line| line.contains('*'))
            .map(str::to_owned)
            .collect();
        assert_eq!(
            starred,
            vec![
                "let base = probe * params.directions;".to_owned(),
                "let advance_x = (ray.dir_x * hit.distance) / FIXED;".to_owned(),
                "let advance_y = (ray.dir_y * hit.distance) / FIXED;".to_owned(),
            ],
            "the multiplications in the cascade kernel are not the three integer \
             ones M8.5a measured. A float multiply feeding an add is the one way \
             two backends were measured to disagree on a radiance field"
        );
        assert!(
            !uncommented(CASCADE_WGSL).contains("fma("),
            "the cascade kernel asks for a fused multiply-add, which WARP and \
             lavapipe were measured computing *unfused* — so it is not even a way \
             to force one, only a way to disagree"
        );
        for normalisation in [
            "let mean_r = sum_r / scale;",
            "let mean_g = sum_g / scale;",
            "let mean_b = sum_b / scale;",
            "let mean_v = escaped / scale;",
        ] {
            assert!(
                uncommented(CASCADE_WGSL).contains(normalisation),
                "the cascade kernel no longer normalises by dividing: `{normalisation}` \
                 is gone, and a multiply by a reciprocal would put a float multiply back"
            );
        }
    }

    /// The dispatch and the kernel agree on the workgroup.
    #[test]
    fn the_dispatch_and_the_kernel_agree_on_the_workgroup() {
        assert!(
            CASCADE_WGSL.contains(&format!("@workgroup_size({CASCADE_WORKGROUP})")),
            "the kernel does not declare `@workgroup_size({CASCADE_WORKGROUP})`, \
             which is what `CascadeKernel::run` divides the probe count by"
        );
        assert!(
            CASCADE_WGSL.contains(&format!("fn {CASCADE_ENTRY}(")),
            "the kernel has no `{CASCADE_ENTRY}` entry point"
        );
    }

    fn uncommented(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -- the parameters -----------------------------------------------------

    /// **§3's penumbra inequality, as a checked assurance rather than a comment.**
    ///
    /// The bound is `directions >= ceil(2 * pi * far)`, from the arc two adjacent
    /// directions span at the far end against one texel. The pairs below are the
    /// derivation evaluated: `2 * pi * 4 = 25.13`, so 32 passes and 16 does not;
    /// `2 * pi * 8 = 50.27`, so 64 passes and 32 does not.
    #[test]
    fn a_stage_refuses_an_angular_resolution_its_interval_outruns() {
        for (far, directions, required) in [(4.0_f32, 16_u32, 26_u32), (8.0, 32, 51)] {
            let layout = StageLayout {
                far,
                directions,
                ..sound()
            };
            match CascadeStage::new(layout) {
                Err(RenderError::PenumbraUnderresolved {
                    directions: reported,
                    far: reported_far,
                    required: reported_required,
                }) => {
                    assert_eq!(reported, directions);
                    assert!((reported_far - far).abs() < f32::EPSILON);
                    assert_eq!(
                        reported_required, required,
                        "the inequality named a different bound than ceil(2*pi*{far})"
                    );
                }
                other => panic!("a stage that stripes was accepted: {other:?}"),
            }
        }
        for (far, directions) in [(4.0_f32, 32_u32), (8.0, 64), (0.0, 1)] {
            assert!(
                CascadeStage::new(StageLayout {
                    far,
                    directions,
                    ..sound()
                })
                .is_ok(),
                "a stage satisfying the inequality was refused at far={far}, \
                 directions={directions}"
            );
        }
    }

    /// The required count is `ceil(2 * pi * far)`, computed rather than tabled.
    #[test]
    fn the_required_direction_count_is_the_arc_over_one_texel() {
        for far in [0.0_f32, 0.1, 1.0, 4.0, 8.0, 100.0, 1000.0] {
            let expected = (std::f64::consts::TAU * f64::from(far)).ceil().max(1.0);
            assert_eq!(
                f64::from(CascadeStage::directions_required(far)),
                expected,
                "the bound for far={far} is not the arc length"
            );
        }
    }

    /// A direction count that is not a power of two is refused, and the reason
    /// is the exact normalisation rather than tidiness.
    #[test]
    fn a_direction_count_that_is_not_a_power_of_two_is_refused() {
        for directions in [0_u32, 3, 26, 100] {
            match CascadeStage::new(StageLayout {
                far: 0.0,
                directions,
                ..sound()
            }) {
                Err(RenderError::DirectionsNotPowerOfTwo { directions: got }) => {
                    assert_eq!(got, directions);
                }
                other => panic!("{directions} directions were accepted: {other:?}"),
            }
        }
    }

    /// The scalar faults are each named, and each names itself.
    #[test]
    fn a_stage_refuses_a_parameter_it_cannot_use() {
        let cases = [
            (
                "a probe spacing",
                StageLayout {
                    spacing: 0.0,
                    ..sound()
                },
            ),
            (
                "a probe spacing",
                StageLayout {
                    spacing: -1.0,
                    ..sound()
                },
            ),
            (
                "a stage's near end",
                StageLayout {
                    near: -0.5,
                    ..sound()
                },
            ),
            (
                "a stage's far end",
                StageLayout {
                    near: 3.0,
                    far: 2.0,
                    ..sound()
                },
            ),
            (
                "a probe grid side",
                StageLayout {
                    probes: [0, 8],
                    ..sound()
                },
            ),
            (
                "a far-radiance channel",
                StageLayout {
                    far_radiance: [0.0, -1.0, 0.0],
                    ..sound()
                },
            ),
            (
                "a stage's far end",
                StageLayout {
                    far: f32::NAN,
                    ..sound()
                },
            ),
        ];
        for (name, layout) in cases {
            match CascadeStage::new(layout) {
                Err(RenderError::StageParameter { name: got, .. }) => {
                    assert_eq!(got, name, "the wrong parameter was blamed for {layout:?}")
                }
                other => panic!("{layout:?} was accepted: {other:?}"),
            }
        }
    }

    /// A stage asking for more rays than one dispatch can hold is refused here
    /// rather than by a wgpu validation failure later.
    #[test]
    fn a_stage_larger_than_one_dispatch_is_refused() {
        // 65535 * 16 probes at 4 directions is 4 194 240 rays, which is
        // 65535 * 64 exactly - the limit, and it passes.
        let at_limit = StageLayout {
            probes: [65_535, 16],
            far: 0.0,
            directions: 4,
            ..sound()
        };
        let stage = CascadeStage::new(at_limit).expect("a stage exactly at the limit");
        assert_eq!(stage.ray_count(), CascadeStage::MAX_RAYS);
        let over = StageLayout {
            probes: [65_535, 16],
            far: 0.0,
            directions: 8,
            ..sound()
        };
        match CascadeStage::new(over) {
            Err(RenderError::StageTooLarge { rays, limit }) => {
                assert_eq!(rays, 8_388_480);
                assert_eq!(limit, CascadeStage::MAX_RAYS);
            }
            other => panic!("an over-large stage was accepted: {other:?}"),
        }
    }

    /// A probe closer to an edge than its near end is refused, and a near end of
    /// zero restricts nothing.
    #[test]
    fn a_probe_must_stand_its_near_end_away_from_every_edge() {
        let inside = CascadeStage::new(sound()).expect("a sound layout");
        assert!(inside.check_fits(64, 64).is_ok());
        assert!(
            inside.check_fits(29, 64).is_err(),
            "a grid running to x = 30 fitted inside a field 29 wide"
        );

        let with_near = CascadeStage::new(StageLayout {
            near: 4.0,
            far: 8.0,
            directions: 64,
            ..sound()
        })
        .expect("a sound layout");
        match with_near.check_fits(64, 64) {
            Err(RenderError::ProbeOutsideField { near, .. }) => {
                assert!((near - 4.0).abs() < f32::EPSILON);
            }
            other => panic!("a probe two texels from the edge kept a near end of four: {other:?}"),
        }
        assert!(
            with_near.check_fits(64, 64).is_err() && inside.check_fits(64, 64).is_ok(),
            "the near end is what made the difference, and it did not"
        );
    }

    /// The ray list is probe-major and its length is the stage's own count.
    #[test]
    fn the_ray_list_is_probe_major_and_as_long_as_the_stage_says() {
        let stage = CascadeStage::new(sound()).expect("a sound layout");
        let rays = stage.rays(64, 64).expect("every endpoint is inside");
        assert_eq!(rays.len() as u32, stage.ray_count());
        assert_eq!(stage.ray_count(), 8 * 8 * 32);
        // Probe (1, 0) is four texels right of probe (0, 0), and its rays start
        // at index `directions`.
        assert_eq!(stage.probe_position(0, 0), Some([2.0, 2.0]));
        assert_eq!(stage.probe_position(1, 0), Some([6.0, 2.0]));
        assert_eq!(stage.probe_position(8, 0), None);
    }

    /// **J1's shape, as an assertion rather than as an injection: no ray is
    /// longer than the interval it was given.**
    ///
    /// A ray's length is `far - near` where the field allows it and less where
    /// the boundary cuts it, and never more. This is the cheapest check there is
    /// that a probe does not march past its own interval, and it needs no GPU.
    #[test]
    fn no_ray_reaches_beyond_the_interval_it_was_given() {
        for (near, far, directions) in [(0.0_f32, 4.0_f32, 32_u32), (2.0, 8.0, 64), (0.0, 0.0, 1)] {
            let stage = CascadeStage::new(StageLayout {
                origin: [16.0, 16.0],
                spacing: 4.0,
                probes: [4, 4],
                near,
                far,
                directions,
                far_radiance: [0.0, 0.0, 0.0],
            })
            .expect("a sound layout");
            let rays = stage.rays(64, 64).expect("every endpoint is inside");
            let span = far - near;
            for ray in &rays {
                assert!(
                    ray.length() <= span + 1.0 / 256.0,
                    "a ray of {} texels was built for an interval of {span}",
                    ray.length()
                );
            }
        }
    }

    // -- the emission map ---------------------------------------------------

    /// The emission map refuses what it cannot mean.
    #[test]
    fn an_emission_map_refuses_a_point_outside_it_and_a_radiance_below_zero() {
        let mut emission = Emission::new(8, 8).expect("a map");
        assert!(emission.set(7, 7, [1.0, 2.0, 3.0]).is_ok());
        assert_eq!(emission.get(7, 7), Some([1.0, 2.0, 3.0]));
        assert_eq!(emission.get(8, 7), None);

        match emission.set(8, 0, [1.0, 1.0, 1.0]) {
            Err(RenderError::EmissionOutsideField {
                x,
                y,
                width,
                height,
            }) => {
                assert_eq!((x, y, width, height), (8, 0, 8, 8));
            }
            other => panic!("a point outside the map was accepted: {other:?}"),
        }
        match emission.set(0, 0, [0.0, -0.1, 0.0]) {
            Err(RenderError::StageParameter { name, .. }) => {
                assert_eq!(name, "an emission channel");
            }
            other => panic!("a negative radiance was accepted: {other:?}"),
        }
        assert!(emission.set(0, 0, [f32::NAN, 0.0, 0.0]).is_err());
        assert!(Emission::new(0, 8).is_err());
    }
}
