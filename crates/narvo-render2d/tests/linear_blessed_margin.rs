//! What `Linear` would cost the five blessed images, and the golden-image
//! strategy behind them.
//!
//! # The question
//!
//! D13 is the sampler question. M3.17 measured what `Linear` *solves* — the
//! interior quantum `Nearest` cannot reach — and what padding costs. What nobody
//! has measured is the price at the **verification strategy**.
//!
//! Two facts set it up, both already recorded and neither re-derived here.
//! M3.14 measured that under `Linear` two rasterisers disagree past the pixel
//! budget by a factor of 1.77, on a scene built to be hard. And the
//! section "The blessed images: one of five moved" records all five as today
//! **byte-identical** across three rasterisers, which is what lets one reference
//! serve all three.
//!
//! So: how many of the five stay byte-identical under `Linear`?
//!
//! # Two comparisons, reported apart
//!
//! They answer different questions and a single table would conflate them.
//!
//! - **Against the blessed reference.** How far each image moves, which is how
//!   many would need a fresh blessing.
//! - **Between rasterisers.** Whether they still agree, which is whether one
//!   reference can still serve three. That is the question the golden-image
//!   strategy hangs on, and it is the more important of the two.
//!
//! The second needs renders from machines this process cannot reach, so it works
//! the way `soft_edge_margin.rs` does: every run writes its renders, and a run
//! pointed at another run's directory by `NARVO_LINEAR_BASELINE` compares
//! against them.
//!
//! # Outside the production path, and checked against it
//!
//! A test-only pipeline, production's in every respect but the sampler. **The
//! replication is checked live** against
//! `OffscreenTarget::render_sprites_viewed_by` at `Nearest`, byte for byte, in
//! every test that publishes a *measured* figure — nextest gives each test its
//! own process,
//! so a control that ran once for the file would not have run for the others.
//! M3.17's own self-audit found a control that ran in one test of three and
//! fixed it before that task shipped; this file is built that way from the start.
//!
//! `tests/golden/` is read and never written.

use std::path::{Path, PathBuf};

use narvo_render2d::{
    CameraView, OffscreenTarget, Pixels, Projection, RenderError, SpriteBatch, SpriteFilter,
    SpriteInstance, SpritePlacement, SpriteTint, TextureRegion, golden_artifact_dir,
};
use wgpu::util::DeviceExt as _;

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Point this at another run's output directory and this run compares against
/// it, which is how three rasterisers are compared from three machines.
const BASELINE_VAR: &str = "NARVO_LINEAR_BASELINE";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

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

/// The production shader, replicated. `quad.wgsl` passes both attributes
/// straight through and samples once; so does this.
///
/// **Including the `center` qualifier**, which is not decoration and is not the
/// default spelled out for tidiness: it is D17, and production writes it out for
/// the same reason. The two qualifiers differ only where a pixel is partly
/// covered, which is exactly the class this file measures, so a replica left on
/// `centroid` would measure a shader the renderer does not have. **The control
/// catches that omission**, which it could not before M3.15a: it compares at
/// `SAMPLES` rather than at one sample. M3.26 demonstrated it — production
/// flipped, this replica held back — and
/// `production_at_linear_is_byte_identical_to_this_file_s_linear` went red on
/// `textured_quad_quadrants_64x64` at worst 20 counts over 7 pixels. All eight
/// pixels that differed at all sat on the quad's own diagonal, `x == y`, at
/// (28, 28) through (35, 35): the seam class exactly, and nothing else.
/// This file exists to say how far the blessed images move under `Linear`.
///
/// **The tint varying carries `@interpolate(flat, first)`, and that is D17's
/// argument one attribute along.** Production writes it out for the reason it
/// writes out `center`: naga's other admissible sampling for `flat` is `Either`,
/// whose "exact choice is implementation-dependent"
/// (naga-30.0.0/src/ir/mod.rs:647-648), and a measurement that rests on
/// rasterisers agreeing cannot anchor on a point no specification pins. A
/// replica left on the default would be measuring a shader the renderer does not
/// have — the same omission M3.26 demonstrated for `centroid`, and the control
/// catches it the same way.
const SHADER: &str = r#"
struct Vertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(perspective, center) uv: vec2<f32>,
    @location(1) @interpolate(flat, first) tint: vec4<f32>,
}

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
) -> Vertex {
    var out: Vertex;
    out.clip_position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.tint = tint;
    return out;
}

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;

@fragment
fn fs_main(input: Vertex) -> @location(0) vec4<f32> {
    return textureSample(scene_texture, scene_sampler, input.uv) * input.tint;
}
"#;

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
    /// The colour its texels are multiplied by (M6b.3).
    ///
    /// [`SpriteTint::UNTINTED`] for every scene blessed before M6b.3, which is
    /// what keeps their rows in this file's table where they were: the identity
    /// is exact in IEEE-754, so a replica that now carries a tint attribute
    /// still renders those four scenes byte for byte as it did.
    tint: SpriteTint,
}

/// One blessed scene, everything needed to draw it.
struct Scene {
    /// The name the reference is blessed under.
    name: &'static str,
    /// Where the reference lives, relative to the workspace root.
    reference: &'static str,
    /// Target width. Square until M3.35's text scene, which is 192 x 80.
    width: u32,
    /// Target height.
    height: u32,
    /// The texture every sprite in this scene samples.
    texture: Pixels,
    /// The sprites, in draw order.
    sprites: Vec<Placed>,
    /// The view the **scene** batch is seen through.
    ///
    /// M3.12's camera scene and M6b.4's HUD scene are the two with a
    /// non-identity one. The sentence used to name only the first, and M6b.4
    /// moving it is the point rather than an aside: a screen-fixed layer is
    /// indistinguishable from a world-fixed one under the identity camera, so
    /// its reference could not have been blessed at the identity.
    camera: CameraView,
    /// A second batch, drawn after the first through a camera of its own.
    ///
    /// `None` for every scene blessed before M6b.4, which is what keeps their
    /// rows measuring exactly what they measured: `scene_vertices` appends
    /// nothing and `production_render` calls the one-batch entry point, so not a
    /// byte of those ten renders goes near the second batch.
    overlay: Option<Overlay>,
}

