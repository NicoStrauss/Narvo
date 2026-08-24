//! What two draw calls do to the drawing order, and what the three ways out of
//! `ProjektPlan.md` §11/D15 cost.
//!
//! # The question
//!
//! D13 is decided: `Linear` with padding, and a second sampler kept in view from
//! the start. **D15 is its precondition and is open.** M3.18's self-audit found
//! why: two samplers mean two draw calls, there is no depth buffer
//! (`depth_stencil: None`), so the drawing order *is* the painter's algorithm —
//! everything in the second draw call lands on top of everything in the first,
//! whatever its depth. Batching by sampler and z-ordering do not compose.
//!
//! §11 names three ways and marks none as checked: a depth buffer, sorting
//! across batch boundaries with as many switches as the order forces, or
//! forbidding sprites of different samplers to overlap.
//!
//! **This file demonstrates and measures. It decides nothing and changes nothing
//! in the production path.** Everything here is a test-only pipeline; `quad.rs`
//! is untouched.
//!
//! # The demonstration, and the predictions written before it ran
//!
//! Two 32 x 32 sprites, overlapping. `BACK` at world (-8, 0) covers image
//! columns 40..72; `FRONT` at (8, 0) covers 56..88; both cover rows 48..80. The
//! overlap is columns 56..72 by rows 48..80 — **512 pixels** — and (64, 64) is
//! interior to both, so no silhouette or region edge reaches it.
//!
//! Their regions are uniform colours, so `Nearest` and `Linear` agree everywhere
//! except within one texel of a region edge, where `Linear` blends into the
//! neighbouring region — the bleed the padding section of `BASELINE.md` prices.
//! The probe at (64, 64) is eight pixels from the nearest region edge, so the
//! sampler cannot be what changes *it*. Two of the sixteen overlap columns, 70
//! and 71, carry that bleed as well as the draw-order swap; both lie inside the
//! overlap and count as differing either way.
//!
//! 1. **One draw call, depth order respected:** the overlap is `FRONT`'s colour.
//! 2. **Two draw calls, `FRONT` in the first:** the overlap is `BACK`'s colour —
//!    exactly 512 pixels differ from case 1, all of them in columns 56..72 and
//!    rows 48..80, and (64, 64) reads `BACK` where it should read `FRONT`.
//! 3. **Two draw calls plus a depth buffer:** case 1 again, byte for byte,
//!    because the depth test decides rather than the draw order.
//! 4. **Equal depths under a depth buffer** are the interesting case: with
//!    `CompareFunction::Less` the second sprite's fragments fail wherever the
//!    first already wrote, so the *first* wins the overlap — the opposite of the
//!    painter's algorithm — while it still draws normally outside it. With
//!    `LessEqual` the later one wins and today's behaviour is reproduced.
//!
//! # Outside the production path, and checked against it
//!
//! The pipeline is production's in every respect the case does not change. **The
//! replication is checked live** against
//! `OffscreenTarget::render_sprites_viewed_by`, byte for byte, over all five
//! blessed scenes, in every test that publishes a measured **GPU** figure — nextest
//! gives each test its own process, which M3.17 found out the hard way and M3.18
//! recorded.

use narvo_render2d::{
    CameraView, OffscreenTarget, Pixels, Projection, RenderError, SpriteInstance, SpritePlacement,
    TextureRegion,
};
use wgpu::util::DeviceExt as _;

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The atlas edge in texels, for this file's own flat atlas.
const ATLAS: u32 = 16;

/// Samples per pixel in the antialiased render.
///
/// The production constant rather than a literal 4. The control below compares
/// this file's render against the production path's, so the two have to agree on
/// the sample count by construction; a local literal would let them drift apart
/// silently and turn the control into a comparison of two different things.
const SAMPLES: u32 = narvo_render2d::SAMPLE_COUNT;

/// Format of both the render target and the texture, matching the production
/// path so the sRGB round trip is the same one.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// The sprite's own corners, as `[x, y, u, v]`, replicated from
/// `sprite.rs`'s `SPRITE_CORNERS`, which is `pub(crate)`.
///
/// The pairing is ADR-0004's: world `y = +0.5` — the top — carries `v = 0.0`,
/// because texture rows run downward while world y runs up. Getting this wrong
/// flips the image, and the control below is what catches it — it compares this
/// table's output against `sprite.rs`'s `SPRITE_CORNERS` by way of two renders.
const CORNERS: [[f32; 4]; 4] = [
    [-0.5, 0.5, 0.0, 0.0],
    [-0.5, -0.5, 0.0, 1.0],
    [0.5, -0.5, 1.0, 1.0],
    [0.5, 0.5, 1.0, 0.0],
];

