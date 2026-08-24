//! How finely a pan actually moves, now that the renderer draws at
//! [`narvo_render2d::SAMPLE_COUNT`] samples per pixel.
//!
//! # The question, and why counting changed pixels does not answer it
//!
//! D14 (`ProjektPlan.md` §11) decided smooth camera movement through coverage
//! antialiasing. M3.13 established why it was not smooth before: with one
//! sample per pixel a pan changes nothing at all until an edge crosses a pixel
//! *centre*, so the whole image sits still and then jumps by a pixel. §12 of
//! `ProjektPlan.md` asks M3.15 the follow-up — whether MSAA makes the movement
//! smooth, "or whether the quantisation from M3.13 survives somewhere else".
//!
//! "Smooth" is not a pixel count. It is a step size, and the way to measure a
//! step size is to sweep the camera across exactly one pixel in equal parts and
//! ask **at which parts the image changes at all**.
//!
//! # Two fixtures, and only one of them carries the assurance (M3.25)
//!
//! The same sweep runs over two atlases, and they answer two different
//! questions.
//!
//! - **Uniform content carries D14's number.** Every texel of
//!   [`uniform_atlas`] holds one colour, so *every* sample point — inside the
//!   primitive, at the pixel centre, extrapolated past the region's edge, or
//!   clamped — reads the same texel value. A pixel is then `coverage x colour`
//!   and nothing else, and coverage is decided by the rasteriser rather than by
//!   an interpolated varying. That is why
//!   [`a_uniform_sprite_moves_its_silhouette_four_times_in_quarter_pixel_steps`]
//!   asserts the four quarter-pixel steps and this file's striped test no longer
//!   does.
//! - **Striped content documents the interaction and asserts only what survives
//!   it.** [`atlas`] paints four colours across the region, so the sampled value
//!   changes with the sample *point* as well as with coverage — and the sample
//!   point is exactly what the uv interpolation qualifier decides.
//!
//! ## Why striped content makes the step count qualifier-dependent
//!
//! Under the default `center` the one fragment invocation reads the varying at
//! the pixel centre, which for a partly covered pixel lies **outside** the
//! primitive; the uv extrapolates past the sprite's own atlas region and
//! `Nearest` fetches the neighbouring texel. That is the mechanism
//! `src/shaders/quad.wgsl` records, measured there rather than argued. Under
//! `centroid` the sample point is dragged back inside the covered area.
//!
//! Two consequences meet in this fixture, and both were measured in M3.24c,
//! M3.24d and M3.25's flip demonstration (probes, no commit — named as history,
//! not cited as a source):
//!
//! 1. **A coverage step is not a texel flip.** The four genuine coverage steps
//!    move *both* silhouette columns, 39 and 87, and sit at `k = 2, 6, 10, 14`
//!    under `Nearest` in **both** qualifiers. Column 39's byte sequence —
//!    `0, 137, 188, 225, 255` — is identical in all four qualifier x sampler
//!    configurations measured; under `Linear` the aggregate step list stops
//!    discriminating in either qualifier, because a continuous interior moves
//!    almost every column at almost every sub-position.
//! 2. **Column 87 is boundary-coincident**, and that is what the qualifier
//!    moves. It is a silhouette column *and* it sits on the region's outer texel
//!    boundary: 48 pixels over 8 texels is 6 pixels per texel, so at `k = 8` the
//!    camera sits at exactly half a pixel and every texel boundary — the seven
//!    interior ones and this one — lands exactly on a pixel centre. Under
//!    `center` that column's sample therefore steps out of the region together
//!    with the interior and reads a neighbouring texel; under `centroid` it does
//!    not. The instrument, which counts "did any silhouette column change",
//!    scores that as a fifth silhouette move: `[2, 6, 8, 10, 14]` instead of
//!    `[2, 6, 10, 14]`.
//!
//! The size of the k = 8 event says which of the two it is. Down column 87, in
//! summed intensity at row 64, the four coverage steps under `centroid` are
//! −60, −74, −102, −274. Those are not quoted from elsewhere: the striped test
//! below prints that column's before and after colour at every step it moves,
//! and these are those pairs summed over R + G + B. Under `center` the same
//! column reads
//! −60, −74, **−188**, −51, −137 (M3.24d, a probe), and the k = 8 drop is larger
//! than any coverage step in either column: it is a change of *colour* under
//! unchanged coverage. Both `Nearest` columns nevertheless run 510 → 0 and so
//! sum to −510 — the qualifier redistributes *when* the value changes, not *how
//! much*.
//!
//! Uniform content has no texel boundary to cross and no neighbouring colour to
//! reach, so neither consequence has anything to act on. The assurance moved
//! there; the striped numbers stayed here, printed and labelled.
//!
//! # The sweep
//!
//! One sprite, 48 pixels across, sampling an 8-texel-wide region: 6 pixels per
//! texel. Zoom 1, so a camera step of `1/16` world unit is `1/16` of a pixel,
//! and both are exact in binary. The camera moves from `0` to `1` in sixteen
//! such steps, which is one pixel of travel.
//!
//! # What this file measures that a second render could not
//!
//! **The run carries its own control.** Two things move in the same image and
//! are governed by different rules:
//!
//! - the sprite's **silhouette** — its left and right edge columns — is decided
//!   by four-sample coverage;
//! - its **interior** is decided by `Nearest` sampling at one point per pixel,
//!   which MSAA does not multiply. A texel boundary moves the interior only
//!   when it crosses a pixel centre, exactly the rule M3.13 described.
//!
//! So the same sixteen renders show the antialiased step size and the
//! unantialiased one side by side, without a second pipeline and without
//! trusting a number recorded on another day.
//!
//! # Predictions, written before the measurement ran
//!
//! 1. **The silhouette changes four times**, once per sample position. The four
//!    x offsets of the standard four-sample pattern are `0.125`, `0.375`,
//!    `0.625` and `0.875`, so the edge crosses one every four steps: at
//!    `k = 2, 6, 10, 14` if a sample exactly on a left edge counts as covered,
//!    and at `k = 3, 7, 11, 15` if it does not. Which of the two is a fill-rule
//!    question this file does not presume to answer — it prints which occurred.
//! 2. **The interior changes once**, at `k = 8`. Texel boundaries sit 6 pixels
//!    apart, all at the same sub-pixel phase, so all of them cross a pixel
//!    centre in the same step — the one where a boundary passes `X.5`.
//! 3. **The step size is therefore quartered, not abolished.** One pixel of pan
//!    still produces a finite number of images: six, if 1 and 2 hold and their
//!    change points do not coincide.
//!
//! If prediction 2 fails and the interior also moves four times, MSAA reached
//! further than this file claims. If prediction 1 fails and the silhouette moves
//! once, it reached nothing and D14 bought nothing.
//!
//! # What happened: 1 held, 2 did not, and the reason is the quad's diagonal
//!
//! **Prediction 1 was exact.** The silhouette moved at `k = 2, 6, 10, 14`, so a
//! sample sitting exactly on a left edge does count as covered, and the step
//! size is a quarter of a pixel rather than a whole one.
//!
//! **Prediction 2 was wrong**, and it was wrong because it treated the sprite as
//! one surface. It is two triangles split along the diagonal from its top-left
//! corner to its bottom-right. What the run shows is the split: the interior
//! crosses its texel boundary at `k = 8` above the diagonal and at `k = 9` below
//! it, and the one pixel *on* the diagonal does something else again. What the
//! run cannot show is the cause - a last-bit difference between the two
//! triangles' interpolation of the same uv plane is the explanation this shape
//! fits, and it was not measured directly.
//!
//! That pixel is partly covered by each triangle, so `centroid` - which exists
//! to keep a partly covered pixel's sample point inside its own primitive -
//! hands the two triangles two different sample points, and they land in two
//! different texels. The resolve then averages two texel colours in quarters.
//! Measured at (45, 46), where the diagonal crosses a texel boundary, **on the
//! AMD adapter**; the two software rasterisers produce a sequence of the same
//! kind with each blended run one position shorter, and only their step numbers
//! and blend counts were kept, in the three-rasteriser table further down.
//!
//! | k | 0-7 | 8-10 | 11 | 12-14 | 15-16 |
//! | --- | --- | --- | --- | --- | --- |
//! | `centroid` | `[255, 0, 0]` | `[225, 137, 0]` | `[0, 255, 0]` | `[188, 188, 0]` | `[0, 255, 0]` |
//! | `center` | `[255, 0, 0]` | `[225, 137, 0]` at `k = 8`, then `[0, 255, 0]` | `[0, 255, 0]` | `[0, 255, 0]` | `[0, 255, 0]` |
//!
//! `[225, 137, 0]` is one quarter and `[188, 188, 0]` two quarters of the next
//! texel's green over this one's red, on the same sRGB scale as every other
//! number here.
//!
//! **`centroid` widens this seam; it does not create it.** That second row is
//! measured, not argued — the shader was switched to `center` and the sweep re-run
//! — and it corrects what this paragraph said before the measurement, which was
//! that `center` leaves the pixel uniform. It does not: at `k = 8` the pixel
//! centre lands exactly on the texel boundary and the two triangles' *centre*
//! interpolations already disagree in the last bit, so one reads texel 0 and the
//! other texel 1. `centroid` turns that one blended camera position into six.
//!
//! **The second row is the one that ships**, since M3.26 carried out D17. The
//! test below prints the count every run: 1 of 17 where `centroid` gave 6.
//!
//! The artefact is bounded either way - only pixels where a quad's internal
//! diagonal crosses a texel boundary, seven of this sprite's 2304. What
//! `centroid` bought for it was every silhouette pixel reading its own region's
//! colour instead of a neighbouring sprite's, and **D17 decided that trade the
//! other way**: padding and `ClampToEdge` make the place `center` reaches a copy
//! of the place it should have read, so on padded content the protection buys
//! nothing while the seam still costs something.
//! `src/shaders/quad.wgsl` carries the argument.
//!
//! The two ways out that would have neither artefact are a per-vertex region
//! clamp in the fragment shader, which widens the vertex format, and per-sample
//! shading, which costs four fragment invocations per pixel and would blend
//! *every* pixel a texel boundary crosses rather than only the diagonal ones -
//! the interior blending this file's own assertion records as absent today. Both
//! remain unmeasured, and neither was needed.
//!
//! **Prediction 3 held in substance, not in its number.** One pixel of pan
//! produces ten distinct images rather than six. Three of the four extra ones
//! are the seven seam pixels flickering: steps 11, 12 and 15 change nothing
//! else. The fourth is the interior's own flip splitting in two - everything
//! above the diagonal at `k = 8` and everything below it at `k = 9`, hundreds of
//! pixels each - which is the picture moving in halves, not a pixel flickering.
//!
//! # Three rasterisers, and which half of the answer is portable
//!
//! | | silhouette steps | control pixel (45, 64) flips at | seam positions blended, of 17 |
//! | --- | --- | --- | --- |
//! | AMD Radeon RX 9070 XT, Vulkan | 2, 6, 10, 14 | `k = 9` | 6 |
//! | Microsoft Basic Render Driver, Dx12 | 2, 6, 10, 14 | `k = 8` | 4 |
//! | llvmpipe, Vulkan | 2, 6, 10, 14 | `k = 8` | 4 |
//!
//! The third column is the control pixel below the diagonal, not the interior as
//! a whole: on AMD the interior flips at 8 *above* the diagonal and 9 below, and
//! the test prints both. The fourth counts camera positions, of the seventeen
//! the sweep visits, not steps - there are sixteen of those.
//!
//! **The half D14 bought is portable and the half it did not is not.** All three
//! put the silhouette's four steps in the same places. Everything else they
//! disagree about follows from one step, `k = 8`, where the camera sits at
//! exactly `0.5` and the pixel centre lands *exactly* on a texel boundary —
//! `(5.5 + 0.5) / 6 = 1.0`. Which side of an exact integer `floor` falls on is
//! decided by the last bit of an interpolated value, and the three do not agree;
//! AMD's split into 8-and-9 is also what lengthens each of its blended runs by
//! one position, which is the 6 against 4 in the last column. One difference
//! does not follow from that step, and is noted at the end.
//!
//! **That divergence is not MSAA's**, and this file does not have to reason
//! about it: with the shader temporarily switched to `center` the same
//! disagreement appeared at the same step on both AMD and llvmpipe. What MSAA
//! *did* change is the silhouette, and there the three agree exactly.
//!
//! The practical consequence is a hazard worth stating once: **a scene that puts
//! a pixel centre exactly on a texel boundary cannot be blessed**, because two
//! rasterisers will render it differently by a whole texel. None of the five
//! blessed images does — with an integer sprite edge the offset `px + 0.5 - X0`
//! is a half-integer, so it is never a whole number of texels. It takes a
//! half-pixel edge, which is what this sweep constructs on purpose.
//!
//! The one blend the three do disagree about is `[188, 188, 0]` against
//! llvmpipe's `[187, 187, 0]`, one count, inside the noise floor of four that
//! `Tolerance::default()` already allows.

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

