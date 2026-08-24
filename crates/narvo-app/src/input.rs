//! Window input: the one place winit is translated, and the rule by which the
//! result reaches a tick.
//!
//! # Two halves, and the seam between them is the point
//!
//! **The paragraph below said "the only function in this workspace" and that was
//! never true.** M6b.1 measured **seven** functions naming a winit type in a
//! signature — three here, four in [`crate::window`]'s event loop — and M6b.8
//! re-measured the same seven. It is corrected rather than deleted, because the
//! *rule* it was reaching for holds and is what ADR-0026's M6b.8 amendment keeps:
//! **two files know winit, and no third does.** Outside this module and
//! `window.rs` no line in the workspace names a winit type; the only other
//! occurrence of the string is `FORBIDDEN_IN_HEADLESS` in `xtask`.
//!
//! [`translate`] is the front door of that boundary. It is a table and nothing
//! else: a key code becomes a [`Control`](narvo_input::Control), a press or a
//! release becomes an [`Edge`](narvo_input::Edge), and everything downstream of
//! it is `narvo-input`'s vocabulary. That is why it is gated behind `render`
//! while the rest of this module is not — [`InputFeed`], which owns the delivery
//! rule, compiles and is tested in the headless configuration, with synthetic
//! device events and no window anywhere.
//!
//! The cut is the acceptance criterion of the design rather than a tidiness
//! preference. A place outside these two that knew about winit would mean the
//! delivery rule could only be tested through a window, and `ProjektPlan.md` §7
//! has no way to verify anything that needs one.
//!
//! # The pointer lives here too, since M6b.8
//!
//! [`Pointer`] is D23's Weg C: the position a `CursorMoved` leaves behind, and
//! what a press at it means. It moved out of `window.rs` — which carries no
//! tests — for exactly one reason, and the reason is the eleven tests at the foot
//! of this file that could not previously be written at all. It names no winit
//! type; a position crosses the boundary as two `f32`.
//!
//! # Delivery, and why it is ADR-0012's rule rather than a new one
//!
//! The headless runner writes a tick's input into the world's
//! `Events<InputEvent>` buffer *between* ticks, and `rotate_events` — first in
//! the run order — makes it readable throughout the tick that follows
//! (ADR-0012, Decision 5). This module does exactly the same thing at exactly
//! the same kind of seam: [`Runner::draw`](crate::window) calls
//! [`InputFeed::deliver`] after the reload check and before `FrameLoop::step`,
//! which is between the last tick of the previous frame and the first of this
//! one — the boundary ADR-0022 Decision 4 already established for the reload.
//!
//! Everything the window needs then falls out of ADR-0011's rotation instead of
//! being invented here:
//!
//! - **Delivered exactly once.** `rotate` moves `pending` into `readable` and
//!   drops what was readable before, so an event is visible in one tick and no
//!   other.
//! - **An advance that runs no ticks keeps the queue.** No tick means no
//!   rotation, so the events stay in `pending` and the next frame's are appended
//!   to them. Nothing is lost and nothing arrives twice.
//! - **Catch-up ticks after the first get nothing.** The second rotation of an
//!   advance finds `pending` empty, so only the first tick of an advance that
//!   really ticks sees the input.
//!
//! One delivery semantics for both runners was the goal. A second one would be
//! the failure mode ADR-0011's Context describes: one channel with two
//! behaviours, and a bug nobody can find afterwards.

#[cfg(feature = "render")]
use narvo_ecs::HitRect;
use narvo_ecs::{Events, World};
use narvo_input::{DeviceEvent, InputEvent, Mapping};
#[cfg(feature = "render")]
use narvo_render2d::Projection;
#[cfg(feature = "render")]
use narvo_view2d::hit_test;

/// What one [`InputFeed::deliver`] did, so a caller and a test can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Nothing had been queued.
    Idle,
    /// This many events reached the world's buffer.
    Delivered(usize),
    /// This many events were dropped because the world carries no buffer.
    ///
    /// Not an error and not a panic. **The comment here was wrong for one
    /// milestone and is corrected rather than deleted:** it said a
    /// scene-constituted world does not carry an `Events<InputEvent>`, which
    /// M5.4 made false in the same commit that wrote it — `sim::scene_file`
    /// registers the buffer and appends a carrier for it. What is left is the
    /// demo path built in code and any world a caller assembles itself, which
    /// may still have none, so the branch stays and is reported rather than
    /// swallowed — see [`InputFeed::deliver`] for the note that goes with it.
    Dropped(usize),
}

