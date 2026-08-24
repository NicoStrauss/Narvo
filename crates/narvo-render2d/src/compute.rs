//! Compute passes over fields, and the chain that runs several of them.
//!
//! Internal, like [`crate::quad`]. This is the crate's **first** compute
//! pipeline — the M8.2 census counted zero in the tree, by grep over every `.rs`
//! and `.wgsl` file for `create_compute_pipeline`, `begin_compute_pass` and
//! `@compute`, and found none.
//!
//! # What it is for, and nothing else
//!
//! ADR-0039 fixed a frame at two draw batches in one render pass. That carries no
//! lighting: jump flooding needs log2(n) passes, a cascade needs one per level,
//! and merging and composition need their own. Four tasks consume what is here
//! and each of them is named beside the thing it consumes:
//!
//! | what | who consumes it |
//! |---|---|
//! | [`FieldKernel`] — a WGSL entry point compiled to a compute pipeline | M8.3, M8.4, M8.5, M8.6 |
//! | [`FieldKernel::run`] — n passes in one encoder, in order | M8.3's log2(n) jump-flooding passes; M8.5's cascade levels |
//! | the per-pass `step` in the uniform | M8.3's halving jump distance |
//! | [`crate::field::FieldPair`]'s ping-pong | M8.3 and M8.5, both of which read the previous pass |
//!
//! **A general render graph was weighed and rejected** (v1.51). Its best
//! argument is that 3D will need one anyway and touching this twice costs more
//! than building it once; against that, nothing here would exercise its
//! generality, so it would be measured against a toy. It reopens at a third
//! consumer with a different pass structure — in practice the first 3D slice.
//!
//! # The rule this file is written under
//!
//! **A merge may not be written order-dependently.** M8.2 measured a reduction
//! over atomics, a reduction over workgroup-shared memory with a barrier, and an
//! order-independent control, eight times each, on eight adapter/backend pairs,
//! in both profiles — 32 cells. The atomic reduction was **not reproducible in
//! any of the 32**, producing four to eight distinct sums from eight identical
//! dispatches, on discrete AMD, integrated AMD, WARP and lavapipe alike. The
//! other two were reproducible in all 32 *and returned one value across every
//! backend, adapter, platform and profile*.
//!
//! So: nothing in this crate may make a value depend on the order invocations
//! run in. In practice that means no atomic accumulation, and a merge writes its
//! result to a slot indexed by the work item rather than folding into a shared
//! one. The kernel here obeys it by construction — one texel per invocation, no
//! shared memory, no barrier, no atomic.

use crate::error::RenderError;
use crate::field::{FIELD_FORMAT, FieldPair};

/// Invocations per workgroup, one dimension.
///
/// Eight by eight is 64 invocations. `max_compute_invocations_per_workgroup`
/// came back as 256 on all eight adapter/backend pairs the M8.2 probe measured,
/// so this is a quarter of the smallest limit seen rather than a number near one.
/// It is `pub(crate)` so the dispatch arithmetic and the shader's
/// `@workgroup_size` can be held together by a test.
pub(crate) const WORKGROUP_SIDE: u32 = 8;

/// The transport kernel's source, and the only compute shader M8.2 ships.
pub(crate) const TRANSPORT_WGSL: &str = include_str!("shaders/transport.wgsl");

/// The entry point in [`TRANSPORT_WGSL`].
pub(crate) const TRANSPORT_ENTRY: &str = "transport";

/// What one pass is told about itself.
///
/// Sixteen bytes, matching `Params` in the shader. Written out as four `u32`
/// rather than derived from a `repr(C)` struct, because this crate carries no
/// `bytemuck` and four fields do not justify one — the same reasoning
/// `quad::vertex_bytes` already uses for the quad's corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PassParams {
    /// What this pass adds. M8.3's jump distance.
    pub(crate) step: u32,
    /// The field's width in texels.
    pub(crate) width: u32,
    /// The field's height in texels.
    pub(crate) height: u32,
}

