//! What `FilterMode::Linear` would do to the interior quantum M3.16 found.
//!
//! # The question this file exists for
//!
//! M3.16 measured that the picture moves in **two quanta at once**: the
//! silhouette in quarter pixels, because MSAA grades a partly covered edge, and
//! the interior in whole pixels, because `Nearest` takes one sample per pixel
//! and a texel boundary can only land on a pixel boundary. D14 is therefore half
//! fulfilled, and D13 — the sampler question, open since M3.9 — is what the
//! other half hangs on.
//!
//! **This file measures and decides nothing.** It repeats M3.16's movement
//! measurement with one variable changed, so the two tables can be read side by
//! side.
//!
//! # Outside the production path, and checked against it
//!
//! The shape M3.14 and M3.15 established: a test-only pipeline, because the
//! production sampler is `Nearest` and this task must not change it. The
//! pipeline is production's in every other respect — same format, same
//! `SAMPLE_COUNT` with a resolve, the same `center` qualifier, the same corner
//! table, the same clear.
//!
//! **The replication is checked live rather than assumed**, the way M3.15a
//! rebuilt that control: the same pipeline set to `Nearest` must come out
//! byte-identical to `OffscreenTarget::render_sprites_viewed_by`, in the same
//! run and on an adapter this file checks. If it does not, the `Linear` numbers
//! are about this file rather than about the sampler, and the measurement says
//! so instead of reporting them.
//!
//! # Measuring an interior that no longer has an edge
//!
//! M3.16 read the interior as "the first row that is not the upper colour",
//! which is a whole number by construction and exactly the right measure for
//! `Nearest`. Under `Linear` there is no such row: the boundary becomes a ramp
//! about one texel wide. So both are read the same, sub-pixel way instead — the
//! position where the value crosses the **midpoint** of the two texel colours,
//! found by interpolating between the two rows that bracket it.
//!
//! In linear light, because the sampler blends there: the texture is
//! `Rgba8UnormSrgb`, so a sampled texel is decoded before the blend and the
//! result is encoded on write. Reading the midpoint from stored bytes would find
//! the midpoint of the *encoded* ramp, which is a different place.
//!
//! That measure reproduces M3.16's answer under `Nearest`. A step from 255 to
//! 128 between rows 63 and 64 puts the crossing at 63.5, and it moves to 64.5 —
//! one whole pixel, the same one step per pixel of travel M3.16 reports.
//!
//! # Predictions, written before the measurement ran
//!
//! 1. **The silhouette is unchanged**: 0.25 px, four steps per pixel. MSAA
//!    grades geometry coverage, which is not the sampler's business.
//! 2. **The interior becomes continuous** at the resolution this sweep can see.
//!    A camera shift of `d` pixels moves the blend weight by `d / M`, where `M`
//!    is the magnification — 4 pixels per texel here — so the value at a fixed
//!    pixel changes by about `(c1 - c0) / M` per pixel of shift. With a contrast
//!    of 127 counts that is about 32 counts per pixel, so an eighth-pixel step
//!    moves the ramp by about 4 counts and is resolvable.
//! 3. **The interior's resolution therefore depends on magnification and on
//!    contrast, not on the pixel grid.** Coarser magnification spreads the same
//!    contrast over fewer pixels and makes each step *larger*, not smaller; less
//!    contrast makes the crossing harder to locate. Both are properties of the
//!    content, which is the substantive difference from `Nearest`, whose quantum
//!    is one pixel whatever the content does.
//!
//! If prediction 2 fails and the interior still steps a whole pixel, `Linear`
//! does not solve what D14 left open, and that is a complete result.
//!
//! # What happened: 2 and 3 held, 1 did not, and 1's failure is a measurement
//!
//! **Prediction 1 was wrong as written, and about the measure rather than the
//! renderer.** Read on sprite B's left edge it gave nine distinct positions under
//! `Linear` instead of five — but that is not the silhouette moving more finely.
//! B's leftmost column samples atlas texel coordinate 8.125, bilinear puts
//! weight 0.375 on texel 7, and texel 7 belongs to another sprite's region: the
//! column reads 0.625 green where geometry says 1.0, so the measure reports the
//! edge at 48.375 with the camera at rest. Measured 48.376, the residue being
//! the 8-bit read-back.
//!
//! That is the bleeding M3.9 computed, arriving inside the instrument. It
//! corroborates M3.9 rather than repeating it: M3.9's scene magnifies 6 pixels
//! to the texel and puts 42 % on the neighbour, this one magnifies 4 and puts
//! 37.5 % — the same rule at two magnifications.
//!
//! So the silhouette is read on **sprite A's** left edge instead, where
//! `ClampToEdge` makes the blend a no-op and geometry is all that is left. B's
//! edge is still measured and still reported, as the bleed's own number.
//!
//! **Predictions 2 and 3 held**, and the interior column is the result this file
//! was written for.