/// The window's input queue: device events in, one tick's worth of actions out.
///
/// Holds the mapping for the run, the device events collected since the last
/// frame, and the one flag that keeps the drop note from repeating.
#[derive(Debug)]
pub struct InputFeed {
    mapping: Mapping,
    /// Device events since the last delivery, in the order they arrived.
    ///
    /// Raw rather than mapped, so that the whole translation happens once per
    /// frame at one place rather than inside a winit callback.
    queued: Vec<DeviceEvent>,
    /// Events that arrived already mapped, from a source with no device term -
    /// today, a click answered by hit testing.
    mapped: Vec<InputEvent>,
    /// Whether the "no buffer" note has already been printed this run.
    noted: bool,
}

impl InputFeed {
    /// A feed driven by `mapping`.
    #[must_use]
    pub fn new(mapping: Mapping) -> Self {
        Self {
            mapping,
            queued: Vec::new(),
            mapped: Vec::new(),
            noted: false,
        }
    }

    /// Adds one device event to the queue.
    pub fn push(&mut self, event: DeviceEvent) {
        self.queued.push(event);
    }

    /// Queues an already-mapped event, for a source that is not a mapping.
    ///
    /// A click has no `Control` and no `Edge`: hit testing turns a world point
    /// straight into an action, so there is nothing for `Mapping::map` to do.
    /// It joins the same queue all the same, so that a frame's keys and its
    /// clicks reach the tick together and in the order they happened.
    ///
    /// `render`-gated for the reason [`crate::watch::Watcher::path`] is: its
    /// only caller is the window's click path, and no test here reaches it, so
    /// an ungated version is dead in the headless test build. Found by M5.7
    /// while gating the three M5.6c2 named — a fourth of the same shape, fixed
    /// the same way rather than left as the odd one out.
    #[cfg(feature = "render")]
    pub fn push_event(&mut self, event: InputEvent) {
        self.mapped.push(event);
    }

    /// How many device events are waiting.
    ///
    /// Test-only: production never asks, because [`discard`](Self::discard) and
    /// [`deliver`](Self::deliver) are both unconditional. It exists so the
    /// reload test can state what it is checking — that the queue was emptied —
    /// rather than inferring it from a later delivery.
    #[cfg(test)]
    pub fn queued(&self) -> usize {
        self.queued.len() + self.mapped.len()
    }

    /// Throws the queue away, because the world it was meant for is gone.
    ///
    /// Called when a reload swaps the world (ADR-0022 Decision 1). A
    /// reconstituted world is a *new run* — its tick counter restarts — so
    /// input aimed at the world that was running has nothing to arrive at. The
    /// alternative, letting it through, would deliver a keystroke meant for one
    /// world into a different one at a tick number that has just been reset,
    /// which is a repro nobody could read.
    ///
    /// Only the *undelivered* queue needs this. Anything already handed to the
    /// old world went into that world's buffer and is dropped with it.
    pub fn discard(&mut self) {
        self.queued.clear();
        self.mapped.clear();
    }

    /// Maps everything queued and puts it where the next tick will read it.
    ///
    /// Call between frames, after the reload check and before any tick runs.
    ///
    /// # The drop path
    ///
    /// If the world carries no `Events<InputEvent>`, the mapped events are
    /// dropped and a note is printed **once per run**. Silently swallowing them
    /// is forbidden: a window whose keys do nothing, with no explanation
    /// anywhere, is the kind of failure this project spends its time removing.
    /// Printing it every frame would be as bad in the other direction — sixty
    /// lines a second is not a diagnostic — so the flag makes it exactly one.
    ///
    /// **It drops rather than inserts, and that is load-bearing.** The obvious
    /// "fix" — insert a buffer into a world that lacks one — would be a defect
    /// with a delay on it. `World::insert` does not consult the registry
    /// (`narvo-ecs/src/world.rs:199-213`), so the insert would *succeed*; the
    /// failure would arrive later and elsewhere, when something asked that world
    /// for a canonical dump and got `EcsError::UnregisteredComponent`. A window
    /// never dumps, so it would not even be the window that broke. Giving a
    /// scene-constituted world an input buffer is M5.4's decision to make in the
    /// registry, where the dump can see it.
    ///
    /// It also does not go through `sim::feed`, which would have been the other
    /// obvious reuse: that function is mode-dispatched and reaches
    /// `unreachable!` for every mode but `Input` (`crate::sim`), so a window
    /// pointed at a scene would have panicked rather than dropped.
    pub fn deliver(&mut self, world: &mut World) -> Delivery {
        if self.queued.is_empty() && self.mapped.is_empty() {
            return Delivery::Idle;
        }

        // Device events first, then the already-mapped ones: within a frame a
        // key and a click are both "what happened", and this is the one order
        // that is stated rather than emergent.
        let mut events = self.mapping.map(&self.queued);
        events.append(&mut self.mapped);
        self.queued.clear();

        if events.is_empty() {
            return Delivery::Idle;
        }

        let Some(buffer) = buffer_entity(world) else {
            if !self.noted {
                self.noted = true;
                eprintln!(
                    "input is mapped but this world has no \"input\" buffer, so {} event(s) \
                     were dropped; a world that consumes input carries an \
                     Events<InputEvent> component. This is said once per run",
                    events.len()
                );
            }
            return Delivery::Dropped(events.len());
        };

        let count = events.len();
        let mut events_in = world
            .get_mut::<Events<InputEvent>>(buffer)
            .expect("the entity was just found by carrying this component");
        for event in events {
            events_in.send(event);
        }

        Delivery::Delivered(count)
    }
}

