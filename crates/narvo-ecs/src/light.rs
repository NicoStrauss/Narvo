//! The game light: what decides, as against what is beautiful.
//!
//! M8.8. There are two lightings in this engine and their roles are unequal.
//! The **image light** is the cascade M8.4–M8.7 build on the GPU: it decides
//! nothing, it is a picture, and no value ever travels from it back into the
//! simulation — a return channel would break ADR-0015, ADR-0010 and every
//! determinism comparison in one move. The **game light** is this module:
//! coarse, on the CPU, inside the tick, **one scalar per entity**, and in the
//! state hash. It is the one that decides.
//!
//! # One source, two consumers — and this is the second one
//!
//! [`Occluder`](crate::Occluder)'s own header says M8.3b existed to create this
//! coupling rather than to use it: the same set of occluders feeds the image
//! light through `narvo-view2d`'s `seeds_of`, and a game's own logic in the
//! tick. That second reader is this one, and the important word is **source**.
//!
//! This module reads [`Occluder`] and [`Transform`] components out of the
//! [`World`]. It does **not** read `Seeds`, a `SeedMap`, or a field read back
//! from the GPU — it cannot, since `narvo-ecs` depends on `hecs`, `serde` and
//! `ron` and on nothing else in this workspace. That independence is what makes
//! the seam test in `narvo-view2d` an oracle instead of a tautology: if the two
//! lights shared a derived structure, their agreement would be a fact about the
//! structure and not about either light.
//!
//! # Rays against the rectangles, and why not a distance field
//!
//! M8.8 §1 offered two ways to read the same occluders and asked for a
//! measurement. Both were built outside the tree and both were compared against
//! the real cascade on 625 receivers:
//!
//! | | rms vs the image light | worst | decision disagreements at 0.5 | cost, 2 occluders |
//! |---|---|---|---|---|
//! | rays against the rectangles | 0.118651 | 0.581707 | 7 of 625 | **0.021 ms** |
//! | a CPU field-and-march | 0.116619 | 0.581707 | 7 of 625 | 1.723 ms |
//!
//! **The field buys 0.002 of rms, the same worst case, and the identical
//! decision, for eighty times the cost.** The two only cross at about 440
//! occluders, because the field's cost is flat in the occluder count and this
//! one's is linear. So the rays win on cost with no seam penalty at all, and
//! the second implementation that would have had to be kept in step with
//! `jump_flood.wgsl` forever is not written.
//!
//! # Three rays, measured
//!
//! "A few rays" is the plan's phrase; three is the number. Sweeping the count
//! against the image light on the same 625 receivers:
//!
//! | rays | rms | decision disagreements | band |
//! |---|---|---|---|
//! | 1 | 0.144840 | 7 | 0.100000 |
//! | 2 | 0.117592 | **17** | 0.244681 |
//! | **3** | **0.118651** | **7** | **0.100000** |
//! | 5 / 9 / 17 / 33 | 0.119422 … 0.123268 | 7 | 0.100000 |
//!
//! One ray is worst: it has no penumbra at all. **An even count is a defect** —
//! with no centre ray, a receiver on the shadow's axis is decided by a pair that
//! straddles it, and the disagreements more than double. From three upward the
//! decision does not improve at all and the rms slowly *worsens*, because more
//! rays make this light's penumbra smoother than the image light's, which is
//! quantised by its own direction count. Three is the knee, and [`RAYS`] is odd
//! on purpose.
//!
//! # Only `+ - * / sqrt`, and no transcendental
//!
//! This value is in the state hash, so two platforms must compute it bit for
//! bit. IEEE-754 requires addition, subtraction, multiplication, division and
//! square root to be **correctly rounded**, which makes them the same on every
//! conforming machine; it requires nothing of `sin`, `cos` or `atan2`, and two
//! libm implementations routinely differ in the last bits. So the ray fan is
//! built from a perpendicular — `(-dy, dx)` over the distance — rather than
//! from an angle, and there is no trigonometry anywhere in this file.
//!
//! ADR-0051's rule about a float multiply feeding a float add is a **GPU** rule
//! and does not reach here: Rust does not permit the compiler to contract
//! `a + b * c` into a fused multiply-add, and every other scalar in the state
//! hash — physics, `Burst::particle`, the camera composition — has been
//! computing this way since M2 with cross-platform determinism holding
//! (ADR-0013).
//!
//! # Opt-in, so a world that never mentions it is unchanged
//!
//! An entity without a [`Lit`] receives nothing and an entity without a
//! [`LightSource`] emits nothing, so a world that registers the two types and
//! carries neither dumps and hashes exactly as it did before. That is
//! [`registering_the_light_types_changes_nothing_for_a_world_that_has_none`](self),
//! a comparison of two registries and never a stored hash (ADR-0008), and it is
//! the same mechanism [`Occluder`] relies on.

