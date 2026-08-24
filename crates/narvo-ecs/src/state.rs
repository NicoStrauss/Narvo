//! Canonical serialization of a whole world, and the hash over it.
//!
//! What ADR-0008 governs and this module implements: a world has exactly one
//! textual form, and two runs that agree on it agree on their simulation. Read
//! that ADR before changing anything here — in particular the rule that no hash
//! value is ever committed to this repository.

use std::fmt;
use std::fmt::Write as _;
use std::num::NonZeroU32;

use crate::entity::EntityId;
use crate::error::EcsError;
use crate::registry::ComponentRegistry;
use crate::world::World;

/// Starting value of the FNV-1a hash.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// Multiplier of the FNV-1a hash.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Writes the entire world as one canonical string.
///
/// Canonical means every degree of freedom is nailed down, because anything
/// left free is something two runs can disagree on without disagreeing about
/// the simulation:
///
/// - entities ascending by [`EntityId`], which is index-major (see its own
///   documentation for why that ordering and not the storage engine's),
/// - components within an entity ascending by their stable registry name,
/// - every component through the registry's type-erased path, so no type gets
///   its own spelling,
/// - no hash map is consulted anywhere along the way.
///
/// Query iteration order — archetype order — appears nowhere in it. That is the
/// point: it is the one order in this engine that is *not* reproducible, and
/// this function is what neutralises it.
///
/// # Format
///
/// ```text
/// entities 2
/// entity 0v1
///   position (x:1,y:2)
///   velocity (dx:3,dy:4)
/// entity 1v1
///   position (x:5,y:6)
/// ```
///
/// Line-based and `\n`-terminated on every platform, so a `diff` between two
/// runs compares simulations rather than operating systems. A component an
/// entity does not have is left out rather than written as absent; the entity
/// count in the first line is there so that a truncated dump shows up in the
/// first line of a diff instead of as a missing block far below.
///
/// # Errors
///
/// [`EcsError::UnregisteredComponent`] if any entity carries a component type
/// the registry does not know. That is a hard failure on purpose: a component
/// outside the dump is outside the hash, and a divergence in it would compare
/// as identical — the same shape of defect as a green test that verifies
/// nothing. Serialization failures from a component's own `Serialize` surface
/// as [`EcsError::ComponentSerialization`].
pub fn canonical_dump(world: &World, registry: &ComponentRegistry) -> Result<String, EcsError> {
    let entities = world.entity_ids();

    let mut dump = String::new();
    writeln!(dump, "entities {}", entities.len()).expect("writing into a String cannot fail");

    for entity in entities {
        reject_unregistered(world, registry, entity)?;

        writeln!(dump, "entity {}v{}", entity.index(), entity.generation())
            .expect("writing into a String cannot fail");

        // The registry iterates in stable-name order, so the components of one
        // entity come out sorted without sorting anything here.
        for info in registry.iter() {
            if let Some(serialized) = info.serialize(world, entity)? {
                writeln!(dump, "  {} {}", info.name(), serialized)
                    .expect("writing into a String cannot fail");
            }
        }
    }

    Ok(dump)
}

/// Fails if `entity` carries a component type the registry does not know.
fn reject_unregistered(
    world: &World,
    registry: &ComponentRegistry,
    entity: EntityId,
) -> Result<(), EcsError> {
    for type_id in world.component_type_ids(entity)? {
        if registry.name_of_type(type_id).is_none() {
            return Err(EcsError::UnregisteredComponent {
                entity,
                // The world records a name for every type that ever reached it
                // through `insert`, which is the only route there is. The
                // fallback exists so a future route cannot turn a diagnosable
                // error into a panic.
                component: world
                    .component_type_name(type_id)
                    .map_or_else(|| format!("component of type {type_id:?}"), str::to_owned),
            });
        }
    }

    Ok(())
}

