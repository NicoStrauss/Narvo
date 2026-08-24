//! The simulation constituted from a scene **file**, rather than from code.
//!
//! Every other mode in this crate builds its world in Rust. This one is the
//! first that does not: `ProjektPlan.md` §6/M4 says a loaded scene *constitutes*
//! the initial state, and until M4.3 nothing in the repository actually did
//! that — the format existed, the loader existed, and no runner called either.
//!
//! # Not to be confused with [`scene`](super::scene)
//!
//! That module is the sprite-field demo, mode `scene`, and it builds its world
//! in code like the rest. This is mode `scene-file`. The names are one word
//! apart because the *thing they build* is one word apart, and renaming the
//! older one would move a mode name that recordings and the determinism suite
//! already carry.
//!
//! # Which systems run, and why only these
//!
//! [`compose_camera`], and nothing else. It is the one system `narvo-ecs`
//! offers that a scene's components drive: it advances every [`Follow`] and
//! [`Shake`] on an entity carrying a [`Camera`], then writes the camera as
//! `base + Σ offsets` (ADR-0017). M4.3 wires existing engine systems and adds
//! none, so the demo-local behaviours next door — `wander`, `arm_shakes` — stay
//! where they are.
//!
//! Since M5.4, [`rotate_events`](narvo_ecs::rotate_events) too, because a
//! scene-file world now carries an input buffer and a window can click into it.
//!
//! Since M5b.4, [`crate::physics::simulate`] as well, so that a scene may carry
//! the eleventh registered component and have it move. Three things about that
//! addition are worth stating rather than inferring:
//!
//! - **It is inert for a scene without bodies.** `crate::physics::step` extracts
//!   first and returns before touching the facade when the extraction is empty,
//!   so a world with no `RigidBody` is stepped as a world with none — no rapier
//!   world is built, nothing is written, and no dump moves. `determinism-case.ron`
//!   is that world, and the cross-platform matrix comparing byte for byte before
//!   and after is what says so rather than this sentence.
//! - **It runs before [`compose_camera`] and after the input systems**, which is
//!   the order the phases already have: input arrives, the world moves, and the
//!   camera composes from the state the tick left behind (ADR-0017).
//! - **Gravity is the engine's and a scene cannot change it.** `crate::physics`
//!   carries the constant and names that limit.
//!
//! **The reason that stood here until M5.4 was wrong and is worth replacing
//! rather than deleting.** It said an events buffer is *"generic over its
//! payload so no scene can carry one"*. Genericity was never the blocker —
//! `narvo-input`'s own integration test has registered and dumped exactly
//! `Events<InputEvent>` since M5.1. **Registration** was the blocker: a scene may
//! name any component the registry loading it knows, and no registry knew that
//! one. This module now registers it, so a scene file may name `"input"` like
//! any other component. That is the component-open consequence ADR-0018 promises
//! and not a special case.
//!
//! # Why the registration is here and not in `register_engine_components`
//!
//! Because `narvo-ecs` cannot name the type. `Events<InputEvent>`'s payload
//! lives in `narvo-input`, and ADR-0025 put that crate below everything:
//! *"`narvo-app` depends on `narvo-ecs` and on `narvo-input`, and neither of
//! those two knows about the other."* Registering the buffer in the engine set
//! would mean `narvo-ecs` depending on `narvo-input` — reversing that layering
//! and closing a dependency cycle against `narvo-input`'s dev-dependency.
//!
//! So it is registered by the crate that already sees both, exactly as
//! [`scene`](super::scene) registers its own `Wander`. The registry's own
//! documentation names this as the intended shape: a caller registers the engine
//! set *"and then whatever else it has"*.
//!
//! The cost is named rather than hidden: `tools/narvo-cli` validates against the
//! engine set alone, so it reports a scene naming `"input"` as carrying an
//! unknown component. That is already true of `wander`, and it is the same
//! trade — the validator knows the engine, and a runner may know more.

use narvo_ecs::{
    ComponentRegistry, EcsError, Events, HitRect, Scheduler, SystemContext, Tally, World,
    advance_bursts, compose_camera, register_engine_components, rotate_events,
};
use narvo_input::InputEvent;

use super::Simulation;

/// The stable name a scene file writes for the input buffer.
///
/// The same name `sim::input`'s demo registry has used since M2.3, so a dump of
/// a scene world and a dump of the demo spell the buffer identically.
pub const INPUT: &str = "input";

