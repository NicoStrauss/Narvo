//! Marching a ray against a distance field.
//!
//! M8.4, and the posting of the transfer table that carries over **whole**: this
//! is the sphere trace Lumen runs in 3D, with one dimension fewer. It answers
//! two questions — whether two points can see each other, and how far a ray gets
//! before it meets something.
//!
//! # Why it runs on the GPU, measured rather than assumed
//!
//! M8.3b reported that `distance_field` costs 22 ms at 1920 x 1080 and named the
//! read-back as a hypothesis. M8.4 measured the phases instead of building on
//! that, and the answer moved the design:
//!
//! | phase | ms at 1920 x 1080 | where |
//! |---|---|---|
//! | the eleven flooding passes | **0.29** | GPU |
//! | uploading and reading back the field, and rebuilding it as a `SeedMap` | **16.9** | CPU |
//! | everything else | 3.4 | mostly GPU |
//!
//! **The flooding is 1.4 % of the cost and the CPU round trip is 79.5 %.** A
//! march that runs where the field already is pays none of the round trip: the
//! field never leaves the GPU, and only one small [`MarchHit`] per ray comes
//! back. That is a posting that disappears by architecture rather than by
//! optimisation, which is why the march is a compute kernel and not a function
//! over [`SeedMap`](crate::SeedMap).
//!
//! # The step is shortened by one texel, and the number is derived
//!
//! A sphere trace needs a **lower** bound on the distance to the nearest
//! occluder. The field gives an upper one — it stores the distance to *a* seeded
//! texel, and jump flooding was measured keeping one up to 0.242480 texels too
//! far. `shaders/march.wgsl`'s header carries the derivation; the short form is
//! `margin >= sqrt(0.5) + 0.242480 = 0.949587`, so one whole texel is enough.
//!
//! M8.3a handed M8.4 a lever and M8.4 declined it, with numbers rather than by
//! preference. Two extra flooding passes drive the over-estimate to
//! **0.000000** on all six arrangements measured — but the other term, the
//! `sqrt(0.5)` a position can be from its own texel's centre, cannot be reduced
//! by any number of passes. The best reachable margin is therefore 0.707107
//! against this 1.0, and buying it would rest on an over-estimate of zero being
//! measured on six arrangements rather than proven. The report carries the
//! table.

use crate::error::RenderError;
use crate::field::Field;

/// The kernel's source.
pub(crate) const MARCH_WGSL: &str = include_str!("shaders/march.wgsl");

/// The entry point in [`MARCH_WGSL`].
pub(crate) const MARCH_ENTRY: &str = "march";

/// Invocations per workgroup. One dimension: a ray list is a list.
pub(crate) const MARCH_WORKGROUP: u32 = 64;

/// Fixed-point units to the texel, on both sides of the wire.
const FIXED: f64 = 256.0;

/// A segment to march, from one point to another, in field texels.
///
/// Positions are in **field space**: `(0.0, 0.0)` is the corner of texel
/// `(0, 0)` and `(1.5, 2.5)` is the centre of texel `(1, 2)`. That is the same
/// convention `narvo_view2d::seeds_of` seeds under — a texel is seeded when its
/// centre falls inside an occluder — so a ray and the thing it might hit are
/// measured in one system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    from_x: i32,
    from_y: i32,
    dir_x: i32,
    dir_y: i32,
    length: i32,
    /// Set when this ray's interval lies entirely outside the field.
    ///
    /// **The march ignores it**, and that is the whole point: `march.wgsl` reads
    /// five words and this is the sixth, which it has always had room for and
    /// never read. What the flag is for lives one layer up — a cascade level
    /// whose near end is beyond the field edge in some direction has a direction
    /// that met *nothing*, and a zero-length ray sitting on the border cannot say
    /// that: the march would answer about whichever texel the border happens to
    /// hold. M8.5b's kernels read the flag and treat such a direction as having
    /// escaped.
    ///
    /// It is `0` or `1` and nothing else, and it is only ever set by
    /// [`Self::escaping`].
    escaping: i32,
}