use narvo_render2d::{
    CameraView, OffscreenTarget, Pixels, Projection, RenderError, SpriteInstance, SpritePlacement,
    TextureRegion,
};
use wgpu::util::DeviceExt as _;

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The render target's edge, and the atlas's, matching M3.16 exactly.
const TARGET: u32 = 128;
const ATLAS: u32 = 16;

/// Production's format and sample count. Not this file's to choose.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SAMPLES: u32 = narvo_render2d::SAMPLE_COUNT;

/// `sprite.rs`'s `SPRITE_CORNERS`, replicated as `[x, y, u, v]`.
const CORNERS: [[f32; 4]; 4] = [
    [-0.5, 0.5, 0.0, 0.0],
    [-0.5, -0.5, 0.0, 1.0],
    [0.5, -0.5, 1.0, 1.0],
    [0.5, 0.5, 1.0, 0.0],
];

/// `quad.rs`'s `INDICES`.
const INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

/// `quad.wgsl`, replicated, its `center` qualifier included (D17, M3.26).
const SHADER: &str = r#"
struct Vertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(perspective, center) uv: vec2<f32>,
}
@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> Vertex {
    var out: Vertex;
    out.clip_position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}
@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@fragment
fn fs_main(input: Vertex) -> @location(0) vec4<f32> {
    return textureSample(scene_texture, scene_sampler, input.uv);
}
"#;

/// M3.16's constants, so the two tables are about the same picture.
const EDGE_ROW: u32 = 52;
const INTERIOR_COLUMN: u32 = 58;
const B_UPPER: [u8; 4] = narvo_testkit::TOP_RIGHT_UPPER;
const B_LOWER: [u8; 4] = narvo_testkit::TOP_RIGHT_LOWER;

/// Sprite A's left edge, and the row it is read on.
///
/// **The silhouette measure has to move here under `Linear`, and why is the
/// point.** M3.16 read sprite B's left edge, which works while the sampler is
/// `Nearest`: the leftmost covered column is pure green, so green over full
/// green is the geometric coverage. Under `Linear` it is not. B's region starts
/// at atlas texel 8, its leftmost column samples texel coordinate 8.125, and
/// bilinear puts weight 0.375 on texel 7 — which belongs to *another sprite's*
/// region. The column reads 0.625 green, the measure reports the edge three
/// eighths of a pixel to the right of where the geometry is, and coverage and
/// bleeding are no longer separable in that number.
///
/// A's left edge has no such problem. A's region starts at atlas texel 0, so the
/// blend partner is outside the texture and `ClampToEdge` returns texel 0 again:
/// the blend is a no-op and the column is pure red at any filter. The same holds
/// down the v axis at [`A_EDGE_ROW`], which sits inside A's uniformly red upper
/// half. So this edge measures geometry alone under both samplers, which is what
/// makes the two columns comparable.
const A_LEFT_AT_IDENTITY: f64 = 28.0;
const A_EDGE_ROW: u32 = 32;
const A_UPPER: [u8; 4] = narvo_testkit::TOP_LEFT_UPPER;

/// Pixels per texel in this scene: a 32-pixel sprite over an 8-texel region.
const PIXELS_PER_TEXEL: f64 = 4.0;

fn atlas() -> Pixels {
    narvo_testkit::AtlasLayout::UNPADDED.atlas()
}