/// The atlas's edge, in texels.
const ATLAS: u32 = 16;

/// Steps the camera takes across one pixel. Sixteenths are exact in binary and
/// fine enough to separate quarters from wholes with room to spare.
const STEPS: u32 = 16;

/// The sprite's edge in pixels, and its region's edge in texels: 6 pixels per
/// texel, so a texel boundary is a coarser grid than the pixel grid and the
/// interior's step size is visible as something other than 1.
const SPRITE_EDGE: f32 = 48.0;
const REGION_TEXELS: u32 = 8;

/// Columns the sprite's silhouette occupies at camera 0, and the interior
/// between them.
///
/// At zoom 1 with the camera at the origin, world `w` lands at `X = 64 + w`, so
/// a 48-pixel sprite centred on the origin has its edges at `40` and `88`.
/// Panning right by up to one pixel moves those to `39` and `87`, so across the
/// sweep the sprite touches columns `39..=87`: column 39 is where the left
/// edge's coverage grows and column 87 is where the right edge's shrinks.
const LEFT_EDGE_COLUMNS: [u32; 2] = [39, 40];
const RIGHT_EDGE_COLUMNS: [u32; 2] = [87, 88];

/// A 16 x 16 atlas whose top-left 8 x 8 quadrant has four differently coloured
/// texel columns, so an interior shift of one texel is visible as a colour
/// change rather than as nothing.
///
/// **The documentation fixture, not the assurance one.** Because the sampled
/// colour changes with the sample *point*, what this atlas shows at the
/// silhouette depends on the uv interpolation qualifier — see the module
/// documentation. [`uniform_atlas`] is the fixture D14's numbers are asserted
/// on.
fn atlas() -> Pixels {
    let mut rgba = Vec::with_capacity((ATLAS * ATLAS * 4) as usize);
    for _ in 0..ATLAS {
        for x in 0..ATLAS {
            rgba.extend_from_slice(&match x % 4 {
                0 => [255, 0, 0, 255],
                1 => [0, 255, 0, 255],
                2 => [0, 0, 255, 255],
                _ => [255, 255, 0, 255],
            });
        }
    }
    Pixels::from_rgba8(ATLAS, ATLAS, rgba).expect("the generated buffer matches its dimensions")
}