/// Where the pointer last was, and what a press there means.
///
/// # Why this is here and not in the window (D23, Weg C)
///
/// ADR-0026's content is that **exactly one place in this workspace knows
/// winit**, not that exactly one *function* maps. Until M6b.8 the pointer's
/// whole behaviour lived inline in `Runner::window_event` and `Runner::click`,
/// and `window.rs` carries no tests at all — measured, not assumed: it held
/// zero `#[test]` and zero `#[cfg(test)]`. The pieces were covered
/// individually (`hit_test` through the blessed click scene,
/// `Projection::screen_to_world` against its own inverse in `crate::frame`) and
/// the *composition* — a remembered position plus a press becomes an action —
/// was covered by nothing.
///
/// Moving it here changes no behaviour and buys exactly one thing: the
/// composition can be driven without a window. That is the whole of what Weg C
/// was chosen for, and the tests at the bottom of this module are the receipt.
///
/// # It does not go through the mapping, and that is the point
///
/// ADR-0025's device vocabulary has no term for a position, so a pointer cannot
/// be spelled as a `Control` and an `Edge`. That is why the alternative — widen
/// the mapping until it can carry a cursor — was refused: it would put a device
/// coordinate into a `.ron` file and into every recording, which is exactly what
/// ADR-0012's M5.2 amendment keeps out. A press is resolved to an *action* here,
/// and only the action travels.
///
/// `render`-gated because [`Pointer::press`] names [`Projection`] and
/// [`hit_test`], and both arrive with that feature. [`InputFeed`] above stays
/// ungated and stays tested in the headless configuration.
#[cfg(feature = "render")]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Pointer {
    /// Physical pixels from the target's top left, or `None` before the first
    /// move.
    ///
    /// winit reports a click without a position, so the position has to come
    /// from the last `CursorMoved`. A click before the pointer has ever moved
    /// has no point to test, which is what `None` means.
    at: Option<(f32, f32)>,
}

#[cfg(feature = "render")]
impl Pointer {
    /// A pointer that has not been seen yet.
    #[must_use]
    pub fn new() -> Self {
        Self { at: None }
    }

    /// Remembers a move, in physical pixels from the target's top left.
    ///
    /// Remembered and nothing more. A pointer *position* is not an input: D8
    /// keeps recordings at the action level, and a position that reached one
    /// would be a device term in a file.
    pub fn moved_to(&mut self, x: f32, y: f32) {
        self.at = Some((x, y));
    }

    /// Where it last was.
    ///
    /// Test-only, and `#[cfg(test)]` for exactly the reason
    /// [`InputFeed::queued`] is: production never asks, because
    /// [`press`](Self::press) reads the field directly. It exists so a test can
    /// state that a move was remembered rather than inferring it from a later
    /// press — the same distinction, one type along.
    #[cfg(test)]
    #[must_use]
    pub fn at(&self) -> Option<(f32, f32)> {
        self.at
    }

    /// The action a press at the remembered position sends, if it hits anything.
    ///
    /// The whole click path, and it is four steps with no state beyond the
    /// remembered point: the position becomes a world point through the *same*
    /// [`Projection`] the renderer draws with, [`hit_test`] picks the front-most
    /// rectangle under it, and that rectangle's own action and value become an
    /// [`InputEvent`].
    ///
    /// `None` when the pointer has never moved, when nothing is under it, or
    /// when the entity found carries no [`HitRect`] — three ordinary outcomes,
    /// not failures. `Some(Err(_))` when the rectangle names an action the
    /// vocabulary refuses, which is a content fault and is reported by the
    /// caller.
    #[must_use]
    pub fn press(
        &self,
        world: &World,
        projection: Projection,
    ) -> Option<Result<InputEvent, narvo_input::InputError>> {
        let (px, py) = self.at?;
        let [x, y] = projection.screen_to_world(px, py);
        let entity = hit_test(world, x, y)?;
        let rect = world.get::<HitRect>(entity).ok()?;
        Some(InputEvent::new(&rect.action, rect.value))
    }
}

