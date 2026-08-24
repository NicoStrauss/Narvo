//! The seam between the world's [`RigidBody`] components and the physics facade.
//!
//! `narvo-ecs` owns the component, which is bare scalars; `narvo-physics2d`
//! owns `Body`, which is bare scalars plus two enums; and neither knows the
//! other exists. This module is the one place that knows both, which is
//! ADR-0015's arrangement — a subsystem takes scalars rather than a `World`, and
//! the translation lives in the crate that already sees both sides. ADR-0029
//! names this crate for exactly this mapping.
//!
//! It is the third seam of that shape in this file's neighbourhood, not a new
//! kind of thing: `sprite_batch::placements_of` extracts what the renderer
//! draws, `audio::cues_of` extracts what the sink plays, and this extracts what
//! the solver steps. M5.6c booked the audio one as "ADR-0015 applied, no new
//! structural decision", and the same sentence covers this.
//!
//! # Order is part of the result, not a detail
//!
//! Bodies go to rapier as a slice, and the order of that slice reaches its
//! internal ordering — so two runs that extract in two different orders can
//! compute two different worlds. Every extraction here walks
//! [`World::entity_ids`], which sorts, for the reason `sim::chance` gives about
//! its own director: a query iterates in archetype order, archetype order is not
//! reproducible, and "there is only one of them" is not a property to lean on.
//!
//! # What comes back, and what does not
//!
//! [`Physics2d::step`] writes seven of `Body`'s twelve fields — position, the
//! rotation pair, both velocities and the angular velocity. The other five —
//! kind, shape and the three material scalars — are inputs to every rebuild and
//! are never touched by a step. [`write_back`] writes exactly the seven, so a
//! reader of this module can see which half of the component the solver owns
//! without having to know rapier.

use narvo_core::FixedTimestep;
use narvo_ecs::{EcsError, EntityId, RigidBody, SystemContext, World};
use narvo_physics2d::{Body, BodyKind, Physics2d, Shape};

/// Downward acceleration, world units per second squared.
///
/// Earth's, to one decimal, which is what `narvo-physics2d`'s own tests use.
/// Nothing depends on the value being right; it depends on it being fixed.
///
/// **It is engine-wide and not content**, which is a limit rather than a design:
/// a scene cannot ask for another gravity, because saying so would need a
/// component and a twelfth registered type is a decision of its own. M5b.4
/// names it rather than smuggling a `Gravity` component in beside a golden
/// image.
pub const GRAVITY: f32 = -9.81;

/// The tick length the solver is stepped with, in seconds.
///
/// `narvo-core`'s own, so ADR-0003's truncation to whole nanoseconds is the same
/// one the rest of the engine runs on: 60 Hz is 16 666 666 ns, not a sixtieth of
/// a second. Recomputed per tick rather than stored, which costs a division and
/// keeps the number in one place.
fn step_seconds() -> f32 {
    FixedTimestep::new(FixedTimestep::DEFAULT_TICK_RATE_HZ)
        .step()
        .as_secs_f32()
}

/// A [`System`](narvo_ecs::System) that advances every rigid body by one tick.
///
/// **One system, two modes**, and that is the point rather than a convenience.
/// `sim::physics` — the demo mode the cross-platform matrix records — and
/// `sim::scene_file` — the mode M5b.4 gave a body to — run this same function, so
/// what the matrix measures on two runners is the path a scene file takes. A
/// second wiring in the scene-file mode would have been four lines and a
/// constant, and the constant is exactly what would have drifted.
///
/// It lived in `sim::physics` until M5b.4 and moved here with its two constants,
/// because this is already the module that owns the seam between the components
/// and the facade (ADR-0015, ADR-0029). Moving it changed no value: the demo mode
/// constructs the same [`Physics2d`] from the same two numbers it always did,
/// which the determinism matrix is the instrument for.
///
/// # Panics
///
/// Panics if an entity that carried a body when it was extracted is gone before
/// its result is written back. Nothing in either mode despawns anything, so the
/// panic is unreachable; a [`System`](narvo_ecs::System) returns nothing, so
/// there is nowhere else for the error to go.
pub fn simulate(world: &mut World, _context: &SystemContext) {
    let physics = Physics2d::new(0.0, GRAVITY, step_seconds());

    step(world, &physics)
        .expect("neither mode despawns a body mid-tick, so every extracted entity survives");
}

