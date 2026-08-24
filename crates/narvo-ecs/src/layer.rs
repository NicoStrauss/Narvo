//! Which drawn thing ends up in front of which.
//!
//! [`Layer`] is simulation state for the same reason [`Transform`](crate::Transform)
//! is: what a frame looks like has to be reproducible from a recording, and a
//! draw order kept outside the world would not be. It goes into the canonical
//! dump and therefore into the state hash of any simulation that registers it.

use serde::{Deserialize, Serialize};

/// How far back an entity is drawn. Smaller is further back.
///
/// # Why a component of its own rather than a field on `Transform`
///
/// Three places could hold this, and they differ in what is reproducible
/// rather than in what is visible. M3.10 picked this one:
///
/// - *A sixth field on [`Transform`](crate::Transform).* Rejected. It would
///   make draw order a part of *position*, and nothing downstream treats it as
///   one: `placements_of` in `narvo-app` copies a transform's five fields into
///   `SpritePlacement`, and `Projection` in `narvo-render2d` maps x and y to
///   normalised device coordinates. A sixth field would have to be threaded
///   through both to mean anything, and until it was it would sit in a struct
///   whose readers do not read it. It also changes the canonical form of a
///   component that already exists, where a component of its own changes
///   nothing for a world that does not use it.
/// - *Render state outside the world.* Rejected, and it is the one option that
///   loses a property: a draw order that is not in the world is not in the
///   canonical dump, so a replay could reproduce every position and still
///   compose the frame differently. That is precisely what ADR-0008's hash is
///   the instrument for noticing, and putting the order out of its reach would
///   make the instrument blind to it by construction.
/// - *This.* An entity without a `Layer` is drawn at [`Layer::DEFAULT`], so the
///   component is opt-in and a world that never mentions it dumps and hashes
///   exactly as it did before M3.10.
///
/// # One bare `f32`
///
/// ADR-0014, as for every registered component: a foreign type here would put
/// that library's serde format inside ADR-0008's stability domain. `f32` rather
/// than an integer layer index because inserting something between two existing
/// layers is the ordinary editing operation, and with integers it renumbers
/// everything above it.
///
/// The price is that floating point brings `NaN` and `-0.0`, which have no
/// obvious order. Neither is normalised here — storing something other than
/// what was written is what [`Transform`](crate::Transform) already refuses to
/// do for rotations — so the ordering rule deals with them instead. That rule
/// is not in this crate: it lives with the extraction in `narvo-app`, where
/// ADR-0015 already put the decision about draw order, and it is documented on
/// `placements_of` there.
///
/// # Larger is nearer
///
/// A sprite with `depth = 1.0` is drawn over one with `depth = 0.0`. The
/// direction is a choice and the opposite would have been just as workable; it
/// is written down so that content and code cannot each pick one.
///
/// **Where it is written down is ADR-0004, in that document's second amendment
/// (M3.11).** It still does not *follow* from anything there: ADR-0004 fixes the
/// two y directions and the texture row order, and a depth is a sort key rather
/// than a coordinate, so this is a new convention and not an inherited one — the
/// analogy to the y axis was drafted in M3.10 and struck in the same task. What
/// earns it a place in that document instead of one of its own is the property
/// it shares with a y-flip: reversed, it is invisible against a test image whose
/// sprites cannot be told apart. Until M3.11 no ADR recorded it: it lived in doc
/// comments — this one, the [`Layer::depth`] field below, [`Layer::DEFAULT`],
/// and `placements_of` in `narvo-app` — and in a bullet of `ProjektPlan.md`
/// §6/M3, and this comment said it was stated here in full rather than pointed
/// at.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Draw order. Larger is nearer the viewer and is drawn later.
    pub depth: f32,
}

impl Layer {
    /// Where an entity that carries no `Layer` is drawn.
    ///
    /// Zero, so that adding a `Layer` to one entity of a world does not move
    /// the others: everything that had none is at this depth already, and a
    /// positive value puts the new one in front, a negative one behind.
    pub const DEFAULT: Self = Self { depth: 0.0 };

    /// A layer at `depth`.
    #[must_use]
    pub const fn at(depth: f32) -> Self {
        Self { depth }
    }
}

impl Default for Layer {
    /// [`Layer::DEFAULT`], written out rather than derived.
    ///
    /// `#[derive(Default)]` would give `0.0` here too, so the two agree today.
    /// It is written out anyway, because the value that "no layer" means is a
    /// decision this type owns, and deriving it would make that decision a
    /// property of `f32`'s default instead.
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::Layer;