/// Builds the world a scene file describes.
///
/// Takes the scene's **text**, not its path: the file has already been read and
/// its anchor checked by the time this is called, and reading it a second time
/// here would open a window in which the file changes between the check and the
/// load. `scene_anchor` explains that in full.
///
/// # Errors
///
/// [`SceneStartError::Scene`] if the text is not a scene this registry can
/// load — with the position of the fault in it, since M4.2 — and
/// [`SceneStartError::Ecs`] if the registry or the scheduler refuses to be
/// built, which would be a mistake in the lines below rather than in the file.
pub fn build(text: &str) -> Result<Simulation, SceneStartError> {
    let mut registry = ComponentRegistry::new();
    register_engine_components(&mut registry)?;

    registry.register_component::<Events<InputEvent>>(INPUT)?;

    let mut world = narvo_scene::from_str(text, &registry)?;
    check_hit_actions(&world)?;
    ensure_input_buffer(&mut world)?;

    let mut scheduler = Scheduler::new();
    scheduler.add_system("input/rotate", rotate_events::<InputEvent>)?;
    scheduler.add_system("tally", count_actions)?;
    scheduler.add_system("physics", crate::physics::simulate)?;
    // ADR-0044: the one system a burst needs. It runs after everything that
    // could arm one and before `compose_camera`, so a burst armed this tick is
    // one tick old when the frame that follows draws it — the same "advance
    // first, then compose" order `Shake` fixed in M3.31 and for the same reason.
    scheduler.add_system("bursts", advance_bursts)?;
    scheduler.add_system("compose_camera", compose_camera)?;

    Ok(Simulation {
        world,
        registry,
        scheduler,
    })
}

/// Counts the input events each [`Tally`] is watching for.
///
/// # Why this system is in `narvo-app` and not in `narvo-ecs`
///
/// Counting means reading an `Events<InputEvent>` buffer, and `InputEvent` lives
/// in `narvo-input`, which `narvo-ecs` does not depend on (ADR-0025). So the
/// component is engine vocabulary and the system that drives it is not — the
/// same split M5.4 made for `HitRect` and `hit_test`, forced by the same
/// layering rather than chosen a second time.
///
/// # What it counts
///
/// One per matching event, not the event's magnitude. A click sends `buy 1` and
/// a key bound `OnPress(1)` sends the same, so occurrences and magnitudes agree
/// today; they would stop agreeing the moment a binding sent `2`, and counting
/// occurrences is the meaning the name `Tally` carries. Summing magnitudes is a
/// different component and a different decision.
///
/// Every tally sees every event, so two tallies watching one action both
/// advance. Order does not enter into it: addition commutes, and the events are
/// read once into a vector before anything is written, because the buffer is
/// borrowed from the same world the counters live in.
pub(crate) fn count_actions(world: &mut World, _context: &SystemContext) {
    let mut arrived: Vec<String> = Vec::new();
    for entity in world.entity_ids() {
        if let Ok(buffer) = world.get::<Events<InputEvent>>(entity) {
            arrived.extend(buffer.iter().map(|event| event.action().to_owned()));
        }
    }

    if arrived.is_empty() {
        return;
    }

    for entity in world.entity_ids() {
        let Ok(mut tally) = world.get_mut::<Tally>(entity) else {
            continue;
        };

        tally.count += arrived
            .iter()
            .filter(|action| **action == tally.action)
            .count() as i64;
    }
}

/// What has changed since the last look, and remembers the new values.
///
/// # Why a snapshot rather than a message
///
/// A counter is world state, and the world has no way to announce that it moved
/// — a system cannot capture anything (ADR-0002) and an event would be a second
/// channel for something the state already says. So the observer keeps the last
/// values it saw and compares. That is the same shape `Watcher` uses for a file
/// (ADR-0022 Decision 2): poll and compare, because the thing being watched does
/// not report.
///
/// # Once per change, not once per frame
///
/// The window draws sixty times a second and a counter moves when somebody
/// clicks. Returning only the differences is what makes the log a record of
/// events rather than a stream: an unchanged counter produces nothing, and a
/// counter that moved by three in one tick produces one line saying three.
///
/// `seen` is updated to the current values, so a caller that ignores the return
/// still advances the snapshot. Entities are visited in canonical id order, so
/// two counters that move in one tick are reported in a fixed order.
///
/// A counter that disappears — its entity despawned, or the world replaced by a
/// reload — leaves a stale entry in `seen`. That is harmless and deliberate: ids
/// are not reused within a generation, so a stale entry can only ever be
/// compared against an entity that no longer exists, and the reload path clears
/// the whole snapshot anyway.
///
/// # Gated like `watch`, `input` and `hit`, and for their reason
///
/// Its only production caller is `window.rs`, which is render-gated, so in a
/// headless build every line of it is dead. The `test` half keeps the eleven
/// assertions below compiling and running in the headless test build, where
/// they cost nothing and catch the same mistakes — the M4.9 trade, applied to a
/// function rather than a module.
#[cfg(any(feature = "render", test))]
pub(crate) fn tally_changes(
    world: &World,
    seen: &mut std::collections::BTreeMap<narvo_ecs::EntityId, i64>,
) -> Vec<(String, i64)> {
    let mut changed = Vec::new();

    for entity in world.entity_ids() {
        let Ok(tally) = world.get::<Tally>(entity) else {
            continue;
        };

        if seen.insert(entity, tally.count) != Some(tally.count) {
            changed.push((tally.action.clone(), tally.count));
        }
    }

    changed
}