/// Two triangles per sprite, replicated from `quad.rs`'s `INDICES`.
const INDICES: [u16; 6] = [0, 1, 2, 0, 2, 3];

// --- the five blessed scenes ----------------------------------------------

/// The 16 x 16 atlas M3.9, M3.11 and M3.12 used: four quadrants, each split
/// into an upper and a lower half of four rows.
///
/// **Deliberately unpadded.** M3.21 gave the originals a one-texel border;
/// this file did not move with them, because it is a measurement
/// baseline rather than a re-derivation of the scene. Under `Nearest` the two
/// render the same image; under `Linear` they will not.
fn atlas() -> Pixels {
    narvo_testkit::AtlasLayout::UNPADDED.atlas()
}

/// One sprite of a scene: where it goes, and which texels it samples.
struct Placed {
    placement: SpritePlacement,
    /// `(left, top, width, height)` in texels, or `None` for the whole texture.
    region: Option<(u32, u32, u32, u32)>,
}

/// One blessed scene, everything needed to draw it.
struct Scene {
    /// The name the reference is blessed under.
    name: &'static str,
    /// Square target edge.
    target: u32,
    /// The texture every sprite in this scene samples.
    texture: Pixels,
    /// The sprites, in draw order.
    sprites: Vec<Placed>,
    /// The view. Only M3.12's scene has a non-identity one.
    camera: CameraView,
}

fn sprite(x: f32, y: f32, w: f32, h: f32, region: Option<(u32, u32, u32, u32)>) -> Placed {
    Placed {
        placement: SpritePlacement {
            x,
            y,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: w,
            scale_y: h,
        },
        region,
    }
}

/// The five scenes, in the order the milestones blessed them.
///
/// Every value is copied from the test that renders it, and **the control does
/// not check the copy** — it renders the same table through both paths, so a
/// wrong placement, texture, camera or target edge moves both sides together.
/// The values are transcribed by hand and stay that way.
///
/// **M1's screen-filling quad is expressed as a sprite covering the whole
/// target**: on a 64 x 64 target with the identity camera, a sprite of scale 64
/// has corners at NDC ±1 and, with the whole-texture region, uv 0 and 1, which
/// is `quad.rs`'s `VERTICES`. That is an argument, not an assertion. **The
/// control does not test it**, because both sides of the comparison go through
/// `render_sprites_viewed_by`, and `VERTICES` belongs to the *other* production
/// entry point — `render_textured_quad`, which draws it through `encode_pass`
/// with `Uint16` indices, and which is what blessed
/// `textured_quad_quadrants_64x64` in the first place.
///
/// So this scene's row measures `Linear` on **the sprite rendering of M1's
/// geometry**, not on M1's path. The path itself is covered by
/// `the_textured_quad_matches_its_golden_reference` against the blessed image,
/// and by nothing here. Saying otherwise would repeat the error `sprite.rs`
/// records at `a_batch_of_one_is_the_single_sprite_path_bit_for_bit`, where a
/// claim about one path was written as a claim about two until M3.9 caught it
/// (`ProjektPlan.md` §12).
fn scenes() -> Vec<Scene> {
    let region = |l, t, w, h| Some((l, t, w, h));
    vec![
        Scene {
            name: "textured_quad_quadrants_64x64",
            target: 64,
            texture: narvo_testkit::quadrant_texture(8),
            sprites: vec![sprite(0.0, 0.0, 64.0, 64.0, None)],
            camera: CameraView::IDENTITY,
        },
        Scene {
            name: "placed_sprite_quadrants_128x128",
            target: 128,
            texture: narvo_testkit::quadrant_texture(8),
            sprites: vec![sprite(16.0, -8.0, 48.0, 32.0, None)],
            camera: CameraView::IDENTITY,
        },
        Scene {
            name: "sprite_atlas_regions_128x128",
            target: 128,
            texture: atlas(),
            sprites: vec![
                sprite(-32.0, 32.0, 32.0, 32.0, region(0, 0, 8, 8)),
                sprite(24.0, 32.0, 48.0, 24.0, region(8, 0, 8, 8)),
                sprite(-16.0, -40.0, 24.0, 40.0, region(0, 8, 8, 8)),
            ],
            camera: CameraView::IDENTITY,
        },
        Scene {
            name: "layer_order_regions_128x128",
            target: 128,
            texture: atlas(),
            // Draw order after `placements_of` sorts by depth: A, B, C.
            sprites: vec![
                sprite(-16.0, 16.0, 64.0, 64.0, region(0, 0, 8, 8)),
                sprite(16.0, 16.0, 64.0, 64.0, region(8, 0, 8, 8)),
                sprite(0.0, -8.0, 64.0, 64.0, region(0, 8, 8, 8)),
            ],
            camera: CameraView::IDENTITY,
        },
        Scene {
            name: "camera_regions_128x128",
            target: 128,
            texture: atlas(),
            sprites: vec![
                sprite(-12.0, 18.0, 32.0, 32.0, region(0, 0, 8, 8)),
                sprite(14.5, 18.0, 32.0, 32.0, region(8, 0, 8, 8)),
                sprite(6.0, -8.0, 32.0, 32.0, region(0, 8, 8, 8)),
            ],
            camera: CameraView::new(6.0, -2.0, 1.5),
        },
    ]
}