/// The `Body` a component describes.
///
/// The two conversions that are not field copies:
///
/// - `kind` is a code, and anything the table does not name is
///   [`RigidBody::FIXED`]. That reading is the component's own contract, and it
///   is applied *here* rather than in `narvo-ecs` for the reason `Sampling`
///   gives: a component is storage, and the crate that sees both sides is where
///   an unknown code becomes a decision.
/// - the shape is always a rectangle, because a rectangle is the only shape
///   `RigidBody` can describe. `Shape::Ball` exists in the facade and is not
///   reachable from a component; `RigidBody`'s own documentation says why, and
///   what adding it later costs.
#[must_use]
pub fn body_of(component: &RigidBody) -> Body {
    Body {
        kind: if component.kind == RigidBody::DYNAMIC {
            BodyKind::Dynamic
        } else {
            BodyKind::Fixed
        },
        shape: Shape::Cuboid {
            half_x: component.half_x,
            half_y: component.half_y,
        },
        x: component.x,
        y: component.y,
        rot_cos: component.rot_cos,
        rot_sin: component.rot_sin,
        vel_x: component.vel_x,
        vel_y: component.vel_y,
        angvel: component.angvel,
        restitution: component.restitution,
        friction: component.friction,
        density: component.density,
    }
}

/// Every rigid body in `world`, with the entity it belongs to, in stable order.
///
/// The order is [`World::entity_ids`]'s, which is sorted. See the module
/// documentation on why that is load-bearing rather than tidy.
#[must_use]
pub fn bodies_of(world: &World) -> Vec<(EntityId, Body)> {
    world
        .entity_ids()
        .into_iter()
        .filter_map(|entity| {
            let component = world.get::<RigidBody>(entity).ok()?;
            Some((entity, body_of(&component)))
        })
        .collect()
}

/// Writes a stepped body's seven solver-owned scalars back onto its entity.
///
/// The five it leaves alone — kind, both extents and the three material
/// scalars — are inputs the solver never writes. Copying them back would work
/// today because a step does not change them, and it would quietly start
/// meaning something else the day one of them did.
///
/// # Errors
///
/// [`EcsError`] if `entity` is no longer alive, which is what
/// [`World::get`](narvo_ecs::World::get) and the insert below raise.
pub fn write_back(world: &mut World, entity: EntityId, body: &Body) -> Result<(), EcsError> {
    let updated = {
        let component = world.get::<RigidBody>(entity)?;
        RigidBody {
            x: body.x,
            y: body.y,
            rot_cos: body.rot_cos,
            rot_sin: body.rot_sin,
            vel_x: body.vel_x,
            vel_y: body.vel_y,
            angvel: body.angvel,
            ..*component
        }
    };

    world.insert(entity, updated)
}

