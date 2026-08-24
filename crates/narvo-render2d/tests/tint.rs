//! What a tint does to a rendered pixel, measured in both directions.
//!
//! # The failure this file is built against
//!
//! A tint that does nothing passes every test that only asks "is it green".
//! Every assertion below therefore comes in a pair at the *same* pixels: the
//! identity tint reproduces the untinted render byte for byte, and a tint that
//! is not the identity produces a value computed by hand — not merely a
//! different one.
//!
//! # The space the multiplication happens in, which is measured and not chosen
//!
//! `S1` of M6b.3 asked whether encoded or linear is a decision here. It is not.
//! The atlas texture and the render target are both `Rgba8UnormSrgb`, which wgpu
//! documents as "Srgb-color [0, 255] converted to/from linear-color float
//! [0, 1] in shader" (`wgpu-types-30.0.0/src/texture/format.rs:186`), so the
//! sample the shader multiplies is already linear light.
//!
//! The two readings are far apart and one render separates them:
//! [`the_half_tint_lands_where_a_linear_multiply_puts_it`] tints a white texel
//! by `0.5` and asks what byte comes back. Linear says **188**; encoded says
//! **128**. That is the measurement, and it is why this question needed no
//! decision.
//!
//! # No new reference here
//!
//! Every pixel this file asserts is predicted arithmetic over flat, opaque,
//! axis-aligned rectangles at whole pixels under `Nearest` — the standing
//! `two_textures.rs` gives its own assertions. `tests/golden/` is read by
//! [`the_tint_scene_matches_its_golden_reference`] and written by nothing here.

use std::path::PathBuf;

use narvo_render2d::{
    Golden, OffscreenTarget, Pixels, RenderError, SpriteFilter, SpriteInstance, SpritePlacement,
    SpriteTint, TextureRegion, golden_artifact_dir,
};

/// Printed instead of failing when this machine has no GPU adapter at all.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// Set in CI, where a missing adapter is a failure rather than a skip.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// The canvas edge for the arithmetic tests. Small: every assertion names a
/// pixel.
const EDGE: u32 = 64;

/// The fixture texture's edge, in texels.
const TEXTURE_EDGE: u32 = 16;

/// The stored byte of the fixture's grey half.
///
/// Mid-grey **as a stored byte**, which is nowhere near mid-grey as light —
/// that gap is the whole reason S1's measurement separates the two readings, so
/// the ground this file composites over is chosen to sit in it.
const GREY_BYTE: u8 = 128;

/// The name this scene's candidate reference is blessed under.
const TINT_SCENE: &str = "tint_over_ground_128x128";

/// The blessed scene's edge, in pixels.
const SCENE_EDGE: u32 = 128;

/// A 16 x 16 texture: opaque white in the left half, opaque grey in the right.
///
/// Two halves of one texture rather than two textures, so both reach the shader
/// through one bind and one draw call — the tint has to work inside a batch,
/// not only in a render of its own.
fn halves() -> Pixels {
    let mut rgba = Vec::with_capacity((TEXTURE_EDGE * TEXTURE_EDGE * 4) as usize);
    for _ in 0..TEXTURE_EDGE {
        for x in 0..TEXTURE_EDGE {
            let value = if x < TEXTURE_EDGE / 2 { 255 } else { GREY_BYTE };
            rgba.extend_from_slice(&[value, value, value, 255]);
        }
    }

    Pixels::from_rgba8(TEXTURE_EDGE, TEXTURE_EDGE, rgba).expect("16x16 is a legal size")
}

/// The white half, with two texels of same-coloured margin on every side.
///
/// The margin is what makes the region safe to sample at any point a rasteriser
/// picks: a `Nearest` fetch that lands one texel outside still reads white, so
/// no assertion below depends on where inside a pixel the sample was taken.
fn white_of(texture: &Pixels) -> TextureRegion {
    TextureRegion::from_texels(2, 2, 4, 12, texture)
}

/// The grey half, with the same margin.
fn grey_of(texture: &Pixels) -> TextureRegion {
    TextureRegion::from_texels(10, 2, 4, 12, texture)
}

