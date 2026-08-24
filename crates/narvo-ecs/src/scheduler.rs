//! The scheduler: an ordered list of named systems, run one after another.

use std::fmt;

use crate::error::EcsError;
use crate::world::World;

/// What a system is told about the run it is part of.
///
/// Deliberately small. Everything a system needs that could instead live in the
/// world *does* live in the world; this carries only what is a property of the
/// tick rather than of the simulation. Today that is the tick number. New fields
/// are added here as later milestones need them, which is why the fields are
/// private and read through accessors: adding one must not break existing
/// callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SystemContext {
    tick: u64,
}

impl SystemContext {
    /// Builds the context for tick `tick`.
    #[must_use]
    pub fn new(tick: u64) -> Self {
        Self { tick }
    }

    /// Which simulation tick is running, counted by whoever drives the loop.
    ///
    /// This is a tick count, not a clock: it says how many fixed steps have been
    /// taken, and nothing may derive a wall-clock time from it (ADR-0003).
    #[must_use]
    pub fn tick(self) -> u64 {
        self.tick
    }
}

/// The signature every system has.
///
/// A plain function pointer, not a boxed closure, and that is the whole point.
///
/// # Systems own no state
///
/// System state belongs in the world. A counter, an accumulator, a cache of last
/// tick's result - if it lives in a closure's captures it is invisible to
/// everything the engine can inspect: it is not in a state dump, not in the
/// state hash of M2.2, and not in a replay. A simulation with hidden state is
/// deterministic only by accident, because two runs that agree on every
/// component can still disagree on what a closure was holding.
///
/// A `fn` pointer cannot capture. A closure that tries to keep state does not
/// coerce to this type and the code does not compile - the rule is enforced by
/// the signature rather than by review. Non-capturing closures and plain
/// functions both coerce, so nothing legitimate is lost.
///
/// Locals inside a system are unaffected: they exist for the duration of one
/// call and are gone before the next tick can observe them.
pub type System = fn(&mut World, &SystemContext);

/// One entry of the run order.
struct RegisteredSystem {
    name: &'static str,
    run: System,
}

impl fmt::Debug for RegisteredSystem {
    /// Prints the name only. The function pointer's address is not reproducible
    /// between runs, and a `Debug` output that differs per process is the kind
    /// of thing that ends up in a test assertion by accident.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// An ordered list of named systems, run strictly in sequence.
///
/// Registration order is execution order. There is no dependency resolution, no
/// priority, no automatic parallelism and no way for a system to be skipped or
/// reordered: the order is what the caller wrote, and it is readable back out of
/// [`system_names`](Self::system_names).
///
/// That is a decision, not a gap to be filled later. An order derived from
/// declared dependencies is only reproducible if the derivation is, and a
/// scheduler that reorders systems between runs makes every determinism
/// guarantee in this engine conditional on the scheduler's own stability.
/// Parallel execution is out for the same reason at this milestone.
///
/// # Examples
///
/// ```
/// use narvo_ecs::{Scheduler, SystemContext, World};
///
/// struct Position { x: i32 }
/// struct Velocity { dx: i32 }
///
/// fn movement(world: &mut World, _context: &SystemContext) {
///     let mut moving = world.query::<(&mut Position, &Velocity)>();
///     for (_id, (position, velocity)) in moving.iter() {
///         position.x += velocity.dx;
///     }
/// }
///
/// let mut scheduler = Scheduler::new();
/// scheduler.add_system("movement", movement)?;
///
/// let mut world = World::new();
/// let entity = world.spawn();
/// world.insert(entity, Position { x: 0 })?;
/// world.insert(entity, Velocity { dx: 2 })?;
///
/// for tick in 0..3 {
///     scheduler.run(&mut world, &SystemContext::new(tick));
/// }
///
/// assert_eq!(world.get::<Position>(entity)?.x, 6);
/// # Ok::<(), narvo_ecs::EcsError>(())
/// ```
#[derive(Debug, Default)]
pub struct Scheduler {
    systems: Vec<RegisteredSystem>,
}

impl Scheduler {
    /// Creates a scheduler with no systems.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `system` to the run order under `name`.
    ///
    /// The name is what the system is called in logs, diagnostics and
    /// [`system_names`](Self::system_names). It has no effect on ordering.
    ///
    /// # Errors
    ///
    /// [`EcsError::DuplicateSystem`] if the name is already in use. Two systems
    /// answering to one name make every message that names a system ambiguous,
    /// including the one reporting which system failed.
    pub fn add_system(&mut self, name: &'static str, system: System) -> Result<(), EcsError> {
        if self.systems.iter().any(|existing| existing.name == name) {
            return Err(EcsError::DuplicateSystem { name });
        }

        self.systems.push(RegisteredSystem { name, run: system });
        Ok(())
    }