impl PassParams {
    /// The sixteen bytes the uniform buffer holds.
    fn bytes(self) -> [u8; 16] {
        let mut bytes = [0_u8; 16];
        bytes[0..4].copy_from_slice(&self.step.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.width.to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.height.to_ne_bytes());
        // The fourth word is the shader's named padding and stays zero.
        bytes
    }
}

/// One WGSL compute entry point, compiled, with the bind group layout it needs.
///
/// Built eagerly so a broken shader fails where it is created rather than on the
/// first dispatch — the same reason `OffscreenTarget` builds its
/// [`QuadPipeline`](crate::quad::QuadPipeline) in its constructor.
#[derive(Debug)]
pub(crate) struct FieldKernel {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl FieldKernel {
    /// Compiles `entry` out of `source` against the field bindings.
    ///
    /// The layout is spelled out rather than derived from the module. wgpu will
    /// derive one, and the M8.2 probe used exactly that to find out which storage
    /// formats a backend refuses — but a derived layout is a layout nothing in
    /// this crate states, so a shader that quietly stopped binding the source
    /// texture would still compile.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        source: &str,
        entry: &str,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{label} bindings")),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        // **Not filterable, and that is a measurement.** A
                        // 32-bit float texture is filterable only with
                        // `FLOAT32_FILTERABLE`, and `gpu::create_device` requests
                        // no features at all. Nothing here samples: the kernel
                        // reads with `textureLoad`, which fetches a texel and
                        // interpolates nothing, so the feature would buy a
                        // capability no consumer named.
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        // Write only. `ReadWrite` exists and every adapter the
                        // probe measured reported `STORAGE_READ_WRITE` for this
                        // format — and it is not used, because a pass that could
                        // read its own output is the defect the ping-pong is
                        // there to make unrepresentable.
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: FIELD_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(&format!("{label} layout")),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some(entry),
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

    /// Runs one pass per entry of `steps`, in that order, over `pair`.
    ///
    /// Returns how many passes were encoded, which is `steps.len()`. The count is
    /// returned rather than assumed because it is the thing a caller checks: a
    /// chain that silently ran fewer passes than it was asked for is the failure
    /// mode a `()` return would hide.
    ///
    /// # How the passes are ordered
    ///
    /// One encoder, one `begin_compute_pass` per step, each dropped before the
    /// next begins. Ordering is the encoder's and needs no barrier of this
    /// crate's own: wgpu inserts the memory barriers between passes that touch
    /// the same resource, which is what makes the ping-pong safe without this
    /// file computing dependencies. That is also the half a general render graph
    /// would have rebuilt by hand, and the reason not to.
    ///
    /// The swap happens **between** passes, on the CPU, while the encoder is
    /// being recorded — so pass *k* reads what pass *k-1* wrote and writes the
    /// field pass *k-1* read.
    ///
    /// An empty `steps` encodes nothing, submits nothing and leaves `pair`
    /// untouched, including its orientation. A chain of zero passes has to be
    /// indistinguishable from no chain at all, or "the lighting is off" and "the
    /// lighting ran and did nothing" become the same observation.
    ///
    /// # Errors
    ///
    /// Nothing here fails today; the `Result` is what [`Self::run`]'s callers
    /// already thread and what a dispatch limit check will use.
    pub(crate) fn run(&self, pair: &mut FieldPair, steps: &[u32]) -> Result<u32, RenderError> {
        if steps.is_empty() {
            return Ok(0);
        }

        let width = pair.width();
        let height = pair.height();
        let groups_x = width.div_ceil(WORKGROUP_SIDE);
        let groups_y = height.div_ceil(WORKGROUP_SIDE);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("narvo field chain"),
            });