/// The overlay batch of a scene that has one: sprites, and the view they are
/// seen through (M6b.4).
///
/// It samples the **scene's** texture. Two batches from one texture is what the
/// blessed scene does, and it is why this file needs no second bind group: the
/// replica's pipeline binds once and draws every vertex, exactly as it did
/// before. Which texture a batch binds is `two_textures.rs`'s subject; what is
/// transcribed here is which *camera* it is seen through, because that is the
/// choice M6b.4 made and the one thing the control below can catch.
struct Overlay {
    /// The sprites, in draw order, after the scene's.
    sprites: Vec<Placed>,
    /// The view. `CameraView::IDENTITY` is the screen-fixed layer.
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
        tint: SpriteTint::UNTINTED,
    }
}

/// The same sprite, multiplied by `tint`.
fn tinted(placed: Placed, tint: SpriteTint) -> Placed {
    Placed { tint, ..placed }
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
/// The click-counter scene's texture: 32 px glyphs with the colour strip beside
/// them (M5.5).
fn counter_texture() -> Pixels {
    let glyphs = narvo_testkit::glyph_atlas::rasterize(32.0);
    narvo_testkit::blend::ground_beside(glyphs.pixels()).0
}

/// M5.5's click-counter scene at counter three, transcribed from
/// `narvo-app`'s `sprite_batch.rs`.
///
/// Backdrop, button, then the digit. The two solid sprites are transcribed by
/// hand — their regions are stated in texels here rather than taken from a
/// helper, so this file's copy is its own statement of where the strip is. The
/// digit's placement is **not**: it comes from `narvo_testkit::text`, the same
/// scoped exception this file already takes for `text_over_scene`, and it costs
/// the same thing — a layout defect would move both sides of this file's
/// comparison together and go unseen here. What limits the cost is unchanged:
/// this file measures a margin between two render paths over a fixed geometry,
/// and placement correctness is pinned by the testkit's own hand-computed tests
/// and by the blessed image.
fn counter_sprites() -> Vec<Placed> {
    use narvo_testkit::blend::CELL_TEXELS;

    let glyphs = narvo_testkit::glyph_atlas::rasterize(32.0);
    let texture = counter_texture();
    let strip = glyphs.pixels().width();
    let height = texture.height();

    let mut sprites = vec![
        // The backdrop is the strip's second cell, spanning the whole height
        // because `ground_beside` repeats each cell's row all the way down.
        Placed {
            placement: SpritePlacement {
                x: 0.0,
                y: 0.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 128.0,
                scale_y: 128.0,
            },
            region: Some((strip + CELL_TEXELS, 0, CELL_TEXELS, height)),
            tint: SpriteTint::UNTINTED,
        },
        // The button is the first cell, and it sits where the scene's `HitRect`
        // covers.
        Placed {
            placement: SpritePlacement {
                x: 0.0,
                y: -32.0,
                rot_cos: 1.0,
                rot_sin: 0.0,
                scale_x: 64.0,
                scale_y: 64.0,
            },
            region: Some((strip, 0, CELL_TEXELS, height)),
            tint: SpriteTint::UNTINTED,
        },
    ];

    let placed = narvo_testkit::text::layout_line("3", &glyphs, 52.0, 40.0);
    sprites.extend(
        narvo_testkit::text::sprites_for(&placed, &texture, 128, 128)
            .iter()
            .zip(&placed)
            .map(|(drawn, glyph)| Placed {
                placement: drawn.placement,
                region: Some((
                    glyph.region.left,
                    glyph.region.top,
                    glyph.region.width,
                    glyph.region.height,
                )),
                tint: SpriteTint::UNTINTED,
            }),
    );

    sprites
}

fn scenes() -> Vec<Scene> {
    let region = |l, t, w, h| Some((l, t, w, h));
    vec![
        Scene {
            name: "textured_quad_quadrants_64x64",
            reference: "crates/narvo-render2d/tests/golden",
            width: 64,
            height: 64,
            texture: narvo_testkit::quadrant_texture(8),
            sprites: vec![sprite(0.0, 0.0, 64.0, 64.0, None)],
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "placed_sprite_quadrants_128x128",
            reference: "crates/narvo-render2d/tests/golden",
            width: 128,
            height: 128,
            texture: narvo_testkit::quadrant_texture(8),
            sprites: vec![sprite(16.0, -8.0, 48.0, 32.0, None)],
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "sprite_atlas_regions_128x128",
            reference: "crates/narvo-render2d/tests/golden",
            width: 128,
            height: 128,
            texture: atlas(),
            sprites: vec![
                sprite(-32.0, 32.0, 32.0, 32.0, region(0, 0, 8, 8)),
                sprite(24.0, 32.0, 48.0, 24.0, region(8, 0, 8, 8)),
                sprite(-16.0, -40.0, 24.0, 40.0, region(0, 8, 8, 8)),
            ],
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "layer_order_regions_128x128",
            reference: "crates/narvo-app/tests/golden",
            width: 128,
            height: 128,
            texture: atlas(),
            // Draw order after `placements_of` sorts by depth: A, B, C.
            sprites: vec![
                sprite(-16.0, 16.0, 64.0, 64.0, region(0, 0, 8, 8)),
                sprite(16.0, 16.0, 64.0, 64.0, region(8, 0, 8, 8)),
                sprite(0.0, -8.0, 64.0, 64.0, region(0, 8, 8, 8)),
            ],
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "camera_regions_128x128",
            reference: "crates/narvo-render2d/tests/golden",
            width: 128,
            height: 128,
            texture: atlas(),
            sprites: vec![
                sprite(-12.0, 18.0, 32.0, 32.0, region(0, 0, 8, 8)),
                sprite(14.5, 18.0, 32.0, 32.0, region(8, 0, 8, 8)),
                sprite(6.0, -8.0, 32.0, 32.0, region(0, 8, 8, 8)),
            ],
            camera: CameraView::new(6.0, -2.0, 1.5),
            overlay: None,
        },
        Scene {
            name: "text_lines_ascii_192x80",
            reference: "crates/narvo-render2d/tests/golden",
            width: 192,
            height: 80,
            texture: text_texture(),
            sprites: text_sprites(),
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "blend_proof_steps_128x128",
            reference: "crates/narvo-render2d/tests/golden",
            width: 128,
            height: 128,
            texture: narvo_testkit::blend::atlas(),
            sprites: blend_sprites(),
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "click_counter_state3_128x128",
            reference: "crates/narvo-app/tests/golden",
            width: 128,
            height: 128,
            texture: counter_texture(),
            // Backdrop, then button (depth 1), then the digit's glyphs. The
            // glyph placements come from `narvo_testkit::text`, the scoped
            // exception this file already takes for `text_over_scene`.
            sprites: counter_sprites(),
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "text_over_scene_192x80",
            reference: "crates/narvo-render2d/tests/golden",
            width: 192,
            height: 80,
            texture: text_over_scene_texture(),
            sprites: text_over_scene_sprites(),
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "tint_over_ground_128x128",
            reference: "crates/narvo-render2d/tests/golden",
            width: 128,
            height: 128,
            texture: tint_texture(),
            sprites: tint_sprites(),
            camera: CameraView::IDENTITY,
            overlay: None,
        },
        Scene {
            name: "hud_over_moving_scene_128x128",
            reference: "crates/narvo-render2d/tests/golden",
            width: 128,
            height: 128,
            texture: hud_texture(),
            sprites: hud_world_sprites(),
            camera: HUD_SCENE_CAMERA,
            overlay: Some(Overlay {
                sprites: hud_overlay_sprites(),
                camera: CameraView::IDENTITY,
            }),
        },
    ]
}

/// M6b.4's HUD scene, transcribed from `screen_fixed.rs`.
///
/// The **padded** atlas, unlike this file's own `atlas()` measurement baseline:
/// the scene under `Linear` samples across region edges, and D13's border is
/// what keeps that from reading a neighbouring cell's colour. Transcribed as the
/// layout constant rather than as texel literals, because
/// `the_cells_this_file_samples_are_the_padded_layouts` in `screen_fixed.rs`
/// holds the literals against the layout and there is no second place for them
/// to drift.
fn hud_texture() -> Pixels {
    narvo_testkit::AtlasLayout::PADDED.atlas()
}

/// The scene camera of M6b.4's reference. Not the identity, and that is the
/// point: under the identity a screen-fixed batch and a world-fixed one draw the
/// same picture.
const HUD_SCENE_CAMERA: CameraView = CameraView::new(24.0, -16.0, 1.0);

/// The world batch: red at the origin, green 48 units to its right.
fn hud_world_sprites() -> Vec<Placed> {
    vec![
        sprite(0.0, 0.0, 32.0, 32.0, Some((1, 1, 8, 8))),
        sprite(48.0, 0.0, 32.0, 32.0, Some((11, 1, 8, 8))),
    ]
}

/// The screen-fixed batch: a bar low in the frame and a badge in the upper left.
fn hud_overlay_sprites() -> Vec<Placed> {
    vec![
        sprite(0.0, -48.0, 96.0, 16.0, Some((1, 11, 8, 8))),
        sprite(-48.0, 48.0, 16.0, 16.0, Some((11, 11, 8, 8))),
    ]
}

/// M6b.3's tint fixture, transcribed from `tint.rs`.
///
/// 16 x 16, opaque white in the left half and opaque grey in the right. The
/// halves are wide enough that every region below keeps two texels of
/// same-coloured margin, which is what makes the scene safe to measure at
/// `Linear` as well: a bilinear fetch at a region edge blends two texels of the
/// same colour.
fn tint_texture() -> Pixels {
    let edge = 16;
    let mut rgba = Vec::with_capacity((edge * edge * 4) as usize);
    for _ in 0..edge {
        for x in 0..edge {
            let value = if x < edge / 2 { 255 } else { 128 };
            rgba.extend_from_slice(&[value, value, value, 255]);
        }
    }

    Pixels::from_rgba8(edge, edge, rgba).expect("16x16 is a legal size")
}

/// M6b.3's tint scene, transcribed from `tint.rs`.
///
/// The grey ground over the whole canvas, then four 48 x 40 sprites of the white
/// half: opaque untinted, opaque tinted, half coverage untinted, half coverage
/// tinted. The fourth is the one the scene exists for — a tint whose alpha and
/// whose colour both act, over a ground bright enough for a broken
/// premultiplication to saturate.
fn tint_sprites() -> Vec<Placed> {
    let white = Some((2, 2, 4, 12));
    let grey = Some((10, 2, 4, 12));
    let orange = SpriteTint::rgb(1.0, 0.5, 0.125);
    let fade = SpriteTint {
        red: 1.0,
        green: 1.0,
        blue: 1.0,
        alpha: 0.5,
    };
    let faded_orange = SpriteTint {
        red: 1.0,
        green: 0.5,
        blue: 0.125,
        alpha: 0.5,
    };

    vec![
        sprite(0.0, 0.0, 128.0, 128.0, grey),
        sprite(-30.0, 28.0, 48.0, 40.0, white),
        tinted(sprite(30.0, 28.0, 48.0, 40.0, white), orange),
        tinted(sprite(-30.0, -28.0, 48.0, 40.0, white), fade),
        tinted(sprite(30.0, -28.0, 48.0, 40.0, white), faded_orange),
    ]
}

/// M4.7's blend proof, transcribed from `blend_proof.rs`.
///
/// Ten sprites in draw order: the red ground over the whole canvas, the black
/// band under the lower row, then four alpha steps in each row. The fixture is
/// a strip of six 8-texel cells, so a cell's region is its index times eight.
///
/// **This scene is the first whose draw order is composition.** Until M4.7 a
/// later sprite simply replaced an earlier one and a transcription that got the
/// order wrong would still have produced the same picture wherever nothing
/// overlapped. Here the ground is under everything by construction, so an order
/// error changes the image — which makes this transcription load-bearing in a
/// way the five before it were not.
fn blend_sprites() -> Vec<Placed> {
    let cell = |index: u32| Some((index * 8, 0, 8, 8));

    let mut sprites = vec![
        sprite(0.0, 0.0, 128.0, 128.0, cell(0)),
        sprite(0.0, -32.0, 128.0, 32.0, cell(1)),
    ];

    // Columns left to right, steps 0, 85, 170, 255 in cells 2 to 5. Each step
    // is drawn once over the red row and once over the black one.
    for (x, step) in [(-48.0, 2), (-16.0, 3), (16.0, 4), (48.0, 5)] {
        sprites.push(sprite(x, 32.0, 32.0, 32.0, cell(step)));
        sprites.push(sprite(x, -32.0, 32.0, 32.0, cell(step)));
    }

    sprites
}

/// The stacked glyph atlases with the blend fixture's ground appended.
///
/// Calls `narvo_testkit::blend::ground_beside` rather than transcribing the
/// appending, which is the scoped exception that function documents — the same
/// shape as this file's existing exception for `narvo_testkit::text`.
fn text_over_scene_texture() -> Pixels {
    let (glyphs, _) = stacked_glyph_atlases();
    narvo_testkit::blend::ground_beside(&glyphs).0
}

/// The stacked glyph atlases and the row the 32 px one starts at.
fn stacked_glyph_atlases() -> (Pixels, u32) {
    let small = narvo_testkit::glyph_atlas::rasterize(16.0);
    let large = narvo_testkit::glyph_atlas::rasterize(32.0);

    let (width, small_h) = (small.pixels().width(), small.pixels().height());
    let height = small_h + large.pixels().height();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    rgba.extend_from_slice(small.pixels().rgba());
    rgba.extend_from_slice(large.pixels().rgba());

    (
        Pixels::from_rgba8(width, height, rgba).expect("the stacked buffer matches its dimensions"),
        small_h,
    )
}

/// M4.7's text-over-a-ground scene, transcribed from `text_over_scene.rs`.
///
/// The ground first, then the glyphs. Its strings are this file's own copy.
fn text_over_scene_sprites() -> Vec<Placed> {
    use narvo_testkit::text;

    let small = narvo_testkit::glyph_atlas::rasterize(16.0);
    let large = narvo_testkit::glyph_atlas::rasterize(32.0);
    let (glyphs, offset) = stacked_glyph_atlases();
    let (texture, _) = narvo_testkit::blend::ground_beside(&glyphs);

    let mut placed = text::layout_line("over a scene 16", &small, 8.0, 24.0);
    for glyph in text::layout_line("over 32", &large, 8.0, 64.0) {
        let mut glyph = glyph;
        glyph.region.top += offset;
        placed.push(glyph);
    }

    // The ground column sits immediately right of the glyph texture, is one
    // cell wide, and spans the whole height — stated here in texels rather than
    // taken from the region the fixture returns, so that this file's copy is its
    // own statement of where the ground is.
    let mut sprites = vec![Placed {
        placement: SpritePlacement {
            x: 0.0,
            y: 0.0,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: 192.0,
            scale_y: 80.0,
        },
        region: Some((
            glyphs.width(),
            0,
            narvo_testkit::blend::CELL_TEXELS,
            glyphs.height(),
        )),
        tint: SpriteTint::UNTINTED,
    }];

    sprites.extend(
        text::sprites_for(&placed, &texture, 192, 80)
            .iter()
            .zip(&placed)
            .map(|(drawn, glyph)| Placed {
                placement: drawn.placement,
                region: Some((
                    glyph.region.left,
                    glyph.region.top,
                    glyph.region.width,
                    glyph.region.height,
                )),
                tint: SpriteTint::UNTINTED,
            }),
    );

    sprites
}

/// The stacked atlas the text scene samples, and its two lines' glyphs.
///
/// **These two functions call `narvo_testkit::text`, and that is a departure
/// from this file's rule.** Every other scene here is transcribed by hand so
/// that the control and the production path share no arithmetic. Twenty-one
/// glyph placements transcribed twice — once in each margin file — would be the
/// duplication D16 exists to prevent, and every one of them would have to be
/// recomputed whenever the font, the atlas packing or the baseline moved.
///
/// What that costs is stated rather than hidden: **a layout defect would move
/// both sides of this file's comparison together and go unseen here.** It is
/// acceptable because this file measures a *margin between two render paths*
/// over a fixed geometry, not whether the geometry is right; placement
/// correctness is pinned by `narvo_testkit::text`'s own hand-computed tests
/// and by the blessed image itself.
fn text_texture() -> Pixels {
    let small = narvo_testkit::glyph_atlas::rasterize(16.0);
    let large = narvo_testkit::glyph_atlas::rasterize(32.0);

    let width = small.pixels().width();
    let height = small.pixels().height() + large.pixels().height();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    rgba.extend_from_slice(small.pixels().rgba());
    rgba.extend_from_slice(large.pixels().rgba());

    Pixels::from_rgba8(width, height, rgba).expect("the stacked buffer matches its dimensions")
}

fn text_sprites() -> Vec<Placed> {
    use narvo_testkit::text;

    let small = narvo_testkit::glyph_atlas::rasterize(16.0);
    let large = narvo_testkit::glyph_atlas::rasterize(32.0);
    let offset = small.pixels().height();
    let texture = text_texture();

    // "Amboss", not "Narvo": this rebuilds the blessed `text_lines_ascii_192x80`
    // scene, whose reference holds these glyphs as pixels. See the note on
    // `LINE_16` in `text_lines.rs` (U1a).
    let mut placed = text::layout_line("Amboss 16 gjpq!", &small, 8.0, 24.0);
    for glyph in text::layout_line("Amboss 32", &large, 8.0, 64.0) {
        let mut glyph = glyph;
        glyph.region.top += offset;
        placed.push(glyph);
    }

    text::sprites_for(&placed, &texture, 192, 80)
        .iter()
        .zip(&placed)
        .map(|(drawn, glyph)| Placed {
            placement: drawn.placement,
            region: Some((
                glyph.region.left,
                glyph.region.top,
                glyph.region.width,
                glyph.region.height,
            )),
            tint: SpriteTint::UNTINTED,
        })
        .collect()
}

/// A scene's corners as `[ndc_x, ndc_y, u, v, r, g, b, a]`, four per sprite.
///
/// The replicated half of the production path. Rotation is asserted to be zero
/// rather than implemented: every blessed scene has it, and a sin/cos here that
/// nothing exercises would be a second place for the convention to drift.
///
/// **The last four are the premultiplied tint**, and unlike the rotation this
/// *is* implemented rather than asserted away — because a blessed scene now
/// carries one. The premultiplication is written out here in full rather than
/// calling `SpriteTint::premultiplied`, which is what makes the control a
/// comparison of two statements of the arithmetic instead of one statement
/// compared against itself.
fn scene_vertices(scene: &Scene) -> Vec<[f32; 8]> {
    let projection = Projection::for_target(scene.width, scene.height).viewed_by(scene.camera);
    let mut vertices = batch_corners(&scene.sprites, &scene.texture, projection);

    // M6b.4: an overlay is a second batch through a camera of its own, and its
    // projection is **this** projection with the camera field replaced — the
    // spelling production uses, for the reason production uses it: the two
    // batches then share the target's half-extents by construction rather than
    // by two calls to `for_target` agreeing.
    //
    // This is the transcription the control earns its keep on. A projection is
    // shared code and proved by nothing here; *which camera goes to which
    // batch* is chosen separately on each side, and that choice is the whole of
    // what M6b.4 decided.
    if let Some(overlay) = &scene.overlay {
        vertices.extend(batch_corners(
            &overlay.sprites,
            &scene.texture,
            projection.viewed_by(overlay.camera),
        ));
    }

    vertices
}

/// One batch's corners, through one projection.
fn batch_corners(sprites: &[Placed], texture: &Pixels, projection: Projection) -> Vec<[f32; 8]> {
    let mut vertices = Vec::with_capacity(sprites.len() * 4);

    for placed in sprites {
        let p = placed.placement;
        assert_eq!(
            (p.rot_cos, p.rot_sin),
            SpritePlacement::UNTURNED,
            "no blessed scene is rotated; this file does not implement rotation"
        );

        let bounds = match placed.region {
            Some((l, t, w, h)) => TextureRegion::from_texels(l, t, w, h, texture).uv_bounds(),
            None => TextureRegion::WHOLE_TEXTURE.uv_bounds(),
        };
        let [u_left, v_top, u_right, v_bottom] = bounds;

        // `out_rgb = t_rgb * t_a`, `out_a = t_a` — the derivation on
        // `SpriteTint`, written out rather than called.
        let t = placed.tint;
        let tint = [
            t.red * t.alpha,
            t.green * t.alpha,
            t.blue * t.alpha,
            t.alpha,
        ];

        for [local_x, local_y, u, v] in CORNERS {
            let [ndc_x, ndc_y] =
                projection.world_to_ndc(local_x * p.scale_x + p.x, local_y * p.scale_y + p.y);
            vertices.push([
                ndc_x,
                ndc_y,
                u_left + u * (u_right - u_left),
                v_top + v * (v_bottom - v_top),
                tint[0],
                tint[1],
                tint[2],
                tint[3],
            ]);
        }
    }

    vertices
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

/// Renders `scene` at [`SAMPLES`] samples per pixel, with `filter` as the
/// sampler's magnification and minification mode.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene: &Scene,
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

    // The one variable this file changes. Everything else is production's.
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
        label: Some("narvo linear measurement texture"),
        size: wgpu::Extent3d {
            width: scene.texture.width(),
            height: scene.texture.height(),
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
        scene.texture.rgba(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(scene.texture.width() * 4),
            rows_per_image: Some(scene.texture.height()),
        },
        wgpu::Extent3d {
            width: scene.texture.width(),
            height: scene.texture.height(),
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

    let corners = scene_vertices(scene);
    let vertex_bytes: Vec<u8> = corners
        .iter()
        .flatten()
        .flat_map(|c| c.to_ne_bytes())
        .collect();
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("narvo linear measurement vertices"),
        contents: &vertex_bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });

    // **Every quad in the buffer, not every quad in the scene batch.** M6b.4
    // appended a second batch's corners above, and a count taken from
    // `scene.sprites` alone would leave them unreferenced: the vertices would be
    // uploaded and never drawn. Production takes the same total —
    // `batch_index_buffer(device, sprites.len() + overlay_len)` — and the control
    // is what caught the first version of this line, which did not.
    let quads = corners.len() / 4;
    let mut index_bytes = Vec::new();
    for i in 0..quads {
        let base = u32::try_from(i * 4).expect("five scenes stay inside u32");
        for index in INDICES {
            index_bytes.extend_from_slice(&(base + u32::from(index)).to_ne_bytes());
        }
    }
    let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("narvo linear measurement indices"),
        contents: &index_bytes,
        usage: wgpu::BufferUsages::INDEX,
    });
    let index_count = u32::try_from(quads * 6).expect("no blessed scene is anywhere near u32");

    let make_target = |sample_count: u32, copyable: bool| {
        let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if copyable {
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("narvo linear measurement target"),
            size: wgpu::Extent3d {
                width: scene.width,
                height: scene.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage,
            view_formats: &[],
        })
    };

    let resolved = make_target(1, true);
    let resolved_view = resolved.create_view(&wgpu::TextureViewDescriptor::default());
    let multisampled = make_target(SAMPLES, false);
    let ms_view = multisampled.create_view(&wgpu::TextureViewDescriptor::default());

    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("narvo linear measurement pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &ms_view,
                depth_slice: None,
                resolve_target: Some(&resolved_view),
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
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..index_count, 0, 0..1);
    }
    queue.submit(std::iter::once(encoder.finish()));

    read_back(device, queue, &resolved, scene.width, scene.height)
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
    width: u32,
    height: u32,
) -> Pixels {
    let bytes_per_row = width * 4;
    assert_eq!(
        bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "a target width of {width} needs row padding; this file has no padding \
         strip and would read back a sheared image"
    );

    let transfer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("narvo linear read-back"),
        size: u64::from(bytes_per_row) * u64::from(height),
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
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
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

    Pixels::from_rgba8(width, height, rgba).expect("the buffer matches the dimensions")
}