/// A sprite of `size` world units at the origin, sampled `Nearest`.
fn quad(size: f32, region: TextureRegion) -> SpriteInstance {
    SpriteInstance::new(
        SpritePlacement {
            x: 0.0,
            y: 0.0,
            rot_cos: SpritePlacement::UNTURNED.0,
            rot_sin: SpritePlacement::UNTURNED.1,
            scale_x: size,
            scale_y: size,
        },
        region,
    )
    .sampled(SpriteFilter::Nearest)
}

/// The pixel at (`x`, `y`), which must be inside the canvas.
fn pixel(frame: &Pixels, x: u32, y: u32) -> [u8; 4] {
    frame.pixel(x, y).expect("inside the canvas")
}

/// What the model says a linear multiply of `byte` by `factor` reads back as.
///
/// Decode, multiply, encode — `narvo_testkit::srgb` in `f64` against the
/// shader's `f32`, which is why every comparison against this allows
/// [`MODEL_SLACK`].
fn linear_product(byte: u8, factor: f64) -> u8 {
    narvo_testkit::srgb::encode(narvo_testkit::srgb::decode(byte) * factor)
}

/// ADR-0023's `OVER`, in linear light, as a stored byte.
///
/// `src + dst * (1 - src_alpha)`, with a **premultiplied** source — so
/// `src_colour` carries no factor of its own and the destination is scaled by
/// the source's *alpha*, which is not the same number.
///
/// Written with both arguments rather than one because they only coincide in
/// the easy case. A faded white has `src_colour == src_alpha` and a faded
/// *orange* does not; a model that used the colour in place of the alpha was
/// eleven counts wrong on exactly that case and right on all the others, which
/// is how it survived being written.
fn over(src_colour: f64, src_alpha: f64, ground: f64) -> u8 {
    narvo_testkit::srgb::encode(ground.mul_add(1.0 - src_alpha, src_colour))
}

/// How far a measured byte may sit from the `f64` model before it counts.
///
/// Two counts. The model runs in `f64` and the shader in `f32`, and the encoder
/// rounds to nearest, so a value landing near a half-count can differ by one on
/// each side of the comparison. It is deliberately far below the distance
/// between the two readings this file separates — 60 counts at the half tint —
/// so no slack here could make an encoded-space multiply look linear.
const MODEL_SLACK: i32 = 2;

/// Asserts `measured` sits within [`MODEL_SLACK`] of `expected`.
fn assert_near(measured: u8, expected: u8, what: &str) {
    let distance = i32::from(measured) - i32::from(expected);
    assert!(
        distance.abs() <= MODEL_SLACK,
        "{what}: measured {measured}, model says {expected}, which is {distance} \
         counts apart and the slack is {MODEL_SLACK}"
    );
}

// --- Direction one: the identity tint changes nothing --------------------

/// An explicit identity tint renders exactly what no tint renders.
///
/// **The whole frame, not a sampled pixel.** V1 of this task is that a tint of
/// one moves zero pixels of ten blessed references; this is the same statement
/// made where it can be checked in one process, against a render taken through
/// the same call with the field left at its default.
///
/// It is not a tautology even though both sprites end up carrying
/// [`SpriteTint::UNTINTED`]: what it asserts is that the *builder* leaves the
/// value alone, and a `tinted` that mangled its argument — normalising it,
/// premultiplying it twice — would show here.
#[test]
fn the_identity_tint_renders_what_no_tint_renders() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = halves();
    let plain = [quad(48.0, white_of(&texture))];
    let identity = [quad(48.0, white_of(&texture)).tinted(SpriteTint::UNTINTED)];

    let without = target
        .render_sprites(&texture, &plain)
        .expect("one sprite is inside the batch limit");
    let with = target
        .render_sprites(&texture, &identity)
        .expect("one sprite is inside the batch limit");

    assert_eq!(
        without.rgba(),
        with.rgba(),
        "an identity tint moved a pixel, which makes every blessed reference \
         suspect rather than this test"
    );
}

