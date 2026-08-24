//! How an entity is named on the protocol boundary, and what a bad name says.

use std::error::Error;
use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An entity, named the way the canonical dump spells it: `3v1` is slot 3 on
/// its first use.
///
/// # Why this is not `narvo_ecs::EntityId`
///
/// Because it is not a handle. An `EntityId` is meant to come from a world that
/// actually holds that entity — `EntityId::from_parts` is `pub(crate)` for
/// exactly that reason, and its own documentation says why: a fabricated handle
/// looked up in a world is how a stale-handle defect gets written. A name that
/// arrived over a socket is fabricated by definition. Keeping the two as
/// different types means the step that turns one into the other cannot be
/// forgotten, because it cannot be skipped: it is a conversion, not a move.
///
/// "Meant to" rather than "can only", because that is what is actually true and
/// M6.1 checked it: `EntityId` derives `Deserialize` on a public type, so
/// `ron::from_str::<EntityId>("(index:7,generation:2)")` fabricates one from any
/// crate in the workspace. The `pub(crate)` on `from_parts` closes one door and
/// the derive leaves another open. That is reported rather than changed here —
/// it is `narvo-ecs`'s to decide — and it makes the case for a separate type
/// stronger, not weaker: the guarantee is a convention, so a protocol that
/// leaned on it would be leaning on nothing.
///
/// It is also what keeps this crate a leaf. `narvo-input`, `narvo-audio` and
/// `narvo-physics2d` depend on nothing in this workspace, and a protocol
/// vocabulary that pulled in the ECS would put `hecs` under every future MCP
/// client. ADR-0030 records both halves of that.
///
/// # The spelling is the dump's, deliberately
///
/// `canonical_dump` writes `entity 3v1`, `DumpDifference`'s `Display` writes
/// `entity    3v1`, and `EntityId`'s `Debug` writes `EntityId(3v1)`. An agent
/// reading any of those can paste the token straight into a request. The
/// *reader* is not shared: `narvo_ecs`'s own `parse_entity_line` is private and
/// reads a whole dump line — `entity 3v1`, prefix included — rather than a bare
/// token, so there is nothing here to reuse without widening that crate's API.
///
/// # Ordering
///
/// By `index` first and by `generation` only within one index — the same order
/// `EntityId` uses, and for the same reason: it is the order everything
/// observable in this engine is written in. The field order in this struct is
/// therefore load bearing, because the derived [`Ord`] compares fields in
/// declaration order.
///
/// # On the wire
///
/// A JSON string, not an object: `"3v1"`. So the protocol carries no number
/// here at all, and `generation`'s "never zero" survives the crossing as a
/// parse error rather than as a convention nobody checks.
///
/// # Examples
///
/// ```
/// use narvo_ipc::EntityName;
///
/// let name: EntityName = "3v1".parse()?;
/// assert_eq!(name.index(), 3);
/// assert_eq!(name.generation(), 1);
/// assert_eq!(name.to_string(), "3v1");
///
/// // Generations count from one, so this is not a name.
/// assert!("3v0".parse::<EntityName>().is_err());
/// # Ok::<(), narvo_ipc::ParseEntityNameError>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityName {
    // Declaration order is the sort order - see the `Ordering` section above.
    index: u32,
    generation: NonZeroU32,
}

impl EntityName {
    /// Names the entity in slot `index` on its `generation`-th use.
    ///
    /// Naming an entity is not looking one up: nothing here knows whether that
    /// entity exists, and finding out is the job of whatever holds the world.
    #[must_use]
    pub fn new(index: u32, generation: NonZeroU32) -> Self {
        Self { index, generation }
    }

    /// Which slot this name addresses.
    #[must_use]
    pub fn index(self) -> u32 {
        self.index
    }

    /// How often that slot had been handed out, counting from one.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation.get()
    }
}

impl fmt::Display for EntityName {
    /// The dump's spelling: slot, `v`, generation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

impl fmt::Debug for EntityName {
    /// `EntityName(3v1)`, matching how `EntityId` spells itself.
    ///
    /// Hand-written for the reason that type gives: a response can list
    /// thousands of these, and a two-field struct dump per entity buries the one
    /// that matters. It also keeps every rendering of a name in one spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityName({self})")
    }
}