/// Refuses a scene whose hit rectangles carry action names nothing could record.
///
/// # Why the check is here and not on the component
///
/// [`HitRect`] is storage and does not validate, exactly as
/// [`Sprite`](narvo_ecs::Sprite) does not reject a region name no atlas
/// carries. It also *cannot*: the rule lives in `narvo-input`, which
/// `narvo-ecs` does not depend on (ADR-0025). This module is the first place a
/// world and that rule are both in scope, which is where the resolution belongs.
///
/// # Why at all, when nothing here records
///
/// ADR-0012 restricts an action name to an identifier so that a line-based
/// recording can hold it without a quoting rule, and D8 keeps a recording at the
/// action level (ADR-0012's M5.2 amendment). So a click on a rectangle named
/// `"buy thing"` would produce an event no recording could write — and it would
/// do so at the moment somebody clicked, in a window, rather than at load. One
/// comparison per rectangle at load turns that into a sentence on the terminal.
///
/// # Where
///
/// The entity's index in the file's entity list, which `narvo-scene`'s own
/// error type calls the stronger of its two spellings of *where*: it is also the
/// entity's slot in the loaded world, so it names the mistake in the file **and**
/// in any dump or `first_difference` report that follows.
///
/// # Errors
///
/// [`SceneStartError::HitAction`] naming the entity and the rejected name.
fn check_hit_actions(world: &World) -> Result<(), SceneStartError> {
    for (index, entity) in world.entity_ids().into_iter().enumerate() {
        let Ok(rect) = world.get::<HitRect>(entity) else {
            continue;
        };

        if !InputEvent::is_valid_action(&rect.action) {
            return Err(SceneStartError::HitAction {
                index,
                name: rect.action.clone(),
            });
        }
    }

    Ok(())
}

/// Gives the world exactly one input buffer, if the scene did not.
///
/// # The insertion rule, and why it is stated rather than obvious
///
/// A scene file may carry an `"input"` component itself, and most will not. So:
///
/// - **If any entity already carries one, nothing is inserted.** The scene said
///   where the buffer goes and that is the answer; adding a second would give
///   the world two, and `rotate_events` rotates every buffer it finds while a
///   feeder writes to the first — which is a silent half-delivery rather than an
///   error.
/// - **Otherwise exactly one entity is spawned to carry it**, after every entity
///   the file describes. That position is what makes it deterministic: entity
///   ids are handed out in spawn order, the scene's own entities are spawned in
///   file order (ADR-0018), and appending leaves every one of their ids exactly
///   where it was. The new entity always takes the next id, and the canonical
///   dump — which sorts by id — always puts it last.
///
/// The collision case is therefore not "two buffers exist" but "the scene put
/// one somewhere and we respect it", and both branches are tested.
///
/// # It changes the dump of every scene-file world
///
/// Deliberately, and it is the only thing in M5.4 that does. One more entity
/// means one more line in the canonical dump and a higher entity count, so every
/// scene-file state hash moves. That is not a regression: both sides of every
/// comparison are produced from one build (ADR-0008), and no expected hash is
/// stored anywhere to disagree with.
///
/// # Errors
///
/// Anything `World::insert` can raise, which for an entity spawned one line
/// earlier is nothing.
fn ensure_input_buffer(world: &mut World) -> Result<(), EcsError> {
    let carried = world
        .entity_ids()
        .into_iter()
        .any(|entity| world.has::<Events<InputEvent>>(entity));

    if carried {
        return Ok(());
    }

    let console = world.spawn();
    world.insert(console, Events::<InputEvent>::new())
}

/// What can stop a scene-file run from starting.
#[derive(Debug)]
pub enum SceneStartError {
    /// The file is not a scene this build can load.
    Scene(narvo_scene::SceneError),
    /// The registry or the scheduler refused to be built.
    Ecs(EcsError),
    /// A hit rectangle names an action no recording could hold.
    HitAction {
        /// The entity's position in the file's entity list, counting from zero,
        /// which is also its slot in the loaded world.
        index: usize,
        /// The name that was rejected.
        name: String,
    },
}

