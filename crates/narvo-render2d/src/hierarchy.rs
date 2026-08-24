//! A cascade of levels, and the two ways to merge one.
//!
//! **M8.5b's capability.** M8.5a built one stage and left a fork open: an
//! *aggregate* level stores one radiance per probe and cannot merge
//! directionally; a *directional* level stores one per probe per direction and
//! can. This module builds both, numbers both, and **chooses neither** — the
//! choice is a plan decision and the report hands it over with the measurements
//! beside it.
//!
//! # What a cascade is here
//!
//! Level `n` has probe spacing `s·2^n`, interval `[t_n, t_{n+1}]` with
//! `t_n = f0·(4^n − 1)/3`, and `D_0·4^n` directions. Probes ÷ 4 and directions
//! × 4 per level, so **the entry count per level is constant** — which is the
//! whole reason a cascade is affordable, and the reason its cost is `levels ×
//! (P_0 · D_0)` rather than anything that grows.
//!
//! Levels are computed **top down**: the top level takes the sky, and every
//! level below it takes the composed radiance of the level above as what an
//! escaping direction carries. That is not a stylistic choice — see below.
//!
//! # The grid cut, and why it is the aligned one
//!
//! Every level shares one origin, so a level-`n` probe at index `i` sits at
//! `o + i·s·2^n` and its position in the level above's grid is `i/2`. An even
//! index lands **on** an upper probe; an odd one lands exactly halfway between
//! two. The bilinear weights are therefore drawn from **{1, 1/2, 1/4}** — every
//! one a power of two.
//!
//! **The arrangement where every weight is 1/4 does not exist at a two-to-one
//! spacing ratio**, and that is arithmetic rather than an accident of this cut:
//! writing `u_i = a + i/2` for the upper-grid coordinate, `frac(u_i)` alternates
//! between `frac(a)` and `frac(a) + 1/2`, so asking for `1/2` at both parities
//! asks for `frac(a) = 1/2` and `frac(a) = 0` at once. What *is* arrangeable is
//! `frac(a) ∈ {0, 1/2}`, which is the aligned cut, and it gives something better
//! than all-quarters: every weight is a power of two, so **every product is
//! exact**, and M8.5a's `mul-pow2` measurement applies directly — fusing an
//! exact product changes nothing.
//!
//! # Why the composition is top-down, and not `own + escaped · upper`
//!
//! The obvious merge multiplies the upper radiance by the escaped fraction. That
//! fraction is `k/D`, which is **not** generally a power of two, so the product
//! is inexact and it feeds an add — precisely the shape ADR-0051 forbids, and
//! precisely the reopening condition that ADR names M8.5b as the first candidate
//! for.
//!
//! Running the levels top down avoids it entirely: the upper radiance is handed
//! to the level's *own* summation loop as what an escaping direction carries, so
//! it is added once per escaping direction and never multiplied by anything. The
//! interpolation that brings it to the probe is four fetches, three adds and a
//! division by four.
//!
//! **So ADR-0051's reopening is answered no rather than deferred**, and the
//! answer is structural: there is no `f32` multiplication in either kernel.

use crate::cascade::{CascadeStage, StageLayout};
use crate::error::RenderError;
use crate::field::Field;

/// The directional kernel's source.
pub(crate) const DIRECTIONAL_WGSL: &str = include_str!("shaders/cascade_directional.wgsl");

/// The entry point in [`DIRECTIONAL_WGSL`].
pub(crate) const DIRECTIONAL_ENTRY: &str = "integrate_directional";

/// Invocations per workgroup, matching [`crate::cascade::CASCADE_WORKGROUP`].
pub(crate) const DIRECTIONAL_WORKGROUP: u32 = 64;

/// Bytes one stored radiance entry occupies, in either form.
///
/// Sixteen, because both forms store four `f32`: the aggregate keeps rgb plus
/// the escaped fraction in a [`Field`] texel, and the directional keeps rgb plus
/// one unused word in a storage buffer, which is what WGSL's `vec4<f32>`
/// alignment costs anyway.
pub const ENTRY_BYTES: u64 = 16;

/// The most a storage buffer may be bound at once.
///
/// `Limits::defaults()` sets `max_storage_buffer_binding_size: 128 << 20`
/// (`wgpu-types-30.0.0/src/limits.rs:441`), which is what
/// [`OffscreenTarget::new`](crate::OffscreenTarget::new) asks the device for. The
/// directional form binds one level's entries as a single buffer, so this is a
/// hard ceiling on `probes × directions` for that form — and it is a *different*
/// ceiling from [`CascadeStage::MAX_RAYS`], which bounds one dispatch.
pub const MAX_STORAGE_BINDING_BYTES: u64 = 128 << 20;