impl FromStr for EntityName {
    type Err = ParseEntityNameError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (index, generation) =
            text.split_once('v')
                .ok_or_else(|| ParseEntityNameError::NoSeparator {
                    text: text.to_owned(),
                })?;

        let index = index
            .parse::<u32>()
            .map_err(|_| ParseEntityNameError::Slot {
                text: text.to_owned(),
                slot: index.to_owned(),
            })?;

        // Parsed as a `u32` first and narrowed afterwards, so that zero is
        // reported as the rule it breaks rather than as an unreadable number.
        let generation =
            generation
                .parse::<u32>()
                .map_err(|_| ParseEntityNameError::Generation {
                    text: text.to_owned(),
                    generation: generation.to_owned(),
                })?;

        let generation =
            NonZeroU32::new(generation).ok_or_else(|| ParseEntityNameError::ZeroGeneration {
                text: text.to_owned(),
            })?;

        Ok(Self { index, generation })
    }
}

impl Serialize for EntityName {
    /// As a string, in the dump's spelling.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EntityName {
    /// From a string, through [`FromStr`], so the rules are checked once.
    ///
    /// The parse error is handed to serde as a custom error rather than
    /// flattened into a sentence, which is what lets `serde_json` attach the
    /// position inside the request — measured in M6.1 rather than assumed, and
    /// held by `every_malformed_request_is_located` in `error.rs`.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// Why a piece of text is not an entity name.
///
/// # These messages are the product
///
/// A request is written by an agent, and the message is the whole of the
/// feedback it gets. Every variant here answers **what** was wrong and **what to
/// do about it**, and every one is asserted on in a test, wording included —
/// the standard `narvo-scene` set in M4.2 and `narvo-input` was held to in
/// M5.1.
///
/// There is no line and column here, deliberately. This type is handed a bare
/// token and has no idea where in a document it came from; the position is
/// attached one level up, by the deserializer that does know. That is the same
/// split `narvo-input` makes between its `Syntax` variant and its own two
/// rules.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseEntityNameError {
    /// There is no `v` in the text, so there is nothing to split.
    NoSeparator {
        /// The text that was rejected.
        text: String,
    },
    /// The part before the `v` is not a slot number.
    Slot {
        /// The text that was rejected.
        text: String,
        /// The part before the `v`.
        slot: String,
    },
    /// The part after the `v` is not a generation number.
    Generation {
        /// The text that was rejected.
        text: String,
        /// The part after the `v`.
        generation: String,
    },
    /// The generation is zero, which no entity has.
    ZeroGeneration {
        /// The text that was rejected.
        text: String,
    },
}

/// How a name is built, written once.
///
/// Three of the four messages end in it. Two copies of a rule are two chances to
/// describe it differently.
const SHAPE: &str = "an entity name is a slot, the letter `v`, and a generation, spelled the way \
                     the canonical dump spells it — `3v1` is slot 3 on its first use";

impl fmt::Display for ParseEntityNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSeparator { text } => {
                write!(
                    f,
                    "\"{text}\" is not an entity name: it has no `v`; {SHAPE}"
                )
            }
            Self::Slot { text, slot } => write!(
                f,
                "\"{text}\" is not an entity name: \"{slot}\" is not a slot number; {SHAPE}"
            ),
            Self::Generation { text, generation } => write!(
                f,
                "\"{text}\" is not an entity name: \"{generation}\" is not a generation number; \
                 {SHAPE}"
            ),
            Self::ZeroGeneration { text } => write!(
                f,
                "\"{text}\" is not an entity name: generations count from one, so no entity has \
                 generation 0. The first use of a slot is `v1`"
            ),
        }
    }
}

impl Error for ParseEntityNameError {}

#[cfg(test)]
mod tests {
    use super::{EntityName, ParseEntityNameError};
    use std::num::NonZeroU32;

    fn name(index: u32, generation: u32) -> EntityName {
        EntityName::new(
            index,
            NonZeroU32::new(generation).expect("test generations are non-zero"),
        )
    }

