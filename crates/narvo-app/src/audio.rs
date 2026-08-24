//! The click demo's cue mapping: what its world state should sound like.
//!
//! # Why the extraction is here and not in `narvo-audio`
//!
//! ADR-0015 settled this shape for the renderer — *the renderer takes an
//! explicit buffer of scalars and never the ECS; the extraction from a `World`
//! lives in the crate that already sees both sides.* Audio is the same case with
//! the same answer: `narvo-audio` knows [`Cue`] and nothing about entities, and
//! this module is where a `World` and that vocabulary are both in scope.
//!
//! The alternative was ADR-0025's dev-edge precedent — put the extraction in
//! `narvo-audio` behind a dev-dependency on `narvo-ecs`. It was rejected
//! because it would make the audio crate's *production* API depend on which ECS
//! this engine uses, which is exactly what ADR-0002's facade exists to prevent,
//! and because the mapping below is **demo content** rather than engine
//! vocabulary: the sounds a click makes belong with `count_actions` and
//! `tally_changes`, which live next door for the same layering reason.
//!
//! No ADR was written for that, deliberately. It applies two decisions that are
//! already recorded rather than making a third.
//!
//! # This reads and never writes
//!
//! ADR-0028: *"Whatever consumes these cues must not write to the world: cues
//! are extracted from state, and nothing in ADR-0008's hash domain may move
//! because a sound played."* [`cues_of`] takes `&World`, so the compiler holds
//! that rather than a convention. What it remembers between ticks lives in
//! [`CueMemory`], which the *runner* owns — outside the world, outside the
//! canonical dump, outside the state hash. That is the shape `tally_changes`
//! already uses for its `seen` map.
//!
//! # Per tick, never per frame
//!
//! A frame runs zero or more ticks (ADR-0003), so a cue list built once per
//! frame would depend on how fast frames happen and the oracle would be
//! meaningless. Both runners call [`cues_of`] once per tick — `SceneHost::tick`
//! for the window, and the headless runner's own loop — and the counter deltas
//! below are what make that lossless: `Tally::count` is cumulative, so three
//! buys in one catch-up burst are three clicks rather than one.

use std::collections::BTreeMap;

use narvo_audio::{Channel, Cue, SoundLibrary};
use narvo_ecs::{EntityId, Tally, World};

/// The action the click demo counts, and the only one that makes a sound.
///
/// The same literal the scene's `hitrect` and the mapping's binding both carry
/// (`scenes/click_counter.ron`, `scenes/click_counter.mapping.ron`), which is
/// what makes the cue hang on the *action* rather than on the device: a mouse
/// click and the space bar arrive here indistinguishable.
pub(crate) const BUY: &str = "buy";

/// The count at which the second music layer arrives.
pub(crate) const LAYER_AT: i64 = 3;

/// How long the layer takes to fade in, in ticks.
///
/// Sixty ticks is one second at the default tick rate, which is slow enough to
/// be audibly a fade rather than a switch.
pub(crate) const LAYER_TWEEN_TICKS: u32 = 60;

/// What the extractor remembers between ticks.
///
/// Owned by the runner rather than by the world, for the reason in the module
/// documentation. A fresh one produces the opening music cue again, which is
/// correct: a reloaded or replayed world is a new run.
#[derive(Debug)]
pub(crate) struct CueMemory {
    /// The last count seen for each counter, by entity.
    counts: BTreeMap<EntityId, i64>,
    /// Whether the music bed has been started.
    started: bool,
    /// Whether the layer has been asked for.
    layered: bool,
}

