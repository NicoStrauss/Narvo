//! The pipeline that draws one texture across a screen-filling quad.
//!
//! Internal: nothing here is part of the crate's public API, and none of these
//! `wgpu` types leave the crate.

use crate::sprite::SpriteFilter;
use wgpu::util::DeviceExt as _;

/// The quad's corners as `[x, y, u, v]`.
///
/// Normalised device coordinates have y pointing up while the framebuffer has y
/// pointing down, so the corner at `y = +1` is the *top* row of the output. The
/// texture coordinates use the opposite convention, origin at the top left.
/// Getting either of those backwards flips the image, which is precisely what
/// the quadrant test is built to catch.
///
/// `pub(crate)` only so that the convention guard in `sprite.rs` can read it.
/// Nothing outside this module uses it to draw.
pub(crate) const VERTICES: [[f32; 4]; 4] = [
    [-1.0, 1.0, 0.0, 0.0],  // top left
    [-1.0, -1.0, 0.0, 1.0], // bottom left
    [1.0, -1.0, 1.0, 1.0],  // bottom right
    [1.0, 1.0, 1.0, 0.0],   // top right
];

/// How a fragment is combined with what the target already holds.
///
/// **Premultiplied alpha, and one pipeline for everything** — ADR-0023. Written
/// out rather than named as `wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING`
/// because both halves are decisions this project owes an account of, and a
/// constant's name is not one. That the spelled-out state *is* that constant is
/// asserted by `the_blend_state_is_the_standard_premultiplied_over`, so the two
/// cannot drift apart and the equality stays a checked fact.
///
/// # Colour: `src + dst * (1 - src.a)`
///
/// The source term carries no factor because a premultiplied source has already
/// been multiplied by its own alpha — that is what premultiplied means, and it
/// is what the glyph atlas already stores (`narvo_testkit::glyph_atlas` writes
/// coverage into all four channels, which is white already multiplied through).
///
/// # Alpha: the same `OVER`, and why not something cheaper
///
/// `a_out = a_src + a_dst * (1 - a_src)`. Two alternatives were available and
/// both are wrong here:
///
/// - **`One, Zero`** (the source's alpha replaces the target's) would let a
///   half-transparent sprite punch a half-transparent hole in an opaque scene.
///   The read-back would then carry an alpha nobody composited for.
/// - **`Zero, One`** (keep the target's alpha) would be indistinguishable from
///   the above on an opaque background and wrong the moment anything renders
///   onto a transparent one.
///
/// `OVER` is the only one of the three that keeps the result **premultiplied**,
/// which is the invariant the next draw depends on: it holds `rgb <= a` when the
/// source holds it, so a target can be blended into again and again without ever
/// producing a colour brighter than its own coverage. That property is what
/// makes "one pipeline" possible at all — every draw both consumes and produces
/// the same representation.
///
/// # Where the arithmetic happens, which is not where it looks like it happens
///
/// The render target is `Rgba8UnormSrgb`, so the hardware decodes the target's
/// stored bytes to linear light, blends there, and re-encodes on write. The
/// blend is therefore **not** `(dst * (255 - a)) / 255` in stored bytes; it is
/// `D(src) + D(dst) * (1 - a)` put back through the encoder. Alpha is not
/// sRGB-encoded and does pass through as plain arithmetic. `blend_proof`'s
/// probes measure both readings and report which one the hardware follows.
const BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Two triangles over the four corners. Culling is off, so the winding order
/// carries no meaning here.
const INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

/// Number of indices to draw.
const INDEX_COUNT: u32 = INDICES.len() as u32;

/// Vertex data as bytes, without a `bytemuck`-style cast.
///
/// Four vertices do not justify a dependency, and `to_ne_bytes` keeps the crate
/// free of `unsafe` - which the workspace lints deny outright. Real geometry
/// will want a proper cast; this is not it yet.
///
/// **The identity tint is appended here rather than written into [`VERTICES`].**
/// Since M6b.3 the pipeline's vertex is eight floats, and the screen-filling
/// quad needs the last four as much as a sprite does. Putting them in the table
/// would have moved the corner literals that the convention guard in `sprite.rs`
/// reads and that ADR-0004 pins; appending them here leaves that table exactly
/// as M1 wrote it, so the guard still compares four-component rows and this
/// path's blessed reference has no literal in it that moved.
fn vertex_bytes() -> Vec<u8> {
    VERTICES
        .iter()
        .flat_map(|corner| {
            corner
                .iter()
                .copied()
                .chain(crate::sprite::SpriteTint::UNTINTED.premultiplied())
        })
        .flat_map(f32::to_ne_bytes)
        .collect()
}