/// A white texel under an identity tint reads back as the byte that went in.
///
/// The sRGB round trip is the identity on every byte, and the tint must not
/// disturb it. This is the pixel-level statement the test above makes about the
/// whole frame, and it is here because a *pair* of renders that were both
/// wrong in the same way would agree with each other and say nothing.
#[test]
fn a_white_texel_under_the_identity_tint_reads_back_white() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = halves();
    let frame = target
        .render_sprites(&texture, &[quad(48.0, white_of(&texture))])
        .expect("one sprite is inside the batch limit");

    assert_eq!(pixel(&frame, 32, 32), [255, 255, 255, 255]);
}

// --- Direction two: a tint that is not the identity ----------------------

/// A half tint lands where a **linear** multiply puts it, not where an encoded
/// one would.
///
/// **This is S1's measurement**, and it needs no argument: a white texel tinted
/// by `0.5` reads back as 188 if the product is taken in linear light and as
/// 128 if it is taken on the stored byte. The two are 60 counts apart, so no
/// rounding, no rasteriser and no slack can carry one into the other.
///
/// Both readings are named in the failure message on purpose. A bare "expected
/// 188, got 128" would look like an off-by-something; naming the second reading
/// says what a failure here would actually mean, which is that the pipeline's
/// formats moved.
#[test]
fn the_half_tint_lands_where_a_linear_multiply_puts_it() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = halves();
    let sprites = [quad(48.0, white_of(&texture)).tinted(SpriteTint::rgb(0.5, 0.5, 0.5))];

    let frame = target
        .render_sprites(&texture, &sprites)
        .expect("one sprite is inside the batch limit");

    let [red, green, blue, alpha] = pixel(&frame, 32, 32);
    let linear_reading = linear_product(255, 0.5);
    let encoded_reading = 128_u8;

    println!(
        "half tint on white: measured {red}, linear model {linear_reading}, \
         encoded model {encoded_reading}"
    );

    assert_near(
        red,
        linear_reading,
        "the red channel of a half-tinted white",
    );
    assert_eq!([red, green, blue], [red, red, red], "the channels disagree");
    assert_eq!(alpha, 255, "an opaque tint moved the alpha");

    assert!(
        i32::from(red) - i32::from(encoded_reading) > 10 * MODEL_SLACK,
        "a half tint on white read back as {red}, which is the encoded-space \
         product rather than the linear one. The multiplication is supposed to \
         be forced into linear light by the two sRGB formats; if this fails, one \
         of those formats has changed and SpriteTint's account of the space is \
         now wrong."
    );
}

/// A per-channel tint multiplies each channel on its own.
///
/// Non-vacuity with the answer computed rather than compared to "not equal": a
/// tint that swapped or shared channels would still be "different from
/// untinted".
#[test]
fn a_channel_tint_multiplies_each_channel_on_its_own() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = halves();
    let sprites = [quad(48.0, white_of(&texture)).tinted(SpriteTint::rgb(1.0, 0.5, 0.0))];

    let frame = target
        .render_sprites(&texture, &sprites)
        .expect("one sprite is inside the batch limit");

    let [red, green, blue, alpha] = pixel(&frame, 32, 32);

    assert_eq!(
        red, 255,
        "a factor of one is not the identity on the channel"
    );
    assert_near(green, linear_product(255, 0.5), "the green channel");
    assert_eq!(blue, 0, "a factor of zero left something behind");
    assert_eq!(alpha, 255, "an opaque tint moved the alpha");
}

/// The tinted and the untinted render differ **at the same pixels**.
///
/// The blunt half of non-vacuity, and it is here beside the computed
/// assertions rather than instead of them: a tint that silently did nothing
/// would pass `a_white_texel_under_the_identity_tint_reads_back_white` and fail
/// this.
#[test]
fn a_tint_that_is_not_the_identity_changes_the_pixels_it_covers() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = halves();
    let plain = [quad(48.0, white_of(&texture))];
    let tinted = [quad(48.0, white_of(&texture)).tinted(SpriteTint::rgb(0.5, 0.5, 0.5))];

    let without = target
        .render_sprites(&texture, &plain)
        .expect("one sprite is inside the batch limit");
    let with = target
        .render_sprites(&texture, &tinted)
        .expect("one sprite is inside the batch limit");

    assert_ne!(
        without.rgba(),
        with.rgba(),
        "a tint of 0.5 produced the same frame as no tint at all, so the tint \
         reaches no pixel"
    );

    // And the difference is under the sprite rather than beside it: a corner no
    // sprite covers reads the clear in both.
    assert_eq!(pixel(&without, 2, 2), pixel(&with, 2, 2));
    assert_ne!(pixel(&without, 32, 32), pixel(&with, 32, 32));
}

