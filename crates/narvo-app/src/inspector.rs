//! What the debug overlay says about one entity, as text.
//!
//! The content half of M6.6d, and **it is a pure function**: a `&World`, a
//! `&ComponentRegistry`, an index, and out come lines. No device, no window, no
//! pixels. That is what makes the part of an inspector that can be *wrong*
//! entirely machine-checkable, and it is the substitute check M6.6b's halt
//! report rested its "not pinned" answer on.
//!
//! # No third format
//!
//! The component lines are **exactly what `canonical_dump` writes** for that
//! entity, character for character: `entity {index}v{generation}` for the head,
//! and two spaces, the stable name, one space, the RON value for each component
//! (`narvo-ecs/src/state.rs:65-88`). Nothing here formats a component itself —
//! `ComponentRegistry::serialize_component` does, through `ron::to_string`.
//!
//! M6.7a rejected a third format for "what a world is" because every consumer
//! would have to move when the dump moved. The same argument applies to a
//! *line*: a reader comparing the overlay against `narvo --dump` should be
//! comparing, not translating. [`overlay_lines_are_the_dump_s_own_lines`] holds
//! that as an equality rather than as this paragraph.
//!
//! # What it cannot see, and that is measured rather than assumed
//!
//! An entity can carry a component the registry does not know. `canonical_dump`
//! **fails** on that (ADR-0008: a component outside the hash makes a divergence
//! in it invisible); this function walks the registry, so it cannot notice such
//! a component at all and its answer is silently short.
//!
//! **Counting them is not possible from this crate.** It would need
//! `World::component_type_ids`, which is `pub(crate)` to `narvo-ecs` on purpose
//! — "answering it publicly would invite code that branches on component types
//! at runtime" (`world.rs:320-329`). `crates/narvo-app/src/ipc.rs` already
//! carries the identical limit for `get_entity`, named there rather than closed,
//! and this is the second consumer to inherit it. Reported, not worked around.

use narvo_ecs::{ComponentRegistry, World};

/// What replaces a character the glyph atlas cannot draw.
///
/// The atlas covers ASCII 32..=126 (D10), and `layout_line` **skips anything
/// else and does not advance the pen** — so an unreplaced character leaves no
/// gap and no mark: a value of `héro` would be drawn as `hro`, shorter than it
/// is and silently so.
///
/// M6.6c measured which characters actually reach here. A newline cannot: RON
/// escapes it to a backslash and an `n`, two drawable characters. **Non-ASCII
/// can** — `ron::to_string` leaves `é` alone — and three of the eleven
/// registered components carry a `String` an author writes (`Sprite`, `HitRect`,
/// `Tally`).
///
/// # The limit of this substitute
///
/// A replaced character and a literal `?` look the same. That is a real
/// ambiguity and it is accepted rather than engineered around: every printable
/// ASCII character can legitimately appear in a value, so no substitute is
/// unambiguous. What the substitution buys is that the text stops being *short*
/// — a reader sees that something was there.
pub const UNDRAWABLE: char = '?';

/// `text` with every character the atlas cannot draw replaced by [`UNDRAWABLE`].
///
/// One output character per input character, so the length in *characters* is
/// preserved and a truncation cannot hide in it.
#[must_use]
pub fn drawable(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_ascii() && !ch.is_ascii_control() {
                ch
            } else {
                UNDRAWABLE
            }
        })
        .collect()
}

/// The lines the overlay shows for the entity at `selected`.
///
/// `selected` is taken modulo the entity count, so stepping through wraps and no
/// caller has to know how many there are. An empty world yields one line saying
/// so rather than nothing at all — an overlay that vanishes looks broken.
///
/// Every line is passed through [`drawable`], including the ones this function
/// writes itself: a component *name* is a Rust identifier and cannot contain
/// anything undrawable, but a value can, and treating both the same way means
/// there is no path to the screen that skips the substitution.
#[must_use]
pub fn lines_for(world: &World, registry: &ComponentRegistry, selected: usize) -> Vec<String> {
    let entities = world.entity_ids();
    if entities.is_empty() {
        return vec!["no entities".to_owned()];
    }

    let index = selected % entities.len();
    let entity = entities[index];

    let mut lines = vec![
        format!("inspector {}/{}", index + 1, entities.len()),
        format!("entity {}v{}", entity.index(), entity.generation()),
    ];

    let before = lines.len();
    for info in registry.iter() {
        match registry.serialize_component(info.name(), world, entity) {
            // The component is there. Two spaces, the name, one space, the RON
            // value — `canonical_dump`'s own line.
            Ok(Some(value)) => lines.push(format!("  {} {}", info.name(), value)),
            // The normal case when walking a registry: this entity does not
            // carry this component. `serialize_component`'s own documentation
            // says so, and leaving the line out is what the dump does too.
            Ok(None) => {}
            // Not skipped. A component whose own `Serialize` failed is a defect
            // worth seeing, and an overlay that hid it would be the silent
            // shortening this module exists to avoid.
            Err(error) => lines.push(format!("  {} <error: {}>", info.name(), error)),
        }
    }

    if lines.len() == before {
        lines.push("  (no registered components)".to_owned());
    }

    lines.into_iter().map(|line| drawable(&line)).collect()
}

#[cfg(test)]
mod tests {
    use super::{UNDRAWABLE, drawable, lines_for};
    use narvo_ecs::{ComponentRegistry, Layer, Sprite, Transform, World, canonical_dump};