/// Index data as bytes, for the same reason.
fn index_bytes() -> Vec<u8> {
    INDICES
        .iter()
        .flat_map(|index| index.to_ne_bytes())
        .collect()
}

/// One uploaded texture, bound once per sampler.
///
/// Two bind groups over **one** texture: they differ in binding 1 alone, so a
/// batch that mixes filters costs no second upload — which is what makes D15's
/// per-run sampler cheap.
///
/// Both are built on every call, unconditionally: `bind_texture` never sees the
/// sprites, so it cannot know which filters a batch wants. An all-`Nearest`
/// caller therefore builds one bind group it discards — `present_textured_quad`
/// is one, once per presented frame. The window's *sprite* path is not: since
/// M3.32 it binds per run like the offscreen one, and the scene it draws uses
/// both filters. The trade is that no caller has to know its batch's filters.
pub(crate) struct TextureBindings {
    /// Indexed by [`SpriteFilter::index`].
    per_filter: [wgpu::BindGroup; 2],
}

impl TextureBindings {
    /// The bind group that samples this texture the way `filter` asks.
    pub(crate) fn for_filter(&self, filter: SpriteFilter) -> &wgpu::BindGroup {
        &self.per_filter[filter.index()]
    }
}

/// [`OffscreenTarget`]: crate::OffscreenTarget
#[derive(Debug)]
pub(crate) struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    /// One per [`SpriteFilter`], indexed by [`SpriteFilter::index`].
    samplers: [wgpu::Sampler; 2],
}

