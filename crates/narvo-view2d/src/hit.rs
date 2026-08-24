//! Which entity a click belongs to.
//!
//! The mirror of the draw order, and it lives here for that reason.
//! [`HitRect`](narvo_ecs::HitRect) is the *state* and belongs to `narvo-ecs`;
//! deciding which of several overlapping rectangles wins is the same decision as
//! deciding which sprite is drawn on top, and ADR-0015 put that one beside the
//! extraction. **Since M6b.1 that place is this crate rather than `narvo-app`**
//! (ADR-0041); `Layer`'s own documentation still says *"lives with the extraction
//! in `narvo-app`"*, and it is the sentence that is out of date rather than the
//! arrangement — the rule still lives with the extraction, and the extraction
//! moved.
//!
//! So [`depth_order`] is here rather than in [`sprite_batch`](crate::sprite_batch),
//! and `placements_of` imports it from this module. **One copy, two consumers** —
//! the draw order and its mirror cannot disagree about what `-0.0` means, because
//! they ask the same function.
//!
//! **That sentence is why this crate was not split in two.** The obvious cut runs
//! between what needs the renderer (the extraction) and what needs only the ECS
//! (this module), and it runs straight through `depth_order`: either each side
//! gets a copy, or one crate depends on the other to borrow it. ADR-0041 records
//! what keeping them together costs — a consumer wanting only [`hit_test`]
//! carries the graphics stack — and the condition under which the cut is
//! reconsidered.
//!
//! Nothing here sees a window or a winit type, which is what lets the whole
//! module be tested headless: a hit test takes a world and a world point.
//! Turning a cursor into that point is `Projection::screen_to_world`'s job, one
//! layer out.

use narvo_ecs::{EntityId, HitRect, Layer, Transform, World};

/// The value a depth is compared by, which is the depth itself except for zero.
///
/// One transformation, and it exists for one reason: [`f32::total_cmp`] follows
/// IEEE 754's `totalOrder`, in which `-0.0` sits strictly below `+0.0`. Content
/// does not mean that — a depth of `-0.0` is a depth of zero, however it was
/// computed — so every zero is mapped to `+0.0` here and the two tie, leaving
/// the order to the entity id.
///
/// `depth == 0.0` is true for both zeros and false for `NaN`, so a `NaN` passes
/// through untouched and keeps the position `total_cmp` gives it.
///
/// **It moved here from `sprite_batch` in M5.4** and gained a second caller
/// rather than a second copy. A hit test that normalised zero differently from
/// the draw order would put a click on the sprite that is *not* in front, and
/// only for depths of `-0.0` — which is the kind of defect that survives every
/// test somebody thinks to write.
///
/// **M6b.1 moved both callers into this crate**, which is a smaller change than
/// it sounds: the two were already in one crate and are still in one crate, and
/// the single copy is the reason that crate was not split along the line the
/// dependency graph suggests (ADR-0041).
pub fn depth_order(depth: f32) -> f32 {
    if depth == 0.0 { 0.0 } else { depth }
}

/// The front-most entity whose [`HitRect`] contains the world point `(x, y)`.
///
/// # Front-most is the mirror of last-drawn
///
/// `placements_of` sorts ascending by `depth_order(depth)` and then by
/// [`EntityId`], and draws in that order — so the **last** entity in that
/// sequence is on top. This returns the entity that would be last among those
/// hit: the greatest `(depth_order(depth), id)` pair.
///
/// The tie-break is therefore the entity id, and the larger id wins, because the
/// larger id is drawn later. Stating it as one comparison over a pair rather
/// than as two rules is what keeps it the same comparison the sort uses.
///
/// An entity is a candidate only if it carries **both** a `Transform` and a
/// `HitRect`: the transform says where the rectangle is, and without one there
/// is nothing to place. A missing [`Layer`] means [`Layer::DEFAULT`], exactly as
/// it does when drawing.
///
/// # It is pure
///
/// No clock, no entropy, no state, and it does not touch the world. The same
/// world and the same point give the same answer on every platform, which is
/// what lets a click become an `narvo_input::InputEvent` that a
/// replay reproduces.
#[must_use]
pub fn hit_test(world: &World, x: f32, y: f32) -> Option<EntityId> {
    let mut best: Option<(f32, EntityId)> = None;

    for entity in world.entity_ids() {
        let Ok(rect) = world.get::<HitRect>(entity) else {
            continue;
        };
        let Ok(transform) = world.get::<Transform>(entity) else {
            continue;
        };

        if !rect.contains(transform.x, transform.y, x, y) {
            continue;
        }

        let depth = depth_order(
            world
                .get::<Layer>(entity)
                .map_or(Layer::DEFAULT.depth, |layer| layer.depth),
        );

        let better = match best {
            None => true,
            Some((best_depth, best_entity)) => depth
                .total_cmp(&best_depth)
                .then(entity.cmp(&best_entity))
                .is_gt(),
        };

        if better {
            best = Some((depth, entity));
        }
    }

    best.map(|(_, entity)| entity)
}

#[cfg(test)]
mod tests {
    use super::{depth_order, hit_test};
    use narvo_ecs::{EntityId, HitRect, Layer, Transform, World};

