//! Events between systems: one buffer, one tick of latency, one documented
//! order.
//!
//! Read ADR-0011 before changing anything here. The semantics below are a
//! decision, not an implementation accident, and the half-way version of them —
//! "usually the same tick, sometimes the next one" — is the origin of the kind
//! of nondeterminism that cannot be found afterwards.

use std::fmt;
use std::mem;

use serde::{Deserialize, Serialize};

use crate::scheduler::SystemContext;
use crate::world::{Component, World};

/// A buffer of events of one type, stored on an entity like any other component.
///
/// # Semantics: an event sent during a tick is readable during the next one
///
/// [`send`](Self::send) puts an event where nothing can read it yet.
/// [`rotate`](Self::rotate), run once per tick by
/// [`rotate_events`], makes the previous tick's sends readable and drops
/// what was readable before. So an event sent in tick *N* is visible to every
/// system in tick *N+1*, to none in tick *N*, and to none in tick *N+2*.
///
/// The alternative — delivering within the same tick, to systems scheduled after
/// the sender — was rejected. Under it, whether a system sees an event depends
/// on where it sits in the run order relative to the sender, so moving a system
/// for an unrelated reason silently changes behaviour: no error, no failed
/// assertion, just an event that now arrives a tick later or not at all. Under
/// the rule above, every system in a tick sees exactly the same set of events
/// regardless of its position, and that is a property that can be stated in one
/// sentence and tested in one test.
///
/// The cost is one tick of latency, and it is paid knowingly. At 60 Hz that is
/// under 17 ms, and a reaction that genuinely cannot wait a tick belongs in the
/// system that caused it rather than in a message.
///
/// # Order
///
/// [`iter`](Self::iter) yields events in the order they were sent. The buffer is
/// a `Vec` and nothing sorts, groups or deduplicates it; no hash map is
/// consulted anywhere on the path from `send` to `iter`.
///
/// Between *different* buffers there is no order and none is implied: two event
/// types are two components and two independent sequences.
///
/// # Lifecycle
///
/// An event is dropped by the second [`rotate`](Self::rotate) after the one that
/// made it readable — that is, exactly one tick of visibility, then gone. It is
/// never delivered twice, and a system that does not look during that tick does
/// not get another chance. Nothing accumulates: a buffer nobody reads still
/// empties itself.
///
/// # This buffer is part of the state hash
///
/// Deliberately. A pending event is state that decides what happens next tick,
/// so two runs holding different pending events have already diverged even
/// though every other component still agrees. Keeping the buffer out of the dump
/// would hide exactly that moment — the one the instrument exists to catch. It
/// also makes a forgotten [`rotate_events`] loud rather than silent: the
/// unrotated buffer grows, and the growth is in the dump.
///
/// # Examples
///
/// ```
/// use narvo_ecs::{Events, Scheduler, SystemContext, World, rotate_events};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
/// struct Damage(i32);
///
/// let mut world = World::new();
/// let bus = world.spawn();
/// world.insert(bus, Events::<Damage>::new())?;
///
/// // Rotation is an ordinary system, registered first so that what was sent
/// // last tick becomes readable before anything looks.
/// let mut scheduler = Scheduler::new();
/// scheduler.add_system("damage/rotate", rotate_events::<Damage>)?;
///
/// world.get_mut::<Events<Damage>>(bus)?.send(Damage(3));
/// // Not readable in the tick it was sent in.
/// assert_eq!(world.get::<Events<Damage>>(bus)?.len(), 0);
///
/// scheduler.run(&mut world, &SystemContext::new(1));
/// assert_eq!(
///     world.get::<Events<Damage>>(bus)?.iter().copied().collect::<Vec<_>>(),
///     vec![Damage(3)]
/// );
///
/// // ... and gone the tick after that.
/// scheduler.run(&mut world, &SystemContext::new(2));
/// assert_eq!(world.get::<Events<Damage>>(bus)?.len(), 0);
/// # Ok::<(), narvo_ecs::EcsError>(())
/// ```
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Events<E> {
    /// Sent during the current tick. Not readable until the next rotation.
    pending: Vec<E>,
    /// Sent during the previous tick. What [`iter`](Self::iter) yields.
    readable: Vec<E>,
}

impl<E> Events<E> {
    /// Creates an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            readable: Vec::new(),
        }
    }

    /// Queues `event` for delivery in the next tick.
    ///
    /// It is not readable from this buffer until [`rotate`](Self::rotate) runs,
    /// including to the sender itself.
    pub fn send(&mut self, event: E) {
        self.pending.push(event);
    }

    /// The events readable this tick, in the order they were sent.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &E> {
        self.readable.iter()
    }

    /// How many events are readable this tick.
    #[must_use]
    pub fn len(&self) -> usize {
        self.readable.len()
    }

    /// Whether nothing is readable this tick.
    ///
    /// Says nothing about what has been sent during this tick — that becomes
    /// readable at the next rotation and is reported by
    /// [`pending_len`](Self::pending_len).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.readable.is_empty()
    }

    /// How many events have been sent this tick and are not readable yet.
    ///
    /// Diagnostics and tests. A system deciding what to do based on this would
    /// be reading the future, which is the thing the one-tick delay exists to
    /// prevent.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Advances the buffer by one tick: what was sent becomes readable, and what
    /// was readable is dropped.
    ///
    /// Called once per tick per buffer by [`rotate_events`]. Calling it twice in
    /// one tick discards a tick's worth of events without delivering them, which
    /// is why it is a system rather than something a reader does for itself.
    pub fn rotate(&mut self) {
        // Clear first, then swap: the events that were readable are dropped
        // here, and their allocation is handed back to `pending` instead of
        // being freed and reallocated every tick.
        self.readable.clear();
        mem::swap(&mut self.readable, &mut self.pending);
    }
}

