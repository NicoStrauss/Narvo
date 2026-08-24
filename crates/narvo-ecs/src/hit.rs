//! What a click on an entity means.
//!
//! [`HitRect`] is simulation state for the same reason [`Layer`](crate::Layer)
//! and [`Sprite`](crate::Sprite) are: a click that changes the world has to be
//! reproducible from a recording, and an area that lived outside the world would
//! not be. It goes into the canonical dump and therefore into the state hash of
//! any simulation that registers it.
//!
//! **What is not here is the ordering.** Deciding *which* of several overlapping
//! rectangles a click belongs to is the mirror of draw order, and `Layer`'s own
//! documentation puts that rule in `narvo-app` beside the extraction, where
//! ADR-0015 placed the draw-order decision. This module is storage; `crate::hit`
//! in `narvo-app` is the query.

use serde::{Deserialize, Serialize};

/// An axis-parallel rectangle that turns a click into a named action.
///
/// # The rectangle
///
/// Centred on the entity's [`Transform`](crate::Transform) position and measured
/// in world units by its half-extents, so a `HitRect` of `2.0` by `1.0` covers
/// four units across and two down, centred on the transform.
///
/// **It ignores `rotation` and `scale`, and that is one rule rather than two.**
/// Axis-parallel is the decision (an oriented rectangle is a different type with
/// a different test), and an axis-parallel rectangle cannot follow a rotation at
/// all. Honouring `scale` but not `rotation` would be the half-rule that is worse
/// than either: it would look like it tracked the sprite, and it would stop doing
/// so the moment anything turned. So the rectangle is exactly what the file says,
/// where the transform says, and a scaled or turned sprite whose hit area no
/// longer matches it is visible in the file rather than surprising in a click.
///
/// # The action
///
/// A name and a magnitude — the same pair
/// [`InputEvent`](https://docs.rs/narvo-input) carries, because that is what a
/// hit is turned into. The name is **not** validated here, and the omission is
/// deliberate in the way [`Sprite`](crate::Sprite)'s unresolvable region name is:
/// a component is storage, and this crate cannot reach the charset rule anyway —
/// it lives in `narvo-input`, which `narvo-ecs` does not depend on (ADR-0025).
/// The check happens where a world and that rule are both in scope, which is
/// `narvo-app`, and it happens at load time with the position M4.2 gives it.
///
/// # A string in a registered component
///
/// Permitted, and by ADR-0014's own words rather than by analogy: its
/// consequences say a registered type *"holds only scalars, `String`, and other
/// registered-component types"*. [`Sprite`](crate::Sprite) is the precedent that
/// exercised it, and the reasoning there transfers unchanged — the ADR's concern
/// is a dependency's formatting choice entering the hash domain, and `String` has
/// no such owner. What it does bring is escaping, which is checked directly here
/// on adversarial names rather than assumed.
///
/// # Opt-in, so a world that never mentions it is unchanged
///
/// An entity without a `HitRect` cannot be clicked, and a world that never
/// registers the type dumps and hashes exactly as it did before —
/// `registering_the_type_changes_nothing_for_a_world_that_has_none` is the
/// proof, a comparison of two registries and never a stored hash (ADR-0008).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HitRect {
    /// Half the width, in world units, from the transform's position.
    pub half_width: f32,
    /// Half the height, in world units, from the transform's position.
    pub half_height: f32,
    /// The action name a click on this rectangle sends.
    pub action: String,
    /// The magnitude that goes with it.
    pub value: i64,
}

impl HitRect {
    /// A rectangle of these half-extents sending `action` with `value`.
    #[must_use]
    pub fn new(half_width: f32, half_height: f32, action: impl Into<String>, value: i64) -> Self {
        Self {
            half_width,
            half_height,
            action: action.into(),
            value,
        }
    }

    /// Whether the world point `(x, y)` is inside this rectangle, placed at
    /// `(at_x, at_y)`.
    ///
    /// # The boundary belongs to the rectangle
    ///
    /// The comparison is inclusive on all four edges, so a point exactly on an
    /// edge is a hit. Two rectangles that share an edge therefore both contain a
    /// point on it, and the front-most rule decides between them — which is the
    /// same thing that happens when they overlap by a whole unit, rather than a
    /// special case that only appears at a boundary.
    ///
    /// A negative half-extent contains nothing, because the comparison cannot be
    /// satisfied. It is not rejected at construction, for
    /// [`Sprite`](crate::Sprite)'s reason: a component is storage. A `NaN`
    /// half-extent likewise contains nothing, since every comparison against
    /// `NaN` is false.
    #[must_use]
    pub fn contains(&self, at_x: f32, at_y: f32, x: f32, y: f32) -> bool {
        (x - at_x).abs() <= self.half_width && (y - at_y).abs() <= self.half_height
    }
}

