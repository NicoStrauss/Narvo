//! How far three rasterisers drift apart once a pixel carries a **mixed**
//! value.
//!
//! # Why this file builds its own pipeline
//!
//! `BASELINE.md` records **one** blessed image — the 64 x 64 quad of M1 — byte
//! identical across llvmpipe, WARP and a discrete AMD GPU, and six camera
//! renders, which are not blessed references, byte-identical against the AMD
//! run. `worst channel deviation 0` in both. (The repository has five blessed
//! images; four of them have no cross-rasteriser figure anywhere.) M3.13
//! established why every one of those zeroes was cheap: **this renderer cannot
//! produce a mixed pixel at all.** One
//! sample per pixel (`multisample: MultisampleState::default()` in `quad.rs`)
//! and `Nearest` filtering mean every pixel is exactly one texel, and the three
//! rasterisers only ever had to agree on which side of a pixel centre an edge
//! falls.
//!
//! So there is nothing to measure *through* the production path, and M3.14 must
//! not change it. What it does instead: build a second, test-only pipeline that
//! blends, and measure that. `wgpu` is a dependency of this crate, so an
//! integration test can — CLAUDE.md records the same fact from M2.1, that tests
//! under `tests/` see the package's `[dependencies]`.
//!
//! **What that costs, stated rather than hidden.** These numbers are about
//! *these three rasterisers doing this blending*, not about a hypothetical
//! Narvo renderer with antialiasing switched on. The pipeline below uses the
//! same target format, the same clear, and a shader of the same shape, so the
//! result transfers — but it is a transfer, not a measurement of the shipped
//! path, and no row of `BASELINE.md` should be read as if it were.
//!
//! **A rejected alternative, because it looks cheaper and answers nothing.**
//! Render at 4x through the existing pipeline and average on the CPU. The
//! averaging would then happen in Rust, identically on every platform, so the
//! three rasterisers would agree by construction: the measurement would report
//! zero and would have measured its own arithmetic.
//!
//! # The two regimes, and where each one mixes
//!
//! - **Coverage (`msaa`).** Four samples per pixel, resolved. Mixes **only at
//!   geometry edges**, where some samples are inside the primitive and some are
//!   not. The interior of a primitive is fully covered and gets no blend.
//! - **Filtering (`linear`).** `FilterMode::Linear`, one sample per pixel.
//!   Mixes **only inside a primitive**, wherever a sample lands between texel
//!   centres. A geometry edge is still a hard in/out decision.
//!
//! They are complementary, which is why both are measured. Neither produces the
//! other's mixed pixels.

use std::path::{Path, PathBuf};

use narvo_render2d::{Golden, Pixels, golden_artifact_dir};
use wgpu::util::DeviceExt as _;

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// A directory of another rasteriser's PNGs to compare against.
const BASELINE_VAR: &str = "NARVO_SOFT_EDGE_BASELINE";

/// Target edge, in pixels.
///
/// 128 rather than any other size for one mechanical reason: `128 * 4` is 512
/// bytes per row, a multiple of `COPY_BYTES_PER_ROW_ALIGNMENT` (256), so the
/// read-back needs no padding strip. `offscreen.rs` carries that strip because
/// it accepts any width; this file does not accept any width.
const TARGET: u32 = 128;

/// Samples per pixel in the coverage regime.
const SAMPLES: u32 = 4;

/// Format of both the render target and the texture, matching the production
/// path so that the sRGB decode/encode round trip is the same one.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The two shaders, one module.
const SHADER: &str = r#"
@vertex
fn vs_solid(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 0.0, 1.0);
}

@fragment
fn fs_solid() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}

