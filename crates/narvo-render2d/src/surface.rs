//! The surface cache: what a frame lit becomes what the next frame marches
//! against.
//!
//! **M8.6's capability, and Lumen's surface cache in two dimensions.** A cascade
//! answers "how much light reaches this probe". A surface cache takes that
//! answer, multiplies it by how much each surface *reflects*, and writes it back
//! as emission — so the next frame's march finds a wall that is itself a lamp.
//! Multiple bounces are spread over frames instead of being solved inside one,
//! which is the whole of the trade: a second bounce costs one frame of latency
//! rather than a second cascade.
//!
//! # The recurrence, and why it is written the way it is
//!
//! ```text
//! bounce_0    = whatever the cache was seeded with, zero by default
//! emission_n  = direct + bounce_n
//! R_n         = cascade(emission_n)          -- level zero's radiance
//! bounce_n+1  = albedo * R_n
//! ```
//!
//! `direct` is what an author wrote and never changes; `bounce` is the cache's
//! state and is all that carries from frame to frame. Nothing accumulates: the
//! geometric series lives in `R` rather than in a running sum, which is what
//! keeps a finished scene from drifting and what makes the fixed point below an
//! equality rather than a limit.
//!
//! **`shaders/bounce.wgsl` runs the two arithmetic steps as two dispatches**, so
//! no float multiply feeds a float add anywhere in the write-back. Its header
//! carries the argument; the short form is that ADR-0051's rule survives the
//! feedback rather than being spent on it.
//!
//! # Where colour comes from
//!
//! From [`Albedo`] having three channels, and from nothing else. A red wall is a
//! wall whose albedo is `[1, 0, 0]`; the light that leaves it is red because the
//! green and blue it received were multiplied by zero. There is no colour
//! feature, no tint and no palette — GDD-L6's coloured bounce light is a
//! *consequence* of the reflectance being a vector, and the three channels have
//! travelled through every kernel since M8.5a carrying the same number in each.
//! M8.6 is where they start carrying different ones.
//!
//! # The fixed point this is checked by
//!
//! A closed chamber with albedo one, no direct emission, and a uniform field
//! **holds that field exactly**. Every direction of every probe meets a wall
//! carrying `v`, the mean of a power-of-two count of copies of `v` is `v`, and
//! `1.0 * v` is `v` — so the step is the identity, byte for byte, and any loss is
//! an energy leak while any gain is energy from nothing. At albedo one half the
//! same chamber **halves exactly** and does so monotonically, which is the half a
//! limit alone would not show: a sequence that reaches the right limit while
//! overshooting on the way has a defect the limit cannot report.
//!
//! `tests/surface_cache.rs` carries both, and M8.6's report carries the
//! derivation and where it parts from the plan's wording.

use crate::cascade::{Emission, RadianceField};
use crate::error::RenderError;
use crate::field::{FIELD_CHANNELS, Field};
use crate::hierarchy::{Cascade, MergeForm};

/// The write-back's source.
pub(crate) const BOUNCE_WGSL: &str = include_str!("shaders/bounce.wgsl");

/// The entry point in [`BOUNCE_WGSL`] that multiplies radiance by albedo.
pub(crate) const REFLECT_ENTRY: &str = "reflect";

/// The entry point in [`BOUNCE_WGSL`] that adds the bounce to the direct light.
pub(crate) const COMBINE_ENTRY: &str = "combine";

/// Invocations per workgroup, both axes. A field is walked in two dimensions.
pub(crate) const BOUNCE_WORKGROUP: u32 = 8;

/// How much of what reaches a texel leaves it again, per channel.
///
/// Plain data, like [`Seeds`](crate::Seeds) and [`Emission`], and for the same
/// reason: building one needs no GPU, so a cache's inputs can be constructed and
/// checked on a machine with no adapter at all.
///
/// **A channel lies in `[0, 1]` and that is enforced.** Above one is not a bright
/// surface, it is a surface that returns more light than it received — and under
/// feedback that is not a slightly wrong picture but an exponential divergence,
/// because the recurrence's ratio would exceed one. Below zero is the defect
/// [`Emission::set`] refuses for the same reason. Both are refused at the point
/// a value is written rather than at the point a field explodes.
///
/// **Only texels that are occluders are ever read**, exactly as for [`Emission`]:
/// `cascade.wgsl` reads emission at the seed a direction stopped on, so albedo on
/// empty space is inert. It is stored rather than refused, because refusing it
/// would make "which texels are occluders" a fact this type had to know, and it
/// does not.
#[derive(Debug, Clone, PartialEq)]
pub struct Albedo {
    width: u32,
    height: u32,
    /// Four floats a texel, so it can be uploaded as a field without a repack.
    /// The fourth is unused and stays zero.
    texels: Vec<f32>,
}