// --- The premultiplication invariant, where it is observable -------------

/// A half-transparent tint over a bright ground composites at half coverage.
///
/// **This is the invariant's only observable form, and the case ADR-0023 makes
/// load bearing.** The pipeline consumes premultiplied colour, so a tint with
/// an alpha of its own has to reach the colour channels too; a tint applied to
/// the alpha alone would leave a fragment brighter than its own coverage.
///
/// Over the pass's own black ground that defect is invisible — `dst * (1 - a)`
/// is zero there, so the composite is the source term whatever the alpha says.
/// Over a *bright* ground it is not: the correct arithmetic reads about 205 and
/// the broken one saturates at 255. The grey backdrop is drawn first, in the
/// same batch, which is why this test needs two sprites and not one.
#[test]
fn a_half_transparent_tint_over_a_bright_ground_composites_at_half_coverage() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = halves();
    let sprites = [
        // The ground: grey, untinted, covering the whole canvas.
        quad(64.0, grey_of(&texture)),
        // Over it: white at half coverage.
        quad(32.0, white_of(&texture)).tinted(SpriteTint {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 0.5,
        }),
    ];

    let frame = target
        .render_sprites(&texture, &sprites)
        .expect("two sprites are inside the batch limit");

    // Beside the foreground: the ground, untouched.
    assert_eq!(
        pixel(&frame, 4, 32),
        [GREY_BYTE, GREY_BYTE, GREY_BYTE, 255],
        "the untinted backdrop did not survive"
    );

    // `src + dst * (1 - a)` in linear light, with `src` the premultiplied white
    // at half coverage and `dst` the grey ground.
    let ground = narvo_testkit::srgb::decode(GREY_BYTE);
    let composite = over(0.5, 0.5, ground);

    let [red, green, blue, alpha] = pixel(&frame, 32, 32);
    println!("half-coverage white over grey: measured {red}, model {composite}");

    assert_near(red, composite, "the composite of a faded white over grey");
    assert_eq!([red, green, blue], [red, red, red], "the channels disagree");
    assert_eq!(
        alpha, 255,
        "the ground is opaque, so the composite must be too"
    );

    assert!(
        red < 255,
        "a white sprite at half coverage saturated the ground it was drawn over. \
         That is what happens when the tint's alpha reaches the alpha channel and \
         not the colour channels: the fragment is then brighter than its own \
         coverage, which breaks the premultiplied invariant ADR-0023 rests on."
    );
}

/// A batch of differently tinted sprites is drawn in one pass, each with its
/// own colour.
///
/// The structural claim measured through the renderer rather than through
/// `batch_runs`: the tint is a vertex attribute, so two tints cost one draw
/// call and neither leaks into the other. A tint that had become uniform state
/// would paint both sprites the same.
#[test]
fn two_sprites_in_one_batch_keep_their_own_tints() {
    let Some(target) = target_or_skip(EDGE, EDGE) else {
        return;
    };

    let texture = halves();
    let left = SpritePlacement {
        x: -16.0,
        y: 0.0,
        rot_cos: SpritePlacement::UNTURNED.0,
        rot_sin: SpritePlacement::UNTURNED.1,
        scale_x: 24.0,
        scale_y: 24.0,
    };
    let right = SpritePlacement { x: 16.0, ..left };

    let sprites = [
        SpriteInstance::new(left, white_of(&texture)).tinted(SpriteTint::rgb(1.0, 0.0, 0.0)),
        SpriteInstance::new(right, white_of(&texture)).tinted(SpriteTint::rgb(0.0, 0.0, 1.0)),
    ];

    let frame = target
        .render_sprites(&texture, &sprites)
        .expect("two sprites are inside the batch limit");

    assert_eq!(pixel(&frame, 16, 32), [255, 0, 0, 255], "the left sprite");
    assert_eq!(pixel(&frame, 48, 32), [0, 0, 255, 255], "the right sprite");
}