impl Ray {
    /// A ray from `(from_x, from_y)` to `(to_x, to_y)`, both in field texels.
    ///
    /// # The normalisation happens here, in `f64`, and that is deliberate
    ///
    /// The kernel needs a unit direction and computing one needs a square root.
    /// Doing it here keeps `sqrt` out of WGSL — M8.3a's standing assurance — and
    /// costs nothing, because a direction is a parameter of the query rather
    /// than something the march derives. `f64::sqrt` is one operation on one
    /// input, so two runs on one machine agree; whether two *platforms* agree on
    /// the last bit is not measured, and a ray whose direction differed in its
    /// last fixed-point unit would be a different query rather than a wrong
    /// answer.
    ///
    /// # Errors
    ///
    /// [`RenderError::RayOutsideField`] if either endpoint is outside
    /// `0..=width` by `0..=height`. **This is the guard, and the kernel's clamp
    /// is not** — M8.3b measured that a clamp at a field edge can mask an
    /// off-by-one, so the position is refused here where it can be named rather
    /// than quietly folded onto the border.
    ///
    /// [`RenderError::RayNotFinite`] if any coordinate is not finite. A `NaN`
    /// endpoint has no fixed-point image at all, so it is refused rather than
    /// cast.
    pub fn new(
        from_x: f32,
        from_y: f32,
        to_x: f32,
        to_y: f32,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        for value in [from_x, from_y, to_x, to_y] {
            if !value.is_finite() {
                return Err(RenderError::RayNotFinite);
            }
        }
        for (x, y) in [(from_x, from_y), (to_x, to_y)] {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a field dimension is at most 8192, exact in an f32"
            )]
            let (limit_x, limit_y) = (width as f32, height as f32);
            if x < 0.0 || y < 0.0 || x > limit_x || y > limit_y {
                return Err(RenderError::RayOutsideField {
                    x,
                    y,
                    width,
                    height,
                });
            }
        }

        let (dx, dy) = (f64::from(to_x - from_x), f64::from(to_y - from_y));
        let length = (dx * dx + dy * dy).sqrt();
        // A zero-length ray has no direction. It is legal and its answer is
        // decided by whether its single position is inside an occluder, so the
        // direction is arbitrary and the length is zero.
        let (unit_x, unit_y) = if length == 0.0 {
            (0.0, 0.0)
        } else {
            (dx / length, dy / length)
        };

        #[expect(
            clippy::cast_possible_truncation,
            reason = "every value below is bounded by 8192 * 256, far inside i32"
        )]
        let ray = Self {
            from_x: (f64::from(from_x) * FIXED).round() as i32,
            from_y: (f64::from(from_y) * FIXED).round() as i32,
            dir_x: (unit_x * FIXED).round() as i32,
            dir_y: (unit_y * FIXED).round() as i32,
            length: (length * FIXED).round() as i32,
            escaping: 0,
        };
        Ok(ray)
    }

    /// How far the far end is, in texels.
    #[must_use]
    pub fn length(&self) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a fixed-point length is below 8192 * 256 * 1.5, exact in an f32"
        )]
        let length = self.length as f32 / 256.0;
        length
    }

    /// Marks this ray as one whose interval lies entirely outside the field.
    ///
    /// See [`Self::escaping`](Ray#structfield.escaping). Consumed by M8.5b's
    /// cascade kernels; **the march does not read it**, so a marked ray marches
    /// exactly as an unmarked one would.
    #[must_use]
    pub(crate) fn escaping(mut self) -> Self {
        self.escaping = 1;
        self
    }

    /// Whether [`Self::escaping`] was called on this ray.
    ///
    /// **Read only by `cascade.rs`'s own tests**, which is what the attribute
    /// says: the kernels read the flag out of the storage buffer rather than
    /// through this accessor, so nothing in a non-test build calls it. It exists
    /// because the mark is otherwise unobservable from the CPU, and a mark
    /// nothing can check is a mark nobody can guard.
    #[must_use]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the kernels read the flag from the buffer; only the guard for it calls this"
        )
    )]
    pub(crate) fn is_escaping(&self) -> bool {
        self.escaping != 0
    }

    /// The eight words this ray occupies in the kernel's storage buffer.
    fn words(&self) -> [i32; 8] {
        [
            self.from_x,
            self.from_y,
            self.dir_x,
            self.dir_y,
            self.length,
            self.escaping,
            0,
            0,
        ]
    }

    /// The provably sufficient step budget for this ray.
    ///
    /// **Derived and not chosen.** A step the kernel takes is either zero — in
    /// which case it stops — or at least one fixed-point unit, because the step
    /// is an integer and the loop only continues while it is positive. So a
    /// march cannot take more steps than the segment has fixed-point units in
    /// it, and one more covers the step on which it arrives.
    ///
    /// It is a *bound*, not an estimate: an ordinary march takes a handful of
    /// steps. The pathological case it exists for is a ray running alongside a
    /// wall at a distance just over the margin, where each step advances by a
    /// sliver.
    fn derived_budget(&self) -> u32 {
        u32::try_from(self.length)
            .unwrap_or(u32::MAX)
            .saturating_add(1)
    }
}

/// What a march concluded, and how it got there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarchVerdict {
    /// The ray reached its far end without meeting an occluder.
    Visible,
    /// The ray came within a texel of a seeded texel and stopped.
    Blocked,
    /// The ray ran out of steps.
    ///
    /// **Deliberately its own verdict rather than folded into `Blocked` or
    /// `Visible`.** A march that ran out of steps has not established anything;
    /// reporting it as visible would be claiming something it never checked,
    /// which is the failure §3(d) of M8.4's brief names. Reporting it as blocked
    /// would be safer but still a claim. A caller that wants a conservative
    /// answer treats this as "not visible" — which is what
    /// [`MarchHit::is_visible`] does — and a caller that wants to know it ran
    /// out can see that it did.
    Exhausted,
}