impl Albedo {
    /// A `width` x `height` map that reflects nothing.
    ///
    /// Black, not white: a surface nobody has described absorbs. The other
    /// default would make every wall in an undescribed scene bounce light, which
    /// is the more surprising of the two silences.
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

    /// A `width` x `height` map where every texel reflects `rgb`.
    ///
    /// # Errors
    ///
    /// As [`Albedo::new`] and [`Albedo::set`].
    pub fn uniform(width: u32, height: u32, rgb: [f32; 3]) -> Result<Self, RenderError> {
        let mut map = Self::new(width, height)?;
        for y in 0..height {
            for x in 0..width {
                map.set(x, y, rgb)?;
            }
        }
        Ok(map)
    }

    /// Sets the texel at `(x, y)` to reflect `rgb`.
    ///
    /// # Errors
    ///
    /// [`RenderError::AlbedoOutsideField`] if the point is outside the map, and
    /// [`RenderError::StageParameter`] if a channel is not a finite number
    /// between zero and one — the header says why the upper bound is a refusal
    /// rather than a clamp.
    pub fn set(&mut self, x: u32, y: u32, rgb: [f32; 3]) -> Result<(), RenderError> {
        if x >= self.width || y >= self.height {
            return Err(RenderError::AlbedoOutsideField {
                x,
                y,
                width: self.width,
                height: self.height,
            });
        }
        for value in rgb {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(RenderError::StageParameter {
                    name: "an albedo channel",
                    value,
                    requirement: "be a finite number between zero and one",
                });
            }
        }
        let base = (y as usize * self.width as usize + x as usize) * FIELD_CHANNELS;
        self.texels[base] = rgb[0];
        self.texels[base + 1] = rgb[1];
        self.texels[base + 2] = rgb[2];
        Ok(())
    }

    /// What `(x, y)` reflects, or `None` outside the map.
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

    /// Texels across.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Texels down.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw four-channel buffer, for upload.
    pub(crate) fn texels(&self) -> &[f32] {
        &self.texels
    }
}

/// A world's lighting state between two frames.
///
/// Holds the flooded distance field, the author's direct emission, the albedo
/// map and — the part that is actually *state* — the field that bounced last
/// frame. [`OffscreenTarget::bounce`](crate::OffscreenTarget::bounce) advances it
/// by one frame.
///
/// **The distance field is flooded once**, at construction, and every frame
/// marches against it. That is the cost a surface cache exists to avoid paying
/// per frame, and it is also the assumption it makes: a cache is valid for as
/// long as its occluders do not move. When they do, build another one — which is
/// ADR-0022's reconstitution reasoning arriving in the lighting path, and is
/// cheaper to state than an invalidation protocol nothing yet needs.
///
/// No `wgpu` type appears in its public API, so it is an opaque handle in the
/// sense the crate header means.
#[derive(Debug)]
pub struct SurfaceCache {
    pub(crate) field: Field,
    pub(crate) direct: Field,
    pub(crate) albedo: Field,
    pub(crate) bounced: Field,
    pub(crate) emission: Field,
    pub(crate) cascade: Cascade,
    pub(crate) form: MergeForm,
    extent: [u32; 2],
    pub(crate) grid: BounceParams,
    frames: u32,
}

impl SurfaceCache {
    /// How many frames of feedback this cache has run.
    #[must_use]
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Which merge its cascade runs.
    #[must_use]
    pub fn form(&self) -> MergeForm {
        self.form
    }

    /// The cascade it advances.
    #[must_use]
    pub fn cascade(&self) -> &Cascade {
        &self.cascade
    }

    /// What bounced in the last frame, as an emission map.
    ///
    /// An [`Emission`] rather than a type of its own, because that is exactly
    /// what it is: the light every texel gives off because of what reached it.
    /// Reading it is what lets a test check the write-back per texel against a
    /// model it computed itself.
    ///
    /// # Errors
    ///
    /// [`RenderError::Readback`] if the copy to the CPU fails.
    pub fn bounced(&self) -> Result<Emission, RenderError> {
        Ok(Emission::from_texels(
            self.extent[0],
            self.extent[1],
            self.bounced.read_back()?,
        ))
    }