        let mut encoded = 0_u32;
        for (index, step) in steps.iter().enumerate() {
            let params = PassParams {
                step: *step,
                width,
                height,
            };
            let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("narvo field pass params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&uniform, 0, &params.bytes());

            // Built here rather than hoisted: the two views change every pass,
            // which is the whole of the ping-pong. `read()` and `write()` cannot
            // name one field — that is arithmetic over an index rather than a
            // convention, and `field.rs` holds it with a test.
            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("narvo field pass"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(pair.read().view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(pair.write().view()),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            });

            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("narvo field pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(groups_x, groups_y, 1);
            }

            pair.swap();
            encoded += 1;
            debug_assert_eq!(
                encoded as usize,
                index + 1,
                "a pass was encoded without being counted"
            );
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        Ok(encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::{TRANSPORT_ENTRY, TRANSPORT_WGSL, WORKGROUP_SIDE};
    use crate::field::pattern;
    use crate::{OffscreenTarget, RenderError};

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

    /// The shader's `@workgroup_size` and the dispatch arithmetic agree.
    ///
    /// Two numbers in two languages that have to be one number. Nothing else
    /// would notice them parting: a shader declaring `@workgroup_size(16, 16)`
    /// against a dispatch computed for eight would simply run four times as many
    /// invocations, every one of them writing the texel it owns, and the picture
    /// would be right.
    #[test]
    fn the_dispatch_and_the_shader_agree_on_the_workgroup() {
        let declared = format!("@workgroup_size({WORKGROUP_SIDE}, {WORKGROUP_SIDE})");
        assert!(
            TRANSPORT_WGSL.contains(&declared),
            "the shader does not declare `{declared}`, which is what the dispatch \
             in `FieldKernel::run` divides by"
        );
        assert!(
            TRANSPORT_WGSL.contains(&format!("fn {TRANSPORT_ENTRY}(")),
            "the shader has no `{TRANSPORT_ENTRY}` entry point"
        );
    }

    /// The kernel writes only `+`, which is §3's reproducible subset.
    ///
    /// A source read rather than a behaviour, and it is the only kind of check
    /// there is for this: a contracted `a*b+c` produces a *plausible* number, so
    /// no comparison of outputs can report it. M8.0 measured DX12 contracting 928
    /// of 4096 such expressions inside one run.
    #[test]
    fn the_kernel_stays_inside_the_reproducible_subset() {
        let body: String = TRANSPORT_WGSL
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        for forbidden in ["*", "/", "sqrt", "exp", "log", "pow", "sin", "cos", "fma"] {
            assert!(
                !body.contains(forbidden),
                "the transport kernel contains `{forbidden}`, which is outside the \
                 subset M8.0 measured as reproducible — see this file's header"
            );
        }
        assert!(
            !body.contains("atomic") && !body.contains("workgroupBarrier"),
            "the transport kernel reaches for order-dependent arithmetic, which \
             M8.2 measured as irreproducible in 32 of 32 cells"
        );
    }

    /// A chain of no passes encodes nothing and moves nothing.
    #[test]
    fn a_chain_of_no_passes_is_indistinguishable_from_no_chain() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let kernel = target.transport_kernel();
        let mut pair = target
            .field_pair(4, 4, "narvo empty chain")
            .expect("a pair");
        let seeded = pattern(4, 4);
        pair.read().write(&seeded).expect("the seed fits");

        let passes = kernel.run(&mut pair, &[]).expect("an empty chain runs");

        assert_eq!(passes, 0, "an empty chain claimed to have encoded a pass");
        assert_eq!(
            pair.read().read_back().expect("the read-back succeeds"),
            seeded,
            "an empty chain moved the field it was handed"
        );
    }

    /// **§2's oracle: a pattern no drawing could make, through a chain of passes,
    /// unchanged where it must be and moved exactly where it was told to be.**
    ///
    /// The pattern goes in with `write_texture` and never touches the draw path.
    /// Its z and w channels are negative, which nothing this crate renders can
    /// produce — the targets are `…UnormSrgb`, premultiplied `OVER` stays inside
    /// `0.0..=1.0` (ADR-0023), and a nearest sampler returns a texel rather than
    /// inventing one. So if the pattern comes back out, the chain *moved* it.
    ///
    /// # What each channel decides
    ///
    /// - **y doubles per pass**, so `y0 * 2^n` separates three passes from one.
    ///   A chain in which every pass read the *original* field rather than the
    ///   previous pass's output gives `y0 * 2` and fails here. That is the
    ///   ping-pong defect, and it is this assertion that catches it.
    /// - **x is order-sensitive**: `x0 * 2^n + Σ s_i * 2^(n-i)`. The steps below
    ///   are 1, 2, 4, so a chain running them in any other order lands on a
    ///   different number. A sum could not tell those apart.
    /// - **z and w must not move at all.** A pass that invents a texel — because
    ///   it read a field nobody wrote, say — moves them.
    ///
    /// Every intermediate is an exact integer below 2^24, so contraction cannot
    /// change the arithmetic (this module's header, and the shader's).
    #[test]
    fn a_pattern_survives_a_chain_of_passes_and_is_moved_only_as_told() {
        let Some(target) = target_or_skip() else {
            return;
        };
        // Nine by five: neither dimension is a multiple of the workgroup side, so
        // the dispatch rounds up and the last workgroup runs partly past the
        // edge. A size of 8 x 8 would have hidden a missing bounds check.
        let (width, height) = (9, 5);
        let mut pair = target
            .field_pair(width, height, "narvo oracle")
            .expect("a 9 x 5 pair");
        let seeded = pattern(width, height);
        pair.read().write(&seeded).expect("the seed fits");

        let steps = [1_u32, 2, 4];
        let passes = target
            .transport_kernel()
            .run(&mut pair, &steps)
            .expect("the chain runs");
        assert_eq!(
            passes,
            u32::try_from(steps.len()).expect("three fits"),
            "the chain encoded a different number of passes than it was given"
        );

        let out = pair.read().read_back().expect("the read-back succeeds");
        assert_eq!(out.len(), seeded.len());

        // x0 * 8 + (1 * 4 + 2 * 2 + 4 * 1), computed on the CPU in the same
        // order the passes apply it. All integers, all exact.
        let expected_tail = 1.0 * 4.0 + 2.0 * 2.0 + 4.0 * 1.0;
        for (index, texel) in out.chunks_exact(4).enumerate() {
            let x0 = seeded[index * 4];
            let y0 = seeded[index * 4 + 1];
            let x = index as u32 % width;
            let y = index as u32 / width;

            assert_eq!(
                texel[0],
                x0 * 8.0 + expected_tail,
                "texel ({x}, {y}) took the three steps in a different order, or a \
                 different number of them"
            );
            assert_eq!(
                texel[1],
                y0 * 8.0,
                "texel ({x}, {y}) did not pass through three chained passes — a \
                 value of {y0}*2 means every pass read the field the first one \
                 read instead of the one before it",
            );
            assert_eq!(
                texel[2], -1.0,
                "texel ({x}, {y}) had a channel changed that no pass writes, so \
                 something was rendered into the field rather than moved through it"
            );
            assert_eq!(
                texel[3], -2.0,
                "texel ({x}, {y}) had a channel changed that no pass writes"
            );
        }
    }

    /// One pass is the same arithmetic as one step of the chain.
    ///
    /// The chain's own base case, and it is what says the multi-pass result above
    /// is not an artefact of a chain being long: a single pass moves x by
    /// `x0 + x0 + step` and y by `y0 + y0`, exactly.
    #[test]
    fn one_pass_moves_the_field_by_exactly_one_step() {
        let Some(target) = target_or_skip() else {
            return;
        };
        let mut pair = target.field_pair(4, 4, "narvo one pass").expect("a pair");
        let seeded = pattern(4, 4);
        pair.read().write(&seeded).expect("the seed fits");

        let passes = target
            .transport_kernel()
            .run(&mut pair, &[5])
            .expect("one pass runs");
        assert_eq!(passes, 1);

        let out = pair.read().read_back().expect("the read-back succeeds");
        for (index, texel) in out.chunks_exact(4).enumerate() {
            assert_eq!(texel[0], seeded[index * 4] + seeded[index * 4] + 5.0);
            assert_eq!(texel[1], seeded[index * 4 + 1] + seeded[index * 4 + 1]);
        }
    }
}
