//! The colour an entity's texels are multiplied by.
//!
//! [`Tint`] is simulation state for the same reason [`Sampling`](crate::Sampling)
//! is: what a frame looks like has to be reproducible from a recording, and a
//! colour kept outside the world would not be. It goes into the canonical dump
//! and therefore into the state hash of any simulation that registers it.
//!
//! # What multiplies what, and in which space
//!
//! The multiplication happens in the fragment shader, on the value the sampler
//! returned. The atlas texture and the render target are both
//! `Rgba8UnormSrgb`, which wgpu documents as "Srgb-color [0, 255] converted
//! to/from linear-color float [0, 1] in shader"
//! (`wgpu-types-30.0.0/src/texture/format.rs:186`), so what the shader
//! multiplies is **linear light**. The space is a property of the formats and
//! not a choice this type makes; `narvo-render2d`'s `SpriteTint` records the
//! measurement that fixed it.
//!
//! The consequence a content author meets is worth stating here rather than
//! leaving to be discovered: `0.5` is half the *light*, not half the stored
//! byte. A white texel tinted `0.5` reads back as **188**, not 128.

use serde::{Deserialize, Serialize};

/// A colour multiplied into an entity's texels.
///
/// # Four bare `f32`, not a colour type
///
/// ADR-0014: a registered component holds bare scalars, because its serde form
/// governs every state hash that includes it. A colour type from a maths or
/// graphics crate would put that crate's serde decisions — field names, tuple
/// versus struct, a newtype's transparency — inside the hash domain ADR-0008
/// defines. Four named `f32` fields have no such freedom.
///
/// # The range, and what falls outside it
///
/// Each channel is meant to lie in `0.0..=1.0`. This type does **not** reject
/// anything outside that, for the reason [`Sampling`](crate::Sampling) does not
/// reject an unknown code and [`Layer`](crate::Layer) does not reject a `NaN`
/// depth: a component is storage, and rejecting here would mean deciding what a
/// partially valid world is.
///
/// What a value above `1.0` costs is stated rather than hidden, because it is
/// not merely "brighter than expected". The pipeline consumes premultiplied
/// colour (ADR-0023), whose invariant is `rgb <= a`; the renderer preserves it
/// only while every channel of the tint is at most one. A tint above one can
/// therefore produce a fragment brighter than its own coverage, and what that
/// does at a half-transparent edge is undefined by this project rather than by
/// the hardware. `narvo-render2d`'s `SpriteTint::premultiplied` is where the
/// arithmetic lives and where that limit is measured.
///
/// # Opt-in, so a world that never mentions it is unchanged
///
/// An entity without a `Tint` is drawn at [`Tint::DEFAULT`], and a world that
/// never registers the type dumps and hashes exactly as it did before this type
/// existed. That is the construction [`Layer`](crate::Layer) and
/// [`Sampling`](crate::Sampling) both use, and
/// `registering_the_tint_type_changes_nothing_for_a_world_that_has_none` below
/// is the proof — a comparison of two registries, never a stored hash
/// (ADR-0008).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tint {
    /// The red multiplier.
    pub red: f32,
    /// The green multiplier.
    pub green: f32,
    /// The blue multiplier.
    pub blue: f32,
    /// The alpha multiplier, which scales coverage and the colour alike.
    pub alpha: f32,
}

impl Tint {
    /// How an entity that carries no `Tint` is drawn.
    ///
    /// All ones, so that adding a `Tint` to one entity of a world does not move
    /// the others — and so that every scene drawn before this type existed keeps
    /// the picture it had. In IEEE-754 a multiplication by `1.0` is the
    /// identity on every finite value, so "keeps the picture it had" is exact
    /// rather than approximate.
    pub const DEFAULT: Self = Self {
        red: 1.0,
        green: 1.0,
        blue: 1.0,
        alpha: 1.0,
    };