// --- the device and the pipeline ------------------------------------------

/// The adapter ladder of `gpu.rs`, replicated for the reason
/// `soft_edge_margin.rs` gives: it makes the adapter string comparable with the
/// rows already in `BASELINE.md`.
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

/// Copies a single-sample texture back as tightly packed RGBA8.
///
/// No padding strip: every blessed target is 64 or 128 wide, so a row is 256 or
/// 512 bytes and already a multiple of `COPY_BYTES_PER_ROW_ALIGNMENT`. The
/// assertion below makes a future scene of another width fail here rather than
/// read back sheared.
fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    edge: u32,
) -> Pixels {
    let bytes_per_row = edge * 4;
    assert_eq!(
        bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "a target edge of {edge} needs row padding; this file has no padding \
         strip and would read back a sheared image"
    );

    let transfer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("narvo linear read-back"),
        size: u64::from(bytes_per_row) * u64::from(edge),
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
                rows_per_image: Some(edge),
            },
        },
        wgpu::Extent3d {
            width: edge,
            height: edge,
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

    Pixels::from_rgba8(edge, edge, rgba).expect("the buffer matches the dimensions")
}

/// The scene's sprites in the form the production path takes them.
///
/// The same `Placed` table `scene_vertices` reads, turned into the public type
/// instead of into corners.
///
/// **What the control can and cannot see**, since a control that is vaguer than
/// its own coverage is worth little. Genuinely replicated, and therefore under
/// test — each is written twice, once here and once in `src/`:
///
/// - the corner table: `CORNERS` against `sprite.rs`'s `SPRITE_CORNERS`;
/// - the index order and its per-sprite offset, against `batch_index_buffer`;
/// - the uv interpolation: this file's `u_left + u * (u_right - u_left)` against
///   `TextureRegion::sample_at`, which is the same expression written twice;
/// - the shader, the pipeline state and the pass setup — blending, culling,
///   sample count, format, clear colour, resolve target.
///
/// All four are transcriptions rather than independent derivations, so what the
/// control proves is that the two copies have not **diverged** — not that either
/// is right. Being right is what the blessed images are for, and they are
/// checked by the five golden tests.
///
/// **Shared rather than replicated, and so proved by nothing here:**
/// `Projection::for_target(..).viewed_by(..)` and `TextureRegion::from_texels`
/// are public API that both sides call, so an error inside either moves both
/// sides together. What the control does catch is their *application* — the
/// wrong camera, the wrong target size, the wrong region for a sprite — because
/// that is chosen separately on each side. Their implementations are covered by
/// their own unit tests in `sprite.rs`, not by this file.
///
/// **Not covered at all:** rotation. `scene_vertices` asserts it is zero instead
/// of implementing it, so the sin/cos in `sprite_vertices` has no counterpart to
/// disagree with.
fn sprites_of(scene: &Scene) -> Vec<SpriteInstance> {
    scene
        .sprites
        .iter()
        .map(|placed| {
            let region = match placed.region {
                Some((left, top, width, height)) => {
                    TextureRegion::from_texels(left, top, width, height, &scene.texture)
                }
                None => TextureRegion::WHOLE_TEXTURE,
            };
            SpriteInstance::new(placed.placement, region)
        })
        .collect()
}

