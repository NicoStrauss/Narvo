//! How far the picture actually moves when the camera does.
//!
//! # The question, and why a pixel count cannot answer it
//!
//! D14 (`ProjektPlan.md` §11) decided smooth camera movement through MSAA, and
//! M3.15 built it. Whether the movement became smooth is a separate question and
//! nobody has measured it. M3.13 showed the quantisation was never in the code
//! but in the rasterisation rule — one sample per pixel — and MSAA raises the
//! number of samples. **That half a pixel of camera therefore produces half a
//! pixel of movement is an inference, and this file is the measurement.**
//!
//! A count of differing pixels cannot answer it. M3.13's sharpest number on this
//! scene was 16 256 of 16 256 comparable pixels *matching* once one image was
//! shifted a whole column — a statement about whether a whole-pixel shift fits,
//! which cannot distinguish a picture that slid half a pixel from one that
//! jumped a whole one. The measurement has to yield a *displacement*, in pixels,
//! on a scale finer than a pixel.
//!
//! # Two measures, because they fail differently
//!
//! **The sub-pixel edge position**, and it is the primary one. Take a vertical
//! silhouette edge with background on one side. Under MSAA the column the edge
//! falls in is partly covered, and the covered fraction *is* the sub-pixel
//! position: a column covered `f` of the way has its edge at `column + (1 - f)`.
//! Coverage is read in **linear** light, because the target is `Rgba8UnormSrgb`
//! and the resolve averages before the encode — reading it from the stored byte
//! would report `sRGB(f)` and not `f`.
//!
//! This measure is direct, local to the silhouette, and has a known answer to
//! check against: at the identity camera sprite B's left edge is at exactly
//! `X = 48`, so a camera moved `d` to the right must put it at `48 - d`.
//!
//! **The intensity centroid of the whole frame**, as the second. If the content
//! translates rigidly by `δ`, its centroid translates by exactly `δ` — that is
//! what makes it a displacement measure rather than a difference measure, and it
//! needs no choice of which edge to watch. It is reported with the frame's total
//! intensity beside it, because the centroid is only a translation measure while
//! that total holds still: `Nearest` re-sampling the interior changes *which*
//! texel a pixel reads, which changes the mass rather than moving it.
//!
//! The two disagreeing is a result and not a fault. The edge measure sees the
//! silhouette alone; the centroid sees the silhouette and the interior together.
//!
//! # Predictions, written before the measurement ran
//!
//! Pre-MSAA, M3.13 measured this scene at these cameras and found whole-pixel
//! behaviour with a sign asymmetry: `offset_half_x` was `identity` shifted by
//! **one whole column**, while `(-0.5, 0)` was **byte-identical to identity** —
//! a half pixel of camera bought one pixel of movement in one direction and none
//! in the other.
//!
//! 1. **`offset_half_x` moves the edge by 0.5, not 1.0 and not 0.** The camera
//!    puts the edge on a pixel centre; four samples at x-offsets `0.125, 0.375,
//!    0.625, 0.875` put two of them past it, so the column is half covered.
//! 2. **The sign asymmetry is gone.** `(-0.5, 0)` moves the edge by `+0.5`,
//!    where M3.13 measured `0`.
//! 3. **`offset_half_xy` moves x exactly as `offset_half_x` does**, since the
//!    two differ only in y.
//! 4. **The motion is stepped, not continuous, at a quarter of a pixel.** Four
//!    sample positions across a pixel means the covered fraction takes five
//!    values — `0, 1/4, 2/4, 3/4, 1` — so over one pixel of travel the edge
//!    position takes **five distinct values** and moves in **four steps**.
//! 5. **On the prompt's "at most four coverage levels per edge":** four is the
//!    number of *steps*; the number of *levels* is five, counting both ends.
//!    Recorded as a prediction rather than adopted, because it was handed over as
//!    a guess.
//! 6. **The centroid moves less cleanly than the edge**, because the interior
//!    still samples once per pixel and snaps at texel boundaries — 4 pixels per
//!    texel in this scene.
//!
//! # Two fixtures, and only one of them carries the assurance (M3.25)
//!
//! Everything above was measured on [`atlas`], M3.12's scene: four 8 x 8
//! regions, each split into an upper and a lower half of different intensity,
//! and **deliberately unpadded**. The rigid measure is asserted on [`uniform_atlas`]
//! instead, and the reason is not a preference.
//!
//! **What the qualifier decides is where the varying is read**, and on this
//! atlas that decides *what colour comes back*. Under the default `center` the
//! one fragment invocation reads the uv at the pixel centre, which for a partly
//! covered pixel lies outside the primitive; the uv extrapolates past the
//! sprite's own region and, with no padding between regions, `Nearest` fetches
//! the *neighbouring sprite's* texel. `src/shaders/quad.wgsl` records that
//! mechanism and measured it rather than arguing it, on an unpadded atlas of
//! these same regions at a camera that puts sprite B's left edge at pixel 52.75:
//! that quarter-covered column came back `[137, 0, 0]`, a quarter of A's red,
//! where B's own region is green. **A different camera from this file's** — the
//! atlas and the three sprites are the ones here, the view is not.
//!
//! That has two consequences here, and they are the two failures M3.24b found
//! in this file:
//!
//! - **The intensity centroid stops being a rigid-translation measure.** An edge
//!   pixel that borrows a neighbour's colour changes the frame's *mass* rather
//!   than moving it, and the centroid is only a displacement measure while the
//!   mass holds still — this file's own opening says so. Under `center` the
//!   `offset_half_x` camera gains ~9.95 units of mass where `centroid` gains
//!   0.19, and the centroid moves −0.31 instead of −0.5 (M3.24c, a probe: named
//!   as history, not cited as a source).
//! - **The edge measure loses its finer positions.** [`edge_x`] reads coverage
//!   out of B's *green* channel, so a column that came back red reads as
//!   uncovered and the scan reports the next column instead. At the half-pixel
//!   cameras the pixel centre lands exactly on the sprite's edge and the fetch
//!   stays inside the region, which is why those five stay where they were; at
//!   the quarter-pixel positions of the sweep it does not.
//!
//! [`uniform_atlas`] holds one colour in every texel, so every sample point —
//! inside the primitive, at the pixel centre, extrapolated past the region, or
//! clamped to the texture's border — returns the same value. A pixel is then
//! `coverage x colour`, coverage is the rasteriser's and not a varying's, and
//! the qualifier has nothing left to change. **That is where D14's numbers are
//! asserted; the striped fixture keeps its edge assertion, prints its centroid
//! and mass, and labels them.**
//!
//! On uniform content the y axis also stops being the ragged one: the ~1.9 %
//! mass loss and the ~0.11 px bias the three y cameras show on [`atlas`] are the
//! upper/lower half split being re-sampled, and a single-colour atlas has no
//! such split. The uniform carrier therefore holds y to the same 0.01 as x.

