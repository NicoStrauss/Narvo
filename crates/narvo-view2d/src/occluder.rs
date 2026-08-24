//! Turning the occluders in a world into the seed texels a distance field
//! floods from.
//!
//! The second half of M8.3b, and it sits here for the reason ADR-0041 gives this
//! crate its charter: it needs a [`World`] and it needs `narvo-render2d`'s
//! [`Seeds`], and this is the crate that already sees both sides. `narvo-ecs`
//! cannot reach `Seeds`, `narvo-render2d` cannot reach a `World`, and a crate of
//! its own was measured rather than argued — M8.3b's report carries the numbers
//! and ADR-0041's charter carries the position.
//!
//! # The same path a sprite takes
//!
//! ADR-0015: the renderer takes scalars and never a world. An occluder reaches
//! the renderer as a `Seeds` set, which is plain data — no `wgpu` type is
//! involved on either side of this function, exactly as none is involved on
//! either side of [`placements_of`](crate::placements_of).
//!
//! It uses the **same [`Projection`]** the sprites are drawn through, and that is
//! the whole reason the parameter exists rather than being built here: a light
//! computed through a different camera than the picture would be a light that
//! drifts when the camera moves, and the drift would be invisible in any test
//! that held the camera still.
//!
//! # Why the seeded set cannot depend on iteration order
//!
//! It is a **union**, and union is commutative. Two occluders claiming one texel
//! both mark it, [`Seeds::set`] is idempotent, and no information about *which*
//! one claimed it is kept — because jump flooding does not want any: a seed
//! texel's seed coordinate is its own coordinate. So the overlap case M8.3b's
//! brief names as a plausible defect is unrepresentable here, and that was
//! confirmed by injection rather than assumed.
//!
//! The entity walk is still [`World::entity_ids`] rather than a query, matching
//! [`regions_of`](crate::regions_of). That is belt and braces: `narvo-ecs`
//! documents query order as explicitly unstable, and while the union makes it
//! not matter today, an extraction that ever grew an ordered output would need
//! the canonical order and would not think to add it.

use narvo_ecs::{Occluder, Transform, World};
use narvo_render2d::{Projection, RenderError, Seeds};

/// Every occluder in `world`, as the seed texels of a `width` x `height` field.
///
/// A texel is a seed when **its centre** lies inside some occluder's rectangle,
/// edges included. Centres rather than corners, because a texel is a sample of
/// an area and its centre is where that sample sits; edges included, because
/// [`Occluder::contains`] includes them and the two halves of the engine
/// disagreeing at a boundary is the defect that would show up one texel wide and
/// only sometimes.
///
/// An entity needs both an [`Occluder`] and a [`Transform`]: the component is
/// half-extents and the position comes from the transform, so one without the
/// other describes no rectangle and is skipped rather than defaulted. An
/// occluder with a negative or `NaN` half-extent seeds nothing, matching
/// [`Occluder::contains`].
///
/// # Errors
///
/// [`RenderError::InvalidSize`] if `width` or `height` is zero or above
/// `OffscreenTarget::MAX_DIMENSION`, which is [`Seeds::new`]'s own condition.
///
/// # Panics
///
/// Never. The `expect` inside is on a coordinate this function has already
/// bounds-checked against the field it is building.
pub fn seeds_of(
    world: &World,
    projection: &Projection,
    width: u32,
    height: u32,
) -> Result<Seeds, RenderError> {
    let mut seeds = Seeds::new(width, height)?;

    for entity in world.entity_ids() {
        let Ok(occluder) = world.get::<Occluder>(entity) else {
            continue;
        };
        let Ok(transform) = world.get::<Transform>(entity) else {
            continue;
        };

        // The rectangle's world-space bounds. `half_extent` may be negative or
        // `NaN`; both make the span below empty rather than being rejected here,
        // which is `Occluder::contains`' rule read from the other side.
        let left = transform.x - occluder.half_width;
        let right = transform.x + occluder.half_width;
        let bottom = transform.y - occluder.half_height;
        let top = transform.y + occluder.half_height;

        // To screen. The projection has no rotation — `CameraView` is a position
        // and a zoom — so an axis-parallel world rectangle is an axis-parallel
        // screen rectangle and two mapped corners describe it completely.
        //
        // **`top` maps to the smaller row.** ADR-0004 fixes y as up in world
        // space and down in framebuffer rows, and `world_to_screen` performs that
        // single reconciliation; pairing world `top` with screen `y0` is that
        // fact written down rather than a swap to be discovered later.
        let [x0, y0] = projection.world_to_screen(left, top);
        let [x1, y1] = projection.world_to_screen(right, bottom);

        let Some((first_x, last_x)) = texel_span(x0, x1, width) else {
            continue;
        };
        let Some((first_y, last_y)) = texel_span(y0, y1, height) else {
            continue;
        };

        for y in first_y..=last_y {
            for x in first_x..=last_x {
                seeds
                    .set(x, y)
                    .expect("a span is clamped to the field it was measured against");
            }
        }
    }

    Ok(seeds)
}