/// The colour every texel of [`uniform_atlas`] holds.
///
/// One saturated channel, so a covered fraction reads straight off the green
/// byte and the other two channels are a free check that nothing else was
/// sampled.
const UNIFORM: [u8; 4] = [0, 255, 0, 255];

/// A 16 x 16 atlas of one colour — the fixture D14's step number is asserted on.
///
/// **The whole atlas, not just the sampled region.** A partly covered pixel's
/// uv can leave the region under `center`, and `ClampToEdge` can take it to the
/// texture's border; making every texel equal means all three destinations
/// return the same value, so the qualifier has nothing left to change.
fn uniform_atlas() -> Pixels {
    let rgba = UNIFORM.repeat((ATLAS * ATLAS) as usize);
    Pixels::from_rgba8(ATLAS, ATLAS, rgba).expect("the generated buffer matches its dimensions")
}

/// The stored bytes a pixel of [`UNIFORM`] can take, one per coverage level.
///
/// `SAMPLE_COUNT` samples give `SAMPLE_COUNT + 1` levels counting both ends, the
/// resolve averages them in linear light, and the `Rgba8UnormSrgb` target
/// encodes on write. IEC 61966-2-1's transfer function is written out rather
/// than taken from a crate, the same way `camera_scene.rs` writes it out:
/// `1.055 * c.powf(1.0 / 2.4) - 0.055` above the linear toe at `0.0031308`.
///
/// At four samples this is `[0, 137, 188, 225, 255]`.
fn coverage_levels() -> Vec<u8> {
    (0..=narvo_render2d::SAMPLE_COUNT)
        .map(|covered| {
            let linear = f64::from(covered) / f64::from(narvo_render2d::SAMPLE_COUNT);
            let encoded = if linear <= 0.003_130_8 {
                linear * 12.92
            } else {
                linear.powf(1.0 / 2.4).mul_add(1.055, -0.055)
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "encoded is in 0.0..=1.0, so the product is in 0..=255"
            )]
            let byte = (encoded * 255.0).round() as u8;
            byte
        })
        .collect()
}