fn scene(texture: &Pixels) -> [SpriteInstance; 3] {
    let placed = |x: f32, y: f32| SpritePlacement {
        x,
        y,
        rot_cos: 1.0,
        rot_sin: 0.0,
        scale_x: 32.0,
        scale_y: 32.0,
    };
    [
        SpriteInstance::new(
            placed(-20.0, 20.0),
            TextureRegion::from_texels(0, 0, 8, 8, texture),
        ),
        SpriteInstance::new(
            placed(0.0, 0.0),
            TextureRegion::from_texels(8, 0, 8, 8, texture),
        ),
        SpriteInstance::new(
            placed(20.0, -20.0),
            TextureRegion::from_texels(0, 8, 8, 8, texture),
        ),
    ]
}

// --- measures, shared with M3.16 -------------------------------------------

fn srgb_to_linear(byte: u8) -> f64 {
    let c = f64::from(byte) / 255.0;
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The green channel in linear light: the channel B's colours vary in.
fn green(image: &Pixels, x: u32, y: u32) -> f64 {
    srgb_to_linear(image.pixel(x, y).expect("inside the target")[1])
}

/// A channel of a pixel, in linear light.
fn channel(image: &Pixels, x: u32, y: u32, index: usize) -> f64 {
    srgb_to_linear(image.pixel(x, y).expect("inside the target")[index])
}

/// The sub-pixel image x of a sprite's left edge, from the covered fraction of
/// the first column carrying its colour.
///
/// M3.16's measure, generalised over which sprite and which channel. The first
/// column with any ink is either fully covered — edge on its left boundary — or
/// partly, and the fraction says how far into it the edge falls.
fn edge_x(image: &Pixels, row: u32, colour: [u8; 4], index: usize) -> f64 {
    let full = srgb_to_linear(colour[index]);
    for x in 0..image.width() {
        let covered = (channel(image, x, row, index) / full).clamp(0.0, 1.0);
        if covered > 0.0 {
            return f64::from(x) + (1.0 - covered);
        }
    }
    panic!("that colour appears nowhere on row {row}");
}

/// Sprite A's left edge: geometry alone, at either filter.
fn a_edge_x(image: &Pixels) -> f64 {
    edge_x(image, A_EDGE_ROW, A_UPPER, 0)
}

/// Sprite B's left edge: geometry under `Nearest`, geometry *and* the region
/// bleed under `Linear`. Kept because that contamination is a measurement of the
/// bleed rather than a defect in the file.
fn b_edge_x(image: &Pixels) -> f64 {
    edge_x(image, EDGE_ROW, B_UPPER, 1)
}

/// The sub-pixel row where B's upper half gives way to its lower half.
///
/// The midpoint of the two texel colours, in linear light, located by
/// interpolating between the two rows that bracket it. Under `Nearest` that is a
/// step and the crossing sits on the half; under `Linear` it is a ramp about one
/// texel wide and the crossing moves through it continuously. **One measure for
/// both**, so the two columns of the result table are comparable.
fn interior_crossing(image: &Pixels) -> f64 {
    let upper = srgb_to_linear(B_UPPER[1]);
    let lower = srgb_to_linear(B_LOWER[1]);
    let midpoint = f64::midpoint(upper, lower);

    for y in 50..78 {
        let here = green(image, INTERIOR_COLUMN, y);
        if here <= midpoint {
            let above = green(image, INTERIOR_COLUMN, y - 1);
            assert!(
                above > midpoint,
                "row {y} is the first at or below the midpoint but row {} is not \
                 above it — the ramp is not monotonic here and this measure does \
                 not apply",
                y - 1
            );
            let span = above - here;
            assert!(span > 0.0, "a bracketing pair must straddle the midpoint");
            return f64::from(y - 1) + (above - midpoint) / span;
        }
    }
    panic!("no crossing on column {INTERIOR_COLUMN}: the ramp never falls to the midpoint");
}

// --- the test-only pipeline ------------------------------------------------

fn device_or_skip() -> Option<(wgpu::Device, wgpu::Queue, String)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let ladder = [
        (
            "high-performance",
            wgpu::PowerPreference::HighPerformance,
            false,
        ),
        ("no preference", wgpu::PowerPreference::None, false),
        (
            "forced software fallback",
            wgpu::PowerPreference::None,
            true,
        ),
    ];

    let mut rejections = Vec::new();
    for (label, power_preference, force_fallback_adapter) in ladder {
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference,
            force_fallback_adapter,
            compatible_surface: None,
            ..Default::default()
        })) {
            Ok(adapter) => {
                let info = adapter.get_info();
                let summary = format!(
                    "{} [{:?}, {:?}] chosen by: {label}",
                    info.name, info.backend, info.device_type
                );
                let (device, queue) =
                    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                        label: Some("narvo linear measurement device"),
                        ..Default::default()
                    }))
                    .expect("an adapter that answered must give a device");
                println!("adapter in use: {summary}");
                return Some((device, queue, summary));
            }
            Err(error) => rejections.push(format!("{label} ({error})")),
        }
    }

    assert!(
        std::env::var_os(REQUIRE_GPU_VAR).is_none(),
        "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a failure \
         rather than a skip. Tried: {}",
        rejections.join(", ")
    );
    println!(
        "{SKIP_MARKER}: no adapter. Tried: {}",
        rejections.join(", ")
    );
    None
}