impl CueMemory {
    /// A memory whose baseline is the world as it stands.
    ///
    /// **Seeded from the world rather than started empty, and the difference is
    /// audible.** A counter's *value* is state and its *movement* is the event;
    /// an empty memory would read a scene authored with `count: 5` as five
    /// purchases arriving at once. Taking the baseline here, before the first
    /// tick runs, also keeps the other end honest: a buy on tick 0 moves the
    /// counter from the seeded 0 to 1 and clicks, where a baseline established
    /// after that tick would have swallowed it.
    pub(crate) fn new(world: &World) -> Self {
        let counts = world
            .entity_ids()
            .into_iter()
            .filter_map(|entity| {
                let tally = world.get::<Tally>(entity).ok()?;
                Some((entity, tally.count))
            })
            .collect();

        Self {
            counts,
            started: false,
            layered: false,
        }
    }
}

/// The cues tick `tick` produced, in a fixed order.
///
/// Returns nothing at all for a world with no [`Tally`] in it. That is not a
/// guard against a missing component but the mapping's actual scope: this is the
/// click demo's sound, and a world with no counter is not the click demo. It is
/// also what keeps the headless runner's call site free of side effects for
/// every mode the determinism suite drives.
///
/// # Order within a tick
///
/// The bed, then one click per buy, then the layer. Fixed so that a test can
/// assert the list as a value, and so that the layer's fade starts after the
/// click that triggered it rather than in an order that depends on iteration.
pub(crate) fn cues_of(world: &World, tick: u64, memory: &mut CueMemory) -> Vec<Cue> {
    // Entities in canonical ascending order, not query order: query order is
    // archetype order and is not reproducible. Nothing here is hashed, but a
    // cue list that reordered itself between runs would fail its own oracle.
    let tallies: Vec<(EntityId, String, i64)> = world
        .entity_ids()
        .into_iter()
        .filter_map(|entity| {
            let tally = world.get::<Tally>(entity).ok()?;
            Some((entity, tally.action.clone(), tally.count))
        })
        .collect();

    if tallies.is_empty() {
        return Vec::new();
    }

    let mut cues = Vec::new();

    if !memory.started {
        memory.started = true;
        cues.push(Cue::play(tick, Channel::Music, SoundLibrary::MUSIC_BASE));
    }

    let mut bought = 0_i64;
    for (entity, action, count) in &tallies {
        // Cumulative counters, so the delta is exactly how many arrived — the
        // property that makes a catch-up burst lossless.
        //
        // A counter this memory has never seen establishes a baseline and
        // sounds nothing — the same rule [`CueMemory::new`] applies at the
        // start, here for an entity that appeared mid-run. `max(0)` covers the
        // other direction, since nothing decrements a tally today and a
        // negative delta could only mean the world moved under a memory that
        // did not.
        let arrived = match memory.counts.insert(*entity, *count) {
            None => 0,
            Some(previous) => (count - previous).max(0),
        };

        if action != BUY {
            continue;
        }

        for _ in 0..arrived {
            cues.push(Cue::play(tick, Channel::Sfx, SoundLibrary::CLICK));
        }
        bought += count;
    }

    if !memory.layered && bought >= LAYER_AT {
        memory.layered = true;
        cues.push(Cue::layer_on(
            tick,
            Channel::Music,
            SoundLibrary::MUSIC_LAYER,
            0.0,
            LAYER_TWEEN_TICKS,
        ));
    }

    cues
}

#[cfg(test)]
mod tests {
    use super::{BUY, CueMemory, LAYER_TWEEN_TICKS, cues_of};
    use narvo_audio::{Channel, Cue, NullSink, Op, Samples, Sink, SoundLibrary};
    use narvo_ecs::{ComponentRegistry, Events, SystemContext, Tally, World};
    use narvo_input::InputEvent;

    /// The committed click scene, loaded through the real loader.
    ///
    /// Read from the source tree rather than from `target/`, so the test
    /// exercises the content the repository ships rather than a copy of it.
    fn click_world() -> crate::sim::Simulation {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scenes")
            .join("click_counter.ron");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        crate::sim::scene_file::build(&text).expect("the committed click scene loads")
    }