impl<E> Default for Events<E> {
    /// The same as [`Events::new`].
    ///
    /// Hand-written rather than derived: a derived `Default` would demand
    /// `E: Default`, and an event type has no reason to have a default value.
    fn default() -> Self {
        Self::new()
    }
}

impl<E: fmt::Debug> fmt::Debug for Events<E> {
    /// Shows both halves, labelled, because which half an event is in is the
    /// thing worth knowing about a buffer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Events")
            .field("readable", &self.readable)
            .field("pending", &self.pending)
            .finish()
    }
}

/// The system that advances every [`Events<E>`] in the world by one tick.
///
/// Register it once per event type, and register it *first*, so that what was
/// sent last tick is readable before anything in this tick looks:
///
/// ```
/// # use narvo_ecs::{Events, Scheduler, rotate_events};
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Serialize, Deserialize)]
/// # struct Damage(i32);
/// let mut scheduler = Scheduler::new();
/// scheduler.add_system("damage/rotate", rotate_events::<Damage>)?;
/// # Ok::<(), narvo_ecs::EcsError>(())
/// ```
///
/// It is a plain system rather than something the scheduler does on its own
/// because the scheduler knows nothing about event types, and because a
/// rotation that happens invisibly is a tick boundary nobody can see in the run
/// order. Here it appears in
/// [`Scheduler::system_names`](crate::Scheduler::system_names) like everything
/// else.
///
/// If several buffers of the same type exist, all of them are rotated. The order
/// in which they are visited is query order and therefore not stable, and that
/// is harmless here and only here: rotating a buffer touches nothing outside it,
/// so any order produces the same result. Nothing else in this crate may lean on
/// that.
pub fn rotate_events<E: Component>(world: &mut World, _context: &SystemContext) {
    let mut buffers = world.query::<&mut Events<E>>();

    for (_entity, events) in buffers.iter() {
        events.rotate();
    }
}

#[cfg(test)]
mod tests {
    use super::{Events, rotate_events};
    use crate::{Scheduler, SystemContext, World};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    struct Ping(i32);

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    struct Pong(i32);

    /// A world holding one buffer, and a scheduler that rotates it first.
    fn bus() -> (World, crate::EntityId, Scheduler) {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Events::<Ping>::new())
            .expect("the entity is alive");

        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("ping/rotate", rotate_events::<Ping>)
            .expect("a fresh name");