    /// A world with one clickable square per `(x, depth)`, in the order given.
    fn world_of(entities: &[(f32, f32)]) -> (World, Vec<EntityId>) {
        let mut world = World::new();
        let mut ids = Vec::new();

        for (x, depth) in entities {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        x: *x,
                        ..Transform::IDENTITY
                    },
                )
                .expect("a fresh entity takes a component");
            world
                .insert(entity, Layer { depth: *depth })
                .expect("a fresh entity takes a component");
            world
                .insert(entity, HitRect::new(10.0, 10.0, "click", 1))
                .expect("a fresh entity takes a component");
            ids.push(entity);
        }

        (world, ids)
    }

    #[test]
    fn a_point_inside_one_rectangle_finds_it() {
        let (world, ids) = world_of(&[(0.0, 0.0)]);

        assert_eq!(hit_test(&world, 5.0, -5.0), Some(ids[0]));
    }

    #[test]
    fn a_point_outside_every_rectangle_finds_nothing() {
        let (world, _) = world_of(&[(0.0, 0.0)]);

        assert_eq!(hit_test(&world, 50.0, 0.0), None);
        assert_eq!(hit_test(&world, 0.0, 50.0), None);
    }

    #[test]
    fn the_greater_depth_wins_however_the_entities_were_spawned() {
        // Spawned front-first, so a rule that just took the first or the last
        // hit would get one of these two backwards.
        let (front_first, ids) = world_of(&[(0.0, 5.0), (0.0, 1.0)]);
        assert_eq!(hit_test(&front_first, 0.0, 0.0), Some(ids[0]));

        let (back_first, ids) = world_of(&[(0.0, 1.0), (0.0, 5.0)]);
        assert_eq!(hit_test(&back_first, 0.0, 0.0), Some(ids[1]));
    }

    #[test]
    fn at_equal_depth_the_later_entity_wins() {
        // The tie-break, and it is the mirror of the draw order: equal depths
        // are drawn in id order, so the greater id is drawn last and is in
        // front.
        let (world, ids) = world_of(&[(0.0, 2.0), (0.0, 2.0), (0.0, 2.0)]);

        assert_eq!(hit_test(&world, 0.0, 0.0), Some(ids[2]));
    }

    #[test]
    fn the_two_zeros_are_one_depth_and_the_id_decides() {
        // The reason `depth_order` is shared rather than copied. Under
        // `total_cmp` alone `-0.0` sorts below `+0.0`, so the second entity
        // would lose despite the later id.
        let (world, ids) = world_of(&[(0.0, -0.0), (0.0, 0.0)]);
        assert_eq!(hit_test(&world, 0.0, 0.0), Some(ids[1]));

        let (reversed, ids) = world_of(&[(0.0, 0.0), (0.0, -0.0)]);
        assert_eq!(
            hit_test(&reversed, 0.0, 0.0),
            Some(ids[1]),
            "whichever way round they are written, the id decides"
        );

        assert_eq!(depth_order(-0.0).to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn an_entity_without_a_transform_or_without_a_rect_is_not_a_candidate() {
        let mut world = World::new();

        let no_transform = world.spawn();
        world
            .insert(no_transform, HitRect::new(10.0, 10.0, "click", 1))
            .expect("a fresh entity takes a component");

        let no_rect = world.spawn();
        world
            .insert(no_rect, Transform::IDENTITY)
            .expect("a fresh entity takes a component");

        assert_eq!(hit_test(&world, 0.0, 0.0), None);
    }

    #[test]
    fn a_missing_layer_is_the_default_depth() {
        let mut world = World::new();

        let bare = world.spawn();
        world
            .insert(bare, Transform::IDENTITY)
            .expect("a fresh entity takes a component");
        world
            .insert(bare, HitRect::new(10.0, 10.0, "click", 1))
            .expect("a fresh entity takes a component");

        let layered = world.spawn();
        world
            .insert(layered, Transform::IDENTITY)
            .expect("a fresh entity takes a component");
        world
            .insert(
                layered,
                Layer {
                    depth: Layer::DEFAULT.depth,
                },
            )
            .expect("a fresh entity takes a component");
        world
            .insert(layered, HitRect::new(10.0, 10.0, "click", 1))
            .expect("a fresh entity takes a component");

        // Same depth, so the later id wins - which it could only do if the bare
        // entity was given the default rather than skipped or sunk.
        assert_eq!(hit_test(&world, 0.0, 0.0), Some(layered));
    }

    #[test]
    fn the_rectangle_is_placed_at_the_transform_and_ignores_scale_and_rotation() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform {
                    x: 100.0,
                    y: 0.0,
                    rotation: 0.75,
                    scale_x: 8.0,
                    scale_y: 8.0,
                },
            )
            .expect("a fresh entity takes a component");
        world
            .insert(entity, HitRect::new(10.0, 10.0, "click", 1))
            .expect("a fresh entity takes a component");

        assert_eq!(
            hit_test(&world, 105.0, 0.0),
            Some(entity),
            "at the transform"
        );
        assert_eq!(
            hit_test(&world, 140.0, 0.0),
            None,
            "a scale of 8 does not make the rectangle eighty units wide"
        );
    }

    #[test]
    fn an_empty_world_is_a_miss_rather_than_a_panic() {
        assert_eq!(hit_test(&World::new(), 0.0, 0.0), None);
    }
}
