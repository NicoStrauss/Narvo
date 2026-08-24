//! What blocks light.
//!
//! [`Occluder`] is simulation state for the same reason [`HitRect`](crate::HitRect)
//! is, and the reasoning transfers word for word: a world in which light is
//! blocked has to be reproducible from a recording, and a shape that lived
//! outside the world would not be. It goes into the canonical dump and therefore
//! into the state hash of any simulation that registers it.
//!
//! # One source, two consumers — and only one of them exists
//!
//! This is the coupling M8.3b exists to create rather than to use. The same set
//! of occluders is meant to feed **two** readers: the image light that M8.4's ray
//! march builds from the extracted field, and a game's own logic in the tick —
//! line of sight, stealth, a door that darkens a room — which is M8.8's.
//!
//! The second reader does not exist, and nothing here is built for it. What is
//! built is the weaker property that makes it possible later: **the source is
//! readable from the simulation side with no GPU type in the way.** `Occluder`
//! lives in `narvo-ecs`, which depends on `hecs`, `serde` and `ron` and on
//! nothing else in this workspace; a system in the tick reads it with
//! `World::get` exactly as it reads a `Transform`. Nothing about it points at a
//! renderer. `narvo-view2d`'s `seeds_of` is one consumer built on top of that,
//! not the thing the type is for.
//!
//! # The rectangle
//!
//! Axis-parallel, centred on the entity's [`Transform`](crate::Transform)
//! position, measured in world units by its half-extents — the same geometry
//! `HitRect` carries, and deliberately so. The two are not merged into one type:
//! a clickable area and a light blocker are different facts about an entity, a
//! button is usually not an occluder and a wall is usually not clickable, and an
//! entity may need both with different extents. Sharing the *shape* while keeping
//! the *meaning* separate is what lets `contains` be written twice in four lines
//! rather than the two concepts being welded together.
//!
//! **It ignores `rotation` and `scale`**, for `HitRect`'s reason rather than by
//! analogy: axis-parallel is the decision, an axis-parallel rectangle cannot
//! follow a rotation at all, and honouring `scale` but not `rotation` would be
//! the half-rule that looks like it tracks the sprite and stops the moment
//! anything turns.
//!
//! # Opt-in, so a world that never mentions it is unchanged
//!
//! An entity without an `Occluder` blocks nothing, and a world that never
//! registers the type dumps and hashes exactly as it did before —
//! [`registering_the_occluder_type_changes_nothing_for_a_world_that_has_none`](
//! self) is the proof, a comparison of two registries and never a stored hash
//! (ADR-0008).

use serde::{Deserialize, Serialize};

/// An axis-parallel rectangle that blocks light.
///
/// Bare scalars only, which is ADR-0014: no maths library's type enters a
/// registered component, because its serde format would then govern every state
/// hash. Two `f32`, and the module header carries the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Occluder {
    /// Half the width, in world units, from the transform's position.
    pub half_width: f32,
    /// Half the height, in world units, from the transform's position.
    pub half_height: f32,
}

impl Occluder {
    /// A blocker of these half-extents.
    #[must_use]
    pub fn new(half_width: f32, half_height: f32) -> Self {
        Self {
            half_width,
            half_height,
        }
    }

    /// Whether the world point `(x, y)` is inside this rectangle, placed at
    /// `(at_x, at_y)`.
    ///
    /// # The boundary belongs to the rectangle
    ///
    /// Inclusive on all four edges, matching [`HitRect::contains`](crate::HitRect::contains)
    /// exactly. That is not decoration: the extraction in `narvo-view2d` decides
    /// which texels a rectangle covers by asking this question per texel centre,
    /// so an exclusive edge here and an inclusive one there would put the two
    /// halves of the engine one texel apart at every boundary.
    ///
    /// A negative half-extent contains nothing, because the comparison cannot be
    /// satisfied; a `NaN` one likewise, since every comparison against `NaN` is
    /// false. Neither is rejected at construction, for `Sprite`'s reason: a
    /// component is storage.
    #[must_use]
    pub fn contains(&self, at_x: f32, at_y: f32, x: f32, y: f32) -> bool {
        (x - at_x).abs() <= self.half_width && (y - at_y).abs() <= self.half_height
    }
}

#[cfg(test)]
mod tests {
    use super::Occluder;
    use crate::{ComponentRegistry, Transform, World, canonical_dump};

    #[test]
    fn a_point_inside_is_blocked_and_one_outside_is_not() {
        let blocker = Occluder::new(2.0, 1.0);

        assert!(blocker.contains(0.0, 0.0, 0.0, 0.0), "the centre");
        assert!(blocker.contains(0.0, 0.0, 1.9, 0.9), "inside");
        assert!(!blocker.contains(0.0, 0.0, 2.1, 0.0), "outside in x");
        assert!(!blocker.contains(0.0, 0.0, 0.0, 1.1), "outside in y");
    }