/// Renders the scene through this file's pipeline, at `filter`.
#[expect(
    clippy::too_many_lines,
    reason = "one pipeline built inline; splitting it would hide what it \
              replicates from the production path it is checked against"
)]
fn render_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_image: &Pixels,
    sprites: &[SpriteInstance],
    camera: CameraView,
    filter: wgpu::FilterMode,
) -> Pixels {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("narvo linear measurement shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
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

    // The one variable. Everything else is production's.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("narvo linear measurement sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: texture_image.width(),
            height: texture_image.height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        texture_image.rgba(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture_image.width() * 4),
            rows_per_image: Some(texture_image.height()),
        },
        wgpu::Extent3d {
            width: texture_image.width(),
            height: texture_image.height(),
            depth_or_array_layers: 1,
        },
    );

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("narvo linear measurement pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<[f32; 4]>() as wgpu::BufferAddress,
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
                ],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: FORMAT,
                // ADR-0023: production blends, so a replica that did not would be
                // measuring its own pipeline. Named here rather than spelled out as
                // `quad.rs` spells it, so the two are independent statements of the
                // same intent and a change to either shows up as a difference.
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: SAMPLES,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    });

    // Corners: the production projection applied to the production corner table.
    let projection = Projection::for_target(TARGET, TARGET).viewed_by(camera);
    let mut corners: Vec<[f32; 4]> = Vec::with_capacity(sprites.len() * 4);
    for sprite in sprites {
        let p = sprite.placement;
        assert_eq!(
            (p.rot_cos, p.rot_sin),
            SpritePlacement::UNTURNED,
            "this file does not implement rotation"
        );
        let [u_left, v_top, u_right, v_bottom] = sprite.region.uv_bounds();
        for [local_x, local_y, u, v] in CORNERS {
            let [ndc_x, ndc_y] =
                projection.world_to_ndc(local_x * p.scale_x + p.x, local_y * p.scale_y + p.y);
            corners.push([
                ndc_x,
                ndc_y,
                u_left + u * (u_right - u_left),
                v_top + v * (v_bottom - v_top),
            ]);
        }
    }

    let vertex_bytes: Vec<u8> = corners
        .iter()
        .flatten()
        .flat_map(|c| c.to_ne_bytes())
        .collect();
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &vertex_bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let mut index_bytes = Vec::new();
    for i in 0..sprites.len() {
        let base = u32::try_from(i * 4).expect("three sprites stay inside u32");
        for index in INDICES {
            index_bytes.extend_from_slice(&(base + u32::from(index)).to_ne_bytes());
        }
    }
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &index_bytes,
        usage: wgpu::BufferUsages::INDEX,
    });

    let make = |count: u32, copyable: bool| {
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if copyable {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: TARGET,
                height: TARGET,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: count,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        })
    };
    let resolved = make(1, true);
    let resolved_view = resolved.create_view(&wgpu::TextureViewDescriptor::default());
    let multisampled = make(SAMPLES, false);
    let ms_view = multisampled.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &ms_view,
                depth_slice: None,
                resolve_target: Some(&resolved_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, Some(&bind_group), &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        let count = u32::try_from(sprites.len() * 6).expect("three sprites stay inside u32");
        pass.draw_indexed(0..count, 0, 0..1);
    }

    let bytes_per_row = TARGET * 4;
    assert_eq!(bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    let transfer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(bytes_per_row) * u64::from(TARGET),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &resolved,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &transfer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TARGET),
            },
        },
        wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = transfer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = sender.send(r);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("the copy must complete");
    receiver
        .recv()
        .expect("the mapping callback must run")
        .expect("the buffer must map");
    let rgba = slice
        .get_mapped_range()
        .expect("the buffer mapped")
        .to_vec();
    transfer.unmap();

    Pixels::from_rgba8(TARGET, TARGET, rgba).expect("the buffer matches the dimensions")
}