    /// An opaque tint of the three colour channels.
    ///
    /// The common case: recolour without changing coverage.
    #[must_use]
    pub const fn rgb(red: f32, green: f32, blue: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    /// A colourless tint that only scales coverage.
    ///
    /// What a fade is. The colour channels stay at one, so the sprite keeps its
    /// hue while the alpha carries it towards invisible.
    #[must_use]
    pub const fn fade(alpha: f32) -> Self {
        Self {
            red: 1.0,
            green: 1.0,
            blue: 1.0,
            alpha,
        }
    }
}

impl Default for Tint {
    /// [`Tint::DEFAULT`].
    ///
    /// Written out rather than derived, for the reason [`Layer`](crate::Layer)
    /// gives: deriving it would make the choice a property of `f32`'s default
    /// instead of a decision this type took — and `f32::default()` is `0.0`,
    /// which would make the derived default a sprite nobody can see.
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Tint;
    use crate::{ComponentRegistry, Transform, World, canonical_dump, state_hash};

    /// Tints worth round-tripping: the identity, a pure channel, a fade, zero,
    /// and two values outside the intended range.
    fn probes() -> [Tint; 6] {
        [
            Tint::DEFAULT,
            Tint::rgb(1.0, 0.0, 0.0),
            Tint::fade(0.5),
            Tint {
                red: 0.0,
                green: 0.0,
                blue: 0.0,
                alpha: 0.0,
            },
            Tint::rgb(2.0, 2.0, 2.0),
            Tint {
                red: -1.0,
                green: 0.25,
                blue: 0.75,
                alpha: 1.5,
            },
        ]
    }

    /// The RON round trip is exact for every probe, in range or out of it.
    ///
    /// Through the actual serialization the registry uses, not through a claim
    /// about it. A component that did not survive its own dump would make every
    /// replay of a tinted world wrong, and the failure would show up as a
    /// picture rather than as an error.
    #[test]
    fn a_tint_round_trips_through_the_real_serialization() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Tint>("tint")
            .expect("a fresh registry accepts the type");

        for tint in probes() {
            let mut world = World::new();
            let entity = world.spawn();
            world.insert(entity, tint).expect("the entity is alive");

            let dumped = canonical_dump(&world, &registry).expect("the world dumps");

            let mut reloaded = World::new();
            let other = reloaded.spawn();
            reloaded.insert(other, tint).expect("the entity is alive");

            assert_eq!(
                dumped,
                canonical_dump(&reloaded, &registry).expect("the world dumps"),
                "{tint:?} did not survive the dump"
            );
        }
    }

    /// Registering the type moves nothing for a world that carries none.
    ///
    /// Two registries over one world, compared against each other — never
    /// against a stored hash (ADR-0008). This is what lets the type be added to
    /// `register_engine_components` without every existing scene's hash moving.
    #[test]
    fn registering_the_tint_type_changes_nothing_for_a_world_that_has_none() {
        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts the type");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts the type");
        with.register_component::<Tint>("tint")
            .expect("a fresh registry accepts the type");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::at(1.0, 2.0))
            .expect("the entity is alive");

        let bare = canonical_dump(&world, &without).expect("the world dumps");
        let registered = canonical_dump(&world, &with).expect("the world dumps");

        assert_eq!(bare, registered);
        assert_eq!(state_hash(&bare), state_hash(&registered));
    }

    /// The identity is all ones, and each constructor says which channels it
    /// leaves there.
    #[test]
    fn the_constructors_leave_the_channels_they_do_not_name_at_one() {
        assert_eq!(Tint::DEFAULT, Tint::rgb(1.0, 1.0, 1.0));
        assert_eq!(Tint::DEFAULT, Tint::fade(1.0));
        assert_eq!(Tint::DEFAULT, Tint::default());

        let red = Tint::rgb(1.0, 0.0, 0.0);
        assert!((red.alpha - 1.0).abs() < f32::EPSILON);

        let half = Tint::fade(0.5);
        assert!((half.red - 1.0).abs() < f32::EPSILON);
        assert!((half.green - 1.0).abs() < f32::EPSILON);
        assert!((half.blue - 1.0).abs() < f32::EPSILON);
    }
}