impl QuadPipeline {
    /// Compiles the shader and builds the pipeline for `target_format`, drawing
    /// at [`crate::SAMPLE_COUNT`] samples per pixel.
    pub(crate) fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("narvo quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/quad.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("narvo quad bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // One sampler per `SpriteFilter`, in that enum's own order, built once
        // and kept for the pipeline's life. Two samplers and **one pipeline**:
        // the filter is a field of `SamplerDescriptor` and the sampler reaches
        // the shader as a bind-group entry (binding 1 above), not as pipeline
        // state — so nothing about the pipeline or the layout depends on it.
        //
        // The layout needed no change for the second sampler, and that is
        // checked rather than inherited from M3.18's report: binding 1 is
        // `SamplerBindingType::Filtering` (not `NonFiltering`) and binding 0 is
        // `Float { filterable: true }`, which is exactly the pair WebGPU
        // requires of a bind group that may carry a filtering sampler. A
        // `Nearest` sampler is admissible under `Filtering` too, so one layout
        // serves both.
        let samplers = [
            Self::sampler(device, "nearest", wgpu::FilterMode::Nearest),
            Self::sampler(device, "linear", wgpu::FilterMode::Linear),
        ];

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("narvo quad pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            // wgpu 30 calls push constants "immediate data"; the quad uses none.
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("narvo quad pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                // `[x, y, u, v, r, g, b, a]`, thirty-two bytes. The last four
                // are the premultiplied tint, the same value on all four
                // corners of a quad — a per-sprite property carried per vertex,
                // which is what keeps a batch of differently tinted sprites a
                // single draw call. A uniform would have been sixteen bytes
                // instead of sixty-four per sprite and would have made the tint
                // a second thing `batch_runs` cuts on; D15's cut would then have
                // become a colour policy.
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: size_of::<[f32; 2]>() as wgpu::BufferAddress,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: size_of::<[f32; 4]>() as wgpu::BufferAddress,
                            shader_location: 2,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(BLEND),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                // No culling: a screen-filling quad has no meaningful facing,
                // and leaving it on would make the winding order a silent trap.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: crate::SAMPLE_COUNT,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("narvo quad vertices"),
                contents: &vertex_bytes(),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("narvo quad indices"),
                contents: &index_bytes(),
                usage: wgpu::BufferUsages::INDEX,
            }),
            bind_group_layout,
            samplers,
        }
    }

    /// Records the one draw call this pipeline exists for.
    ///
    /// Shared verbatim by the offscreen target and the window target. Both clear
    /// the attachment and draw the quad over it; only which view they draw into,
    /// and what they do with the result, differ. Keeping the draw here is what
    /// stops the window from becoming a second renderer that drifts away from
    /// the one the tests cover.
    pub(crate) fn encode_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        resolve_target: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
    ) {
        self.encode_pass_with(
            encoder,
            view,
            resolve_target,
            bind_group,
            &self.vertices,
            &self.indices,
            wgpu::IndexFormat::Uint16,
            INDEX_COUNT,
        );
    }

    /// A vertex buffer holding a batch's `[x, y, u, v, r, g, b, a]` corners.
    ///
    /// Built per draw rather than by overwriting [`Self::vertices`]: the
    /// screen-filling quad and the sprite batch would otherwise share one
    /// buffer, and the full-screen path — which the M1 golden image pins — would
    /// start depending on what was drawn before it.
    ///
    /// Rebuilt on every call rather than kept and refilled. A persistent buffer
    /// needs `COPY_DST`, a capacity policy and a decision about what happens
    /// when a batch outgrows it; none of that changes a pixel, and all of it
    /// belongs with the throughput measurement rather than with the change that
    /// has to leave two blessed images untouched.
    pub(crate) fn corner_buffer(
        &self,
        device: &wgpu::Device,
        corners: &[[f32; 8]],
    ) -> wgpu::Buffer {
        let bytes: Vec<u8> = corners
            .iter()
            .flatten()
            .flat_map(|component| component.to_ne_bytes())
            .collect();

        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("narvo sprite batch vertices"),
            contents: &bytes,
            usage: wgpu::BufferUsages::VERTEX,
        })
    }

    /// An index buffer addressing `count` quads, six indices each.
    ///
    /// `Uint32` rather than the `Uint16` the single screen-filling quad uses.
    /// Sixteen-bit indices would cap a batch at 16 384 sprites — below the
    /// 50 000 of `ProjektPlan.md` §6/M3 — and a cap that low would be a
    /// throughput decision smuggled in as an index format.
    ///
    /// The winding repeats `[0, 1, 2, 0, 2, 3]` per quad, offset by `4 * i`,
    /// which is the same pair of triangles the single quad has always drawn:
    /// the diagonal runs from each sprite's top-left corner to its bottom-right.
    pub(crate) fn batch_index_buffer(&self, device: &wgpu::Device, count: usize) -> wgpu::Buffer {
        let mut bytes = Vec::with_capacity(count * INDICES.len() * size_of::<u32>());

        for sprite in 0..count {
            let base = u32::try_from(sprite * 4).expect("the batch cap keeps this inside u32");
            for index in INDICES {
                bytes.extend_from_slice(&(base + u32::from(index)).to_ne_bytes());
            }
        }

        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("narvo sprite batch indices"),
            contents: &bytes,
            usage: wgpu::BufferUsages::INDEX,
        })
    }

    /// The same draw call, over caller-supplied buffers.
    ///
    /// One `draw_indexed` whatever `index_count` is: a batch of a thousand
    /// sprites is one call, which is the point of batching.
    #[expect(
        clippy::too_many_arguments,
        reason = "every argument is a distinct GPU resource with no sensible grouping; \
                  a struct here would be a parameter list with extra steps"
    )]
    pub(crate) fn encode_pass_with(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        resolve_target: &wgpu::TextureView,
        bind_group: &wgpu::BindGroup,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        index_format: wgpu::IndexFormat,
        index_count: u32,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("narvo quad pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                // `view` is the multisampled attachment and `resolve_target` the
                // single-sample texture the rest of the crate reads. A set
                // resolve target "is always written to, regardless of how
                // [`Self::ops`] is configured"
                // (wgpu-30.0.0/src/api/render_pass.rs:626-628), so a pass that
                // draws nothing still resolves - which is what lets the
                // clear-only path share this shape. That the write happens at
                // end of pass rather than earlier is the WebGPU spec's resolve
                // step and is not sourced here: uncertain, and nothing depends
                // on it.
                view,
                depth_slice: None,
                resolve_target: Some(resolve_target),
                ops: wgpu::Operations {
                    // Opaque black underneath. The screen-filling quad covers
                    // the target so this never shows there - unless the draw
                    // silently does nothing, and then it shows very clearly
                    // indeed. A sprite covers only part of it, and the black is
                    // then the background the sprite sits on.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        // An empty batch clears and draws nothing. The early return is not an
        // optimisation: `Buffer::slice` panics on a zero-length buffer inside
        // wgpu, so binding the empty buffers would take the process down on a
        // world that simply has no sprites in it yet.
        if index_count == 0 {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), index_format);
        pass.draw_indexed(0..index_count, 0, 0..1);
    }

    /// The same pass, drawing one call per run of the batch.
    ///
    /// **This is where D15 lands.** The runs come from `batch_runs`, which cuts
    /// the depth-ordered sequence wherever the sampler wish changes; each is a
    /// half-open range of *sprites*, and six indices per sprite turns it into a
    /// range of the shared index buffer. One pass, so the clear happens once;
    /// one `draw_indexed` per run, issued **in run order**, so the drawing order
    /// across the cuts is the drawing order within them.
    ///
    /// With one filter in the world there is one run, and this issues exactly
    /// the single draw call the batch path issued before D15.
    pub(crate) fn encode_runs(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        resolve_target: &wgpu::TextureView,
        vertices: &wgpu::Buffer,
        indices: &wgpu::Buffer,
        runs: &[(std::ops::Range<usize>, &wgpu::BindGroup)],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("narvo quad pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: Some(resolve_target),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });

        // No runs means no sprites: clear and draw nothing. Binding the empty
        // buffers would take the process down on a world with no sprites yet —
        // not in `Buffer::slice`, which accepts a zero-length range, but one
        // call later in `set_vertex_buffer`, whose `size_expect_nonzero`
        // (`wgpu-30.0.0/src/api/render_pass.rs:144` into
        // `src/api/buffer.rs:683`) expects a non-empty slice.
        if runs.is_empty() {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);

        // The bind group is the one thing that is *not* hoisted: it carries the
        // run's sampler, which is what a run is cut on. Pipeline, vertex and
        // index buffer are shared by every run and stay above the loop.
        for (run, bind_group) in runs {
            pass.set_bind_group(0, *bind_group, &[]);
            let first = u32::try_from(run.start * 6)
                .expect("MAX_SPRITES_PER_BATCH keeps six indices per sprite inside u32");
            let last = u32::try_from(run.end * 6)
                .expect("MAX_SPRITES_PER_BATCH keeps six indices per sprite inside u32");
            pass.draw_indexed(first..last, 0, 0..1);
        }
    }

    /// One sampler, built the same way for both filters.
    fn sampler(device: &wgpu::Device, name: &str, filter: wgpu::FilterMode) -> wgpu::Sampler {
        device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("narvo quad {name} sampler")),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: filter,
            min_filter: filter,
            // Nearest between mip levels, for both: there are no mip levels.
            // `bind_texture` creates every texture with `mip_level_count: 1`, so
            // this field selects between one thing and itself.
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        })
    }

    /// Uploads `rgba` as a texture and binds it once per sampler.
    ///
    /// `rgba` must already be `width * height * 4` bytes; the public entry point
    /// checks that before getting here.
    pub(crate) fn bind_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> TextureBindings {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        // Rgba8UnormSrgb, matching the render target: the sample is decoded to
        // linear and re-encoded on write, so bytes survive the round trip. A
        // linear format here would silently brighten everything.
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("narvo quad texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // `write_texture` carries no 256-byte row alignment requirement - that
        // applies to buffer-to-texture copies, not to this upload path - so the
        // rows go up tightly packed.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // One upload, two bind groups, both built every time. The texture is
        // written once above; what differs between the two is binding 1 alone.
        TextureBindings {
            per_filter: std::array::from_fn(|index| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("narvo quad bind group"),
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.samplers[index]),
                        },
                    ],
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BLEND;

    /// The spelled-out blend state and `wgpu`'s own name for it are the same
    /// thing, so the account above documents what actually runs.
    ///
    /// Written out for the account and checked against the constant for the
    /// arithmetic — neither alone would do both. If a future `wgpu` changes what
    /// `PREMULTIPLIED_ALPHA_BLENDING` means, this fails and ADR-0023 gets
    /// re-read rather than silently overtaken.
    #[test]
    fn the_blend_state_is_the_standard_premultiplied_over() {
        assert_eq!(BLEND, wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
    }

    /// Neither component may depend on the blend constant colour, which nothing
    /// in this crate ever sets.
    ///
    /// `set_blend_constant` is never called, so a factor referring to it would
    /// read whatever the default is — a silent dependency on state this crate
    /// does not manage. `wgpu` offers the predicate per component; this uses it
    /// on both.
    #[test]
    fn the_blend_state_needs_no_constant_colour() {
        assert!(!BLEND.color.uses_constant());
        assert!(!BLEND.alpha.uses_constant());
    }
}