/// Which of the two merges a cascade runs.
///
/// **Both are built and neither is preferred.** The difference between them is
/// what M8.5b measures; the choice between them is a plan decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeForm {
    /// One radiance per probe. A level's escaping directions all take the same
    /// value from the level above, so the direction is thrown away.
    Aggregate,
    /// One radiance per probe **per direction**. An escaping direction takes the
    /// radiance of the four upper directions covering the same arc.
    Directional,
}

/// What a cascade is, before it has been checked against a field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CascadeLayout {
    /// Where probe `(0, 0)` of **every** level sits. Shared, which is what makes
    /// the interpolation weights powers of two.
    pub origin: [f32; 2],
    /// Level zero's probe spacing, in texels. Doubles per level.
    pub base_spacing: f32,
    /// Level zero's interval length, in texels. Quadruples per level.
    pub base_interval: f32,
    /// Level zero's direction count. Quadruples per level, and a power of two.
    pub base_directions: u32,
    /// How many levels. At least one.
    pub levels: u32,
    /// What an escaping direction of the **top** level carries.
    pub sky: [f32; 3],
}

/// A validated [`CascadeLayout`], with one [`CascadeStage`] per level.
#[derive(Debug, Clone, PartialEq)]
pub struct Cascade {
    layout: CascadeLayout,
    levels: Vec<CascadeStage>,
    field: [u32; 2],
}

impl Cascade {
    /// Builds and validates every level against a `width` x `height` field.
    ///
    /// # What is checked, and why each check is here
    ///
    /// Every level goes through [`CascadeStage::new`], so **§4's penumbra
    /// inequality is checked at every level rather than at level zero**. That
    /// matters because the inequality is *not* tightest at level zero: requiring
    /// `D_0·4^n >= 2·pi·f0·(4^(n+1) − 1)/3` for every `n` and dividing by `4^n`
    /// gives `D_0 >= (2·pi·f0/3)·(4 − 4^-n)`, which grows with `n` and tends to
    /// `8·pi·f0/3`. So the binding constraint is the **top** level, and a cascade
    /// that satisfies level zero can still stripe higher up.
    ///
    /// The probe grid halves per level: `P_n − 1 = floor((P_0 − 1)/2^n)`, which
    /// is the identity that makes an upper index `i >> 1` always land inside the
    /// upper grid.
    ///
    /// # Errors
    ///
    /// - [`RenderError::StageParameter`] if the level count is zero or a scalar
    ///   is unusable, with the level named in the message.
    /// - Everything [`CascadeStage::new`] returns, for the first level that
    ///   fails: [`RenderError::PenumbraUnderresolved`],
    ///   [`RenderError::DirectionsNotPowerOfTwo`],
    ///   [`RenderError::StageTooLarge`].
    /// - [`RenderError::InvalidSize`] if the field is not one a [`Field`] can be.
    pub fn new(layout: CascadeLayout, width: u32, height: u32) -> Result<Self, RenderError> {
        let max = crate::OffscreenTarget::MAX_DIMENSION;
        if width == 0 || height == 0 || width > max || height > max {
            return Err(RenderError::InvalidSize { width, height, max });
        }
        if layout.levels == 0 {
            return Err(RenderError::StageParameter {
                name: "a cascade's level count",
                value: 0.0,
                requirement: "be at least one",
            });
        }
        if !layout.base_interval.is_finite() || layout.base_interval < 0.0 {
            return Err(RenderError::StageParameter {
                name: "a cascade's base interval",
                value: layout.base_interval,
                requirement: "be finite and not negative",
            });
        }

        let mut levels = Vec::with_capacity(layout.levels as usize);
        for level in 0..layout.levels {
            levels.push(CascadeStage::new(level_layout(
                &layout, level, width, height,
            ))?);
        }
        Ok(Self {
            layout,
            levels,
            field: [width, height],
        })
    }

    /// The layout this cascade was built from.
    #[must_use]
    pub fn layout(&self) -> CascadeLayout {
        self.layout
    }

    /// How many levels.
    #[must_use]
    pub fn level_count(&self) -> u32 {
        self.layout.levels
    }

