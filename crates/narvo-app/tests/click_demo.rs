//! Writes the runnable click-counter demo under `target/`, and checks it works.
//!
//! # Why a test rather than a script
//!
//! M4.8's precedent, and its reasoning transfers unchanged: making the generator
//! a test buys something a script would not have — **it checks that what it
//! wrote actually loads and resolves**, through the same two steps the window
//! takes. A demo that was written but did not run would fail here rather than in
//! front of whoever tried it.
//!
//! # Why the demo has to be generated at all
//!
//! `crates/narvo-app/scenes/click_counter.ron` names two atlas regions, and the
//! runner packs an atlas from the scene's own `assets/` directory before it
//! opens a window. ADR-0024 forbids committing the PNGs that would satisfy it —
//! *"A committed binary. No PNG is committed, here or anywhere."* — so the
//! shipped scene cannot be windowed where it lives. M5.5 measured that refusal;
//! this closes it the way M4.8 closed the same problem, by writing a runnable
//! copy under `target/`.
//!
//! **The scene and the mapping are copied byte for byte** and asserted so by
//! digest. That is what keeps the demo a demo *of the shipped content* rather
//! than a second, drifting version of it — the failure this would otherwise
//! invite is a demo that works while the content it claims to demonstrate does
//! not.
//!
//! After any test run with the `render` feature, this exists:
//!
//! ```text
//! target/m55-demo/click_counter.ron
//! target/m55-demo/click_counter.mapping.ron
//! target/m55-demo/assets/{backdrop,button}.png
//! target/m55-demo/README.md
//! ```

#![cfg(feature = "render")]

use std::path::{Path, PathBuf};

use narvo_ecs::Sprite;

/// The committed originals this demo copies.
const SCENE: &str = "click_counter.ron";
const MAPPING: &str = "click_counter.mapping.ron";

/// The demo's own directory under `target/`.
fn demo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("m55-demo")
}

/// The directory the committed scene and mapping live in.
fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes")
}

/// A solid RGBA image, encoded as a PNG.
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

/// Copies `name` from the committed scenes directory into the demo, and proves
/// it is the same bytes.
///
/// The digest is taken over what was *written*, not over what was read, so a
/// copy that lost a byte on the way out is caught rather than assumed away.
fn copy_verbatim(name: &str, into: &Path) {
    let source = source_root().join(name);
    let original = std::fs::read(&source)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", source.display()));

    let destination = into.join(name);
    std::fs::write(&destination, &original)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", destination.display()));

    let written = std::fs::read(&destination).expect("what was just written reads back");

    assert_eq!(
        narvo_core::sha256::hex(&narvo_core::sha256::sha256(&written)),
        narvo_core::sha256::hex(&narvo_core::sha256::sha256(&original)),
        "the demo's copy of {name} is not the committed file"
    );
}

/// The two regions the scene names, as solid colours.
///
/// The same two the blessed image uses — black ground, red button — so the demo
/// and `click_counter_state3_128x128` show the same thing, one interactively and
/// one at a fixed count. Taken from `narvo_testkit::blend`'s grounds rather
/// than typed again here.
fn write_assets(directory: &Path) {
    use narvo_testkit::blend::Ground;

    for (name, texel) in [("backdrop", Ground::BLACK), ("button", Ground::RED)] {
        let path = directory.join(format!("{name}.png"));
        std::fs::write(&path, encode(4, 4, &texel.repeat(16)))
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
    }
}

#[test]
fn a_runnable_click_demo_is_written_under_target_and_loads() {
    let root = demo_root();
    let assets = root.join("assets");
    std::fs::create_dir_all(&assets).expect("the demo directory");

    copy_verbatim(SCENE, &root);
    copy_verbatim(MAPPING, &root);
    write_assets(&assets);

    let scene = root.join(SCENE);

    // The window's own two steps, in order: load the scene, then resolve every
    // region it names against the packed directory. Anything this misses is
    // something whoever runs the demo would have hit instead.
    let text = std::fs::read_to_string(&scene).expect("the demo scene reads");
    let world = load_scene(&text);

    let regions = narvo_assets::regions_from_directory(&assets).expect("the demo assets load");
    let atlas = narvo_assets::pack(regions).expect("two small regions pack");

    let mut covered = 0;
    for entity in world.entity_ids() {
        if let Ok(sprite) = world.get::<Sprite>(entity) {
            assert!(
                atlas.region(&sprite.region).is_some(),
                "the demo scene names the region \"{}\", which its own assets do not carry",
                sprite.region
            );
            covered += 1;
        }
    }
    assert_eq!(
        covered, 2,
        "the scene names two regions and both are covered"
    );

    // The mapping is content too, and a demo that shipped an unloadable one
    // would fail at the keyboard rather than here.
    let mapping_text = std::fs::read_to_string(root.join(MAPPING)).expect("the mapping reads");
    let mapping = narvo_input::from_str(&mapping_text).expect("the demo mapping loads");
    assert_eq!(mapping.bindings().len(), 1, "Space, and nothing else");

    write_readme(&root, &scene);

    println!(
        "demo written: ./target/debug/narvo --scene {} --mapping {}",
        scene.display(),
        root.join(MAPPING).display()
    );
}