/// The one sprite: centred on the origin, 48 pixels square, sampling the atlas's
/// top-left 8 x 8 texels.
fn sprite(texture: &Pixels) -> [SpriteInstance; 1] {
    [SpriteInstance::new(
        SpritePlacement {
            x: 0.0,
            y: 0.0,
            rot_cos: 1.0,
            rot_sin: 0.0,
            scale_x: SPRITE_EDGE,
            scale_y: SPRITE_EDGE,
        },
        TextureRegion::from_texels(0, 0, REGION_TEXELS, REGION_TEXELS, texture),
    )]
}

fn target_or_skip(width: u32, height: u32) -> Option<OffscreenTarget> {
    match OffscreenTarget::new(width, height) {
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

/// The columns in which two images differ.
fn differing_columns(before: &Pixels, after: &Pixels) -> Vec<u32> {
    let mut columns = Vec::new();
    for x in 0..before.width() {
        let differs = (0..before.height()).any(|y| before.pixel(x, y) != after.pixel(x, y));
        if differs {
            columns.push(x);
        }
    }
    columns
}

/// The sweep itself: one frame per camera position, `STEPS + 1` of them, over
/// whichever atlas it is handed.
///
/// The camera positions are the only thing that moves, so two calls with two
/// atlases differ in exactly one variable.
fn sweep(target: &OffscreenTarget, texture: &Pixels) -> Vec<Pixels> {
    let sprites = sprite(texture);
    (0..=STEPS)
        .map(|k| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "k is at most 16 and every sixteenth is exact in f32"
            )]
            let camera = CameraView::new(k as f32 / STEPS as f32, 0.0, 1.0);
            target
                .render_sprites_viewed_by(texture, &sprites, camera)
                .expect("one sprite is far inside the batch limit")
        })
        .collect()
}