        (world, entity, scheduler)
    }

    fn readable(world: &World, entity: crate::EntityId) -> Vec<Ping> {
        world
            .get::<Events<Ping>>(entity)
            .expect("alive")
            .iter()
            .copied()
            .collect()
    }

    #[test]
    fn a_fresh_buffer_holds_nothing() {
        let events = Events::<Ping>::new();

        assert!(events.is_empty());
        assert_eq!(events.len(), 0);
        assert_eq!(events.pending_len(), 0);
        assert_eq!(events.iter().count(), 0);
        assert_eq!(events, Events::default());
    }

    #[test]
    fn an_event_is_not_readable_in_the_tick_it_was_sent_in() {
        let (mut world, entity, _scheduler) = bus();

        world
            .get_mut::<Events<Ping>>(entity)
            .expect("alive")
            .send(Ping(1));

        assert_eq!(readable(&world, entity), Vec::new());
        assert_eq!(
            world
                .get::<Events<Ping>>(entity)
                .expect("alive")
                .pending_len(),
            1
        );
    }

    #[test]
    fn an_event_is_received_exactly_once() {
        // The requirement in one test: not zero times, not twice. The count is
        // taken every tick for four ticks, so a redelivery in any later tick
        // shows up as well as one in the tick after the send.
        let (mut world, entity, scheduler) = bus();

        world
            .get_mut::<Events<Ping>>(entity)
            .expect("alive")
            .send(Ping(7));

        let mut received = Vec::new();
        for tick in 0..4 {
            scheduler.run(&mut world, &SystemContext::new(tick));
            received.extend(readable(&world, entity));
        }

        assert_eq!(
            received,
            vec![Ping(7)],
            "delivered {} times",
            received.len()
        );
    }

    #[test]
    fn events_are_readable_in_the_order_they_were_sent() {
        let (mut world, entity, scheduler) = bus();

        {
            let mut events = world.get_mut::<Events<Ping>>(entity).expect("alive");
            // Deliberately not ascending: a buffer that sorted would pass an
            // ascending sequence by accident.
            for value in [3, 1, 2, 1] {
                events.send(Ping(value));
            }
        }

        scheduler.run(&mut world, &SystemContext::new(0));

        assert_eq!(
            readable(&world, entity),
            vec![Ping(3), Ping(1), Ping(2), Ping(1)]
        );
    }

    #[test]
    fn an_event_read_in_one_tick_is_gone_in_the_next() {
        let (mut world, entity, scheduler) = bus();

        world
            .get_mut::<Events<Ping>>(entity)
            .expect("alive")
            .send(Ping(1));

        scheduler.run(&mut world, &SystemContext::new(0));
        assert_eq!(readable(&world, entity), vec![Ping(1)]);

        scheduler.run(&mut world, &SystemContext::new(1));
        assert_eq!(readable(&world, entity), Vec::new());
    }

    #[test]
    fn a_buffer_nobody_reads_still_empties_itself() {
        // The lifecycle rule stated as the property that matters: nothing
        // accumulates, so a buffer cannot quietly grow into the state hash.
        let (mut world, entity, scheduler) = bus();

        for tick in 0..32 {
            world
                .get_mut::<Events<Ping>>(entity)
                .expect("alive")
                .send(Ping(tick as i32));
            scheduler.run(&mut world, &SystemContext::new(tick));
        }

        let events = world.get::<Events<Ping>>(entity).expect("alive");
        assert_eq!(events.len(), 1, "at most one tick of events is ever held");
        assert_eq!(events.pending_len(), 0);
    }

    #[test]
    fn rotation_delivers_across_ticks_without_reordering() {
        let (mut world, entity, scheduler) = bus();

        let mut received = Vec::new();
        for tick in 0..4 {
            scheduler.run(&mut world, &SystemContext::new(tick));
            received.extend(readable(&world, entity));

            let mut events = world.get_mut::<Events<Ping>>(entity).expect("alive");
            events.send(Ping(tick as i32 * 2));
            events.send(Ping(tick as i32 * 2 + 1));
        }
        scheduler.run(&mut world, &SystemContext::new(4));
        received.extend(readable(&world, entity));

        assert_eq!(
            received,
            (0..8).map(Ping).collect::<Vec<_>>(),
            "every event has to arrive once, in send order"
        );
    }

    #[test]
    fn two_event_types_are_two_independent_buffers() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Events::<Ping>::new()).expect("alive");
        world.insert(entity, Events::<Pong>::new()).expect("alive");

        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("ping/rotate", rotate_events::<Ping>)
            .expect("a fresh name");

        world
            .get_mut::<Events<Ping>>(entity)
            .expect("alive")
            .send(Ping(1));
        world
            .get_mut::<Events<Pong>>(entity)
            .expect("alive")
            .send(Pong(2));

        scheduler.run(&mut world, &SystemContext::new(0));

        // Only the type whose rotation was registered advanced.
        assert_eq!(world.get::<Events<Ping>>(entity).expect("alive").len(), 1);
        assert_eq!(world.get::<Events<Pong>>(entity).expect("alive").len(), 0);
        assert_eq!(
            world
                .get::<Events<Pong>>(entity)
                .expect("alive")
                .pending_len(),
            1,
            "an unrotated buffer holds on to what was sent, visibly"
        );
    }

    #[test]
    fn every_buffer_of_the_type_is_rotated() {
        let mut world = World::new();
        let first = world.spawn();
        world.insert(first, Events::<Ping>::new()).expect("alive");
        let second = world.spawn();
        world.insert(second, Events::<Ping>::new()).expect("alive");

        world
            .get_mut::<Events<Ping>>(first)
            .expect("alive")
            .send(Ping(1));
        world
            .get_mut::<Events<Ping>>(second)
            .expect("alive")
            .send(Ping(2));

        rotate_events::<Ping>(&mut world, &SystemContext::new(0));

        assert_eq!(readable(&world, first), vec![Ping(1)]);
        assert_eq!(readable(&world, second), vec![Ping(2)]);
    }

    #[test]
    fn rotating_a_world_without_any_buffer_does_nothing() {
        let mut world = World::new();
        world.spawn();

        rotate_events::<Ping>(&mut world, &SystemContext::new(0));

        assert_eq!(world.len(), 1);
    }

    #[test]
    fn a_buffer_serializes_with_both_halves_visible() {
        // What a buffer looks like in a state dump is observable surface: it is
        // what a `diff` between two diverging runs shows.
        let mut events = Events::<Ping>::new();
        events.send(Ping(4));

        let text = ron::to_string(&events).expect("a buffer of serializable events serializes");
        assert_eq!(text, "(pending:[(4)],readable:[])");

        events.rotate();
        let text = ron::to_string(&events).expect("a buffer of serializable events serializes");
        assert_eq!(text, "(pending:[],readable:[(4)])");

        let restored: Events<Ping> = ron::from_str(&text).expect("what was written reads back");
        assert_eq!(restored, events);
    }
}