    /// Replaces what bounced, so the next frame starts from a field of the
    /// caller's choosing.
    ///
    /// **Its consumers are M8.7's temporal accumulation**, which has to be able
    /// to restore a cache it carried across a frame boundary, and the
    /// closed-chamber oracle, which needs a uniform starting field to check that
    /// the step is the identity. State that cannot be set is state that cannot be
    /// tested.
    ///
    /// # Errors
    ///
    /// [`RenderError::EmissionSizeMismatch`] if the map is not the field's size,
    /// and [`RenderError::FieldTexelCount`] if the upload is rejected, which
    /// cannot happen once the size matches.
    pub fn set_bounced(&mut self, bounced: &Emission) -> Result<(), RenderError> {
        if bounced.width() != self.extent[0] || bounced.height() != self.extent[1] {
            return Err(RenderError::EmissionSizeMismatch {
                seed_width: self.extent[0],
                seed_height: self.extent[1],
                emission_width: bounced.width(),
                emission_height: bounced.height(),
            });
        }
        self.bounced.write(bounced.texels())
    }
}

/// The thirty-two bytes [`BOUNCE_WGSL`]'s uniform holds.
///
/// A struct rather than a `[u8; 32]` built at the call site, for
/// [`StageLayout`](crate::StageLayout)'s reason: five of the seven numbers are
/// adjacent same-typed integers, and a swap between `origin_x` and `origin_y` or
/// between `probes_x` and `probes_y` is a defect nothing else would catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BounceParams {
    pub(crate) width: u32,
    pub(crate) height: u32,
    origin_x: i32,
    origin_y: i32,
    spacing: i32,
    probes_x: u32,
    probes_y: u32,
}

impl BounceParams {
    /// The uniform's bytes, in the order `struct Bounce` declares them.
    fn bytes(self) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        bytes[0..4].copy_from_slice(&self.width.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.height.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.origin_x.to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.origin_y.to_ne_bytes());
        bytes[16..20].copy_from_slice(&self.spacing.to_ne_bytes());
        bytes[20..24].copy_from_slice(&self.probes_x.to_ne_bytes());
        bytes[24..28].copy_from_slice(&self.probes_y.to_ne_bytes());
        bytes
    }
}

/// A whole number of texels, or the error naming which number was not one.
///
/// Separate from the cache's constructor so that the check is one expression a
/// test can call rather than a branch inside a longer function.
fn whole_texels(name: &'static str, value: f32) -> Result<i32, RenderError> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return Err(RenderError::ProbeGridNotIntegral { name, value });
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a probe origin or spacing above MAX_DIMENSION is refused by Cascade::new"
    )]
    let whole = value as i32;
    Ok(whole)
}

/// The two compiled write-back pipelines, with the bindings they share.
///
/// One bind group layout and two pipelines, because the two entry points bind
/// the same *shapes*: two textures to read, one to write, one uniform. What
/// changes is which resource goes where, and that is the bind group rather than
/// its layout.
///
/// **Neither pipeline is ever given a texture it also writes.** That is a
/// property of the two call sites rather than of the layout, and it is what
/// `reflect` and `combine` being separate dispatches over separate fields
/// guarantees.
#[derive(Debug)]
pub(crate) struct BounceKernel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    reflect: wgpu::ComputePipeline,
    combine: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl BounceKernel {
    /// Compiles both entry points against the four bindings they share.
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
            label: Some("narvo bounce bindings"),
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

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("narvo bounce"),
            source: wgpu::ShaderSource::Wgsl(BOUNCE_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("narvo bounce layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("narvo bounce"),
                module: &module,
                layout: Some(&pipeline_layout),
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };

        Self {
            device: device.clone(),
            queue: queue.clone(),
            reflect: pipeline(REFLECT_ENTRY),
            combine: pipeline(COMBINE_ENTRY),
            layout,
        }
    }

    /// `target = albedo * radiance`, one texel per invocation.
    pub(crate) fn reflect(
        &self,
        albedo: &Field,
        radiance: &Field,
        target: &Field,
        grid: &BounceParams,
    ) {
        self.run(
            &self.reflect,
            albedo,
            radiance,
            target,
            grid,
            "narvo reflect",
        );
    }

    /// `target = direct + bounced`, one texel per invocation.
    pub(crate) fn combine(
        &self,
        direct: &Field,
        bounced: &Field,
        target: &Field,
        grid: &BounceParams,
    ) {
        self.run(
            &self.combine,
            direct,
            bounced,
            target,
            grid,
            "narvo combine",
        );
    }

    /// The half both entry points share: bind four resources and dispatch over
    /// the field.
    fn run(
        &self,
        pipeline: &wgpu::ComputePipeline,
        a: &Field,
        b: &Field,
        target: &Field,
        grid: &BounceParams,
        label: &str,
    ) {
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uniform, 0, &grid.bytes());

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
                grid.width.div_ceil(BOUNCE_WORKGROUP),
                grid.height.div_ceil(BOUNCE_WORKGROUP),
                1,
            );
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }
}