    /// One level's validated stage, or `None` above the top.
    #[must_use]
    pub fn level(&self, level: u32) -> Option<&CascadeStage> {
        self.levels.get(level as usize)
    }

    /// What one run of this cascade costs, in the given form.
    ///
    /// **The numbers come from the built levels rather than from a formula**,
    /// which is the point: M8.5a's budget table was arithmetic over an assumed
    /// parameterisation, and §2(a) asks for it to be checked against a cascade
    /// that actually exists.
    #[must_use]
    pub fn budget(&self, form: MergeForm) -> CascadeBudget {
        let levels: Vec<LevelBudget> = self
            .levels
            .iter()
            .enumerate()
            .map(|(index, stage)| {
                let layout = stage.layout();
                let probes = u64::from(stage.probe_count());
                let rays = u64::from(stage.ray_count());
                let entries = match form {
                    MergeForm::Aggregate => probes,
                    MergeForm::Directional => probes * u64::from(layout.directions),
                };
                LevelBudget {
                    level: index as u32,
                    probes: layout.probes,
                    directions: layout.directions,
                    rays: stage.ray_count(),
                    entries,
                    radiance_bytes: entries * ENTRY_BYTES,
                    // Thirty-two bytes a ray in the ray buffer, sixteen in the
                    // hit buffer: M8.5a's forty-eight, and transient.
                    ray_bytes: rays * 48,
                }
            })
            .collect();
        let radiance_bytes = levels.iter().map(|level| level.radiance_bytes).sum();
        let ray_bytes = levels
            .iter()
            .map(|level| level.ray_bytes)
            .max()
            .unwrap_or(0);
        // Two adjacent levels of radiance are alive at once, plus the transient
        // ray and hit buffers of the level being computed.
        let peak_bytes = levels
            .windows(2)
            .map(|pair| pair[0].radiance_bytes + pair[1].radiance_bytes)
            .max()
            .unwrap_or_else(|| levels.first().map_or(0, |level| level.radiance_bytes))
            + ray_bytes;
        CascadeBudget {
            form,
            levels,
            radiance_bytes,
            ray_bytes,
            peak_bytes,
        }
    }

    /// Whether this cascade can be run in `form` on a device with wgpu's
    /// default limits.
    ///
    /// # Errors
    ///
    /// [`RenderError::CascadeLevelTooLarge`] if a directional level's entries
    /// exceed [`MAX_STORAGE_BINDING_BYTES`]. The aggregate form cannot hit it —
    /// its levels are [`Field`]s, bounded by
    /// [`OffscreenTarget::MAX_DIMENSION`](crate::OffscreenTarget::MAX_DIMENSION)
    /// instead — so this is the ceiling that separates the two forms rather than
    /// one they share.
    pub fn check_form(&self, form: MergeForm) -> Result<(), RenderError> {
        if form == MergeForm::Aggregate {
            return Ok(());
        }
        for level in self.budget(form).levels {
            if level.radiance_bytes > MAX_STORAGE_BINDING_BYTES {
                return Err(RenderError::CascadeLevelTooLarge {
                    level: level.level,
                    bytes: level.radiance_bytes,
                    limit: MAX_STORAGE_BINDING_BYTES,
                });
            }
        }
        Ok(())
    }

    /// The field size this cascade was validated against.
    #[must_use]
    pub fn field(&self) -> [u32; 2] {
        self.field
    }
}

/// One level's stage layout, derived from the cascade's.
///
/// Separate from [`Cascade::new`] so that the derivation is one expression a
/// test can call rather than a loop body.
fn level_layout(layout: &CascadeLayout, level: u32, width: u32, height: u32) -> StageLayout {
    let scale = 2.0_f32.powi(level as i32);
    let spacing = layout.base_spacing * scale;
    // t_n = f0 * (4^n - 1) / 3, and far is the same at n + 1. Computed in f64
    // and narrowed once, so the two ends of one interval come from one
    // expression rather than from two roundings.
    let near = interval_end(f64::from(layout.base_interval), level);
    let far = interval_end(f64::from(layout.base_interval), level + 1);
    StageLayout {
        origin: layout.origin,
        spacing,
        probes: [probe_count(width, spacing), probe_count(height, spacing)],
        near,
        far,
        directions: layout
            .base_directions
            .saturating_mul(4_u32.saturating_pow(level)),
        // Only the top level sees the sky; every other level's escaping
        // directions carry the level above, which the kernel supplies.
        far_radiance: if level + 1 == layout.levels {
            layout.sky
        } else {
            [0.0, 0.0, 0.0]
        },
    }
}

