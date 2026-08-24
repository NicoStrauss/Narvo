//! Files on disk to pixels on screen, with nothing committed in between.
//!
//! The proof M4.8 exists for, and the only test in the repository that walks the
//! whole chain: **PNG bytes written by this test → decoded → premultiplied →
//! `SourceRegion` → the M4.4 packer → an atlas → a scene naming its regions by
//! the eighth registered component → the frame path → probes on the rendered
//! image.**
//!
//! # Nothing binary is committed (ADR-0024)
//!
//! Every PNG here is encoded by the test that reads it, into a scratch directory
//! under the cargo target directory. A committed fixture image would be a binary
//! blob nobody can review in a diff, and the repository's whole content rule is
//! that fixtures are generated and derivable.
//!
//! What that costs is stated rather than hidden: **encoder and decoder are the
//! same crate**, so a symmetric defect in both would be invisible here. What is
//! not invisible is everything between them, which is the part this project
//! wrote — the exact expansion, the premultiply, the packing, the resolution and
//! the blend.
//!
//! # Why the probes are what they are
//!
//! Three of them, and each answers a question no other test in this file can:
//!
//! 1. **An opaque icon** reads back as exactly its source colour. That is the
//!    contract's verbatim property from M4.4, sharpened by M4.8: a region's
//!    pixels in the atlas are the source pixels *after* premultiplication, and
//!    for an opaque pixel premultiplication is the identity.
//! 2. **A half-transparent icon over a ground** reads back as ADR-0023's
//!    arithmetic applied to file-loaded content. This is where the two
//!    milestones meet, and neither one alone would have caught a premultiply
//!    that ran in the wrong space.
//! 3. **A dirty-transparent pixel** — a bright colour stored under `alpha = 0`,
//!    which every image editor produces — reads back as the ground, untouched.
//!    Without the load-time premultiply that colour would be in the atlas and
//!    would bleed under `Linear`.
//!
//! # Gated on `render`, like every other test here that renders
//!
//! `narvo-render2d` is optional and behind that feature, so this whole file has
//! to disappear from a headless build — the same `#![cfg]` `transform_to_sprite.rs`
//! carries. M4.8 shipped it without one and CI caught it on both platforms,
//! because the headless build and test were CI steps *outside* the verification
//! set and a locally green `cargo xtask ci` said nothing about them.
//!
//! **M4.9 closed that gap**: those three steps are now steps seven to nine of
//! `cargo xtask ci`, and removing the attribute above turns the local run red
//! with the same seven errors CI reported. So the reproduction is the ordinary
//! one, and this note records why it exists rather than how to work around it.

#![cfg(feature = "render")]

use narvo_ecs::{Layer, Sprite, Transform, World};
use narvo_render2d::{
    OffscreenTarget, Pixels, RenderError, SpriteFilter, SpriteInstance, TextureRegion,
    golden_artifact_dir,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Printed instead of failing when this machine has no GPU adapter at all.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set in CI, where a missing adapter is a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// The canvas the scene is rendered on.
const SIZE: u32 = 64;

/// A scratch directory under the cargo target directory, named for the case.
fn scratch(case: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(case);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("the scratch directory");
    directory
}

/// Straight-alpha RGBA8 as PNG bytes, the way an image editor would write them.
fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header can be written");
        writer
            .write_image_data(rgba)
            .expect("the data is the size the header says");
    }
    bytes
}

/// Builds a target, or reports that this machine cannot host one.
fn target_or_skip() -> Option<OffscreenTarget> {
    match OffscreenTarget::new(SIZE, SIZE) {
        Ok(target) => {
            println!("adapter in use: {}", target.adapter_summary());
            Some(target)
        }
        Err(error @ RenderError::NoAdapter { .. }) => {
            assert!(
                std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a failure \
                 rather than a skip: {error}"
            );
            println!("{SKIP_MARKER}: {error}");
            None
        }
        Err(other) => panic!("creating the offscreen target failed: {other}"),
    }
}