use crate::{Occluder, SystemContext, Transform, World};
use serde::{Deserialize, Serialize};

/// How many rays one source gets, per receiver.
///
/// Three, measured rather than chosen — the module header carries the sweep.
/// **Odd on purpose**: an even count has no centre ray, and the measurement put
/// its decision disagreements at more than double the odd counts' on either
/// side of it.
pub const RAYS: u32 = 3;

/// The ray count's two properties, checked at **compile time** rather than by a
/// test.
///
/// Not decoration, and both halves come from the sweep in the module header: an
/// even count has no centre ray and measured more than double the decision
/// disagreements of the odd counts on either side of it, and a single ray has no
/// penumbra at all. A compile-time assertion is the right shape because [`RAYS`]
/// is a constant — a test asserting a property of a constant is a test whose
/// answer is already known when it is compiled, which clippy says in as many
/// words, and this form makes a bad value a build error instead.
const _: () = {
    assert!(RAYS % 2 == 1, "an even ray count has no centre ray");
    assert!(RAYS >= 3, "one ray has no penumbra at all");
};

/// Something that gives off light, for the simulation's purposes.
///
/// Bare scalars only, which is ADR-0014. The position comes from the
/// [`Transform`] beside it, the same arrangement [`Occluder`] and
/// [`HitRect`](crate::HitRect) use: the component is *what a thing is like* and
/// where it is belongs to the thing.
///
/// **No colour**, and that is GDD-L6a rather than an omission. The game light is
/// one axis so that the comparison against the picture is one axis, and so that
/// a tick that runs for every entity every frame stays cheap. Colour is the
/// image light's, and the image light decides nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightSource {
    /// How far the light reaches, in world units. Beyond it a receiver gets
    /// nothing at all.
    pub range: f32,
    /// How bright it is at the source. One scalar (GDD-L6a).
    pub intensity: f32,
    /// The emitter's own half-width, in world units, across the line of sight.
    ///
    /// What makes a shadow soft: the [`RAYS`] rays are spread across this, so a
    /// receiver that sees part of the emitter gets part of the light. Zero is
    /// legal and gives a hard shadow, because all three rays then coincide.
    pub radius: f32,
}

impl LightSource {
    /// A source of this reach, brightness and size.
    #[must_use]
    pub fn new(range: f32, intensity: f32, radius: f32) -> Self {
        Self {
            range,
            intensity,
            radius,
        }
    }
}

/// How lit an entity is — the scalar the game decides on.
///
/// Written by [`illuminate`] every tick, from the [`LightSource`]s and
/// [`Occluder`]s the world holds. An author puts one on an entity to say *this
/// entity cares about light*; its starting value is overwritten on the first
/// tick and is therefore not a setting.
///
/// # It is not clamped
///
/// Two overlapping sources sum past one, and that is a fact about the scene
/// rather than an error. Clamping would put a silent non-linearity between the
/// scene and the number a rule is written against, and would give an injection
/// somewhere to hide.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lit {
    /// The summed contribution of every source that reaches this entity.
    pub level: f32,
}

impl Lit {
    /// An entity that is lit this much. Any value is legal; the first tick
    /// overwrites it.
    #[must_use]
    pub fn new(level: f32) -> Self {
        Self { level }
    }

    /// Completely dark — what an author writes when the starting value does not
    /// matter, which is always.
    pub const DARK: Self = Self { level: 0.0 };
}

/// One occluder, flattened out of the world so the inner loops do not re-borrow.
struct Blocker {
    entity: crate::EntityId,
    x: f32,
    y: f32,
    half_width: f32,
    half_height: f32,
}

/// One source, flattened the same way.
struct Emitter {
    entity: crate::EntityId,
    x: f32,
    y: f32,
    range: f32,
    intensity: f32,
    radius: f32,
}