/// One step of the sweep that changed something, and what it changed.
#[derive(Debug)]
struct StepChange {
    /// Which of the [`STEPS`] the change happened at.
    k: u32,
    /// The silhouette columns that changed — [`LEFT_EDGE_COLUMNS`] and
    /// [`RIGHT_EDGE_COLUMNS`].
    silhouette: Vec<u32>,
    /// Every other column that changed.
    interior: Vec<u32>,
}

/// Per step of the sweep, the columns that changed, split into the silhouette's
/// and the interior's.
///
/// One classifier for both fixtures, so "which step counts as a silhouette
/// move" is decided in one place and cannot drift between the assurance and the
/// documentation.
fn changes_per_step(frames: &[Pixels]) -> Vec<StepChange> {
    let edge_columns: Vec<u32> = LEFT_EDGE_COLUMNS
        .into_iter()
        .chain(RIGHT_EDGE_COLUMNS)
        .collect();

    (1..=STEPS)
        .filter_map(|k| {
            let columns = differing_columns(&frames[(k - 1) as usize], &frames[k as usize]);
            if columns.is_empty() {
                return None;
            }
            let (silhouette, interior): (Vec<u32>, Vec<u32>) =
                columns.iter().partition(|x| edge_columns.contains(x));
            Some(StepChange {
                k,
                silhouette,
                interior,
            })
        })
        .collect()
}