    /// Pushes events into the world's input buffer, the way the window's
    /// `InputFeed::deliver` does — `sim::feed` is mode-dispatched and reaches
    /// `unreachable!` for `SceneFile`.
    fn feed(world: &mut World, events: Vec<InputEvent>) {
        if events.is_empty() {
            return;
        }
        let entity = world
            .entity_ids()
            .into_iter()
            .find(|entity| world.has::<Events<InputEvent>>(*entity))
            .expect("the click scene carries an input buffer");
        let mut buffer = world
            .get_mut::<Events<InputEvent>>(entity)
            .expect("the entity was just found by carrying it");
        for event in events {
            buffer.send(event);
        }
    }

    fn buy() -> InputEvent {
        InputEvent::new(BUY, 1).expect("\"buy\" is a valid action name")
    }

    /// Runs the click world for `ticks`, feeding `inputs(tick)` before each tick
    /// and collecting the cues each tick produced.
    ///
    /// The M5.5 in-process precedent (`sim::input`'s own `run`), pointed at the
    /// scene-file world.
    fn run(ticks: u64, mut inputs: impl FnMut(u64) -> Vec<InputEvent>) -> Vec<Cue> {
        let mut simulation = click_world();
        let mut memory = CueMemory::new(&simulation.world);
        let mut cues = Vec::new();

        for tick in 0..ticks {
            feed(&mut simulation.world, inputs(tick));
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
            cues.extend(cues_of(&simulation.world, tick, &mut memory));
        }

        cues
    }

    /// Buys at ticks 1, 4 and 9. The third one crosses the threshold.
    fn script(tick: u64) -> Vec<InputEvent> {
        if [1, 4, 9].contains(&tick) {
            vec![buy()]
        } else {
            Vec::new()
        }
    }

    /// (a) The list as a value.
    ///
    /// Asserted whole, not merely compared against a second run: two runs of an
    /// extraction that returned nothing would agree perfectly.
    #[test]
    fn the_scripted_run_produces_exactly_this_cue_list() {
        let cues = run(12, script);

        assert_eq!(
            cues,
            vec![
                // The bed, at the first tick that has a counter to watch.
                Cue::play(0, Channel::Music, SoundLibrary::MUSIC_BASE),
                // An event fed before tick 1 is counted by that tick's systems,
                // so the click lands on the tick that counted it.
                Cue::play(1, Channel::Sfx, SoundLibrary::CLICK),
                Cue::play(4, Channel::Sfx, SoundLibrary::CLICK),
                Cue::play(9, Channel::Sfx, SoundLibrary::CLICK),
                // Third buy, so the layer arrives on the same tick as the click
                // that earned it, after it.
                Cue::layer_on(
                    9,
                    Channel::Music,
                    SoundLibrary::MUSIC_LAYER,
                    0.0,
                    LAYER_TWEEN_TICKS
                ),
            ]
        );
    }

    /// (b) Two independent runs agree.
    #[test]
    fn two_independent_runs_produce_the_same_list() {
        assert_eq!(run(12, script), run(12, script));
    }

    /// (c) The replay half: the same recorded input, fed to a world built again
    /// from the same scene text, reproduces the list.
    ///
    /// **What this is and is not.** It replays an input sequence, which is what
    /// the cue oracle is about. It is not `narvo` in replay mode over the click
    /// scene: `Mode::SceneFile::actions()` is `&[]` (`sim.rs:105-110`) and
    /// `validate_recording` rejects a recording that carries input for a mode
    /// consuming none (`sim.rs:274-278`), so no recording of this scene can
    /// exist today. M5_6b.md §1c-ter carries that as a standing gap, and this
    /// test does not paper over it.
    #[test]
    fn replaying_the_same_input_sequence_reproduces_the_list() {
        let recorded: Vec<(u64, Vec<InputEvent>)> =
            (0..12).map(|tick| (tick, script(tick))).collect();

        let original = run(12, script);
        let replayed = run(12, |tick| {
            recorded
                .iter()
                .find(|(at, _)| *at == tick)
                .map(|(_, events)| events.clone())
                .unwrap_or_default()
        });

        assert_eq!(original, replayed);
    }