/// The half of the cache's construction that needs no GPU.
///
/// Separate so that every refusal a caller can trigger is reachable from a
/// machine with no adapter, which is the same reason `Seeds`, `Emission` and
/// `Albedo` are plain data.
///
/// # Errors
///
/// - [`RenderError::EmissionSizeMismatch`] and [`RenderError::AlbedoSizeMismatch`]
///   if either map is not the seed set's size.
/// - [`RenderError::InvalidSize`] if the cascade was validated against a field of
///   another size.
/// - [`RenderError::ProbeGridNotIntegral`] if level zero's origin or spacing is
///   not a whole number of texels.
/// - [`RenderError::CascadeLevelTooLarge`] if the chosen merge does not fit one
///   storage buffer binding.
pub(crate) fn plan(
    extent: [u32; 2],
    emission: &Emission,
    albedo: &Albedo,
    cascade: &Cascade,
    form: MergeForm,
) -> Result<BounceParams, RenderError> {
    let [width, height] = extent;
    if emission.width() != width || emission.height() != height {
        return Err(RenderError::EmissionSizeMismatch {
            seed_width: width,
            seed_height: height,
            emission_width: emission.width(),
            emission_height: emission.height(),
        });
    }
    if albedo.width() != width || albedo.height() != height {
        return Err(RenderError::AlbedoSizeMismatch {
            seed_width: width,
            seed_height: height,
            albedo_width: albedo.width(),
            albedo_height: albedo.height(),
        });
    }
    if cascade.field() != extent {
        return Err(RenderError::InvalidSize {
            width,
            height,
            max: crate::OffscreenTarget::MAX_DIMENSION,
        });
    }
    cascade.check_form(form)?;

    let base = cascade
        .level(0)
        .expect("a cascade has at least one level")
        .layout();
    let origin_x = whole_texels("the level-zero probe origin's x", base.origin[0])?;
    let origin_y = whole_texels("the level-zero probe origin's y", base.origin[1])?;
    let spacing = whole_texels("the level-zero probe spacing", base.spacing)?;
    if spacing == 0 {
        return Err(RenderError::ProbeGridNotIntegral {
            name: "the level-zero probe spacing",
            value: base.spacing,
        });
    }
    Ok(BounceParams {
        width,
        height,
        origin_x,
        origin_y,
        spacing,
        probes_x: base.probes[0],
        probes_y: base.probes[1],
    })
}

/// Assembles a cache from parts [`plan`] has already validated.
///
/// Private to the crate and taking eight arguments, because it is the tail of
/// [`OffscreenTarget::surface_cache`](crate::OffscreenTarget::surface_cache)
/// rather than an API of its own: every argument is a resource that constructor
/// just built, and bundling them would name each of them twice.
#[expect(
    clippy::too_many_arguments,
    reason = "the tail of one constructor; every argument is a resource that constructor built"
)]
pub(crate) fn assemble(
    field: Field,
    direct: Field,
    albedo: Field,
    bounced: Field,
    emission: Field,
    cascade: Cascade,
    form: MergeForm,
    grid: BounceParams,
) -> SurfaceCache {
    SurfaceCache {
        field,
        direct,
        albedo,
        bounced,
        emission,
        cascade,
        form,
        extent: [grid.width, grid.height],
        grid,
        frames: 0,
    }
}

impl SurfaceCache {
    /// Records that a frame of feedback has run.
    pub(crate) fn advanced(&mut self) {
        self.frames = self.frames.saturating_add(1);
    }