    /// The engine set, which is what a scene-file world is read against.
    fn registry() -> ComponentRegistry {
        let mut registry = ComponentRegistry::new();
        narvo_ecs::register_engine_components(&mut registry).expect("a fresh registry");
        registry
    }

    /// One entity carrying a transform and a layer, and one carrying nothing.
    fn world() -> World {
        let mut world = World::new();
        let first = world.spawn();
        world
            .insert(first, Transform::IDENTITY)
            .expect("just spawned");
        world.insert(first, Layer::at(0.5)).expect("just spawned");
        world.spawn();
        world
    }

    #[test]
    fn an_empty_world_says_so_rather_than_showing_nothing() {
        let lines = lines_for(&World::new(), &registry(), 0);
        assert_eq!(lines, vec!["no entities".to_owned()]);
    }

    #[test]
    fn the_head_names_which_entity_of_how_many() {
        let world = world();
        let lines = lines_for(&world, &registry(), 0);
        assert_eq!(lines[0], "inspector 1/2");

        let second = lines_for(&world, &registry(), 1);
        assert_eq!(second[0], "inspector 2/2");
    }

    /// Stepping past the end wraps rather than falling off it.
    #[test]
    fn the_selection_wraps_around_the_entity_count() {
        let world = world();
        assert_eq!(
            lines_for(&world, &registry(), 2),
            lines_for(&world, &registry(), 0),
            "stepping past the last entity must come back to the first"
        );
    }

    /// **The line format is the dump's, not a second one.**
    ///
    /// Not "looks similar": every component line this function produces appears
    /// verbatim in `canonical_dump`'s output for the same world, and so does the
    /// entity head. That is the property M6.7a's rejected third format was
    /// rejected to preserve, and it is what lets a reader compare the overlay
    /// with `narvo --dump` instead of translating between them.
    #[test]
    fn overlay_lines_are_the_dump_s_own_lines() {
        let world = world();
        let registry = registry();
        let dump = canonical_dump(&world, &registry).expect("the world is dumpable");

        let lines = lines_for(&world, &registry, 0);
        // Skip the inspector's own head, which is meta rather than state.
        for line in lines.iter().skip(1) {
            assert!(
                dump.lines().any(|dumped| dumped == line),
                "the overlay wrote a line the dump does not contain: {line:?}\n{dump}"
            );
        }

        // And it really did write component lines, so the loop above was not
        // vacuous.
        assert!(lines.iter().any(|line| line.starts_with("  transform ")));
        assert!(lines.iter().any(|line| line.starts_with("  layer ")));
    }

    /// An entity carrying nothing the registry knows says so.
    #[test]
    fn an_entity_with_no_components_says_so() {
        let world = world();
        let lines = lines_for(&world, &registry(), 1);
        assert_eq!(lines[0], "inspector 2/2");
        assert_eq!(lines[2], "  (no registered components)");
        assert_eq!(lines.len(), 3, "no component lines for an empty entity");
    }

    /// **A non-ASCII character is replaced, not dropped.**
    ///
    /// The edge M6.6c measured: `ron::to_string` leaves `é` alone, and
    /// `layout_line` would skip it *and not advance the pen*, so the drawn text
    /// would be silently short. `Sprite` carries an author-written `String`, so
    /// this is reachable from a scene file rather than hypothetical.
    #[test]
    fn a_non_ascii_value_is_marked_rather_than_silently_shortened() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Sprite::new("héro"))
            .expect("just spawned");

        let lines = lines_for(&world, &registry(), 0);
        let sprite = lines
            .iter()
            .find(|line| line.starts_with("  sprite "))
            .expect("the entity carries a sprite");

        assert!(
            sprite.contains("h?ro"),
            "the accented character was not replaced: {sprite:?}"
        );
        assert!(
            sprite.is_ascii(),
            "a line reaching the atlas must be drawable throughout: {sprite:?}"
        );
        assert!(
            !sprite.contains('é'),
            "the original character survived into a line the atlas cannot draw"
        );
    }

    /// The substitution preserves length in characters.
    ///
    /// One in, one out — so a reader can see *that* something was replaced by
    /// the shape of the line, and a truncation cannot hide inside the
    /// replacement.
    #[test]
    fn replacing_keeps_one_character_per_character() {
        let source = "aébÿc";
        let drawn = drawable(source);

        assert_eq!(drawn.chars().count(), source.chars().count());
        assert_eq!(drawn, format!("a{UNDRAWABLE}b{UNDRAWABLE}c"));
    }

    /// A newline never reaches the atlas from this path, and a tab does not
    /// either — but if one ever did, it would be replaced rather than skipped.
    ///
    /// M6.6c measured that `ron::to_string` escapes both into two drawable
    /// characters, so the *value* path cannot deliver them. This asserts the
    /// substitution covers them anyway, because "cannot happen" is a claim about
    /// today's serializer and not about this function.
    #[test]
    fn a_control_character_is_replaced_even_though_ron_never_emits_one() {
        assert_eq!(drawable("a\nb"), format!("a{UNDRAWABLE}b"));
        assert_eq!(drawable("a\tb"), format!("a{UNDRAWABLE}b"));
    }

    /// Everything the atlas can draw passes through untouched.
    ///
    /// The counter-proof: a substitution that replaced too much would still pass
    /// the tests above.
    #[test]
    fn every_drawable_character_survives_unchanged() {
        let printable: String = (32_u8..=126).map(char::from).collect();
        assert_eq!(drawable(&printable), printable);
    }
}