/// The workspace root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf()
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
    instances_of(&scene.sprites, &scene.texture)
}

/// The overlay batch as production instances, or nothing when there is no
/// overlay.
///
/// **Empty and absent are the same thing**, which is ADR-0039's rule and not a
/// convenience here: `production_render` decides between the two entry points on
/// `scene.overlay` rather than on this being empty, so a scene without an
/// overlay reaches the one-batch call it always reached.
fn overlay_sprites_of(scene: &Scene) -> Vec<SpriteInstance> {
    scene
        .overlay
        .as_ref()
        .map(|overlay| instances_of(&overlay.sprites, &scene.texture))
        .unwrap_or_default()
}

/// One batch's `Placed` rows as production instances against `texture`.
fn instances_of(sprites: &[Placed], texture: &Pixels) -> Vec<SpriteInstance> {
    sprites
        .iter()
        .map(|placed| {
            let region = match placed.region {
                Some((left, top, width, height)) => {
                    TextureRegion::from_texels(left, top, width, height, texture)
                }
                None => TextureRegion::WHOLE_TEXTURE,
            };
            SpriteInstance::new(placed.placement, region).tinted(placed.tint)
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
    targets: &mut Vec<((u32, u32), OffscreenTarget)>,
    scene: &Scene,
    expected_adapter: &str,
    wish: SpriteFilter,
) -> Pixels {
    let size = (scene.width, scene.height);
    if !targets.iter().any(|(cached, _)| *cached == size) {
        let target = match OffscreenTarget::new(scene.width, scene.height) {
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
        targets.push((size, target));
    }

    let target = &targets
        .iter()
        .find(|(cached, _)| *cached == size)
        .expect("inserted just above when it was missing")
        .1;

    let sprites: Vec<SpriteInstance> = sprites_of(scene)
        .into_iter()
        .map(|sprite| sprite.sampled(wish))
        .collect();

    // M6b.4: a scene with an overlay goes through the two-batch entry point, and
    // a scene without one goes through the call it has always gone through. The
    // branch is on `scene.overlay` rather than on an empty slice so that the ten
    // scenes blessed before M6b.4 reach `render_sprites_viewed_by` itself — the
    // same function, with the same arguments, that produced their references.
    match &scene.overlay {
        None => target.render_sprites_viewed_by(&scene.texture, &sprites, scene.camera),
        Some(overlay) => {
            let hud: Vec<SpriteInstance> = overlay_sprites_of(scene)
                .into_iter()
                .map(|sprite| sprite.sampled(wish))
                .collect();
            target.render_sprites_over(
                &scene.texture,
                &sprites,
                SpriteBatch {
                    image: &scene.texture,
                    sprites: &hud,
                    camera: overlay.camera,
                },
                scene.camera,
            )
        }
    }
    .expect("no blessed scene is anywhere near the batch limit")
}

/// The three numbers `BASELINE.md` reports, plus the shape M3.14 showed matters.
struct Margin {
    differing: u64,
    ratio: f64,
    worst: u8,
    histogram: Vec<(u8, u64)>,
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

    let mut counts = [0_u64; 256];
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
        counts[deviation as usize] += 1;
        worst = worst.max(deviation);
        if deviation > narvo_render2d::Tolerance::default().channel {
            differing += 1;
        }
    }

    let total = (left.rgba().len() / 4) as u64;
    #[expect(
        clippy::cast_precision_loss,
        reason = "a ratio only needs enough digits to compare against a budget"
    )]
    let ratio = differing as f64 / total as f64;

    Margin {
        differing,
        ratio,
        worst,
        histogram: counts
            .iter()
            .enumerate()
            .filter(|&(_, &n)| n > 0)
            .map(|(d, &n)| (u8::try_from(d).expect("a deviation is at most 255"), n))
            .collect(),
    }
}

