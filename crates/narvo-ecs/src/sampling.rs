//! How a drawn thing wants its texture read.
//!
//! [`Sampling`] is simulation state for the same reason [`Layer`](crate::Layer)
//! is: what a frame looks like has to be reproducible from a recording, and a
//! sampler choice kept outside the world would not be. It goes into the
//! canonical dump and therefore into the state hash of any simulation that
//! registers it.
//!
//! **This became true in M3.23 and was not true before.** M3.20 introduced the
//! wish as a field on the renderer's own `Sprite` and recorded, in as many
//! words, that making it simulation state was "an obligation, not built" —
//! because the renderer honoured `Nearest` only, so the wish could not change a
//! picture and a replay could not miss it. The second sampler is what makes the
//! obligation fall due.

use serde::{Deserialize, Serialize};

/// How an entity's texture is sampled.
///
/// # One bare integer, not an enum
///
/// ADR-0014: a registered component holds bare scalars, because its serde form
/// governs every state hash that includes it. An enum's RON representation is
/// serde's to choose — `Nearest`, `(Nearest)`, `0`, depending on
/// representation attributes and on serde's own version — and that would put a
/// dependency's formatting decision inside the hash domain ADR-0008 defines.
/// An integer with the mapping written down here has no such freedom.
///
/// The mapping, and it is the whole contract:
///
/// | value | meaning |
/// | --- | --- |
/// | [`Sampling::NEAREST`] = 0 | one texel, unblended |
/// | [`Sampling::LINEAR`] = 1 | bilinear blend of the four nearest texels |
/// | anything else | reserved; a reader treats it as `NEAREST` |
///
/// **The reserved row is deliberate and is not validation.** This type does not
/// reject an unknown code, exactly as [`Layer`](crate::Layer) does not reject a
/// `NaN` depth: a component is storage, and rejecting here would mean deciding
/// what a partially valid world is. The renderer-side mapping is where an
/// unknown code becomes `Nearest`, in `narvo-app`, which is the crate that
/// sees both the world and the renderer (ADR-0015).
///
/// # Opt-in, so a world that never mentions it is unchanged
///
/// An entity without a `Sampling` is drawn at [`Sampling::DEFAULT`], and a world
/// that never registers the type dumps and hashes exactly as it did before
/// M3.23. That is the same construction `Layer` uses, and
/// `registering_the_sampling_type_changes_nothing_for_a_world_that_has_none`
/// below is the proof — a comparison of two registries, never a stored hash
/// (ADR-0008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sampling {
    /// The sampler code. See the table on [`Sampling`].
    pub filter: u8,
}

impl Sampling {
    /// One texel, unblended.
    pub const NEAREST: u8 = 0;

    /// A bilinear blend of the four nearest texels.
    pub const LINEAR: u8 = 1;

    /// How an entity that carries no `Sampling` is drawn.
    ///
    /// `NEAREST`, so that adding a `Sampling` to one entity of a world does not
    /// move the others — and so that every scene drawn before M3.23 keeps the
    /// picture it had.
    pub const DEFAULT: Self = Self {
        filter: Self::NEAREST,
    };

    /// The unblended sampling.
    #[must_use]
    pub const fn nearest() -> Self {
        Self::DEFAULT
    }

    /// The bilinear sampling.
    #[must_use]
    pub const fn linear() -> Self {
        Self {
            filter: Self::LINEAR,
        }
    }
}

impl Default for Sampling {
    /// [`Sampling::DEFAULT`].
    ///
    /// Written out rather than derived, for the reason `Layer` gives: deriving
    /// it would make the choice a property of `u8`'s default instead of a
    /// decision this type took. That `u8::default()` happens to be 0 as well is
    /// a coincidence this type does not want to rely on.
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Sampling;
    use crate::{ComponentRegistry, Transform, World, canonical_dump, state_hash};

    /// Every code the mapping names, plus codes it does not.
    fn probes() -> [u8; 6] {
        [
            Sampling::NEAREST,
            Sampling::LINEAR,
            2,
            7,
            u8::MAX,
            u8::MAX - 1,
        ]
    }

    /// The RON round trip is exact for every code, named or reserved.
    ///
    /// Through the actual serialization, not through a claim about it: `ron`'s
    /// own `to_string` and `from_str`, which is what the component registry
    /// uses.
    #[test]
    fn the_ron_round_trip_is_exact_on_every_code() {
        for filter in probes() {
            let original = Sampling { filter };
            let text = ron::to_string(&original).expect("a struct of one u8 serializes");
            let returned: Sampling = ron::from_str(&text).expect("what we just wrote parses");

            assert_eq!(
                original, returned,
                "the round trip lost something for code {filter}: wrote {text}"
            );
        }
    }