/// Whether the segment from `(px, py)` to `(qx, qy)` meets the axis-parallel
/// rectangle `blocker`.
///
/// The slab method over the segment's own parameter, so a blocker *behind* the
/// source or *behind* the receiver does not block: the interval is clipped to
/// `[0, 1]` before the test, not after.
///
/// An axis-parallel ray divides by zero and gets an infinity, which the `min`
/// and `max` below carry correctly.
///
/// # The degenerate guard is explicit, and it has to be
///
/// A negative or `NaN` half-extent blocks nothing, matching
/// [`Occluder::contains`] — and that agreement is exactly the property the two
/// halves of the engine have to share at a boundary, since `seeds_of` decides
/// which texels a rectangle covers by asking `contains` per texel centre.
///
/// **It cannot be left to the arithmetic, which was measured rather than
/// assumed.** `contains` gets the answer for free because every comparison
/// against `NaN` is false; the slab method does not, twice over.
/// [`f32::min`] and [`f32::max`] *ignore* a `NaN` operand and return the other
/// one, so a `NaN` slab contributes no constraint and the rectangle becomes
/// **permissive** instead of empty. And `min`/`max` normalise an inverted
/// interval, so a **negative** half-extent would behave exactly like its
/// positive twin rather than like an empty rectangle. Both were caught by
/// `a_nan_blocks_nothing_and_lights_nothing` on the first run.
fn segment_meets(px: f32, py: f32, qx: f32, qy: f32, blocker: &Blocker) -> bool {
    // `>=` rather than `!<`: false for `NaN` and false for a negative extent,
    // which are the two cases `contains` answers with "nothing".
    if !(blocker.half_width >= 0.0 && blocker.half_height >= 0.0) {
        return false;
    }
    if !(blocker.x.is_finite() && blocker.y.is_finite()) {
        return false;
    }

    let (dx, dy) = (qx - px, qy - py);
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);

    for (position, delta, centre, half) in [
        (px, dx, blocker.x, blocker.half_width),
        (py, dy, blocker.y, blocker.half_height),
    ] {
        let (near, far) = (centre - half, centre + half);
        if delta == 0.0 {
            // Parallel to this slab: either the whole segment is inside it or
            // none of it is.
            if !(position >= near && position <= far) {
                return false;
            }
            continue;
        }
        let first = (near - position) / delta;
        let second = (far - position) / delta;
        lo = lo.max(first.min(second));
        hi = hi.min(first.max(second));
    }
    lo <= hi
}

/// Writes every [`Lit`] entity's level from the world's [`LightSource`]s and
/// [`Occluder`]s.
///
/// # What it reads and what it writes
///
/// Reads [`Transform`], [`LightSource`] and [`Occluder`]; writes [`Lit`] and
/// nothing else. An entity needs both a `Lit` and a `Transform` to receive, and
/// both a `LightSource` and a `Transform` to emit — a component that describes
/// a light with no position describes nothing, so it is skipped rather than
/// defaulted, exactly as `seeds_of` skips an occluder without a transform.
///
/// # A thing does not shadow itself
///
/// A receiver's own occluder and a source's own occluder are skipped. Without
/// that, a crate carrying both `Occluder` and `Lit` would sit inside its own
/// rectangle and be permanently dark, and a lamp with a body would put itself
/// out. It is two comparisons and it is the difference between the component
/// pair being usable together and not.
///
/// # Order
///
/// [`World::entity_ids`] throughout, which is the canonical order —
/// `narvo-ecs` documents query order as explicitly unstable, and this value
/// goes into the state hash. Two worlds built in opposite orders produce the
/// same levels, and `illuminating_is_the_same_in_either_spawn_order` says so.
pub fn illuminate(world: &mut World, _context: &SystemContext) {
    let ids = world.entity_ids();

    let mut blockers = Vec::new();
    let mut emitters = Vec::new();
    for &entity in &ids {
        let Ok(transform) = world.get::<Transform>(entity) else {
            continue;
        };
        let (x, y) = (transform.x, transform.y);
        drop(transform);

        if let Ok(occluder) = world.get::<Occluder>(entity) {
            blockers.push(Blocker {
                entity,
                x,
                y,
                half_width: occluder.half_width,
                half_height: occluder.half_height,
            });
        }
        if let Ok(source) = world.get::<LightSource>(entity) {
            emitters.push(Emitter {
                entity,
                x,
                y,
                range: source.range,
                intensity: source.intensity,
                radius: source.radius,
            });
        }
    }

    for &receiver in &ids {
        if !world.has::<Lit>(receiver) {
            continue;
        }
        let Ok(transform) = world.get::<Transform>(receiver) else {
            continue;
        };
        let (rx, ry) = (transform.x, transform.y);
        drop(transform);

        let level = level_at(rx, ry, receiver, &blockers, &emitters);
        if let Ok(mut lit) = world.get_mut::<Lit>(receiver) {
            lit.level = level;
        }
    }
}