/// The texels whose **centres** lie in `low..=high`, clamped to `0..extent`.
///
/// `None` when the span covers no texel centre at all, which is the same answer
/// for four different situations, deliberately: a rectangle outside the field, a
/// rectangle between two centres, a negative half-extent (`low > high`), and a
/// `NaN` one. All four seed nothing, so all four are one branch rather than four
/// with three of them untested.
///
/// # The arithmetic, and where the off-by-one would live
///
/// Texel `t` has its centre at `t + 0.5`. It is covered when
/// `low <= t + 0.5 <= high`, so `t >= low - 0.5` and `t <= high - 0.5`, which is
/// `ceil(low - 0.5)` to `floor(high - 0.5)`. Every one of those four terms is a
/// place an off-by-one hides — a `floor` for a `ceil`, a `+ 0.5` for a `- 0.5`,
/// a `<` for a `<=` — and M8.3b injected one of them to watch the guards fall
/// rather than trusting the derivation.
fn texel_span(low: f32, high: f32, extent: u32) -> Option<(u32, u32)> {
    if !low.is_finite() || !high.is_finite() || low > high {
        return None;
    }

    let first = (low - 0.5).ceil();
    let last = (high - 0.5).floor();
    if first > last {
        return None;
    }

    // `extent` is at least one: `Seeds::new` refused zero before this ran.
    let highest = f32::from(u16::try_from(extent - 1).ok()?);
    if last < 0.0 || first > highest {
        return None;
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "both bounds are clamped into 0..=extent-1 on the line above each cast"
    )]
    let span = (first.max(0.0) as u32, last.min(highest) as u32);
    Some(span)
}

#[cfg(test)]
mod tests {
    use super::{seeds_of, texel_span};
    use narvo_ecs::{Occluder, Transform, World};
    use narvo_render2d::{CameraView, Projection, RenderError, Seeds};

    /// A 16 x 16 field seen from the identity camera.
    ///
    /// Sixteen so that one world unit is exactly one texel at this projection:
    /// `Projection::for_target(16, 16)` has half-extents of 8, and NDC spans
    /// -1..1, so a world x of 1.0 is 1 texel right of centre. That makes every
    /// expected texel in these tests something a reader can check by hand.
    fn projection() -> Projection {
        Projection::for_target(16, 16)
    }