/// Hashes a canonical dump into 64 bits.
///
/// FNV-1a, written out here rather than taken from a crate. ADR-0008 explains
/// the reasoning in full; the short form is that the guarantee wanted from this
/// function is *stability*, and stability comes from the constants being frozen
/// in this file where changing them is a visible diff. `std::hash::DefaultHasher`
/// is disqualified because its own documentation reserves the right to change
/// the algorithm between releases.
///
/// # What this is for
///
/// Answering "did two runs diverge" in one cheap comparison instead of a string
/// compare over the whole dump. It does not answer "is this state correct" and
/// cannot: it is a lossy summary, and nothing here knows what the right answer
/// would be. The moment two hashes disagree, the dumps are the tool — that is
/// why [`canonical_dump`] is public and returns the text rather than hiding it.
///
/// A hash value must never be committed to this repository, in a test, in a
/// document or in a fixture. Two runs are compared against each other.
#[must_use]
pub fn state_hash(dump: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;

    for byte in dump.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }

    hash
}

/// Where two canonical dumps first stop agreeing.
///
/// Returned by [`first_difference`]. Every field is what somebody diagnosing a
/// divergence actually needs: not "these two states differ" but *which entity*,
/// *which component*, and *what each side says*.
///
/// # Reading the fields
///
/// [`line`](Self::line) counts from one and refers to both dumps, which share a
/// prefix up to that point. [`entity`](Self::entity) is the entity block the
/// line falls inside — `None` only for a difference in the header line before
/// any entity. [`component`](Self::component) is the stable registry name when
/// the differing line is a component line, and `None` when it is an entity or
/// header line. [`left`](Self::left) and [`right`](Self::right) are the two
/// lines; one of them is `None` when that dump simply ended.
///
/// The `entity` is a handle *read out of text*. It identifies an entity in the
/// world the dump came from and nothing checks that it came from anywhere — see
/// the note on that in `EntityId`'s internals. Treat it as a label, and look it
/// up only in the world you know produced the dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpDifference {
    line: usize,
    entity: Option<EntityId>,
    component: Option<String>,
    left: Option<String>,
    right: Option<String>,
}

impl DumpDifference {
    /// Which line differs, counting from one.
    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }

    /// The entity whose block the differing line falls in.
    #[must_use]
    pub fn entity(&self) -> Option<EntityId> {
        self.entity
    }

    /// The stable component name, when the differing line is a component line.
    #[must_use]
    pub fn component(&self) -> Option<&str> {
        self.component.as_deref()
    }

    /// The left dump's line, or `None` if the left dump ended before here.
    #[must_use]
    pub fn left(&self) -> Option<&str> {
        self.left.as_deref()
    }

    /// The right dump's line, or `None` if the right dump ended before here.
    #[must_use]
    pub fn right(&self) -> Option<&str> {
        self.right.as_deref()
    }
}

impl fmt::Display for DumpDifference {
    /// A block a person can read, for callers that just want to print it.
    ///
    /// A convenience, not the API: anything that needs to *act* on a difference
    /// — an assertion message that adds its own label, a future introspection
    /// service that answers structurally — reads the accessors instead. That is
    /// why [`first_difference`] returns a value and formats nothing itself.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "line      {}", self.line)?;
        // Spelled the way the dump spells it - `1v1`, not `EntityId(1v1)` - so
        // the line can be searched for in the dump it came from.
        match self.entity {
            Some(entity) => {
                writeln!(f, "entity    {}v{}", entity.index(), entity.generation())?;
            }
            None => writeln!(f, "entity    (before the first entity line)")?,
        }
        if let Some(component) = &self.component {
            writeln!(f, "component {component}")?;
        }
        writeln!(f, "left      {}", Self::side(self.left.as_deref()))?;
        write!(f, "right     {}", Self::side(self.right.as_deref()))
    }
}

impl DumpDifference {
    /// Renders one side, naming the case where that dump simply stopped.
    fn side(line: Option<&str>) -> &str {
        line.unwrap_or("(this dump ends here)")
    }
}