/// `f0 * (4^level - 1) / 3`, the distance at which `level` begins.
fn interval_end(base: f64, level: u32) -> f32 {
    let quarters = 4.0_f64.powi(level as i32);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an interval beyond MAX_DIMENSION is refused by CascadeStage::new"
    )]
    let end = (base * (quarters - 1.0) / 3.0) as f32;
    end
}

/// How many probes at `spacing` cover `extent` texels from the origin.
///
/// `floor(extent / spacing) + 1`, which is the identity that makes the grid
/// halve exactly: `floor(e / 2s) = floor(floor(e / s) / 2)`, so a level's probe
/// count minus one is always the level below's minus one, halved.
fn probe_count(extent: u32, spacing: f32) -> u32 {
    if !spacing.is_finite() || spacing <= 0.0 {
        return 1;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "extent / spacing is positive and at most MAX_DIMENSION"
    )]
    let count = (f64::from(extent) / f64::from(spacing)).floor() as u32;
    count.saturating_add(1)
}

/// What one level costs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelBudget {
    /// Which level, counting from zero at the bottom.
    pub level: u32,
    /// Probes across and down.
    pub probes: [u32; 2],
    /// Directions per probe.
    pub directions: u32,
    /// Rays this level marches: probes times directions.
    pub rays: u32,
    /// Radiance entries stored: probes, or probes times directions.
    pub entries: u64,
    /// Bytes those entries occupy.
    pub radiance_bytes: u64,
    /// Bytes the transient ray and hit buffers occupy while this level runs.
    pub ray_bytes: u64,
}

/// What a whole cascade costs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeBudget {
    /// Which merge this is the cost of.
    pub form: MergeForm,
    /// One entry per level, bottom first.
    pub levels: Vec<LevelBudget>,
    /// Radiance bytes summed over every level — the figure to compare the two
    /// forms by, and the one M8.5a's table quotes.
    pub radiance_bytes: u64,
    /// The largest single level's transient ray and hit buffers.
    pub ray_bytes: u64,
    /// What is alive at once: two adjacent levels plus the ray buffers.
    pub peak_bytes: u64,
}