/// The entity carrying the input buffer, if any does.
///
/// Found through the sorted [`World::entity_ids`] rather than through a query,
/// for the reason `sim::input` gives: query order is archetype order and is not
/// reproducible. Nothing here is hashed, but a lookup that picks a different
/// entity on different runs is a bug waiting for a second buffer to exist.
fn buffer_entity(world: &World) -> Option<narvo_ecs::EntityId> {
    world
        .entity_ids()
        .into_iter()
        .find(|entity| world.has::<Events<InputEvent>>(*entity))
}

/// The whole of winit, in one table.
///
/// Returns `None` for a key this project's vocabulary does not name, and for an
/// auto-repeat. Neither is an error: a mapping is a statement about the controls
/// a game uses, not about the keys a keyboard has.
///
/// # Auto-repeat is dropped here, and this is the place for it
///
/// `narvo_input::DeviceEvent` documents that it does not filter repeats and
/// says why: a pure function over a slice cannot know whether a press is a
/// repeat, and "whoever reads the device knows". This is that reader. winit
/// hands the answer over directly as `KeyEvent::repeat`
/// (`winit-0.30.13/src/event.rs:647`), so the obligation is discharged with one
/// comparison rather than with state.
///
/// Letting a repeat through would re-fire an `OnPress` binding for as long as a
/// key was held — a click that buys one upgrade would buy thirty a second — and
/// would re-send an `OnPressAndRelease` binding's press value into the state
/// hash on every repeat.
/// # Why this is three field reads and not the logic
///
/// A `winit::event::KeyEvent` **cannot be constructed outside winit**, so this
/// function cannot be called from a test. Its `platform_specific` field is
/// `pub(crate)` (`winit-0.30.13/src/event.rs:654`) and the module holding that
/// field's type is private (`src/lib.rs:213`, `mod platform_impl;`) — so there
/// is no portable way to build one, and on Windows the type does not implement
/// `Default` either.
///
/// That is a fact about winit rather than a design choice, and the design
/// answers it: everything that could be got wrong lives in
/// [`device_event`], which takes the three fields as arguments and is tested
/// exhaustively. What is left here is reading them off, which has no branch a
/// test could exercise that `device_event`'s tests do not already cover.
#[cfg(feature = "render")]
pub fn translate(event: &winit::event::KeyEvent) -> Option<DeviceEvent> {
    device_event(event.physical_key, event.state, event.repeat)
}

/// The translation itself, over the three fields that carry it.
#[cfg(feature = "render")]
#[must_use]
pub fn device_event(
    key: winit::keyboard::PhysicalKey,
    state: winit::event::ElementState,
    repeat: bool,
) -> Option<DeviceEvent> {
    use winit::event::ElementState;
    use winit::keyboard::PhysicalKey;

    if repeat {
        return None;
    }

    let PhysicalKey::Code(code) = key else {
        return None;
    };

    let control = control_of(code)?;

    Some(match state {
        ElementState::Pressed => DeviceEvent::press(control),
        ElementState::Released => DeviceEvent::release(control),
    })
}

/// The key-code table, as data.
///
/// A slice rather than a `match` so that the two properties worth checking —
/// that it is complete and that nothing appears twice — are testable by walking
/// it. A `match` would compile to a faster jump table and would be untestable
/// in exactly that way; at twenty-one entries and a handful of keystrokes per
/// frame, the lookup cost is not a number worth having.
///
/// **Every name is identical on both sides**, which is not a coincidence and is
/// also not a dependency. `narvo_input::Control` follows the W3C UI Events
/// `code` convention by its own decision (ADR-0025), and winit's `KeyCode`
/// documents itself as conforming to the same specification
/// (`winit-0.30.13/src/keyboard.rs:285-292`). Two independent choices of one
/// public convention, which is why this table is a straight rename and why a
/// mistranslation here would be visible on sight.
#[cfg(feature = "render")]
const TABLE: &[(winit::keyboard::KeyCode, narvo_input::Control)] = {
    use narvo_input::Control as C;
    use winit::keyboard::KeyCode as K;

    &[
        (K::KeyW, C::KeyW),
        (K::KeyA, C::KeyA),
        (K::KeyS, C::KeyS),
        (K::KeyD, C::KeyD),
        (K::ArrowUp, C::ArrowUp),
        (K::ArrowDown, C::ArrowDown),
        (K::ArrowLeft, C::ArrowLeft),
        (K::ArrowRight, C::ArrowRight),
        (K::Space, C::Space),
        (K::Enter, C::Enter),
        (K::Escape, C::Escape),
        (K::Digit0, C::Digit0),
        (K::Digit1, C::Digit1),
        (K::Digit2, C::Digit2),
        (K::Digit3, C::Digit3),
        (K::Digit4, C::Digit4),
        (K::Digit5, C::Digit5),
        (K::Digit6, C::Digit6),
        (K::Digit7, C::Digit7),
        (K::Digit8, C::Digit8),
        (K::Digit9, C::Digit9),
    ]
};