impl From<narvo_scene::SceneError> for SceneStartError {
    fn from(error: narvo_scene::SceneError) -> Self {
        Self::Scene(error)
    }
}

impl From<EcsError> for SceneStartError {
    fn from(error: EcsError) -> Self {
        Self::Ecs(error)
    }
}

impl std::fmt::Display for SceneStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // The scene error already carries its own position and says what to
            // do about it (M4.2), so there is nothing to add in front of it.
            Self::Scene(error) => write!(f, "{error}"),
            Self::Ecs(error) => write!(f, "{error}"),
            Self::HitAction { index, name } => write!(
                f,
                "entity {index} has a hitrect whose action is \"{name}\", which is not a usable \
                 action name; an action name is a non-empty run of ASCII letters, digits, `_`, \
                 `-` and `.`, so that a recording of the click can hold it on one line without a \
                 quoting rule"
            ),
        }
    }
}

impl std::error::Error for SceneStartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Scene(error) => Some(error),
            Self::Ecs(error) => Some(error),
            // This crate's own finding rather than another layer's failure, so
            // there is nothing underneath it.
            Self::HitAction { .. } => None,
        }
    }
}

/// The shipped drop scene, and the two numbers every test of it shares.
///
/// `pub(crate)` and test-only: `frame.rs` draws this scene into a blessed
/// reference and this module simulates it, and the two must be talking about the
/// same file at the same tick. A second copy of either number is how a picture
/// comes to be blessed at a tick nothing asserts anything about.
#[cfg(test)]
pub(crate) mod drop_scene {
    use std::path::PathBuf;

    /// Where the committed scene lives.
    pub(crate) fn path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes/physics_drop.ron")
    }

    /// Its text, read from the repository.
    pub(crate) fn text() -> String {
        let path = path();
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    }

    /// The tick the reference image is taken at, and the tick every assertion
    /// about the picture is made at.
    ///
    /// **Chosen after the first contact, which this scene resolves at tick 15**
    /// — measured, and `the_drop_scene_lands_before_the_blessed_tick` is what
    /// keeps the number honest if the scene ever moves. Thirty ticks later is
    /// where the picture says the most:
    ///
    /// - `box_a` is at rest on the ground and `box_b` has just come down onto
    ///   it, so a **contact between two dynamic bodies** is visible and not only
    ///   a contact with the world;
    /// - `box_c` and `box_d` are still in flight and visibly **turned** — 26° and
    ///   −20° — so the rotation pair that M5b.4's seam carries is in the picture
    ///   rather than only in the dump. A settled box is a flat box: the contact
    ///   solver drives it level, so a later tick would show four rectangles and
    ///   nothing about rotation at all.
    ///
    /// ADR-0029's residual motion is in this picture and is not tuned away: the
    /// rebuilt world jitters where a retained one would rest. At this tick that
    /// is thousandths of a world unit, well under a pixel, and the scene is not
    /// cut to hide it.
    pub(crate) const TICK: u64 = 45;
}

#[cfg(test)]
mod tests {
    use super::{SceneStartError, build, drop_scene};
    use narvo_ecs::{
        Burst, Camera, EntityId, Events, Follow, Shake, SystemContext, Tally, World, canonical_dump,
    };
    use narvo_ecs::{RigidBody, Sprite};
    use narvo_input::InputEvent;
    use std::path::PathBuf;

    /// A scene small enough to read, carrying the three components the wired
    /// system actually drives.
    const MOVING: &str = "Scene(entities: [\n\
        \x20   (name: \"target\", components: {\n\
        \x20       \"transform\": (x: 8.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),\n\
        \x20   }),\n\
        \x20   (\n\
        \x20       components: {\n\
        \x20           \"camera\": (x: 0.0, y: 0.0, zoom: 1.0),\n\
        \x20           \"follow\": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false),\n\
        \x20       },\n\
        \x20       refs: { \"follow\": { \"target\": \"target\" } },\n\
        \x20   ),\n\
        ])\n";

    #[test]
    fn a_scene_becomes_a_world_with_its_entities_in_file_order() {
        let simulation = build(MOVING).expect("a scene this build can load");

        // Three, not two: the file describes two entities and `build` appends
        // the input buffer's carrier after them (M5.4). The scene's own ids are
        // untouched, which is what the `ids[1]` lookups below still rely on.
        assert_eq!(simulation.world.len(), 3);
        let ids = simulation.world.entity_ids();
        assert!(simulation.world.get::<Camera>(ids[1]).is_ok());
        assert_eq!(
            simulation
                .world
                .get::<Follow>(ids[1])
                .expect("the eye follows")
                .target,
            ids[0]
        );
    }