/// Where this file writes what it rendered.
fn output_dir() -> PathBuf {
    golden_artifact_dir().join("linear")
}

/// Loads a PNG written by an earlier run of this test on another rasteriser.
fn load_png(path: &Path) -> Pixels {
    let decoded = image::open(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .to_rgba8();
    let (width, height) = (decoded.width(), decoded.height());
    Pixels::from_rgba8(width, height, decoded.into_raw())
        .expect("a decoded PNG matches its own dimensions")
}

/// Loads a blessed reference. Read-only, always.
fn load_reference(root: &Path, scene: &Scene) -> Pixels {
    load_png(
        &root
            .join(scene.reference)
            .join(format!("{}.png", scene.name)),
    )
}

/// Prints one comparison in the three numbers `BASELINE.md` reports, plus the
/// distribution, and returns it.
fn report(label: &str, scene: &Scene, measured: &Margin) -> u64 {
    let tolerance = narvo_render2d::Tolerance::default();
    let total = u64::from(scene.width) * u64::from(scene.height);
    let touched: u64 = measured
        .histogram
        .iter()
        .filter(|(deviation, _)| *deviation > 0)
        .map(|(_, count)| count)
        .sum();

    println!(
        "{} [{label}]: {} of {total} pixels ({:.4}%) differ by more than {} counts, \
         budget {:.4}%; worst channel deviation {} counts, limit {}",
        scene.name,
        measured.differing,
        measured.ratio * 100.0,
        tolerance.channel,
        tolerance.max_differing_ratio * 100.0,
        measured.worst,
        tolerance.max_channel_deviation
    );
    println!(
        "{} [{label}]: {touched} pixels differ at all; histogram (counts -> pixels): {:?}",
        scene.name, measured.histogram
    );
    touched
}

/// The control, called by every test here that publishes a figure.
///
/// nextest gives each test its own process. M3.17's self-audit found a control that ran in
/// one test of three, and fixed it before shipping; this is that lesson applied
/// from the start rather than restated.
fn control(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    adapter: &str,
    targets: &mut Vec<((u32, u32), OffscreenTarget)>,
) {
    for scene in &scenes() {
        let replicated = render(device, queue, scene, wgpu::FilterMode::Nearest);
        let produced = production_render(targets, scene, adapter, SpriteFilter::Nearest);
        let check = margin(&produced, &replicated);
        assert_eq!(
            check.worst, 0,
            concat!(
                "{}: this file's render at `Nearest` is not byte-identical to ",
                "the production path's — worst {} counts over {} pixels. The ",
                "geometry replicated here is then not the geometry the renderer ",
                "draws, and every `Linear` number below would be measuring the ",
                "replication rather than the sampler."
            ),
            scene.name, check.worst, check.differing
        );
    }
}

/// **The measurement: what `Linear` costs the five blessed images.**
///
/// Two comparisons, reported apart. Against the blessed reference, which says
/// how many would need re-blessing; and against another rasteriser's run of this
/// same test, which says whether one reference can still serve three.
#[test]
fn the_linear_margin_of_every_blessed_image_is_recorded() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };

    let root = workspace_root();
    let output = output_dir();
    std::fs::create_dir_all(&output).expect("the output directory must be creatable");

    let baseline = std::env::var_os(BASELINE_VAR).map(PathBuf::from);
    match &baseline {
        Some(dir) => println!("comparing against {}", dir.display()),
        None => println!("no {BASELINE_VAR} set: writing only"),
    }

    let mut targets: Vec<((u32, u32), OffscreenTarget)> = Vec::new();
    control(&device, &queue, &adapter, &mut targets);

    let scenes = scenes();
    let mut moved: Vec<&str> = Vec::new();
    let mut disagreed: Vec<&str> = Vec::new();

    for scene in &scenes {
        let linear = render(&device, &queue, scene, wgpu::FilterMode::Linear);

        // Comparison one: against what a human blessed.
        let reference = load_reference(&root, scene);
        if report("vs reference", scene, &margin(&reference, &linear)) > 0 {
            moved.push(scene.name);
        }

        // Every run writes, so a later run on another rasteriser can compare.
        let path = output.join(format!("{}.linear.png", scene.name));
        linear
            .save_png(&path)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));

        // Comparison two: against another rasteriser, when one is offered.
        if let Some(dir) = &baseline {
            let theirs = load_png(&dir.join(format!("{}.linear.png", scene.name)));
            if report("vs other rasteriser", scene, &margin(&theirs, &linear)) > 0 {
                disagreed.push(scene.name);
            }
        }
    }

    println!(
        "under Linear, images that move from their reference: {} of {} — {moved:?}",
        moved.len(),
        scenes.len()
    );
    if baseline.is_some() {
        println!(
            "under Linear, images where two rasterisers disagree at all: {} of {} — {disagreed:?}",
            disagreed.len(),
            scenes.len()
        );
    }
    println!("renders written to {}", output.display());
}