/// The control a key code stands for, if the vocabulary names it.
///
/// `KeyCode` is `#[non_exhaustive]` and has hundreds of variants; this covers
/// twenty-one of them on purpose (ADR-0025). A key outside the table — a
/// function key, punctuation, and notably the numeric keypad, whose `Numpad0`
/// … `Numpad9` and `NumpadEnter` are variants distinct from the digits and
/// `Enter` above — produces `None` and therefore nothing.
#[cfg(feature = "render")]
#[must_use]
pub fn control_of(code: winit::keyboard::KeyCode) -> Option<narvo_input::Control> {
    TABLE
        .iter()
        .find(|(key, _)| *key == code)
        .map(|(_, control)| *control)
}

#[cfg(test)]
mod tests {
    use super::{Delivery, InputFeed};
    use narvo_ecs::{Events, Scheduler, SystemContext, World, rotate_events};
    use narvo_input::{Control, DeviceEvent, InputEvent, Mapping};

    /// A mapping covering what the tests press.
    fn mapping() -> Mapping {
        narvo_input::from_str(
            r#"Mapping(bindings: [
                (control: KeyW, action: "thrust", emit: OnPressAndRelease(press: 1, release: 0)),
                (control: Digit1, action: "select", emit: OnPress(1)),
                (control: Digit2, action: "select", emit: OnPress(2)),
            ])"#,
        )
        .expect("the test mapping is valid")
    }

    /// A world carrying an input buffer, and the scheduler that rotates it.
    fn world_with_buffer() -> (World, Scheduler) {
        let mut world = World::new();
        let console = world.spawn();
        world
            .insert(console, Events::<InputEvent>::new())
            .expect("a fresh entity takes a component");

        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("input/rotate", rotate_events::<InputEvent>)
            .expect("the first system by that name");

        (world, scheduler)
    }

    /// What is readable in the world right now, as pairs.
    fn readable(world: &World) -> Vec<(String, i64)> {
        let entity = super::buffer_entity(world).expect("the world carries a buffer");
        world
            .get::<Events<InputEvent>>(entity)
            .expect("the buffer is there")
            .iter()
            .map(|event| (event.action().to_owned(), event.value()))
            .collect()
    }

    #[test]
    fn a_frames_input_arrives_in_the_next_tick_and_only_there() {
        let (mut world, scheduler) = world_with_buffer();
        let mut feed = InputFeed::new(mapping());

        feed.push(DeviceEvent::press(Control::KeyW));
        feed.push(DeviceEvent::press(Control::Digit1));
        assert_eq!(feed.deliver(&mut world), Delivery::Delivered(2));

        // Nothing is readable until the rotation runs (ADR-0011).
        assert_eq!(readable(&world), Vec::new());

        scheduler.run(&mut world, &SystemContext::new(1));
        assert_eq!(
            readable(&world),
            vec![("thrust".to_owned(), 1), ("select".to_owned(), 1)]
        );

        // ... and it is gone the tick after, delivered exactly once.
        scheduler.run(&mut world, &SystemContext::new(2));
        assert_eq!(readable(&world), Vec::new());
    }

    #[test]
    fn an_advance_that_runs_no_ticks_keeps_the_queue() {
        // The zero-tick frame. Two frames deliver, no tick runs in between, and
        // the third frame's tick sees both — nothing lost, nothing doubled.
        let (mut world, scheduler) = world_with_buffer();
        let mut feed = InputFeed::new(mapping());

        feed.push(DeviceEvent::press(Control::Digit1));
        assert_eq!(feed.deliver(&mut world), Delivery::Delivered(1));

        feed.push(DeviceEvent::press(Control::Digit2));
        assert_eq!(feed.deliver(&mut world), Delivery::Delivered(1));

        assert_eq!(readable(&world), Vec::new());

        scheduler.run(&mut world, &SystemContext::new(1));
        assert_eq!(
            readable(&world),
            vec![("select".to_owned(), 1), ("select".to_owned(), 2)],
            "both frames' input arrives together, in the order it happened"
        );
    }

    #[test]
    fn the_catch_up_ticks_after_the_first_see_nothing() {
        // An advance owing three ticks: the input belongs to the first of them.
        let (mut world, scheduler) = world_with_buffer();
        let mut feed = InputFeed::new(mapping());

        feed.push(DeviceEvent::press(Control::KeyW));
        feed.deliver(&mut world);

        scheduler.run(&mut world, &SystemContext::new(1));
        assert_eq!(readable(&world), vec![("thrust".to_owned(), 1)]);

        for tick in 2..=3 {
            scheduler.run(&mut world, &SystemContext::new(tick));
            assert_eq!(
                readable(&world),
                Vec::new(),
                "tick {tick} of the same advance must see nothing"
            );
        }
    }

    #[test]
    fn a_world_swap_throws_the_undelivered_queue_away() {
        // ADR-0022: a reconstituted world is a new run. Input aimed at the world
        // that was running has nothing to arrive at.
        let (mut world, scheduler) = world_with_buffer();
        let mut feed = InputFeed::new(mapping());

        feed.push(DeviceEvent::press(Control::KeyW));
        assert_eq!(feed.queued(), 1);

        feed.discard();
        assert_eq!(feed.queued(), 0);

        let (mut reloaded, _) = world_with_buffer();
        assert_eq!(feed.deliver(&mut reloaded), Delivery::Idle);

        scheduler.run(&mut world, &SystemContext::new(1));
        assert_eq!(readable(&world), Vec::new());
    }

    #[test]
    fn a_world_without_a_buffer_drops_the_events_and_says_so_once() {
        // A world assembled without a buffer. Not the scene-file case any
        // more - M5.4 gave those one - but a caller may still build a world
        // that has none, and a window pointed at it must say so rather than
        // swallow the input.
        let mut world = World::new();
        world.spawn();

        let mut feed = InputFeed::new(mapping());

        feed.push(DeviceEvent::press(Control::KeyW));
        assert_eq!(feed.deliver(&mut world), Delivery::Dropped(1));

        feed.push(DeviceEvent::press(Control::Digit1));
        assert_eq!(
            feed.deliver(&mut world),
            Delivery::Dropped(1),
            "the drop keeps happening; only the note is once"
        );
    }

    #[test]
    fn the_note_is_printed_once_and_the_flag_is_what_says_so() {
        let mut world = World::new();
        world.spawn();
        let mut feed = InputFeed::new(mapping());

        feed.push(DeviceEvent::press(Control::KeyW));
        feed.deliver(&mut world);
        assert!(feed.noted, "the first drop arms the flag");

        feed.push(DeviceEvent::press(Control::KeyW));
        feed.deliver(&mut world);
        assert!(feed.noted, "and it stays armed");
    }

    #[test]
    fn an_unbound_control_delivers_nothing_at_all() {
        let (mut world, _) = world_with_buffer();
        let mut feed = InputFeed::new(mapping());

        // `Escape` is not in the test mapping.
        feed.push(DeviceEvent::press(Control::Escape));
        assert_eq!(feed.deliver(&mut world), Delivery::Idle);
        assert_eq!(feed.queued(), 0, "the queue is drained either way");
    }

    #[test]
    fn an_empty_queue_is_idle_and_touches_nothing() {
        let (mut world, _) = world_with_buffer();
        let mut feed = InputFeed::new(mapping());

        assert_eq!(feed.deliver(&mut world), Delivery::Idle);
    }
}