/// **D14's step number, on content the uv qualifier cannot reach** (M3.25).
///
/// Every texel of the atlas is [`UNIFORM`], so a pixel's value is its coverage
/// and nothing else: whichever point the varying is interpolated at — the pixel
/// centre, the centroid of the covered samples, or a point outside the region
/// altogether — the fetch returns the same colour. What is left to measure is
/// the rasteriser's coverage, which is what D14 bought.
///
/// Two of the assertions are D14's number — `SAMPLE_COUNT` silhouette steps,
/// evenly spaced by `STEPS / SAMPLE_COUNT` of the sixteen, which is a quarter of
/// a pixel. The others are the fixture's premise, machine-checked rather than
/// assumed: no interior column may change at all, no pixel anywhere may take a
/// green byte outside the `SAMPLE_COUNT + 1` coverage levels of the one colour
/// or carry ink in the other two channels, and the left edge column must climb
/// through those levels once each without ever falling back.
#[test]
fn a_uniform_sprite_moves_its_silhouette_four_times_in_quarter_pixel_steps() {
    let Some(target) = target_or_skip(TARGET, TARGET) else {
        return;
    };

    let texture = uniform_atlas();
    let frames = sweep(&target, &texture);
    let changes = changes_per_step(&frames);

    for change in &changes {
        println!(
            "uniform, step {}: silhouette {:?}, interior {:?}",
            change.k, change.silhouette, change.interior
        );
    }

    let silhouette_steps: Vec<u32> = changes
        .iter()
        .filter(|c| !c.silhouette.is_empty())
        .map(|c| c.k)
        .collect();
    let interior_steps: Vec<u32> = changes
        .iter()
        .filter(|c| !c.interior.is_empty())
        .map(|c| c.k)
        .collect();

    let left_edge: Vec<u8> = frames
        .iter()
        .map(|f| {
            f.pixel(LEFT_EDGE_COLUMNS[0], TARGET / 2)
                .expect("inside the target")[1]
        })
        .collect();
    println!("uniform silhouette moved at steps {silhouette_steps:?} of {STEPS}");
    println!("uniform interior moved at steps {interior_steps:?} of {STEPS}");
    println!(
        "uniform column {} across the sweep: {left_edge:?}",
        LEFT_EDGE_COLUMNS[0]
    );

    assert_eq!(
        silhouette_steps.len(),
        narvo_render2d::SAMPLE_COUNT as usize,
        "the silhouette moved {} times across one pixel, not once per sample \
         position. Coverage antialiasing is what D14 bought, and this is the \
         number that says whether it arrived. Steps: {silhouette_steps:?}",
        silhouette_steps.len()
    );

    // And the steps are a quarter of a pixel apart, which is the other half of
    // D14's number. Sixteenths of a pixel divided by SAMPLE_COUNT steps: four
    // sixteenths, and the sweep is exactly one pixel wide.
    let quantum = STEPS / narvo_render2d::SAMPLE_COUNT;
    for pair in silhouette_steps.windows(2) {
        assert_eq!(
            pair[1] - pair[0],
            quantum,
            "the silhouette's steps are {silhouette_steps:?} of {STEPS} across one \
             pixel, which are not evenly spaced by 1/{} of a pixel",
            narvo_render2d::SAMPLE_COUNT
        );
    }

    assert!(
        interior_steps.is_empty(),
        "an interior column changed at steps {interior_steps:?}. This atlas has \
         one colour, so there is no texel boundary for a sample point to cross \
         and nothing but coverage can change a pixel. A change away from the \
         silhouette means the fixture is no longer uniform — and then this \
         file's assurance has stopped being qualifier-independent. Columns per \
         step: {changes:?}"
    );

    let levels = coverage_levels();
    for (k, frame) in frames.iter().enumerate() {
        for y in 0..frame.height() {
            for x in 0..frame.width() {
                let pixel = frame.pixel(x, y).expect("inside the target");
                assert!(
                    // One count of slack, and it is the read-back's: sRGB(0.5)
                    // is 187.5 of 255, which AMD stores as 188 and llvmpipe as
                    // 187. `BASELINE.md` records that same one-count difference
                    // between these two adapters.
                    levels.iter().any(|l| pixel[1].abs_diff(*l) <= 1),
                    "at camera step {k} pixel ({x}, {y}) is {pixel:?}, whose green \
                     byte is none of the {} coverage levels {levels:?} of a \
                     single-colour atlas. Only coverage can vary here, and \
                     coverage takes {} values.",
                    levels.len(),
                    levels.len()
                );
                assert!(
                    pixel[0] == 0 && pixel[2] == 0,
                    "at camera step {k} pixel ({x}, {y}) is {pixel:?}, which \
                     carries ink in a channel the atlas has none in. Something \
                     other than {UNIFORM:?} was sampled."
                );
            }
        }
    }

    let mut distinct_levels: Vec<u8> = left_edge.clone();
    distinct_levels.dedup();
    assert!(
        left_edge.windows(2).all(|pair| pair[1] >= pair[0]),
        "the left silhouette column's coverage fell during a pan that only ever \
         uncovers it from the left: {left_edge:?}"
    );
    assert_eq!(
        distinct_levels.len(),
        levels.len(),
        "the left silhouette column took {} distinct values across one pixel of \
         pan, not one per coverage level. Values: {left_edge:?}",
        distinct_levels.len()
    );
}