    /// Level zero's radiance as a [`RadianceField`], from texels just read back.
    pub(crate) fn radiance_of(&self, texels: Vec<f32>) -> RadianceField {
        let base = self
            .cascade
            .level(0)
            .expect("a cascade has at least one level")
            .layout();
        RadianceField::from_texels(base.probes[0], base.probes[1], texels)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Albedo, BOUNCE_WGSL, BOUNCE_WORKGROUP, COMBINE_ENTRY, REFLECT_ENTRY, whole_texels,
    };
    use crate::RenderError;

    // -- the source guards --------------------------------------------------

    /// **ADR-0049's guard for the write-back.**
    ///
    /// The same reasoning as `cascade.rs`'s and `hierarchy.rs`'s: an
    /// order-dependent write produces a plausible number that no output
    /// comparison can report, so the only check there is is that the machinery is
    /// absent. Two stores, because there are two entry points and each writes its
    /// own texel exactly once.
    #[test]
    fn the_write_back_is_not_written_order_dependently() {
        let body = uncommented(BOUNCE_WGSL);
        for forbidden in [
            "atomic",
            "workgroupBarrier",
            "storageBarrier",
            "workgroupUniformLoad",
            "var<workgroup>",
        ] {
            assert!(
                !body.contains(forbidden),
                "the write-back contains `{forbidden}`, which makes it depend on the \
                 order invocations run in"
            );
        }
        assert_eq!(
            body.matches("textureStore(").count(),
            2,
            "the write-back does not store exactly once per entry point"
        );
    }

    /// **ADR-0051's guard for the write-back, and it is the one that says the
    /// feedback did not spend the rule it inherited.**
    ///
    /// ADR-0051 forbids a float multiply feeding a float add. The write-back has
    /// to compute `direct + albedo * radiance`, which is exactly that shape — so
    /// it does not compute it in one kernel. `reflect` multiplies and **stores**;
    /// `combine` adds two loads and contains no multiply at all. This test reads
    /// both halves of that:
    ///
    /// 1. the float multiplies are the three registered channel products, and
    /// 2. **no line that multiplies also adds**, which is the property the split
    ///    exists to create.
    ///
    /// A session that folds the two entry points back into one has to change this
    /// line, and then has to say which eight adapter/backend pairs it re-measured.
    #[test]
    fn no_float_multiplication_in_the_write_back_feeds_an_addition() {
        let body = uncommented(BOUNCE_WGSL);
        let starred: Vec<String> = body
            .lines()
            .map(str::trim)
            .filter(|line| line.contains('*'))
            .map(str::to_owned)
            .collect();
        assert_eq!(
            starred,
            vec![
                "let r = albedo.x * radiance.x;".to_owned(),
                "let g = albedo.y * radiance.y;".to_owned(),
                "let b = albedo.z * radiance.z;".to_owned(),
            ],
            "the multiplications in the write-back are not the three channel \
             products M8.6 measured. A float multiply feeding an add is the one way \
             two backends were measured to compute two radiance fields"
        );
        for line in &starred {
            assert!(
                !line.contains('+'),
                "`{line}` both multiplies and adds, so a backend may contract it; \
                 the two belong in separate dispatches"
            );
        }
        assert!(
            !body.contains("fma("),
            "the write-back asks for a fused multiply-add"
        );
    }

    /// The dispatch and both entry points agree on the workgroup.
    #[test]
    fn the_dispatch_and_the_write_back_agree_on_the_workgroup() {
        assert!(
            BOUNCE_WGSL.contains(&format!(
                "@workgroup_size({BOUNCE_WORKGROUP}, {BOUNCE_WORKGROUP})"
            )),
            "the write-back does not declare `@workgroup_size({BOUNCE_WORKGROUP}, {BOUNCE_WORKGROUP})`"
        );
        for entry in [REFLECT_ENTRY, COMBINE_ENTRY] {
            assert!(
                BOUNCE_WGSL.contains(&format!("fn {entry}(")),
                "the write-back has no `{entry}` entry point"
            );
        }
    }