/// Finds the first place two canonical dumps disagree.
///
/// Returns `None` when they are identical — equality is an ordinary answer, not
/// an error, so there is no error type here and nothing to unwrap.
///
/// This is the reading half of [`canonical_dump`]. It lives beside it because
/// the dump format is this crate's, and a tool that parses it has to move when
/// the format does. The alternative — every consumer writing its own line walk —
/// is what this workspace had until M2.6, in three copies.
///
/// # Why a value and not a message
///
/// A caller that prints wants one shape; an assertion that adds its own label
/// wants another; the introspection service of M6 wants neither and needs the
/// entity and the component as data. Formatting is therefore the caller's, with
/// [`Display`](fmt::Display) available for the common case.
///
/// # What it does not do
///
/// It reports the *first* difference, not all of them, and it does not align or
/// resynchronise: two dumps that differ by an inserted line are reported at that
/// line, which is correct but says nothing about the rest. That is the right
/// answer for a determinism comparison, where the first divergence is the one
/// worth chasing and everything after it is downstream of it.
///
/// # Examples
///
/// ```
/// use narvo_ecs::first_difference;
///
/// let before = "entities 2\nentity 0v1\n  position (x:1,y:2)\nentity 1v1\n  position (x:5,y:6)\n";
/// let after  = "entities 2\nentity 0v1\n  position (x:1,y:2)\nentity 1v1\n  position (x:5,y:7)\n";
///
/// assert_eq!(first_difference(before, before), None);
///
/// let difference = first_difference(before, after).expect("these two differ");
/// assert_eq!(difference.line(), 5);
/// assert_eq!(difference.component(), Some("position"));
/// assert_eq!(difference.entity().map(|entity| entity.index()), Some(1));
/// assert_eq!(difference.left(), Some("  position (x:5,y:6)"));
/// assert_eq!(difference.right(), Some("  position (x:5,y:7)"));
/// ```
#[must_use]
pub fn first_difference(left: &str, right: &str) -> Option<DumpDifference> {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();
    let mut entity = None;

    for (index, (a, b)) in left_lines.iter().zip(right_lines.iter()).enumerate() {
        // Tracked from the left side. When the differing line is itself an
        // entity line the two sides name different entities, and reporting the
        // left one is a choice rather than a fact - the caller has both lines.
        if let Some(found) = parse_entity_line(a) {
            entity = Some(found);
        }

        if a == b {
            continue;
        }

        return Some(DumpDifference {
            line: index + 1,
            entity,
            component: component_name(a),
            left: Some((*a).to_owned()),
            right: Some((*b).to_owned()),
        });
    }

    if left_lines.len() == right_lines.len() {
        return None;
    }

    // One side is a prefix of the other. Without this the walk above would run
    // out of pairs and report agreement, which is how a truncated dump would
    // compare as identical.
    let common = left_lines.len().min(right_lines.len());
    let extra = left_lines.get(common).or_else(|| right_lines.get(common));

    Some(DumpDifference {
        line: common + 1,
        entity,
        component: extra.and_then(|line| component_name(line)),
        left: left_lines.get(common).map(|line| (*line).to_owned()),
        right: right_lines.get(common).map(|line| (*line).to_owned()),
    })
}

/// Reads `entity 7v2` back into a handle, or `None` for any other line.
fn parse_entity_line(line: &str) -> Option<EntityId> {
    let (index, generation) = line.strip_prefix("entity ")?.split_once('v')?;

    Some(EntityId::from_parts(
        index.parse().ok()?,
        NonZeroU32::new(generation.parse().ok()?)?,
    ))
}

