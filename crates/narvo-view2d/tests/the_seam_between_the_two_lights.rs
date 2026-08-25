//! M8.8 §2 — the seam, and it is an oracle rather than a risk.
//!
//! There are two lightings in this engine. The **game light** is
//! `narvo_ecs::illuminate`: coarse, on the CPU, in the tick, in the state hash,
//! and it decides whether an enemy is vulnerable. The **image light** is the
//! cascade M8.4–M8.7 build on the GPU: it decides nothing and is a picture.
//!
//! **If the two drift apart at entity positions, the picture lies to the
//! player** — he sees a figure standing in the dark that the rules hold to be
//! lit. That is the failure class M9 can be broken by, and this file is its only
//! guard.
//!
//! # This crate, because it is the only one that sees both sides
//!
//! ADR-0041's charter. `narvo-ecs` cannot reach a `Cascade` and
//! `narvo-render2d` cannot reach a `World`; this crate already depends on both,
//! and `seeds_of` — the image light's own reader of the same occluders — lives
//! here.
//!
//! # What is compared, and why it is not the level
//!
//! **Visibility, not radiance.** The two lights do not share a unit and do not
//! even share a functional form: the game light applies a linear falloff because
//! a designer chose one, and the cascade integrates radiance over directions and
//! comes out as 1/r (M8.5a). Comparing the two *values* compares two falloff
//! laws and says nothing whatever about the occluders — which was measured
//! before this file existed, at an rms of 0.42 against 0.12 for the comparison
//! below.
//!
//! So each side is divided by **its own** reading of the same scene with the
//! occluders taken away. That is M8.5b's technique — it reports residuals as a
//! share of the same probe's unoccluded reading — and it leaves exactly the
//! quantity both halves computed from the same rectangles. The normalisation is
//! symmetric and neither side borrows the other's: each is
//! `its own occluded / its own open`.
//!
//! # The form, and why it is the decision and not the value
//!
//! §2 asked for both forms to be tried. They were, and the value form does not
//! carry: over 625 receivers the two lights differ by an rms of 0.118651 and a
//! **worst of 0.581707**, so a value tolerance wide enough to hold would admit
//! very nearly everything. [`the_value_form_does_not_carry`] keeps that
//! measurement standing as a test rather than as a sentence in a report.
//!
//! The decision form does carry. The game does not act on the value, it acts on
//! a **threshold** — vulnerable or not — and the claim that holds is:
//!
//! > wherever the image light's visibility is further than [`BAND`] from the
//! > threshold, the two lights make the same decision.
//!
//! # [`BAND`] is derived, term by term, and every term was measured in this block
//!
//! §2's instruction was to compose it rather than to measure a new number. Each
//! term comes from the task that measured it, and the two units are joined by
//! the field's own spatial gradient, which **is** measured here rather than
//! assumed — 0.154965 per texel at probes near the threshold, over the same
//! arrangement.
//!
//! | term | from | size | as visibility |
//! |---|---|---|---|
//! | world → texel mapping | M8.3b | < 0.001 texel | 0.000155 |
//! | `q`, a position's distance from its texel centre | M8.4 | 0.707107 texels | **0.109577** |
//! | `eps`, jump flooding's overestimate | M8.3a | 0.242480 texels | 0.037576 |
//! | the interpolation residue at odd probes | M8.5b | 6–10 % | **0.100000** |
//! | | | | **0.247308** |
//!
//! The first three sum to **0.949587 texels**, which is exactly M8.4's own
//! derived margin `q + eps` — three of the terms compose back into a number that
//! task had already written down.
//!
//! **A fifth term is named and is deliberately not in [`BAND`]**: M8.7's
//! registration bound, half a probe forever, which is 2.0 texels at this
//! spacing and 0.309930 of visibility. It applies to an **accumulated** field
//! over camera motion and would more than double the bound to 0.557238. This
//! test holds the camera still and accumulates nothing, so including it would
//! be padding the tolerance with an error the arrangement cannot produce. It is
//! written down because a consumer that accumulates has to add it back, and
//! because it is the term that **dominates** once it applies: 55.6 % of the
//! moving bound, against `q`'s 44.3 % of the static one.
//!
//! # Reach — what this test cannot see
//!
//! - **A threshold near the ends of the range.** The bound was measured to hold
//!   from 0.30 to 0.70 and to fail above it: at 0.75 the observed band is
//!   0.331707. The reason is structural rather than numerical — the image light
//!   never quite reaches a visibility of 1.0, because interpolation and the
//!   direction count always leak a little shadow in, while the game light
//!   reaches it exactly. [`THRESHOLD`] therefore sits in the middle, and a
//!   consumer that wants a threshold at 0.9 is outside what this guard covers.
//! - **A defect symmetric in both lights.** Nothing here can see one; that is
//!   what J4 and [`a_constant_game_light_fails_the_same_test`] exist for.
//! - **One machine.** A test observes only the platform it runs on.