/// Renders `scene` through the production path, at the production sample count.
///
/// `targets` caches one [`OffscreenTarget`] per distinct edge length — there are
/// two across the five scenes, and each costs an adapter request and a device.
///
/// **The adapter is checked, not assumed.** This target picks its own adapter
/// through `gpu.rs`'s ladder and this file picks one through its replica of that
/// ladder. They agree in practice because the ladder is deterministic, but a
/// comparison across two rasterisers would be meaningless rather than merely
/// wrong, so it is asserted instead of hoped for.
fn production_render(
    targets: &mut Vec<(u32, OffscreenTarget)>,
    scene: &Scene,
    expected_adapter: &str,
) -> Pixels {
    if !targets.iter().any(|(edge, _)| *edge == scene.target) {
        let target = match OffscreenTarget::new(scene.target, scene.target) {
            Ok(target) => target,
            Err(RenderError::NoAdapter { attempts }) => panic!(
                "this file already holds a device, so the production path must \
                 find an adapter too. Tried: {attempts}"
            ),
            Err(other) => panic!("the production target could not be created: {other}"),
        };
        assert_eq!(
            target.adapter_summary(),
            expected_adapter,
            "the production path and this file's pipeline landed on different \
             adapters, so comparing their output would compare two rasterisers \
             rather than two geometries"
        );
        targets.push((scene.target, target));
    }

    let target = &targets
        .iter()
        .find(|(edge, _)| *edge == scene.target)
        .expect("inserted just above when it was missing")
        .1;

    target
        .render_sprites_viewed_by(&scene.texture, &sprites_of(scene), scene.camera)
        .expect("no blessed scene is anywhere near the batch limit")
}

/// The two numbers this file reports: how many pixels differ, and how far the
/// worst one moved.
struct Margin {
    differing: u64,
    worst: u8,
}

/// Compares two images the way `Golden` does, and keeps the distribution too.
///
/// The floor of 4 is `Tolerance::default().channel`, written out because this
/// file compares two renders rather than verifying against a reference and so
/// does not go through `Golden`. **No threshold is changed by writing it here**;
/// `the_floor_this_file_uses_is_the_one_golden_uses` pins the two together.
fn margin(left: &Pixels, right: &Pixels) -> Margin {
    assert_eq!(
        left.rgba().len(),
        right.rgba().len(),
        "two images of different sizes cannot be compared pixel for pixel"
    );

    let mut differing = 0;
    let mut worst = 0;
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
        worst = worst.max(deviation);
        if deviation > narvo_render2d::Tolerance::default().channel {
            differing += 1;
        }
    }

    Margin { differing, worst }
}

// --- the draw-order machinery ----------------------------------------------