use narvo_render2d::{
    CameraView, OffscreenTarget, Pixels, RenderError, SpriteInstance, SpritePlacement,
    TextureRegion,
};

/// Set this, to anything, and a missing adapter fails instead of skipping.
const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

/// Printed when a test cannot run for lack of an adapter.
const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

/// The render target's edge. Square, so one number.
const TARGET: u32 = 128;

/// Sprite B's left edge at the identity camera, in image coordinates.
///
/// B sits at world `(0, 0)` and is 32 across, so it spans world `-16..16`, and
/// `X = 64 + w` puts its left edge at `48`. Whole number by construction: the
/// camera is the only thing in this file that moves it, which is what makes a
/// measured `48 - d` a statement about the camera.
const B_LEFT_AT_IDENTITY: f64 = 48.0;

/// The row the edge is read on: inside sprite B's upper half and clear of the
/// other two sprites.
///
/// B spans image rows `48..80` and columns `48..80`; its region's upper half is
/// the first 16 rows of that.
///
/// **Sprite A lies underneath here**, and the first version of this comment
/// denied it: A is at world `(-20, 20)` and so spans image rows and columns
/// `28..60`, which covers column 48 on this row. `camera_margin.rs`, the file
/// this scene is copied from, says so plainly — the three sprites overlap in a
/// chain. The reading survives for a different reason than "nothing is there":
/// A's region is red, [`coverage`] reads only the channel carrying B's colour,
/// and B is drawn after A. A partly covered edge pixel over A therefore still
/// reports B's coverage in green, exactly as it would over the background.
const EDGE_ROW: u32 = 52;