/// This file's pipeline over the file's own scene.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_image: &Pixels,
    camera: CameraView,
    filter: wgpu::FilterMode,
) -> Pixels {
    render_with(
        device,
        queue,
        texture_image,
        &scene(texture_image),
        camera,
        filter,
    )
}

fn production(target: &OffscreenTarget, texture: &Pixels, camera: CameraView) -> Pixels {
    target
        .render_sprites_viewed_by(texture, &scene(texture), camera)
        .expect("three sprites are far inside the batch limit")
}

/// The control: this file's pipeline at `Nearest`, byte-for-byte against the
/// production path, in this process.
///
/// **Called by every test that reads a `Linear` number.** nextest gives each
/// test its own process, so a control that ran only in the test drawing the
/// camera table would not have run in the other two — and two of the three
/// figures this file publishes come from those.
fn control(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &Pixels,
    adapter: &str,
) -> OffscreenTarget {
    let target = offscreen_or_skip(adapter);
    let replicated = render(
        device,
        queue,
        texture,
        CameraView::IDENTITY,
        wgpu::FilterMode::Nearest,
    );
    let produced = production(&target, texture, CameraView::IDENTITY);
    assert_eq!(
        replicated.rgba(),
        produced.rgba(),
        "this file's pipeline at Nearest is not byte-identical to the production \
         path's. Every Linear number in this process would then be measuring the \
         replication rather than the sampler."
    );
    target
}

fn offscreen_or_skip(expected_adapter: &str) -> OffscreenTarget {
    let target = match OffscreenTarget::new(TARGET, TARGET) {
        Ok(target) => target,
        Err(RenderError::NoAdapter { attempts }) => panic!(
            "this file already holds a device, so the production path must find \
             an adapter too. Tried: {attempts}"
        ),
        Err(other) => panic!("the production target could not be created: {other}"),
    };
    assert_eq!(
        target.adapter_summary(),
        expected_adapter,
        "the production path and this file's pipeline landed on different \
         adapters, so comparing their output would compare two rasterisers \
         rather than two samplers"
    );
    target
}