/// The depth attachment's format, when there is one.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// `quad.wgsl` with a z on the position, so a depth test has something to
/// compare.
///
/// With no depth attachment the z is written and never read, which is what lets
/// the control below use this same shader at z = 0 and still reproduce the
/// production path byte for byte.
const DEPTH_SHADER: &str = r#"
struct Vertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(perspective, center) uv: vec2<f32>,
}
@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Vertex {
    var out: Vertex;
    out.clip_position = vec4<f32>(position, 1.0);
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

/// One draw call: the sprites in it, each with a z, and the sampler it uses.
struct Group {
    sprites: Vec<(SpriteInstance, f32)>,
    filter: wgpu::FilterMode,
}

/// Renders `groups` as one draw call each, optionally through a depth buffer.
///
/// `depth` of `None` is today's renderer: no attachment, no test, drawing order
/// alone decides. `Some(compare)` adds a depth attachment cleared to 1.0 with
/// writes enabled, so a smaller z is nearer.
#[expect(
    clippy::too_many_lines,
    reason = "one pipeline and one pass built inline; splitting it would hide \
              what it replicates from the production path it is checked against"
)]
fn render_groups(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_image: &Pixels,
    target_edge: u32,
    groups: &[Group],
    depth: Option<wgpu::CompareFunction>,
    camera: CameraView,
) -> Pixels {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("narvo draw-order shader"),
        source: wgpu::ShaderSource::Wgsl(DEPTH_SHADER.into()),
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
    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("narvo draw-order pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: size_of::<[f32; 5]>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: size_of::<[f32; 3]>() as wgpu::BufferAddress,
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
        depth_stencil: depth.map(|compare| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: SAMPLES,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    });

    let make = |count: u32, format: wgpu::TextureFormat, copyable: bool| {
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if copyable {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: target_edge,
                height: target_edge,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: count,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let resolved = make(1, FORMAT, true);
    let resolved_view = resolved.create_view(&wgpu::TextureViewDescriptor::default());
    let multisampled = make(SAMPLES, FORMAT, false);
    let ms_view = multisampled.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_texture = depth.map(|_| make(SAMPLES, DEPTH_FORMAT, false));
    let depth_view = depth_texture
        .as_ref()
        .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));

    // Built before the pass, so the pass can borrow them all at once.
    let projection = Projection::for_target(target_edge, target_edge).viewed_by(camera);
    let buffers: Vec<(wgpu::Buffer, wgpu::Buffer, u32, wgpu::BindGroup)> = groups
        .iter()
        .map(|group| {
            let mut corners: Vec<[f32; 5]> = Vec::with_capacity(group.sprites.len() * 4);
            for (sprite, z) in &group.sprites {
                let p = sprite.placement;
                assert_eq!(
                    (p.rot_cos, p.rot_sin),
                    SpritePlacement::UNTURNED,
                    "this file does not implement rotation"
                );
                let [u_left, v_top, u_right, v_bottom] = sprite.region.uv_bounds();
                for [local_x, local_y, u, v] in CORNERS {
                    let [ndc_x, ndc_y] = projection
                        .world_to_ndc(local_x * p.scale_x + p.x, local_y * p.scale_y + p.y);
                    corners.push([
                        ndc_x,
                        ndc_y,
                        *z,
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
            let mut index_bytes = Vec::new();
            for i in 0..group.sprites.len() {
                let base = u32::try_from(i * 4).expect("small");
                for index in INDICES {
                    index_bytes.extend_from_slice(&(base + u32::from(index)).to_ne_bytes());
                }
            }
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: None,
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: group.filter,
                min_filter: group.filter,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });
            (
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: &vertex_bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: &index_bytes,
                    usage: wgpu::BufferUsages::INDEX,
                }),
                u32::try_from(group.sprites.len() * 6).expect("small"),
                bind_group,
            )
        })
        .collect();

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("narvo draw-order pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &ms_view,
                depth_slice: None,
                resolve_target: Some(&resolved_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: depth_view.as_ref().map(|view| {
                wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        // One draw call per group. This loop is the whole subject of D15.
        for (vertices, indices, count, bind_group) in &buffers {
            pass.set_bind_group(0, Some(bind_group), &[]);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..*count, 0, 0..1);
        }
    }

    // Submitted here, and this line is load-bearing: `read_back` builds and
    // submits an encoder of its own for the copy, so without this the whole
    // render pass is dropped unrecorded and the read-back returns an untouched
    // buffer — [0, 0, 0, 0], not even the clear. CLAUDE.md records the same
    // shape from M1, where a frame was built correctly and never handed over.
    queue.submit(std::iter::once(encoder.finish()));

    read_back(device, queue, &resolved, target_edge)
}

// --- the two-sprite scene the demonstration turns on ------------------------

/// A 16 x 16 atlas whose left half is red and right half green, both uniform.
///
/// Uniform on purpose: inside a region `Nearest` and `Linear` return the same
/// value, so the sampler cannot be what changes a pixel and the only variable
/// left is which draw call a sprite is in.
fn flat_atlas() -> Pixels {
    let mut rgba = Vec::with_capacity((ATLAS * ATLAS * 4) as usize);
    for _ in 0..ATLAS {
        for x in 0..ATLAS {
            rgba.extend_from_slice(if x < ATLAS / 2 {
                &BACK_COLOUR
            } else {
                &FRONT_COLOUR
            });
        }
    }
    Pixels::from_rgba8(ATLAS, ATLAS, rgba).expect("the buffer matches its dimensions")
}

const BACK_COLOUR: [u8; 4] = [255, 0, 0, 255];
const FRONT_COLOUR: [u8; 4] = [0, 255, 0, 255];

/// The target edge for the two-sprite scene, the same 128 the blessed scenes use.
const DEMO_TARGET: u32 = 128;

/// Where the overlap is, derived from the placements rather than measured.
///
/// `BACK` at world (-8, 0) and `FRONT` at (8, 0), both 32 across, map through
/// `X = 64 + w` to columns 40..72 and 56..88. Both span rows 48..80. So the
/// overlap is columns 56..72 by rows 48..80, and (64, 64) is interior to both.
const OVERLAP_X: std::ops::Range<u32> = 56..72;
const OVERLAP_Y: std::ops::Range<u32> = 48..80;
const PROBE: (u32, u32) = (64, 64);

/// `(back, front)`, each with its own atlas half.
fn two_sprites(texture: &Pixels) -> (SpriteInstance, SpriteInstance) {
    let placed = |x: f32| SpritePlacement {
        x,
        y: 0.0,
        rot_cos: 1.0,
        rot_sin: 0.0,
        scale_x: 32.0,
        scale_y: 32.0,
    };
    (
        SpriteInstance::new(
            placed(-8.0),
            TextureRegion::from_texels(0, 0, 8, 8, texture),
        ),
        SpriteInstance::new(placed(8.0), TextureRegion::from_texels(8, 0, 8, 8, texture)),
    )
}

/// Which pixels of `left` and `right` differ at all, as a count and a bounding
/// box.
fn differing_box(left: &Pixels, right: &Pixels) -> (u64, Option<(u32, u32, u32, u32)>) {
    let mut count = 0_u64;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for y in 0..left.height() {
        for x in 0..left.width() {
            if left.pixel(x, y) != right.pixel(x, y) {
                count += 1;
                bounds = Some(match bounds {
                    None => (x, y, x, y),
                    Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                });
            }
        }
    }
    (count, bounds)
}

// --- the control ------------------------------------------------------------

/// This file's pipeline, at z = 0 with no depth buffer and one draw call, must
/// be the production path byte for byte, over all five blessed scenes.
///
/// **Called by every test here that publishes a measured GPU figure.** nextest gives
/// each test its own process, so a control that ran once for the file would not
/// have run for the others — M3.17 found that out and M3.18 recorded it.
///
/// It also validates the machinery this file adds: the `vec3` position, the
/// group loop and the read-back all sit on the path being compared.
fn control(device: &wgpu::Device, queue: &wgpu::Queue, adapter: &str) {
    let mut targets: Vec<(u32, OffscreenTarget)> = Vec::new();
    for scene in &scenes() {
        let groups = [Group {
            sprites: sprites_of(scene).into_iter().map(|s| (s, 0.0)).collect(),
            filter: wgpu::FilterMode::Nearest,
        }];
        let replicated = render_groups(
            device,
            queue,
            &scene.texture,
            scene.target,
            &groups,
            None,
            scene.camera,
        );
        let produced = production_render(&mut targets, scene, adapter);
        let check = margin(&produced, &replicated);
        assert_eq!(
            check.worst, 0,
            "{}: this file's one-group render is not byte-identical to the \
             production path's — worst {} counts over {} pixels. Everything below \
             would then be measuring the replication rather than the draw-call \
             split.",
            scene.name, check.worst, check.differing
        );
    }
}

// --- section 1: the incompatibility -----------------------------------------

/// **The demonstration.** Two draw calls put the wrong sprite in front.
#[test]
fn two_draw_calls_put_the_wrong_sprite_in_front() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };
    control(&device, &queue, &adapter);

    let texture = flat_atlas();
    let (back, front) = two_sprites(&texture);

    // One draw call, in depth order: back first, front second. This is what the
    // renderer does today, and it is the correct picture.
    let correct = render_groups(
        &device,
        &queue,
        &texture,
        DEMO_TARGET,
        &[Group {
            sprites: vec![(back, 0.0), (front, 0.0)],
            filter: wgpu::FilterMode::Nearest,
        }],
        None,
        CameraView::IDENTITY,
    );

    // Two draw calls, split by sampler. `front` uses `Nearest` and `back` uses
    // `Linear`, so batching by sampler puts `front` in the first call — and the
    // sprite that should be behind is drawn last.
    let split = render_groups(
        &device,
        &queue,
        &texture,
        DEMO_TARGET,
        &[
            Group {
                sprites: vec![(front, 0.0)],
                filter: wgpu::FilterMode::Nearest,
            },
            Group {
                sprites: vec![(back, 0.0)],
                filter: wgpu::FilterMode::Linear,
            },
        ],
        None,
        CameraView::IDENTITY,
    );

    let correct_probe = correct.pixel(PROBE.0, PROBE.1).expect("inside");
    let split_probe = split.pixel(PROBE.0, PROBE.1).expect("inside");
    let (differing, bounds) = differing_box(&correct, &split);

    println!("probe {PROBE:?}: one draw call {correct_probe:?}, two draw calls {split_probe:?}");
    println!("differing pixels: {differing}, bounding box {bounds:?}");

    assert_eq!(
        correct_probe, FRONT_COLOUR,
        "in depth order the front sprite must own the overlap"
    );
    assert_eq!(
        split_probe, BACK_COLOUR,
        "split into two draw calls, the sprite that should be *behind* owns the \
         overlap: it is in the second call, and with no depth buffer the second \
         call lands on top of the first whatever its depth. This is D15."
    );

    let expected = u64::from(OVERLAP_X.len() as u32) * u64::from(OVERLAP_Y.len() as u32);
    assert_eq!(
        differing,
        expected,
        "exactly the overlap must differ: {} columns by {} rows",
        OVERLAP_X.len(),
        OVERLAP_Y.len()
    );
    assert_eq!(
        bounds,
        Some((
            OVERLAP_X.start,
            OVERLAP_Y.start,
            OVERLAP_X.end - 1,
            OVERLAP_Y.end - 1
        )),
        "and it must be the overlap rectangle and nothing else"
    );
}

