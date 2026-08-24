//! Reading `Transform`s out of a world and handing them to the renderer.
//!
//! This is the seam D12 puts here rather than inside `narvo-render2d`
//! (ADR-0015). The renderer takes a slice of scalars and knows nothing about an
//! ECS; everything that knows what a `World` is lives on this side of the line.
//!
//! The interesting decision — what order the sprites come out in — is small
//! enough to state, test without a GPU, and read in one screen.
//!
//! **This module lived in `narvo-app` until M6b.1** (ADR-0041). Nothing in it
//! changed shape by moving: the same functions read the same components and
//! return the same scalars, and the reference image drawn through
//! [`placements_of`] is byte-identical either side. What moved is the *reach* —
//! `narvo-app` is a binary with no library target, so until the move these
//! functions were callable from unit tests inside `src/` and from nowhere else,
//! which ADR-0015 named as the thing that would stop working the moment a second
//! consumer needed the extraction. It did.
//!
//! What stayed behind is the half that cannot travel: the tests that render a
//! world and compare it against a PNG committed under
//! `crates/narvo-app/tests/golden/`. They live in that crate's `blessed_scenes`
//! and call these functions from outside now.

use narvo_ecs::{
    Burst, Camera, EntityId, Layer, RigidBody, Sampling, Sprite, Tint, Transform, World,
};
use narvo_render2d::{CameraView, SpriteFilter, SpritePlacement, SpriteTint};

use crate::hit::depth_order;

/// Every entity carrying a [`Transform`], as placements for the renderer.
///
/// # Order
///
/// **By [`Layer::depth`] ascending, ties broken by ascending
/// [`EntityId`](narvo_ecs::EntityId).** Draw order is the order of this
/// vector, and there is no depth buffer (`depth_stencil: None` in
/// `narvo-render2d`'s pipeline), so a later sprite overwrites an earlier one
/// where they overlap: larger depth ends up in front.
///
/// An entity without a `Layer` is drawn at [`Layer::DEFAULT`], which is `0.0`.
/// A world that mentions no layer at all therefore comes out in exactly the
/// order it came out in before M3.10 — ascending `EntityId` — because that is
/// what the tie-break degenerates to when every depth is equal.
///
/// **This is where draw order is decided, and that is not an accident of
/// implementation.** ADR-0015 records the finding from building this seam: it
/// is the natural place for draw order, because it iterates
/// [`World::entity_ids`] — the canonical enumeration — rather than a query,
/// whose archetype order `narvo-ecs` documents as explicitly unstable. Sorting
/// here also keeps `narvo-render2d`'s contract intact: that crate draws the
/// slice in the order it is given and sorts nothing, so it needs no depth
/// scalar and no dependency on `narvo-ecs`.
///
/// # The tie-break, and why the order is total
///
/// No two entities alive at the same moment share an `EntityId`: it is a slot
/// index plus a generation, and a slot holds one entity at a time. So the
/// comparison *(depth, id)* is a **total** order and no two sprites can compare
/// equal, which means the result does not depend on whether the sort is stable.
/// [`slice::sort_by`], which the standard library documents as stable, is used
/// anyway: if a later edit ever dropped the id from the comparator, ties would
/// still keep the `entity_ids` order they arrived in, so the failure mode would
/// be a weaker guarantee rather than a nondeterministic image.
///
/// One consequence of ordering by slot index, named rather than hidden:
/// despawning an entity and spawning another reuses the slot with the
/// generation raised, and `EntityId`'s [`Ord`] compares the index first
/// (`narvo-ecs`'s `entity.rs` documents that field order as load-bearing). The
/// new entity therefore takes the old one's place in the tie-break rather than
/// joining the end. That is reproducible, which is what this order is for, but
/// it is not "later spawns draw later".
///
/// # `NaN` and `-0.0`
///
/// Both are reachable, both are defined here, and neither is normalised in the
/// stored component — [`Transform`] already refuses to normalise a rotation for
/// the same reason, that storing something other than what was written puts a
/// rounding step between a system and its own state.
///
/// - **`-0.0` ties with `+0.0`.** The comparison maps every zero to `+0.0`
///   before ordering, so two sprites at `-0.0` and `0.0` are at the same depth
///   and their order falls to the id. Without that step
///   [`f32::total_cmp`] would place `-0.0` strictly below `+0.0`, which is IEEE
///   754's `totalOrder` and would mean that a depth computed as `-1.0 * 0.0`
///   draws behind one written as `0.0`.
/// - **`NaN` sorts to an end and does not panic.** [`f32::total_cmp`] is a
///   total order over every bit pattern: a positive `NaN` orders above `+inf`,
///   a negative one below `-inf`. So a `NaN` depth draws last, or first if its
///   sign bit is set. That is defined and reproducible rather than correct —
///   a `NaN` depth is a bug in whatever produced it, and this only guarantees
///   that two runs agree about where it lands. `partial_cmp` was rejected for
///   this: it returns `None` for `NaN`, and every way of spending that `None`
///   is worse — `unwrap` panics inside a render frame, and a fallback ordering
///   invents an answer at the point where the caller most needs to be told.
///
/// # Cost
///
/// One `SpritePlacement` is copied per entity, every time this is called. That
/// copy is the price D12 names and accepts; ADR-0015 records what it buys and
/// what it will cost when the throughput measurement arrives. M3.7 measured it:
/// 14.6 µs of 1 025 µs at 50 000 sprites, so the copy is not what this costs.
///
/// # The `collect` grows, and preallocating it was measured to be slower
///
/// `filter_map`'s size hint has a lower bound of zero, so this `collect` cannot
/// preallocate and doubles instead — 50 000 placements land in a vector of
/// capacity 65 536, about `log2(n)` allocations. That reads like a defect and
/// M3.7 reported it as one.
///
/// M3.8 replaced it with `Vec::with_capacity(entities.len())` and a push loop,
/// and measured the result rather than assuming it: **the preallocated shape is
/// consistently slower on this platform** — 11 to 19 % at 50 000 entities, about
/// 80 % at 1 000 — across five order-balanced paired runs in one process and two
/// independent eight-run distributions. The change was therefore not kept. The
/// figures are in `docs/perf/BASELINE.md`.
///
/// Why it is slower is **not established**. The plausible mechanism is that one
/// fresh megabyte has to be faulted in page by page on first write, while
/// growth through `realloc` can extend a block that is already resident; that is
/// a hypothesis, not a measurement, and it is written here as one.
///
/// The point of the paragraph is the trap, not the mechanism: this looks like a
/// one-line improvement and measures as a regression. Anyone reaching for
/// `with_capacity` here should measure it order-balanced before believing it.
///
/// # The first caller arrived in M3.32
///
/// Until then no runner drew a world — the window and screenshot paths rendered
/// a fixed image — so this had none inside the binary, and it carried
/// `#[cfg_attr(not(test), expect(dead_code, …))]` saying so. The frame loop's
/// `SceneHost::extract` is that caller, and the attribute is gone.
///
/// **Removing it was forced rather than chosen**, which is the whole reason it
/// was an `expect` and not an `allow`: an `expect` that stops being needed
/// becomes an unfulfilled-lint warning, and the workspace denies warnings. The
/// compiler said so on the first build that wired the seam up.
#[must_use]
pub fn placements_of(world: &World) -> Vec<Drawn> {
    let mut drawn: Vec<(EntityId, f32, Drawn)> = world
        .entity_ids()
        .into_iter()
        .filter_map(|entity| {
            let transform = *world.get::<Transform>(entity).ok()?;
            let depth = world
                .get::<Layer>(entity)
                .map_or(Layer::DEFAULT.depth, |layer| layer.depth);
            let filter = world
                .get::<Sampling>(entity)
                .map_or(Sampling::DEFAULT.filter, |sampling| sampling.filter);
            let tint = world
                .get::<Tint>(entity)
                .map_or(Tint::DEFAULT, |tint| *tint);

            Some((
                entity,
                depth,
                Drawn {
                    placement: placement_of_transform(transform),
                    filter: filter_of(filter),
                    tint: tint_of(tint),
                },
            ))
        })
        .collect();

    drawn.sort_by(|(left_id, left_depth, _), (right_id, right_depth, _)| {
        depth_order(*left_depth)
            .total_cmp(&depth_order(*right_depth))
            .then(left_id.cmp(right_id))
    });

    drawn.into_iter().map(|(_, _, drawn)| drawn).collect()
}