#[cfg(all(test, feature = "render"))]
mod table_tests {
    use super::{TABLE, control_of, device_event};
    use narvo_input::{Control, Edge};
    use winit::event::ElementState;
    use winit::keyboard::{KeyCode, PhysicalKey};

    /// The table's size, as a content anchor.
    ///
    /// ADR-0008's third kind of literal: the number is not a magic constant but
    /// the statement "this many controls are translated". A control added to
    /// `narvo_input::Control` without a line here moves it, and the movement is
    /// the finding — `Control` is `#[non_exhaustive]`, so no compiler check can
    /// take this test's place from outside that crate.
    #[test]
    fn the_table_covers_twenty_one_controls() {
        assert_eq!(TABLE.len(), 21);
    }

    #[test]
    fn no_key_code_and_no_control_appears_twice() {
        for (index, (code, control)) in TABLE.iter().enumerate() {
            for (other_code, other_control) in TABLE.iter().skip(index + 1) {
                assert_ne!(code, other_code, "{code:?} is translated twice");
                assert_ne!(
                    control, other_control,
                    "{control:?} is produced by two key codes"
                );
            }
        }
    }

    #[test]
    fn every_entry_is_reachable_through_the_lookup() {
        for (code, control) in TABLE {
            assert_eq!(control_of(*code), Some(*control), "{code:?}");
        }
    }

    #[test]
    fn a_key_outside_the_vocabulary_is_not_translated() {
        // The numeric keypad is the near miss worth pinning: winit spells it
        // with variants of its own, so `Numpad1` is not `Digit1` and must not
        // silently become one.
        for code in [
            KeyCode::Numpad1,
            KeyCode::NumpadEnter,
            KeyCode::F1,
            KeyCode::Tab,
            KeyCode::ShiftLeft,
            KeyCode::Comma,
        ] {
            assert_eq!(control_of(code), None, "{code:?}");
        }
    }