/// **The measurement.** The interior quantum under `Linear`, against `Nearest`.
#[test]
fn linear_gives_the_interior_a_finer_quantum_than_a_whole_pixel() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };
    let texture = atlas();
    let _target = control(&device, &queue, &texture, &adapter);

    let steps = 8_u32;
    println!(
        "camera | Nearest edge (A) | Nearest interior | Linear edge (A) | \
         Linear interior | Linear edge (B, bled)"
    );

    let mut nearest_edges = Vec::new();
    let mut nearest_interiors = Vec::new();
    let mut linear_edges = Vec::new();
    let mut linear_interiors = Vec::new();
    let mut bled_edges = Vec::new();

    for k in 0..=steps {
        #[expect(
            clippy::cast_precision_loss,
            reason = "k is at most 8 and every eighth is exact in f32"
        )]
        let offset = k as f32 / steps as f32;

        // x for the silhouette, y for the interior: the same two axes M3.16
        // read, so the two tables line up row for row.
        let along_x = CameraView::new(offset, 0.0, 1.0);
        let along_y = CameraView::new(0.0, offset, 1.0);

        let near_x = render(
            &device,
            &queue,
            &texture,
            along_x,
            wgpu::FilterMode::Nearest,
        );
        let near_y = render(
            &device,
            &queue,
            &texture,
            along_y,
            wgpu::FilterMode::Nearest,
        );
        let lin_x = render(&device, &queue, &texture, along_x, wgpu::FilterMode::Linear);
        let lin_y = render(&device, &queue, &texture, along_y, wgpu::FilterMode::Linear);

        let ne = a_edge_x(&near_x);
        let ni = interior_crossing(&near_y);
        let le = a_edge_x(&lin_x);
        let li = interior_crossing(&lin_y);
        let bled = b_edge_x(&lin_x);

        println!("{offset:.3} | {ne:.4} | {ni:.4} | {le:.4} | {li:.4} | {bled:.4}");
        nearest_edges.push(ne);
        nearest_interiors.push(ni);
        linear_edges.push(le);
        linear_interiors.push(li);
        bled_edges.push(bled);
    }

    let distinct = |values: &[f64]| {
        let mut seen: Vec<f64> = Vec::new();
        for v in values {
            if !seen.iter().any(|s: &f64| (s - v).abs() < 1e-9) {
                seen.push(*v);
            }
        }
        seen.len()
    };

    println!(
        "distinct positions over one pixel — Nearest edge {}, Nearest interior {}, \
         Linear edge {}, Linear interior {}",
        distinct(&nearest_edges),
        distinct(&nearest_interiors),
        distinct(&linear_edges),
        distinct(&linear_interiors)
    );

    // The reproduction check: this file's Nearest column has to be M3.16's.
    assert_eq!(
        distinct(&nearest_edges),
        SAMPLES as usize + 1,
        "M3.16 measured five distinct silhouette positions per pixel under \
         Nearest, and this file has to reproduce that before its Linear column \
         means anything: {nearest_edges:?}"
    );
    assert_eq!(
        distinct(&nearest_interiors),
        2,
        "M3.16 measured the interior stepping one whole pixel under Nearest: \
         {nearest_interiors:?}"
    );

    // The finding.
    assert!(
        (nearest_interiors[0] - 63.5).abs() < 1e-9,
        "the Nearest crossing must sit on the half-pixel between the two texel \
         rows; measured {}",
        nearest_interiors[0]
    );
    assert!(
        (nearest_edges[0] - A_LEFT_AT_IDENTITY).abs() < 1e-9,
        "sprite A's left edge must be at exactly {A_LEFT_AT_IDENTITY} with the \
         camera at rest, or every displacement below means nothing; measured {}",
        nearest_edges[0]
    );

    // The bleed, reported as its own number rather than left to contaminate the
    // silhouette column. Geometry puts B's left edge at 48; this measure puts it
    // at 48.375, which is a bilinear weight of 0.375 on the neighbouring
    // region's texel — M3.9's finding at this scene's magnification.
    assert!(
        (bled_edges[0] - 48.375).abs() < 0.01,
        "B's left edge under Linear reads {}, not the 48.375 that a bilinear \
         weight of 0.375 on the neighbouring region's texel implies",
        bled_edges[0]
    );
    println!(
        "B's left edge under Linear, contaminated by the region bleed: {} \
         distinct positions — {bled_edges:?}",
        distinct(&bled_edges)
    );
    assert_eq!(
        distinct(&linear_interiors),
        steps as usize + 1,
        "the interior gave {} distinct positions over {steps} steps, not one per \n         step. BASELINE.md reports this as nine of nine: {linear_interiors:?}",
        distinct(&linear_interiors)
    );
    assert!(
        distinct(&linear_interiors) > 2,
        "Linear left the interior with {} distinct positions over one pixel of \
         travel, which is no finer than Nearest's whole-pixel step. Then Linear \
         does not solve what D14 left open. Measured: {linear_interiors:?}",
        distinct(&linear_interiors)
    );
    assert_eq!(
        distinct(&linear_edges),
        SAMPLES as usize + 1,
        "the silhouette is graded by MSAA coverage, which the sampler does not \
         touch, so the Linear edge column must match the Nearest one: \
         {linear_edges:?}"
    );
}