// --- The blessed scene --------------------------------------------------

/// The four cases a picture has to show, drawn at 128 x 128.
///
/// Two rows over a grey ground. The top row is an opaque sprite, untinted then
/// tinted; the bottom row is the same sprite at half coverage, untinted then
/// tinted. The bottom right is the one no arithmetic test can bless on its own:
/// a tint *and* a coverage, composited over a ground bright enough for a broken
/// premultiplication to saturate.
fn tint_scene(texture: &Pixels) -> Vec<SpriteInstance> {
    let sized = |x: f32, y: f32, width: f32, height: f32, region| {
        SpriteInstance::new(
            SpritePlacement {
                x,
                y,
                rot_cos: SpritePlacement::UNTURNED.0,
                rot_sin: SpritePlacement::UNTURNED.1,
                scale_x: width,
                scale_y: height,
            },
            region,
        )
        .sampled(SpriteFilter::Nearest)
    };
    let at = |x: f32, y: f32, region| sized(x, y, 48.0, 40.0, region);

    let white = white_of(texture);
    let orange = SpriteTint::rgb(1.0, 0.5, 0.125);
    let faded = SpriteTint {
        red: 1.0,
        green: 0.5,
        blue: 0.125,
        alpha: 0.5,
    };

    vec![
        // The ground, covering everything, and untinted so that a tint which
        // leaked into the wrong sprite would be visible against it.
        sized(
            0.0,
            0.0,
            SCENE_EDGE as f32,
            SCENE_EDGE as f32,
            grey_of(texture),
        ),
        // Top row: opaque, untinted then tinted.
        at(-30.0, 28.0, white),
        at(30.0, 28.0, white).tinted(orange),
        // Bottom row: half coverage, untinted then tinted.
        at(-30.0, -28.0, white).tinted(SpriteTint {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha: 0.5,
        }),
        at(30.0, -28.0, white).tinted(faded),
    ]
}

/// Where the ground shows through, so a scene that drew nothing is not silently
/// "grey".
fn ground_sample() -> (u32, u32) {
    (4, 64)
}