    /// The wired system actually runs, and moves what the scene set up.
    ///
    /// Without this the mode could load a world and tick nothing, and every
    /// hash over it would still be stable — stable and empty of the thing the
    /// case exists to exercise. `0.5` of the way from 0 to 8 is 4, exactly, so
    /// this asserts a value rather than a change.
    #[test]
    fn the_camera_composition_system_is_wired_and_advances_the_world() {
        let mut simulation = build(MOVING).expect("loads");
        let eye = simulation.world.entity_ids()[1];

        simulation
            .scheduler
            .run(&mut simulation.world, &SystemContext::new(0));

        let camera = *simulation
            .world
            .get::<Camera>(eye)
            .expect("the eye has one");
        assert_eq!(camera.x.to_bits(), 4.0_f32.to_bits());
    }

    /// Every component a scene can carry is registered, so the dump succeeds.
    ///
    /// The canonical dump refuses a world holding an unregistered component, so
    /// this is what says the mode's registry covers what its scenes may hold —
    /// and it is the same set the validation CLI checks against, from the same
    /// function.
    /// The scene may carry the buffer itself, and then nothing is appended.
    /// A world with one counter of `action` at `count`.
    fn world_with_tally(action: &str, count: i64) -> (World, EntityId) {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Tally {
                    action: action.to_owned(),
                    count,
                },
            )
            .expect("a fresh entity takes a component");

        (world, entity)
    }

    #[test]
    fn the_first_look_reports_every_counter_once() {
        let (world, _) = world_with_tally("buy", 0);
        let mut seen = std::collections::BTreeMap::new();

        assert_eq!(
            super::tally_changes(&world, &mut seen),
            vec![("buy".to_owned(), 0)],
            "a counter nobody has seen yet is a change"
        );
    }

    #[test]
    fn an_unchanged_counter_reports_nothing_however_often_it_is_looked_at() {
        // The property the window rests on: sixty frames a second must not
        // produce sixty lines.
        let (world, _) = world_with_tally("buy", 3);
        let mut seen = std::collections::BTreeMap::new();

        assert_eq!(super::tally_changes(&world, &mut seen).len(), 1);

        for _frame in 0..60 {
            assert!(
                super::tally_changes(&world, &mut seen).is_empty(),
                "an unchanged counter reported a change"
            );
        }
    }

    #[test]
    fn a_counter_that_moves_reports_once_per_change_and_carries_the_new_value() {
        let (mut world, entity) = world_with_tally("buy", 0);
        let mut seen = std::collections::BTreeMap::new();
        let _ = super::tally_changes(&world, &mut seen);

        for expected in 1..=3 {
            world
                .get_mut::<Tally>(entity)
                .expect("the counter is there")
                .count = expected;

            assert_eq!(
                super::tally_changes(&world, &mut seen),
                vec![("buy".to_owned(), expected)],
                "one line per change, carrying the value after it"
            );
            assert!(super::tally_changes(&world, &mut seen).is_empty());
        }
    }

    #[test]
    fn a_jump_of_three_in_one_tick_is_one_line_saying_three() {
        // Not three lines. The log records what the state is, not how it got
        // there - three clicks inside one tick are one change to the counter.
        let (mut world, entity) = world_with_tally("buy", 0);
        let mut seen = std::collections::BTreeMap::new();
        let _ = super::tally_changes(&world, &mut seen);

        world
            .get_mut::<Tally>(entity)
            .expect("the counter is there")
            .count = 3;

        assert_eq!(
            super::tally_changes(&world, &mut seen),
            vec![("buy".to_owned(), 3)]
        );
    }

    #[test]
    fn two_counters_are_reported_in_canonical_id_order() {
        let mut world = World::new();
        let first = world.spawn();
        let second = world.spawn();
        world
            .insert(first, Tally::new("alpha"))
            .expect("a fresh entity takes a component");
        world
            .insert(second, Tally::new("beta"))
            .expect("a fresh entity takes a component");

        let mut seen = std::collections::BTreeMap::new();

        assert_eq!(
            super::tally_changes(&world, &mut seen),
            vec![("alpha".to_owned(), 0), ("beta".to_owned(), 0)],
            "entity_ids is sorted, so two counters have a fixed order"
        );
    }

    #[test]
    fn a_world_with_no_counter_reports_nothing() {
        let mut world = World::new();
        world.spawn();
        let mut seen = std::collections::BTreeMap::new();

        assert!(super::tally_changes(&world, &mut seen).is_empty());
        assert!(seen.is_empty());
    }

    #[test]
    fn a_scene_that_names_the_input_buffer_keeps_its_own() {
        let text = "Scene(entities: [(components: {
                \"input\": (pending:[],readable:[]),
            })])