/// Green, sprite B's upper-half colour, as the fully covered value.
const B_UPPER: [u8; 4] = narvo_testkit::TOP_RIGHT_UPPER;

/// The largest camera displacement any test in this file uses, in world units.
///
/// One: the sweeps run from 0 to 1 and the six cameras sit at half. The frame
/// guard reads this instead of a literal of its own, so a camera added beyond it
/// reddens that guard rather than quietly leaving it uncovered.
const FURTHEST_CAMERA: f64 = 1.0;

/// The 16 x 16 atlas M3.12 used: four 8 x 8 quadrants, upper and lower halves.
///
/// **Deliberately unpadded.** M3.21 gave the originals a one-texel border;
/// this file did not move with them, because it is a measurement
/// baseline rather than a re-derivation of the scene. Under `Nearest` the two
/// render the same image; under `Linear` they will not.
///
/// **The documentation fixture, not the assurance one** (M3.25) — see the module
/// documentation for what an unpadded neighbour does to a partly covered pixel.
fn atlas() -> Pixels {
    narvo_testkit::AtlasLayout::UNPADDED.atlas()
}

/// The same 16 x 16 texture with one colour in every texel — the fixture D14's
/// rigid displacement is asserted on.
///
/// [`B_UPPER`] is that colour, so [`coverage`] and [`edge_x_on_row`] read it
/// through the same channel they read the striped scene through. The scene, the
/// cameras and both measures are the striped fixture's; the texture is the one
/// variable that moves, and [`UNIFORM_EDGE_ROW`] is the one thing that has to
/// move with it.
fn uniform_atlas() -> Pixels {
    let edge = narvo_testkit::AtlasLayout::UNPADDED.edge();
    let rgba = B_UPPER.repeat((edge * edge) as usize);
    Pixels::from_rgba8(edge, edge, rgba).expect("the generated buffer matches its dimensions")
}

/// M3.12's scene, unchanged: three 32 x 32 sprites, one atlas region each.
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

fn target_or_skip() -> Option<OffscreenTarget> {
    match OffscreenTarget::new(TARGET, TARGET) {
        Ok(target) => {
            println!("adapter in use: {}", target.adapter_summary());
            Some(target)
        }
        Err(RenderError::NoAdapter { attempts }) => {
            assert!(
                std::env::var_os(REQUIRE_GPU_VAR).is_none(),
                "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a \
                 failure rather than a skip. Tried: {attempts}"
            );
            println!("{SKIP_MARKER}: no adapter. Tried: {attempts}");
            None
        }
        Err(other) => panic!("the offscreen target could not be created: {other}"),
    }
}