    fn uncommented(source: &str) -> String {
        source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -- the map ------------------------------------------------------------

    /// A channel above one is refused, and that is the feedback's stability
    /// condition rather than a taste in colour.
    #[test]
    fn an_albedo_above_one_is_refused() {
        let mut map = Albedo::new(4, 4).expect("a usable size");
        for rgb in [
            [1.000_001_f32, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [0.0, 0.0, f32::INFINITY],
        ] {
            let error = map.set(1, 1, rgb).expect_err("above one is refused");
            assert!(
                matches!(
                    error,
                    RenderError::StageParameter {
                        name: "an albedo channel",
                        ..
                    }
                ),
                "an albedo of {rgb:?} was refused as {error}"
            );
        }
        // And exactly one is accepted, which is the white chamber's whole
        // premise: the boundary belongs to the usable side.
        map.set(1, 1, [1.0, 1.0, 1.0])
            .expect("albedo one is usable");
    }

    /// A channel below zero, or not a number, is refused for `Emission`'s reason.
    #[test]
    fn an_albedo_below_zero_or_not_a_number_is_refused() {
        let mut map = Albedo::new(4, 4).expect("a usable size");
        for rgb in [[-0.001_f32, 0.0, 0.0], [0.0, f32::NAN, 0.0]] {
            let error = map.set(0, 0, rgb).expect_err("the value is refused");
            assert!(
                matches!(
                    error,
                    RenderError::StageParameter {
                        name: "an albedo channel",
                        ..
                    }
                ),
                "an albedo of {rgb:?} was refused as {error}"
            );
        }
    }

    /// A point outside the map is refused, naming the map's own size.
    #[test]
    fn an_albedo_outside_the_map_is_refused() {
        let mut map = Albedo::new(4, 3).expect("a usable size");
        let error = map
            .set(4, 0, [0.5, 0.5, 0.5])
            .expect_err("outside is refused");
        assert!(
            matches!(
                error,
                RenderError::AlbedoOutsideField {
                    x: 4,
                    y: 0,
                    width: 4,
                    height: 3
                }
            ),
            "the refusal did not name the point and the map: {error}"
        );
    }

    /// A fresh map absorbs, a uniform one reflects, and a set channel reads back.
    #[test]
    fn a_map_absorbs_until_it_is_told_otherwise() {
        let mut map = Albedo::new(3, 2).expect("a usable size");
        assert_eq!(map.get(0, 0), Some([0.0, 0.0, 0.0]), "a fresh map absorbs");
        assert_eq!(map.get(3, 0), None, "outside the map");

        map.set(2, 1, [0.25, 0.5, 1.0]).expect("a usable albedo");
        assert_eq!(map.get(2, 1), Some([0.25, 0.5, 1.0]));
        assert_eq!(
            map.get(1, 1),
            Some([0.0, 0.0, 0.0]),
            "the neighbour is untouched"
        );

        let all = Albedo::uniform(3, 2, [0.5, 0.5, 0.5]).expect("a usable map");
        for y in 0..2 {
            for x in 0..3 {
                assert_eq!(all.get(x, y), Some([0.5, 0.5, 0.5]), "at ({x}, {y})");
            }
        }
    }

    /// A dimension a field cannot have is refused before anything is allocated.
    #[test]
    fn an_albedo_map_of_an_impossible_size_is_refused() {
        for (width, height) in [
            (0, 4),
            (4, 0),
            (crate::OffscreenTarget::MAX_DIMENSION + 1, 4),
        ] {
            let error = Albedo::new(width, height).expect_err("the size is refused");
            assert!(
                matches!(error, RenderError::InvalidSize { .. }),
                "{width}x{height} was refused as {error}"
            );
        }
    }

    // -- the integer probe grid ---------------------------------------------

    /// **ADR-0050's reasoning reaching an index.**
    ///
    /// A whole number of texels passes and anything else is refused by name, so
    /// the write-back's division is exact by construction rather than by
    /// rounding.
    #[test]
    fn a_probe_grid_that_cannot_be_indexed_in_integers_is_refused() {
        for good in [0.0_f32, 1.0, 4.0, 64.0] {
            assert_eq!(
                whole_texels("the level-zero probe spacing", good).expect("a whole number"),
                good as i32,
                "{good} is a whole number of texels"
            );
        }
        for bad in [0.5_f32, 4.25, -1.0, f32::NAN, f32::INFINITY] {
            let error = whole_texels("the level-zero probe spacing", bad)
                .expect_err("a fractional spacing is refused");
            assert!(
                matches!(
                    error,
                    RenderError::ProbeGridNotIntegral {
                        name: "the level-zero probe spacing",
                        ..
                    }
                ),
                "{bad} was refused as {error}"
            );
        }
    }
}