    fn world_with(occluders: &[(f32, f32, f32, f32)]) -> World {
        let mut world = World::new();
        for &(x, y, half_width, half_height) in occluders {
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(x, y))
                .expect("the world takes a component");
            world
                .insert(entity, Occluder::new(half_width, half_height))
                .expect("the world takes a component");
        }
        world
    }

    /// Which texels a `Seeds` marks, in row-major order.
    fn marked(seeds: &Seeds) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        for y in 0..seeds.height() {
            for x in 0..seeds.width() {
                if seeds.is_seed(x, y) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    /// A world with no occluders seeds nothing, and the field is still its own
    /// size.
    #[test]
    fn a_world_with_no_occluders_seeds_nothing() {
        let world = World::new();
        let seeds = seeds_of(&world, &projection(), 16, 16).expect("a legal size");

        assert_eq!((seeds.width(), seeds.height()), (16, 16));
        assert_eq!(seeds.count(), 0);
        assert!(marked(&seeds).is_empty());
    }

    /// An entity with only one half of the pair describes no rectangle.
    #[test]
    fn an_occluder_without_a_transform_seeds_nothing() {
        let mut world = World::new();

        let lonely = world.spawn();
        world
            .insert(lonely, Occluder::new(4.0, 4.0))
            .expect("the world takes a component");

        let placed = world.spawn();
        world
            .insert(placed, Transform::at(0.0, 0.0))
            .expect("the world takes a component");

        let seeds = seeds_of(&world, &projection(), 16, 16).expect("a legal size");
        assert_eq!(seeds.count(), 0, "half a rectangle seeded something anyway");
    }

    /// **§4(d): a rectangle at a known place seeds known texels, exactly.**
    ///
    /// # Why the bound is zero and not a tolerance
    ///
    /// The expected set is computed here from the closed form — a texel is
    /// seeded when its centre is inside the rectangle — and asserted for
    /// *equality*, with no tolerance at all. That is not optimism, it is a margin
    /// argument, and the margin is measured rather than assumed.
    ///
    /// The rectangle below is centred at the world origin with half-extents of
    /// **2.0**. At this projection one world unit is one texel and the field's
    /// centre is at screen 8.0, so the screen edges land at 6.0 and 10.0 —
    /// integers, which is **half a texel** from the nearest centre in either
    /// direction and the furthest an edge can be from one.
    /// `the_mapping_is_far_more_accurate_than_the_margin` measures the
    /// projection's own error at these magnitudes against an `f64` computation of
    /// the same affine map and reports it in texels; the assertion there is what
    /// licenses the equality here.
    ///
    /// **Half-extents of 2.5 were written first and were wrong**, and the reason
    /// is worth keeping: they put the screen edges at 5.5 and 10.5, exactly on
    /// texel centres, which is the coin-toss case the paragraph above warns
    /// against. The implementation was right and the expectation was not. An edge
    /// on a centre is decided by the last bit of a float and by whether the
    /// comparison is inclusive; asserting an answer there asserts the rounding
    /// rather than the mapping. It is a named limit in M8.3b's report.
    #[test]
    fn a_rectangle_at_a_known_place_seeds_the_texels_the_closed_form_names() {
        let world = world_with(&[(0.0, 0.0, 2.0, 2.0)]);
        let seeds = seeds_of(&world, &projection(), 16, 16).expect("a legal size");

        // Screen span 6.0 ..= 10.0; centres inside are 6.5, 7.5, 8.5, 9.5 —
        // texels 6, 7, 8, 9, in both axes.
        let expected: Vec<(u32, u32)> =
            (6..=9).flat_map(|y| (6..=9).map(move |x| (x, y))).collect();

        assert_eq!(marked(&seeds), expected);
        assert_eq!(seeds.count(), 16);
    }

    /// The measurement the test above rests on.
    ///
    /// `world_to_screen` is `f32` throughout. The same affine map computed in
    /// `f64` is the reference, and the largest disagreement over the corners this
    /// module's tests use is reported in **texels**. The margin those tests need
    /// is half a texel; this asserts the error is below a thousandth of one, so
    /// the equality assertions above have three orders of magnitude of room.
    ///
    /// It is a measurement rather than an appeal to IEEE 754: what matters is
    /// this projection at these magnitudes, not floats in general.
    #[test]
    fn the_mapping_is_far_more_accurate_than_the_margin() {
        let projection = projection();
        let mut worst = 0.0_f64;

        for corner in [-2.5_f32, -1.5, -0.5, 0.5, 1.5, 2.5, 7.5, -7.5] {
            let [screen_x, screen_y] = projection.world_to_screen(corner, corner);
            // The same map in f64: NDC is world / half_extent, screen is
            // (ndc + 1) * half_extent for x and (1 - ndc) * half_extent for y.
            let exact_x = (f64::from(corner) / 8.0 + 1.0) * 8.0;
            let exact_y = (1.0 - f64::from(corner) / 8.0) * 8.0;
            worst = worst
                .max((f64::from(screen_x) - exact_x).abs())
                .max((f64::from(screen_y) - exact_y).abs());
        }

        assert!(
            worst < 0.001,
            "the projection is {worst} texels from an f64 computation of the same \
             map, which is too close to the half-texel margin the equality \
             assertions in this module rely on"
        );
    }

    /// **§4(c): the same world yields the same seeds, twice.**
    #[test]
    fn the_extraction_is_the_same_twice() {
        let world = world_with(&[(0.0, 0.0, 2.0, 2.0), (-4.0, 3.0, 1.0, 1.0)]);
        let projection = projection();

        let first = seeds_of(&world, &projection, 16, 16).expect("a legal size");
        let second = seeds_of(&world, &projection, 16, 16).expect("a legal size");

        assert_eq!(first, second, "one world extracted two different seed sets");
    }

    /// **§4(c), the half a repeated call cannot show: two worlds built in
    /// opposite orders extract identically.**
    ///
    /// The overlap case. Two occluders share texels; if anything about the
    /// extraction depended on which entity was walked first, these two worlds
    /// would differ. They cannot, because the seeded set is a union and
    /// `Seeds::set` is idempotent — this test is what says that out loud, and
    /// M8.3b's report records that the injection meant to break it could not.
    #[test]
    fn two_overlapping_occluders_extract_the_same_in_either_spawn_order() {
        let projection = projection();
        let forwards = world_with(&[(0.0, 0.0, 2.0, 2.0), (1.0, 1.0, 2.0, 2.0)]);
        let backwards = world_with(&[(1.0, 1.0, 2.0, 2.0), (0.0, 0.0, 2.0, 2.0)]);

        let a = seeds_of(&forwards, &projection, 16, 16).expect("a legal size");
        let b = seeds_of(&backwards, &projection, 16, 16).expect("a legal size");

        assert_eq!(a, b, "the spawn order changed the seeded set");
        assert!(
            a.count() > 16,
            "the two rectangles did not overlap into more than one covers alone, \
             so this test would pass without exercising the union"
        );
    }

    /// A rectangle entirely outside the field seeds nothing, and one straddling
    /// the edge seeds only the part inside.
    ///
    /// **The border case, and the one M8.3b injected.** M8.3a measured that a
    /// missing border test in the *kernel* was not a defect, because a clamped
    /// probe cannot invent a seed. That argument does not transfer here: a
    /// mapping can point at the wrong texel, and an unclamped span would index
    /// past the field or wrap.
    #[test]
    fn a_rectangle_outside_the_field_seeds_nothing_and_one_across_the_edge_is_clipped() {
        let projection = projection();

        let far = world_with(&[(100.0, 100.0, 1.0, 1.0)]);
        assert_eq!(
            seeds_of(&far, &projection, 16, 16)
                .expect("a legal size")
                .count(),
            0,
            "a rectangle far outside the field seeded something"
        );

        // Centred on the left edge: world x of -8.0 is screen 0.0. Half-extent
        // 2.0 spans screen -2.0 ..= 2.0, so centres 0.5 and 1.5 are inside —
        // texels 0 and 1, and nothing to the left of 0.
        let straddling = world_with(&[(-8.0, 0.0, 2.0, 2.0)]);
        let seeds = seeds_of(&straddling, &projection, 16, 16).expect("a legal size");
        let columns: Vec<u32> = (0..16).filter(|&x| seeds.is_seed(x, 8)).collect();
        assert_eq!(
            columns,
            vec![0, 1],
            "a rectangle across the left edge did not clip to the field"
        );
    }

    /// A negative or `NaN` half-extent seeds nothing, matching
    /// `Occluder::contains`.
    #[test]
    fn a_degenerate_rectangle_seeds_nothing() {
        let projection = projection();
        for (half_width, half_height) in [(-2.0, 2.0), (2.0, -2.0), (f32::NAN, 2.0)] {
            let world = world_with(&[(0.0, 0.0, half_width, half_height)]);
            assert_eq!(
                seeds_of(&world, &projection, 16, 16)
                    .expect("a legal size")
                    .count(),
                0,
                "an occluder of ({half_width}, {half_height}) seeded something"
            );
        }
    }

    /// The span helper answers the four ways of covering nothing with one `None`.
    #[test]
    fn a_span_that_covers_no_texel_centre_is_none() {
        assert_eq!(texel_span(0.6, 0.7, 16), None, "between two centres");
        assert_eq!(texel_span(20.0, 30.0, 16), None, "past the far edge");
        assert_eq!(texel_span(-30.0, -20.0, 16), None, "before the near edge");
        assert_eq!(
            texel_span(2.0, 1.0, 16),
            None,
            "inverted, a negative extent"
        );
        assert_eq!(texel_span(f32::NAN, 1.0, 16), None, "not a number");

        // And the ones that do cover something, including both clamps. Every
        // bound here is off a texel centre by 0.5, so none of them is deciding
        // an inclusive comparison by the last bit of a float.
        assert_eq!(texel_span(6.0, 10.0, 16), Some((6, 9)));
        assert_eq!(texel_span(-4.0, 3.0, 16), Some((0, 2)), "clamped at zero");
        assert_eq!(
            texel_span(14.0, 40.0, 16),
            Some((14, 15)),
            "clamped at the end"
        );

        // The inclusive edge itself, asserted once and on purpose rather than
        // stumbled into: a span whose bounds *are* texel centres includes both.
        assert_eq!(
            texel_span(5.5, 10.5, 16),
            Some((5, 10)),
            "an edge exactly on a texel centre is inside, matching Occluder::contains"
        );
    }

    /// A field size a `Seeds` cannot have is refused before any world is walked.
    #[test]
    fn an_impossible_field_size_is_refused() {
        let world = world_with(&[(0.0, 0.0, 2.0, 2.0)]);
        assert!(matches!(
            seeds_of(&world, &projection(), 0, 16),
            Err(RenderError::InvalidSize { .. })
        ));
    }

    /// The camera moves the light with the picture.
    ///
    /// Not decoration: `seeds_of` takes a projection so that the light is
    /// computed through the same camera the sprites are drawn through. A pan of
    /// one world unit has to move the seeded texels by one, or the two would
    /// drift apart the moment anything scrolled — and no test that held the
    /// camera still would notice.
    #[test]
    fn panning_the_camera_moves_the_seeded_texels_with_it() {
        let world = world_with(&[(0.0, 0.0, 2.0, 2.0)]);

        let still = seeds_of(&world, &projection(), 16, 16).expect("a legal size");
        let panned = seeds_of(
            &world,
            &projection().viewed_by(CameraView::new(1.0, 0.0, 1.0)),
            16,
            16,
        )
        .expect("a legal size");

        let shifted: Vec<(u32, u32)> = marked(&still)
            .into_iter()
            .map(|(x, y)| (x - 1, y))
            .collect();
        assert_eq!(
            marked(&panned),
            shifted,
            "panning the camera one world unit right did not move the seeds one \
             texel left"
        );
    }
}