/// What the world says about drawing one entity.
///
/// Not an `narvo_render2d::SpriteInstance`: that also needs a `TextureRegion`, and which part of
/// which atlas an entity shows is the caller's knowledge, not the world's. This
/// carries exactly the two things the world does hold — where it goes, and how
/// it wants to be sampled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drawn {
    /// Where it goes, in the renderer's own scalars (ADR-0015).
    pub placement: SpritePlacement,
    /// How it wants its texture read.
    pub filter: SpriteFilter,
    /// The colour its texels are multiplied by.
    ///
    /// [`SpriteTint::UNTINTED`] for an entity carrying no `Tint`, which is what
    /// keeps every world written before M6b.3 drawing the picture it had.
    pub tint: SpriteTint,
}

/// One entity that draws a **named** region, in draw order.
///
/// The scene-file mode's counterpart to [`Drawn`], and the reason there are two
/// of them rather than one with an optional field: M4.8's constraint was that
/// [`placements_of`] stay exactly as it is, because a blessed reference is drawn
/// through it. A second function is what "untouched" means when a new consumer
/// needs different information.
///
/// **This paragraph said "six blessed references" until M6b.1, and the number was
/// wrong.** Counted from the call sites, before the move and again after it, it
/// is **one**: `layer_order_regions_128x128`, through
/// `render_the_overlapping_scene` in `narvo-app`'s `blessed_scenes`. Two of
/// `narvo-app`'s other references go through [`regions_of`] instead —
/// `click_counter_state3_128x128` via `counter_sprites`, and
/// `physics_drop_tick45_128x128` via the `Regions::ByName` arm of
/// `frame::SceneHost::extract` — and `narvo-render2d`'s seven can reach neither
/// function, because that crate does not depend on this one and builds its
/// `SpritePlacement` values directly.
///
/// The constraint the number was there to justify is untouched: one reference is
/// still a reference, and M4.8 still added a second function rather than widening
/// the first. What changed is that the figure is now countable rather than
/// remembered.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawnRegion {
    /// Where it goes, in the renderer's own scalars (ADR-0015).
    pub placement: SpritePlacement,
    /// How it wants its texture read.
    pub filter: SpriteFilter,
    /// Which packed region it shows, by name.
    pub region: String,
    /// The colour its texels are multiplied by.
    ///
    /// [`SpriteTint::UNTINTED`] for an entity carrying no `Tint`.
    pub tint: SpriteTint,
}

/// Every entity carrying a pose and a [`Sprite`], in draw order.
///
/// # What it does that [`placements_of`] does not
///
/// Two things. It requires a [`Sprite`] and carries its region name — an entity
/// with a pose and no sprite **does not appear**, which is the scene-file mode's
/// rule since M4.8: what a thing looks like is content, and an entity that says
/// nothing about its appearance has not asked to be drawn. The transitional
/// answer it replaces — a quadrant of the demo texture chosen by draw-order
/// position — is retired, and ADR-0024 records the retirement.
///
/// And since M5b.4 it reads a **pose**, not only a [`Transform`]: an entity
/// carrying a [`RigidBody`] is drawn from the body. [`placement_of`] is that
/// choice and says why the body wins where both are present.
///
/// # The order is the same order, and that is measured rather than shared
///
/// Sorting is duplicated here rather than factored out, because factoring it out
/// would mean changing [`placements_of`]. What keeps the two from drifting is
/// `the_two_extractions_agree_on_order_when_every_entity_has_a_sprite`, which
/// renders the same world through both and compares the placements they yield.
/// A shared helper would make the two agree by construction; this makes them
/// agree by measurement, which is the stronger statement and the one that
/// survives somebody editing either.
///
/// # Since M6b.7 it also draws a [`Burst`]'s particles
///
/// An entity carrying a live `Burst` draws its own sprite **and** one copy of it
/// per particle, scattered by [`Burst::particle`] and dimmed by the fade that
/// call returns. ADR-0044 is the decision behind that: the world stores the
/// emitter and the particles are a function of it, so there is no entity to
/// enumerate and the copies have to be made here, at the seam that already knows
/// what a world is and what the renderer wants.
///
/// **A world with no live burst comes out exactly as it did before**, and that
/// is a property of the sort key rather than of a branch: the key is
/// *(depth, entity, slot)* where slot is `0` for an entity's own sprite and
/// `index + 1` for its particles. Every entity without a burst has slot `0`
/// alone, so the key degenerates to the *(depth, entity)* it always was.
/// `a_world_without_a_burst_draws_exactly_what_it_drew` measures it against a
/// world holding a spent burst.
#[must_use]
pub fn regions_of(world: &World) -> Vec<DrawnRegion> {
    // The slot: 0 for the entity's own sprite, index + 1 for particle index.
    // `usize` rather than `u32` so the sprite's own slot has somewhere to sit
    // below a burst of `u32::MAX` particles.
    let mut drawn: Vec<(f32, EntityId, usize, DrawnRegion)> = Vec::new();

    for entity in world.entity_ids() {
        let Some(placement) = placement_of(world, entity) else {
            continue;
        };
        let Ok(sprite) = world.get::<Sprite>(entity) else {
            continue;
        };
        let region = sprite.region.clone();
        let depth = world
            .get::<Layer>(entity)
            .map_or(Layer::DEFAULT.depth, |layer| layer.depth);
        let filter = world
            .get::<Sampling>(entity)
            .map_or(Sampling::DEFAULT.filter, |sampling| sampling.filter);
        let tint = world
            .get::<Tint>(entity)
            .map_or(Tint::DEFAULT, |tint| *tint);

        drawn.push((
            depth,
            entity,
            0,
            DrawnRegion {
                placement,
                filter: filter_of(filter),
                region: region.clone(),
                tint: tint_of(tint),
            },
        ));

        let Ok(burst) = world.get::<Burst>(entity) else {
            continue;
        };
        let burst = *burst;
        for index in 0..burst.count {
            let Some((offset_x, offset_y, fade)) = burst.particle(index) else {
                // Spent, or an index the burst does not have. `particle` is the
                // one place that decides either, so this loop asks rather than
                // deciding a second time.
                break;
            };

            drawn.push((
                depth,
                entity,
                index as usize + 1,
                DrawnRegion {
                    placement: SpritePlacement {
                        x: placement.x + offset_x,
                        y: placement.y + offset_y,
                        ..placement
                    },
                    filter: filter_of(filter),
                    region: region.clone(),
                    // The fade is in `0.0 ..= 1.0` and multiplies the alpha
                    // alone, so a tint inside ADR-0023's premultiplied invariant
                    // stays inside it. A fade is not a glow (ADR-0044).
                    tint: tint_of(Tint {
                        alpha: tint.alpha * fade,
                        ..tint
                    }),
                },
            ));
        }
    }

    drawn.sort_by(
        |(left_depth, left_id, left_slot, _), (right_depth, right_id, right_slot, _)| {
            depth_order(*left_depth)
                .total_cmp(&depth_order(*right_depth))
                .then(left_id.cmp(right_id))
                .then(left_slot.cmp(right_slot))
        },
    );

    drawn.into_iter().map(|(_, _, _, drawn)| drawn).collect()
}