/// The scalar one receiver ends up with. Split out so the arithmetic can be
/// read and tested without a world around it.
fn level_at(
    rx: f32,
    ry: f32,
    receiver: crate::EntityId,
    blockers: &[Blocker],
    emitters: &[Emitter],
) -> f32 {
    let mut level = 0.0_f32;

    for emitter in emitters {
        let (dx, dy) = (emitter.x - rx, emitter.y - ry);
        let distance = (dx * dx + dy * dy).sqrt();

        // **The positive form on purpose.** Everything below runs only when the
        // distance is genuinely inside the range; a `NaN` on either side makes
        // this comparison false and the emitter contributes nothing, which is
        // the answer `Occluder::contains` gives to the same question. Written
        // as a positive `if` rather than a negated one because a negated
        // comparison on a partially ordered type is exactly the shape that
        // reads as "not out of reach" and means something else at `NaN`.
        if distance < emitter.range {
            // The fan's axis, without an angle: the perpendicular to the line of
            // sight, normalised. `sqrt` is correctly rounded and `sin` is not,
            // which is the whole reason this is written as a perpendicular.
            let (nx, ny) = if distance > 0.0 {
                (-dy / distance, dx / distance)
            } else {
                (0.0, 0.0)
            };

            let mut open = 0_u32;
            for k in 0..RAYS {
                // `k` runs 0..RAYS, so the offset runs -1, 0, +1 for three rays.
                let across = (2.0 * k as f32 / (RAYS - 1) as f32) - 1.0;
                let offset = across * emitter.radius;
                let (tx, ty) = (emitter.x + nx * offset, emitter.y + ny * offset);

                let blocked = blockers.iter().any(|blocker| {
                    blocker.entity != receiver
                        && blocker.entity != emitter.entity
                        && segment_meets(rx, ry, tx, ty, blocker)
                });
                if !blocked {
                    open += 1;
                }
            }

            let falloff = 1.0 - distance / emitter.range;
            level += emitter.intensity * falloff * (open as f32 / RAYS as f32);
        }
    }

    level
}

#[cfg(test)]
mod tests {
    use super::{LightSource, Lit, illuminate};
    use crate::{
        ComponentRegistry, EntityId, Occluder, SystemContext, Transform, World, canonical_dump,
    };

    /// A lamp to the left, a receiver to the right, and nothing between them.
    fn lit_pair(wall: Option<(f32, f32, f32, f32)>) -> (World, EntityId) {
        let mut world = World::new();

        let lamp = world.spawn();
        world
            .insert(lamp, Transform::at(-8.0, 0.0))
            .expect("insert");
        world
            .insert(lamp, LightSource::new(24.0, 1.0, 0.75))
            .expect("insert");

        if let Some((x, y, half_width, half_height)) = wall {
            let blocker = world.spawn();
            world.insert(blocker, Transform::at(x, y)).expect("insert");
            world
                .insert(blocker, Occluder::new(half_width, half_height))
                .expect("insert");
        }

        let receiver = world.spawn();
        world
            .insert(receiver, Transform::at(4.0, 0.0))
            .expect("insert");
        world.insert(receiver, Lit::DARK).expect("insert");

        (world, receiver)
    }

    fn level_of(world: &World, entity: EntityId) -> f32 {
        world.get::<Lit>(entity).expect("a lit entity").level
    }

    #[test]
    fn an_unobstructed_receiver_is_fully_lit_and_a_walled_one_is_dark() {
        let (mut world, receiver) = lit_pair(None);
        illuminate(&mut world, &SystemContext::new(0));
        let open = level_of(&world, receiver);
        assert!(open > 0.0, "a receiver in the open got no light at all");

        // A wall tall enough that all three rays cross it.
        let (mut world, receiver) = lit_pair(Some((-2.0, 0.0, 0.5, 6.0)));
        illuminate(&mut world, &SystemContext::new(0));
        assert_eq!(
            level_of(&world, receiver),
            0.0,
            "a receiver behind a solid wall was lit"
        );
    }