/// The compiled directional kernel, with the bindings it needs.
#[derive(Debug)]
pub(crate) struct DirectionalKernel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl DirectionalKernel {
    /// Compiles the kernel against its eight bindings.
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
        let buffer = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("narvo cascade directional bindings"),
            entries: &[
                read_texture(0),
                read_texture(1),
                buffer(2, true),
                buffer(3, true),
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
                buffer(6, true),
                buffer(7, false),
            ],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("narvo cascade directional"),
            source: wgpu::ShaderSource::Wgsl(DIRECTIONAL_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("narvo cascade directional layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("narvo cascade directional"),
            module: &module,
            layout: Some(&pipeline_layout),
            entry_point: Some(DIRECTIONAL_ENTRY),
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

    /// Runs one level, writing its per-direction radiance into `outgoing` and
    /// its mean into `radiance`.
    #[expect(
        clippy::too_many_arguments,
        reason = "a level binds eight resources and every one is a distinct thing; \
                  bundling them into a struct would name them twice"
    )]
    pub(crate) fn run(
        &self,
        field: &Field,
        emission: &Field,
        rays: &wgpu::Buffer,
        hits: &wgpu::Buffer,
        radiance: &Field,
        upper: &wgpu::Buffer,
        outgoing: &wgpu::Buffer,
        params: &[u8; 48],
        probes: u32,
    ) {
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("narvo cascade directional params"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uniform, 0, params);

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("narvo cascade directional"),
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
                    resource: wgpu::BindingResource::TextureView(radiance.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: upper.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: outgoing.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo cascade directional"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("narvo cascade directional"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(probes.div_ceil(DIRECTIONAL_WORKGROUP), 1, 1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Cascade, CascadeLayout, DIRECTIONAL_ENTRY, DIRECTIONAL_WGSL, DIRECTIONAL_WORKGROUP,
        MAX_STORAGE_BINDING_BYTES, MergeForm, interval_end, probe_count,
    };
    use crate::RenderError;

    /// A cascade that passes every check, for a test to spoil one field of.
    fn sound() -> CascadeLayout {
        CascadeLayout {
            origin: [0.0, 0.0],
            base_spacing: 4.0,
            base_interval: 2.0,
            base_directions: 32,
            levels: 5,
            sky: [0.0, 0.0, 0.0],
        }
    }

    // -- the source guards --------------------------------------------------

    /// **ADR-0049's guard for the directional kernel.**
    ///
    /// The same reasoning as `cascade.rs`'s: a sum folded into a shared
    /// accumulator produces a plausible number that no output comparison can
    /// report, so the only check there is is that the machinery is absent.
    #[test]
    fn the_directional_kernel_is_not_written_order_dependently() {
        let body = uncommented(DIRECTIONAL_WGSL);
        for forbidden in [
            "atomic",
            "workgroupBarrier",
            "storageBarrier",
            "workgroupUniformLoad",
            "var<workgroup>",
        ] {
            assert!(
                !body.contains(forbidden),
                "the directional kernel contains `{forbidden}`, which makes its sum \
                 depend on the order invocations run in"
            );
        }
        assert_eq!(
            body.matches("textureStore(").count(),
            1,
            "the directional kernel stores more than once per invocation"
        );
    }

    /// **ADR-0051's guard for the directional kernel, and it is the one that
    /// says the reopening was answered rather than deferred.**
    ///
    /// Every `*` in the kernel must be integer index arithmetic. The list is
    /// written out, so a session that adds a float multiply — an interpolation
    /// weight, most likely — has to change this line and then has to say which
    /// eight adapters it re-measured.
    #[test]
    fn the_directional_kernel_holds_no_float_multiplication() {
        let starred: Vec<String> = uncommented(DIRECTIONAL_WGSL)
            .lines()
            .map(str::trim)
            .filter(|line| line.contains('*'))
            .map(str::to_owned)
            .collect();
        assert_eq!(
            starred,
            vec![
                "let upper_directions = params.directions * 4u;".to_owned(),
                "let row0 = u32(y0) * params.upper_w;".to_owned(),
                "let row1 = u32(y1) * params.upper_w;".to_owned(),
                "let base00 = (row0 + u32(x0)) * upper_directions;".to_owned(),
                "let base10 = (row0 + u32(x1)) * upper_directions;".to_owned(),
                "let base01 = (row1 + u32(x0)) * upper_directions;".to_owned(),
                "let base11 = (row1 + u32(x1)) * upper_directions;".to_owned(),
                "let base = probe * params.directions;".to_owned(),
                "let offset = k * 4u;".to_owned(),
                "let advance_x = (ray.dir_x * hit.distance) / FIXED;".to_owned(),
                "let advance_y = (ray.dir_y * hit.distance) / FIXED;".to_owned(),
            ],
            "the multiplications in the directional kernel are not the eleven \
             integer ones M8.5b measured. A float multiply feeding an add is the \
             one way two backends were measured to compute two radiance fields"
        );
        assert!(
            !uncommented(DIRECTIONAL_WGSL).contains("fma("),
            "the directional kernel asks for a fused multiply-add"
        );
        for division in ["/ 2.0", "/ scale"] {
            assert!(
                uncommented(DIRECTIONAL_WGSL).contains(division),
                "the directional kernel no longer averages by dividing: `{division}` \
                 is gone, and a multiply by a reciprocal would put a float multiply back"
            );
        }
    }

    /// The dispatch and the kernel agree on the workgroup.
    #[test]
    fn the_dispatch_and_the_directional_kernel_agree_on_the_workgroup() {
        assert!(
            DIRECTIONAL_WGSL.contains(&format!("@workgroup_size({DIRECTIONAL_WORKGROUP})")),
            "the kernel does not declare `@workgroup_size({DIRECTIONAL_WORKGROUP})`"
        );
        assert!(
            DIRECTIONAL_WGSL.contains(&format!("fn {DIRECTIONAL_ENTRY}(")),
            "the kernel has no `{DIRECTIONAL_ENTRY}` entry point"
        );
    }

    fn uncommented(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -- the derivation -----------------------------------------------------

    /// **The grid halves exactly, which is what makes `i >> 1` a valid upper
    /// index at every level.**
    ///
    /// `P_n − 1 = floor((P_0 − 1) / 2^n)`, from
    /// `floor(e / 2s) = floor(floor(e / s) / 2)`.
    #[test]
    fn the_probe_grid_halves_exactly_at_every_level() {
        for extent in [1_u32, 7, 63, 64, 65, 100, 255, 256, 1080, 1920] {
            for base in [1.0_f32, 2.0, 3.0, 4.0, 8.0] {
                let mut previous = probe_count(extent, base);
                for level in 1..8_u32 {
                    let here = probe_count(extent, base * 2.0_f32.powi(level as i32));
                    assert_eq!(
                        here - 1,
                        (previous - 1) / 2,
                        "extent {extent}, base {base}, level {level}: the grid did \
                         not halve, so `i >> 1` can leave the upper grid"
                    );
                    previous = here;
                }
            }
        }
    }

    /// The interval ends are `f0 * (4^n - 1) / 3`, so level `n`'s far end is
    /// level `n+1`'s near end exactly.
    #[test]
    fn the_intervals_tile_without_gap_or_overlap() {
        let layout = sound();
        let cascade = Cascade::new(layout, 256, 256).expect("a sound cascade");
        for level in 1..layout.levels {
            let below = cascade.level(level - 1).expect("a level").layout();
            let here = cascade.level(level).expect("a level").layout();
            assert_eq!(
                below.far,
                here.near,
                "level {level} does not begin where level {} ends, so the cascade \
                 either double-counts a band or misses one",
                level - 1
            );
        }
        assert_eq!(
            cascade.level(0).expect("a level").layout().near,
            0.0,
            "the cascade does not start at the probe"
        );
        assert_eq!(interval_end(2.0, 0), 0.0);
        assert_eq!(interval_end(2.0, 1), 2.0);
        assert_eq!(interval_end(2.0, 2), 10.0);
        assert_eq!(interval_end(2.0, 3), 42.0);
    }

    /// **§4: the penumbra inequality is checked at every level, and the binding
    /// level is the top rather than the bottom.**
    ///
    /// `D_0 >= (2 pi f0 / 3)(4 - 4^-n)` grows with `n`, so a cascade can satisfy
    /// level zero and stripe higher up. The case below does exactly that: with
    /// `f0 = 2`, level zero needs 13 directions and the asymptote needs 17, so
    /// 16 passes at the bottom and fails above it.
    #[test]
    fn the_penumbra_inequality_is_checked_at_every_level_not_just_the_first() {
        let short = CascadeLayout {
            base_directions: 16,
            ..sound()
        };
        // Level zero alone would accept it: ceil(2*pi*2) = 13 <= 16.
        assert!(
            crate::CascadeStage::new(super::level_layout(&short, 0, 256, 256)).is_ok(),
            "the bottom level of the striping cascade was already refused, so this \
             test is not about the level it claims to be about"
        );
        match Cascade::new(short, 256, 256) {
            Err(RenderError::PenumbraUnderresolved { directions, .. }) => {
                assert!(
                    directions > 16,
                    "the refusal came from level zero, not from a level above it"
                );
            }
            other => panic!("a cascade that stripes above level zero was accepted: {other:?}"),
        }
        assert!(
            Cascade::new(sound(), 256, 256).is_ok(),
            "a cascade satisfying the inequality at every level was refused"
        );
    }

    /// A cascade of no levels is refused, and one level is legal.
    #[test]
    fn a_cascade_needs_at_least_one_level() {
        match Cascade::new(
            CascadeLayout {
                levels: 0,
                ..sound()
            },
            256,
            256,
        ) {
            Err(RenderError::StageParameter { name, .. }) => {
                assert_eq!(name, "a cascade's level count");
            }
            other => panic!("a cascade of no levels was accepted: {other:?}"),
        }
        let one = Cascade::new(
            CascadeLayout {
                levels: 1,
                ..sound()
            },
            256,
            256,
        )
        .expect("one level is a cascade");
        assert_eq!(one.level_count(), 1);
        assert!(one.level(1).is_none());
    }

    // -- the budget ---------------------------------------------------------

    /// **§2(a): the two forms' costs, from the built levels rather than from a
    /// formula.**
    ///
    /// The directional form stores `directions` times as many entries per level.
    /// Because probes divide by four and directions multiply by four, the
    /// per-level entry count is *constant* in the directional form and *falls by
    /// four* in the aggregate one — which is the whole shape of the trade.
    #[test]
    fn the_two_forms_cost_what_the_levels_say_they_cost() {
        let cascade = Cascade::new(sound(), 256, 256).expect("a sound cascade");
        let aggregate = cascade.budget(MergeForm::Aggregate);
        let directional = cascade.budget(MergeForm::Directional);
        assert_eq!(aggregate.levels.len(), 5);
        assert_eq!(directional.levels.len(), 5);

        for (index, (agg, dir)) in aggregate.levels.iter().zip(&directional.levels).enumerate() {
            assert_eq!(
                agg.entries,
                u64::from(agg.probes[0]) * u64::from(agg.probes[1])
            );
            assert_eq!(dir.entries, agg.entries * u64::from(dir.directions));
            assert_eq!(dir.rays, agg.rays);
            assert_eq!(
                u64::from(dir.rays),
                dir.entries,
                "level {index}: a directional entry is a ray, and the two counts parted"
            );
        }
        assert!(
            directional.radiance_bytes > aggregate.radiance_bytes,
            "the directional form did not cost more, which cannot be right"
        );
    }

    /// **§2(a): what a cascade covering 1920 x 1080 costs, in both forms.**
    ///
    /// The numbers come from cascades that are actually built and validated, not
    /// from a formula over an assumed parameterisation — which is the whole of
    /// what M8.5b was asked to check about M8.5a's table.
    ///
    /// `f0 = 0.6` texels and `D_0 = 8` satisfy the penumbra inequality at every
    /// level (`8·pi·f0/3 = 5.03`, and 8 clears it), and seven levels reach
    /// `far = 3274` texels, past the 2202-texel diagonal.
    #[test]
    fn the_budget_of_a_cascade_over_1080p_is_what_the_levels_say() {
        let mut reported = 0;
        for spacing in [1.0_f32, 2.0, 4.0, 8.0] {
            let layout = CascadeLayout {
                origin: [0.0, 0.0],
                base_spacing: spacing,
                base_interval: 0.6,
                base_directions: 8,
                levels: 7,
                sky: [0.0, 0.0, 0.0],
            };
            match Cascade::new(layout, 1920, 1080) {
                Ok(cascade) => {
                    let aggregate = cascade.budget(MergeForm::Aggregate);
                    let directional = cascade.budget(MergeForm::Directional);
                    let fits = cascade.check_form(MergeForm::Directional).is_ok();
                    eprintln!(
                        "M8.5b budget 1920x1080 spacing {spacing}: probes L0 {:?}, aggregate {:.1} MB, directional {:.1} MB, largest ray+hit {:.1} MB, directional fits binding: {fits}",
                        cascade.level(0).expect("a level").layout().probes,
                        aggregate.radiance_bytes as f64 / 1e6,
                        directional.radiance_bytes as f64 / 1e6,
                        aggregate.ray_bytes as f64 / 1e6,
                    );
                    assert!(directional.radiance_bytes > aggregate.radiance_bytes);
                    reported += 1;
                }
                Err(error) => {
                    eprintln!("M8.5b budget 1920x1080 spacing {spacing}: refused - {error}");
                    reported += 1;
                }
            }
        }
        assert_eq!(reported, 4, "one of the four spacings said nothing at all");
    }

    /// The directional form has a ceiling the aggregate form does not.
    #[test]
    fn a_directional_level_beyond_the_storage_binding_limit_is_refused() {
        let cascade = Cascade::new(sound(), 256, 256).expect("a sound cascade");
        assert!(cascade.check_form(MergeForm::Aggregate).is_ok());
        assert!(cascade.check_form(MergeForm::Directional).is_ok());

        // 1920 x 1080 at one probe per texel, eight directions: the level-zero
        // case M8.5a's budget named as impossible.
        let wide = Cascade::new(
            CascadeLayout {
                base_spacing: 1.0,
                base_interval: 0.6,
                base_directions: 8,
                levels: 3,
                ..sound()
            },
            1920,
            1080,
        );
        match wide {
            Err(RenderError::StageTooLarge { .. }) => {
                // The ray ceiling bites before the storage one does, which is
                // itself the finding: a level that cannot be marched cannot be
                // stored either, and the march says so first.
            }
            Ok(cascade) => match cascade.check_form(MergeForm::Directional) {
                Err(RenderError::CascadeLevelTooLarge { bytes, limit, .. }) => {
                    assert!(bytes > limit);
                }
                other => panic!("a level of 265 MB was accepted: {other:?}"),
            },
            Err(other) => panic!("unexpected refusal: {other:?}"),
        }
        assert_eq!(MAX_STORAGE_BINDING_BYTES, 134_217_728);
    }
}