use narvo_ecs::{LightSource, Lit, Occluder, SystemContext, Transform, World, illuminate};
use narvo_render2d::{
    CameraView, Cascade, CascadeLayout, Emission, MergeForm, OffscreenTarget, Projection,
    RadianceField, RenderError, Seeds,
};
use narvo_view2d::seeds_of;

/// The field the image light is computed over, in texels.
const FIELD: u32 = 128;
/// Texels per world unit.
const ZOOM: f32 = 4.0;
/// Level zero's probe spacing, in texels — the unit M8.7's half-probe bound is
/// measured in, and the divisor that turns a texel position into a probe index.
const SPACING: u32 = 4;

/// The threshold the decision is made at.
///
/// The middle of the range on purpose; the module header says what happens
/// towards the ends and why it is structural.
const THRESHOLD: f32 = 0.5;

/// How far from [`THRESHOLD`] the two lights are allowed to disagree.
///
/// **Derived, not chosen** — the module header carries the composition term by
/// term. Every term was measured by an earlier task in this block, and the
/// gradient that joins the spatial terms to the value ones is measured by
/// [`the_gradient_the_bound_rests_on_is_what_was_measured`] rather than taken on
/// trust.
const BAND: f32 = 0.247308;

/// A target to borrow a device from, or `None` on a machine with no adapter.
fn target_or_skip() -> Option<OffscreenTarget> {
    match OffscreenTarget::new(8, 8) {
        Ok(target) => Some(target),
        Err(RenderError::NoAdapter { .. }) => None,
        Err(other) => {
            panic!("the offscreen target failed for a reason that is not absence: {other}")
        }
    }
}

fn projection() -> Projection {
    Projection::for_target(FIELD, FIELD).viewed_by(CameraView::new(0.0, 0.0, ZOOM))
}

/// Where the lamp stands and how big it is, in world units.
const LAMP: (f32, f32, f32) = (-8.0, 0.0, 0.75);

/// The receivers, on a one-unit grid over the middle of the field.
fn receivers() -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut y = -12.0_f32;
    while y <= 12.0 {
        let mut x = -12.0_f32;
        while x <= 12.0 {
            out.push((x, y));
            x += 1.0;
        }
        y += 1.0;
    }
    out
}

/// The occluders of one arrangement, as `(x, y, half_width, half_height)`.
type Rects = &'static [(f32, f32, f32, f32)];

/// A wall and a pillar — the arrangement §1's measurement was taken on.
const WALL_AND_PILLAR: Rects = &[(-2.0, 0.0, 0.5, 5.0), (4.0, 3.0, 1.5, 1.5)];

/// The same wall with a gap in it, which is M8.5b's own hard case. A second
/// arrangement is what says [`BAND`] is a property of the seam rather than of
/// one scene.
const SLIT: Rects = &[(-2.0, 3.0, 0.5, 2.0), (-2.0, -3.0, 0.5, 2.0)];

/// A world holding the lamp, the receivers and — if `with_occluders` — the
/// rectangles.
fn world_of(rects: Rects, with_occluders: bool) -> (World, Vec<narvo_ecs::EntityId>) {
    let mut world = World::new();

    let lamp = world.spawn();
    world
        .insert(lamp, Transform::at(LAMP.0, LAMP.1))
        .expect("insert");
    world
        .insert(lamp, LightSource::new(64.0, 1.0, LAMP.2))
        .expect("insert");

    if with_occluders {
        for &(x, y, half_width, half_height) in rects {
            let blocker = world.spawn();
            world.insert(blocker, Transform::at(x, y)).expect("insert");
            world
                .insert(blocker, Occluder::new(half_width, half_height))
                .expect("insert");
        }
    }

    let mut ids = Vec::new();
    for (x, y) in receivers() {
        let entity = world.spawn();
        world.insert(entity, Transform::at(x, y)).expect("insert");
        world.insert(entity, Lit::DARK).expect("insert");
        ids.push(entity);
    }

    (world, ids)
}