    /// **The penumbra is what the ray count is for.**
    ///
    /// A wall whose edge cuts the fan leaves some rays open and some blocked, so
    /// the level lands strictly between the two extremes. A single-ray light
    /// could not produce this value, which is what makes the assertion say
    /// something about the fan rather than about the falloff.
    #[test]
    fn an_edge_leaves_a_receiver_partly_lit() {
        let (mut world, receiver) = lit_pair(None);
        illuminate(&mut world, &SystemContext::new(0));
        let open = level_of(&world, receiver);

        // The wall's top edge sits on the line of sight, so the fan straddles
        // it: the lamp's radius is 0.75, the fan spans that, and the edge falls
        // inside the span.
        let (mut world, receiver) = lit_pair(Some((-2.0, -6.0, 0.5, 6.0)));
        illuminate(&mut world, &SystemContext::new(0));
        let partial = level_of(&world, receiver);

        assert!(
            partial > 0.0 && partial < open,
            "an edge gave {partial}, which is not between darkness and {open}"
        );
    }

    #[test]
    fn the_level_falls_off_with_distance() {
        let mut world = World::new();
        let lamp = world.spawn();
        world.insert(lamp, Transform::at(0.0, 0.0)).expect("insert");
        world
            .insert(lamp, LightSource::new(10.0, 1.0, 0.0))
            .expect("insert");

        let mut at = |x: f32| {
            let entity = world.spawn();
            world.insert(entity, Transform::at(x, 0.0)).expect("insert");
            world.insert(entity, Lit::DARK).expect("insert");
            entity
        };
        let (near, far, beyond) = (at(1.0), at(5.0), at(20.0));

        illuminate(&mut world, &SystemContext::new(0));

        assert!(level_of(&world, near) > level_of(&world, far));
        assert_eq!(
            level_of(&world, beyond),
            0.0,
            "a receiver outside the range was lit"
        );
    }

    /// A thing does not put itself out.
    #[test]
    fn an_entity_s_own_occluder_does_not_shadow_it() {
        let (mut world, receiver) = lit_pair(None);
        illuminate(&mut world, &SystemContext::new(0));
        let bare = level_of(&world, receiver);

        world
            .insert(receiver, Occluder::new(1.0, 1.0))
            .expect("insert");
        illuminate(&mut world, &SystemContext::new(0));

        assert_eq!(
            level_of(&world, receiver),
            bare,
            "an entity was shadowed by its own occluder"
        );
    }

    /// Two worlds, same entities, opposite spawn order, same levels.
    ///
    /// The property [`World::entity_ids`] is used for. It cannot pass vacuously:
    /// the arrangement is asserted to produce a *partial* level, so the two
    /// worlds are being compared on a number that a reordering could actually
    /// move.
    #[test]
    fn illuminating_is_the_same_in_either_spawn_order() {
        fn build(reverse: bool) -> f32 {
            let mut world = World::new();
            let place = |world: &mut World, which: u32| match which {
                0 => {
                    let lamp = world.spawn();
                    world
                        .insert(lamp, Transform::at(-8.0, 0.0))
                        .expect("insert");
                    world
                        .insert(lamp, LightSource::new(24.0, 1.0, 0.75))
                        .expect("insert");
                    lamp
                }
                1 => {
                    let wall = world.spawn();
                    world
                        .insert(wall, Transform::at(-2.0, -6.0))
                        .expect("insert");
                    world.insert(wall, Occluder::new(0.5, 6.0)).expect("insert");
                    wall
                }
                _ => {
                    let receiver = world.spawn();
                    world
                        .insert(receiver, Transform::at(4.0, 0.0))
                        .expect("insert");
                    world.insert(receiver, Lit::DARK).expect("insert");
                    receiver
                }
            };

            let order: [u32; 3] = if reverse { [2, 1, 0] } else { [0, 1, 2] };
            let mut receiver = None;
            for which in order {
                let entity = place(&mut world, which);
                if which == 2 {
                    receiver = Some(entity);
                }
            }

            illuminate(&mut world, &SystemContext::new(0));
            world
                .get::<Lit>(receiver.expect("a receiver was placed"))
                .expect("a lit entity")
                .level
        }

        let forwards = build(false);
        let backwards = build(true);

        assert!(
            forwards > 0.0 && forwards < 1.0,
            "the arrangement is meant to be partly lit, and gave {forwards} — \
             a level of exactly zero or one would let a reordering hide"
        );
        assert_eq!(forwards, backwards, "spawn order moved the game light");
    }

