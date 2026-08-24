//! A rigid body, as the scalars a physics step needs and returns.
//!
//! [`RigidBody`] is the registered form of `narvo_physics2d::Body`. ADR-0029
//! decided that the physics world is rebuilt from the caller's scalars every
//! tick and keeps nothing, and stated the consequence as a rule about *this*
//! side of the seam:
//!
//! > there is no physics state outside the registry. What the dump cannot see,
//! > the simulation does not have.
//!
//! So every quantity that survives a tick is a field here. A body's position,
//! its rotation, both velocities, its extents and its material all cross the
//! boundary, and all of them are in the canonical dump of any simulation that
//! registers this type.
//!
//! # Why this type is self-contained, and does not lean on `Transform`
//!
//! The obvious economy is to keep the position in [`Transform`](crate::Transform),
//! which already means "where something is", and to carry only the physics
//! extras here. It was rejected, and the reason is the rotation rather than the
//! position.
//!
//! `Transform` holds rotation as a scalar **angle**. rapier holds it as a
//! `(cos, sin)` **pair**, and M5b.2 measured what happens to a simulation whose
//! rotation is round-tripped through an angle each tick: it leaves the exact one
//! at tick 2, before anything has touched anything. Worse, the conversion is
//! `atan2` and `sin`/`cos` out of the standard library, and
//! `enhanced-determinism` routes rapier's and glam's maths through `libm` but
//! does not reach `std` — the same measurement found a `std` angle round trip
//! that was lossy on Windows and exact on Linux. A system writing an angle into
//! a registered component would put that platform-dependent value straight into
//! the state hash.
//!
//! Splitting the pose — position in `Transform`, rotation pair here — avoids the
//! conversion but buys a worse problem: a body would have two homes, an entity
//! carrying one and not the other would be a body without a position, and the
//! reconstitution property ADR-0029 rests on would depend on two components
//! agreeing. One type that holds the whole body has no such state.
//!
//! Nothing is lost for drawing. A consumer that wants a `Transform`-shaped pose
//! converts at its own call site, outside the world, where the choice of
//! trigonometry is visible and its result is never hashed — which is what
//! `narvo-physics2d`'s own crate documentation says about the missing
//! `angle()` accessor.
//!
//! # Opt-in, so a world that never mentions it is unchanged
//!
//! An entity without a `RigidBody` is not a physics body, and a world that never
//! registers the type dumps and hashes exactly as it did before. That is the
//! construction every optional component here uses, and
//! `registering_the_type_changes_nothing_for_a_world_that_has_none` below is the
//! proof — a comparison of two registries, never a stored hash (ADR-0008).

use serde::{Deserialize, Serialize};