/// IEC 61966-2-1, written out rather than depended on.
///
/// The same transfer function `camera_scene.rs` uses for the other direction.
/// Coverage has to be read here and not from the stored byte: the resolve
/// averages the samples in linear light and the format encodes on write, so a
/// half-covered pixel stores `sRGB(0.5) = 188`, not `128`.
fn srgb_to_linear(byte: u8) -> f64 {
    let c = f64::from(byte) / 255.0;
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// How much of `full` covers this pixel, in linear light, `0.0..=1.0`.
///
/// Reads the one channel that carries `full`'s colour, so it is a coverage
/// against black and not a distance between two arbitrary colours.
fn coverage(pixel: [u8; 4], full: [u8; 4]) -> f64 {
    let channel = (0..3)
        .max_by_key(|&i| full[i])
        .expect("a colour has three channels");
    let numerator = srgb_to_linear(pixel[channel]);
    let denominator = srgb_to_linear(full[channel]);
    assert!(denominator > 0.0, "the reference colour must have some ink");
    (numerator / denominator).clamp(0.0, 1.0)
}

/// The sub-pixel image x of sprite B's left edge, on [`EDGE_ROW`].
///
/// Scans right until a column carries any of B's colour. That column is either
/// fully covered — the edge is on its left boundary — or partly, and then the
/// covered fraction says how far into it the edge falls. A fully covered column
/// preceded by an empty one puts the edge exactly on the boundary, which is the
/// identity case and the check that this function is reading what it thinks.
fn edge_x(image: &Pixels) -> f64 {
    edge_x_on_row(image, EDGE_ROW)
}

/// The row [`uniform_atlas`] is read on, and it has to be a different one.
///
/// On the striped atlas the scan can start at column 0 because sprite A is red
/// and [`coverage`] reads green. On a single-colour atlas all three sprites are
/// B's colour, so the scan would stop at A's left edge instead. At the identity
/// camera A's last row is 59 and C's first is 68, so row 64 has only B on it,
/// and nothing here moves by more than the one pixel of camera travel this file
/// uses.
///
/// B's own diagonal — the line `y = x` for a sprite spanning `48..80` in both
/// axes — crosses this row at column 64, sixteen columns right of the edge being
/// read. That is recorded rather than relied on: on uniform content the two
/// triangles read the same colour, so the diagonal has nothing to do here.
const UNIFORM_EDGE_ROW: u32 = 64;

/// [`edge_x`], on a row the caller chooses.
fn edge_x_on_row(image: &Pixels, row: u32) -> f64 {
    for x in 0..image.width() {
        let pixel = image.pixel(x, row).expect("inside the target");
        let covered = coverage(pixel, B_UPPER);
        if covered > 0.0 {
            return f64::from(x) + (1.0 - covered);
        }
    }
    panic!("sprite B's colour appears nowhere on row {row}");
}

/// The intensity-weighted centroid of the frame, and its total intensity.
///
/// Linear light again, and summed over the three colour channels: a rigid
/// translation of the content moves this by exactly the translation, which is
/// the property that makes it a displacement rather than a difference.
fn centroid(image: &Pixels) -> (f64, f64, f64) {
    let (mut sum_x, mut sum_y, mut mass) = (0.0, 0.0, 0.0);

    for y in 0..image.height() {
        for x in 0..image.width() {
            let pixel = image.pixel(x, y).expect("inside the target");
            let weight: f64 = (0..3).map(|i| srgb_to_linear(pixel[i])).sum();
            sum_x += weight * f64::from(x);
            sum_y += weight * f64::from(y);
            mass += weight;
        }
    }

    assert!(mass > 0.0, "an empty frame has no centroid");
    (sum_x / mass, sum_y / mass, mass)
}

fn render(target: &OffscreenTarget, texture: &Pixels, camera: CameraView) -> Pixels {
    target
        .render_sprites_viewed_by(texture, &scene(texture), camera)
        .expect("three sprites are far inside the batch limit")
}

/// The five displaced cameras, and what a rigid translation owes each of them.
///
/// `(label, camera, expected edge displacement, expected centroid dx, dy)`. The
/// identity camera is the sixth and is not in here: it is the baseline the other
/// five are measured against. One table for both fixtures, so the two
/// measurements differ in the texture and in nothing else.
fn cases() -> [(&'static str, CameraView, f64, f64, f64); 5] {
    [
        (
            "offset_half_x",
            CameraView::new(0.5, 0.0, 1.0),
            -0.5,
            -0.5,
            0.0,
        ),
        (
            "offset_half_xy",
            CameraView::new(0.5, 0.5, 1.0),
            -0.5,
            -0.5,
            0.5,
        ),
        (
            "minus_half_x",
            CameraView::new(-0.5, 0.0, 1.0),
            0.5,
            0.5,
            0.0,
        ),
        (
            "minus_half_y",
            CameraView::new(0.0, -0.5, 1.0),
            0.0,
            0.0,
            -0.5,
        ),
        (
            "minus_half_xy",
            CameraView::new(-0.5, -0.5, 1.0),
            0.5,
            0.5,
            -0.5,
        ),
    ]
}

/// **D14's rigid displacement, on content the uv qualifier cannot reach**
/// (M3.25).
///
/// Same scene, same six cameras, same two measures as
/// [`a_half_pixel_of_camera_moves_the_striped_edge_by_half_a_pixel`] — only the
/// texture differs, and it is [`uniform_atlas`]. On one colour a pixel is its
/// coverage and nothing else, so the centroid is a rigid-translation measure
/// again and can be held to 0.01 in **both** axes rather than 0.15 in y.
///
/// The mass is asserted too, and that is the premise rather than a result: the
/// centroid is only a displacement measure while the frame's total intensity
/// holds still. On the striped fixture it does not — the y cameras lose about
/// 1.9 % of it.
#[test]
fn a_half_pixel_of_camera_moves_uniform_content_by_half_a_pixel() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let texture = uniform_atlas();

    let base = render(&target, &texture, CameraView::IDENTITY);
    let (base_cx, base_cy, base_mass) = centroid(&base);
    let base_edge = edge_x_on_row(&base, UNIFORM_EDGE_ROW);

    println!(
        "uniform identity: edge x {base_edge:.4}, centroid ({base_cx:.4}, {base_cy:.4}), \
         mass {base_mass:.2}"
    );
    assert!(
        (base_edge - B_LEFT_AT_IDENTITY).abs() < 1e-9,
        "at the identity camera sprite B's left edge must be exactly \
         {B_LEFT_AT_IDENTITY}; measured {base_edge}. Everything below is a \
         displacement from this, so if it is wrong nothing else means anything."
    );

    println!("uniform case | edge x | edge moved | centroid dx | centroid dy | mass change");
    for (label, camera, want_edge, want_cx, want_cy) in cases() {
        let image = render(&target, &texture, camera);
        let measured_edge = edge_x_on_row(&image, UNIFORM_EDGE_ROW);
        let (cx, cy, mass) = centroid(&image);
        let (moved, dx, dy) = (measured_edge - base_edge, cx - base_cx, cy - base_cy);

        println!(
            "{label} | {measured_edge:.4} | {moved:+.4} (want {want_edge:+.1}) | \
             {dx:+.4} (want {want_cx:+.1}) | {dy:+.4} (want {want_cy:+.1}) | {:+.4}",
            mass - base_mass
        );

        assert!(
            (moved - want_edge).abs() < 0.01,
            "{label}: the edge moved {moved:+.4} pixels where the camera asked \
             for {want_edge:+.1}. A half pixel of camera has to buy a half pixel \
             of picture, or D14 is not finished."
        );
        assert!(
            (dx - want_cx).abs() < 0.01,
            "{label}: the centroid moved {dx:+.4} in x, not {want_cx:+.1}"
        );
        assert!(
            (dy - want_cy).abs() < 0.01,
            "{label}: the centroid moved {dy:+.4} in y, not {want_cy:+.1}. On one \
             colour there is no upper/lower half to re-sample, so y is held to \
             the same bound as x."
        );
        assert!(
            (mass - base_mass).abs() < base_mass * 0.005,
            "{label}: the frame's total intensity moved by {:+.4} of {base_mass:.2}. \
             A translation moves the picture; it does not change how much ink is \
             in it, and the centroid stops being a displacement measure as soon \
             as it does.",
            mass - base_mass
        );

        // The fixture's own premise, checked rather than assumed: on a
        // single-colour atlas nothing can put ink in a channel that colour has
        // none in. A sample point that left the region would show up here.
        for y in 0..image.height() {
            for x in 0..image.width() {
                let pixel = image.pixel(x, y).expect("inside the target");
                assert!(
                    pixel[0] == 0 && pixel[2] == 0,
                    "{label}: pixel ({x}, {y}) is {pixel:?}, which carries ink in \
                     a channel {B_UPPER:?} has none in. Something other than the \
                     uniform atlas was sampled, and this fixture's \
                     qualifier-independence rests on that not happening."
                );
            }
        }
    }
}

/// **The documentation measurement.** Six cameras on M3.12's striped, unpadded
/// scene, and what each does to the picture.
///
/// The **edge** displacement is still asserted here: at a half-pixel camera the
/// pixel centre lands exactly on the sprite's edge, so the fetch stays inside
/// the region whichever point the varying is read at, and the number is the same
/// under both qualifiers. The **centroid and mass** columns are printed and not
/// asserted since M3.25 — on this atlas they depend on the qualifier, and D14's
/// rigid displacement moved to
/// [`a_half_pixel_of_camera_moves_uniform_content_by_half_a_pixel`].
#[test]
fn a_half_pixel_of_camera_moves_the_striped_edge_by_half_a_pixel() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let texture = atlas();

    let base = render(&target, &texture, CameraView::IDENTITY);
    let (base_cx, base_cy, base_mass) = centroid(&base);
    let base_edge = edge_x(&base);

    println!(
        "identity: edge x {base_edge:.4}, centroid ({base_cx:.4}, {base_cy:.4}), mass {base_mass:.2}"
    );
    assert!(
        (base_edge - B_LEFT_AT_IDENTITY).abs() < 1e-9,
        "at the identity camera sprite B's left edge must be exactly \
         {B_LEFT_AT_IDENTITY}; measured {base_edge}. Everything below is a \
         displacement from this, so if it is wrong nothing else means anything."
    );

    let cases = cases();

    println!("case | edge x | edge moved | centroid dx | centroid dy | mass change");
    let mut edge_moves = Vec::new();

    for (label, camera, want_edge, want_cx, want_cy) in cases {
        let image = render(&target, &texture, camera);
        let measured_edge = edge_x(&image);
        let (cx, cy, mass) = centroid(&image);

        let moved = measured_edge - base_edge;
        println!(
            "{label} | {measured_edge:.4} | {moved:+.4} (want {want_edge:+.1}) | \
             {:+.4} (want {want_cx:+.1}) | {:+.4} (want {want_cy:+.1}) | {:+.4}",
            cx - base_cx,
            cy - base_cy,
            mass - base_mass
        );
        edge_moves.push((label, moved, want_edge));
    }

    for (label, moved, want) in edge_moves {
        assert!(
            (moved - want).abs() < 0.01,
            "{label}: the edge moved {moved:+.4} pixels where the camera asked \
             for {want:+.1}. A half pixel of camera has to buy a half pixel of \
             picture, or D14 is not finished."
        );
    }

    // **The centroid columns are printed above and asserted nowhere in this
    // test, since M3.25.** From M3.16 to M3.24 they were asserted here — x to
    // 0.01, y to the 0.15 the upper/lower half re-sampling forced. Both are
    // statements about the striped, unpadded fixture *under `centroid`*: an edge
    // pixel that borrows the neighbouring region's colour changes the frame's
    // mass rather than moving it, and which colour it borrows is exactly what
    // the uv qualifier decides. Measured under `center` in M3.24c and again in
    // M3.25's flip demonstration, `offset_half_x` moves the centroid −0.3063
    // rather than −0.5 while the mass gains +9.9486 —
    // that is the fixture reacting to the qualifier, not the picture failing to
    // move, and the edge assertion above says so from the other side by staying
    // put.
    //
    // The assurance itself did not go away: it is
    // `a_half_pixel_of_camera_moves_uniform_content_by_half_a_pixel`, which
    // holds both axes to 0.01 on content the qualifier cannot reach.
    // `BASELINE.md`'s camera-motion section carries the same note.
}