    /// The physical key winit would report for `code`.
    fn physical(code: KeyCode) -> PhysicalKey {
        PhysicalKey::Code(code)
    }

    #[test]
    fn a_press_and_a_release_translate_to_the_two_edges() {
        let pressed = device_event(physical(KeyCode::KeyW), ElementState::Pressed, false)
            .expect("KeyW is in the table");
        assert_eq!(
            (pressed.control(), pressed.edge()),
            (Control::KeyW, Edge::Press)
        );

        let released = device_event(physical(KeyCode::KeyW), ElementState::Released, false)
            .expect("KeyW is in the table");
        assert_eq!(
            (released.control(), released.edge()),
            (Control::KeyW, Edge::Release)
        );
    }

    #[test]
    fn an_auto_repeat_is_not_translated() {
        // Both edges, because a repeat is reported as a press and the guard must
        // not depend on which edge it is.
        for state in [ElementState::Pressed, ElementState::Released] {
            assert_eq!(
                device_event(physical(KeyCode::KeyW), state, true),
                None,
                "a held key must not re-fire an OnPress binding"
            );
        }
    }

    #[test]
    fn a_key_outside_the_table_translates_to_nothing() {
        assert_eq!(
            device_event(physical(KeyCode::F5), ElementState::Pressed, false),
            None
        );
    }

    #[test]
    fn a_key_winit_could_not_identify_translates_to_nothing() {
        // `PhysicalKey::Unidentified` is the other variant, and it carries a
        // native code this vocabulary has no name for.
        use winit::keyboard::NativeKeyCode;

        assert_eq!(
            device_event(
                PhysicalKey::Unidentified(NativeKeyCode::Windows(0)),
                ElementState::Pressed,
                false
            ),
            None
        );
    }

    #[test]
    fn every_control_in_the_table_survives_the_whole_boundary() {
        // The table and the translation together: each entry, pressed, comes
        // out as its own control on the press edge.
        for (code, control) in TABLE {
            let event = device_event(physical(*code), ElementState::Pressed, false)
                .unwrap_or_else(|| panic!("{code:?} is in the table"));

            assert_eq!((event.control(), event.edge()), (*control, Edge::Press));
        }
    }
}

/// D23's receipt: the pointer path, driven without a window.
///
/// Every test here would have been unwritable before M6b.8 — not hard, but
/// impossible: the state was a private field of `window.rs`'s `Live` and the
/// resolution was a private method of its `Runner`, and neither can be reached
/// without an `ActiveEventLoop`. That is the coverage Weg C was chosen to buy,
/// and it is the thing V34 predicted.
#[cfg(all(test, feature = "render"))]
mod pointer_tests {
    use super::Pointer;
    use narvo_ecs::{HitRect, Layer, Transform, World};
    use narvo_render2d::Projection;

    /// A 128 x 128 world with one 32 x 32 button centred on the origin.
    ///
    /// Its action is `buy` with value `1`, which is what a resolved press has
    /// to come back with.
    fn world_with_a_button() -> World {
        let mut world = World::new();
        let button = world.spawn();
        world
            .insert(button, Transform::at(0.0, 0.0))
            .expect("a fresh entity takes a transform");
        world
            .insert(button, HitRect::new(16.0, 16.0, "buy", 1))
            .expect("a fresh entity takes a hit rectangle");
        world
    }

    /// The projection a 128 x 128 target is drawn through, camera at the origin.
    fn projection() -> Projection {
        Projection::for_target(128, 128)
    }

    #[test]
    fn a_pointer_that_has_never_moved_knows_no_position() {
        assert_eq!(Pointer::new().at(), None);
    }

    #[test]
    fn a_press_before_the_pointer_ever_moved_resolves_to_nothing() {
        // The `None` branch that existed in `window.rs` and that nothing could
        // execute: winit reports a click without a position, so a click before
        // the first `CursorMoved` has no point to test.
        let world = world_with_a_button();
        assert!(Pointer::new().press(&world, projection()).is_none());
    }

    #[test]
    fn a_move_is_remembered_verbatim() {
        let mut pointer = Pointer::new();
        pointer.moved_to(12.0, 34.0);
        assert_eq!(pointer.at(), Some((12.0, 34.0)));
    }

    #[test]
    fn the_last_move_is_the_one_that_counts() {
        let mut pointer = Pointer::new();
        pointer.moved_to(12.0, 34.0);
        pointer.moved_to(64.0, 64.0);
        assert_eq!(pointer.at(), Some((64.0, 64.0)));
    }