/// The three assets the scene draws, written as files.
///
/// * `ground` — opaque dark blue, the whole canvas.
/// * `solid` — opaque orange, so an opaque region proves the verbatim property.
/// * `veil` — white at `alpha = 128`, so a half-transparent region proves the
///   blend arithmetic on file-loaded content.
/// * `dirty` — fully transparent with a bright magenta stored under it, so the
///   load-time premultiply proves itself.
fn write_assets(directory: &Path) {
    std::fs::write(
        directory.join("ground.png"),
        encode(4, 4, &[0, 0, 64, 255].repeat(16)),
    )
    .expect("the file is written");

    std::fs::write(
        directory.join("solid.png"),
        encode(4, 4, &[255, 128, 0, 255].repeat(16)),
    )
    .expect("the file is written");

    std::fs::write(
        directory.join("veil.png"),
        encode(4, 4, &[255, 255, 255, 128].repeat(16)),
    )
    .expect("the file is written");

    std::fs::write(
        directory.join("dirty.png"),
        encode(4, 4, &[255, 0, 255, 0].repeat(16)),
    )
    .expect("the file is written");
}

/// The scene, as a file: a ground and three 16-pixel panels over it.
///
/// Written as RON text and loaded through the real loader, so the region names
/// travel the way they will in a game rather than through a hand-built world.
fn write_scene(path: &Path) {
    let text = r#"Scene(
    entities: [
        (
            components: {
                "transform": (x: 0.0, y: 0.0, rotation: 0.0, scale_x: 64.0, scale_y: 64.0),
                "layer": (depth: 0.0),
                "sprite": (region: "ground"),
            },
        ),
        (
            components: {
                "transform": (x: -16.0, y: 0.0, rotation: 0.0, scale_x: 16.0, scale_y: 16.0),
                "layer": (depth: 1.0),
                "sprite": (region: "solid"),
            },
        ),
        (
            components: {
                "transform": (x: 0.0, y: 0.0, rotation: 0.0, scale_x: 16.0, scale_y: 16.0),
                "layer": (depth: 1.0),
                "sprite": (region: "veil"),
            },
        ),
        (
            components: {
                "transform": (x: 16.0, y: 0.0, rotation: 0.0, scale_x: 16.0, scale_y: 16.0),
                "layer": (depth: 1.0),
                "sprite": (region: "dirty"),
            },
        ),
    ],
)
"#;
    std::fs::write(path, text).expect("the scene is written");
}

/// Loads the scene and its assets, then renders the world offscreen.
///
/// Deliberately rebuilds what the window's frame path does rather than calling
/// it: the window needs a window. What is shared is everything that matters —
/// the loader, the packer, the resolution and the sprite building — and the
/// draw order comes from the same `Layer` depths.
fn render(target: &OffscreenTarget, scene: &Path) -> Pixels {
    let text = std::fs::read_to_string(scene).expect("the scene reads");
    let world = load(&text);

    let directory = scene
        .parent()
        .expect("the scene has a directory")
        .join("assets");
    let regions = narvo_assets::regions_from_directory(&directory).expect("the assets load");
    let atlas = narvo_assets::pack(regions).expect("four small regions pack");

    let texture = Pixels::from_rgba8(atlas.width(), atlas.height(), atlas.rgba().to_vec())
        .expect("the packed atlas is a texture");
    let table: BTreeMap<String, TextureRegion> = atlas
        .regions()
        .map(|(name, place)| {
            (
                name.to_owned(),
                TextureRegion::from_texels(
                    place.left(),
                    place.top(),
                    place.width(),
                    place.height(),
                    &texture,
                ),
            )
        })
        .collect();

    let sprites: Vec<SpriteInstance> = drawn_in_depth_order(&world)
        .into_iter()
        .map(|(placement, region)| {
            SpriteInstance::new(placement, table[&region]).sampled(SpriteFilter::Nearest)
        })
        .collect();

    target
        .render_sprites(&texture, &sprites)
        .expect("four sprites are far inside the batch limit")
}

/// The scene text as a world, through the real registry.
fn load(text: &str) -> World {
    let mut registry = narvo_ecs::ComponentRegistry::new();
    narvo_ecs::register_engine_components(&mut registry).expect("a fresh registry accepts them");
    narvo_scene::from_str(text, &registry).expect("the scene loads")
}