/// One pixel of camera travel in eighths, reading the sub-pixel edge position.
///
/// The sweep is run over both fixtures and the two are printed side by side.
/// `row` is [`EDGE_ROW`] for the striped atlas and [`UNIFORM_EDGE_ROW`] for the
/// uniform one — see [`UNIFORM_EDGE_ROW`] for why they cannot be the same row.
fn edge_sweep(target: &OffscreenTarget, texture: &Pixels, row: u32, label: &str) -> Vec<f64> {
    let steps = 8_u32;
    let mut positions = Vec::new();

    for k in 0..=steps {
        #[expect(
            clippy::cast_precision_loss,
            reason = "k is at most 8 and every eighth is exact in f32"
        )]
        let camera_x = k as f32 / steps as f32;
        let image = render(target, texture, CameraView::new(camera_x, 0.0, 1.0));
        let x = edge_x_on_row(&image, row);
        println!("{label} camera x {camera_x:.3}: edge x {x:.4}");
        positions.push(x);
    }

    positions
}

/// How many distinct positions the edge takes across one pixel of travel.
///
/// Eighth-pixel camera steps, which is finer than the quarter the four sample
/// positions can express — so a continuous answer and a stepped one look
/// different rather than both looking like "it moved".
///
/// **Asserted on [`uniform_atlas`] since M3.25**, and the striped sweep printed
/// beside it. The two agree under `centroid` — coverage is the same and only the
/// sampled colour differed — so the committed positions
/// `48.0000, 47.7498, 47.7498, 47.4971, 47.4971, 47.2471, 47.2471, 47.0000,
/// 47.0000` are the uniform fixture's too. Under `center` only the uniform one
/// keeps them: on the striped atlas a quarter-covered column's uv extrapolates
/// into sprite A's red region, [`coverage`] reads green and sees nothing, and
/// the scan reports the next column instead. The module documentation gives the
/// mechanism.
#[test]
fn the_edge_advances_in_quarter_pixel_steps_and_there_are_four_of_them() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let positions = edge_sweep(&target, &uniform_atlas(), UNIFORM_EDGE_ROW, "uniform");

    let mut levels: Vec<f64> = Vec::new();
    for x in &positions {
        if !levels.iter().any(|seen: &f64| (seen - x).abs() < 1e-9) {
            levels.push(*x);
        }
    }
    levels.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a coverage"));

    let transitions = positions
        .windows(2)
        .filter(|pair| (pair[0] - pair[1]).abs() > 1e-9)
        .count();

    println!(
        "distinct edge positions across one pixel: {} — {levels:?}",
        levels.len()
    );
    println!("transitions: {transitions}");

    assert_eq!(
        levels.len(),
        narvo_render2d::SAMPLE_COUNT as usize + 1,
        "a vertical edge sweeping one pixel over {} samples must take {} distinct \
         positions, one per coverage level including both ends. Measured \
         {levels:?}",
        narvo_render2d::SAMPLE_COUNT,
        narvo_render2d::SAMPLE_COUNT + 1
    );
    assert_eq!(
        transitions,
        narvo_render2d::SAMPLE_COUNT as usize,
        "and it must get there in {} steps",
        narvo_render2d::SAMPLE_COUNT
    );

    // The steps are even: a quarter of a pixel each, not four ragged ones.
    //
    // Within 0.01 and not exactly, and the slack is this file's rather than the
    // renderer's. Coverage is recovered from a stored byte, so a half-covered
    // pixel is `sRGB(0.5)` rounded to 188 of 255 and decoded back to 0.4971, not
    // 0.5. The measured gaps are 0.2502, 0.2527, 0.2500 and 0.2471: quarters
    // seen through eight bits. A tolerance tighter than the read-back would be
    // asserting the precision of the PNG, not of the movement.
    let quarter = 1.0 / f64::from(narvo_render2d::SAMPLE_COUNT);
    for pair in levels.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            (gap - quarter).abs() < 0.01,
            "the edge positions are {levels:?}, which are not evenly spaced by a \
             quarter of a pixel to within the 8-bit read-back"
        );
    }

    // The documentation half: the same sweep over M3.12's striped, unpadded
    // atlas, printed and not asserted. It runs after the assertions above so it
    // can never stand between them and a failure.
    let striped = edge_sweep(&target, &atlas(), EDGE_ROW, "striped");
    println!("striped edge positions across one pixel (qualifier-dependent): {striped:?}");
}