/// One rigid body, as bare scalars.
///
/// Every field is an `f32` or a `u8`, so the type stays inside ADR-0014: no
/// dependency's serde format governs a state hash through it. The mapping to
/// and from `narvo_physics2d::Body` lives in the crate that sees both sides,
/// which is `narvo-app` — ADR-0015's arrangement, the same one the sprite
/// extraction and the audio cue mapping already use. **`narvo-ecs` has no
/// dependency on `narvo-physics2d` and must not grow one:** this component
/// carries scalars, and only the mapping needs `Body`.
///
/// # `kind` is a bare integer, not an enum
///
/// The same decision `Sampling` took and for the same reason. An enum's RON
/// representation is serde's to choose — `Dynamic`, `(Dynamic)`, `1`, depending
/// on representation attributes and on serde's own version — and that would put
/// a formatting decision inside the hash domain ADR-0008 defines. An integer
/// with the mapping written down here has no such freedom.
///
/// The mapping, and it is the whole contract:
///
/// | value | meaning |
/// | --- | --- |
/// | [`RigidBody::FIXED`] = 0 | never moved by the solver: ground, walls |
/// | [`RigidBody::DYNAMIC`] = 1 | integrated and collided by the solver |
/// | anything else | reserved; a reader treats it as `FIXED` |
///
/// **The reserved row is deliberate and is not validation.** This type does not
/// reject an unknown code, exactly as `Layer` does not reject a `NaN` depth: a
/// component is storage, and rejecting here would mean deciding what a partially
/// valid world is. `FIXED` is the reserved reading because it is the inert one —
/// an unrecognised code produces a body that sits still, which is visible and
/// harmless, rather than one that falls out of the world.
///
/// # The shape is a rectangle, and that is the whole vocabulary today
///
/// [`half_x`](Self::half_x) and [`half_y`](Self::half_y) are the half-extents of
/// an axis-aligned rectangle. `narvo_physics2d::Shape` also offers a ball, and
/// **it is deliberately not reachable from here**: `ProjektPlan.md` §2 forbids
/// building a capability without a consumer, and M5b.4's falling, colliding
/// bodies need one shape — a rectangle serves as the ground and as the things
/// that land on it, which a ball cannot.
///
/// The price is named rather than hidden. Adding the ball later means adding a
/// shape discriminant beside `kind` and giving these two fields a second
/// meaning, and ADR-0014 says what that costs: widening a component already
/// inside the state hash is a more expensive edit than carrying the field from
/// the start. That is accepted here because the alternative is a discriminant
/// with one value, which is exactly the stock §2 refuses.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RigidBody {
    /// Whether the solver moves this body. See the table on [`RigidBody`].
    pub kind: u8,
    /// Half the body's width, in world units. Must be finite and above zero by
    /// the time it reaches the solver.
    pub half_x: f32,
    /// Half the body's height, in world units.
    pub half_y: f32,
    /// World-space x of the body's centre.
    pub x: f32,
    /// World-space y of the body's centre.
    pub y: f32,
    /// Cosine of the body's rotation. A pair rather than an angle; the module
    /// documentation says why.
    pub rot_cos: f32,
    /// Sine of the body's rotation.
    pub rot_sin: f32,
    /// Linear velocity along x, world units per second.
    pub vel_x: f32,
    /// Linear velocity along y, world units per second.
    pub vel_y: f32,
    /// Angular velocity, radians per second.
    pub angvel: f32,
    /// Bounciness, 0 for none.
    pub restitution: f32,
    /// Coulomb friction coefficient.
    pub friction: f32,
    /// Mass per unit area, from which the solver derives mass and inertia.
    /// Ignored for a [`FIXED`](Self::FIXED) body.
    pub density: f32,
}

impl RigidBody {
    /// Never moved by the solver.
    pub const FIXED: u8 = 0;

    /// Integrated and collided by the solver.
    pub const DYNAMIC: u8 = 1;

    /// A dynamic rectangle of these half-extents, at rest at the origin.
    ///
    /// The rotation starts as the identity — `rot_cos = 1.0`, `rot_sin = 0.0` —
    /// which is the pair form of an angle of zero. The material defaults match
    /// `narvo_physics2d::Body::dynamic` so that a body built here and a body
    /// built there describe the same thing.
    #[must_use]
    pub const fn dynamic(half_x: f32, half_y: f32) -> Self {
        Self {
            kind: Self::DYNAMIC,
            half_x,
            half_y,
            x: 0.0,
            y: 0.0,
            rot_cos: 1.0,
            rot_sin: 0.0,
            vel_x: 0.0,
            vel_y: 0.0,
            angvel: 0.0,
            restitution: 0.0,
            friction: 0.5,
            density: 1.0,
        }
    }

    /// A fixed rectangle of these half-extents, at the origin.
    #[must_use]
    pub const fn fixed(half_x: f32, half_y: f32) -> Self {
        Self {
            kind: Self::FIXED,
            ..Self::dynamic(half_x, half_y)
        }
    }

    /// Places the body, keeping everything else.
    #[must_use]
    pub const fn at(mut self, x: f32, y: f32) -> Self {
        self.x = x;
        self.y = y;
        self
    }

    /// Sets the linear velocity, keeping everything else.
    #[must_use]
    pub const fn with_velocity(mut self, vel_x: f32, vel_y: f32) -> Self {
        self.vel_x = vel_x;
        self.vel_y = vel_y;
        self
    }

    /// Sets the angular velocity, keeping everything else.
    #[must_use]
    pub const fn with_angvel(mut self, angvel: f32) -> Self {
        self.angvel = angvel;
        self
    }

    /// Sets the material scalars, keeping everything else.
    #[must_use]
    pub const fn with_material(mut self, restitution: f32, friction: f32, density: f32) -> Self {
        self.restitution = restitution;
        self.friction = friction;
        self.density = density;
        self
    }