// --- section 2, way one: a depth buffer -------------------------------------

/// A depth buffer decides the order independently of the draw calls.
#[test]
fn a_depth_buffer_restores_the_order_across_draw_calls() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };
    control(&device, &queue, &adapter);

    let texture = flat_atlas();
    let (back, front) = two_sprites(&texture);

    let correct = render_groups(
        &device,
        &queue,
        &texture,
        DEMO_TARGET,
        &[Group {
            sprites: vec![(back, 0.0), (front, 0.0)],
            filter: wgpu::FilterMode::Nearest,
        }],
        None,
        CameraView::IDENTITY,
    );

    // The same wrong-order split, with a depth buffer and distinct depths:
    // smaller z is nearer, so `front` at 0.25 beats `back` at 0.75.
    let with_depth = render_groups(
        &device,
        &queue,
        &texture,
        DEMO_TARGET,
        &[
            Group {
                sprites: vec![(front, 0.25)],
                filter: wgpu::FilterMode::Nearest,
            },
            Group {
                sprites: vec![(back, 0.75)],
                filter: wgpu::FilterMode::Linear,
            },
        ],
        Some(wgpu::CompareFunction::Less),
        CameraView::IDENTITY,
    );

    let (differing, bounds) = differing_box(&correct, &with_depth);
    println!("depth buffer vs single draw call: {differing} differing, {bounds:?}");
    assert_eq!(
        differing, 0,
        "with distinct depths a depth buffer must reproduce the single-draw-call \
         picture exactly, whatever order the draw calls come in"
    );
}