    /// Values chosen to stress the text round trip rather than to look tidy —
    /// the same set `Transform`'s round trip uses, for the same reasons.
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

    #[test]
    fn the_ron_round_trip_is_bit_exact_on_values_that_stress_it() {
        for value in probes() {
            let original = Layer::at(value);
            let text = ron::to_string(&original).expect("a layer serializes");
            let returned: Layer = ron::from_str(&text)
                .unwrap_or_else(|error| panic!("{text} does not parse back: {error}"));

            println!("{:#010x} -> {text}", value.to_bits());
            assert_eq!(
                original.depth.to_bits(),
                returned.depth.to_bits(),
                "{value:?} did not survive the round trip bit for bit: {:#010x} became \
                 {:#010x}. A depth that loses bits here loses them in the canonical dump \
                 too, and a replay built on it composes a different frame while every \
                 hash still reports agreement.",
                original.depth.to_bits(),
                returned.depth.to_bits()
            );
        }
    }

    /// `-0.0` survives, and the test above can tell that it did.
    ///
    /// The negative control for the one probe whose loss `==` cannot see. If a
    /// formatter dropped the sign, `original == returned` would still hold and
    /// the round-trip test would still pass; the bit comparison is what makes
    /// it not.
    #[test]
    fn a_dropped_sign_on_negative_zero_would_be_visible_in_the_bits() {
        let negative = Layer::at(-0.0);
        let positive = Layer::at(0.0);

        assert_eq!(negative, positive, "`==` cannot tell these apart");
        assert_ne!(
            negative.depth.to_bits(),
            positive.depth.to_bits(),
            "the bit comparison must be able to, or the round-trip test above is \
             checking nothing for this value"
        );
    }

    #[test]
    fn an_entity_without_a_layer_is_treated_as_zero() {
        assert_eq!(Layer::DEFAULT.depth.to_bits(), 0.0_f32.to_bits());
        assert_eq!(Layer::default(), Layer::DEFAULT);
    }

    /// A world that uses no `Layer` dumps and hashes exactly as it did before
    /// this component existed.
    ///
    /// This is M3.10's regression evidence for ADR-0008, and it is stated as a
    /// comparison rather than as a stored value, because ADR-0008 forbids
    /// committing a hash: knowing the type exists — registering it — must not
    /// change the dump of a world in which no entity carries it.
    ///
    /// **It holds by construction today, and that is the point of writing it
    /// down.** `canonical_dump` asks each registered component to serialize
    /// itself for each entity and skips the `None`, so a registered type that
    /// no entity carries contributes nothing. The test pins that property
    /// rather than discovering it: an implementation that emitted an empty
    /// entry, or listed the registry's names as a header, would move every hash
    /// in the project the day a new component type was registered anywhere.
    ///
    /// What it cannot show is that the *bytes* match some earlier release of
    /// this repository. Nothing can: no hash is recorded anywhere to compare
    /// against, by the same ADR. What the two halves of the evidence show
    /// together is that registering `Layer` adds nothing here, and that the
    /// three demo simulations register neither `Layer` nor `Transform` —
    /// `chance.rs`, `input.rs` and `motion.rs` register `Position`, `Velocity`,
    /// `Rng`, `Controls`, `Wobble`, `Ledger` and two `Events` — so their dumps
    /// cannot have moved either.
    #[test]
    fn registering_the_layer_type_changes_nothing_for_a_world_that_has_none() {
        use crate::{ComponentRegistry, Transform, World, canonical_dump, state_hash};

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
            .expect("a fresh registry accepts a first component");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts a first component");
        with.register_component::<Layer>("layer")
            .expect("layer is a second, differently named component");

        let dump_without = canonical_dump(&world, &without).expect("every component is registered");
        let dump_with = canonical_dump(&world, &with).expect("every component is registered");

        assert_eq!(
            dump_without, dump_with,
            "registering a component type that no entity carries changed the \
             canonical dump. Then adding `Layer` to this project would move the \
             state hash of every simulation that merely knows about it, and \
             ADR-0008's promise about what a hash comparison means would be \
             weaker than it says."
        );
        assert_eq!(state_hash(&dump_without), state_hash(&dump_with));
        assert!(
            !dump_with.contains("layer"),
            "the dump of a world with no Layer must not mention one; it reads:\n{dump_with}"
        );
    }
}