/// One ray's answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarchHit {
    verdict: MarchVerdict,
    distance: f32,
    steps: u32,
}

impl MarchHit {
    /// What the march concluded.
    #[must_use]
    pub fn verdict(&self) -> MarchVerdict {
        self.verdict
    }

    /// How far the ray travelled, in texels.
    ///
    /// The full segment length when [`MarchVerdict::Visible`], the distance to
    /// the stopping point when [`MarchVerdict::Blocked`], and how far it had got
    /// when [`MarchVerdict::Exhausted`].
    #[must_use]
    pub fn distance(&self) -> f32 {
        self.distance
    }

    /// How many steps it took. The evidence for the termination oracle, and what
    /// a caller sizing a budget for [`OffscreenTarget::march_within`](crate::OffscreenTarget::march_within)
    /// measures.
    #[must_use]
    pub fn steps(&self) -> u32 {
        self.steps
    }

    /// Whether the far end was reached, treating exhaustion as "no".
    ///
    /// The conservative reading, and the one a light wants: a ray that did not
    /// finish did not establish a line of sight.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.verdict == MarchVerdict::Visible
    }

    /// Reads one hit out of the four words the kernel wrote.
    pub(crate) fn from_words(words: [i32; 4]) -> Self {
        let verdict = match words[0] {
            1 => MarchVerdict::Visible,
            2 => MarchVerdict::Exhausted,
            _ => MarchVerdict::Blocked,
        };
        #[expect(
            clippy::cast_precision_loss,
            reason = "a fixed-point distance is below 8192 * 256 * 1.5, exact in an f32"
        )]
        let distance = words[1] as f32 / 256.0;
        Self {
            verdict,
            distance,
            steps: words[2].unsigned_abs(),
        }
    }
}

/// The compiled march kernel, with the bindings it needs.
#[derive(Debug)]
pub(crate) struct MarchKernel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl MarchKernel {
    /// Compiles the kernel against its four bindings.
    ///
    /// Spelled out rather than derived from the module, for the reason
    /// `FieldKernel::new` gives: a derived layout is a layout nothing in this
    /// crate states.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("narvo march bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        // Not filterable, for `FieldKernel`'s reason: nothing
                        // samples, `textureLoad` fetches a texel and
                        // interpolates nothing, and the device requests no
                        // features.
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
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
            label: Some("narvo march"),
            source: wgpu::ShaderSource::Wgsl(MARCH_WGSL.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("narvo march layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("narvo march"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(MARCH_ENTRY),
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

    /// Marches every ray against `field` and leaves the answers **on the GPU**.
    ///
    /// Returns the ray buffer and the hit buffer, in that order, with the march
    /// already submitted. Both are storage buffers a later pass in the same
    /// queue can bind; the hit buffer also carries `COPY_SRC`, which is what
    /// [`Self::run`] uses to bring it back.
    ///
    /// **This split is M8.5a's, and it is the whole of what the cascade needed
    /// from M8.4.** A stage marches a probe's directions and then integrates
    /// them, and a round trip to the CPU in between would pay exactly the
    /// marshalling cost this module's header measured away. Nothing about what
    /// the march *computes* moved: `run` is this function plus the read-back it
    /// always did, and M8.4's fourteen tests are unchanged.
    ///
    /// # Panics
    ///
    /// `rays` must not be empty — a zero-sized storage buffer is not a thing
    /// wgpu will create. Both callers check first, which is why this takes no
    /// `Result` for it.
    pub(crate) fn dispatch(
        &self,
        field: &Field,
        rays: &[Ray],
        max_steps: u32,
    ) -> (wgpu::Buffer, wgpu::Buffer) {
        assert!(
            !rays.is_empty(),
            "a march of no rays has no buffers to dispatch over"
        );

        let mut ray_bytes = Vec::with_capacity(rays.len() * 32);
        for ray in rays {
            for word in ray.words() {
                ray_bytes.extend_from_slice(&word.to_ne_bytes());
            }
        }
        let ray_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("narvo march rays"),
            size: ray_bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&ray_buffer, 0, &ray_bytes);

        let hit_bytes = (rays.len() * 16) as u64;
        let hit_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("narvo march hits"),
            size: hit_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let mut params = [0_u8; 16];
        params[0..4].copy_from_slice(&field.width().to_ne_bytes());
        params[4..8].copy_from_slice(&field.height().to_ne_bytes());
        params[8..12].copy_from_slice(&(rays.len() as u32).to_ne_bytes());
        params[12..16].copy_from_slice(&max_steps.to_ne_bytes());
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("narvo march params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uniform, 0, &params);

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("narvo march"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(field.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ray_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: hit_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo march"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("narvo march"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = u32::try_from(rays.len())
                .unwrap_or(u32::MAX)
                .div_ceil(MARCH_WORKGROUP);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        (ray_buffer, hit_buffer)
    }

    /// [`Self::dispatch`], then the sixteen bytes a ray brought back.
    ///
    /// The **field stays on the GPU**; what crosses back is the hits. That is
    /// the architectural point of this module and the reason the 16.9 ms of CPU
    /// marshalling in this module's header does not apply here.
    ///
    /// # Errors
    ///
    /// [`RenderError::Readback`] if the answers could not be copied back.
    pub(crate) fn run(
        &self,
        field: &Field,
        rays: &[Ray],
        max_steps: u32,
    ) -> Result<Vec<MarchHit>, RenderError> {
        if rays.is_empty() {
            return Ok(Vec::new());
        }
        let hit_bytes = (rays.len() * 16) as u64;
        let (_rays, hit_buffer) = self.dispatch(field, rays, max_steps);

        let transfer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("narvo march read-back"),
            size: hit_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo march read-back"),
            });
        encoder.copy_buffer_to_buffer(&hit_buffer, 0, &transfer, 0, hit_bytes);
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = transfer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|error| RenderError::Readback {
                step: "waiting for the GPU to finish the march",
                source: Box::new(error),
            })?;
        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(RenderError::Readback {
                    step: "mapping the march transfer buffer",
                    source: Box::new(error),
                });
            }
            Err(error) => {
                return Err(RenderError::Readback {
                    step: "waiting for the march mapping callback after a blocking poll",
                    source: Box::new(error),
                });
            }
        }