/// What a depth buffer does to **equal** depths, which is what today's ties are.
///
/// Today `placements_of` breaks ties on `EntityId` and the later sprite wins by
/// being drawn later. Under a depth buffer that is a property of the compare
/// function, not of the sort.
#[test]
fn equal_depths_under_a_depth_buffer_depend_on_the_compare_function() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };
    control(&device, &queue, &adapter);

    let texture = flat_atlas();
    let (back, front) = two_sprites(&texture);

    // Drawn back-then-front at the *same* depth. That is not what the blessed
    // scenes do — four of them are `SpriteInstance` arrays handed straight to the
    // renderer with no depth at all, and `layer_order_regions_128x128` is built
    // from a world whose three entities carry *distinct* `Layer` depths. The
    // equal-depth case is what a world *without* `Layer` components does: every
    // entity sits at `Layer::DEFAULT.depth` and `placements_of` breaks the tie
    // on `EntityId`.
    let mut probes = Vec::new();
    for compare in [
        wgpu::CompareFunction::Less,
        wgpu::CompareFunction::LessEqual,
    ] {
        let image = render_groups(
            &device,
            &queue,
            &texture,
            DEMO_TARGET,
            &[Group {
                sprites: vec![(back, 0.5), (front, 0.5)],
                filter: wgpu::FilterMode::Nearest,
            }],
            Some(compare),
            CameraView::IDENTITY,
        );
        let probe = image.pixel(PROBE.0, PROBE.1).expect("inside");
        println!("equal depths under {compare:?}: probe {probe:?}");
        probes.push((compare, probe));
    }

    assert_eq!(
        probes[0].1, BACK_COLOUR,
        "under `Less`, the second sprite's fragments fail the test wherever the \
         first already wrote, so the *first* keeps the overlap — the opposite of \
         the painter's algorithm the renderer uses today. Outside the overlap the \
         second sprite draws normally, against the cleared depth"
    );
    assert_eq!(
        probes[1].1, FRONT_COLOUR,
        "under `LessEqual` the later sprite wins, which is today's behaviour"
    );
}