/// **The documentation measurement.** Sweeps one pixel in sixteenths over
/// striped content and reports where the silhouette moves and where the interior
/// does.
///
/// What it asserts is the interior — the half MSAA did not reach, and the half
/// that does not depend on the uv qualifier because an interior pixel is fully
/// covered. The silhouette's step list is **printed and not asserted** here
/// since M3.25: on striped content it is qualifier-dependent, for the reasons
/// the module documentation sets out, and D14's number moved to
/// [`a_uniform_sprite_moves_its_silhouette_four_times_in_quarter_pixel_steps`].
#[test]
fn a_pan_of_one_pixel_over_striped_content_moves_the_interior_in_two_halves() {
    let Some(target) = target_or_skip(TARGET, TARGET) else {
        return;
    };

    let texture = atlas();
    let frames = sweep(&target, &texture);
    let changes = changes_per_step(&frames);

    let mut silhouette_steps = Vec::new();
    let mut interior_steps = Vec::new();

    for change in &changes {
        let k = change.k;
        let sample = |x: u32| {
            let before = frames[(k - 1) as usize].pixel(x, TARGET / 2);
            let after = frames[k as usize].pixel(x, TARGET / 2);
            format!("{x}: {before:?} -> {after:?}")
        };
        println!(
            "step {k}: {} columns changed — silhouette {:?}, interior {:?}",
            change.silhouette.len() + change.interior.len(),
            change.silhouette,
            change.interior
        );
        for x in change.silhouette.iter().chain(&change.interior).take(3) {
            let rows: Vec<u32> = (0..TARGET)
                .filter(|&y| {
                    frames[(k - 1) as usize].pixel(*x, y) != frames[k as usize].pixel(*x, y)
                })
                .collect();
            println!("    {} | rows {rows:?}", sample(*x));
        }
        if !change.silhouette.is_empty() {
            silhouette_steps.push(k);
        }
        if !change.interior.is_empty() {
            interior_steps.push(k);
        }
    }

    for (x, y) in [(45_u32, 46_u32), (51, 52), (45, 64)] {
        let history: Vec<[u8; 4]> = frames
            .iter()
            .map(|f| f.pixel(x, y).expect("inside the target"))
            .collect();
        println!("({x}, {y}) across the sweep: {history:?}");
    }

    let mut distinct = 1;
    for k in 1..=STEPS {
        if frames[(k - 1) as usize].rgba() != frames[k as usize].rgba() {
            distinct += 1;
        }
    }

    println!("interior moved at steps {interior_steps:?} of {STEPS}");
    println!("distinct images across one pixel of pan: {distinct}");

    // **Printed, not asserted, since M3.25**, and the number to compare it
    // against is the one
    // `a_uniform_sprite_moves_its_silhouette_four_times_in_quarter_pixel_steps`
    // asserts. Under `centroid` this list is `[2, 6, 10, 14]`; under `center` it
    // is `[2, 6, 8, 10, 14]`, the extra entry being column 87 stepping out of
    // its region at the half-pixel position rather than a fifth coverage step.
    // Which of the two appears is a property of the qualifier the shader
    // carries, and a test cannot read that, so this file states the measurement
    // and leaves the assurance to content the qualifier cannot reach.
    println!("silhouette moved at steps {silhouette_steps:?} of {STEPS} (qualifier-dependent)");

    // An interior pixel clear of the diagonal: the unantialiased control, and
    // the half of this measurement MSAA does not touch. Column 45 sits just
    // left of a texel boundary; row 64 is well below the diagonal, which passes
    // through this column around row 46.
    let interior_history: Vec<[u8; 4]> = frames
        .iter()
        .map(|f| f.pixel(45, TARGET / 2).expect("inside the target"))
        .collect();
    let interior_changes = interior_history
        .windows(2)
        .filter(|pair| pair[0] != pair[1])
        .count();
    assert_eq!(
        interior_changes, 1,
        "an interior pixel away from the diagonal changed {interior_changes} \
         times across one pixel of pan. It must change exactly once: `Nearest` \
         samples it at one point, and that point crosses one texel boundary in \
         one pixel of travel. MSAA multiplies coverage, not texture sampling, so \
         any other number means the interior is governed by something this file \
         has not accounted for. History: {interior_history:?}"
    );
    assert!(
        // Every value is one of the atlas's own colours, which all have a
        // saturated channel: no blend anywhere off the diagonal.
        interior_history
            .iter()
            .all(|c| c[0] == 255 || c[1] == 255 || c[2] == 255),
        "an interior pixel away from the diagonal took a blended value. Only a \
         partly covered pixel blends, and this one is covered four times over. \
         History: {interior_history:?}"
    );

    assert!(
        distinct > 2,
        "one pixel of pan produced {distinct} distinct images. Before MSAA it \
         produced two, and D14 exists to raise that number."
    );

    // The diagonal seam, printed rather than asserted **since M3.25**. It was
    // asserted as `blended > 0` from M3.15 (`8190c50`, which is where `git log
    // -S` puts it — M3.24b's report calls it M3.16's, and that is the one thing
    // about it worth correcting) to M3.24: the pixel where the quad's
    // internal diagonal crosses a texel boundary has to blend, because
    // `centroid` gives the two triangles two sample points. That is a statement
    // about one qualifier — it is the artefact `centroid` costs, and the one
    // D17 was decided to remove — so it is exactly the kind of assurance this
    // file may no longer make. The count is qualifier-labelled documentation
    // now, and the module table above holds both rows.
    let seam: Vec<[u8; 4]> = frames
        .iter()
        .map(|f| f.pixel(45, 46).expect("inside the target"))
        .collect();
    let blended = seam
        .iter()
        .filter(|c| c[0] != 0 && c[1] != 0 && c[0] != 255 && c[1] != 255)
        .count();
    println!(
        "diagonal seam at (45, 46): {blended} of {} camera positions blended \
         (qualifier-dependent); history {seam:?}",
        seam.len()
    );
}

