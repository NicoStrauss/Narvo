//! Which packed region an entity draws.
//!
//! [`Sprite`] is simulation state for the same reason [`Sampling`](crate::Sampling)
//! and [`Layer`](crate::Layer) are: what a frame looks like has to be
//! reproducible from a recording, and a picture chosen outside the world would
//! not be. It goes into the canonical dump and therefore into the state hash of
//! any simulation that registers it.
//!
//! **It is the eighth registered type, and the first that holds a string.** The
//! seven before it hold nothing but numbers. What that costs and why it is
//! nevertheless the right shape is on [`Sprite::region`].

use serde::{Deserialize, Serialize};

/// The name of the atlas region this entity draws.
///
/// # The naming convention, and why this type has the plain name (M4.9)
///
/// **A component owns the plain name; a renderer type carries its role.**
/// `narvo-render2d` had a `Sprite` before this component existed, and for one
/// milestone `narvo-app` — the crate that sees both — imported this one as
/// `Sprite as SpriteComponent`. M4.9 renamed the renderer's to
/// `SpriteInstance` and left this one alone.
///
/// The plain name came here because **this is what content says**: a scene file
/// writes `"sprite"`, the registry knows it under that name, and the state hash
/// covers it. A type whose name an author already types should be spelled the
/// way they type it. The renderer's is an instance handed to a draw call, built
/// per frame and gone by the next, and its name now says so.
///
/// # A name, not a rectangle
///
/// The obvious alternative is to store the four texel coordinates directly, and
/// it is worth saying why they are not here. A rectangle is a fact about **one
/// packing** — where the packer happened to put a region on the day it ran — and
/// ADR-0020 makes the packing an output rather than an input: add an asset,
/// change a padding rule, and every rectangle moves. A scene holding rectangles
/// would have to be rewritten by a tool whenever the atlas was rebuilt, and a
/// recording made against it would describe a world nobody could reconstruct.
///
/// A name survives all of that. It is what the contract already speaks: an
/// `Atlas` is a name-to-placement table (`narvo_assets::Atlas::region`), and the
/// name is the file stem the region was loaded from.
///
/// # The first string in a registered component
///
/// ADR-0014 requires a registered component to hold bare scalars, and its
/// reasoning is about **a dependency's formatting decisions entering the hash
/// domain**: a maths library's vector type or an enum's RON representation is
/// serde's to render, and would put that choice inside ADR-0008's stability
/// domain.
///
/// A `String` is not that case, and the difference is worth stating rather than
/// waving at. `String`'s serde representation is not a library's design choice
/// that could sensibly be made differently — it is a string, RON writes it as a
/// quoted string, and there is no representation attribute that would change it.
/// What it does bring is **escaping**, which is a real surface: a name
/// containing a quote or a backslash is written escaped and read back
/// unescaped. That round trip is checked directly rather than assumed, on
/// adversarial names, by `a_region_name_survives_the_ron_round_trip_with_anything_in_it`.
///
/// # Unresolvable is not this type's problem
///
/// A name that no atlas carries is **not** rejected here, exactly as
/// [`Sampling`](crate::Sampling) does not reject an unknown filter code and
/// [`Layer`](crate::Layer) does not reject a `NaN` depth. A component is
/// storage. Resolution happens where the world and the atlas are both in scope —
/// `narvo-app` — and that is where an unknown name becomes an error naming the
/// ones that exist.
///
/// # Opt-in, so a world that never mentions it is unchanged
///
/// An entity without a `Sprite` draws nothing in the mode that consumes this,
/// and a world that never registers the type dumps and hashes exactly as it did
/// before. That is the construction every optional component here uses, and
/// `registering_the_sprite_type_changes_nothing_for_a_world_that_has_none` is
/// the proof — a comparison of two registries, never a stored hash (ADR-0008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sprite {
    /// The region name, which is the file stem the region was loaded from.
    pub region: String,
}

impl Sprite {
    /// A sprite drawing the region called `region`.
    #[must_use]
    pub fn new(region: impl Into<String>) -> Self {
        Self {
            region: region.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Sprite;
    use crate::{ComponentRegistry, World, canonical_dump};

    #[test]
    fn a_sprite_round_trips_through_the_real_serialization() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Sprite>("sprite")
            .expect("a fresh registry accepts the type");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Sprite::new("icon"))
            .expect("just spawned");

        let text = registry
            .serialize_component("sprite", &world, entity)
            .expect("the registry knows the name")
            .expect("the entity carries the component");
        assert_eq!(text.trim(), "(region:\"icon\")");

        let mut second = World::new();
        let other = second.spawn();
        registry
            .deserialize_component("sprite", &mut second, other, &text)
            .expect("what was written can be read");
        assert_eq!(
            *second.get::<Sprite>(other).expect("just inserted"),
            Sprite::new("icon")
        );
    }

    /// The escaping surface a `String` brings, checked on names that exercise
    /// it rather than on the ones a file system would produce.
    ///
    /// None of these can be a file stem on Windows, and that is the point: the
    /// component does not know where its name came from, so the round trip has
    /// to hold for whatever a scene author writes. If RON's escaping ever
    /// stopped being symmetric, this is what would say so — and it would say so
    /// here rather than as a corrupted region name in a picture.
    #[test]
    fn a_region_name_survives_the_ron_round_trip_with_anything_in_it() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Sprite>("sprite")
            .expect("a fresh registry accepts the type");

        for name in [
            "icon",
            "with a space",
            "with\"a quote",
            "with\\a backslash",
            "with\na newline",
            "wíth ünicode",
            "",
        ] {
            let mut world = World::new();
            let entity = world.spawn();
            world
                .insert(entity, Sprite::new(name))
                .expect("just spawned");

            let text = registry
                .serialize_component("sprite", &world, entity)
                .expect("the registry knows the name")
                .expect("the entity carries the component");

            let mut second = World::new();
            let other = second.spawn();
            registry
                .deserialize_component("sprite", &mut second, other, &text)
                .unwrap_or_else(|error| {
                    panic!("{name:?} serialized to {text:?} and would not read back: {error}")
                });

            assert_eq!(
                second.get::<Sprite>(other).expect("just inserted").region,
                name,
                "{name:?} did not survive, having been written as {text:?}"
            );
        }
    }

    /// Registering the type changes nothing for a world that carries none.
    ///
    /// The same construction every optional component here uses, and the same
    /// proof shape: two registries compared against each other, never a stored
    /// hash (ADR-0008).
    #[test]
    fn registering_the_sprite_type_changes_nothing_for_a_world_that_has_none() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, crate::Layer::at(2.0))
            .expect("just spawned");

        let mut without = ComponentRegistry::new();
        without
            .register_component::<crate::Layer>("layer")
            .expect("a fresh registry accepts the type");

        let mut with = ComponentRegistry::new();
        with.register_component::<crate::Layer>("layer")
            .expect("a fresh registry accepts the type");
        with.register_component::<Sprite>("sprite")
            .expect("a fresh registry accepts the type");

        assert_eq!(
            canonical_dump(&world, &without).expect("every component is registered"),
            canonical_dump(&world, &with).expect("every component is registered"),
            "a world with no sprite dumps differently once the type is known"
        );
    }
}