    /// (d) A different input sequence must produce a different list, or the
    /// oracle is measuring nothing.
    #[test]
    fn a_different_input_sequence_changes_the_list() {
        let other = run(12, |tick| {
            if [2, 3].contains(&tick) {
                vec![buy()]
            } else {
                Vec::new()
            }
        });

        assert_ne!(run(12, script), other);

        // And it differs the way it should: two buys never reach the threshold.
        assert!(
            !other.iter().any(|cue| cue.op == Op::LayerOn),
            "two buys must not raise the layer: {other:?}"
        );
        assert_eq!(
            other.iter().filter(|cue| cue.op == Op::Play).count(),
            3,
            "the bed plus two clicks"
        );
    }

    /// Several buys inside one tick are several clicks, which is the whole
    /// reason the mapping reads a cumulative delta instead of a change flag.
    #[test]
    fn three_buys_in_one_tick_are_three_clicks() {
        let cues = run(3, |tick| {
            if tick == 1 {
                vec![buy(), buy(), buy()]
            } else {
                Vec::new()
            }
        });

        let clicks = cues
            .iter()
            .filter(|cue| cue.op == Op::Play && cue.sound == SoundLibrary::CLICK)
            .count();
        assert_eq!(clicks, 3, "{cues:?}");
        assert!(
            cues.iter().any(|cue| cue.op == Op::LayerOn),
            "three in one tick still crosses the threshold: {cues:?}"
        );
    }

    /// A world with no counter is not the click demo and makes no sound.
    #[test]
    fn a_world_without_a_tally_produces_nothing() {
        let mut world = World::new();
        world.spawn();
        let mut memory = CueMemory::new(&world);

        for tick in 0..5 {
            assert!(cues_of(&world, tick, &mut memory).is_empty());
        }
    }

    /// The extraction may not move anything the state hash covers.
    #[test]
    fn extraction_leaves_the_canonical_dump_untouched() {
        let mut simulation = click_world();
        let mut registry = ComponentRegistry::new();
        narvo_ecs::register_engine_components(&mut registry).expect("a fresh registry");
        registry
            .register_component::<Events<InputEvent>>("input")
            .expect("a fresh registry");

        feed(&mut simulation.world, vec![buy()]);
        simulation
            .scheduler
            .run(&mut simulation.world, &SystemContext::new(0));

        let before = narvo_ecs::canonical_dump(&simulation.world, &registry)
            .expect("every component is registered");

        let mut memory = CueMemory::new(&simulation.world);
        for tick in 0..20 {
            let _ = cues_of(&simulation.world, tick, &mut memory);
        }

        let after = narvo_ecs::canonical_dump(&simulation.world, &registry)
            .expect("every component is registered");

        assert_eq!(before, after, "reading cues moved world state");
    }

    /// The threshold is crossed once, however long the run continues.
    #[test]
    fn the_layer_arrives_once_and_not_again() {
        let cues = run(40, script);
        assert_eq!(
            cues.iter().filter(|cue| cue.op == Op::LayerOn).count(),
            1,
            "{cues:?}"
        );
    }

    /// The counter is world state, so a fresh memory over an already-counted
    /// world does not replay its history as clicks.
    #[test]
    fn a_fresh_memory_does_not_reissue_past_clicks() {
        let mut simulation = click_world();
        let mut memory = CueMemory::new(&simulation.world);

        for tick in 0..12 {
            feed(&mut simulation.world, script(tick));
            simulation
                .scheduler
                .run(&mut simulation.world, &SystemContext::new(tick));
            let _ = cues_of(&simulation.world, tick, &mut memory);
        }

        // A reload hands the same world to a new memory. The bed starts again,
        // the layer is re-raised because the count is already past it, and no
        // click is invented for the three that already happened.
        let mut reloaded = CueMemory::new(&simulation.world);
        let cues = cues_of(&simulation.world, 12, &mut reloaded);

        assert_eq!(
            cues,
            vec![
                Cue::play(12, Channel::Music, SoundLibrary::MUSIC_BASE),
                Cue::layer_on(
                    12,
                    Channel::Music,
                    SoundLibrary::MUSIC_LAYER,
                    0.0,
                    LAYER_TWEEN_TICKS
                ),
            ]
        );
    }

