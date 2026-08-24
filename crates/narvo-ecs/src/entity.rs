//! Entity identity: [`EntityId`] and its conversion to and from the hecs handle.

use std::fmt;
use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

/// Handle identifying one entity of a [`World`](crate::World).
///
/// An entity is a slot plus a generation. `index` says which slot, `generation`
/// says how often that slot has been handed out. Despawning frees the slot, and
/// the next spawn may reuse it with the generation raised by one, so a handle
/// kept across a despawn does not silently start addressing a different entity -
/// it becomes permanently invalid and every access through it returns
/// [`EcsError::NoSuchEntity`](crate::EcsError::NoSuchEntity).
///
/// # Ordering
///
/// [`Ord`] sorts by `index` first and by `generation` only within one index.
/// That is the canonical order of everything observable in this engine: entity
/// lists, serialized state, and the state hash M2.2 builds on top of them.
///
/// The obvious shortcut - ordering by the underlying hecs bit pattern - is
/// wrong, and quietly so. `hecs::Entity::to_bits` packs the generation into the
/// *upper* 32 bits, so ordering by bits sorts by generation first and produces
/// an order in which a long-lived entity in slot 0 lands behind a freshly
/// recycled one in slot 900. The field order in this struct is therefore load
/// bearing: `index` is declared first because the derived [`Ord`] compares
/// fields in declaration order. Reordering the fields silently reorders every
/// canonical dump in the engine.
///
/// # Serde representation
///
/// `EntityId` serializes as a struct with two fields, `index` and `generation`,
/// both plain unsigned 32-bit integers (`generation` is never zero). In RON that
/// is `(index: 7, generation: 2)`.
///
/// This representation is part of the crate's stable API rather than an
/// implementation detail. M2.2 hashes over serialized state, so a change to the
/// field names, their order or their types changes every recorded state hash and
/// invalidates every stored replay. It is deliberately *not* the hecs bit
/// pattern: that pattern is documented as unspecified, and pinning the engine's
/// on-disk format to it would tie replay compatibility to a dependency's
/// internals.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntityId {
    // Declaration order is the sort order - see the `Ordering` section above.
    index: u32,
    generation: NonZeroU32,
}

/// Bit position of the generation inside `hecs::Entity::to_bits`.
///
/// hecs documents the bit pattern as unspecified, and this crate nevertheless
/// takes it apart, because the generation is needed for the index-major ordering
/// above and hecs exposes no accessor for it. The assumption is not left to
/// trust: [`EntityId::from_hecs`] checks both halves against hecs' own public
/// [`hecs::Entity::id`] and against a repacking round trip in debug builds, and
/// `the_hecs_bit_layout_is_what_this_module_decodes` pins it in every build. If
/// hecs ever changes the layout, those fail loudly instead of producing entity
/// handles that address the wrong slot.
const GENERATION_SHIFT: u32 = 32;

impl EntityId {
    /// Which slot of the world this handle addresses.
    ///
    /// Unique among simultaneously live entities and reused after a despawn.
    /// On its own it does not identify an entity over time - that is what
    /// [`generation`](Self::generation) is for.
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }

    /// How often this handle's slot has been handed out, counting from one.
    ///
    /// Raised every time the slot is recycled, which is what makes a stale
    /// handle detectable rather than dangerous.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation.get()
    }

    /// Rebuilds a handle from the two numbers a canonical dump prints.
    ///
    /// The one caller is [`first_difference`](crate::first_difference), which
    /// reads `entity 7v2` back out of dump text so a difference can name the
    /// entity structurally instead of as a string.
    ///
    /// `pub(crate)` deliberately. A handle made this way is a *label read out of
    /// text*: it is only meaningful in the world the dump came from, and nothing
    /// here can check that it came from anywhere at all. Exposing a public
    /// constructor would invite fabricating handles and looking them up, which
    /// is how a stale-handle bug gets written; the generation exists precisely
    /// to make that detectable, and hand-built handles undermine it.
    pub(crate) fn from_parts(index: u32, generation: NonZeroU32) -> Self {
        Self { index, generation }
    }

    /// Wraps a hecs handle.
    ///
    /// Part of the facade seam described in ADR-0005 and kept `pub(crate)`: no
    /// `hecs` type appears in this crate's public API.
    pub(crate) fn from_hecs(entity: hecs::Entity) -> Self {
        let bits = entity.to_bits().get();
        let index = bits as u32;
        let generation = NonZeroU32::new((bits >> GENERATION_SHIFT) as u32).expect(
            "hecs generations start at one and only ever grow, so the upper half of \
             the bit pattern is never zero - a zero here means the hecs bit layout \
             changed and GENERATION_SHIFT in narvo-ecs no longer matches it",
        );

        let id = Self { index, generation };

        // Both halves are checked against something hecs guarantees: the lower
        // one against its public `id()` accessor, the upper one against a
        // repacking round trip. `the_hecs_bit_layout_is_what_this_module_decodes`
        // covers the same ground in release builds.
        debug_assert_eq!(
            index,
            entity.id(),
            "the lower half of the hecs bit pattern is no longer the entity index"
        );
        debug_assert_eq!(
            id.to_hecs(),
            entity,
            "repacking the decoded index and generation no longer yields the original hecs entity"
        );

        id
    }

    /// Unwraps back into a hecs handle.
    ///
    /// The counterpart of [`from_hecs`](Self::from_hecs), and the only way a
    /// facade method reaches an entity inside the wrapped world.
    pub(crate) fn to_hecs(self) -> hecs::Entity {
        let bits = (u64::from(self.generation.get()) << GENERATION_SHIFT) | u64::from(self.index);

        hecs::Entity::from_bits(bits).expect(
            "the generation half is non-zero by construction, which is the only \
             condition hecs places on a valid bit pattern",
        )
    }
}