/// The magnification this scene renders at, which the interior's resolution
/// under `Linear` depends on.
///
/// Runs without a GPU. The claim "about 32 counts per pixel of shift" is only
/// meaningful beside the number it is derived from.
#[test]
fn the_scene_magnifies_four_pixels_to_the_texel() {
    let texture = atlas();
    // Both axes. The number this underwrites — "the ramp spans one texel", and
    // from it "about 32 counts per pixel of shift" — is used on the *vertical*
    // ramp: `interior_crossing` scans down a column and the interior sweep moves
    // the camera in y. Checking u alone would leave the axis the claim is about
    // unchecked, and v happening to match is not the same as v being checked.
    for sprite in scene(&texture) {
        let [u_left, v_top, u_right, v_bottom] = sprite.region.uv_bounds();
        for (axis, span, extent) in [
            ("u", u_right - u_left, sprite.placement.scale_x),
            ("v", v_bottom - v_top, sprite.placement.scale_y),
        ] {
            let texels = f64::from(span) * f64::from(ATLAS);
            let pixels = f64::from(extent);
            assert!(
                (pixels / texels - PIXELS_PER_TEXEL).abs() < 1e-9,
                "along {axis} a sprite spans {pixels} pixels over {texels} \
                 texels, not {PIXELS_PER_TEXEL} pixels per texel"
            );
        }
    }
}

/// The same atlas with every region given a one-texel duplicated border.
///
/// Each 8 x 8 quadrant becomes 10 x 10, so the atlas grows from 16 x 16 to
/// 20 x 20 and the regions move to `(q * 10 + 1, ...)`. The border repeats the
/// region's own outermost row or column, which is what makes the bilinear blend
/// partner at a region edge equal to the edge texel itself.
fn padded_atlas() -> Pixels {
    narvo_testkit::AtlasLayout::PADDED.atlas()
}

/// The scene against the padded atlas: same sprites, regions moved inside their
/// borders.
fn padded_scene(texture: &Pixels) -> [SpriteInstance; 3] {
    let placed = |x: f32, y: f32| SpritePlacement {
        x,
        y,
        rot_cos: 1.0,
        rot_sin: 0.0,
        scale_x: 32.0,
        scale_y: 32.0,
    };
    [
        SpriteInstance::new(
            placed(-20.0, 20.0),
            TextureRegion::from_texels(1, 1, 8, 8, texture),
        ),
        SpriteInstance::new(
            placed(0.0, 0.0),
            TextureRegion::from_texels(11, 1, 8, 8, texture),
        ),
        SpriteInstance::new(
            placed(20.0, -20.0),
            TextureRegion::from_texels(1, 11, 8, 8, texture),
        ),
    ]
}