        let mapped = slice
            .get_mapped_range()
            .map_err(|error| RenderError::Readback {
                step: "reading the mapped march transfer buffer",
                source: Box::new(error),
            })?;
        let hits = mapped
            .chunks_exact(16)
            .map(|chunk| {
                let word = |at: usize| {
                    i32::from_ne_bytes([chunk[at], chunk[at + 1], chunk[at + 2], chunk[at + 3]])
                };
                MarchHit::from_words([word(0), word(4), word(8), word(12)])
            })
            .collect();
        drop(mapped);
        transfer.unmap();

        Ok(hits)
    }
}

/// The step budget that covers every ray in `rays`.
pub(crate) fn derived_budget(rays: &[Ray]) -> u32 {
    rays.iter().map(Ray::derived_budget).max().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::{FIXED, MARCH_WGSL, MARCH_WORKGROUP, MarchVerdict, Ray, derived_budget};
    use crate::{OffscreenTarget, RenderError, Seeds};

    /// A target to borrow a device from, or `None` on a machine with no adapter.
    fn target_or_skip() -> Option<OffscreenTarget> {
        match OffscreenTarget::new(8, 8) {
            Ok(target) => Some(target),
            Err(RenderError::NoAdapter { .. }) => None,
            Err(other) => {
                panic!("the offscreen target failed for a reason that is not absence: {other}")
            }
        }
    }

    const SIDE: u32 = 64;

    /// A seed set of the given texels in a `SIDE` x `SIDE` field.
    fn seeds_at(points: &[(u32, u32)]) -> Seeds {
        let mut seeds = Seeds::new(SIDE, SIDE).expect("a legal size");
        for &(x, y) in points {
            seeds.set(x, y).expect("inside");
        }
        seeds
    }

    /// A vertical wall one texel wide at column `x`, spanning the field.
    fn thin_wall(x: u32) -> Seeds {
        seeds_at(&(0..SIDE).map(|y| (x, y)).collect::<Vec<_>>())
    }

    fn ray(from: (f32, f32), to: (f32, f32)) -> Ray {
        Ray::new(from.0, from.1, to.0, to.1, SIDE, SIDE).expect("inside the field")
    }

    /// **The oracle the GPU march is checked against: the same algorithm, on the
    /// CPU, in `i64`.**
    ///
    /// Written from `march.wgsl` line by line — the same margin, the same
    /// fixed-point positions, the same integer square root, the same three
    /// verdicts. It is not a *simpler* model, and that is the honest limitation:
    /// it can show that the GPU runs what this says, and it cannot show that what
    /// this says is right. The four oracles below are what does that, and each is
    /// stated against a closed form or an invariant rather than against this.
    ///
    /// It differs in one place on purpose: the field is brute force rather than
    /// jump flooding, so the model does not inherit the approximation the margin
    /// exists to absorb. Where the two agree, the margin did its job.
    fn cpu_march(seeds: &Seeds, ray: &Ray, budget: u32) -> (MarchVerdict, i64, u32) {
        let (width, height) = (i64::from(seeds.width()), i64::from(seeds.height()));
        let mut points = Vec::new();
        for y in 0..seeds.height() {
            for x in 0..seeds.width() {
                if seeds.is_seed(x, y) {
                    points.push((i64::from(x), i64::from(y)));
                }
            }
        }

        let mut travelled: i64 = 0;
        let mut steps: u32 = 0;
        let mut verdict = MarchVerdict::Exhausted;
        loop {
            if steps >= budget {
                break;
            }
            steps += 1;

            let px = i64::from(ray.from_x) + (i64::from(ray.dir_x) * travelled) / 256;
            let py = i64::from(ray.from_y) + (i64::from(ray.dir_y) * travelled) / 256;
            let tx = (px >> 8).clamp(0, width - 1);
            let ty = (py >> 8).clamp(0, height - 1);

            let Some(squared) = points
                .iter()
                .map(|&(sx, sy)| (sx - tx) * (sx - tx) + (sy - ty) * (sy - ty))
                .min()
            else {
                verdict = MarchVerdict::Visible;
                travelled = i64::from(ray.length);
                break;
            };

            let whole = squared.isqrt();
            let rest = squared - whole * whole;
            let below = (whole << 8) + (rest << 8) / (2 * whole + 1);
            let step = below - 256;
            if step <= 0 {
                verdict = MarchVerdict::Blocked;
                break;
            }
            travelled += step;
            if travelled >= i64::from(ray.length) {
                verdict = MarchVerdict::Visible;
                travelled = i64::from(ray.length);
                break;
            }
        }
        (verdict, travelled, steps)
    }

    // -- CPU only ----------------------------------------------------------

    /// A ray refuses an endpoint the field does not contain, and a `NaN` one.
    ///
    /// **The guard, where the kernel's clamp is not.** M8.3b measured that a
    /// clamp at a field edge can absorb an off-by-one and leave a test green, so
    /// the position is refused here where the message can name it.
    #[test]
    fn a_ray_refuses_an_endpoint_the_field_does_not_contain() {
        assert!(matches!(
            Ray::new(-0.5, 1.0, 2.0, 2.0, SIDE, SIDE),
            Err(RenderError::RayOutsideField { .. })
        ));
        let Err(RenderError::RayOutsideField {
            x,
            y,
            width,
            height,
        }) = Ray::new(1.0, 1.0, 64.5, 2.0, SIDE, SIDE)
        else {
            panic!("an endpoint past the right edge was accepted");
        };
        assert_eq!((x, y, width, height), (64.5, 2.0, SIDE, SIDE));

        assert!(matches!(
            Ray::new(f32::NAN, 1.0, 2.0, 2.0, SIDE, SIDE),
            Err(RenderError::RayNotFinite)
        ));
        assert!(matches!(
            Ray::new(1.0, 1.0, 2.0, f32::INFINITY, SIDE, SIDE),
            Err(RenderError::RayNotFinite)
        ));

        // The corners are inside: the field spans 0..=width by 0..=height.
        Ray::new(0.0, 0.0, 64.0, 64.0, SIDE, SIDE).expect("the far corner is inside");
    }

    /// A ray's length is what its endpoints say, and a zero-length ray is legal.
    #[test]
    fn a_ray_carries_the_length_its_endpoints_give_it() {
        let across = ray((1.0, 1.0), (4.0, 5.0));
        assert!(
            (across.length() - 5.0).abs() < 1.0 / 256.0,
            "a 3-4-5 ray reported {}",
            across.length()
        );

        let still = ray((3.0, 3.0), (3.0, 3.0));
        assert_eq!(still.length(), 0.0);
        assert_eq!((still.dir_x, still.dir_y), (0, 0));
    }

    /// **§3(d): the step budget is derived, and the derivation is what it says.**
    ///
    /// A step is either zero — which ends the march — or at least one
    /// fixed-point unit, because it is an integer and the loop continues only
    /// while it is positive. So a ray cannot take more steps than its length in
    /// fixed-point units, plus the one it arrives on. The budget is that number,
    /// asserted here against the length rather than against a constant.
    #[test]
    fn the_step_budget_is_the_length_in_fixed_point_units() {
        for (from, to) in [
            ((0.0_f32, 0.0_f32), (1.0_f32, 0.0_f32)),
            ((0.0, 0.0), (10.0, 0.0)),
            ((0.0, 0.0), (60.0, 60.0)),
        ] {
            let r = ray(from, to);
            let budget = derived_budget(&[r]);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a length below 64 * 1.5 texels is far inside u32 once scaled"
            )]
            let expected = (f64::from(r.length()) * FIXED).round() as u32 + 1;
            assert_eq!(
                budget,
                expected,
                "a ray of {} texels got a budget of {budget}",
                r.length()
            );
        }
        assert_eq!(
            derived_budget(&[]),
            1,
            "an empty ray list still needs a budget"
        );
    }

    // -- source reads ------------------------------------------------------

    /// **M8.3a's standing assurance, still true: no square root in WGSL.**
    ///
    /// M8.4 is the task that could have broken it — a march needs distances on
    /// the GPU — and did not, by taking `isqrt` of an exact integer and
    /// normalising the ray's direction once on the CPU. ADR-0050 needs no
    /// amendment as a result, which is what this test is really holding.
    #[test]
    fn the_march_kernel_takes_no_square_root_and_no_float_distance() {
        let body: String = MARCH_WGSL
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        // `isqrt` contains `sqrt`, and the first version of this guard fired on
        // the kernel's own integer root. Removing the integer names first is
        // what makes the check about the *float* square root it is aimed at —
        // and the guard is kept rather than loosened, because a bare `sqrt(`
        // slipping in is exactly the regression it exists for.
        let body = body.replace("isqrt", "@").replace("distance_below", "@");
        for forbidden in [
            "sqrt",
            "inverseSqrt",
            "pow",
            "exp",
            "log",
            "sin",
            "cos",
            "fma",
            "distance(",
            "length(",
            "normalize",
        ] {
            assert!(
                !body.contains(forbidden),
                "the march kernel contains `{forbidden}`, which M8.3a's assurance \
                 and ADR-0050 between them keep out of WGSL"
            );
        }
        assert!(
            !body.contains("atomic") && !body.contains("workgroupBarrier"),
            "the march kernel reaches for order-dependent arithmetic"
        );
        assert!(
            MARCH_WGSL.contains("fn isqrt(") && MARCH_WGSL.contains("fn distance_below("),
            "the kernel no longer computes its distance as an integer lower bound"
        );
    }

    /// The step really is shortened, and by the derived amount.
    ///
    /// A literal against a literal, worth one thing: a session that changes the
    /// margin has to change this line too, and then has to say which numbers it
    /// re-derived. The derivation is `sqrt(0.5) + 0.242480 = 0.949587`, so one
    /// whole texel.
    #[test]
    fn the_step_is_shortened_by_one_whole_texel() {
        let body: String = MARCH_WGSL
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            body.contains("const MARGIN: i32 = 256;"),
            "the march's margin is not one texel"
        );
        assert!(
            body.contains("distance_below(squared) - MARGIN"),
            "the march no longer steps by a shortened distance"
        );
    }

    /// The dispatch and the kernel agree on the workgroup.
    #[test]
    fn the_dispatch_and_the_kernel_agree_on_the_workgroup() {
        assert!(
            MARCH_WGSL.contains(&format!("@workgroup_size({MARCH_WORKGROUP})")),
            "the kernel does not declare `@workgroup_size({MARCH_WORKGROUP})`"
        );
    }

    // -- GPU ---------------------------------------------------------------

    /// **Oracle (a): unoccluded, the march arrives at the analytic distance.**
    ///
    /// # The tolerance, derived rather than chosen
    ///
    /// A visible march reports exactly `ray.length`, because that is what the
    /// kernel writes when it arrives. So the only error between the reported
    /// distance and `|B - A|` is the one `Ray::new` introduced rounding the
    /// length into fixed point, which is **half a fixed-point unit**:
    /// `0.5 / 256 = 0.001953` texels. The assertion uses one whole unit,
    /// `1/256`, which is that bound rounded outward.
    ///
    /// It is not the march's own accumulated error, because a visible march does
    /// not accumulate: it clamps to the length on arrival.
    #[test]
    fn an_unoccluded_march_arrives_at_the_analytic_distance() {
        let Some(target) = target_or_skip() else {
            return;
        };
        // One seed in a far corner, so nothing is near the rays below.
        let seeds = seeds_at(&[(63, 63)]);
        let cases = [
            ((2.0_f32, 2.0_f32), (2.0_f32, 30.0_f32)),
            ((2.0, 2.0), (30.0, 2.0)),
            ((4.0, 4.0), (16.0, 12.0)),
            ((1.0, 30.0), (25.0, 6.0)),
        ];
        let rays: Vec<Ray> = cases.iter().map(|&(a, b)| ray(a, b)).collect();
        let hits = target.march(&seeds, &rays).expect("the march runs");

        for (index, &(a, b)) in cases.iter().enumerate() {
            let analytic = f64::from((b.0 - a.0).hypot(b.1 - a.1));
            let hit = hits[index];
            assert_eq!(
                hit.verdict(),
                MarchVerdict::Visible,
                "case {index} was blocked by nothing"
            );
            assert!(
                (f64::from(hit.distance()) - analytic).abs() <= 1.0 / 256.0,
                "case {index} reported {} against an analytic {analytic}",
                hit.distance()
            );
        }
    }

    /// **Oracle (b): fully occluded returns zero distance and a blocked verdict.**
    ///
    /// The ray starts *inside* the wall, so the field's distance at its first
    /// position is zero, the step is negative, and the march stops before it has
    /// gone anywhere. Zero is exact: no tolerance, because no arithmetic ran.
    #[test]
    fn a_march_that_starts_inside_an_occluder_returns_zero() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = thin_wall(32);
        let rays = [ray((32.5, 10.5), (50.0, 10.5))];
        let hits = target.march(&seeds, &rays).expect("the march runs");

        assert_eq!(hits[0].verdict(), MarchVerdict::Blocked);
        assert_eq!(
            hits[0].distance(),
            0.0,
            "a march inside a wall went somewhere"
        );
    }

    /// **The defect this task exists to catch: a one-texel wall stops a march.**
    ///
    /// A wall one texel wide is the thinnest thing the field can describe, and it
    /// is exactly what an over-estimating step jumps through — the field says
    /// "nothing within d" while the truth is "a wall at d - 0.24", so a march
    /// stepping the full d lands past it and reports a visibility that is not
    /// there. Every ray below crosses the wall and every one must be blocked.
    #[test]
    fn a_one_texel_wall_blocks_every_ray_that_crosses_it() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = thin_wall(32);

        let mut rays = Vec::new();
        for row in 0..SIDE {
            #[expect(
                clippy::cast_precision_loss,
                reason = "a row index below 64 is exact in an f32"
            )]
            let y = row as f32 + 0.5;
            rays.push(ray((2.0, y), (62.0, y)));
        }
        let hits = target.march(&seeds, &rays).expect("the march runs");

        for (row, hit) in hits.iter().enumerate() {
            assert_eq!(
                hit.verdict(),
                MarchVerdict::Blocked,
                "row {row} crossed a one-texel wall and was not blocked; it \
                 travelled {} of 60 texels",
                hit.distance()
            );
            assert!(
                hit.distance() < 31.0,
                "row {row} stopped at {}, which is past the wall at 32",
                hit.distance()
            );
        }
    }

    /// **Oracle (c): reciprocity is a *bound*, and the plan called it a
    /// certainty.**
    ///
    /// The plan names this "the most valuable of the four: cheap to check and
    /// catches almost every sign and index error", and M8.4's brief said to check
    /// whether it holds before writing it as a test. It does not.
    ///
    /// A march from A to B samples the field at positions that depend on the
    /// distances it meets along the way, and a march from B to A samples
    /// different ones. Where an occluder grazes the segment — passing within a
    /// texel or two — one direction can land near it and stop while the other
    /// steps past. Both answers are defensible; they are not the same answer.
    ///
    /// **Measured over 128 352 ordered pairs across seven arrangements: 869
    /// disagree, 0.677 %.** The rate is not uniform — 0.229 % where a single
    /// corner is the only occluder, **2.563 %** over forty scattered seeds, where
    /// almost every segment grazes something. The disagreements go both ways.
    ///
    /// **The first version of this test asserted zero and passed**, because its
    /// twenty-five gridded endpoints never produced a grazing segment. That is
    /// the more useful half of the finding: an invariant that does not hold can
    /// look like one for as long as the sample is coarse. The arrangement below
    /// includes scattered seeds and off-lattice endpoints for exactly that
    /// reason, and the bound is the measurement rounded outward.
    #[test]
    fn reciprocity_holds_only_within_its_measured_bound() {
        let Some(target) = target_or_skip() else {
            return;
        };

        let mut scatter = Vec::new();
        let mut state: u64 = 0xf00d_0000_0000_0001;
        while scatter.len() < 40 {
            let mut next = || {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((state >> 33) as u32) % SIDE
            };
            let point = (next(), next());
            if !scatter.contains(&point) {
                scatter.push(point);
            }
        }

        // (name, seeds, the most of the pairs below that may disagree)
        let arrangements: [(&str, Seeds, f64); 3] = [
            ("thin wall", thin_wall(32), 0.01),
            ("corner", seeds_at(&[(32, 32), (33, 32), (32, 33)]), 0.01),
            ("scatter40", seeds_at(&scatter), 0.05),
        ];

        for (name, seeds, allowed) in &arrangements {
            // Off-lattice on purpose: endpoints on texel centres produce
            // segments that meet occluders squarely, which is the case
            // reciprocity survives.
            let mut points = Vec::new();
            for i in 0..6_u32 {
                for j in 0..6_u32 {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "an index below six is exact in an f32"
                    )]
                    let (i, j) = (i as f32, j as f32);
                    points.push((1.3 + i * 12.1, 2.7 + j * 11.9));
                }
            }

            let mut forward = Vec::new();
            let mut backward = Vec::new();
            for (index, &a) in points.iter().enumerate() {
                for &b in points.iter().skip(index + 1) {
                    forward.push(ray(a, b));
                    backward.push(ray(b, a));
                }
            }

            let there = target.march(seeds, &forward).expect("the march runs");
            let back = target.march(seeds, &backward).expect("the march runs");

            let differing = there
                .iter()
                .zip(back.iter())
                .filter(|(a, b)| a.is_visible() != b.is_visible())
                .count();
            #[expect(
                clippy::cast_precision_loss,
                reason = "a pair count below a thousand is exact in an f64"
            )]
            let ratio = differing as f64 / there.len() as f64;
            assert!(
                ratio <= *allowed,
                "on {name}, {differing} of {} pairs ({ratio:.6}) disagreed about \
                 visibility depending on which end the march started from, past \
                 the {allowed} M8.4 measured",
                there.len()
            );
        }
    }

    /// **Oracle (d): a march terminates inside its derived budget.**
    ///
    /// Not "it did not hang" — the budget makes hanging impossible — but that the
    /// derived budget is never actually reached, so no march here ends as
    /// `Exhausted`. That is what says the derivation is a bound rather than a
    /// wish.
    #[test]
    fn no_march_reaches_its_derived_budget() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = seeds_at(&[(32, 32), (10, 50), (55, 8)]);
        let mut rays = Vec::new();
        for y in [1.5_f32, 16.5, 31.5, 47.5, 62.5] {
            rays.push(ray((1.5, y), (62.5, y)));
            rays.push(ray((1.5, 1.5), (62.5, y)));
        }
        let budget = derived_budget(&rays);
        let hits = target.march(&seeds, &rays).expect("the march runs");

        for (index, hit) in hits.iter().enumerate() {
            assert_ne!(
                hit.verdict(),
                MarchVerdict::Exhausted,
                "ray {index} used its whole budget of {budget}"
            );
            assert!(
                hit.steps() < budget,
                "ray {index} took {} steps of a budget of {budget}",
                hit.steps()
            );
        }
    }

    /// **§3(d)'s decision, exercised: a march that runs out says so, and that is
    /// not visible.**
    ///
    /// A budget of one step cannot cross a 58-texel field, so every ray runs out.
    /// The verdict must be `Exhausted` and `is_visible` must be false — the
    /// second is the part that matters, because a march reporting a visibility it
    /// never checked would be a failure outward.
    #[test]
    fn a_march_that_runs_out_of_steps_is_not_visible() {
        let Some(target) = target_or_skip() else {
            return;
        };
        // **Alongside the wall, not across open ground.** The first version of
        // this test put a lone seed in a far corner and marched across the
        // middle: the field then said "nothing within 86 texels", the march
        // cleared a 58-texel crossing in *one* step, and a budget of one was
        // enough. That was the march being right and the test being wrong. A ray
        // running parallel to a wall a couple of texels away takes a step of
        // about one texel each time, so a budget of one cannot finish it.
        let seeds = thin_wall(32);
        let rays = [
            ray((30.5, 1.0), (30.5, 63.0)),
            ray((29.5, 1.0), (29.5, 63.0)),
        ];
        let hits = target
            .march_within(&seeds, &rays, 1)
            .expect("the march runs");

        for (index, hit) in hits.iter().enumerate() {
            assert_eq!(
                hit.verdict(),
                MarchVerdict::Exhausted,
                "ray {index} finished a 62-texel crawl alongside a wall in one step"
            );
            assert!(
                !hit.is_visible(),
                "ray {index} ran out of steps and still claimed to be visible"
            );
            assert_eq!(hit.steps(), 1);
        }
    }

    /// A field with no occluders lets every ray through.
    #[test]
    fn an_empty_field_blocks_nothing() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = Seeds::new(SIDE, SIDE).expect("a legal size");
        let rays = [
            ray((1.0, 1.0), (60.0, 60.0)),
            ray((30.0, 0.0), (30.0, 64.0)),
        ];
        let hits = target.march(&seeds, &rays).expect("the march runs");

        for hit in &hits {
            assert_eq!(hit.verdict(), MarchVerdict::Visible);
            assert_eq!(hit.steps(), 1, "an empty field should answer in one look");
        }
    }

    /// An empty ray list is not a GPU call.
    #[test]
    fn marching_no_rays_answers_without_a_field() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let seeds = thin_wall(32);
        assert!(
            target
                .march(&seeds, &[])
                .expect("no rays is legal")
                .is_empty()
        );
    }

    /// The GPU runs what the CPU model says it runs.
    ///
    /// Not one of the four oracles: it says the kernel and the model agree, which
    /// is what lets a failure above be read as being about the algorithm rather
    /// than about the GPU.
    #[test]
    fn the_kernel_agrees_with_the_cpu_model() {
        let Some(target) = target_or_skip() else {
            return;
        };
        for seeds in [thin_wall(32), seeds_at(&[(32, 32), (10, 50)])] {
            let mut rays = Vec::new();
            for y in [2.5_f32, 20.5, 40.5, 61.5] {
                rays.push(ray((1.5, y), (62.5, y)));
                rays.push(ray((1.5, 1.5), (62.5, y)));
            }
            let budget = derived_budget(&rays);
            let hits = target.march(&seeds, &rays).expect("the march runs");

            for (index, hit) in hits.iter().enumerate() {
                let (verdict, travelled, steps) = cpu_march(&seeds, &rays[index], budget);
                assert_eq!(
                    hit.verdict(),
                    verdict,
                    "ray {index}: the GPU said {:?} and the model said {verdict:?}",
                    hit.verdict()
                );
                assert_eq!(hit.steps(), steps, "ray {index}: step counts differ");
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a fixed-point distance below 64 * 256 * 1.5 is exact in an f64"
                )]
                let modelled = travelled as f64 / 256.0;
                assert!(
                    (f64::from(hit.distance()) - modelled).abs() < 1.0 / 256.0,
                    "ray {index}: distances differ"
                );
            }
        }
    }
}