/// **The game light's visibility**: its own occluded reading over its own open
/// one.
///
/// Nothing from the renderer is touched here — this half is `narvo-ecs` and
/// arithmetic. That is the independence the whole test rests on, and J1 is the
/// injection that removes it.
fn game_visibility(rects: Rects) -> Vec<f32> {
    let mut occluded = {
        let (mut world, ids) = world_of(rects, true);
        illuminate(&mut world, &SystemContext::new(0));
        ids.into_iter()
            .map(|e| world.get::<Lit>(e).expect("a lit entity").level)
            .collect::<Vec<f32>>()
    };
    let open = {
        let (mut world, ids) = world_of(rects, false);
        illuminate(&mut world, &SystemContext::new(0));
        ids.into_iter()
            .map(|e| world.get::<Lit>(e).expect("a lit entity").level)
            .collect::<Vec<f32>>()
    };

    for (value, divisor) in occluded.iter_mut().zip(&open) {
        *value = if *divisor > 0.0 {
            (*value / *divisor).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    occluded
}

/// The two inputs a cascade takes for one arrangement.
fn cascade_inputs(rects: Rects, with_occluders: bool) -> (Seeds, Emission) {
    let projection = projection();
    let (world, _) = world_of(rects, with_occluders);

    let mut seeds = seeds_of(&world, &projection, FIELD, FIELD).expect("a legal size");
    let mut emission = Emission::new(FIELD, FIELD).expect("a legal size");

    let [cx, cy] = projection.world_to_screen(LAMP.0, LAMP.1);
    let radius = (LAMP.2 * ZOOM).max(1.0);
    for y in 0..FIELD {
        for x in 0..FIELD {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= radius * radius {
                seeds.set(x, y).expect("inside");
                emission.set(x, y, [1.0, 1.0, 1.0]).expect("inside");
            }
        }
    }
    (seeds, emission)
}

fn run_cascade(target: &OffscreenTarget, rects: Rects, with_occluders: bool) -> RadianceField {
    let (seeds, emission) = cascade_inputs(rects, with_occluders);
    let cascade = Cascade::new(
        CascadeLayout {
            origin: [0.0, 0.0],
            base_spacing: SPACING as f32,
            base_interval: 2.0,
            base_directions: 32,
            levels: 5,
            sky: [0.0, 0.0, 0.0],
        },
        FIELD,
        FIELD,
    )
    .expect("a sound cascade");
    target
        .cascade(&seeds, &emission, &cascade, MergeForm::Directional)
        .expect("the cascade runs")
}

/// **The image light's visibility**, at the receivers' own positions.
fn image_visibility(target: &OffscreenTarget, rects: Rects) -> Vec<f32> {
    let occluded = run_cascade(target, rects, true);
    let open = run_cascade(target, rects, false);
    let projection = projection();

    receivers()
        .into_iter()
        .map(|(x, y)| {
            let [sx, sy] = projection.world_to_screen(x, y);
            let (px, py) = ((sx / SPACING as f32) as u32, (sy / SPACING as f32) as u32);
            let a = occluded.radiance(px, py).map_or(0.0, |c| c[0]);
            let b = open.radiance(px, py).map_or(0.0, |c| c[0]);
            if b > 0.0 {
                (a / b).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .collect()
}

/// The widest distance from [`THRESHOLD`] at which the two columns disagree,
/// measured on the **image** side — the side the player sees.
fn observed_band(game: &[f32], image: &[f32]) -> (usize, f32) {
    let (mut disagreements, mut band) = (0, 0.0_f32);
    for (mine, theirs) in game.iter().zip(image) {
        if (*mine >= THRESHOLD) != (*theirs >= THRESHOLD) {
            disagreements += 1;
            band = band.max((theirs - THRESHOLD).abs());
        }
    }
    (disagreements, band)
}

// -- the oracle ------------------------------------------------------------

/// **The two lights agree on the decision, outside a band that was derived.**
///
/// Checked on two arrangements, because a band that held on one scene would be
/// a fact about that scene. The disagreement *count* is allowed to differ
/// between them — it is a property of how much penumbra the arrangement has —
/// and the **band** is what has to hold, because that is the quantity [`BAND`]
/// bounds.
#[test]
fn the_two_lights_agree_on_the_decision_outside_the_derived_band() {
    let Some(target) = target_or_skip() else {
        return;
    };

    for (name, rects) in [("wall and pillar", WALL_AND_PILLAR), ("slit", SLIT)] {
        let game = game_visibility(rects);
        let image = image_visibility(&target, rects);
        let (disagreements, band) = observed_band(&game, &image);

        // It cannot pass by both lights being blank, and it cannot pass by
        // neither light casting a shadow: the arrangement has to produce both
        // answers on both sides before the comparison means anything.
        for (label, column) in [("the game light", &game), ("the image light", &image)] {
            assert!(
                column.iter().any(|v| *v > 0.9),
                "{name}: {label} left nothing visible, so the comparison is between two blanks"
            );
            assert!(
                column.iter().any(|v| *v < 0.1),
                "{name}: {label} cast no shadow at all, so the comparison is between two open rooms"
            );
        }

        assert!(
            band <= BAND,
            "{name}: the two lights disagreed at {disagreements} of {} receivers, and the \
             furthest disagreement sat {band} from the threshold — outside the derived band of \
             {BAND}. Either a light changed or the derivation's terms did",
            game.len()
        );
    }
}

/// **The game light has to be doing work: a constant one fails this test.**
///
/// A game light replaced by a constant is put through the identical comparison
/// and has to **fail** it. Without this, "the two agree outside the band" would
/// be satisfied by any pair of columns that happened to be mostly extreme.
///
/// Two constants, not one, and each fails from a different direction — a light
/// that says everything is lit and one that says everything is dark. A guard
/// that only tried "everything lit" would be satisfied by a game light that had
/// silently become "everything dark".
///
/// # What this is **not**, measured
///
/// **It is not the anti-tautology guard, although M8.8 designed it to be one.**
/// J4 replaced `image_visibility` with the game light's own column, so both
/// sides of the seam came from one computation — and this test **passed**. It
/// had to: a constant column still disagrees wildly with a game column that has
/// real shadows in it, whichever computation produced that column. What this
/// checks is that the *game* side is not degenerate, and it is blind to where
/// the *image* side came from.
///
/// The test that caught J4 is [`the_value_form_does_not_carry`], because that one
/// asserts the two sides **differ**. That is the shape an anti-tautology guard
/// has to have, and M8.1's `the_screenshot_is_the_frame_that_was_drawn` is the
/// standing warning it answers: a comparison whose two sides move together is a
/// tautology with a name, and only a test that requires them to disagree can
/// see it.
#[test]
fn a_constant_game_light_fails_the_same_test() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let image = image_visibility(&target, WALL_AND_PILLAR);

    for constant in [1.0_f32, 0.0] {
        let flat = vec![constant; image.len()];
        let (disagreements, band) = observed_band(&flat, &image);

        assert!(
            band > BAND,
            "a game light stuck at {constant} passed the seam test — it disagreed at only \
             {disagreements} receivers with a band of {band}, inside the derived {BAND}. The \
             comparison is not measuring what it claims to"
        );
    }
}

/// **The value form does not carry, and this is the measurement that says so.**
///
/// §2 asked for both forms to be tried and one chosen. The value form was tried
/// and rejected: the two lights differ by far more than [`BAND`] at their worst,
/// because the image light's penumbra extends past the geometric shadow by much
/// more than the derivation's terms cover.
///
/// **This test asserts the rejection rather than restating it in prose**, in the
/// shape M8.7 used for its two resample arms: it says the difference *exists*.
/// If it ever goes red, the finding is that the two lights have converged and
/// the decision to use the decision form should be taken again — which is the
/// revision condition, written where it can fire.
///
/// # And it is this file's anti-tautology guard, which was measured rather than
/// intended
///
/// M8.8 built [`a_constant_game_light_fails_the_same_test`] to be the guard
/// against the two sides of the seam coming from one computation, and injection
/// J4 showed that it is not: with `image_visibility` replaced by the game
/// light's own column, that test **passed** and this one **failed**, reporting
/// that the two lights "agree in value to within 0".
///
/// The reason is structural and worth keeping. A guard against a tautology has
/// to assert that the two sides **differ**; perturbing one side and watching the
/// comparison break only shows that the comparison reads that side. So this
/// test carries two jobs — it records why the value form was rejected, and it is
/// the only thing standing between the seam and a comparison of one column with
/// itself. **Do not delete it because the prose above is also in a report.**
#[test]
fn the_value_form_does_not_carry() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let game = game_visibility(WALL_AND_PILLAR);
    let image = image_visibility(&target, WALL_AND_PILLAR);

    let worst = game
        .iter()
        .zip(&image)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);

    assert!(
        worst > BAND,
        "the two lights now agree in value to within {worst}, inside the derived band of \
         {BAND}. That is better than M8.8 measured and it means the value form may now carry — \
         re-take the decision rather than deleting this test"
    );
}

/// **The gradient the bound rests on is what was measured.**
///
/// [`BAND`] converts three spatial terms into visibility by multiplying them
/// with the field's own spatial gradient near the threshold. That number is a
/// measurement and not a constant of nature, so it is re-measured here: if the
/// cascade's layout or resolution changed, the terms would still be right and
/// the composition would be wrong, and nothing else in this file would notice.
#[test]
fn the_gradient_the_bound_rests_on_is_what_was_measured() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let occluded = run_cascade(&target, WALL_AND_PILLAR, true);
    let open = run_cascade(&target, WALL_AND_PILLAR, false);
    let probes = FIELD / SPACING;

    let visibility = |px: u32, py: u32| -> Option<f32> {
        let a = occluded.radiance(px, py)?[0];
        let b = open.radiance(px, py)?[0];
        Some(if b > 0.0 {
            (a / b).clamp(0.0, 1.0)
        } else {
            0.0
        })
    };

    let mut worst = 0.0_f32;
    let mut counted = 0_usize;
    for py in 0..probes {
        for px in 0..probes {
            let Some(here) = visibility(px, py) else {
                continue;
            };
            // Only near the threshold: that is the only place a decision can
            // flip, and it is where the bound is spent.
            if !(THRESHOLD - 0.2..=THRESHOLD + 0.2).contains(&here) {
                continue;
            }
            for (dx, dy) in [(1_u32, 0_u32), (0, 1)] {
                let (nx, ny) = (px + dx, py + dy);
                if nx >= probes || ny >= probes {
                    continue;
                }
                let Some(there) = visibility(nx, ny) else {
                    continue;
                };
                worst = worst.max((there - here).abs() / SPACING as f32);
                counted += 1;
            }
        }
    }

    assert!(
        counted > 0,
        "no probe sat near the threshold, so the gradient was measured over nothing"
    );

    // The composition used 0.154965 per texel. A little headroom, because this
    // is a worst case over a finite grid and not a constant — but not much, or
    // the check would stop saying anything.
    const MEASURED: f32 = 0.154965;
    assert!(
        worst <= MEASURED * 1.25,
        "the field's gradient near the threshold is now {worst} per texel, against the \
         {MEASURED} the band was composed from over {counted} probe pairs. The terms of the \
         derivation are unchanged but the composition is stale — recompose it"
    );
}

/// Two runs of the whole seam agree, so a disagreement in the tests above is a
/// disagreement between the lights and not between two runs of one.
#[test]
fn the_seam_is_the_same_twice() {
    let Some(target) = target_or_skip() else {
        return;
    };

    let first = image_visibility(&target, WALL_AND_PILLAR);
    let second = image_visibility(&target, WALL_AND_PILLAR);
    assert_eq!(first, second, "two runs of the image light disagreed");

    assert_eq!(
        game_visibility(WALL_AND_PILLAR),
        game_visibility(WALL_AND_PILLAR),
        "two runs of the game light disagreed"
    );

    assert!(
        first.iter().any(|v| *v > 0.9) && first.iter().any(|v| *v < 0.1),
        "the scene is blank, so the equality above holds for nothing"
    );
}