struct Textured {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_textured(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> Textured {
    var out: Textured;
    out.clip_position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var soft_texture: texture_2d<f32>;
@group(0) @binding(1) var soft_sampler: sampler;

@fragment
fn fs_textured(input: Textured) -> @location(0) vec4<f32> {
    return textureSample(soft_texture, soft_sampler, input.uv);
}
"#;

// --- the two scenes -------------------------------------------------------

/// The triangle of the coverage regime, in **image** coordinates.
///
/// Chosen against the case the repository already knows. **Every blessed scene
/// is axis-aligned** — and one of them, `camera_regions_128x128`, already has a
/// vertical edge at `X = 52.75`, so off-grid geometry is not the new thing here.
/// A non-axis-aligned edge is: no blessed scene has one, and an edge at an angle
/// is what makes coverage partial rather than merely offset.
///
/// | edge | slope | angle from horizontal |
/// | --- | --- | --- |
/// | A→B | -18/106 = -0.1698 | 9.6 deg |
/// | B→C | 108/-48 = -2.25 | 66.0 deg |
/// | C→A | -90/-58 = 1.5517 | 57.2 deg |
///
/// **No slope is 0, ±1 or infinite.** 45 degrees is excluded deliberately: an
/// edge at exactly 45 degrees passes through the corner of every pixel it
/// crosses and hits the standard 4x sample pattern symmetrically, which is a
/// special case rather than a typical one.
///
/// **Vertex A sits exactly on a pixel centre** — (12.5, 30.5) is the centre of
/// pixel (12, 30) — and the A→B edge, of slope -9/53, passes through the centre
/// of pixel (65, 21) as well. That is the tie case the production path resolves
/// by a fill rule; with four samples per pixel it is no longer a tie, and the
/// point of including it is that the two regimes disagree about what is
/// special.
const TRIANGLE: [[f32; 2]; 3] = [[12.5, 30.5], [118.5, 12.5], [70.5, 120.5]];

/// The quad of the filtering regime, in **image** coordinates, as
/// `[left, top, right, bottom]`.
///
/// 101.3 pixels across 8 texels is 12.6625 pixels per texel — not a whole
/// number, so the sample pattern does not repeat from texel to texel and the
/// blend weight differs in every column of the interior. A whole-number
/// magnification would repeat the same handful of offsets in every texel
/// instead of walking through roughly a hundred of them.
///
/// Not every pixel of the quad blends: `ClampToEdge` holds the outer half-texel
/// band on each side at the edge texel, so those columns and rows carry a pure
/// colour. That band is why the mixed-pixel count below is not the whole quad.
const QUAD: [f32; 4] = [9.7, 13.3, 111.0, 114.6];

/// Edge of the checkerboard texture, in texels.
const CHECKER: u32 = 8;

/// The checkerboard: one texel per cell, black against white.
///
/// Maximum contrast on purpose. The question this file asks is how far the
/// rasterisers drift, and a blend between two colours 255 counts apart is where
/// a difference in blending has the most room to show. A gentle texture would
/// measure the same disagreement scaled down and report it as smaller than it
/// is.
fn checkerboard() -> Vec<u8> {
    let mut rgba = Vec::with_capacity((CHECKER * CHECKER * 4) as usize);
    for y in 0..CHECKER {
        for x in 0..CHECKER {
            let white = (x + y) % 2 == 0;
            rgba.extend_from_slice(if white {
                &[255, 255, 255, 255]
            } else {
                &[0, 0, 0, 255]
            });
        }
    }
    rgba
}

/// Image coordinates to normalised device coordinates, ADR-0004's directions.
///
/// x right, y **up** in NDC while image rows run down, which is the one
/// reconciliation this file has to make and it makes it here, once.
fn to_ndc(x: f32, y: f32) -> [f32; 2] {
    let half = TARGET as f32 / 2.0;
    [x / half - 1.0, 1.0 - y / half]
}

// --- the device -----------------------------------------------------------

/// The adapter ladder of `gpu.rs`, replicated.
///
/// Written out rather than reached for: `select_adapter` is `pub(crate)` and no
/// `wgpu` type crosses this crate's public API by design (ADR-0015's
/// neighbour). Replicating the three rungs in the same order is what makes the
/// adapter string this file prints comparable with the ones already in
/// `BASELINE.md`.
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
                        label: Some("narvo soft-edge measurement device"),
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

/// A colour target, `sample_count` samples deep.
fn target_texture(device: &wgpu::Device, sample_count: u32, copyable: bool) -> wgpu::Texture {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if copyable {
        usage |= wgpu::TextureUsages::COPY_SRC;
    }
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("narvo soft-edge target"),
        size: wgpu::Extent3d {
            width: TARGET,
            height: TARGET,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage,
        view_formats: &[],
    })
}

/// Copies a single-sample texture back as tightly packed RGBA8.
///
/// No padding strip: `TARGET * 4` is 512, a multiple of 256, asserted below so
/// that changing `TARGET` fails here rather than producing a sheared image.
fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Pixels {
    let bytes_per_row = TARGET * 4;
    assert_eq!(
        bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "TARGET was changed to a width whose rows need padding; this file has no \
         padding strip and would read back a sheared image"
    );

    let transfer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("narvo soft-edge read-back"),
        size: u64::from(bytes_per_row) * u64::from(TARGET),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
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
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
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

    let rgba = {
        let mapped = slice
            .get_mapped_range()
            .expect("the buffer mapped, so the range is readable");
        mapped.to_vec()
    };
    transfer.unmap();

    Pixels::from_rgba8(TARGET, TARGET, rgba).expect("the buffer matches the dimensions")
}

// --- the two renders ------------------------------------------------------

/// The coverage regime: one white triangle, four samples per pixel, resolved.
fn render_msaa(device: &wgpu::Device, queue: &wgpu::Queue) -> Pixels {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("narvo soft-edge shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("narvo soft-edge msaa pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_solid"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<[f32; 2]>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                }],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_solid"),
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

    let corners: Vec<f32> = TRIANGLE.iter().flat_map(|[x, y]| to_ndc(*x, *y)).collect();
    let bytes: Vec<u8> = corners.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("narvo soft-edge triangle"),
        contents: &bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });

    let multisampled = target_texture(device, SAMPLES, false);
    let resolved = target_texture(device, 1, true);
    let ms_view = multisampled.create_view(&wgpu::TextureViewDescriptor::default());
    let re_view = resolved.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("narvo soft-edge msaa pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &ms_view,
                depth_slice: None,
                resolve_target: Some(&re_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(0..3, 0..1);
    }
    queue.submit(std::iter::once(encoder.finish()));

    read_back(device, queue, &resolved)
}

/// The filtering regime: a magnified checkerboard through a `Linear` sampler.
fn render_linear(device: &wgpu::Device, queue: &wgpu::Queue) -> Pixels {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("narvo soft-edge shader"),
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

    // The one difference from the production sampler, and the whole point of
    // this regime.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("narvo soft-edge linear sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("narvo soft-edge checkerboard"),
        size: wgpu::Extent3d {
            width: CHECKER,
            height: CHECKER,
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
        &checkerboard(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(CHECKER * 4),
            rows_per_image: Some(CHECKER),
        },
        wgpu::Extent3d {
            width: CHECKER,
            height: CHECKER,
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
        label: Some("narvo soft-edge linear pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_textured"),
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
            entry_point: Some("fs_textured"),
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
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let [left, top, right, bottom] = QUAD;
    let corners: Vec<f32> = [
        (left, top, 0.0_f32, 0.0_f32),
        (left, bottom, 0.0, 1.0),
        (right, bottom, 1.0, 1.0),
        (left, top, 0.0, 0.0),
        (right, bottom, 1.0, 1.0),
        (right, top, 1.0, 0.0),
    ]
    .iter()
    .flat_map(|(x, y, u, v)| {
        let [nx, ny] = to_ndc(*x, *y);
        [nx, ny, *u, *v]
    })
    .collect();
    let bytes: Vec<u8> = corners.iter().flat_map(|v| v.to_ne_bytes()).collect();
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("narvo soft-edge quad"),
        contents: &bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });

    let target = target_texture(device, 1, true);
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("narvo soft-edge linear pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(0..6, 0..1);
    }
    queue.submit(std::iter::once(encoder.finish()));

    read_back(device, queue, &target)
}

// --- counting and comparing -----------------------------------------------

/// Pixels whose value is neither pure black nor pure white.
///
/// **The first number of the measurement, and the one that decides whether the
/// rest means anything.** A scene that produces no mixed pixel measures zero
/// and answers nothing, whatever the rasterisers do. Both scenes here draw only
/// black and white, so "mixed" is exactly "not 0 and not 255".
fn mixed_pixels(image: &Pixels) -> u64 {
    let mut mixed = 0;
    for pixel in image.rgba().chunks_exact(4) {
        if pixel[..3].iter().any(|&c| c != 0 && c != 255) {
            mixed += 1;
        }
    }
    mixed
}

/// How many pixels differ by how much, bucketed.
///
/// `GoldenReport` gives the worst pixel and a count over one threshold, which is
/// what a pass/fail needs. A *margin* needs the shape: one edge pixel 40 counts
/// out is a different fact from a thousand pixels 6 counts out, and the two
/// would be reported identically by a maximum and a ratio.
fn deviation_histogram(left: &Pixels, right: &Pixels) -> Vec<(u8, u64)> {
    let mut counts = [0_u64; 256];
    for (a, b) in left
        .rgba()
        .chunks_exact(4)
        .zip(right.rgba().chunks_exact(4))
    {
        let deviation = a
            .iter()
            .zip(b.iter())
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        counts[deviation as usize] += 1;
    }
    counts
        .iter()
        .enumerate()
        .filter(|&(_, &n)| n > 0)
        .map(|(d, &n)| {
            (
                u8::try_from(d).expect("a deviation index is at most 255"),
                n,
            )
        })
        .collect()
}

/// Where this file writes its renders.
fn output_dir() -> PathBuf {
    golden_artifact_dir().join("soft-edge")
}

/// Prints how far `rendered` sits from `reference_dir/<case>.png`.
fn compare_against(reference_dir: &Path, case: &str, rendered: &Pixels, output: &Path) {
    let golden = Golden::new(reference_dir, output);
    match golden.verify(case, rendered) {
        Ok(report) => println!("  {case}: {}", report.measured_against(golden.tolerance)),
        Err(error) => println!("  {case}: {error}"),
    }

    let path = reference_dir.join(format!("{case}.png"));
    if let Ok(decoded) = image::open(&path) {
        let decoded = decoded.to_rgba8();
        let (w, h) = (decoded.width(), decoded.height());
        if let Ok(reference) = Pixels::from_rgba8(w, h, decoded.into_raw()) {
            let histogram = deviation_histogram(&reference, rendered);
            let spread: u64 = histogram
                .iter()
                .filter(|(d, _)| *d > 0)
                .map(|(_, n)| n)
                .sum();
            println!("  {case}: deviation histogram (counts -> pixels): {histogram:?}");
            println!("  {case}: {spread} pixels differ at all");
        }
    }
}

/// **The measurement.**
#[test]
fn the_soft_edge_rasteriser_margin_is_recorded() {
    let Some((device, queue, _adapter)) = device_or_skip() else {
        return;
    };

    let output = output_dir();
    std::fs::create_dir_all(&output).expect("the output directory must be creatable");

    let baseline = std::env::var_os(BASELINE_VAR).map(PathBuf::from);
    match &baseline {
        Some(dir) => println!("comparing against {}", dir.display()),
        None => println!("no {BASELINE_VAR} set: writing only"),
    }

    for (case, rendered) in [
        ("msaa", render_msaa(&device, &queue)),
        ("linear", render_linear(&device, &queue)),
    ] {
        let mixed = mixed_pixels(&rendered);
        let total = u64::from(TARGET) * u64::from(TARGET);

        // The scene has to produce the case before the case can be measured.
        assert!(
            mixed > 0,
            "the {case} scene produced {mixed} mixed pixels of {total}. A scene \
             with no mixed pixel measures zero however the rasterisers behave, \
             and a zero from it would be read as agreement rather than as an \
             empty series."
        );

        #[expect(
            clippy::cast_precision_loss,
            reason = "a percentage of a pixel count needs four significant digits"
        )]
        let share = mixed as f64 / total as f64 * 100.0;
        println!("{case}: {mixed} of {total} pixels carry a mixed value ({share:.4}%)");