/// Does a depth buffer change the five blessed images?
///
/// Rendered with clip-space z *decreasing* by draw index — first drawn is
/// furthest — which is the inverse of `Layer::depth` (which `placements_of`
/// sorts ascending, lowest drawn first) and the mapping that reproduces the
/// painter's algorithm. Compared against the production path.
#[test]
fn a_depth_buffer_leaves_the_five_blessed_scenes_alone() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };
    control(&device, &queue, &adapter);

    let mut targets: Vec<(u32, OffscreenTarget)> = Vec::new();
    let mut moved: Vec<&str> = Vec::new();

    for scene in &scenes() {
        let sprites = sprites_of(scene);
        // Depth by draw index, nearer = smaller z. Three sprites use 0.75,
        // 0.583 and 0.417; the range is not spread to the ends.
        // The first-drawn sprite is furthest, which is what "drawn first means
        // behind" means once a depth test is doing the deciding.
        let count = sprites.len();
        #[expect(
            clippy::cast_precision_loss,
            reason = "no blessed scene has more than three sprites"
        )]
        let with_depth: Vec<(SpriteInstance, f32)> = sprites
            .into_iter()
            .enumerate()
            .map(|(i, s)| (s, 0.75 - 0.5 * (i as f32) / (count as f32)))
            .collect();

        let rendered = render_groups(
            &device,
            &queue,
            &scene.texture,
            scene.target,
            &[Group {
                sprites: with_depth,
                filter: wgpu::FilterMode::Nearest,
            }],
            Some(wgpu::CompareFunction::Less),
            scene.camera,
        );
        let produced = production_render(&mut targets, scene, &adapter);
        let measured = margin(&produced, &rendered);
        println!(
            "{}: depth buffer vs production — {} differing, worst {}",
            scene.name, measured.differing, measured.worst
        );
        if measured.worst > 0 {
            moved.push(scene.name);
        }
    }

    println!(
        "blessed scenes moved by a depth buffer: {} — {moved:?}",
        moved.len()
    );
    assert!(
        moved.is_empty(),
        "a depth buffer with depth by draw index must reproduce the painter's \
         algorithm exactly, and did not for {moved:?}"
    );
}

/// What the depth attachment costs in memory, at the sizes this repository uses.
///
/// Runs without a GPU: it is arithmetic over the format's own block size, not a
/// driver allocation. `Depth32Float` is four bytes per sample and the attachment
/// is multisampled like the colour one.
#[test]
fn the_depth_attachment_costs_four_bytes_per_sample() {
    let bytes = DEPTH_FORMAT
        .block_copy_size(Some(wgpu::TextureAspect::DepthOnly))
        .expect("a depth format has a block size");
    assert_eq!(bytes, 4);

    for (w, h) in [(128_u64, 128_u64), (1920, 1080)] {
        let nominal = w * h * u64::from(SAMPLES) * u64::from(bytes);
        #[expect(
            clippy::cast_precision_loss,
            reason = "a megabyte figure needs three significant digits"
        )]
        let mib = nominal as f64 / (1024.0 * 1024.0);
        // Two decimals: 262 144 B is 0.25 MiB, and one decimal rounds the tie to
        // 0.2, understating it by a fifth.
        println!("depth attachment at {w} x {h}: {nominal} bytes ({mib:.2} MiB)");
    }
}