    /// The serialized form is the integer, not the name of a variant.
    ///
    /// The point of ADR-0014 in one assertion: whatever serde would have done
    /// with an enum, this type's dump is a number, and a number has no
    /// representation for a dependency to change its mind about.
    #[test]
    fn the_dump_is_an_integer_and_not_a_variant_name() {
        let text = ron::to_string(&Sampling::linear()).expect("serializes");

        assert!(
            text.contains('1'),
            "the linear code should appear as the digit 1: {text}"
        );
        assert!(
            !text.contains("Linear") && !text.contains("linear"),
            "no variant name may reach the dump, or serde's enum representation \
             would be inside the state hash: {text}"
        );
    }

    /// The two codes are the two integers the doc table names.
    ///
    /// They *are* the serialized form, so changing either silently changes the
    /// state hash of every world that carries the component and re-reads an
    /// older recording as something else. Pinned here so that a change to them
    /// is a change to a test.
    #[test]
    fn the_codes_are_the_ones_the_table_writes_down() {
        assert_eq!(Sampling::NEAREST, 0);
        assert_eq!(Sampling::LINEAR, 1);
        assert_eq!(
            ron::to_string(&Sampling::nearest()).expect("serializes"),
            "(filter:0)"
        );
        assert_eq!(
            ron::to_string(&Sampling::linear()).expect("serializes"),
            "(filter:1)"
        );
    }

    /// An entity without a `Sampling` samples as `Nearest`.
    #[test]
    fn an_entity_without_a_sampling_is_treated_as_nearest() {
        assert_eq!(Sampling::DEFAULT.filter, Sampling::NEAREST);
        assert_eq!(Sampling::default(), Sampling::DEFAULT);
        assert_eq!(Sampling::nearest(), Sampling::DEFAULT);
        assert_eq!(Sampling::linear().filter, Sampling::LINEAR);
        assert_ne!(Sampling::nearest(), Sampling::linear());
    }

    /// Registering the type changes nothing for a world that carries none.
    ///
    /// The `Layer` construction of M3.10, and the reason it is written down
    /// rather than assumed: it is what lets `Sampling` exist without moving any
    /// existing simulation's state hash. **No hash value is stored** (ADR-0008);
    /// two registries are compared over one world.
    ///
    /// The other half of the evidence is that the three demo simulations
    /// register neither `Sampling` nor `Transform` — `chance.rs`, `input.rs` and
    /// `motion.rs` register `Position`, `Velocity`, `Rng`, `Controls`,
    /// `Wobble`, `Ledger` and two `Events` — so their dumps cannot have moved
    /// either.
    #[test]
    fn registering_the_sampling_type_changes_nothing_for_a_world_that_has_none() {
        let mut world = World::new();
        for index in 0..4_u8 {
            let entity = world.spawn();
            world
                .insert(entity, Transform::at(f32::from(index), 0.0))
                .expect("the entity was just spawned");
        }

        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts this component");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts this component");
        with.register_component::<Sampling>("sampling")
            .expect("a fresh registry accepts this component");

        let dump_without = canonical_dump(&world, &without).expect("every component is registered");
        let dump_with = canonical_dump(&world, &with).expect("every component is registered");

        assert_eq!(
            dump_without, dump_with,
            "registering `Sampling` changed the dump of a world that carries none"
        );
        assert_eq!(state_hash(&dump_without), state_hash(&dump_with));
        assert!(
            !dump_with.contains("sampling"),
            "a type no entity carries must not appear in the dump: {dump_with}"
        );
    }

    /// And a world that *does* carry it hashes differently when it changes.
    ///
    /// The other direction, without which the test above would be satisfied by
    /// a component the dump ignores entirely.
    #[test]
    fn changing_a_carried_sampling_changes_the_hash() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts this component");
        registry
            .register_component::<Sampling>("sampling")
            .expect("a fresh registry accepts this component");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::IDENTITY)
            .expect("the entity was just spawned");
        world
            .insert(entity, Sampling::nearest())
            .expect("the entity was just spawned");
        let before = canonical_dump(&world, &registry).expect("every component is registered");

        world
            .insert(entity, Sampling::linear())
            .expect("the entity is alive");
        let after = canonical_dump(&world, &registry).expect("every component is registered");

        assert_ne!(
            before, after,
            "a changed sampler wish must reach the dump, or a replay could draw \
             a different picture and the state hash would not notice"
        );
        assert_ne!(state_hash(&before), state_hash(&after));
    }
}