/// Placements and region names, sorted the way the renderer draws them.
fn drawn_in_depth_order(world: &World) -> Vec<(narvo_render2d::SpritePlacement, String)> {
    let mut drawn: Vec<(f32, u32, narvo_render2d::SpritePlacement, String)> = Vec::new();
    for (index, entity) in world.entity_ids().into_iter().enumerate() {
        let Ok(transform) = world.get::<Transform>(entity) else {
            continue;
        };
        let Ok(sprite) = world.get::<Sprite>(entity) else {
            continue;
        };
        let depth = world
            .get::<Layer>(entity)
            .map_or(Layer::DEFAULT.depth, |layer| layer.depth);

        drawn.push((
            depth,
            u32::try_from(index).expect("a small index"),
            narvo_render2d::SpritePlacement {
                x: transform.x,
                y: transform.y,
                ..narvo_render2d::SpritePlacement::new(transform.scale_x, transform.scale_y)
                    .turned(transform.rotation)
            },
            sprite.region.clone(),
        ));
    }

    drawn.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    drawn
        .into_iter()
        .map(|(_, _, placement, region)| (placement, region))
        .collect()
}

/// The whole chain, probed at three points.
#[test]
fn files_become_pixels_and_the_three_probes_hold() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let root = scratch("end-to-end");
    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).expect("the assets directory");
    write_assets(&assets);

    let scene = root.join("scene.ron");
    write_scene(&scene);

    let rendered = render(&target, &scene);
    let at = |x: u32, y: u32| rendered.pixel(x, y).expect("inside the canvas");

    // The ground, away from every panel. Opaque dark blue, straight through:
    // premultiplication is the identity at alpha 255, and the sRGB round trip
    // returns the byte it started from.
    let ground = at(4, 4);
    println!("  ground            (4, 4):   {ground:?}");
    assert_eq!(ground, [0, 0, 64, 255], "the ground is not its own colour");

    // Panel 1: an opaque icon. Its texels reach the screen verbatim, which is
    // the M4.4 property with M4.8's precision — the atlas holds the source
    // pixels *after* premultiply, and for an opaque pixel that is the source.
    let solid = at(16, 32);
    println!("  opaque icon       (16, 32): {solid:?}");
    assert_eq!(solid, [255, 128, 0, 255], "an opaque icon lost its colour");

    // Panel 2: white at alpha 128 over the dark blue ground. ADR-0023's
    // arithmetic on file-loaded content:
    //   the file holds  [255, 255, 255, 128]  straight
    //   the atlas holds [128, 128, 128, 128]  premultiplied, (255*128+127)/255
    //   the blend is    src + dst * (1 - 128/255), in linear light
    //
    // **Two of the three colour channels are exact and need no colour-space
    // model at all**, which is the same structure M4.7's `text_over_scene`
    // found: the ground is [0, 0, 64], so red and green take nothing from it,
    // the result is the source term alone, and the sRGB round trip returns the
    // byte it started from — 128, the premultiplied value, on the counter.
    //
    // Blue is the one channel the ground contributes to, so it is the one that
    // goes through the transfer function; it is asserted by direction rather
    // than by a literal, because its exact byte is a rasteriser's rounding (the
    // 1-count spread M4.7 measured across three of them). Measured here: 135.
    let veil = at(32, 32);
    println!("  half-transparent  (32, 32): {veil:?}");
    assert_eq!(
        veil[3], 255,
        "compositing over an opaque ground left a hole"
    );
    assert_eq!(
        [veil[0], veil[1]],
        [128, 128],
        "red and green take nothing from this ground, so they must be exactly the \
         premultiplied source: {veil:?}"
    );
    assert!(
        veil[2] > 128,
        "blue carries the source term plus what survives of the ground, so it must \
         exceed the source alone: {veil:?}"
    );
    assert!(
        veil[2] < 160,
        "the ground contributes less than half its own blue at alpha 128, so this is \
         too bright to be the arithmetic ADR-0023 describes: {veil:?}"
    );

    // Panel 3: the dirty-transparent icon. Bright magenta stored under alpha 0
    // in the file; the load-time premultiply makes it [0, 0, 0, 0], so the
    // panel contributes nothing and the ground stands. **A single magenta
    // channel here would mean the premultiply did not happen** — and under
    // `Linear` that colour would bleed into every neighbouring texel.
    let dirty = at(48, 32);
    println!("  dirty transparent (48, 32): {dirty:?}");
    assert_eq!(
        dirty,
        [0, 0, 64, 255],
        "a fully transparent source changed the ground, so its stored colour survived the load"
    );
}