        let path = output.join(format!("{case}.png"));
        rendered
            .save_png(&path)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        println!("{case} -> {}", path.display());

        if let Some(reference_dir) = &baseline {
            compare_against(reference_dir, case, &rendered, &output);
        }
    }
}

/// The two scenes are the scenes the documentation describes.
///
/// Runs without a GPU. Every number the measurement is read through — that the
/// triangle is not axis-aligned, that no edge is at 45 degrees, that a vertex
/// lands on a pixel centre, that the quad's magnification is not a whole number
/// — is a claim about arithmetic, and a measurement read through a false claim
/// is worse than none.
#[test]
fn the_scenes_are_what_the_documentation_says() {
    // No slope is 0, +/-1 or infinite.
    for (from, to) in [(0, 1), (1, 2), (2, 0)] {
        let [x0, y0] = TRIANGLE[from];
        let [x1, y1] = TRIANGLE[to];
        assert_ne!(x0, x1, "edge {from}->{to} is vertical");
        assert_ne!(y0, y1, "edge {from}->{to} is horizontal");
        let slope = (y1 - y0) / (x1 - x0);
        println!("edge {from}->{to}: slope {slope}");
        assert!(
            (slope.abs() - 1.0).abs() > 0.05,
            "edge {from}->{to} has slope {slope}, within a twentieth of 45 \
             degrees. That angle hits the sample pattern symmetrically and is a \
             special case, not a typical one."
        );
    }

    // One vertex sits exactly on a pixel centre.
    assert_eq!(TRIANGLE[0], [12.5, 30.5]);
    assert_eq!(TRIANGLE[0][0].fract(), 0.5);
    assert_eq!(TRIANGLE[0][1].fract(), 0.5);

    // The quad magnifies by a non-integer factor on both axes.
    let [left, top, right, bottom] = QUAD;
    let across = (right - left) / CHECKER as f32;
    let down = (bottom - top) / CHECKER as f32;
    println!("quad: {across} pixels per texel across, {down} down");
    assert_ne!(
        across.fract(),
        0.0,
        "a whole-number magnification would put \
         every sample at the same offset inside its texel"
    );
    assert_ne!(down.fract(), 0.0);

    // The checkerboard really alternates, or the linear regime blends a colour
    // with itself and mixes nothing.
    let checker = checkerboard();
    let at = |x: usize, y: usize| checker[(y * CHECKER as usize + x) * 4];
    assert_eq!(at(0, 0), 255);
    assert_eq!(at(1, 0), 0);
    assert_eq!(at(0, 1), 0);
    assert_eq!(at(1, 1), 255);
}