/// The stable name on a component line, or `None` for any other line.
///
/// A component line is the indented one — `canonical_dump` writes two spaces
/// before the name — so the indent is what tells the three kinds of line apart
/// without parsing the rest.
fn component_name(line: &str) -> Option<String> {
    line.strip_prefix("  ")?
        .split(' ')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{canonical_dump, first_difference, state_hash};
    use crate::{ComponentRegistry, EcsError, EntityId, World};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize)]
    struct Position {
        x: i64,
        y: i64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Velocity {
        dx: i64,
        dy: i64,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Label(String);

    /// A component nobody registers, for the unregistered-component test.
    #[derive(Debug, Serialize, Deserialize)]
    struct Forgotten {
        important: i32,
    }

    fn registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        // Deliberately not in alphabetical order: the registry sorts, and the
        // dump inherits that order rather than this one.
        registry
            .register_component::<Velocity>("velocity")
            .expect("first");
        registry
            .register_component::<Label>("label")
            .expect("first");
        registry
            .register_component::<Position>("position")
            .expect("first");
        registry
    }

    /// Two entities, the second without a velocity.
    fn world() -> (World, EntityId, EntityId) {
        let mut world = World::new();

        let first = world.spawn();
        world.insert(first, Position { x: 1, y: 2 }).expect("alive");
        world
            .insert(first, Velocity { dx: 3, dy: 4 })
            .expect("alive");
        world.insert(first, Label("one".to_owned())).expect("alive");

        let second = world.spawn();
        world
            .insert(second, Position { x: 5, y: 6 })
            .expect("alive");

        (world, first, second)
    }

    #[test]
    fn the_dump_is_ordered_by_entity_then_by_stable_component_name() {
        let (world, _first, _second) = world();
        let registry = registry();

        let dump = canonical_dump(&world, &registry).expect("everything is registered");

        assert_eq!(
            dump,
            "entities 2\n\
             entity 0v1\n\
             \x20 label (\"one\")\n\
             \x20 position (x:1,y:2)\n\
             \x20 velocity (dx:3,dy:4)\n\
             entity 1v1\n\
             \x20 position (x:5,y:6)\n"
        );
    }

    #[test]
    fn the_same_state_dumps_to_the_same_string_twice() {
        let (world, _first, _second) = world();
        let registry = registry();

        let once = canonical_dump(&world, &registry).expect("everything is registered");
        let twice = canonical_dump(&world, &registry).expect("everything is registered");

        assert_eq!(once, twice);
        assert_eq!(state_hash(&once), state_hash(&twice));
    }

    #[test]
    fn a_dump_ends_in_a_newline_and_uses_no_carriage_returns() {
        // A diff between two platforms has to compare simulations, not line
        // ending conventions.
        let (world, _first, _second) = world();
        let dump = canonical_dump(&world, &registry()).expect("everything is registered");

        assert!(dump.ends_with('\n'));
        assert!(!dump.contains('\r'), "a carriage return got into the dump");
    }

    #[test]
    fn the_same_content_built_in_a_different_order_dumps_identically() {
        // The real test of this module. Inserting components in a different
        // order sends the entities through different archetypes, so the storage
        // engine iterates them differently - and the dump has to come out the
        // same anyway. Entities are spawned first in both worlds, so the ids
        // match; the ids are part of the state, not an artifact of it.
        let registry = registry();

        let mut ordered = World::new();
        let a1 = ordered.spawn();
        let b1 = ordered.spawn();
        ordered.insert(a1, Position { x: 1, y: 2 }).expect("alive");
        ordered
            .insert(a1, Velocity { dx: 3, dy: 4 })
            .expect("alive");
        ordered.insert(a1, Label("one".to_owned())).expect("alive");
        ordered.insert(b1, Position { x: 5, y: 6 }).expect("alive");

        let mut shuffled = World::new();
        let a2 = shuffled.spawn();
        let b2 = shuffled.spawn();
        shuffled.insert(b2, Position { x: 5, y: 6 }).expect("alive");
        shuffled.insert(a2, Label("one".to_owned())).expect("alive");
        shuffled
            .insert(a2, Velocity { dx: 3, dy: 4 })
            .expect("alive");
        shuffled.insert(a2, Position { x: 1, y: 2 }).expect("alive");

        assert_eq!(
            (a1, b1),
            (a2, b2),
            "the two worlds must hand out the same ids"
        );

        let first = canonical_dump(&ordered, &registry).expect("everything is registered");
        let second = canonical_dump(&shuffled, &registry).expect("everything is registered");

        assert_eq!(first, second);
        assert_eq!(state_hash(&first), state_hash(&second));
    }

    #[test]
    fn changing_one_component_of_one_entity_changes_the_hash() {
        // Without this the two-run comparison is worthless: a function that
        // returned a constant would pass it perfectly.
        let (mut world, first, _second) = world();
        let registry = registry();

        let before = state_hash(&canonical_dump(&world, &registry).expect("registered"));

        world.get_mut::<Position>(first).expect("alive").x += 1;

        let after = state_hash(&canonical_dump(&world, &registry).expect("registered"));

        assert_ne!(before, after, "a changed component has to change the hash");
    }

    #[test]
    fn adding_or_removing_a_component_changes_the_hash() {
        let (mut world, _first, second) = world();
        let registry = registry();

        let before = state_hash(&canonical_dump(&world, &registry).expect("registered"));

        world
            .insert(second, Velocity { dx: 0, dy: 0 })
            .expect("alive");
        let with_velocity = state_hash(&canonical_dump(&world, &registry).expect("registered"));
        assert_ne!(before, with_velocity);

        world.remove::<Velocity>(second).expect("just inserted");
        let removed_again = state_hash(&canonical_dump(&world, &registry).expect("registered"));
        assert_eq!(before, removed_again, "removing it must restore the state");
    }

    #[test]
    fn a_one_byte_difference_changes_the_hash() {
        assert_ne!(state_hash("entities 1\n"), state_hash("entities 2\n"));
        assert_ne!(state_hash(""), state_hash("\n"));
        assert_eq!(state_hash("same"), state_hash("same"));
    }

    /// The published FNV-1a-64 test vectors.
    ///
    /// # Why these literals are allowed and state hashes are not
    ///
    /// ADR-0008 forbids committing the hash of a *state*, because such a value
    /// depends on RON's formatting and on serde's field order: a `ron` release
    /// that moves a separator turns the suite red while the simulation is
    /// perfectly correct, which is the worst failure mode this project has.
    ///
    /// A vector below depends on none of that. It pins this function's
    /// *arithmetic* against the published definition of FNV-1a, and the only
    /// thing that can move it is a change to the algorithm itself — which is
    /// precisely what it is here to catch. The two constants are one transposed
    /// digit away from a hash function that would still pass every other test in
    /// this file, because a two-run comparison compares two runs of the same
    /// wrong code. Nothing in the workspace notices that except a value from
    /// outside it.
    ///
    /// The distinction is the direction the value comes from: a state hash is
    /// something this repository produces and would be checking against itself,
    /// a test vector is something the specification produces and this repository
    /// is checked against.
    ///
    /// Sources. The constants are from *The FNV Non-Cryptographic Hash
    /// Algorithm*, draft-eastlake-fnv-21, section 5: the 64-bit prime is
    /// `0x00000100000001B3` and the 64-bit offset basis is
    /// `0xCBF29CE484222325`. The input/output pairs are from the FNV reference
    /// distribution by Landon Curt Noll, `test_fnv.c`, array `fnv1a_64_vector`
    /// (<https://github.com/lcn2/fnv>).
    #[test]
    fn the_hash_matches_the_published_fnv_1a_64_vectors() {
        assert_eq!(state_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(state_hash("a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(state_hash("b"), 0xaf63_df4c_8601_f1a5);
        assert_eq!(state_hash("c"), 0xaf63_de4c_8601_eff2);
        assert_eq!(state_hash("fo"), 0x0898_5907_b541_d342);
    }

    #[test]
    fn the_constants_are_the_published_ones() {
        // The empty-input vector above already pins the offset basis, since it
        // is the whole output for no input. The prime needs at least one byte to
        // become observable, which "a" provides - but naming both constants
        // directly says which line to look at when a vector fails.
        assert_eq!(super::FNV_OFFSET_BASIS, 0xcbf2_9ce4_8422_2325);
        assert_eq!(super::FNV_PRIME, 0x0000_0100_0000_01b3);
    }

    #[test]
    fn the_hash_is_fnv_1a_rather_than_fnv_1() {
        // The two differ only in the order of the xor and the multiply, so an
        // implementation that swapped them would still hash, still be
        // deterministic, and still pass every two-run comparison. It would not
        // produce the published FNV-1a value for "a": FNV-1 multiplies first and
        // arrives somewhere else entirely.
        let mut fnv1 = super::FNV_OFFSET_BASIS;
        fnv1 = fnv1.wrapping_mul(super::FNV_PRIME);
        fnv1 ^= u64::from(b'a');

        assert_ne!(fnv1, state_hash("a"));
        assert_eq!(state_hash("a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn an_unregistered_component_is_an_error_that_names_the_entity_and_the_type() {
        let (mut world, _first, second) = world();
        let registry = registry();

        world
            .insert(second, Forgotten { important: 42 })
            .expect("alive");

        let error = canonical_dump(&world, &registry)
            .expect_err("an unregistered component must not be skipped quietly");

        match error {
            EcsError::UnregisteredComponent { entity, component } => {
                assert_eq!(entity, second);
                assert!(
                    component.ends_with("Forgotten"),
                    "the error has to name the type, got {component}"
                );
            }
            other => panic!("expected an unregistered component error, got {other:?}"),
        }

        // Registering it makes the very same world serialize.
        let mut complete = registry;
        complete
            .register_component::<Forgotten>("forgotten")
            .expect("first");
        let dump = canonical_dump(&world, &complete).expect("now everything is registered");
        assert!(dump.contains("forgotten (important:42)"));
    }

    #[test]
    fn an_empty_world_still_dumps() {
        let world = World::new();

        assert_eq!(
            canonical_dump(&world, &registry()).expect("nothing to serialize"),
            "entities 0\n"
        );
    }

    /// A three-entity dump, used by the difference tests below.
    fn sample() -> String {
        "entities 3\n\
         entity 0v1\n\
         \x20 position (x:1,y:2)\n\
         entity 1v1\n\
         \x20 position (x:3,y:4)\n\
         \x20 velocity (dx:5,dy:6)\n\
         entity 2v3\n\
         \x20 position (x:7,y:8)\n"
            .to_owned()
    }

    /// `sample()` with one line replaced.
    fn sample_with(line: usize, replacement: &str) -> String {
        let mut lines: Vec<String> = sample().lines().map(str::to_owned).collect();
        lines[line - 1] = replacement.to_owned();
        format!("{}\n", lines.join("\n"))
    }

    #[test]
    fn identical_dumps_have_no_difference() {
        assert_eq!(first_difference(&sample(), &sample()), None);
        assert_eq!(first_difference("", ""), None);
    }

    #[test]
    fn a_difference_in_the_first_line_is_found() {
        let difference =
            first_difference(&sample(), &sample_with(1, "entities 4")).expect("the header differs");

        assert_eq!(difference.line(), 1);
        assert_eq!(
            difference.entity(),
            None,
            "there is no entity yet on line 1"
        );
        assert_eq!(difference.component(), None);
        assert_eq!(difference.left(), Some("entities 3"));
        assert_eq!(difference.right(), Some("entities 4"));
    }

    #[test]
    fn a_difference_in_the_middle_names_its_entity_and_component() {
        // The property the whole function exists for: not "line 6 differs" but
        // which entity and which component, which is what a reader acts on.
        let difference = first_difference(&sample(), &sample_with(6, "  velocity (dx:5,dy:99)"))
            .expect("the velocity differs");

        assert_eq!(difference.line(), 6);
        assert_eq!(difference.component(), Some("velocity"));

        let entity = difference.entity().expect("line 6 is inside entity 1v1");
        assert_eq!((entity.index(), entity.generation()), (1, 1));
        assert_eq!(difference.left(), Some("  velocity (dx:5,dy:6)"));
        assert_eq!(difference.right(), Some("  velocity (dx:5,dy:99)"));
    }

    #[test]
    fn a_difference_in_the_last_line_is_found() {
        let difference = first_difference(&sample(), &sample_with(8, "  position (x:7,y:9)"))
            .expect("the last line differs");

        assert_eq!(difference.line(), 8);
        assert_eq!(difference.component(), Some("position"));

        // The generation is carried through, not assumed to be one.
        let entity = difference.entity().expect("line 8 is inside entity 2v3");
        assert_eq!((entity.index(), entity.generation()), (2, 3));
    }

    #[test]
    fn a_difference_on_an_entity_line_reports_no_component() {
        let difference =
            first_difference(&sample(), &sample_with(4, "entity 9v1")).expect("differs");

        assert_eq!(difference.line(), 4);
        assert_eq!(difference.component(), None);
        // Tracked from the left side, which the documentation states.
        assert_eq!(difference.entity().map(|entity| entity.index()), Some(1));
    }

    #[test]
    fn one_dump_being_a_prefix_of_the_other_is_a_difference() {
        // Without this the walk would run out of pairs and report agreement,
        // which is how a truncated dump would compare as identical.
        let full = sample();
        let short: String = full
            .lines()
            .take(5)
            .map(|line| format!("{line}\n"))
            .collect();

        let difference = first_difference(&short, &full).expect("one side is shorter");

        assert_eq!(difference.line(), 6);
        assert_eq!(difference.left(), None, "the short dump ends here");
        assert_eq!(difference.right(), Some("  velocity (dx:5,dy:6)"));
        assert_eq!(difference.component(), Some("velocity"));
        assert_eq!(difference.entity().map(|entity| entity.index()), Some(1));

        // ... and the same the other way round.
        let mirrored = first_difference(&full, &short).expect("one side is shorter");
        assert_eq!(mirrored.left(), Some("  velocity (dx:5,dy:6)"));
        assert_eq!(mirrored.right(), None);
    }

    #[test]
    fn an_empty_dump_against_a_full_one_is_a_difference_at_line_one() {
        let difference = first_difference("", &sample()).expect("empty is not the same as full");

        assert_eq!(difference.line(), 1);
        assert_eq!(difference.left(), None);
        assert_eq!(difference.right(), Some("entities 3"));
        assert_eq!(difference.entity(), None);
    }

    #[test]
    fn a_real_pair_of_dumps_is_compared_the_same_way() {
        // The end to end case: two worlds, not two string literals.
        let (mut world, first, _second) = world();
        let registry = registry();
        let before = canonical_dump(&world, &registry).expect("registered");

        world.get_mut::<Position>(first).expect("alive").y -= 1;
        let after = canonical_dump(&world, &registry).expect("registered");

        let difference = first_difference(&before, &after).expect("one component moved");
        assert_eq!(difference.entity(), Some(first));
        assert_eq!(difference.component(), Some("position"));
    }

    #[test]
    fn the_rendered_form_carries_every_field() {
        let difference = first_difference(&sample(), &sample_with(6, "  velocity (dx:5,dy:99)"))
            .expect("differs");
        let rendered = difference.to_string();

        for expected in [
            "line      6",
            "entity    1v1",
            "component velocity",
            "(dx:5,dy:6)",
            "(dx:5,dy:99)",
        ] {
            assert!(
                rendered.contains(expected),
                "the rendered difference is missing {expected:?}:\n{rendered}"
            );
        }
    }

    #[test]
    fn the_rendered_form_says_when_a_dump_simply_ended() {
        let short = "entities 3\n";
        let rendered = first_difference(short, &sample())
            .expect("differs")
            .to_string();

        assert!(rendered.contains("ends here"), "{rendered}");
    }
}