    #[test]
    fn a_press_on_the_button_resolves_to_its_action_and_value() {
        // The composition, end to end: a screen pixel becomes a world point,
        // the world point finds the rectangle, the rectangle names the action.
        // The centre of a 128 x 128 target is (64, 64) in physical pixels.
        let world = world_with_a_button();
        let mut pointer = Pointer::new();
        pointer.moved_to(64.0, 64.0);

        let event = pointer
            .press(&world, projection())
            .expect("the centre is over the button")
            .expect("`buy` is a name the vocabulary takes");

        assert_eq!((event.action(), event.value()), ("buy", 1));
    }

    #[test]
    fn a_press_away_from_the_button_resolves_to_nothing() {
        // The negative half, and it is what makes the test above mean
        // something: a `press` that answered everywhere would pass that one too.
        let world = world_with_a_button();
        let mut pointer = Pointer::new();
        pointer.moved_to(4.0, 4.0);

        assert!(pointer.press(&world, projection()).is_none());
    }

    #[test]
    fn the_projection_is_what_decides_where_a_pixel_lands() {
        // One pixel, two projections, two answers. This is the property that
        // made the resolution worth moving rather than merely worth extracting:
        // it says the press goes through the *renderer's* projection, and a
        // second one that drifted from it would show up here.
        let world = world_with_a_button();
        let mut pointer = Pointer::new();
        pointer.moved_to(64.0, 64.0);

        assert!(
            pointer
                .press(&world, Projection::for_target(128, 128))
                .is_some()
        );
        // On a 512 x 512 target the same pixel is far up and to the left of the
        // centre, and the button is not there.
        assert!(
            pointer
                .press(&world, Projection::for_target(512, 512))
                .is_none()
        );
    }

    #[test]
    fn a_camera_moves_what_a_press_hits() {
        // The press is answered through the camera the renderer draws with, so
        // panning the camera moves what is under a fixed pixel.
        use narvo_render2d::CameraView;

        let world = world_with_a_button();
        let mut pointer = Pointer::new();
        pointer.moved_to(64.0, 64.0);

        let panned = projection().viewed_by(CameraView::new(200.0, 0.0, 1.0));
        assert!(pointer.press(&world, panned).is_none());
    }

    #[test]
    fn the_front_most_rectangle_answers_a_press() {
        // Two buttons on the same point, and the one in front is the one that
        // answers. `hit_test` owns that rule; this pins that the pointer path
        // asks it rather than picking for itself.
        let mut world = world_with_a_button();
        let front = world.spawn();
        world
            .insert(front, Transform::at(0.0, 0.0))
            .expect("a fresh entity takes a transform");
        world
            .insert(front, HitRect::new(16.0, 16.0, "sell", 7))
            .expect("a fresh entity takes a hit rectangle");
        world
            .insert(front, Layer::at(1.0))
            .expect("a fresh entity takes a layer");

        let mut pointer = Pointer::new();
        pointer.moved_to(64.0, 64.0);

        let event = pointer
            .press(&world, projection())
            .expect("the centre is over both")
            .expect("`sell` is a name the vocabulary takes");

        assert_eq!((event.action(), event.value()), ("sell", 7));
    }

    #[test]
    fn a_rectangle_naming_an_unusable_action_reports_rather_than_resolves() {
        // The `Some(Err(_))` arm, which is a content fault: the rectangle was
        // hit, and what it names is not a legal action. It has to be
        // distinguishable from "nothing was hit", because the window prints for
        // one and stays silent for the other.
        let mut world = World::new();
        let button = world.spawn();
        world
            .insert(button, Transform::at(0.0, 0.0))
            .expect("a fresh entity takes a transform");
        world
            .insert(button, HitRect::new(16.0, 16.0, "", 1))
            .expect("a fresh entity takes a hit rectangle");

        let mut pointer = Pointer::new();
        pointer.moved_to(64.0, 64.0);

        assert!(matches!(pointer.press(&world, projection()), Some(Err(_))));
    }

    #[test]
    fn an_entity_hit_without_a_rectangle_cannot_occur_and_is_still_handled() {
        // `hit_test` only ever returns an entity that carries a `HitRect`, so
        // the `None` this branch produces is unreachable through that door.
        // It is asserted anyway, on a world with no rectangles at all, because
        // the branch exists and an untested branch is where a later change
        // hides.
        let mut world = World::new();
        let bare = world.spawn();
        world
            .insert(bare, Transform::at(0.0, 0.0))
            .expect("a fresh entity takes a transform");

        let mut pointer = Pointer::new();
        pointer.moved_to(64.0, 64.0);

        assert!(pointer.press(&world, projection()).is_none());
    }
}