    // There is deliberately no `angle()` and no `with_angle()`, for the reason
    // `narvo-physics2d` gives at its own `Body`: both would be one line of
    // `f32::atan2` and `f32::sin`/`cos`, the standard library's trigonometry is
    // outside the domain `enhanced-determinism` establishes, and a value
    // computed that way in a *registered* component would reach the state hash.
}

#[cfg(test)]
mod tests {
    use super::RigidBody;
    use crate::{ComponentRegistry, World, canonical_dump, state_hash};

    /// Values chosen to break a lossy encoder, the set `Transform` uses.
    ///
    /// `0.1` and `-1.0 / 3.0` do not terminate in binary; [`f32::MIN_POSITIVE`]
    /// and the subnormal below it carry precision in the mantissa alone;
    /// [`f32::MAX`] is where neighbours are further apart than most whole
    /// numbers; `-0.0` is what `==` cannot tell from `0.0` and what a formatter
    /// can silently drop; and the pair one mantissa bit apart is the smallest
    /// difference that exists at its magnitude.
    fn probes() -> Vec<f32> {
        let one_tenth = 0.1_f32;
        vec![
            one_tenth,
            f32::from_bits(one_tenth.to_bits() + 1),
            -1.0 / 3.0,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::MAX,
            -f32::MAX,
            -0.0,
            0.0,
            core::f32::consts::PI,
        ]
    }

    /// A body whose every float field carries `value`.
    fn filled(value: f32) -> RigidBody {
        RigidBody {
            kind: RigidBody::DYNAMIC,
            half_x: value,
            half_y: value,
            x: value,
            y: value,
            rot_cos: value,
            rot_sin: value,
            vel_x: value,
            vel_y: value,
            angvel: value,
            restitution: value,
            friction: value,
            density: value,
        }
    }

    /// Every float field of two bodies, paired by name for the message.
    fn fields(left: RigidBody, right: RigidBody) -> Vec<(&'static str, f32, f32)> {
        vec![
            ("half_x", left.half_x, right.half_x),
            ("half_y", left.half_y, right.half_y),
            ("x", left.x, right.x),
            ("y", left.y, right.y),
            ("rot_cos", left.rot_cos, right.rot_cos),
            ("rot_sin", left.rot_sin, right.rot_sin),
            ("vel_x", left.vel_x, right.vel_x),
            ("vel_y", left.vel_y, right.vel_y),
            ("angvel", left.angvel, right.angvel),
            ("restitution", left.restitution, right.restitution),
            ("friction", left.friction, right.friction),
            ("density", left.density, right.density),
        ]
    }

    /// Compares two bodies on bit patterns, field by field.
    ///
    /// `==` is the wrong instrument twice over, which is ADR-0014's own
    /// consequence: it calls `0.0` and `-0.0` equal, and it calls two NaNs
    /// unequal. A state hash sees bits, so a test guarding the hash sees bits.
    fn assert_same_bits(left: RigidBody, right: RigidBody, case: &str) {
        assert_eq!(
            left.kind, right.kind,
            "{case}: field `kind` did not survive"
        );

        for (name, l, r) in fields(left, right) {
            assert_eq!(
                l.to_bits(),
                r.to_bits(),
                "{case}: field `{name}` did not survive the round trip bit for bit: \
                 {l:?} ({:#010x}) became {r:?} ({:#010x}). A field that loses bits here \
                 loses them in the canonical dump too, and a replay built on it drifts \
                 while every hash still reports agreement.",
                l.to_bits(),
                r.to_bits()
            );
        }
    }

    #[test]
    fn the_ron_round_trip_is_bit_exact_on_values_that_stress_it() {
        for value in probes() {
            let original = filled(value);
            let text = ron::to_string(&original).expect("a body of scalars serializes");
            let returned: RigidBody = ron::from_str(&text)
                .unwrap_or_else(|error| panic!("{text} does not parse back: {error}"));

            println!("{:#010x} -> {text}", value.to_bits());
            assert_same_bits(original, returned, &format!("{value:?}"));
        }
    }