/// Every region name a world asks for, ascending and without repeats.
///
/// What a loader checks against an atlas before a window opens, so that an
/// unknown name is a load error rather than a missing sprite nobody notices.
#[must_use]
pub fn region_names_of(world: &World) -> std::collections::BTreeSet<String> {
    world
        .entity_ids()
        .into_iter()
        .filter_map(|entity| world.get::<Sprite>(entity).ok().map(|s| s.region.clone()))
        .collect()
}

/// The placement a [`Transform`] describes.
///
/// **This is where an angle becomes a `(cos, sin)` pair**, and until M5b.4 the
/// conversion sat one stage later, inside `narvo-render2d`'s `sprite_vertices`.
/// It is the same [`f32::sin_cos`] call on the same value, so the pixels of every
/// blessed reference are unmoved; what changed is that a caller which already
/// holds a pair — [`placement_of_body`] — no longer passes through it.
///
/// It is trigonometry, and this is the crate ADR-0015 puts trigonometry in when a
/// component's own form and a renderer's differ. The result is never written back
/// into the world and never hashed (ADR-0005: the render path only reads), which
/// is what makes a standard-library `sin_cos` admissible here and inadmissible in
/// a system. [`RigidBody`]'s module documentation carries the measurement behind
/// that distinction.
fn placement_of_transform(transform: Transform) -> SpritePlacement {
    SpritePlacement {
        x: transform.x,
        y: transform.y,
        ..SpritePlacement::new(transform.scale_x, transform.scale_y).turned(transform.rotation)
    }
}

/// The placement a [`RigidBody`] describes.
///
/// **A sprite drawn from a body covers the collider exactly.** The half-extents
/// are doubled into the placement's width and height, which is a multiplication
/// by two and therefore exact in binary floating point, and the rotation pair is
/// copied verbatim. So what a reader sees on screen is the rectangle the solver
/// collided, not an approximation of it — which is the property that makes this
/// scene worth looking at rather than merely worth hashing.
///
/// **No trigonometric operation anywhere on this path.** The body holds
/// `(rot_cos, rot_sin)`, [`SpritePlacement`] holds the same pair since M5b.4, and
/// the corner arithmetic multiplies it. The alternative — projecting the pair to
/// an angle with `atan2` so that the pre-M5b.4 placement could carry it — would
/// have put standard-library trigonometry between a body and its pixels, twice:
/// `atan2` here and `sin_cos` there. Both are outside `enhanced-determinism`'s
/// reach and M5b.2 measured the round trip to be lossy on Windows and exact on
/// Linux, so the price would not have been paid in the state hash — the render
/// path writes nothing — but in a **golden image**, which is rendered on both
/// platforms and compared against one committed reference.
fn placement_of_body(body: &RigidBody) -> SpritePlacement {
    SpritePlacement {
        x: body.x,
        y: body.y,
        rot_cos: body.rot_cos,
        rot_sin: body.rot_sin,
        scale_x: body.half_x * 2.0,
        scale_y: body.half_y * 2.0,
    }
}

/// Where an entity is drawn: its body if it has one, otherwise its transform.
///
/// `None` for an entity that carries neither, which is what keeps a bare entity
/// out of the batch.
///
/// # The body wins, and that is a decision rather than an ordering accident
///
/// An entity carrying both a [`RigidBody`] and a [`Transform`] is drawn from the
/// body. The solver owns a body's pose and writes it every tick
/// (`narvo-app`'s `physics::write_back`), while nothing in the engine writes a
/// `Transform` from a body — so the transform beside a body is whatever the scene
/// file said at load, and drawing from it would put the sprite where the thing
/// started rather than where it is.
///
/// **Neither reading changes any existing picture**, because no world in this
/// repository carries both today; the choice is made here so that the first one
/// which does behaves the way it reads. `a_body_beside_a_transform_is_drawn_from_the_body`
/// pins it.
///
/// The two are not merged and a body does not consult a transform for its size:
/// a body's extents *are* its size, and taking `scale` from a neighbouring
/// component would let a sprite and its collider disagree without anything
/// saying so.
fn placement_of(world: &World, entity: EntityId) -> Option<SpritePlacement> {
    if let Ok(body) = world.get::<RigidBody>(entity) {
        return Some(placement_of_body(&body));
    }

    world
        .get::<Transform>(entity)
        .ok()
        .map(|transform| placement_of_transform(*transform))
}

/// The renderer's filter for a [`Sampling`] code.
///
/// **This is the mapping `Sampling`'s table names**, and it lives here rather
/// than in `narvo-ecs` for the reason ADR-0015 gives about `depth_order`: this
/// is the crate that sees both the world and the renderer, and `narvo-ecs` has
/// no business knowing that a `SpriteFilter` exists.
///
/// An unrecognised code is `Nearest` — the same value an entity carrying no
/// `Sampling` at all gets. It is not an error: the component is storage and does
/// not validate, so the conservative reading belongs here, where "conservative"
/// means "the picture every scene had before M3.23".
const fn filter_of(code: u8) -> SpriteFilter {
    match code {
        Sampling::LINEAR => SpriteFilter::Linear,
        _ => SpriteFilter::Nearest,
    }
}

/// The renderer's tint for a world's [`Tint`].
///
/// Here for the same reason [`filter_of`] is: this is the crate that sees both
/// sides, and `narvo-ecs` has no business knowing a `SpriteTint` exists
/// (ADR-0015).
///
/// **A copy of four numbers, and nothing else.** Unlike [`filter_of`] there is
/// no mapping to get wrong — no code to interpret, no reserved value, no
/// conservative reading — because ADR-0014 already forced both sides to be bare
/// `f32`. That the two types stay in this shape is what keeps this function
/// trivial, and it is why the function exists at all rather than the seam
/// handing `narvo-ecs`'s type to the renderer: a shared type would make the
/// renderer depend on the ECS.
///
/// Nothing is clamped. `Tint` records that a channel above one breaks the
/// premultiplied invariant and is a named limit rather than a rejected value;
/// clamping *here* would be the seam quietly changing what a world says, which
/// is the one thing an extraction must not do.
const fn tint_of(tint: Tint) -> SpriteTint {
    SpriteTint {
        red: tint.red,
        green: tint.green,
        blue: tint.blue,
        alpha: tint.alpha,
    }
}