/// The scene's world, through the real loader and the engine registry.
fn load_scene(text: &str) -> narvo_ecs::World {
    let mut registry = narvo_ecs::ComponentRegistry::new();
    narvo_ecs::register_engine_components(&mut registry).expect("a fresh registry accepts them");

    // The scene carries an `input` buffer, which the runner registers itself
    // (ADR-0027 Decision 4): `narvo-ecs` cannot name the payload type. This
    // test is outside the binary crate, so it registers the same thing here.
    registry
        .register_component::<narvo_ecs::Events<narvo_input::InputEvent>>("input")
        .expect("a fresh registry accepts it");

    narvo_scene::from_str(text, &registry).expect("the demo scene loads")
}

/// A note beside the demo saying how to run it.
fn write_readme(root: &Path, scene: &Path) {
    let text = format!(
        "# The click-counter demo\n\
         \n\
         Written by `a_runnable_click_demo_is_written_under_target_and_loads` in\n\
         `crates/narvo-app/tests/click_demo.rs`. Nothing here is under version\n\
         control: ADR-0024 keeps PNGs out of the repository, so the assets are\n\
         generated and this directory is rebuilt by any test run.\n\
         \n\
         The scene and the mapping are byte-for-byte copies of the committed\n\
         originals under `crates/narvo-app/scenes/`, asserted by digest.\n\
         \n\
         Run it:\n\
         \n\
         ```\n\
         ./target/debug/narvo --scene {} \\\n\
         \x20                     --mapping {}\n\
         ```\n\
         \n\
         Keep `--mapping` even if you only intend to click: the mouse path needs\n\
         the same queue the keyboard does, so without it neither works.\n\
         \n\
         Click the red button, or press Space. Each change prints one line on\n\
         stdout, `buy: 1`, `buy: 2`, and so on - one per change, not one per\n\
         frame. The window draws no number: giving it a HUD is M7's business.\n\
         \n\
         ## What it should sound like\n\
         \n\
         Sound is the one thing here no test can check, so this is for a human.\n\
         Every cue also prints a line, and the lines are the point of comparison:\n\
         if a line appears and nothing is heard, the fault is below the cue layer.\n\
         \n\
         At startup, one of:\n\
         \n\
         ```\n\
         audio: output opened, one track per channel\n\
         audio: running silently, no audio output is available: ...\n\
         ```\n\
         \n\
         The second is not a failure - a machine without an output runs the demo\n\
         silently and still prints every cue line below.\n\
         \n\
         Then, in this order:\n\
         \n\
         1. immediately, a steady low tone starts and keeps looping -\n\
            `cue tick 0 music play music_base`;\n\
         2. on the first buy, a short high blip over the tone -\n\
            `buy: 1` then `cue tick N sfx play click`;\n\
         3. on the second buy, the same blip - `buy: 2`, one more click line;\n\
         4. on the **third** buy, the blip *and* a second, higher tone that\n\
            fades in over about a second rather than appearing at once -\n\
            `buy: 3`, a click line, then `cue tick N music layer_on music_layer`.\n\
            The fade is what to listen for: kira tweens the layer's volume, so it\n\
            should swell rather than switch on;\n\
         5. press Space for the fourth buy. It must sound **exactly** like a\n\
            mouse click, because the cue hangs on the action and not on the\n\
            device - `buy: 4` and another `sfx play click` line.\n\
         \n\
         The layer arrives once and stays. Further buys keep clicking.\n",
        scene.display(),
        root.join(MAPPING).display()
    );

    let path = root.join("README.md");
    std::fs::write(&path, text)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}