/// Writes a runnable demo into `target/`, and proves it runs.
///
/// **The "generatable `assets/`" M4.8 asks for, as a test rather than as a
/// tool.** Two rules pointed here rather than at `cargo xtask`: nothing binary
/// is committed (ADR-0024), so the PNGs have to be generated by something; and
/// `xtask`'s manifest keeps its dependencies deliberately empty, so a PNG
/// encoder cannot live there.
///
/// Making it a test buys something a generator script would not have: **it
/// checks that what it wrote actually loads and resolves**, through the same
/// path the window takes. A demo that was written but did not work would fail
/// here rather than in front of whoever tried it.
///
/// After any test run, this exists:
///
/// ```text
/// target/m48-demo/scene.ron
/// target/m48-demo/assets/{ground,solid,veil,dirty}.png
/// ```
///
/// and `cargo run -p narvo-app -- --scene target/m48-demo/scene.ron` opens a
/// window on it.
#[test]
fn a_runnable_demo_is_written_under_target_and_loads() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("m48-demo");
    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).expect("the demo directory");

    write_assets(&assets);
    let scene = root.join("scene.ron");
    write_scene(&scene);

    // The window's own two steps, in order: load the scene, then resolve every
    // region it names against the packed directory.
    let text = std::fs::read_to_string(&scene).expect("the demo scene reads");
    let world = load(&text);

    let regions = narvo_assets::regions_from_directory(&assets).expect("the demo assets load");
    let atlas = narvo_assets::pack(regions).expect("the demo assets pack");
    for entity in world.entity_ids() {
        if let Ok(sprite) = world.get::<Sprite>(entity) {
            assert!(
                atlas.region(&sprite.region).is_some(),
                "the demo names the region \"{}\", which its own assets do not carry",
                sprite.region
            );
        }
    }

    println!(
        "demo written: cargo run -p narvo-app -- --scene {}",
        scene.display()
    );
}

/// The atlas built from files carries the sources' premultiplied pixels,
/// verbatim.
///
/// M4.4's verbatim property, restated for a file source: **region pixels in the
/// atlas equal the source pixels after premultiplication.** Checked without a
/// GPU, so it runs everywhere the suite does.
#[test]
fn a_regions_pixels_in_the_atlas_are_its_source_pixels_after_premultiply() {
    let directory = scratch("verbatim").join("assets");
    std::fs::create_dir_all(&directory).expect("the assets directory");
    write_assets(&directory);

    let regions = narvo_assets::regions_from_directory(&directory).expect("the assets load");
    let atlas = narvo_assets::pack(regions).expect("four small regions pack");

    // What each file should have become, computed here from the straight-alpha
    // values the writer used rather than read back from the loader.
    for (name, straight) in [
        ("ground", [0_u8, 0, 64, 255]),
        ("solid", [255, 128, 0, 255]),
        ("veil", [255, 255, 255, 128]),
        ("dirty", [255, 0, 255, 0]),
    ] {
        let expected = [
            narvo_assets::premultiply(straight[0], straight[3]),
            narvo_assets::premultiply(straight[1], straight[3]),
            narvo_assets::premultiply(straight[2], straight[3]),
            straight[3],
        ];

        let place = atlas.region(name).expect("the region is in the atlas");
        for y in place.top()..place.top() + place.height() {
            for x in place.left()..place.left() + place.width() {
                let offset = ((y * atlas.width() + x) * 4) as usize;
                assert_eq!(
                    &atlas.rgba()[offset..offset + 4],
                    &expected,
                    "{name} at ({x}, {y}) is not its source premultiplied"
                );
            }
        }
    }
}