";
        let simulation = build(text).expect("a scene may name the buffer");

        // One entity, not two: the scene said where the buffer goes.
        assert_eq!(simulation.world.len(), 1);
    }

    /// A scene without one gets exactly one, appended after everything it named.
    #[test]
    fn a_scene_without_the_buffer_gets_exactly_one_appended() {
        let simulation = build(MOVING).expect("a scene this build can load");
        let ids = simulation.world.entity_ids();

        let carriers: Vec<_> = ids
            .iter()
            .filter(|entity| simulation.world.has::<Events<InputEvent>>(**entity))
            .collect();

        assert_eq!(carriers.len(), 1, "exactly one buffer");
        assert_eq!(
            carriers[0],
            ids.last().expect("the world is not empty"),
            "and it is last, so the scene's own ids did not move"
        );
    }

    /// Building the same text twice puts the buffer in the same place.
    #[test]
    fn the_insertion_is_the_same_every_time() {
        let first = build(MOVING).expect("a scene this build can load");
        let second = build(MOVING).expect("a scene this build can load");

        assert_eq!(
            canonical_dump(&first.world, &first.registry).expect("dumps"),
            canonical_dump(&second.world, &second.registry).expect("dumps"),
        );
    }

    /// An action name no recording could hold is refused at load, with the entity.
    #[test]
    fn a_hit_action_outside_the_charset_is_refused_with_its_entity() {
        let text = "Scene(entities: [(components: {
                \"transform\": (x: 0.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),
            }), (components: {
                \"hitrect\": (half_width: 1.0, half_height: 1.0, action: \"buy thing\", value: 1),
            })])