/// The row where sprite B's upper half meets its lower half.
///
/// An *interior* texel boundary, not a silhouette: both sides are opaque sprite,
/// so nothing is partly covered and MSAA has no coverage to grade. Whatever this
/// returns is what `Nearest` picked at the pixel centre, which is the rule M3.13
/// found the quantisation in.
///
/// Scans down column [`INTERIOR_COLUMN`] from inside B's upper half and returns
/// the first row that is no longer the upper colour.
fn interior_boundary_row(image: &Pixels) -> u32 {
    for y in 50..78 {
        let pixel = image.pixel(INTERIOR_COLUMN, y).expect("inside the target");
        if pixel != B_UPPER {
            return y;
        }
    }
    panic!("sprite B's upper half never ends on column {INTERIOR_COLUMN}");
}

/// A column well inside sprite B, clear of its vertical edges **and of its own
/// diagonal**.
///
/// B spans image columns `48..80` at the identity camera and moves by at most a
/// pixel here, so anything near the middle is never partly covered by the
/// silhouette. 64 would be the obvious choice and is the wrong one: a quad is
/// two triangles sharing the diagonal from its top-left corner to its
/// bottom-right, which for B is the line `y = x`, and that runs exactly through
/// the centre of pixel `(64, 64)` — the pixel this scan would read. That is the
/// configuration `camera_pan_steps.rs` singles out as blending: under `centroid`
/// at six of its seventeen camera positions, and under the `center` this
/// renderer ships since M3.26 still at one of them, where the pixel centre lands
/// exactly on a texel boundary and the two triangles' interpolations of the same
/// uv plane disagree in the last bit. The claim "nothing here is partly covered"
/// would be false at the one pixel it is made about, under either qualifier.
///
/// 58 instead. The diagonal crosses this column at row 58, which is fourteen
/// rows above the colour boundary at 64 and inside a single texel row, so the
/// two triangles read the same texel there and the scan meets nothing blended
/// on its way down.
const INTERIOR_COLUMN: u32 = 58;