    /// The negative control: the comparison above would notice a lossy encoder.
    ///
    /// Without it the round-trip test proves only that *something* passed. A
    /// test whose failure mode has never been observed is a test nobody has
    /// checked, which is how a green run comes to mean nothing.
    #[test]
    fn a_lossy_encoder_would_be_caught_by_the_same_comparison() {
        // Six decimals reads as generous and is not: an f32 near 0.1 needs nine
        // significant digits before the bits come back unchanged.
        let lossy = |value: f32| -> f32 {
            format!("{value:.6}")
                .parse()
                .expect("a fixed-point decimal parses back as f32")
        };

        let victim = probes()
            .into_iter()
            .find(|value| lossy(*value).to_bits() != value.to_bits())
            .expect("at least one probe has to survive six decimals badly");

        let outcome = std::panic::catch_unwind(|| {
            assert_same_bits(filled(victim), filled(lossy(victim)), "negative control");
        });

        assert!(
            outcome.is_err(),
            "assert_same_bits accepted {victim:?} against its six-decimal \
             approximation, so it is not comparing bits"
        );
    }

    /// Every `kind` code round-trips, named or reserved.
    #[test]
    fn the_ron_round_trip_is_exact_on_every_kind_code() {
        for kind in [RigidBody::FIXED, RigidBody::DYNAMIC, 2, 7, u8::MAX] {
            let original = RigidBody {
                kind,
                ..RigidBody::dynamic(1.0, 1.0)
            };
            let text = ron::to_string(&original).expect("a body of scalars serializes");
            let returned: RigidBody = ron::from_str(&text).expect("what we just wrote parses");

            assert_eq!(returned.kind, kind, "{text} did not return code {kind}");
        }
    }

    /// The constructors agree with the codes they claim to set.
    #[test]
    fn the_constructors_set_the_codes_the_table_names() {
        assert_eq!(RigidBody::dynamic(1.0, 2.0).kind, RigidBody::DYNAMIC);
        assert_eq!(RigidBody::fixed(1.0, 2.0).kind, RigidBody::FIXED);

        // `fixed` differs from `dynamic` in the code and in nothing else, which
        // is what makes the two comparable in a scene.
        let dynamic = RigidBody::dynamic(3.0, 4.0);
        let fixed = RigidBody::fixed(3.0, 4.0);
        assert_eq!(
            RigidBody {
                kind: RigidBody::DYNAMIC,
                ..fixed
            },
            dynamic
        );
    }

    /// Builds a one-entity world and returns its canonical dump and hash.
    fn hashed(body: Option<RigidBody>) -> (String, u64) {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<RigidBody>("rigidbody")
            .expect("a fresh registry has no rigid body in it");

        let mut world = World::new();
        let entity = world.spawn();
        if let Some(body) = body {
            world
                .insert(entity, body)
                .expect("the entity was just spawned");
        }

        let dump = canonical_dump(&world, &registry).expect("the component is registered");
        (dump.clone(), state_hash(&dump))
    }

    /// The hash sees the component: two bodies that differ hash differently.
    #[test]
    fn a_body_is_in_the_dump_and_therefore_in_the_hash() {
        let (dump, hash) = hashed(Some(RigidBody::dynamic(1.0, 1.0).at(0.5, 0.25)));
        let (other_dump, other_hash) = hashed(Some(RigidBody::dynamic(1.0, 1.0).at(0.5, 0.5)));

        assert!(dump.contains("rigidbody"), "{dump}");
        assert_ne!(dump, other_dump);
        assert_ne!(hash, other_hash);
    }

    /// Registering the type changes nothing for a world that has none.
    ///
    /// Two registries over one world, compared against each other and never
    /// against a stored value (ADR-0008).
    #[test]
    fn registering_the_type_changes_nothing_for_a_world_that_has_none() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, crate::Transform::at(1.0, 2.0))
            .expect("just spawned");

        let mut without = ComponentRegistry::new();
        without
            .register_component::<crate::Transform>("transform")
            .expect("a fresh registry");

        let mut with = ComponentRegistry::new();
        with.register_component::<crate::Transform>("transform")
            .expect("a fresh registry");
        with.register_component::<RigidBody>("rigidbody")
            .expect("a fresh registry");

        assert_eq!(
            canonical_dump(&world, &without).expect("registered"),
            canonical_dump(&world, &with).expect("registered"),
            "registering `RigidBody` moved the dump of a world that carries none, \
             so every existing scenario's hash would move with it"
        );
    }
}