";

        let Err(error) = build(text) else {
            panic!("a space is not an identifier");
        };

        match &error {
            SceneStartError::HitAction { index, name } => {
                assert_eq!(*index, 1);
                assert_eq!(name, "buy thing");
            }
            other => panic!("expected a hit action error, got {other:?}"),
        }

        assert_eq!(
            error.to_string(),
            "entity 1 has a hitrect whose action is \"buy thing\", which is not a usable action \
             name; an action name is a non-empty run of ASCII letters, digits, `_`, `-` and `.`, \
             so that a recording of the click can hold it on one line without a quoting rule"
        );
    }

    #[test]
    fn the_whole_engine_component_set_is_registered() {
        let simulation = build(MOVING).expect("loads");

        assert_eq!(simulation.registry.len(), 14);
        assert!(canonical_dump(&simulation.world, &simulation.registry).is_ok());
    }

    /// A shake in the file is state the run advances, which is what gives the
    /// determinism case something to disagree about.
    #[test]
    fn a_shake_from_the_file_decays_as_the_run_goes_on() {
        let text = "Scene(entities: [(components: {\n\
            \x20   \"camera\": (x: 0.0, y: 0.0, zoom: 1.0),\n\
            \x20   \"shake\": (amplitude: 4.0, frequency: 1.0, decay: 0.5, phase: 0.0, \
                     cutoff: 0.25, base_x: 0.0, base_y: 0.0),\n\
            })])\n";
        let mut simulation = build(text).expect("loads");
        let eye = simulation.world.entity_ids()[0];

        let amplitude = |simulation: &super::Simulation| {
            simulation
                .world
                .get::<Shake>(eye)
                .expect("the eye shakes")
                .amplitude
        };
        let before = amplitude(&simulation);

        simulation
            .scheduler
            .run(&mut simulation.world, &SystemContext::new(0));

        assert!(amplitude(&simulation) < before, "the shake has to decay");
    }

    // --- The shipped drop scene (M5b.4) ------------------------------------
    //
    // Everything below is about `scenes/physics_drop.ron`, the content whose
    // picture `frame.rs` blesses. These are the simulation half and need no GPU;
    // the picture half is over there, and both read the scene through
    // `drop_scene`.

    /// Half-extents the scene gives every falling box, in world units.
    const BOX_HALF_Y: f32 = 0.6;

    /// Where the top of the ground is: its centre plus its half-height.
    const GROUND_TOP: f32 = -5.37 + 0.43;

    /// Where a box comes to rest on the ground, and on another box.
    const ON_GROUND: f32 = GROUND_TOP + BOX_HALF_Y;
    const ON_A_BOX: f32 = ON_GROUND + 2.0 * BOX_HALF_Y;

    /// The scene, advanced `ticks` times through the mode's own scheduler.
    fn drop_world_at(ticks: u64) -> super::Simulation {
        let mut simulation = build(&drop_scene::text()).expect("the shipped scene loads");
        for tick in 0..ticks {
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
        }
        simulation
    }

    /// Every dynamic body in the world, in canonical entity order.
    fn falling_bodies(world: &World) -> Vec<RigidBody> {
        world
            .entity_ids()
            .into_iter()
            .filter_map(|entity| world.get::<RigidBody>(entity).ok().map(|body| *body))
            .filter(|body| body.kind == RigidBody::DYNAMIC)
            .collect()
    }

    /// The shipped scene is the world the picture assumes: one ground and four
    /// boxes, each with a region of its own.
    #[test]
    fn the_drop_scene_is_a_ground_and_four_boxes_with_five_distinct_regions() {
        let simulation = drop_world_at(0);
        let world = &simulation.world;

        let bodies: Vec<RigidBody> = world
            .entity_ids()
            .into_iter()
            .filter_map(|entity| world.get::<RigidBody>(entity).ok().map(|body| *body))
            .collect();

        assert_eq!(bodies.len(), 5, "one ground and four boxes");
        assert_eq!(
            bodies.iter().filter(|b| b.kind == RigidBody::FIXED).count(),
            1,
            "exactly one body is the ground"
        );
        assert_eq!(falling_bodies(world).len(), 4);

        // Each sprite has a region nobody else has, which is the rule M3.10
        // bought and `ProjektPlan.md` §9.1 records: two bodies sharing a colour
        // have no visible boundary where they touch, and a stack is exactly
        // where they touch.
        //
        // Collected here rather than through `sprite_batch::region_names_of`,
        // which would do the same thing: that module is render-gated and this
        // test runs in the headless configuration too, which is steps seven to
        // nine of the verification set.
        let names: std::collections::BTreeSet<String> = world
            .entity_ids()
            .into_iter()
            .filter_map(|entity| world.get::<Sprite>(entity).ok().map(|s| s.region.clone()))
            .collect();
        assert_eq!(
            names.len(),
            5,
            "five bodies, five region names, no two the same: {names:?}"
        );
    }

    /// The scene lands well before the tick the picture is taken at.
    ///
    /// **The blessed tick has to be after the first contact** — before it, every
    /// body is in free fall and the picture would say nothing about the solver
    /// that arithmetic could not. This asserts the consequence rather than the
    /// tick number: by [`drop_scene::TICK`] a box is resting on the ground, to
    /// within a hundredth of a world unit of where geometry says it must sit.
    #[test]
    fn the_drop_scene_lands_before_the_blessed_tick() {
        let simulation = drop_world_at(drop_scene::TICK);

        let resting = falling_bodies(&simulation.world)
            .into_iter()
            .filter(|body| (body.y - ON_GROUND).abs() < 0.01)
            .count();

        assert!(
            resting >= 1,
            "no box is resting on the ground at tick {}, so the reference image \
             is taken during free fall and shows nothing a contact solver did. \
             Boxes are at {:?}, the ground's top is at {GROUND_TOP} and a resting \
             box sits at {ON_GROUND}",
            drop_scene::TICK,
            falling_bodies(&simulation.world)
                .iter()
                .map(|body| body.y)
                .collect::<Vec<_>>()
        );
    }

    /// A box rests on another box, so the picture shows a body-to-body contact.
    ///
    /// Not the same claim as the one above and not implied by it: a scene whose
    /// boxes all land side by side on the ground would satisfy that one and would
    /// never put two dynamic bodies in contact. The height is what says so —
    /// [`ON_A_BOX`] is one box-height above the ground's resting level, and
    /// nothing but another box can hold a body there.
    #[test]
    fn one_box_rests_on_another_at_the_blessed_tick() {
        let simulation = drop_world_at(drop_scene::TICK);

        let stacked = falling_bodies(&simulation.world)
            .into_iter()
            .filter(|body| (body.y - ON_A_BOX).abs() < 0.05)
            .count();

        assert!(
            stacked >= 1,
            "no box is one box-height above the ground at tick {}, so nothing in \
             the picture rests on anything but the world. Boxes are at {:?}; a box \
             on the ground sits at {ON_GROUND} and a box on a box at {ON_A_BOX}",
            drop_scene::TICK,
            falling_bodies(&simulation.world)
                .iter()
                .map(|body| body.y)
                .collect::<Vec<_>>()
        );
    }

    /// Two boxes are visibly turned at the blessed tick.
    ///
    /// **This is what makes the reference image an assertion about M5b.4's seam
    /// at all.** The pair `rot_cos`/`rot_sin` crosses from the component into
    /// `SpritePlacement` untouched; if every body in the picture were level, a
    /// renderer that dropped the rotation entirely would draw the identical
    /// frame. `0.2` is about 11.5 degrees, which is a whole pixel of corner
    /// displacement on an 18 by 12 sprite and then some.
    #[test]
    fn two_boxes_are_visibly_turned_at_the_blessed_tick() {
        let simulation = drop_world_at(drop_scene::TICK);

        let turned: Vec<f32> = falling_bodies(&simulation.world)
            .into_iter()
            .map(|body| body.rot_sin)
            .filter(|rot_sin| rot_sin.abs() > 0.2)
            .collect();

        assert!(
            turned.len() >= 2,
            "only {} box(es) are turned by more than about 11 degrees at tick {}, \
             so the reference image would barely change if the rotation pair were \
             dropped on the way to the renderer. Sines: {:?}",
            turned.len(),
            drop_scene::TICK,
            falling_bodies(&simulation.world)
                .iter()
                .map(|body| body.rot_sin)
                .collect::<Vec<_>>()
        );
    }

    /// Two runs of the scene reach the same state, and the state is not the
    /// scene's own starting one.
    #[test]
    fn two_runs_of_the_drop_scene_agree_and_the_run_changes_something() {
        let dump_at = |ticks| {
            let simulation = drop_world_at(ticks);
            canonical_dump(&simulation.world, &simulation.registry).expect("everything registered")
        };

        assert_eq!(dump_at(drop_scene::TICK), dump_at(drop_scene::TICK));
        assert_ne!(
            dump_at(0),
            dump_at(drop_scene::TICK),
            "the scene did not move, so every comparison above is satisfied by a \
             world that stands still"
        );
    }

    #[test]
    fn a_scene_that_does_not_load_says_where_the_fault_is() {
        let Err(error) = build("Scene(entities: [(components: { \"nope\": (x: 1.0) })])") else {
            panic!("`nope` is not a component, so this must not build")
        };

        let message = error.to_string();
        assert!(
            message.contains("1:"),
            "the position has to survive: {message}"
        );
        assert!(message.contains("\"nope\""), "{message}");
    }

    // ------------------------------------------------- the committed burst scene

    /// The committed burst scene's text, read from the repository.
    fn burst_scene() -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes/burst.ron");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    }

    /// The shipped scene loads, and carries the two bursts it says it does.
    ///
    /// The click-demo precedent: a test over committed content checks that what
    /// the file claims is what the file does, so a scene that stopped loading
    /// fails here rather than in front of whoever tried to run it.
    #[test]
    fn the_committed_burst_scene_loads_and_carries_two_bursts() {
        let simulation = build(&burst_scene()).expect("the committed scene loads");

        let bursts: Vec<Burst> = simulation
            .world
            .entity_ids()
            .into_iter()
            .filter_map(|entity| simulation.world.get::<Burst>(entity).ok().map(|b| *b))
            .collect();

        assert_eq!(bursts.len(), 2, "the file declares two emitters");
        assert!(
            bursts.iter().all(|burst| !burst.spent()),
            "a scene's bursts start armed, not over: {bursts:?}"
        );
        // Two seeds, so the two are two bursts rather than one drawn twice.
        assert_ne!(bursts[0].seed, bursts[1].seed);
    }

    /// The run ages them, and the shorter one ends while the longer one runs.
    ///
    /// **What the scene's own comment claims, asserted rather than trusted.** The
    /// interesting frame is the one holding a spent burst and a live one at the
    /// same time, and the file is written to produce it at tick 12.
    #[test]
    fn the_burst_scene_reaches_a_tick_where_one_burst_is_over_and_one_is_not() {
        let mut simulation = build(&burst_scene()).expect("the committed scene loads");

        for tick in 0..12 {
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
        }

        let mut spent = 0;
        let mut running = 0;
        for entity in simulation.world.entity_ids() {
            let Ok(burst) = simulation.world.get::<Burst>(entity) else {
                continue;
            };
            if burst.spent() {
                spent += 1;
            } else {
                running += 1;
            }
        }

        assert_eq!(spent, 1, "exactly one burst is over at tick 12");
        assert_eq!(running, 1, "exactly one burst is still going at tick 12");
    }

    /// The age reaches the canonical dump, which is the whole of ADR-0044.
    ///
    /// Not a stored value (ADR-0008): the assertion is that the *field* is there
    /// and that it is the tick count, both read out of the dump this run wrote.
    #[test]
    fn a_bursts_age_is_in_the_canonical_dump_and_is_the_tick_count() {
        let mut simulation = build(&burst_scene()).expect("the committed scene loads");
        for tick in 0..7 {
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
        }

        let dump = canonical_dump(&simulation.world, &simulation.registry).expect("dumps");
        let ages: Vec<&str> = dump
            .lines()
            .filter(|line| line.trim_start().starts_with("burst "))
            .collect();

        assert_eq!(ages.len(), 2, "two bursts, two lines: {dump}");
        for line in ages {
            assert!(
                line.contains("age:7"),
                "the age did not reach the dump: {line}"
            );
        }
    }
}