/// **The decisive pair.** How the interior moves, against how the silhouette
/// moves, over the same travel.
///
/// The silhouette's step size is what MSAA changed. The interior's is what it
/// could not: `Nearest` still takes one sample per pixel, so an interior texel
/// boundary can only ever land on a whole pixel. Measuring both over one pixel
/// of camera travel is what turns "is the motion smooth" into two numbers.
#[test]
fn the_interior_still_steps_a_whole_pixel_while_the_silhouette_steps_a_quarter() {
    let Some(target) = target_or_skip() else {
        return;
    };
    let texture = atlas();

    let steps = 8_u32;
    let mut rows = Vec::new();

    for k in 0..=steps {
        #[expect(
            clippy::cast_precision_loss,
            reason = "k is at most 8 and every eighth is exact in f32"
        )]
        let camera_y = k as f32 / steps as f32;
        let image = render(&target, &texture, CameraView::new(0.0, camera_y, 1.0));
        let row = interior_boundary_row(&image);
        println!("camera y {camera_y:.3}: interior boundary at row {row}");
        rows.push(row);
    }

    let mut levels = rows.clone();
    levels.dedup();
    let transitions = rows.windows(2).filter(|pair| pair[0] != pair[1]).count();

    println!("interior boundary positions across one pixel: {levels:?}");
    println!(
        "interior transitions: {transitions}; the silhouette's are measured by \
         the_edge_advances_in_quarter_pixel_steps_and_there_are_four_of_them, \
         which asserts {}",
        narvo_render2d::SAMPLE_COUNT
    );

    // `dedup` only removes *consecutive* repeats, so a boundary that moved
    // backwards and returned would be counted as two levels and hidden here.
    // This is what that would look like, and it cannot be true of a forward pan.
    assert!(
        rows.windows(2).all(|pair| pair[1] >= pair[0]),
        "the interior boundary moved backwards across a forward pan: {rows:?}"
    );
    assert_eq!(
        transitions, 1,
        "the interior boundary moved in {transitions} steps across one pixel of \
         travel. It can only ever be a whole number of pixels — `Nearest` takes \
         one sample per pixel and MSAA does not multiply that — so one step per \
         pixel is the expected answer and anything else means this measurement \
         is not reading what it thinks. Rows: {rows:?}"
    );
}