#[cfg(test)]
mod tests {
    use super::HitRect;
    use crate::{ComponentRegistry, Transform, World, canonical_dump};

    #[test]
    fn a_point_inside_is_a_hit_and_one_outside_is_not() {
        let rect = HitRect::new(2.0, 1.0, "buy", 1);

        assert!(rect.contains(0.0, 0.0, 0.0, 0.0), "the centre");
        assert!(rect.contains(0.0, 0.0, 1.9, 0.9), "inside");
        assert!(!rect.contains(0.0, 0.0, 2.1, 0.0), "outside in x");
        assert!(!rect.contains(0.0, 0.0, 0.0, 1.1), "outside in y");
    }

    #[test]
    fn the_rectangle_moves_with_the_position_it_is_placed_at() {
        let rect = HitRect::new(1.0, 1.0, "buy", 1);

        assert!(!rect.contains(0.0, 0.0, 10.0, 10.0));
        assert!(rect.contains(10.0, 10.0, 10.0, 10.0));
        assert!(rect.contains(10.0, 10.0, 9.5, 10.5));
    }

    #[test]
    fn every_edge_belongs_to_the_rectangle() {
        let rect = HitRect::new(2.0, 1.0, "buy", 1);

        for (x, y) in [(2.0, 0.0), (-2.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            assert!(rect.contains(0.0, 0.0, x, y), "the edge at ({x}, {y})");
        }
        for (x, y) in [(2.0, 1.0), (-2.0, -1.0)] {
            assert!(rect.contains(0.0, 0.0, x, y), "the corner at ({x}, {y})");
        }
    }

    #[test]
    fn a_negative_or_nan_extent_contains_nothing() {
        // Stored as written, like a `NaN` depth on a `Layer`. The rule deals
        // with it rather than the constructor.
        assert!(!HitRect::new(-1.0, 1.0, "buy", 1).contains(0.0, 0.0, 0.0, 0.0));
        assert!(!HitRect::new(f32::NAN, 1.0, "buy", 1).contains(0.0, 0.0, 0.0, 0.0));
        assert!(!HitRect::new(1.0, 1.0, "buy", 1).contains(f32::NAN, 0.0, 0.0, 0.0));
    }

    #[test]
    fn an_action_name_survives_the_ron_round_trip_with_anything_in_it() {
        // `Sprite`'s test, applied to the second string in a registered
        // component. Escaping is the surface a string brings, so it is checked
        // rather than assumed - even though the loader will refuse most of
        // these, because the component is storage and the check is elsewhere.
        for action in [
            "buy",
            "",
            "with \"quotes\"",
            "with \\ backslash",
            "with\ttab",
            "mit Ümläuten",
        ] {
            let rect = HitRect::new(1.5, 2.5, action, -7);

            let text = ron::to_string(&rect).expect("a rect always serializes");
            let restored: HitRect = ron::from_str(&text).expect("what was written reads back");

            assert_eq!(restored, rect, "{action:?} did not survive as {text}");
        }
    }

    #[test]
    fn the_extents_survive_the_round_trip_bit_for_bit() {
        // ADR-0014's requirement for a component holding floats: `==` calls
        // `0.0` and `-0.0` equal and two `NaN`s unequal, so the check is on
        // bits.
        for value in [
            0.0_f32,
            -0.0,
            f32::MIN_POSITIVE / 2.0,
            f32::MAX,
            1.000_000_1,
            -3.5,
        ] {
            let rect = HitRect::new(value, value, "buy", 0);
            let text = ron::to_string(&rect).expect("a rect always serializes");
            let restored: HitRect = ron::from_str(&text).expect("what was written reads back");

            assert_eq!(
                restored.half_width.to_bits(),
                value.to_bits(),
                "{value} came back as {} from {text}",
                restored.half_width
            );
        }
    }

    #[test]
    fn registering_the_type_changes_nothing_for_a_world_that_has_none() {
        // ADR-0008: two registries compared against each other, never against a
        // stored hash. Registration is not insertion.
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::IDENTITY)
            .expect("a fresh entity takes a component");

        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts it");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts it");
        with.register_component::<HitRect>("hitrect")
            .expect("a fresh registry accepts it");

        assert_eq!(
            canonical_dump(&world, &without).expect("every component is registered"),
            canonical_dump(&world, &with).expect("every component is registered"),
        );
    }
}