/// The padding guard sees a file-packed atlas exactly as it sees a code-packed
/// one.
///
/// The contract's other half: the same `check_region_padding` the renderer uses,
/// run against an atlas whose pixels came from disk. If a file source needed a
/// different padding rule, ADR-0020's "the source is exchangeable under the
/// contract" would be wrong, and this is where it would show.
#[test]
fn the_padding_guard_passes_a_file_packed_atlas() {
    let directory = scratch("padding").join("assets");
    std::fs::create_dir_all(&directory).expect("the assets directory");
    write_assets(&directory);

    let regions = narvo_assets::regions_from_directory(&directory).expect("the assets load");
    let atlas = narvo_assets::pack(regions).expect("four small regions pack");
    let texture = Pixels::from_rgba8(atlas.width(), atlas.height(), atlas.rgba().to_vec())
        .expect("the packed atlas is a texture");

    for (name, place) in atlas.regions() {
        narvo_render2d::check_region_padding(
            place.left(),
            place.top(),
            place.width(),
            place.height(),
            narvo_assets::REGION_PADDING_TEXELS,
            &texture,
        )
        .unwrap_or_else(|defect| panic!("{name} is not padded in the file-packed atlas: {defect}"));
    }

    // A directory that is not a stockpile: the artifact directory is used so a
    // failing run leaves something to look at, in the place M3.11 fixed for it.
    let _ = golden_artifact_dir();
}

/// The guard sees a corrupted border in a file-packed atlas, and says where.
///
/// # The flank, and the direction it is *not* sensitive in
///
/// M4.8 first ran this the wrong way round — calling the guard with
/// `REGION_PADDING_TEXELS - 1` — and it stayed **green**. That is correct
/// behaviour and worth writing down rather than filing as a near miss: the
/// engine's border is one texel, so `border - 1` is **zero**, and a guard asked
/// about a zero-texel frame has no texels to look at. Weakening the *question*
/// cannot make an artefact fail; M4.4's own red flank injected the **packer**,
/// not the call.
///
/// So this corrupts the artefact instead, in the same shape a packer bug would:
/// one border texel overwritten. The guard localises it to the texel, names the
/// edge, and prints both colours — on an atlas whose pixels came from disk,
/// which is the thing M4.8 needed to know.
///
/// The other sensitive direction is kept as a second assertion: asking about a
/// border one *wider* than the packer used also fails, because those texels
/// belong to the background. Between the two, the guard is pinned from above and
/// from the side; from below it is silent, by construction.
#[test]
fn the_guard_sees_a_corrupted_border_in_a_file_packed_atlas() {
    let directory = scratch("corrupt-border").join("assets");
    std::fs::create_dir_all(&directory).expect("the assets directory");
    write_assets(&directory);

    let regions = narvo_assets::regions_from_directory(&directory).expect("the assets load");
    let atlas = narvo_assets::pack(regions).expect("four small regions pack");

    let (name, place) = atlas.regions().next().expect("the atlas has a region");
    let name = name.to_owned();

    // One texel of the frame around the first region, painted with something
    // that is not the edge it should replicate.
    let mut rgba = atlas.rgba().to_vec();
    let (x, y) = (place.left() - 1, place.top() - 1);
    let offset = ((y * atlas.width() + x) * 4) as usize;
    rgba[offset..offset + 4].copy_from_slice(&[1, 2, 3, 4]);

    let damaged = Pixels::from_rgba8(atlas.width(), atlas.height(), rgba)
        .expect("the damaged atlas is still a texture");

    let defect = narvo_render2d::check_region_padding(
        place.left(),
        place.top(),
        place.width(),
        place.height(),
        narvo_assets::REGION_PADDING_TEXELS,
        &damaged,
    )
    .expect_err("a corrupted border must be seen");

    let message = format!("{defect:?}");
    println!("  corrupted border in \"{name}\": {message}");
    assert!(message.contains("TopLeft"), "{message}");
    assert!(message.contains("[1, 2, 3, 4]"), "{message}");

    // The second sensitive direction: a border wider than the packer painted.
    narvo_render2d::check_region_padding(
        place.left(),
        place.top(),
        place.width(),
        place.height(),
        narvo_assets::REGION_PADDING_TEXELS + 1,
        &Pixels::from_rgba8(atlas.width(), atlas.height(), atlas.rgba().to_vec())
            .expect("the atlas is a texture"),
    )
    .expect_err("a border wider than the packer used reaches the background");
}