    #[test]
    fn the_scene_the_demo_ships_carries_the_counter_this_mapping_expects() {
        let simulation = click_world();
        let actions: Vec<String> = simulation
            .world
            .entity_ids()
            .into_iter()
            .filter_map(|entity| simulation.world.get::<Tally>(entity).ok())
            .map(|tally| tally.action.clone())
            .collect();

        assert_eq!(actions, vec![BUY.to_owned()]);
    }

    /// Every cue the demo produces names a sound the run can actually play.
    ///
    /// **This is the headless half of M6b.2b's third decision.** Until then only
    /// the device-backed sink resolved a sound, and its one construction site is
    /// production code with no test on it — so the resolution existed exactly
    /// where nothing exercises it. This runs in the configuration verification
    /// steps seven to nine build, which has no `device` feature and no kira in
    /// its dependency tree at all.
    ///
    /// It is a real property and not a stand-in for one: a cue the library cannot
    /// answer is silence on a machine that has a speaker, and this is the cheapest
    /// place the demo can say it does not emit any.
    #[test]
    fn every_cue_the_demo_produces_resolves_in_the_library() {
        let library = SoundLibrary::new();
        let cues = run(12, script);

        for cue in &cues {
            assert!(
                library.get(cue.sound).is_some(),
                "{cue:?} names a sound the library cannot answer"
            );
        }

        // Without this the assertion above is vacuously true over an empty list,
        // which is the shape that makes a green test say nothing.
        assert!(
            cues.len() >= 3,
            "the demo produced only {} cues",
            cues.len()
        );
    }

    /// And the sink every configuration compiles takes them, resolving each one.
    ///
    /// The recording sink keeps what it was given, so the count is the evidence
    /// that submission happened rather than that it compiled.
    #[test]
    fn a_null_sink_takes_every_cue_the_demo_produces() {
        let library = SoundLibrary::new();
        let cues = run(12, script);
        let mut sink = NullSink::recording();

        for cue in &cues {
            sink.submit(cue, &library);
        }

        assert_eq!(sink.received(), cues.as_slice());
        assert!(
            cues.len() >= 3,
            "the demo produced only {} cues",
            cues.len()
        );
    }

    /// A consumer's own sound reaches the same sink through the same call.
    ///
    /// Here rather than in `narvo-audio` because this is the crate that plays
    /// the part a game would: it holds the library, hands out the handle, and
    /// puts it in a cue. Nothing about it needs a device.
    #[test]
    fn a_consumer_registered_sound_travels_the_same_path_as_a_built_in_one() {
        let mut library = SoundLibrary::new();
        let blip = library.register(
            "blip",
            Samples {
                sample_rate: 48_000,
                mono: vec![0.25, -0.25, 0.5, -0.5],
                loops: false,
            },
        );

        let mine = Cue::play(7, Channel::Sfx, blip);
        let theirs = Cue::play(7, Channel::Sfx, SoundLibrary::CLICK);

        let mut sink = NullSink::recording();
        sink.submit(&mine, &library);
        sink.submit(&theirs, &library);

        assert_eq!(sink.received(), &[mine, theirs]);
        assert_eq!(mine.note(&library), "cue tick 7 sfx play blip");
        assert_eq!(theirs.note(&library), "cue tick 7 sfx play click");
        assert!(library.get(blip).is_some());
    }
}