/// What padding costs and what it buys, both measured rather than argued.
///
/// **Costs nothing under `Nearest`** — the production sampler reads one texel and
/// padding does not change which, so a padded atlas with the regions moved to
/// match must render byte-identically. That is the measurable half of "does this
/// antidote move the blessed images".
///
/// **Buys the bleed under `Linear`** — the blend partner at a region edge becomes
/// the duplicated border, which equals the edge texel, so the edge column reads
/// its own colour at full strength instead of 62.5 % of it.
///
/// Neither is built: this renders a different atlas through the unchanged
/// production path, which is a measurement about content and not a change to the
/// renderer.
#[test]
fn padding_is_free_under_nearest_and_removes_the_bleed_under_linear() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };
    let plain = atlas();
    let target = control(&device, &queue, &plain, &adapter);
    let padded = padded_atlas();

    // Under Nearest, through the production path, with nothing else changed.
    let before = production(&target, &plain, CameraView::IDENTITY);
    let after = target
        .render_sprites_viewed_by(&padded, &padded_scene(&padded), CameraView::IDENTITY)
        .expect("three sprites are far inside the batch limit");
    assert_eq!(
        before.rgba(),
        after.rgba(),
        "padding the atlas changed the picture under Nearest. It must not: the \
         sampler reads one texel and padding does not change which one, so this \
         is the check that says an atlas change of this shape costs no blessing."
    );
    println!("padding under Nearest: byte-identical through the production path");

    // Under Linear, in this file's pipeline, on the edge that bleeds.
    let bled = b_edge_x(&render(
        &device,
        &queue,
        &plain,
        CameraView::IDENTITY,
        wgpu::FilterMode::Linear,
    ));
    let healed = b_edge_x(&render_with(
        &device,
        &queue,
        &padded,
        &padded_scene(&padded),
        CameraView::IDENTITY,
        wgpu::FilterMode::Linear,
    ));
    println!("B's left edge under Linear: {bled:.4} unpadded, {healed:.4} padded");

    assert!(
        (bled - 48.375).abs() < 0.01,
        "the unpadded bleed must still read 48.375; measured {bled}"
    );
    assert!(
        (healed - 48.0).abs() < 0.01,
        "with a one-texel border the blend partner is the region's own edge \
         texel, so the column must read full strength and put the edge back at \
         48. Measured {healed}"
    );

    // The area price, at this packing.
    let plain_area = f64::from(plain.width()) * f64::from(plain.height());
    let padded_area = f64::from(padded.width()) * f64::from(padded.height());
    println!(
        "atlas area: {plain_area} texels unpadded, {padded_area} padded — \
         {:.1} % more for four 8 x 8 regions at one texel of border",
        (padded_area / plain_area - 1.0) * 100.0
    );
    // The figures BASELINE.md quotes, pinned rather than only printed.
    assert!((plain_area - 256.0).abs() < f64::EPSILON);
    assert!((padded_area - 400.0).abs() < f64::EPSILON);
}

/// A known disturbance, reported as itself and split across the three thresholds.
///
/// The disturbance is the sampler: the same scene, the same camera, `Nearest`
/// against `Linear`. Its size is not guessed — every pixel whose texel
/// coordinate is not exactly on a texel centre gets a blend, so most of the
/// sprite area moves, and the point of the decomposition is that a change this
/// large reads as a *cap* failure and not as noise past a budget.
///
/// The same three numbers `BASELINE.md` reports everywhere else, so the figure
/// here is on the scale the rest of the document uses.
#[test]
fn the_sampler_change_is_reported_as_a_cap_failure_and_not_as_noise() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };
    let texture = atlas();
    let _target = control(&device, &queue, &texture, &adapter);

    let near = render(
        &device,
        &queue,
        &texture,
        CameraView::IDENTITY,
        wgpu::FilterMode::Nearest,
    );
    let linear = render(
        &device,
        &queue,
        &texture,
        CameraView::IDENTITY,
        wgpu::FilterMode::Linear,
    );

    let tolerance = narvo_render2d::Tolerance::default();
    let mut differing = 0_u64;
    let mut worst = 0_u8;
    let mut touched = 0_u64;
    for (a, b) in near
        .rgba()
        .chunks_exact(4)
        .zip(linear.rgba().chunks_exact(4))
    {
        let deviation = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        worst = worst.max(deviation);
        if deviation > 0 {
            touched += 1;
        }
        if deviation > tolerance.channel {
            differing += 1;
        }
    }

    let total = u64::from(TARGET) * u64::from(TARGET);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a ratio only needs enough digits to compare against a budget"
    )]
    let ratio = differing as f64 / total as f64;

    println!(
        "Nearest against Linear: {differing} of {total} pixels ({:.4}%) past the \
         floor of {}, budget {:.4}%; {touched} differ at all; worst channel \
         deviation {worst}, limit {}",
        ratio * 100.0,
        tolerance.channel,
        tolerance.max_differing_ratio * 100.0,
        tolerance.max_channel_deviation
    );

    assert!(
        ratio > tolerance.max_differing_ratio,
        "a sampler change has to trip the pixel budget; {ratio} against {}",
        tolerance.max_differing_ratio
    );
    assert!(
        worst > tolerance.max_channel_deviation,
        "and it has to trip the cap as well — that is what makes it a different \
         picture rather than a noisier one. Worst {worst} against {}",
        tolerance.max_channel_deviation
    );
}