/// The sweep is a sweep: the geometry really does move by a sixteenth of a pixel
/// per step, and the sprite really does span the columns this file names.
///
/// Runs without a GPU. Without it, a sweep that never moved — a camera whose x
/// were ignored — would report "the image never changed" as a *finding* rather
/// than as the broken measurement it is.
#[test]
fn the_sweep_moves_the_geometry_by_a_sixteenth_of_a_pixel_per_step() {
    let projection = narvo_render2d::Projection::for_target(TARGET, TARGET);

    let edge_at = |camera_x: f32, world_x: f32| {
        let ndc = projection
            .viewed_by(CameraView::new(camera_x, 0.0, 1.0))
            .world_to_ndc(world_x, 0.0)[0];
        f64::from(ndc).mul_add(f64::from(TARGET) / 2.0, f64::from(TARGET) / 2.0)
    };

    let half = f64::from(SPRITE_EDGE) / 2.0;
    assert!(
        (edge_at(0.0, -SPRITE_EDGE / 2.0) - (f64::from(TARGET) / 2.0 - half)).abs() < 1e-9,
        "the sprite's left edge is not where this file's column numbers assume"
    );
    assert_eq!(f64::from(TARGET) / 2.0 - half, 40.0);
    assert_eq!(f64::from(TARGET) / 2.0 + half, 88.0);

    for k in 1..=STEPS {
        #[expect(
            clippy::cast_precision_loss,
            reason = "k is at most 16 and every sixteenth is exact in f32"
        )]
        let camera_x = k as f32 / STEPS as f32;
        let moved = edge_at(0.0, -SPRITE_EDGE / 2.0) - edge_at(camera_x, -SPRITE_EDGE / 2.0);
        assert!(
            (moved - f64::from(camera_x)).abs() < 1e-9,
            "step {k} moved the left edge by {moved} pixels, not {camera_x}"
        );
    }

    // Six pixels per texel: the interior's grid, and the reason its step size
    // is a different number from the silhouette's.
    let pixels_per_texel = f64::from(SPRITE_EDGE) / f64::from(REGION_TEXELS);
    assert_eq!(pixels_per_texel, 6.0);
}