/// The view a world is drawn through, as the renderer's own scalars.
///
/// **The lowest [`EntityId`] carrying a [`Camera`] wins; a world with none is
/// drawn through [`CameraView::IDENTITY`],** which is the fixed projection every
/// render used before M3.12. So a world that never mentions a camera renders
/// exactly as it did, and adding one to a single entity is the whole of turning
/// the camera on.
///
/// # Why the lowest id rather than an error
///
/// Two cameras in one world is a content mistake, and this returns the first
/// rather than refusing. The alternative — a `Result`, and a frame that fails to
/// draw — was rejected because the failure would arrive inside a render call,
/// where nothing can act on it and the user sees a black window instead of a
/// slightly wrong one. Naming it here is the honest half: **the second camera is
/// silently ignored**, and `the_lowest_entity_id_owns_the_camera` pins which one
/// that is so the choice cannot drift into archetype order.
///
/// Lowest id, not "the one spawned first": [`World::entity_ids`] is the
/// canonical ascending enumeration, and `EntityId`'s [`Ord`] compares the slot
/// index before the generation, so a despawn plus a spawn reuses the slot and
/// the new entity inherits the old one's place. That is the same named
/// consequence the draw-order tie-break has (ADR-0004's M3.11 amendment), and it
/// is reproducible, which is what the order is for.
///
/// # Cost
///
/// [`World::entity_ids`] allocates a vector of every live id and sorts it, so
/// every call pays that; the `find_map` over it then stops at the first camera.
/// A caller that wants both this and [`placements_of`] pays the enumeration
/// twice. They are kept apart because they answer different questions rather
/// than because it is cheaper.
///
/// **M3.32 ran the benchmark this paragraph used to say nobody had.** The frame
/// loop calls both once a frame, and at 50 000 sprites the extraction phase is
/// 3.76 ms of a 6.74 ms frame — the largest single item in the decomposition, on
/// a world of 50 002 entities enumerated and sorted twice.
/// `docs/perf/BASELINE.md` carries the table. It is a finding, not a fault that
/// was fixed here: the frame still fits inside its budget with room to spare,
/// and this is where a later optimisation task should look first.
#[must_use]
pub fn camera_of(world: &World) -> CameraView {
    world
        .entity_ids()
        .into_iter()
        .find_map(|entity| world.get::<Camera>(entity).ok().map(|camera| *camera))
        .map_or(CameraView::IDENTITY, |camera| {
            CameraView::new(camera.x, camera.y, camera.zoom)
        })
}

#[cfg(test)]
mod tests {
    use super::{camera_of, filter_of, placements_of, region_names_of, regions_of};
    use crate::hit::depth_order;
    use narvo_ecs::{Burst, Camera, EntityId, Layer, Sampling, Sprite, Tint, Transform, World};
    use narvo_render2d::{CameraView, SpriteFilter, SpritePlacement};