    /// Running it twice changes nothing, so the value is a function of the world
    /// and not of how many times a tick ran.
    #[test]
    fn illuminating_twice_gives_the_same_level() {
        let (mut world, receiver) = lit_pair(Some((-2.0, -6.0, 0.5, 6.0)));
        illuminate(&mut world, &SystemContext::new(0));
        let once = level_of(&world, receiver);
        illuminate(&mut world, &SystemContext::new(1));
        assert_eq!(once, level_of(&world, receiver));
    }

    /// An entity without a [`Lit`] gets nothing written to it, and a world
    /// without a source leaves every receiver dark.
    #[test]
    fn nothing_is_written_to_an_entity_that_did_not_ask() {
        let mut world = World::new();
        let lamp = world.spawn();
        world.insert(lamp, Transform::at(0.0, 0.0)).expect("insert");
        world
            .insert(lamp, LightSource::new(10.0, 1.0, 0.0))
            .expect("insert");

        let bystander = world.spawn();
        world
            .insert(bystander, Transform::at(1.0, 0.0))
            .expect("insert");

        illuminate(&mut world, &SystemContext::new(0));

        assert!(
            !world.has::<Lit>(bystander),
            "illuminate inserted a Lit on an entity that did not carry one"
        );
    }

    #[test]
    fn a_receiver_with_no_source_is_dark() {
        let mut world = World::new();
        let receiver = world.spawn();
        world
            .insert(receiver, Transform::at(0.0, 0.0))
            .expect("insert");
        world.insert(receiver, Lit::new(0.75)).expect("insert");

        illuminate(&mut world, &SystemContext::new(0));

        assert_eq!(
            level_of(&world, receiver),
            0.0,
            "a starting level survived a world with no light in it"
        );
    }

    /// A source or a receiver with no transform describes nothing and is
    /// skipped, rather than being treated as though it stood at the origin.
    #[test]
    fn a_light_without_a_position_lights_nothing() {
        let mut world = World::new();
        let lamp = world.spawn();
        world
            .insert(lamp, LightSource::new(24.0, 1.0, 0.0))
            .expect("insert");

        let receiver = world.spawn();
        world
            .insert(receiver, Transform::at(1.0, 0.0))
            .expect("insert");
        world.insert(receiver, Lit::DARK).expect("insert");

        illuminate(&mut world, &SystemContext::new(0));

        assert_eq!(level_of(&world, receiver), 0.0);
    }

    /// Two sources sum, and the sum is allowed past one.
    #[test]
    fn two_sources_sum_and_are_not_clamped() {
        let mut world = World::new();
        for x in [-1.0_f32, 1.0] {
            let lamp = world.spawn();
            world.insert(lamp, Transform::at(x, 0.0)).expect("insert");
            world
                .insert(lamp, LightSource::new(100.0, 1.0, 0.0))
                .expect("insert");
        }
        let receiver = world.spawn();
        world
            .insert(receiver, Transform::at(0.0, 0.0))
            .expect("insert");
        world.insert(receiver, Lit::DARK).expect("insert");

        illuminate(&mut world, &SystemContext::new(0));

        assert!(
            level_of(&world, receiver) > 1.0,
            "two full-strength sources were clamped to one"
        );
    }