    #[test]
    fn a_name_reads_back_as_what_it_prints() {
        let original = name(3, 1);
        assert_eq!(original.to_string(), "3v1");
        assert_eq!("3v1".parse::<EntityName>().expect("just printed"), original);
    }

    #[test]
    fn a_name_survives_the_wire_as_a_json_string() {
        let original = name(4294967295, 7);
        let json = serde_json::to_string(&original).expect("a name always serializes");
        assert_eq!(json, "\"4294967295v7\"");
        assert_eq!(
            serde_json::from_str::<EntityName>(&json).expect("just written"),
            original
        );
    }

    #[test]
    fn ordering_is_by_slot_first_and_generation_second() {
        // The same trap `EntityId`'s own ordering test names: slot 5 on its
        // ninth use comes before slot 6 on its first.
        let mut names = vec![name(6, 1), name(5, 9), name(5, 2), name(6, 4)];
        names.sort();

        assert_eq!(names, vec![name(5, 2), name(5, 9), name(6, 1), name(6, 4)]);
    }

    #[test]
    fn debug_is_the_compact_slot_and_generation_form() {
        assert_eq!(format!("{:?}", name(7, 2)), "EntityName(7v2)");
    }

    #[test]
    fn a_text_without_a_v_says_so_and_says_what_a_name_looks_like() {
        let error = "37".parse::<EntityName>().expect_err("no separator");
        assert_eq!(
            error,
            ParseEntityNameError::NoSeparator {
                text: "37".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "\"37\" is not an entity name: it has no `v`; an entity name is a slot, the letter \
             `v`, and a generation, spelled the way the canonical dump spells it — `3v1` is slot \
             3 on its first use"
        );
    }

    #[test]
    fn a_slot_that_is_not_a_number_names_the_slot() {
        let error = "xv1".parse::<EntityName>().expect_err("x is not a slot");
        assert_eq!(
            error,
            ParseEntityNameError::Slot {
                text: "xv1".to_owned(),
                slot: "x".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "\"xv1\" is not an entity name: \"x\" is not a slot number; an entity name is a slot, \
             the letter `v`, and a generation, spelled the way the canonical dump spells it — \
             `3v1` is slot 3 on its first use"
        );
    }

    #[test]
    fn a_generation_that_is_not_a_number_names_the_generation() {
        // `3v1v2` splits at the first `v`, so the generation is `1v2`.
        let error = "3v1v2"
            .parse::<EntityName>()
            .expect_err("1v2 is not a generation");
        assert_eq!(
            error,
            ParseEntityNameError::Generation {
                text: "3v1v2".to_owned(),
                generation: "1v2".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "\"3v1v2\" is not an entity name: \"1v2\" is not a generation number; an entity name \
             is a slot, the letter `v`, and a generation, spelled the way the canonical dump \
             spells it — `3v1` is slot 3 on its first use"
        );
    }

    #[test]
    fn generation_zero_is_rejected_as_the_rule_it_breaks() {
        // Not a nicety: generation zero is the one bit pattern hecs refuses, so
        // a name carrying it could never address an entity.
        let error = "3v0".parse::<EntityName>().expect_err("no generation zero");
        assert_eq!(
            error,
            ParseEntityNameError::ZeroGeneration {
                text: "3v0".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "\"3v0\" is not an entity name: generations count from one, so no entity has \
             generation 0. The first use of a slot is `v1`"
        );
    }

    #[test]
    fn an_empty_text_is_rejected_as_a_missing_separator() {
        assert_eq!(
            "".parse::<EntityName>().expect_err("empty"),
            ParseEntityNameError::NoSeparator {
                text: String::new()
            }
        );
    }

    #[test]
    fn a_number_too_large_for_a_slot_is_rejected() {
        let error = "4294967296v1"
            .parse::<EntityName>()
            .expect_err("one past u32::MAX");
        assert_eq!(
            error,
            ParseEntityNameError::Slot {
                text: "4294967296v1".to_owned(),
                slot: "4294967296".to_owned()
            }
        );
    }
}