/// The scene stays inside the target at every camera this file uses.
///
/// Runs without a GPU. The centroid is only a displacement measure while nothing
/// leaves the frame; content clipped at an edge would move it without the
/// picture moving, and the result would look like smooth motion.
#[test]
fn nothing_leaves_the_frame_at_any_camera_this_file_uses() {
    let texture = atlas();
    let sprites = scene(&texture);
    // Taken from the cameras rather than written down beside them: a literal
    // here would go stale the moment a seventh camera was added, and this test
    // would keep its name while quietly stopping being true.
    let widest = FURTHEST_CAMERA;

    // Through `Projection`, not by hand. Re-deriving `X = 64 + w` here would
    // make this guard blind to exactly the kind of change it exists to survive.
    let projection = narvo_render2d::Projection::for_target(TARGET, TARGET);
    let corner = |x: f32, y: f32| {
        let ndc = projection
            .viewed_by(CameraView::IDENTITY)
            .world_to_ndc(x, y);
        let half = f64::from(TARGET) / 2.0;
        (
            f64::from(ndc[0]).mul_add(half, half),
            half - f64::from(ndc[1]) * half,
        )
    };

    for sprite in sprites {
        let p = sprite.placement;
        let (left_x, top_y) = corner(p.x - p.scale_x / 2.0, p.y + p.scale_y / 2.0);
        let (right_x, bottom_y) = corner(p.x + p.scale_x / 2.0, p.y - p.scale_y / 2.0);

        let left = left_x - widest;
        let right = right_x + widest;
        let top = top_y - widest;
        let bottom = bottom_y + widest;

        assert!(
            left > 0.0 && top > 0.0 && right < f64::from(TARGET) && bottom < f64::from(TARGET),
            "a sprite reaches {left}..{right} x {top}..{bottom} once the camera \
             has moved by {widest}, which does not fit in {TARGET}"
        );
    }
}