    /// Every edge and corner belongs to the rectangle.
    ///
    /// **The half of the contract the extraction leans on.** `seeds_of` asks this
    /// question once per texel centre, so whether an edge is inside decides
    /// whether the last row of texels is seeded. It matches
    /// `HitRect::contains` by construction, and this test is what says so.
    #[test]
    fn every_edge_belongs_to_the_occluder() {
        let blocker = Occluder::new(2.0, 1.0);

        for (x, y) in [(2.0, 0.0), (-2.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            assert!(blocker.contains(0.0, 0.0, x, y), "the edge at ({x}, {y})");
        }
        for (x, y) in [(2.0, 1.0), (-2.0, -1.0), (2.0, -1.0), (-2.0, 1.0)] {
            assert!(blocker.contains(0.0, 0.0, x, y), "the corner at ({x}, {y})");
        }
    }

    #[test]
    fn the_rectangle_moves_with_the_position_it_is_placed_at() {
        let blocker = Occluder::new(1.0, 1.0);

        assert!(!blocker.contains(0.0, 0.0, 10.0, 10.0));
        assert!(blocker.contains(10.0, 10.0, 10.0, 10.0));
        assert!(blocker.contains(10.0, 10.0, 9.5, 10.5));
    }

    #[test]
    fn a_negative_or_nan_extent_blocks_nothing() {
        assert!(!Occluder::new(-1.0, 1.0).contains(0.0, 0.0, 0.0, 0.0));
        assert!(!Occluder::new(f32::NAN, 1.0).contains(0.0, 0.0, 0.0, 0.0));
        assert!(!Occluder::new(1.0, 1.0).contains(f32::NAN, 0.0, 0.0, 0.0));
    }

    /// The type survives the registry's own round trip, in the registry's own
    /// spelling.
    ///
    /// The RON is asserted directly rather than only compared against itself, so
    /// that a serde attribute changing the representation shows up here as an
    /// edit rather than in a save file somebody cannot load.
    #[test]
    fn an_occluder_survives_the_canonical_dump_in_its_own_spelling() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Occluder>("occluder")
            .expect("a fresh registry accepts the type");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Occluder::new(2.5, 0.5))
            .expect("the world takes a component");

        let dumped = canonical_dump(&world, &registry).expect("the world dumps");
        assert!(
            dumped.contains("occluder (half_width:2.5,half_height:0.5)"),
            "the occluder is not spelled as expected in:\n{dumped}"
        );
    }

    /// **§4(a): a world that carries one dumps differently from one that does
    /// not, and moving it moves the dump again.**
    ///
    /// Three worlds compared against each other, never against a stored hash
    /// (ADR-0008). The third comparison is the one that matters for a light: an
    /// occluder that moved has to be a different world, or a recording could
    /// replay a wall into the wrong place and agree with itself.
    #[test]
    fn adding_and_moving_an_occluder_both_move_the_dump() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts the type");
        registry
            .register_component::<Occluder>("occluder")
            .expect("a fresh registry accepts the type");

        let bare = {
            let mut world = World::new();
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(1.0, 2.0))
                .expect("the world takes a component");
            canonical_dump(&world, &registry).expect("the world dumps")
        };

        let blocked = {
            let mut world = World::new();
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(1.0, 2.0))
                .expect("the world takes a component");
            world
                .insert(entity, Occluder::new(1.0, 1.0))
                .expect("the world takes a component");
            canonical_dump(&world, &registry).expect("the world dumps")
        };

        let moved = {
            let mut world = World::new();
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(1.0, 3.0))
                .expect("the world takes a component");
            world
                .insert(entity, Occluder::new(1.0, 1.0))
                .expect("the world takes a component");
            canonical_dump(&world, &registry).expect("the world dumps")
        };

        assert_ne!(bare, blocked, "adding an occluder did not move the dump");
        assert_ne!(blocked, moved, "moving an occluder did not move the dump");

        // And the same world twice is the same dump — the other half of (a),
        // which a test asserting only inequality would leave unsaid.
        let again = {
            let mut world = World::new();
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(1.0, 2.0))
                .expect("the world takes a component");
            world
                .insert(entity, Occluder::new(1.0, 1.0))
                .expect("the world takes a component");
            canonical_dump(&world, &registry).expect("the world dumps")
        };
        assert_eq!(blocked, again, "one world dumped two ways");
    }

    /// Registering the type moves nothing for a world that carries none.
    ///
    /// Two registries over one world, compared against each other — never
    /// against a stored hash (ADR-0008). This is what lets the type be added to
    /// `register_engine_components` without every existing scene's hash moving,
    /// and it is the mechanism V3 of M8.3b's pre-registration predicts from.
    #[test]
    fn registering_the_occluder_type_changes_nothing_for_a_world_that_has_none() {
        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts the type");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts the type");
        with.register_component::<Occluder>("occluder")
            .expect("a fresh registry accepts the type");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::at(3.0, 4.0))
            .expect("the world takes a component");

        assert_eq!(
            canonical_dump(&world, &without).expect("the world dumps"),
            canonical_dump(&world, &with).expect("the world dumps"),
            "registering the occluder type moved a world that carries none"
        );
    }
}