    /// Spawns `count` entities whose x is their spawn index, so the order the
    /// placements come out in is readable off the values.
    fn world_with_numbered_transforms(count: u32) -> World {
        let mut world = World::new();

        for index in 0..count {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "a small loop counter is exact in f32"
                        )]
                        x: index as f32,
                        ..Transform::IDENTITY
                    },
                )
                .expect("the entity was just spawned");
        }

        world
    }

    /// Four of the five fields cross verbatim; the rotation crosses as its pair.
    ///
    /// **The angle is the one field that is no longer copied** (M5b.4), and the
    /// assertion says what replaced the copy rather than dropping the claim:
    /// `rot_cos` and `rot_sin` have to be exactly what [`f32::sin_cos`] returns
    /// for the transform's own angle — the same call `narvo-render2d` made until
    /// this task moved it here, compared on bit patterns because that is what the
    /// rasteriser is entitled to act on.
    #[test]
    fn every_transform_becomes_a_placement_field_for_field() {
        let mut world = World::new();
        let entity = world.spawn();
        let transform = Transform {
            x: 1.5,
            y: -2.5,
            rotation: 0.75,
            scale_x: 3.0,
            scale_y: 4.0,
        };
        world.insert(entity, transform).expect("just spawned");

        let placements = placements_of(&world);

        assert_eq!(placements.len(), 1);
        let placement = placements[0].placement;
        assert_eq!(placement.x.to_bits(), transform.x.to_bits());
        assert_eq!(placement.y.to_bits(), transform.y.to_bits());
        assert_eq!(placement.scale_x.to_bits(), transform.scale_x.to_bits());
        assert_eq!(placement.scale_y.to_bits(), transform.scale_y.to_bits());

        let (sin, cos) = transform.rotation.sin_cos();
        assert_eq!(placement.rot_cos.to_bits(), cos.to_bits());
        assert_eq!(placement.rot_sin.to_bits(), sin.to_bits());
    }

    /// An unturned transform produces the pair every untouched literal now
    /// carries, **bit for bit**.
    ///
    /// This is the whole regression argument for the nine blessed references in
    /// one assertion. Every scene in this repository is drawn at `rotation: 0.0`,
    /// M5b.4 replaced that field with the constant pair `(1.0, 0.0)` at some
    /// forty call sites, and the corner arithmetic is unchanged — so no vertex
    /// can move *provided* `sin_cos(0.0)` really is `(0.0, 1.0)` exactly. That is
    /// a property of the standard library, not of this code, and it is the kind
    /// of thing CLAUDE.md says to check rather than to assume.
    #[test]
    fn the_unturned_pair_is_what_sin_cos_of_zero_returns() {
        let (sin, cos) = 0.0_f32.sin_cos();

        assert_eq!(
            (cos.to_bits(), sin.to_bits()),
            (
                SpritePlacement::UNTURNED.0.to_bits(),
                SpritePlacement::UNTURNED.1.to_bits()
            ),
            "sin_cos(0.0) returned {sin:?}, {cos:?}. Every blessed reference was \
             drawn through the old `rotation: 0.0` path and is compared against \
             the new constant pair; if the two are not the same bits, the \
             references moved for a reason no test names."
        );
    }

    /// The sampler wish reaches the sprite, and its absence means `Nearest`.
    ///
    /// GPU-free. The renderer honours both filters since M3.23, so a wish that
    /// the extraction drops is a picture that is wrong with nothing to say so.
    ///
    /// **Since M3.28 a golden test would catch it too**, and until M3.28 none
    /// would: `overlapping_world()` now carries `Sampling::linear()`, so dropping
    /// the wish here turns `layer_order_regions_128x128` back into its `Nearest`
    /// image and moves 768 of its 16 384 pixels. That does not make this test
    /// redundant — it is GPU-free, it names the defect instead of showing it, and
    /// it is the only place that pins the *absence* of a `Sampling` to `Nearest`.
    #[test]
    fn the_sampler_wish_reaches_the_sprite_and_its_absence_means_nearest() {
        let mut world = World::new();
        // Depth orders them: bare, then linear, then an unknown code.
        for (depth, sampling) in [
            (0.0_f32, None),
            (1.0, Some(Sampling::linear())),
            (2.0, Some(Sampling::nearest())),
            (3.0, Some(Sampling { filter: 200 })),
        ] {
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(depth, 0.0))
                .expect("the entity was just spawned");
            world
                .insert(entity, Layer::at(depth))
                .expect("the entity was just spawned");
            if let Some(sampling) = sampling {
                world
                    .insert(entity, sampling)
                    .expect("the entity was just spawned");
            }
        }

        let filters: Vec<SpriteFilter> = placements_of(&world)
            .iter()
            .map(|drawn| drawn.filter)
            .collect();

        assert_eq!(
            filters,
            vec![
                SpriteFilter::Nearest,
                SpriteFilter::Linear,
                SpriteFilter::Nearest,
                SpriteFilter::Nearest,
            ],
            concat!(
                "in order: no component means `Nearest`, the linear code means ",
                "`Linear`, the nearest code means `Nearest`, and an unrecognised ",
                "code means `Nearest` rather than an error"
            )
        );
    }

    /// The mapping is the one `Sampling`'s own table writes down.
    #[test]
    fn the_filter_mapping_is_the_documented_one() {
        assert_eq!(filter_of(Sampling::NEAREST), SpriteFilter::Nearest);
        assert_eq!(filter_of(Sampling::LINEAR), SpriteFilter::Linear);
        for reserved in [2_u8, 3, 17, u8::MAX] {
            assert_eq!(
                filter_of(reserved),
                SpriteFilter::Nearest,
                "code {reserved} is reserved and reads as `Nearest`"
            );
        }
    }

    /// The order is the canonical entity order, and it is the same twice.
    ///
    /// Not a tautology: the obvious implementation iterates a query, and query
    /// order is archetype order, which `narvo-ecs` documents as unstable. This
    /// is the assertion that says the obvious implementation was not used.
    #[test]
    fn placements_come_out_in_ascending_entity_order() {
        let world = world_with_numbered_transforms(8);

        let placements = placements_of(&world);
        let xs: Vec<f32> = placements.iter().map(|p| p.placement.x).collect();

        assert_eq!(xs, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(
            placements_of(&world),
            placements,
            "two calls on one world must produce the same order, or the image \
             drawn from them depends on something no test pins"
        );
    }

    /// An entity without a `Transform` contributes nothing and skips nothing.
    #[test]
    fn entities_without_a_transform_are_left_out_without_disturbing_the_order() {
        let mut world = world_with_numbered_transforms(4);
        let bare = world.spawn();
        assert!(!world.has::<Transform>(bare));

        let xs: Vec<f32> = placements_of(&world)
            .iter()
            .map(|p| p.placement.x)
            .collect();
        assert_eq!(xs, vec![0.0, 1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_world_with_no_transforms_yields_an_empty_batch() {
        let mut world = World::new();
        world.spawn();
        assert!(placements_of(&world).is_empty());
    }

    /// Nanoseconds of the fastest of `rounds` runs, after one unmeasured warm-up.
    ///
    /// Duplicated from `narvo-render2d`'s copy rather than shared: the two
    /// crates measure different functions and neither has a place to put a
    /// helper the other could see, and a measuring tool in a public API is worse
    /// than eight duplicated lines. Both refuse to report a measurement they did
    /// not take.
    fn best_of(rounds: usize, mut work: impl FnMut()) -> u128 {
        assert!(
            rounds > 0,
            "a measurement of zero rounds produces an empty sample series, and \
             every guard downstream of it would pass on no data"
        );

        work();

        let mut samples = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = std::time::Instant::now();
            work();
            samples.push(start.elapsed().as_nanos());
        }

        let best = samples
            .iter()
            .copied()
            .min()
            .expect("rounds is greater than zero, so there is at least one sample");

        assert!(
            best > 0,
            "the fastest of {rounds} rounds took zero nanoseconds, so the clock \
             could not resolve the work; raise the workload rather than trusting \
             the number"
        );

        best
    }

    /// A world of `count` entities, every one carrying a `Transform`.
    fn world_of(count: u32) -> World {
        let mut world = World::new();
        for index in 0..count {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "the value only has to vary between entities"
                        )]
                        x: (index % 64) as f32,
                        ..Transform::IDENTITY
                    },
                )
                .expect("the entity was just spawned");
        }
        world
    }

    /// A world with no camera is drawn exactly as it was before M3.12.
    #[test]
    fn a_world_without_a_camera_is_drawn_through_the_identity_view() {
        let world = world_with_numbered_transforms(3);

        assert_eq!(camera_of(&world), CameraView::IDENTITY);
        assert_eq!(
            camera_of(&World::new()),
            CameraView::IDENTITY,
            "an empty world too. This pins one property only — that a world with \
             no `Camera` is drawn through the pre-M3.12 projection. That \
             registering the type moves no dump and no hash is a separate claim \
             with its own evidence, in `narvo-ecs`'s \
             `registering_the_camera_type_changes_nothing_for_a_world_that_has_none`."
        );
    }

    /// The component's three fields reach the renderer's three scalars, in
    /// order, without a rounding step.
    #[test]
    fn every_camera_field_becomes_a_view_field_bit_for_bit() {
        let mut world = World::new();
        let entity = world.spawn();
        let camera = Camera::new(-12.5, 7.25, 1.5);
        world.insert(entity, camera).expect("just spawned");

        let view = camera_of(&world);
        assert_eq!(view.x.to_bits(), camera.x.to_bits());
        assert_eq!(view.y.to_bits(), camera.y.to_bits());
        assert_eq!(view.zoom.to_bits(), camera.zoom.to_bits());
    }

    /// **The lowest entity id owns the camera, and the second one is ignored.**
    ///
    /// Both halves matter. The first is what makes the choice reproducible: two
    /// worlds built the same way have to pick the same camera, and archetype
    /// order would not promise that. The second is the cost, asserted rather
    /// than left in prose — a scene with two cameras draws through one of them
    /// and says nothing.
    #[test]
    fn the_lowest_entity_id_owns_the_camera() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        assert!(first < second);

        world
            .insert(second, Camera::at(100.0, 100.0))
            .expect("just spawned");
        assert_eq!(
            camera_of(&world),
            CameraView::new(100.0, 100.0, 1.0),
            "the only camera in the world wins whatever its id"
        );

        world.insert(first, Camera::at(1.0, 2.0)).expect("exists");
        assert_eq!(
            camera_of(&world),
            CameraView::new(1.0, 2.0, 1.0),
            "with two cameras the lower id wins. If this ever came out as the \
             other one, the view would depend on insertion or archetype order and \
             two builds of the same world could frame different pictures."
        );
    }

    /// A camera is found whatever archetype its entity sits in.
    ///
    /// The same shape as `the_tie_break_is_the_same_whatever_the_archetypes_are`:
    /// giving one entity an extra component moves it to another archetype, which
    /// `narvo-ecs` documents as unstable iteration order. `camera_of` walks
    /// `entity_ids`, so it must not care.
    #[test]
    fn the_camera_is_found_whatever_the_archetypes_are() {
        /// A component that means nothing, present only to split archetypes.
        #[derive(Debug, Clone, Copy)]
        struct Extra;

        let mut plain = World::new();
        let mut mixed = World::new();
        for index in 0..4_u8 {
            for (world, extra) in [(&mut plain, false), (&mut mixed, index % 2 == 0)] {
                let entity = world.spawn();
                world
                    .insert(entity, Transform::at(f32::from(index), 0.0))
                    .expect("just spawned");
                if index == 2 {
                    world
                        .insert(entity, Camera::new(5.0, 6.0, 2.0))
                        .expect("exists");
                }
                if extra {
                    world.insert(entity, Extra).expect("just spawned");
                }
            }
        }

        assert_eq!(camera_of(&plain), camera_of(&mixed));
        assert_eq!(camera_of(&mixed), CameraView::new(5.0, 6.0, 2.0));
    }

    /// A world of one entity per entry, spawned in order.
    ///
    /// Each entity's `x` is its spawn index, so the draw order can be read off
    /// the returned placements as a list of indices. `None` means the entity
    /// gets no `Layer` at all, which is the case `Layer::DEFAULT` covers.
    fn world_with_depths(depths: &[Option<f32>]) -> World {
        let mut world = World::new();

        for (index, depth) in depths.iter().enumerate() {
            let marker =
                f32::from(u8::try_from(index).expect("these tests stay under 256 entities"));
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        x: marker,
                        ..Transform::IDENTITY
                    },
                )
                .expect("the entity was just spawned");

            if let Some(depth) = *depth {
                world
                    .insert(entity, Layer::at(depth))
                    .expect("the entity was just spawned");
            }
        }

        world
    }

    /// The spawn indices, in draw order.
    fn draw_order(world: &World) -> Vec<u8> {
        placements_of(world)
            .iter()
            .map(|placement| {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the marker was built from a u8 by `world_with_depths` and \
                              survives the extraction unchanged"
                )]
                let index = placement.placement.x as u8;
                index
            })
            .collect()
    }

    /// Smaller depth is drawn first, so it ends up behind.
    #[test]
    fn sprites_are_drawn_from_the_back_forward() {
        let world = world_with_depths(&[Some(2.0), Some(0.0), Some(1.0), Some(0.0)]);

        assert_eq!(
            draw_order(&world),
            vec![1, 3, 2, 0],
            "the two entities at depth 0 come first in spawn order, then depth 1, \
             then depth 2. Draw order is the order of this vector and the last \
             sprite wins where they overlap, so a larger depth has to come later."
        );
    }

    /// A world that mentions no layer comes out exactly as it did before M3.10.
    #[test]
    fn a_world_without_layers_stays_in_canonical_entity_order() {
        let world = world_with_depths(&[None, None, None, None, None]);

        assert_eq!(
            draw_order(&world),
            vec![0, 1, 2, 3, 4],
            "every entity is at Layer::DEFAULT, so every comparison falls to the \
             tie-break, which is ascending EntityId — the order this function \
             returned before it sorted anything"
        );
    }

    /// An entity without a `Layer` sits at zero, between the negative and the
    /// positive ones rather than at either end.
    #[test]
    fn an_entity_without_a_layer_is_drawn_at_the_default_depth() {
        let world = world_with_depths(&[Some(1.0), None, Some(-1.0)]);

        assert_eq!(
            draw_order(&world),
            vec![2, 1, 0],
            "the entity with no Layer must sort as if it carried Layer::DEFAULT \
             (0.0): behind the one at +1.0 and in front of the one at -1.0. \
             Treating it as \"first\" or \"last\" instead would make adding a Layer \
             to one entity move every other one."
        );
    }

    /// Equal depths keep the canonical order, whatever the archetypes are.
    ///
    /// The tie-break is the whole determinism argument, and this is the shape
    /// of world that would break it if the order came from a query instead:
    /// giving one entity an extra component puts it in a different archetype,
    /// which is exactly what `narvo-ecs` documents as unstable iteration
    /// order. The marker order has to be identical in both worlds.
    #[test]
    fn the_tie_break_is_the_same_whatever_the_archetypes_are() {
        /// A component that means nothing, present only to split archetypes.
        #[derive(Debug, Clone, Copy)]
        struct Extra;

        let plain = world_with_depths(&[Some(0.5); 4]);

        let mut mixed = World::new();
        for index in 0..4_u8 {
            let entity = mixed.spawn();
            mixed
                .insert(
                    entity,
                    Transform {
                        x: f32::from(index),
                        ..Transform::IDENTITY
                    },
                )
                .expect("the entity was just spawned");
            mixed
                .insert(entity, Layer::at(0.5))
                .expect("the entity was just spawned");
            if index % 2 == 0 {
                mixed
                    .insert(entity, Extra)
                    .expect("the entity was just spawned");
            }
        }

        assert_eq!(
            draw_order(&plain),
            draw_order(&mixed),
            "two worlds with the same depths and the same spawn order must draw in \
             the same order even when their entities sit in different archetypes. A \
             difference here means the order is coming from iteration rather than \
             from EntityId, and the image would depend on storage layout."
        );
        assert_eq!(draw_order(&mixed), vec![0, 1, 2, 3]);
    }

    /// `-0.0` is the same depth as `0.0`, so the id decides.
    #[test]
    fn negative_zero_is_the_same_depth_as_zero() {
        let world = world_with_depths(&[Some(-0.0), Some(0.0), Some(-0.0)]);

        assert_eq!(
            draw_order(&world),
            vec![0, 1, 2],
            "IEEE 754's totalOrder, which f32::total_cmp implements, puts -0.0 \
             strictly below +0.0. Content does not mean that, so `depth_order` maps \
             every zero to +0.0 and these three tie. Without it the order would be \
             2, 0, 1 — decided by a sign nobody wrote."
        );

        // The negative control: without the mapping the comparison really would
        // separate them, so the test above is checking something.
        assert_eq!((-0.0_f32).total_cmp(&0.0), core::cmp::Ordering::Less);
        assert_eq!(
            depth_order(-0.0).total_cmp(&depth_order(0.0)),
            core::cmp::Ordering::Equal
        );
    }

    /// A `NaN` depth lands at an end, deterministically, without panicking.
    #[test]
    fn a_nan_depth_sorts_to_an_end_rather_than_panicking() {
        let world = world_with_depths(&[
            Some(f32::NAN),
            Some(0.0),
            Some(-f32::NAN),
            Some(f32::INFINITY),
            Some(f32::NEG_INFINITY),
        ]);

        assert_eq!(
            draw_order(&world),
            vec![2, 4, 1, 3, 0],
            "f32::total_cmp is total over every bit pattern: a NaN with its sign \
             bit set orders below -inf, one without orders above +inf. So the order \
             is -NaN, -inf, 0.0, +inf, +NaN. That is defined and reproducible \
             rather than correct — a NaN depth is a bug in whatever produced it, and \
             this only fixes where two runs agree it lands."
        );
    }

    /// **The guarded regression class: extraction stays linear in the entity
    /// count.**
    ///
    /// A ratio rather than a duration, for the reason the twin guard in
    /// `narvo-render2d` gives: the runner's speed appears in both measurements
    /// and cancels, so there is no threshold to raise after a noisy build.
    ///
    /// The class this catches by name is a step from linear to superlinear —
    /// a lookup that walks the world per entity, a sort inside the loop, a
    /// quadratic dedup. It does **not** catch a constant factor; the allocation
    /// structure below is what speaks to that.
    #[test]
    fn extracting_placements_stays_linear_in_the_entity_count() {
        const SMALL: u32 = 2_000;
        const LARGE: u32 = 20_000;
        const ROUNDS: usize = 7;
        /// Linear is 10, quadratic would be 100. Forty for the reason the twin
        /// guard in `narvo-render2d` records: seven runs on the reference
        /// machine gave ratios from 16 to 21, the spread coming from how well
        /// the smaller world stays in cache, and a bound close to the
        /// observations is a bound that flakes.
        const BOUND: u128 = 40;

        let small = world_of(SMALL);
        let large = world_of(LARGE);

        let small_ns = best_of(ROUNDS, || {
            std::hint::black_box(placements_of(std::hint::black_box(&small)));
        });
        let large_ns = best_of(ROUNDS, || {
            std::hint::black_box(placements_of(std::hint::black_box(&large)));
        });

        let ratio = large_ns / small_ns;
        println!(
            "placements_of: {SMALL} -> {small_ns} ns, {LARGE} -> {large_ns} ns, \
             ratio {ratio} (linear is {})",
            LARGE / SMALL
        );

        assert!(
            ratio < BOUND,
            "placements_of took {large_ns} ns for {LARGE} entities against \
             {small_ns} ns for {SMALL}, a ratio of {ratio}. Ten times the entities \
             is ten times the work when extraction is linear; {BOUND} is the bound \
             and a quadratic step would land near 100. The runner's speed is in \
             both numbers and cancels, so this is a change of shape rather than a \
             slow machine."
        );
    }

    /// What the sort inside `World::entity_ids` costs, as a share of extraction.
    ///
    /// # How it is isolated, and why this way
    ///
    /// `entity_ids` is not instrumented and not changed — a measurement that
    /// rebuilds its subject measures something else. Instead the sort's *input*
    /// is reconstructed: a query yields entities in archetype order, which is
    /// the order `entity_ids` collects before it sorts, and sorting a clone of
    /// that vector is the same work on the same data.
    ///
    /// What that makes this: a measurement of an equivalent sort, not of the
    /// call inside `entity_ids`. The two differ in nothing this timing can see,
    /// but they are not the same instruction stream, and the report says so.
    ///
    /// Both orders are timed. The archetype order is what actually happens; the
    /// already-ascending order is the best case a pattern-defeating sort can
    /// have, and printing both says how much of the result is the sort being
    /// handed easy input rather than the sort being cheap.
    #[test]
    fn the_share_the_sort_takes_is_recorded() {
        const ROUNDS: usize = 25;

        println!(
            "entities | entity_ids ns | sort of archetype order ns | sort of sorted order ns | \
             placements_of ns"
        );
        for count in [10_000_u32, 50_000] {
            let world = world_of(count);

            // Archetype order, observed rather than assumed: this is what
            // `entity_ids` iterates before sorting.
            let archetype: Vec<EntityId> = {
                let mut query = world.query::<&Transform>();
                query.iter().map(|(id, _)| id).collect()
            };
            let ascending = {
                let mut sorted = archetype.clone();
                sorted.sort_unstable();
                sorted
            };
            let already_ascending = archetype == ascending;

            let ids_ns = best_of(ROUNDS, || {
                std::hint::black_box(std::hint::black_box(&world).entity_ids());
            });
            let sort_archetype_ns = best_of(ROUNDS, || {
                let mut copy = archetype.clone();
                copy.sort_unstable();
                std::hint::black_box(copy);
            });
            let sort_ascending_ns = best_of(ROUNDS, || {
                let mut copy = ascending.clone();
                copy.sort_unstable();
                std::hint::black_box(copy);
            });
            let extract_ns = best_of(ROUNDS, || {
                std::hint::black_box(placements_of(std::hint::black_box(&world)));
            });

            println!(
                "{count:8} | {ids_ns:13} | {sort_archetype_ns:26} | {sort_ascending_ns:23} | \
                 {extract_ns:16}  (archetype order already ascending: {already_ascending})"
            );

            assert!(
                ids_ns > 0 && sort_archetype_ns > 0 && extract_ns > 0,
                "a measurement of zero is not one"
            );
            assert_eq!(
                archetype.len(),
                count as usize,
                "the reconstructed input must hold every entity, or the sort being \
                 timed is not the sort that happens"
            );
        }
    }

    /// Records what the extraction costs and how it allocates.
    ///
    /// Not a gate. The capacity figure is here because it is the one number that
    /// says how the returned vector was built: `collect` from a `filter_map`
    /// gets a size hint whose lower bound is zero, so it grows by doubling
    /// instead of asking once. Recorded rather than changed — this task measures.
    #[test]
    fn the_cost_of_extracting_placements_is_recorded() {
        const ROUNDS: usize = 25;

        println!("entities | placements_of ns | ns per entity | vec len | vec capacity");
        for count in [100_u32, 1_000, 10_000, 50_000] {
            let world = world_of(count);

            let extract_ns = best_of(ROUNDS, || {
                std::hint::black_box(placements_of(std::hint::black_box(&world)));
            });

            let placements = placements_of(&world);
            #[expect(
                clippy::cast_precision_loss,
                reason = "a nanosecond count divided by an entity count needs two \
                          significant digits, not sixteen"
            )]
            let per_entity = extract_ns as f64 / f64::from(count);

            println!(
                "{count:8} | {extract_ns:16} | {per_entity:13.1} | {:7} | {:12}",
                placements.len(),
                placements.capacity()
            );

            assert!(extract_ns > 0, "a measurement of zero is not one");
            assert_eq!(placements.len() as u32, count);
        }
    }

    /// The two extractions agree on order, which is what makes duplicating the
    /// sort safe.
    ///
    /// `regions_of` repeats `placements_of`'s sort rather than sharing it,
    /// because sharing would mean editing `placements_of`, and a blessed
    /// reference is drawn through that function — **one**, counted from the call
    /// sites in M6b.1; this sentence said "six" until then, and [`DrawnRegion`]
    /// carries the count. This is the price of the
    /// duplication paid in the only currency that matters: a world where every
    /// entity carries a `Sprite` must come out of both in the same order, with
    /// the same placements and the same filters.
    ///
    /// The depths are deliberately awkward — a tie, a negative, a zero written
    /// as `-0.0` — because those are the three cases `depth_order` exists for,
    /// and a sort that agreed on well-separated values would prove nothing about
    /// them.
    #[test]
    fn the_two_extractions_agree_on_order_when_every_entity_has_a_sprite() {
        let mut world = World::new();
        for (index, depth) in [3.0_f32, -1.0, 0.0, -0.0, 3.0, -1.0].iter().enumerate() {
            let entity = world.spawn();
            world
                .insert(
                    entity,
                    Transform {
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "a small loop counter is exact in f32"
                        )]
                        x: index as f32,
                        ..Transform::IDENTITY
                    },
                )
                .expect("just spawned");
            world
                .insert(entity, Layer::at(*depth))
                .expect("just spawned");
            world
                .insert(entity, Sprite::new(format!("region{index}")))
                .expect("just spawned");
            if index % 2 == 0 {
                world
                    .insert(entity, Sampling::linear())
                    .expect("just spawned");
            }
        }

        let plain = placements_of(&world);
        let named = regions_of(&world);

        assert_eq!(plain.len(), named.len(), "the two saw different entities");
        for (index, (plain, named)) in plain.iter().zip(&named).enumerate() {
            assert_eq!(
                plain.placement, named.placement,
                "position {index} of the two extractions is a different entity"
            );
            assert_eq!(plain.filter, named.filter, "position {index}");
        }
    }

    /// An entity with a transform and no sprite is in one and not the other.
    #[test]
    fn only_the_named_extraction_requires_a_sprite() {
        let mut world = world_with_numbered_transforms(3);
        let ids = world.entity_ids();
        world
            .insert(ids[1], Sprite::new("only-this-one"))
            .expect("the entity is alive");

        assert_eq!(placements_of(&world).len(), 3);

        let named = regions_of(&world);
        assert_eq!(named.len(), 1, "two entities never asked to be drawn");
        assert_eq!(named[0].region, "only-this-one");
    }

    /// A sprite with no transform is nowhere, which is the same rule
    /// `placements_of` has ever had: a thing with no position cannot be drawn.
    #[test]
    fn a_sprite_without_a_transform_is_not_drawn() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Sprite::new("homeless"))
            .expect("just spawned");

        assert!(regions_of(&world).is_empty());
        assert!(placements_of(&world).is_empty());
    }

    /// The names a world asks for come out once each, in name order.
    #[test]
    fn the_names_a_world_wants_are_deduplicated_and_sorted() {
        let mut world = World::new();
        for name in ["coin", "hero", "coin"] {
            let entity = world.spawn();
            world
                .insert(entity, Sprite::new(name))
                .expect("just spawned");
        }

        let names: Vec<String> = region_names_of(&world).into_iter().collect();
        assert_eq!(names, vec!["coin".to_owned(), "hero".to_owned()]);
    }

    /// A name is collected even from an entity that will never be drawn.
    ///
    /// Deliberate: the check that every named region exists is about the
    /// *content* being coherent, not about what happens to be visible. A sprite
    /// with no transform still names a region, and a typo there should be
    /// reported rather than hidden by the entity being invisible anyway.
    #[test]
    fn a_name_counts_even_when_the_entity_cannot_be_drawn() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Sprite::new("never-drawn"))
            .expect("just spawned");

        assert!(regions_of(&world).is_empty());
        assert!(region_names_of(&world).contains("never-drawn"));
    }

    // ------------------------------------------------------------- bursts

    /// An emitter at `(x, y)` showing `region`, with `burst` on it.
    fn emitter_at(world: &mut World, x: f32, y: f32, depth: f32, burst: Burst) -> EntityId {
        let entity = world.spawn();
        world
            .insert(
                entity,
                Transform {
                    x,
                    y,
                    ..Transform::IDENTITY
                },
            )
            .expect("just spawned");
        world
            .insert(entity, Layer::at(depth))
            .expect("just spawned");
        world
            .insert(
                entity,
                Sprite {
                    region: "spark".to_owned(),
                },
            )
            .expect("just spawned");
        world.insert(entity, burst).expect("just spawned");
        entity
    }

    /// A burst of eight particles that lasts ten ticks, armed.
    fn a_burst() -> Burst {
        Burst::new(8, 0x5eed_1234_abcd_9876, 10, 2.0, 1.0)
    }

    /// **The construction claim, measured rather than asserted in prose.**
    ///
    /// The sort key gained a third component in M6b.7. A world whose bursts are
    /// all spent has one entry per entity, slot `0`, so the key degenerates to
    /// the *(depth, entity)* it always was — and the vector has to be the vector
    /// the same world without any `Burst` at all produces. That is what keeps
    /// `physics_drop_tick45_128x128`, the blessed reference drawn through this
    /// function, where it is.
    #[test]
    fn a_world_without_a_burst_draws_exactly_what_it_drew() {
        let build = |burst: Option<Burst>| {
            let mut world = World::new();
            for index in 0..4 {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a small loop counter is exact in f32"
                )]
                let offset = index as f32;
                let entity = world.spawn();
                world
                    .insert(
                        entity,
                        Transform {
                            x: offset,
                            y: -offset,
                            ..Transform::IDENTITY
                        },
                    )
                    .expect("just spawned");
                world
                    .insert(entity, Layer::at(offset * 0.5))
                    .expect("just spawned");
                world
                    .insert(
                        entity,
                        Sprite {
                            region: format!("region-{index}"),
                        },
                    )
                    .expect("just spawned");
                if let Some(burst) = burst {
                    world.insert(entity, burst).expect("just spawned");
                }
            }
            regions_of(&world)
        };

        let spent = Burst {
            age: 10,
            ..a_burst()
        };
        assert!(spent.spent(), "the probe has to be a burst that is over");

        assert_eq!(
            build(None),
            build(Some(spent)),
            "a spent burst changed what a world draws"
        );
    }

    #[test]
    fn a_live_burst_draws_the_emitter_and_one_copy_per_particle() {
        let mut world = World::new();
        emitter_at(
            &mut world,
            0.0,
            0.0,
            0.0,
            Burst {
                age: 2,
                ..a_burst()
            },
        );

        let drawn = regions_of(&world);

        assert_eq!(
            drawn.len(),
            1 + a_burst().count as usize,
            "an emitter draws itself and one copy per particle"
        );
        // Every copy shows the emitter's own region: a burst is the entity's
        // sprite, repeated and scattered, and carries no region of its own.
        assert!(drawn.iter().all(|one| one.region == "spark"));
    }

    #[test]
    fn a_particle_sits_at_the_emitter_plus_its_own_offset() {
        let burst = Burst {
            age: 3,
            ..a_burst()
        };
        let mut world = World::new();
        emitter_at(&mut world, 10.0, -20.0, 0.0, burst);

        let drawn = regions_of(&world);
        let emitter = drawn[0].placement;
        assert_eq!(emitter.x.to_bits(), 10.0_f32.to_bits());
        assert_eq!(emitter.y.to_bits(), (-20.0_f32).to_bits());

        for index in 0..burst.count {
            let (offset_x, offset_y, _) = burst.particle(index).expect("a live burst");
            let placed = drawn[index as usize + 1].placement;
            assert_eq!(
                placed.x.to_bits(),
                (10.0_f32 + offset_x).to_bits(),
                "particle {index} x"
            );
            assert_eq!(
                placed.y.to_bits(),
                (-20.0_f32 + offset_y).to_bits(),
                "particle {index} y"
            );
        }
    }

    /// A particle inherits the emitter's tint with the fade multiplied into the
    /// alpha alone — the colour channels are the content's and stay untouched.
    #[test]
    fn a_particle_carries_the_emitters_colour_dimmed_by_its_fade() {
        let burst = Burst {
            age: 5,
            ..a_burst()
        };
        let mut world = World::new();
        let entity = emitter_at(&mut world, 0.0, 0.0, 0.0, burst);
        let tint = Tint {
            red: 0.25,
            green: 0.5,
            blue: 0.75,
            alpha: 0.8,
        };
        world.insert(entity, tint).expect("alive");

        let drawn = regions_of(&world);
        let (_, _, fade) = burst.particle(0).expect("a live burst");

        assert_eq!(drawn[1].tint.red.to_bits(), tint.red.to_bits());
        assert_eq!(drawn[1].tint.green.to_bits(), tint.green.to_bits());
        assert_eq!(drawn[1].tint.blue.to_bits(), tint.blue.to_bits());
        assert_eq!(drawn[1].tint.alpha.to_bits(), (tint.alpha * fade).to_bits());
        // And the emitter's own sprite keeps the undimmed alpha.
        assert_eq!(drawn[0].tint.alpha.to_bits(), tint.alpha.to_bits());
    }

    /// Particles obey the depth order rather than sitting on top of everything.
    ///
    /// The reason the burst is folded into this function instead of being a
    /// second list somebody concatenates: a separate list draws last, which is a
    /// new ordering rule. Here a burst behind a wall is behind the wall.
    #[test]
    fn a_burst_at_a_lower_depth_draws_behind_a_sprite_above_it() {
        let mut world = World::new();
        emitter_at(
            &mut world,
            0.0,
            0.0,
            0.0,
            Burst {
                age: 1,
                ..a_burst()
            },
        );

        let wall = world.spawn();
        world.insert(wall, Transform::IDENTITY).expect("spawned");
        world.insert(wall, Layer::at(5.0)).expect("spawned");
        world
            .insert(
                wall,
                Sprite {
                    region: "wall".to_owned(),
                },
            )
            .expect("spawned");

        let drawn = regions_of(&world);

        assert_eq!(
            drawn.last().expect("something is drawn").region,
            "wall",
            "the burst drew over a sprite in front of it"
        );
        assert_eq!(drawn.len(), 2 + a_burst().count as usize);
    }

    /// The particles of one emitter stay together and in index order.
    #[test]
    fn two_emitters_keep_their_particles_and_their_order() {
        let mut world = World::new();
        let first = emitter_at(
            &mut world,
            0.0,
            0.0,
            1.0,
            Burst {
                age: 2,
                ..a_burst()
            },
        );
        let second = emitter_at(
            &mut world,
            0.0,
            0.0,
            1.0,
            Burst {
                age: 2,
                ..a_burst()
            },
        );
        assert!(first < second, "the second emitter has the higher id");

        let drawn = regions_of(&world);
        let particles = a_burst().count as usize;
        assert_eq!(drawn.len(), 2 * (1 + particles));

        // The block boundary is where the second emitter's own sprite sits, and
        // its offset is zero — every particle before it belongs to the first.
        let block = 1 + particles;
        assert_eq!(drawn[0].placement.x.to_bits(), 0.0_f32.to_bits());
        assert_eq!(drawn[block].placement.x.to_bits(), 0.0_f32.to_bits());
    }

    /// A burst on an entity with no sprite draws nothing.
    ///
    /// The M4.8 rule, unchanged: what a thing looks like is content, and an
    /// entity that says nothing about its appearance has not asked to be drawn.
    /// A burst does not make that decision for it.
    #[test]
    fn a_burst_without_a_sprite_is_not_drawn() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Transform::IDENTITY).expect("spawned");
        world
            .insert(
                entity,
                Burst {
                    age: 1,
                    ..a_burst()
                },
            )
            .expect("spawned");

        assert!(regions_of(&world).is_empty());
    }

    /// `placements_of` is untouched and sees no particle.
    ///
    /// The M4.8 constraint restated for this task: the function every whole-texture
    /// frame is drawn through does not learn about bursts, so the arm of
    /// `SceneHost::extract` that draws through it emits what it always did.
    #[test]
    fn placements_of_does_not_see_a_burst() {
        let mut world = World::new();
        emitter_at(
            &mut world,
            0.0,
            0.0,
            0.0,
            Burst {
                age: 1,
                ..a_burst()
            },
        );

        assert_eq!(
            placements_of(&world).len(),
            1,
            "placements_of grew a particle"
        );
    }
}