/// The centre of each of the four sprites, in framebuffer pixels.
///
/// The projection makes one world unit one pixel and puts the origin at the
/// centre, and framebuffer y runs down while world y runs up (ADR-0004) — so
/// world `+28` is the *upper* row at pixel y 36.
fn scene_samples() -> [((u32, u32), &'static str); 4] {
    [
        ((34, 36), "opaque, untinted"),
        ((94, 36), "opaque, tinted"),
        ((34, 92), "half coverage, untinted"),
        ((94, 92), "half coverage, tinted"),
    ]
}

/// The blessed scene against its reference.
///
/// # Errors it does not resolve
///
/// A missing reference is [`narvo_render2d::GoldenError::ReferenceMissing`],
/// which writes what it rendered under the cargo target directory and says to
/// ask a human. Nothing here creates the file: a self-blessed reference asserts
/// only that the renderer still agrees with itself.
#[test]
fn the_tint_scene_matches_its_golden_reference() {
    let Some(target) = target_or_skip(SCENE_EDGE, SCENE_EDGE) else {
        return;
    };

    let texture = halves();
    let rendered = target
        .render_sprites(&texture, &tint_scene(&texture))
        .expect("five sprites are far inside the batch limit");

    let references = reference_dir();
    let output = golden_artifact_dir();
    let golden = Golden::new(&references, &output);

    match golden.verify(TINT_SCENE, &rendered) {
        // The full measurement rather than the bare fact of passing: what the
        // margin is on this rasteriser is worth a line in a green log, because
        // reconstructing it later costs a run per platform.
        Ok(report) => println!(
            "golden match for {TINT_SCENE:?}: {}",
            report.measured_against(golden.tolerance)
        ),
        Err(error) => panic!("{error}"),
    }
}

/// The four sample points carry four **different** answers, and each is the one
/// the model computes.
///
/// **The reference cannot say this and this cannot say what the reference
/// says.** A picture is what a human blesses; a picture is also what nobody can
/// read a number out of. So the same render is measured here, at named points,
/// against arithmetic — which is what makes the blessing a statement about
/// *appearance* rather than a statement about correctness somebody hoped was
/// implied.
///
/// The four-way inequality is the part that matters most: three of these four
/// would still be distinct if the tint's alpha never reached the colour
/// channels.
#[test]
fn the_blessed_scene_reads_the_four_values_the_model_computes() {
    let Some(target) = target_or_skip(SCENE_EDGE, SCENE_EDGE) else {
        return;
    };

    let texture = halves();
    let frame = target
        .render_sprites(&texture, &tint_scene(&texture))
        .expect("five sprites are far inside the batch limit");

    let ground = narvo_testkit::srgb::decode(GREY_BYTE);
    let (ground_x, ground_y) = ground_sample();
    assert_eq!(
        pixel(&frame, ground_x, ground_y),
        [GREY_BYTE, GREY_BYTE, GREY_BYTE, 255],
        "the ground did not draw, so every sample below sits on nothing"
    );

    // Green is the channel the orange tint scales by a half, and the one the
    // coverage halves again — so it separates all four cases on its own.
    //
    // The fourth entry is the case the whole scene exists for. Its source
    // colour is `1.0 * 0.5 * 0.5` — the texel, the tint's green, the tint's
    // alpha — while its source alpha is `1.0 * 0.5`. The two differ, which is
    // exactly what a tint applied to the alpha alone would not produce.
    let expected_green = [
        255,
        linear_product(255, 0.5),
        over(0.5, 0.5, ground),
        over(0.25, 0.5, ground),
    ];

    let mut measured = Vec::new();
    for (((x, y), what), expected) in scene_samples().into_iter().zip(expected_green) {
        let [_, green, _, alpha] = pixel(&frame, x, y);
        println!("{what} at ({x}, {y}): green {green}, model {expected}");
        assert_near(green, expected, what);
        assert_eq!(alpha, 255, "{what} is drawn over an opaque ground");
        measured.push(green);
    }

    for (index, left) in measured.iter().enumerate() {
        for right in &measured[index + 1..] {
            assert_ne!(
                left, right,
                "two of the four cases read the same value, so the scene does not \
                 show four cases: {measured:?}"
            );
        }
    }
}

/// Writes a nearest-magnified copy of the blessed scene for a human to look at.
///
/// Not an assertion and not a reference. `tests/golden/` is untouched: blessing
/// an image is a human step, and this file only makes the image easy to look
/// at. A 128 x 128 render is smaller than the thing being judged in it — the
/// edge where a broken premultiplication would show is one pixel wide — so this
/// magnifies by whole texels rather than leaving the maintainer to zoom.
#[test]
fn a_magnified_preview_is_written_for_the_maintainer_to_look_at() {
    let Some(target) = target_or_skip(SCENE_EDGE, SCENE_EDGE) else {
        return;
    };

    let texture = halves();
    let rendered = target
        .render_sprites(&texture, &tint_scene(&texture))
        .expect("five sprites are far inside the batch limit");

    let factor = 4;
    let (width, height) = (rendered.width() * factor, rendered.height() * factor);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            rgba.extend_from_slice(&pixel(&rendered, x / factor, y / factor));
        }
    }
    let magnified = Pixels::from_rgba8(width, height, rgba).expect("a whole multiple is legal");

    let directory = golden_artifact_dir();
    std::fs::create_dir_all(&directory).expect("the artifact directory must be creatable");

    let plain = directory.join(format!("{TINT_SCENE}.preview.png"));
    rendered.save_png(&plain).expect("the preview is writable");
    let large = directory.join(format!("{TINT_SCENE}.preview.x{factor}.png"));
    magnified.save_png(&large).expect("the preview is writable");

    println!("preview written to {}", plain.display());
    println!("magnified preview written to {}", large.display());
}

/// Where this crate's blessed references live, resolved from the crate rather
/// than from whatever directory a test runner picked.
fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
}

/// An offscreen target, or `None` on a machine with no adapter at all.
fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
    match OffscreenTarget::new(width, height) {
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
        Err(other) => panic!(
            "creating the offscreen target failed for a reason other than a \
             missing adapter: {other}"
        ),
    }
}