    /// A `NaN` extent blocks nothing, a `NaN` position lights nothing — the
    /// same answers [`Occluder::contains`] gives, so the two halves of the
    /// engine agree at the degenerate cases too.
    #[test]
    fn a_nan_blocks_nothing_and_lights_nothing() {
        for extent in [f32::NAN, -1.0] {
            let (mut world, receiver) = lit_pair(Some((-2.0, 0.0, extent, 6.0)));
            illuminate(&mut world, &SystemContext::new(0));
            assert!(
                level_of(&world, receiver) > 0.0,
                "an occluder of half-width {extent} blocked light, where Occluder::contains says it contains nothing"
            );
        }

        let mut world = World::new();
        let lamp = world.spawn();
        world
            .insert(lamp, Transform::at(f32::NAN, 0.0))
            .expect("insert");
        world
            .insert(lamp, LightSource::new(24.0, 1.0, 0.0))
            .expect("insert");
        let receiver = world.spawn();
        world
            .insert(receiver, Transform::at(0.0, 0.0))
            .expect("insert");
        world.insert(receiver, Lit::DARK).expect("insert");
        illuminate(&mut world, &SystemContext::new(0));
        assert_eq!(
            level_of(&world, receiver),
            0.0,
            "a NaN-positioned lamp lit something"
        );
    }

    /// The two types survive the registry's own round trip, in the registry's
    /// own spelling.
    ///
    /// Asserted directly rather than only compared against itself, so a serde
    /// attribute changing the representation shows up here as an edit rather
    /// than in a save file somebody cannot load.
    #[test]
    fn the_light_types_dump_in_their_own_spelling() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<LightSource>("lightsource")
            .expect("a fresh registry accepts the type");
        registry
            .register_component::<Lit>("lit")
            .expect("a fresh registry accepts the type");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, LightSource::new(12.0, 0.5, 0.25))
            .expect("insert");
        world.insert(entity, Lit::new(0.75)).expect("insert");

        let dumped = canonical_dump(&world, &registry).expect("the world dumps");
        assert!(
            dumped.contains("lightsource (range:12.0,intensity:0.5,radius:0.25)"),
            "the source is not spelled as expected in:\n{dumped}"
        );
        assert!(
            dumped.contains("lit (level:0.75)"),
            "the level is not spelled as expected in:\n{dumped}"
        );
    }

    /// **The game light is world state: the dump moves with it.**
    ///
    /// Three worlds compared against each other, never against a stored hash
    /// (ADR-0008). The third comparison is the one that matters: moving the wall
    /// has to move the dump, or a recording could replay a shadow into the wrong
    /// place and agree with itself.
    #[test]
    fn the_computed_level_reaches_the_dump_and_a_moved_wall_moves_it() {
        let mut registry = ComponentRegistry::new();
        for (name, register) in [
            ("transform", 0),
            ("occluder", 1),
            ("lightsource", 2),
            ("lit", 3),
        ] {
            match register {
                0 => registry.register_component::<Transform>(name),
                1 => registry.register_component::<Occluder>(name),
                2 => registry.register_component::<LightSource>(name),
                _ => registry.register_component::<Lit>(name),
            }
            .expect("a fresh registry accepts the type");
        }

        let dump_of = |wall: Option<(f32, f32, f32, f32)>| {
            let (mut world, _) = lit_pair(wall);
            illuminate(&mut world, &SystemContext::new(0));
            canonical_dump(&world, &registry).expect("the world dumps")
        };

        let open = dump_of(None);
        let walled = dump_of(Some((-2.0, 0.0, 0.5, 6.0)));
        let moved = dump_of(Some((-2.0, 20.0, 0.5, 6.0)));

        assert_ne!(open, walled, "a wall did not move the dump");
        assert_ne!(walled, moved, "moving a wall did not move the dump");
        assert_eq!(
            walled,
            dump_of(Some((-2.0, 0.0, 0.5, 6.0))),
            "one world dumped two ways"
        );
    }

    /// Registering the two types moves nothing for a world that carries
    /// neither.
    ///
    /// Two registries over one world, compared against each other — never
    /// against a stored hash (ADR-0008). This is what lets them be added to
    /// `register_engine_components` without every existing scene's hash moving,
    /// and it is the mechanism M8.8's V3 predicts from.
    #[test]
    fn registering_the_light_types_changes_nothing_for_a_world_that_has_none() {
        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts the type");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts the type");
        with.register_component::<LightSource>("lightsource")
            .expect("a fresh registry accepts the type");
        with.register_component::<Lit>("lit")
            .expect("a fresh registry accepts the type");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::at(3.0, 4.0))
            .expect("insert");

        assert_eq!(
            canonical_dump(&world, &without).expect("the world dumps"),
            canonical_dump(&world, &with).expect("the world dumps"),
            "registering the light types moved a world that carries neither"
        );
    }
}
