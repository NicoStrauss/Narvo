//! How many times a named action has happened.
//!
//! [`Tally`] is the smallest piece of state an incremental game needs: a number
//! that goes up when something arrives. It is registered, so it is in the
//! canonical dump and the state hash — a click that increments a counter has to
//! be reproducible from a recording, and a count kept outside the world would
//! not be.
//!
//! **The system that increments it is not here.** Counting means reading an
//! `Events<InputEvent>` buffer, and `InputEvent` lives in `narvo-input`, which
//! this crate does not depend on (ADR-0025). The component is storage and the
//! system lives in `narvo-app`, which is the same split M5.4 made for
//! [`HitRect`](crate::HitRect) and `hit_test`, and it is forced by the same
//! layering rather than chosen again.

use serde::{Deserialize, Serialize};

/// A count of one named action.
///
/// # Two fields, and why the name is one of them
///
/// The alternative is a bare count whose meaning lives in whichever system
/// increments it. That system would then have the action name compiled into it,
/// so a scene could not say *what* it is counting — and a second counter in the
/// same world would need a second system. Carrying the name makes one system
/// serve every counter and puts the meaning in the content, which is where
/// `ProjektPlan.md` §4 wants it.
///
/// # The string
///
/// Permitted by ADR-0014 in its own words — a registered type *"holds only
/// scalars, `String`, and other registered-component types"* — with
/// [`Sprite`](crate::Sprite) as the precedent that first exercised it and
/// [`HitRect`](crate::HitRect) as the second. The name is not validated here for
/// [`HitRect`](crate::HitRect)'s reason: a component is storage, and the
/// identifier rule lives in a crate this one cannot see.
///
/// # Opt-in
///
/// A world that never registers the type dumps and hashes exactly as it did
/// before, which `registering_the_type_changes_nothing_for_a_world_that_has_none`
/// proves by comparing two registries and never a stored hash (ADR-0008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// The action this counts.
    pub action: String,
    /// How many have arrived so far.
    ///
    /// An `i64` rather than a `u64` because it is the same width and signedness
    /// as the magnitude an [`InputEvent`](https://docs.rs/narvo-input) carries,
    /// and a counter that sums magnitudes rather than occurrences is the obvious
    /// next thing somebody will want. Nothing here decrements it today.
    pub count: i64,
}

impl Tally {
    /// A counter of `action`, starting at zero.
    #[must_use]
    pub fn new(action: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Tally;
    use crate::{ComponentRegistry, Transform, World, canonical_dump};

    #[test]
    fn a_new_counter_starts_at_zero_and_knows_its_action() {
        let tally = Tally::new("buy");

        assert_eq!(tally.action, "buy");
        assert_eq!(tally.count, 0);
    }

    #[test]
    fn a_counter_survives_the_ron_round_trip_with_anything_in_the_name() {
        for action in [
            "buy",
            "",
            "with \"quotes\"",
            "with \\ backslash",
            "mit Ümläuten",
        ] {
            let tally = Tally {
                action: action.to_owned(),
                count: i64::MIN,
            };

            let text = ron::to_string(&tally).expect("a tally always serializes");
            let restored: Tally = ron::from_str(&text).expect("what was written reads back");

            assert_eq!(restored, tally, "{action:?} did not survive as {text}");
        }
    }

    #[test]
    fn registering_the_type_changes_nothing_for_a_world_that_has_none() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Transform::IDENTITY)
            .expect("a fresh entity takes a component");

        let mut without = ComponentRegistry::new();
        without
            .register_component::<Transform>("transform")
            .expect("a fresh registry accepts it");

        let mut with = ComponentRegistry::new();
        with.register_component::<Transform>("transform")
            .expect("a fresh registry accepts it");
        with.register_component::<Tally>("tally")
            .expect("a fresh registry accepts it");

        assert_eq!(
            canonical_dump(&world, &without).expect("every component is registered"),
            canonical_dump(&world, &with).expect("every component is registered"),
        );
    }
}