    /// Runs every system once, in registration order.
    ///
    /// Takes `&self`: the run order cannot change during a run, which is what
    /// makes "registration order is execution order" checkable by reading the
    /// registration site alone.
    pub fn run(&self, world: &mut World, context: &SystemContext) {
        for system in &self.systems {
            (system.run)(world, context);
        }
    }

    /// The names of the registered systems, in execution order.
    pub fn system_names(&self) -> impl ExactSizeIterator<Item = &'static str> {
        self.systems.iter().map(|system| system.name)
    }

    /// How many systems are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// Whether no system is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Scheduler, SystemContext};
    use crate::{EcsError, World};

    /// The systems below record what ran into this component, because they have
    /// nowhere else to put it: a `fn` pointer cannot capture a log. That is the
    /// architecture rule under test as much as the run order is.
    #[derive(Debug, Default)]
    struct RunLog {
        entries: Vec<String>,
    }

    fn log(world: &mut World, entry: String) {
        let mut logs = world.query::<&mut RunLog>();
        for (_id, log) in logs.iter() {
            log.entries.push(entry.clone());
        }
    }

    fn first(world: &mut World, _context: &SystemContext) {
        log(world, "first".to_owned());
    }

    fn second(world: &mut World, _context: &SystemContext) {
        log(world, "second".to_owned());
    }

    fn third(world: &mut World, context: &SystemContext) {
        log(world, format!("third@{}", context.tick()));
    }

    /// Builds a world holding one [`RunLog`], and returns it with the entity.
    fn world_with_log() -> (World, crate::EntityId) {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, RunLog::default())
            .expect("the entity is alive");
        (world, entity)
    }

    #[test]
    fn a_fresh_scheduler_runs_nothing() {
        let scheduler = Scheduler::new();
        let (mut world, entity) = world_with_log();

        assert!(scheduler.is_empty());
        assert_eq!(scheduler.len(), 0);
        assert_eq!(scheduler.system_names().count(), 0);

        scheduler.run(&mut world, &SystemContext::new(0));
        assert!(
            world
                .get::<RunLog>(entity)
                .expect("alive")
                .entries
                .is_empty()
        );
    }

    #[test]
    fn systems_run_in_registration_order() {
        let mut scheduler = Scheduler::new();
        // Registered deliberately out of alphabetical order, so a scheduler that
        // sorted by name would produce a different sequence and fail here.
        scheduler.add_system("third", third).expect("a fresh name");
        scheduler.add_system("first", first).expect("a fresh name");
        scheduler
            .add_system("second", second)
            .expect("a fresh name");

        assert_eq!(
            scheduler.system_names().collect::<Vec<_>>(),
            vec!["third", "first", "second"]
        );
        assert_eq!(scheduler.len(), 3);

        let (mut world, entity) = world_with_log();
        scheduler.run(&mut world, &SystemContext::new(7));

        assert_eq!(
            world.get::<RunLog>(entity).expect("alive").entries,
            vec!["third@7", "first", "second"]
        );
    }

    #[test]
    fn the_order_repeats_identically_on_every_run() {
        let mut scheduler = Scheduler::new();
        scheduler.add_system("first", first).expect("a fresh name");
        scheduler
            .add_system("second", second)
            .expect("a fresh name");

        let (mut world, entity) = world_with_log();
        for tick in 0..3 {
            scheduler.run(&mut world, &SystemContext::new(tick));
        }

        assert_eq!(
            world.get::<RunLog>(entity).expect("alive").entries,
            vec!["first", "second", "first", "second", "first", "second"]
        );
    }

    #[test]
    fn the_tick_number_reaches_the_systems() {
        let mut scheduler = Scheduler::new();
        scheduler.add_system("third", third).expect("a fresh name");

        let (mut world, entity) = world_with_log();
        for tick in [0, 1, 41] {
            scheduler.run(&mut world, &SystemContext::new(tick));
        }

        assert_eq!(
            world.get::<RunLog>(entity).expect("alive").entries,
            vec!["third@0", "third@1", "third@41"]
        );
    }

    #[test]
    fn two_systems_cannot_share_one_name() {
        let mut scheduler = Scheduler::new();
        scheduler
            .add_system("movement", first)
            .expect("a fresh name");

        match scheduler.add_system("movement", second) {
            Err(EcsError::DuplicateSystem { name }) => assert_eq!(name, "movement"),
            other => panic!("expected a duplicate system error, got {other:?}"),
        }

        // The rejected registration is not in the run order.
        assert_eq!(scheduler.len(), 1);

        let (mut world, entity) = world_with_log();
        scheduler.run(&mut world, &SystemContext::new(0));
        assert_eq!(
            world.get::<RunLog>(entity).expect("alive").entries,
            vec!["first"]
        );
    }

    #[test]
    fn a_context_reports_the_tick_it_was_built_with() {
        let context = SystemContext::new(12);

        assert_eq!(context.tick(), 12);
        assert_eq!(context, SystemContext::new(12));
        assert_ne!(context, SystemContext::new(13));
    }
}