/// Blessed references this file does **not** measure, each with its reason.
///
/// `(directory, stem, why)`. The entries are read by
/// [`every_blessed_reference_has_a_scene_here_or_is_excused_in_writing`], which
/// is what makes them a decision rather than a comment.
const NOT_MEASURED_HERE: [(&str, &str, &str); 1] = [(
    "crates/narvo-app/tests/golden",
    "physics_drop_tick45_128x128",
    "M5b.4. Its five sprites are rigid bodies at tick 45, so their positions and \
     rotations are rapier's output rather than authored values, and rapier lives \
     behind `narvo-app`'s physics seam - a crate this one cannot see and must \
     not grow a dependency on. Transcribing the poses as literals would put \
     solver output in this file with nothing keeping it in step: `control` \
     compares this file's pipeline against the production path on *these* \
     sprites, so a wrong transcription renders wrongly through both and passes. \
     It is also the first blessed scene whose sprites are turned, and \
     `scene_vertices` here asserts rotation is zero rather than implementing it, \
     on the argument that a second sin/cos would be a second place for the \
     convention to drift. What that costs is stated rather than hidden: this \
     reference's `Linear` margin is not measured, so if D13's configuration ever \
     moves, this one image is not in the count.",
)];

/// Every blessed reference on disk has a scene in this file, or an excuse.
///
/// Runs without a GPU. **The one failure this measurement cannot afford** is a
/// reference that is reported as unchanged because it was never rendered, and
/// that is exactly what a silently missing entry would produce.
///
/// # Coverage *or* a written excuse, which is not the same guard it was
///
/// Until M5b.4 this demanded coverage outright, and that was right for as long
/// as every blessed scene could be built from literals in this crate. The first
/// one that cannot is `physics_drop_tick45_128x128`, and the two repairs were
/// weighed:
///
/// - **Transcribe it anyway.** Stronger on coverage, and that argument is real —
///   an unmeasured reference is a gap. What defeats it is that the transcription
///   would be solver output copied into two instrument files with no guard able
///   to notice it drifting, which trades a named gap for an unnamed one.
/// - **Coverage or a written excuse**, the shape `xtask`'s
///   `every_mode_is_in_the_matrix_or_excused_in_writing` already has for the
///   determinism matrix (`ProjektPlan.md` §9.2 records the form and forbids its
///   counter-form — extending a hard-coded list by exactly the new case, which is
///   the shape that lets the next one through).
///
/// The second is what this is. It leaves the decision where it was made and makes
/// the next one visible.
#[test]
fn every_blessed_reference_has_a_scene_here_or_is_excused_in_writing() {
    let root = workspace_root();

    // `(directory, stem)`, so this says *where* each reference is and not only
    // that it exists. The two golden directories are separate because the code
    // each covers is: `layer_order_regions_128x128` is blessed in `narvo-app`,
    // whose `sprite_batch.rs` holds `placements_of` and `depth_order`, and the
    // other four in `narvo-render2d`. A scene naming the wrong directory would
    // still find a file of the right name if the two were flattened into one
    // list, which is why the pair is compared rather than the stem.
    let mut on_disk: Vec<(String, String)> = Vec::new();
    for directory in [
        "crates/narvo-render2d/tests/golden",
        "crates/narvo-app/tests/golden",
    ] {
        let entries = std::fs::read_dir(root.join(directory))
            .unwrap_or_else(|error| panic!("cannot read {directory}: {error}"));
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            if path.extension().is_some_and(|extension| extension == "png") {
                on_disk.push((
                    directory.to_owned(),
                    path.file_stem()
                        .expect("a png file has a stem")
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
        }
    }
    on_disk.sort();

    let covered: Vec<(String, String)> = scenes()
        .iter()
        .map(|scene| (scene.reference.to_owned(), scene.name.to_owned()))
        .collect();
    let excused: Vec<(String, String)> = NOT_MEASURED_HERE
        .iter()
        .map(|(directory, name, _)| ((*directory).to_owned(), (*name).to_owned()))
        .collect();

    println!("on disk: {on_disk:?}");
    println!("excused: {excused:?}");

    for entry in &on_disk {
        match (covered.contains(entry), excused.contains(entry)) {
            (true, false) | (false, true) => {}
            (false, false) => panic!(
                "the blessed reference {entry:?} has no scene in this file and no \
                 entry in NOT_MEASURED_HERE. Either add a scene for it or add it \
                 there with the reason it is left out - what is not allowed is \
                 neither, because that is a reference this measurement reports as \
                 unchanged by never rendering it at all."
            ),
            (true, true) => panic!(
                "the blessed reference {entry:?} has a scene in this file *and* an \
                 entry in NOT_MEASURED_HERE, so the reason recorded there is \
                 describing something that is no longer true. Drop the entry."
            ),
        }
    }

    for entry in covered.iter().chain(excused.iter()) {
        assert!(
            on_disk.contains(entry),
            "{entry:?} is named here but there is no such blessed reference on \
             disk. Either it was renamed or it is a typo, and either way this file \
             is measuring - or excusing - something that does not exist."
        );
    }
}

/// The floor this file counts against is the one `Golden` counts against.
///
/// Runs without a GPU. `margin` cannot call `Golden::verify` — it compares two
/// renders rather than verifying against a reference directory — so it reads
/// `Tolerance::default()` instead of holding its own number. This asserts that
/// the numbers printed here are on the same scale as every other figure in
/// `BASELINE.md`, and it changes no threshold.
#[test]
fn the_floor_this_file_uses_is_the_one_golden_uses() {
    let tolerance = narvo_render2d::Tolerance::default();
    assert_eq!(tolerance.channel, 4);
    assert_eq!(tolerance.max_channel_deviation, 24);
    assert!((tolerance.max_differing_ratio - 0.001).abs() < f64::EPSILON);
}

/// A disturbance of known size comes back decomposed into the three numbers.
///
/// Runs without a GPU. Every figure this file prints comes out of [`margin`],
/// and a comparison that quietly reported zero would make the whole measurement
/// read as "nothing moved". So a difference whose size is known by construction
/// goes in, and all three thresholds are checked separately on the way out:
/// which pixels count as differing, what share of the frame that is against the
/// budget, and how far the worst one moved against the cap.
///
/// Predicted before it was run: 20 pixels of 16 384 is 0.1221 %, which is past
/// the 0.1000 % budget by a factor of 1.22 while the worst deviation of 10 stays
/// well inside the cap of 24 — a *budget* failure with an untouched cap, which
/// is a different diagnosis from the camera scene's 137-count *cap* failure and
/// has to look different in the output.
#[test]
fn a_disturbance_of_known_size_is_decomposed_by_threshold() {
    // No GPU figure is published by this test, so it needs no control: it
    // exercises `margin` on two buffers it builds itself. Named rather than left
    // implicit, because the rule for this file is that every test publishing a
    // measured number runs the control.
    let edge = 128_u32;
    let pixels = (edge * edge) as usize;
    let flat = |value: u8| {
        Pixels::from_rgba8(edge, edge, [value, value, value, 255].repeat(pixels))
            .expect("the buffer matches its dimensions")
    };

    let reference = flat(100);
    let mut rgba = reference.rgba().to_vec();
    // Twenty pixels, ten counts each, on one channel.
    for index in 0..20 {
        rgba[index * 4] = 110;
    }
    let disturbed =
        Pixels::from_rgba8(edge, edge, rgba).expect("the buffer matches its dimensions");

    let measured = margin(&reference, &disturbed);
    assert_eq!(measured.differing, 20);
    assert_eq!(measured.worst, 10);
    assert!(
        (measured.ratio - 20.0 / 16_384.0).abs() < 1e-12,
        "ratio was {}",
        measured.ratio
    );
    assert_eq!(measured.histogram, vec![(0, 16_364), (10, 20)]);

    let tolerance = narvo_render2d::Tolerance::default();
    assert!(
        measured.ratio > tolerance.max_differing_ratio,
        "20 of 16 384 pixels must trip the pixel budget"
    );
    assert!(
        measured.worst < tolerance.max_channel_deviation,
        "10 counts must leave the cap untouched"
    );

    // The floor is a floor: a deviation exactly at it does not count as
    // differing, and one count more does.
    let mut at_floor = reference.rgba().to_vec();
    at_floor[0] = 104;
    let at_floor =
        Pixels::from_rgba8(edge, edge, at_floor).expect("the buffer matches its dimensions");
    assert_eq!(margin(&reference, &at_floor).differing, 0);
    assert_eq!(margin(&reference, &at_floor).worst, 4);

    let mut past_floor = reference.rgba().to_vec();
    past_floor[0] = 105;
    let past_floor =
        Pixels::from_rgba8(edge, edge, past_floor).expect("the buffer matches its dimensions");
    assert_eq!(margin(&reference, &past_floor).differing, 1);
}

/// **M3.23's first proof: production at `Linear` is this file's `Linear`.**
///
/// `control` above pins the two at `Nearest` and has since M3.18 — that is what
/// makes every number in this file a measurement of the sampler rather than of
/// the replication. M3.23 gives the production path a second sampler, so the
/// same comparison can now be made at `Linear`, and it validates both
/// directions at once: this file's pipeline is production's in every respect but
/// the sampler, so byte-identity at `Linear` says the production sampler is the
/// one this file has been measuring all along.
///
/// The claim is about the **five blessed scenes' geometry** rendered with a
/// `Linear` wish, not about the blessed images: no reference is consulted here.
/// Nothing in this file changes a threshold or re-collects a number.
///
/// **The clause that used to close that sentence — "and none of the five is
/// drawn at `Linear` anywhere else" — was already false and is now doubly so.**
/// `sprite_atlas_regions_128x128` has been drawn at `Linear` by
/// `texture_region.rs`'s `atlas_scene` since M3.24, and
/// `placed_sprite_quadrants_128x128` by `golden_image.rs` since M3.27. It was
/// never what the test rests on: the comparison below is this file's own replica
/// against the **production** path, both driven from this file's own scene table
/// and both told `Linear` explicitly — so what another file wishes for its own
/// copy of a scene cannot reach it. Nothing here changes but the sentence.
///
/// Byte-identity is the expectation. A difference would be the finding.
#[test]
fn production_at_linear_is_byte_identical_to_this_file_s_linear() {
    let Some((device, queue, adapter)) = device_or_skip() else {
        return;
    };

    let mut targets: Vec<((u32, u32), OffscreenTarget)> = Vec::new();
    for scene in &scenes() {
        let replicated = render(&device, &queue, scene, wgpu::FilterMode::Linear);
        let produced = production_render(&mut targets, scene, &adapter, SpriteFilter::Linear);
        let check = margin(&produced, &replicated);

        println!(
            "{}: production Linear vs replicated Linear — {} differing, worst {}",
            scene.name, check.differing, check.worst
        );
        // `worst`, not `differing`: `margin` counts a pixel as differing only
        // above `Tolerance::default().channel`, which is 4, so asserting
        // `differing == 0` would let every channel move by four counts while
        // this test claimed byte-identity. `control` above asserts `worst` for
        // the same reason.
        assert_eq!(
            check.worst, 0,
            "{}: production at `Linear` is not byte-identical to this file's",
            scene.name
        );
        assert_eq!(check.differing, 0, "{}: pixels past the floor", scene.name);
    }
}