impl fmt::Debug for EntityId {
    /// Prints as `EntityId(7v2)` - slot 7, second use of it.
    ///
    /// The compact form is chosen because these show up in lists of thousands
    /// when a determinism check reports a diff, where a two-field struct dump
    /// per entity buries the one line that differs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({}v{})", self.index, self.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::{EntityId, GENERATION_SHIFT};
    use std::num::NonZeroU32;

    /// Builds an id directly, bypassing any world. Test-only: outside tests an
    /// `EntityId` may only come from a world that actually holds that entity.
    fn id(index: u32, generation: u32) -> EntityId {
        EntityId {
            index,
            generation: NonZeroU32::new(generation).expect("test generations are non-zero"),
        }
    }

    #[test]
    fn ordering_is_by_index_first_and_generation_second() {
        let mut ids = vec![id(6, 1), id(5, 9), id(5, 2), id(6, 4)];
        ids.sort();

        assert_eq!(ids, vec![id(5, 2), id(5, 9), id(6, 1), id(6, 4)]);
    }

    #[test]
    fn ordering_does_not_follow_the_hecs_bit_pattern() {
        // The trap this ordering exists to avoid: slot 5 on its ninth use
        // against slot 6 on its first. By index - the order the engine wants -
        // slot 5 comes first.
        let older_slot = id(5, 9);
        let newer_slot = id(6, 1);
        assert!(older_slot < newer_slot);

        // Ordering by the packed bits instead would answer the other way round,
        // because the generation sits in the upper half and therefore dominates
        // the comparison. Asserting the inversion here means this test fails if
        // anyone ever "simplifies" `Ord` into a comparison over `to_bits`.
        let bits =
            |id: EntityId| (u64::from(id.generation()) << GENERATION_SHIFT) | u64::from(id.index());
        assert!(bits(older_slot) > bits(newer_slot));
    }

    #[test]
    fn the_hecs_bit_layout_is_what_this_module_decodes() {
        let mut world = hecs::World::new();

        let first = world.spawn((1_u32,));
        let decoded = EntityId::from_hecs(first);
        assert_eq!(decoded.index(), first.id());
        assert_eq!(decoded.generation(), 1);
        assert_eq!(decoded.to_hecs(), first);

        // Recycle the slot so the generation actually moves. Without this the
        // upper half is always 1 and a layout change would go unnoticed.
        world.despawn(first).expect("the entity was just spawned");
        let recycled = world.spawn((2_u32,));
        let recycled_id = EntityId::from_hecs(recycled);

        assert_eq!(
            recycled_id.index(),
            decoded.index(),
            "hecs is expected to hand the freed slot straight back"
        );
        assert_eq!(recycled_id.generation(), decoded.generation() + 1);
        assert_eq!(recycled_id.to_hecs(), recycled);
        assert!(decoded < recycled_id);
    }

    #[test]
    fn serde_representation_is_index_and_generation() {
        let original = id(7, 2);

        let text = ron::to_string(&original).expect("an EntityId always serializes");
        assert_eq!(text, "(index:7,generation:2)");

        let parsed: EntityId = ron::from_str(&text).expect("what was just written parses back");
        assert_eq!(parsed, original);
    }

    #[test]
    fn a_zero_generation_is_rejected_on_deserialization() {
        // Not a nicety: generation zero is the one bit pattern hecs refuses, so
        // accepting it here would produce an id that cannot be turned back into
        // a hecs handle without panicking.
        let result = ron::from_str::<EntityId>("(index:7,generation:0)");
        assert!(result.is_err(), "generation zero must not parse");
    }

    #[test]
    fn debug_is_the_compact_slot_and_generation_form() {
        assert_eq!(format!("{:?}", id(7, 2)), "EntityId(7v2)");
    }
}