/// Advances every rigid body in `world` by one tick.
///
/// Extract, step, write back — and nothing is kept between calls, on either side
/// of the seam. The facade rebuilds its rapier world from the slice every time
/// (ADR-0029) and this module holds no state at all, so the components are the
/// whole of the simulation's physics.
///
/// A world with no rigid bodies in it is stepped as a world with none: the
/// facade is not called, and nothing is written. That is not an optimisation but
/// the honest reading — there is no such thing as a step of nothing.
///
/// # Errors
///
/// [`EcsError`] if an entity that carried a body when it was extracted is gone
/// by the time its result is written back. Nothing in a tick removes entities
/// between those two points today, so this is the signature being honest rather
/// than a condition that arises.
pub fn step(world: &mut World, physics: &Physics2d) -> Result<(), EcsError> {
    let extracted = bodies_of(world);
    if extracted.is_empty() {
        return Ok(());
    }

    let (entities, mut bodies): (Vec<EntityId>, Vec<Body>) = extracted.into_iter().unzip();
    physics.step(&mut bodies);

    for (entity, body) in entities.into_iter().zip(&bodies) {
        write_back(world, entity, body)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bodies_of, body_of, step};
    use narvo_ecs::{ComponentRegistry, EntityId, RigidBody, World, canonical_dump};
    use narvo_physics2d::{Body, BodyKind, Physics2d, Shape};

    /// `narvo-core`'s 60 Hz step, the way ADR-0003 says it comes out:
    /// 16 666 666 ns, not a sixtieth of a second.
    fn engine_dt() -> f32 {
        narvo_core::FixedTimestep::new(narvo_core::FixedTimestep::DEFAULT_TICK_RATE_HZ)
            .step()
            .as_secs_f32()
    }

    fn physics() -> Physics2d {
        Physics2d::new(0.0, -9.81, engine_dt())
    }

    /// Ticks the scene is run for, and the points it is compared at.
    ///
    /// The checkpoints are the M2.3 shape — a comparison that only looks at the
    /// end says *that* two runs match and never *where* they stopped matching —
    /// and they are placed to straddle the first contact. `the_scene_resolves_contacts`
    /// below is what keeps that claim honest rather than assumed.
    const TICKS: usize = 150;
    const CHECKPOINTS: [usize; 4] = [1, 40, 90, 150];

    /// A ground and six boxes that fall onto it and onto each other.
    ///
    /// The start values are deliberately not exactly representable in binary,
    /// which is ADR-0013's reasoning about `motion`'s `0.017`: a scene of halves
    /// and quarters would agree across platforms by construction and would
    /// demonstrate nothing.
    fn scene(world: &mut World) -> Vec<EntityId> {
        let mut entities = Vec::new();

        let ground = world.spawn();
        world
            .insert(
                ground,
                RigidBody::fixed(20.0, 0.5)
                    .at(0.0, -0.5)
                    .with_material(0.0, 0.73, 1.0),
            )
            .expect("just spawned");
        entities.push(ground);

        for i in 0..6 {
            let entity = world.spawn();
            let offset = i as f32;
            world
                .insert(
                    entity,
                    RigidBody::dynamic(0.41, 0.29)
                        .at(-1.9 + offset * 0.73, 0.9 + offset * 1.1)
                        .with_velocity((offset - 2.0) * 0.31, -0.17)
                        .with_angvel((offset - 3.0) * 0.19)
                        .with_material(0.42, 0.73, 1.3),
                )
                .expect("just spawned");
            entities.push(entity);
        }

        entities
    }

    fn world_with_scene() -> (World, Vec<EntityId>) {
        let mut world = World::new();
        let entities = scene(&mut world);
        (world, entities)
    }

    /// The bit patterns of every scalar a step writes.
    ///
    /// `==` on `f32` calls `0.0` and `-0.0` equal and two NaNs unequal, so it
    /// cannot see what a hash sees — ADR-0014's round-trip consequence. This
    /// can.
    fn bits(world: &World, entities: &[EntityId]) -> Vec<[u32; 7]> {
        entities
            .iter()
            .map(|entity| {
                let b = world.get::<RigidBody>(*entity).expect("carries a body");
                [
                    b.x.to_bits(),
                    b.y.to_bits(),
                    b.rot_cos.to_bits(),
                    b.rot_sin.to_bits(),
                    b.vel_x.to_bits(),
                    b.vel_y.to_bits(),
                    b.angvel.to_bits(),
                ]
            })
            .collect()
    }

    fn run_to(ticks: usize) -> (World, Vec<EntityId>) {
        let (mut world, entities) = world_with_scene();
        let physics = physics();
        for _ in 0..ticks {
            step(&mut world, &physics).expect("nothing removes an entity mid-tick");
        }
        (world, entities)
    }

    /// A registry holding nothing but the type under test.
    fn registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<RigidBody>("rigidbody")
            .expect("a fresh registry");
        registry
    }

    /// Rebuilds a world from nothing but what the registry can see of it.
    ///
    /// Every component goes out through `serialize` and comes back through
    /// `deserialize` into a world spawned fresh — the same two calls a save file
    /// or a scene reload would make. Anything the mapping keeps outside a
    /// component does not survive this, which is the point.
    fn reconstitute(world: &World, entities: &[EntityId]) -> (World, Vec<EntityId>) {
        let registry = registry();
        let info = registry.get("rigidbody").expect("just registered");

        let mut rebuilt = World::new();
        let mut rebuilt_entities = Vec::new();
        for entity in entities {
            let text = info
                .serialize(world, *entity)
                .expect("the entity is alive")
                .expect("it carries a body");
            let fresh = rebuilt.spawn();
            info.deserialize(&mut rebuilt, fresh, &text)
                .expect("what the registry wrote, the registry reads");
            rebuilt_entities.push(fresh);
        }

        (rebuilt, rebuilt_entities)
    }

    #[test]
    fn two_runs_agree_at_every_checkpoint() {
        let physics = physics();
        let (mut left, left_ids) = world_with_scene();
        let (mut right, right_ids) = world_with_scene();

        for tick in 1..=TICKS {
            step(&mut left, &physics).expect("alive");
            step(&mut right, &physics).expect("alive");
            if CHECKPOINTS.contains(&tick) {
                assert_eq!(
                    bits(&left, &left_ids),
                    bits(&right, &right_ids),
                    "two runs of one scene diverged at tick {tick}"
                );
            }
        }
    }

    /// **The test this task exists to make true**, and it is ADR-0029's property
    /// one level up.
    ///
    /// The facade's own version asserts that a `Body` slice carries everything
    /// across a tick boundary. This asserts the same about the *components*: a
    /// world taken apart into RON and built again from that text alone continues
    /// exactly as the uninterrupted one would have. Between those two points the
    /// simulation exists as nothing but registered component text, which is the
    /// state a save file (§6/M7) or a hot reload (ADR-0022) resumes from.
    ///
    /// The cuts straddle the first contact deliberately. M5b.2 measured that a
    /// rebuilt and a retained world are identical through free fall and part
    /// company at the first tick that resolves one, so a test that only cut
    /// during the fall would be green about the easy half.
    #[test]
    fn a_world_reconstituted_from_its_components_continues_identically() {
        let physics = physics();
        let (uninterrupted, uninterrupted_ids) = run_to(TICKS);
        let expected = bits(&uninterrupted, &uninterrupted_ids);

        for cut in [1, 40, 90, 149] {
            let (world, entities) = run_to(cut);
            let (mut resumed, resumed_ids) = reconstitute(&world, &entities);
            for _ in cut..TICKS {
                step(&mut resumed, &physics).expect("alive");
            }

            assert_eq!(
                expected,
                bits(&resumed, &resumed_ids),
                "a world reconstituted at tick {cut} did not continue like the \
                 uninterrupted one - something the mapping needs is living \
                 outside a registered component, which is what ADR-0029 forbids"
            );
        }
    }

    /// The scene actually resolves contacts, or the test above is weaker than it
    /// reads.
    ///
    /// Free fall is the region where M5b.2 found a rebuilt and a retained world
    /// identical, so a scene that never landed would exercise the easy half and
    /// stay green. This is the guard against that, and it is why the cuts are
    /// where they are: a body must be resting on the ground well before the last
    /// cut.
    #[test]
    fn the_scene_resolves_contacts_before_the_last_checkpoint() {
        let (world, entities) = run_to(CHECKPOINTS[1]);

        let resting = entities
            .iter()
            .filter(|entity| {
                let b = world.get::<RigidBody>(**entity).expect("carries a body");
                b.kind == RigidBody::DYNAMIC && b.y < 1.0 && b.vel_y > -1.0
            })
            .count();

        assert!(
            resting >= 1,
            "no dynamic body has landed by tick {}, so the reconstitution test \
             above never crosses a contact and measures free fall only",
            CHECKPOINTS[1]
        );
    }

    /// The scene turns, so the rotation pair is covered rather than assumed.
    ///
    /// `bits` compares `rot_cos` and `rot_sin` along with the rest, but it can
    /// only exercise them if the boxes rotate. A scene that degenerated into
    /// pure translation would leave the tests above green while two of the seven
    /// fields had quietly stopped being checked.
    #[test]
    fn the_scene_rotates_so_the_pair_is_covered() {
        let (world, entities) = run_to(30);

        let turned = entities
            .iter()
            .filter(|entity| {
                let b = world.get::<RigidBody>(**entity).expect("carries a body");
                b.kind == RigidBody::DYNAMIC && b.rot_sin.to_bits() != 0.0f32.to_bits()
            })
            .count();

        assert!(
            turned >= 3,
            "only {turned} dynamic bodies have turned by tick 30, so `rot_cos` \
             and `rot_sin` are being compared without being moved"
        );
    }

    /// **The guard that a dropped write-back is caught at all**, and it exists
    /// because the obvious candidate does not catch one.
    ///
    /// M5b.3a injected "`write_back` does not write `angvel`" and every test in
    /// this module stayed green, including
    /// `a_world_reconstituted_from_its_components_continues_identically`. The
    /// reason is worth writing down, because it bounds what that test means: a
    /// dropped field is still *present* in the component, merely stale, so the
    /// uninterrupted run and the resumed run are wrong in exactly the same way
    /// and agree perfectly. Reconstitution catches state that lives **outside**
    /// a component; it cannot catch state that never gets **into** one.
    ///
    /// `angvel` was chosen as the injection because it is the field whose loss
    /// shows up latest: a body keeps its initial spin, so it still rotates and
    /// still lands, and the rotation and contact guards below stay green too.
    ///
    /// This compares the two paths instead. The same scene is stepped as a bare
    /// slice through the facade and through the components, and the seven
    /// solver-owned scalars have to agree bit for bit. The mapping's job is to
    /// be transparent, and that is the property stated directly.
    #[test]
    fn the_component_path_computes_what_the_facade_path_computes() {
        let physics = physics();

        let (reference, _) = world_with_scene();
        let mut bare: Vec<Body> = bodies_of(&reference)
            .into_iter()
            .map(|(_, body)| body)
            .collect();

        let (mut world, entities) = world_with_scene();

        for tick in 1..=TICKS {
            physics.step(&mut bare);
            step(&mut world, &physics).expect("alive");

            if CHECKPOINTS.contains(&tick) {
                let expected: Vec<[u32; 7]> = bare
                    .iter()
                    .map(|b| {
                        [
                            b.x.to_bits(),
                            b.y.to_bits(),
                            b.rot_cos.to_bits(),
                            b.rot_sin.to_bits(),
                            b.vel_x.to_bits(),
                            b.vel_y.to_bits(),
                            b.angvel.to_bits(),
                        ]
                    })
                    .collect();

                assert_eq!(
                    expected,
                    bits(&world, &entities),
                    "at tick {tick} the component path and the facade path \
                     disagree. The mapping is meant to be transparent: whatever \
                     the solver writes into a `Body` has to end up in the \
                     component it came from, and a field that is extracted but \
                     never written back is invisible to every other test here"
                );
            }
        }
    }

    #[test]
    fn the_extraction_is_in_sorted_entity_order() {
        let (world, entities) = world_with_scene();
        let extracted: Vec<_> = bodies_of(&world)
            .into_iter()
            .map(|(entity, _)| entity)
            .collect();

        let mut sorted = entities.clone();
        sorted.sort_unstable();

        assert_eq!(
            extracted, sorted,
            "the slice handed to rapier has to be in a reproducible order, and \
             query order is archetype order"
        );
    }

    #[test]
    fn only_entities_that_carry_a_body_are_extracted() {
        let (mut world, entities) = world_with_scene();
        let bystander = world.spawn();
        world
            .insert(bystander, narvo_ecs::Transform::at(1.0, 2.0))
            .expect("just spawned");

        assert_eq!(bodies_of(&world).len(), entities.len());
    }

    #[test]
    fn a_world_without_bodies_steps_without_error() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, narvo_ecs::Transform::at(0.0, 0.0))
            .expect("just spawned");

        step(&mut world, &physics()).expect("a world with no bodies is a valid world");
    }

    /// The `kind` code becomes the facade's enum, and an unnamed code is fixed.
    #[test]
    fn an_unnamed_kind_code_maps_to_a_fixed_body() {
        for (code, expected) in [
            (RigidBody::FIXED, BodyKind::Fixed),
            (RigidBody::DYNAMIC, BodyKind::Dynamic),
            (2, BodyKind::Fixed),
            (u8::MAX, BodyKind::Fixed),
        ] {
            let component = RigidBody {
                kind: code,
                ..RigidBody::dynamic(1.0, 1.0)
            };

            assert_eq!(
                body_of(&component).kind,
                expected,
                "code {code} has to map to {expected:?}, which is what \
                 `RigidBody`'s own table promises a reader"
            );
        }
    }

    #[test]
    fn the_extents_become_a_rectangle_of_the_same_size() {
        let body = body_of(&RigidBody::dynamic(0.41, 0.29));

        assert_eq!(
            body.shape,
            Shape::Cuboid {
                half_x: 0.41,
                half_y: 0.29
            }
        );
    }

    /// Stepping leaves the five input fields alone.
    ///
    /// They are inputs to every rebuild, and a step that quietly rewrote one -
    /// a density normalised by the solver, say - would change what the next
    /// tick computes without anything saying so.
    #[test]
    fn a_step_does_not_move_the_fields_the_solver_does_not_own() {
        let (mut world, entities) = world_with_scene();
        let before: Vec<RigidBody> = entities
            .iter()
            .map(|entity| *world.get::<RigidBody>(*entity).expect("carries a body"))
            .collect();

        for _ in 0..20 {
            step(&mut world, &physics()).expect("alive");
        }

        for (entity, was) in entities.iter().zip(before) {
            let now = *world.get::<RigidBody>(*entity).expect("carries a body");
            for (name, l, r) in [
                ("half_x", was.half_x, now.half_x),
                ("half_y", was.half_y, now.half_y),
                ("restitution", was.restitution, now.restitution),
                ("friction", was.friction, now.friction),
                ("density", was.density, now.density),
            ] {
                assert_eq!(l.to_bits(), r.to_bits(), "a step moved `{name}`");
            }
            assert_eq!(was.kind, now.kind, "a step moved `kind`");
        }
    }

    /// The stepped world dumps, so physics state is inside the hash.
    ///
    /// The whole reason the component exists: what the dump cannot see, the
    /// simulation does not have. Two worlds stepped different numbers of times
    /// have to dump differently, or the bodies are moving outside the registry.
    #[test]
    fn the_stepped_state_is_in_the_canonical_dump() {
        let registry = registry();
        let (early, _) = run_to(1);
        let (late, _) = run_to(TICKS);

        let early = canonical_dump(&early, &registry).expect("registered");
        let late = canonical_dump(&late, &registry).expect("registered");

        assert!(early.contains("rigidbody"), "{early}");
        assert_ne!(
            early, late,
            "the world moved but its dump did not, so the physics state is not \
             where the hash can see it"
        );
    }
}
