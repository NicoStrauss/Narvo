//! The world facade: entities, their components, and access to both.

use std::any::{TypeId, type_name};
use std::collections::BTreeMap;
use std::fmt;
use std::ops::{Deref, DerefMut};

use crate::entity::EntityId;
use crate::error::EcsError;
use crate::query::{Query, QueryBorrow};

/// Anything that can be stored on an entity.
///
/// Implemented automatically for every `Send + Sync + 'static` type and never
/// implemented by hand. `Send + Sync` because a world is meant to be movable
/// between threads even while system execution stays sequential; `'static`
/// because storage is type-erased and a borrowed component could outlive what it
/// borrows from.
///
/// Note what is *not* required: no derive, no registration, no schema. A plain
/// struct is a component. Registration
/// ([`ComponentRegistry`](crate::ComponentRegistry)) is a separate, additional
/// step, and it is only needed for components that have to appear in serialized
/// state.
pub trait Component: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> Component for T {}

/// A shared reference to one component, and the borrow that keeps it valid.
///
/// Behaves like `&T` through [`Deref`]. It is a distinct type rather than a bare
/// reference because the underlying storage tracks borrows dynamically and has
/// to be told when the borrow ends.
pub struct Ref<'a, T: Component> {
    inner: hecs::Ref<'a, T>,
}

impl<T: Component> Deref for Ref<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: Component + fmt::Debug> fmt::Debug for Ref<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// A unique reference to one component, and the borrow that keeps it valid.
///
/// The counterpart of [`Ref`], behaving like `&mut T` through [`DerefMut`].
pub struct RefMut<'a, T: Component> {
    inner: hecs::RefMut<'a, T>,
}

impl<T: Component> Deref for RefMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: Component> DerefMut for RefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: Component + fmt::Debug> fmt::Debug for RefMut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

/// The collection of entities and their components.
///
/// This is the engine's own API over the storage engine chosen in ADR-0002.
/// Systems, tools and tests are written against this type; the storage engine
/// itself does not appear in any signature here. The one deliberate exception is
/// the [`Query`] trait, and ADR-0005 records why.
///
/// # Determinism
///
/// A world contains no clock, no randomness and no interior iteration order that
/// depends on addresses or hashing. Given the same sequence of calls it reaches
/// the same state, which is what makes a headless run reproducible.
///
/// The one thing that is *not* stable is query iteration order - see [`Query`].
/// [`entity_ids`](Self::entity_ids) is the canonical enumeration, and everything
/// that gets logged, serialized or compared goes through it.
///
/// # Examples
///
/// ```
/// use narvo_ecs::World;
///
/// struct Position { x: i32 }
/// struct Velocity { dx: i32 }
///
/// let mut world = World::new();
///
/// let entity = world.spawn();
/// world.insert(entity, Position { x: 0 })?;
/// world.insert(entity, Velocity { dx: 3 })?;
///
/// // The query guard needs its own binding: it holds the borrow that the
/// // references handed out below live in.
/// let mut moving = world.query::<(&mut Position, &Velocity)>();
/// for (_id, (position, velocity)) in moving.iter() {
///     position.x += velocity.dx;
/// }
/// // Dropping the guard releases the write borrow; reading `Position` again
/// // while it is still held would panic.
/// drop(moving);
///
/// assert_eq!(world.get::<Position>(entity)?.x, 3);
/// # Ok::<(), narvo_ecs::EcsError>(())
/// ```
pub struct World {
    inner: hecs::World,
    /// Rust type name per component type ever inserted here.
    ///
    /// The storage engine keeps components under their `TypeId` and has no way
    /// back to a name, so without this a world can say *that* an entity carries
    /// an unregistered component but not *which* — and "some component of some
    /// type is missing from the state hash" is not a message anybody can act on.
    /// [`insert`](Self::insert) is the only route by which a component reaches
    /// an entity, so recording the name there covers every type that can ever be
    /// present.
    ///
    /// A `BTreeMap` rather than a hash map for the reason everything else in
    /// this crate is: nothing here may depend on hash iteration order. It is
    /// only ever looked up, never iterated, so the choice costs nothing.
    component_type_names: BTreeMap<TypeId, &'static str>,
}

impl World {
    /// Creates an empty world.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: hecs::World::new(),
            component_type_names: BTreeMap::new(),
        }
    }

    /// Creates an entity with no components and returns its handle.
    ///
    /// Components are added afterwards with [`insert`](Self::insert). There is
    /// no bundle form: a bundle is a storage-engine concept, and accepting one
    /// would put a foreign type into this signature for the sake of saving a
    /// line.
    pub fn spawn(&mut self) -> EntityId {
        EntityId::from_hecs(self.inner.spawn(()))
    }

    /// Removes an entity and everything it carries.
    ///
    /// The slot becomes available for reuse, so handles to this entity are
    /// invalid from here on - permanently, including after the slot is handed to
    /// a new entity, which receives a higher generation.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if the entity is not alive.
    pub fn despawn(&mut self, entity: EntityId) -> Result<(), EcsError> {
        self.inner
            .despawn(entity.to_hecs())
            .map_err(|_| EcsError::NoSuchEntity { entity })
    }

    /// Whether `entity` is alive in this world.
    #[must_use]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.inner.contains(entity.to_hecs())
    }

    /// Whether `entity` is alive and carries a `T`.
    #[must_use]
    pub fn has<T: Component>(&self, entity: EntityId) -> bool {
        self.inner
            .entity(entity.to_hecs())
            .is_ok_and(|found| found.has::<T>())
    }

    /// Adds `component` to `entity`, replacing any `T` it already had.
    ///
    /// Replacing rather than failing is deliberate: "set this component" is the
    /// common operation, and a caller that needs to know whether one was there
    /// can ask [`has`](Self::has) first.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if the entity is not alive.
    pub fn insert<T: Component>(&mut self, entity: EntityId, component: T) -> Result<(), EcsError> {
        self.inner
            .insert_one(entity.to_hecs(), component)
            .map_err(|_| EcsError::NoSuchEntity { entity })?;

        // Recorded after the insert succeeded, so a failed insert leaves no
        // trace. One map probe per insert is the price of being able to name a
        // component type later; the alternative is a state hash that reports
        // an opaque `TypeId` when it finds something it cannot serialize.
        self.component_type_names
            .entry(TypeId::of::<T>())
            .or_insert_with(type_name::<T>);

        Ok(())
    }

    /// Removes the `T` from `entity` and hands it back.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if the entity is not alive,
    /// [`EcsError::MissingComponent`] if it does not carry a `T`.
    pub fn remove<T: Component>(&mut self, entity: EntityId) -> Result<T, EcsError> {
        self.inner
            .remove_one::<T>(entity.to_hecs())
            .map_err(|error| component_error::<T>(entity, error))
    }

    /// Reads the `T` of `entity`.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if the entity is not alive,
    /// [`EcsError::MissingComponent`] if it does not carry a `T`.
    pub fn get<T: Component>(&self, entity: EntityId) -> Result<Ref<'_, T>, EcsError> {
        self.inner
            .get::<&T>(entity.to_hecs())
            .map(|inner| Ref { inner })
            .map_err(|error| component_error::<T>(entity, error))
    }

    /// Reads the `T` of `entity` for modification.
    ///
    /// Takes `&mut self` although the storage engine would accept a shared
    /// borrow: mutation through a `&World` would leave "the render path only
    /// reads simulation state" as a convention nobody can check, and here the
    /// compiler can check it for free.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if the entity is not alive,
    /// [`EcsError::MissingComponent`] if it does not carry a `T`.
    pub fn get_mut<T: Component>(&mut self, entity: EntityId) -> Result<RefMut<'_, T>, EcsError> {
        self.inner
            .get::<&mut T>(entity.to_hecs())
            .map(|inner| RefMut { inner })
            .map_err(|error| component_error::<T>(entity, error))
    }

    /// Starts a query for every entity carrying the components `Q` names.
    ///
    /// Returns a guard; call [`iter`](QueryBorrow::iter) on it to walk the
    /// results. The two steps exist because the borrow has to outlive the
    /// iterator - [`QueryBorrow`] explains it in full.
    ///
    /// Queries that write (`&mut T` in `Q`) are allowed through a shared
    /// `&World`, unlike [`get_mut`](Self::get_mut), because the storage engine
    /// checks those borrows dynamically and separating the two cases statically
    /// would need a second seam into it. The dynamic check is real: conflicting
    /// borrows panic rather than alias.
    ///
    /// The result order is archetype order and is not stable. See [`Query`].
    ///
    /// # Examples
    ///
    /// ```
    /// use narvo_ecs::World;
    ///
    /// struct Position { x: i32 }
    /// struct Frozen;
    ///
    /// let mut world = World::new();
    /// let moving = world.spawn();
    /// world.insert(moving, Position { x: 1 })?;
    /// let stuck = world.spawn();
    /// world.insert(stuck, Position { x: 5 })?;
    /// world.insert(stuck, Frozen)?;
    ///
    /// // A query matches on the components it names and ignores the rest, so
    /// // the frozen entity is included here.
    /// let mut positions = world.query::<&Position>();
    /// let total: i32 = positions.iter().map(|(_id, position)| position.x).sum();
    /// assert_eq!(total, 6);
    /// # Ok::<(), narvo_ecs::EcsError>(())
    /// ```
    pub fn query<Q: Query>(&self) -> QueryBorrow<'_, Q> {
        QueryBorrow::new(&self.inner)
    }

    /// Every live entity, sorted.
    ///
    /// This is the canonical enumeration of a world: the order is [`EntityId`]
    /// order - by slot index first - and is reproducible across runs and
    /// machines, which query iteration is not. Serialized state, determinism
    /// comparisons and the state hash of M2.2 all start here.
    ///
    /// It collects into a `Vec` rather than returning an iterator because
    /// sorting needs the whole set anyway. That allocation is the price of the
    /// guarantee, and this is not a per-frame operation.
    #[must_use]
    pub fn entity_ids(&self) -> Vec<EntityId> {
        let mut ids: Vec<EntityId> = self
            .inner
            .iter()
            .map(|found| EntityId::from_hecs(found.entity()))
            .collect();

        ids.sort_unstable();
        ids
    }

    /// Every component type `entity` currently carries.
    ///
    /// Used by the canonical dump to notice a component the registry does not
    /// know about. `pub(crate)`, because it is a question about storage rather
    /// than about the simulation, and answering it publicly would invite code
    /// that branches on component types at runtime.
    ///
    /// Collects rather than returning an iterator: the borrow would otherwise
    /// pin the world for the whole walk, and this is not a hot path.
    pub(crate) fn component_type_ids(&self, entity: EntityId) -> Result<Vec<TypeId>, EcsError> {
        Ok(self
            .inner
            .entity(entity.to_hecs())
            .map_err(|_| EcsError::NoSuchEntity { entity })?
            .component_types()
            .collect())
    }

    /// The Rust type name recorded for a component type, if one ever reached
    /// this world.
    pub(crate) fn component_type_name(&self, type_id: TypeId) -> Option<&'static str> {
        self.component_type_names.get(&type_id).copied()
    }

    /// The slots waiting to be handed out again, with the generation each will
    /// carry when it is.
    ///
    /// A despawn frees a slot and raises its generation; the next
    /// [`spawn`](Self::spawn) takes one of those slots back before it reaches for
    /// a fresh one. This is that list, and it is the half of a world's identity
    /// that [`entity_ids`](Self::entity_ids) cannot show — a world's *future*
    /// handles rather than its present ones.
    ///
    /// **The order is the order it will be consumed in reverse: the last entry
    /// is the slot the next spawn takes.** That is the storage engine's own
    /// order and it is passed through rather than reversed, because reversing it
    /// here and again on the way back in is two places to get it wrong.
    /// `the_freelist_is_what_the_next_spawns_take` pins it.
    ///
    /// # What this is for
    ///
    /// Writing a world down and building it back. Two worlds with the same live
    /// entities and the same free list hand out identical handles from there on,
    /// and two worlds with the same live entities and *different* free lists do
    /// not — while their canonical dumps agree, because a dump prints only live
    /// entities. A saved state that leaves this out therefore round-trips
    /// perfectly and diverges on its next spawn.
    #[must_use]
    pub fn freelist(&self) -> Vec<EntityId> {
        self.inner.freelist().map(EntityId::from_hecs).collect()
    }

    /// Builds a world holding exactly `live`, with exactly `free` waiting.
    ///
    /// The entities arrive with no components; put them back with
    /// [`insert`](Self::insert) or through the registry's type-erased path, which
    /// is what a loader does.
    ///
    /// # Why this is an associated function
    ///
    /// Deliberately, and it is the load-bearing half of the signature. Loading a
    /// state that turns out to be broken must not be able to damage the state
    /// that is running, and the cheapest way to guarantee that is for the loader
    /// never to be handed the running world at all. A `&mut self` form could
    /// clear a world and then fail half way; this one has nothing to clear.
    ///
    /// # The table has to describe a world
    ///
    /// `live` and `free` partition the slots `0..=highest`, each named exactly
    /// once. That is not a convention imposed here — it is what spawning and
    /// despawning produce, so a table breaking it is one no world could have
    /// written. Checking it is also what keeps a hand-edited file away from the
    /// storage engine's own `debug_assert` on a free list that addresses a live
    /// entity, which would be a panic reached from file input.
    ///
    /// # Errors
    ///
    /// [`EcsError::SlotNamedTwice`], [`EcsError::SlotLiveAndFree`] or
    /// [`EcsError::SlotMissing`], each naming the slot.
    ///
    /// # Examples
    ///
    /// ```
    /// use narvo_ecs::World;
    ///
    /// let mut world = World::new();
    /// let ids: Vec<_> = (0..3).map(|_| world.spawn()).collect();
    /// world.despawn(ids[1])?;
    ///
    /// let rebuilt = World::reconstitute(&world.entity_ids(), &world.freelist())?;
    ///
    /// assert_eq!(rebuilt.entity_ids(), world.entity_ids());
    /// assert_eq!(rebuilt.freelist(), world.freelist());
    /// # Ok::<(), narvo_ecs::EcsError>(())
    /// ```
    pub fn reconstitute(live: &[EntityId], free: &[EntityId]) -> Result<Self, EcsError> {
        let mut seen: BTreeMap<u32, bool> = BTreeMap::new();

        for entity in live {
            if seen.insert(entity.index(), true).is_some() {
                return Err(EcsError::SlotNamedTwice {
                    index: entity.index(),
                });
            }
        }
        for entity in free {
            match seen.insert(entity.index(), false) {
                None => {}
                Some(true) => {
                    return Err(EcsError::SlotLiveAndFree {
                        index: entity.index(),
                    });
                }
                Some(false) => {
                    return Err(EcsError::SlotNamedTwice {
                        index: entity.index(),
                    });
                }
            }
        }

        // `seen` is a `BTreeMap`, so its last key is the highest slot named.
        if let Some((&highest, _)) = seen.iter().next_back() {
            for index in 0..=highest {
                if !seen.contains_key(&index) {
                    return Err(EcsError::SlotMissing { index, highest });
                }
            }
        }

        let mut inner = hecs::World::new();
        for entity in live {
            // The empty bundle is what makes this "an entity exists here" rather
            // than "an entity with these components exists here": components go
            // back on afterwards, through the same path a scene load uses, so
            // there is one route by which a component reaches an entity.
            inner.spawn_at(entity.to_hecs(), ());
        }

        let freelist: Vec<hecs::Entity> = free.iter().map(|entity| entity.to_hecs()).collect();
        inner.set_freelist(&freelist);

        Ok(Self {
            inner,
            component_type_names: BTreeMap::new(),
        })
    }

    /// How many entities are alive.
    #[must_use]
    pub fn len(&self) -> u32 {
        self.inner.len()
    }

    /// Whether no entity is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for World {
    /// Reports the entity count rather than the contents.
    ///
    /// Dumping components would need every one of them to be `Debug`, which
    /// components are not required to be. The canonical, complete dump is built
    /// from [`World::entity_ids`] and the
    /// [`ComponentRegistry`](crate::ComponentRegistry).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("World")
            .field("entities", &self.inner.len())
            .finish_non_exhaustive()
    }
}

/// Translates a hecs component access failure, naming the entity and the
/// component type that hecs itself cannot name at the point of failure.
fn component_error<T: Component>(entity: EntityId, error: hecs::ComponentError) -> EcsError {
    match error {
        hecs::ComponentError::NoSuchEntity => EcsError::NoSuchEntity { entity },
        hecs::ComponentError::MissingComponent(_) => EcsError::MissingComponent {
            entity,
            component: type_name::<T>(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::World;
    use crate::EcsError;

    #[derive(Debug, PartialEq)]
    struct Position {
        x: i32,
        y: i32,
    }

    #[derive(Debug, PartialEq)]
    struct Velocity {
        dx: i32,
        dy: i32,
    }

    #[derive(Debug, PartialEq)]
    struct Health(i32);

    #[test]
    fn a_fresh_world_is_empty() {
        let world = World::new();

        assert!(world.is_empty());
        assert_eq!(world.len(), 0);
        assert!(world.entity_ids().is_empty());
    }

    #[test]
    fn components_can_be_inserted_read_written_and_removed() {
        let mut world = World::new();
        let entity = world.spawn();

        assert!(world.contains(entity));
        assert!(!world.has::<Position>(entity));

        world
            .insert(entity, Position { x: 1, y: 2 })
            .expect("the entity is alive");
        assert!(world.has::<Position>(entity));
        assert_eq!(
            *world.get::<Position>(entity).expect("just inserted"),
            Position { x: 1, y: 2 }
        );

        world.get_mut::<Position>(entity).expect("just inserted").x = 7;
        assert_eq!(world.get::<Position>(entity).expect("just inserted").x, 7);

        let removed = world.remove::<Position>(entity).expect("just inserted");
        assert_eq!(removed, Position { x: 7, y: 2 });
        assert!(!world.has::<Position>(entity));

        // The entity itself survives losing its last component.
        assert!(world.contains(entity));
        assert_eq!(world.len(), 1);
    }

    #[test]
    fn inserting_the_same_component_twice_replaces_it() {
        let mut world = World::new();
        let entity = world.spawn();

        world
            .insert(entity, Health(10))
            .expect("the entity is alive");
        world
            .insert(entity, Health(3))
            .expect("the entity is alive");

        assert_eq!(
            *world.get::<Health>(entity).expect("just inserted"),
            Health(3)
        );
    }

    #[test]
    fn reading_a_component_the_entity_does_not_have_names_it() {
        let mut world = World::new();
        let entity = world.spawn();

        let error = world
            .get::<Position>(entity)
            .expect_err("no Position was inserted");

        match error {
            EcsError::MissingComponent {
                entity: reported,
                component,
            } => {
                assert_eq!(reported, entity);
                assert!(
                    component.ends_with("Position"),
                    "the error should name the component type, got {component}"
                );
            }
            other => panic!("expected a missing component error, got {other:?}"),
        }
    }

    #[test]
    fn a_despawned_entity_is_gone_and_its_handle_stays_invalid() {
        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Health(1))
            .expect("the entity is alive");

        world.despawn(entity).expect("the entity is alive");

        assert!(!world.contains(entity));
        assert!(!world.has::<Health>(entity));
        assert!(world.is_empty());

        assert!(matches!(
            world.despawn(entity),
            Err(EcsError::NoSuchEntity { .. })
        ));
        assert!(matches!(
            world.get::<Health>(entity),
            Err(EcsError::NoSuchEntity { .. })
        ));
        assert!(matches!(
            world.insert(entity, Health(2)),
            Err(EcsError::NoSuchEntity { .. })
        ));
        assert!(matches!(
            world.remove::<Health>(entity),
            Err(EcsError::NoSuchEntity { .. })
        ));
    }

    #[test]
    fn a_recycled_slot_does_not_answer_for_the_handle_that_used_it() {
        let mut world = World::new();

        let old = world.spawn();
        world.insert(old, Health(1)).expect("the entity is alive");
        world.despawn(old).expect("the entity is alive");

        // hecs hands the freed slot straight back, which is exactly the
        // situation a generation exists for.
        let new = world.spawn();
        world.insert(new, Health(99)).expect("the entity is alive");

        assert_eq!(
            new.index(),
            old.index(),
            "the slot is expected to be reused"
        );
        assert_ne!(new, old);
        assert!(new.generation() > old.generation());

        // The stale handle must not reach the new entity's data.
        assert!(!world.contains(old));
        assert!(matches!(
            world.get::<Health>(old),
            Err(EcsError::NoSuchEntity { .. })
        ));
        assert_eq!(
            *world.get::<Health>(new).expect("just inserted"),
            Health(99)
        );

        // ... and the new entity is unharmed by the stale handle's despawn.
        assert!(matches!(
            world.despawn(old),
            Err(EcsError::NoSuchEntity { .. })
        ));
        assert!(world.contains(new));
    }

    #[test]
    fn entity_ids_are_sorted_regardless_of_spawn_and_archetype_order() {
        let mut world = World::new();

        // Different component sets put these in different archetypes, and the
        // despawn in the middle makes the slots come back out of order.
        let a = world.spawn();
        world.insert(a, Health(1)).expect("alive");
        let b = world.spawn();
        world.insert(b, Position { x: 0, y: 0 }).expect("alive");
        let c = world.spawn();
        world.insert(c, Health(2)).expect("alive");
        world.insert(c, Velocity { dx: 1, dy: 1 }).expect("alive");

        world.despawn(b).expect("alive");
        let d = world.spawn();

        let ids = world.entity_ids();

        assert_eq!(ids.len(), 3);
        let mut expected = vec![a, c, d];
        expected.sort_unstable();
        assert_eq!(ids, expected);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_query_over_two_components_reads_only_matching_entities() {
        let mut world = World::new();

        let moving = world.spawn();
        world
            .insert(moving, Position { x: 1, y: 1 })
            .expect("alive");
        world
            .insert(moving, Velocity { dx: 2, dy: 3 })
            .expect("alive");

        let still = world.spawn();
        world.insert(still, Position { x: 9, y: 9 }).expect("alive");

        let mut query = world.query::<(&Position, &Velocity)>();
        let mut seen: Vec<_> = query
            .iter()
            .map(|(id, (position, velocity))| (id, position.x, velocity.dx))
            .collect();
        seen.sort_unstable();

        assert_eq!(seen, vec![(moving, 1, 2)]);
    }

    #[test]
    fn a_query_over_two_components_can_write_one_of_them() {
        let mut world = World::new();

        let entity = world.spawn();
        world
            .insert(entity, Position { x: 0, y: 0 })
            .expect("alive");
        world
            .insert(entity, Velocity { dx: 2, dy: -1 })
            .expect("alive");

        {
            let mut query = world.query::<(&mut Position, &Velocity)>();
            for (_id, (position, velocity)) in query.iter() {
                position.x += velocity.dx;
                position.y += velocity.dy;
            }
        }

        assert_eq!(
            *world.get::<Position>(entity).expect("alive"),
            Position { x: 2, y: -1 }
        );
    }

    #[test]
    fn a_query_over_three_components_reads_only_matching_entities() {
        let mut world = World::new();

        let full = world.spawn();
        world.insert(full, Position { x: 1, y: 1 }).expect("alive");
        world
            .insert(full, Velocity { dx: 1, dy: 1 })
            .expect("alive");
        world.insert(full, Health(50)).expect("alive");

        let partial = world.spawn();
        world
            .insert(partial, Position { x: 2, y: 2 })
            .expect("alive");
        world.insert(partial, Health(10)).expect("alive");

        let mut query = world.query::<(&Position, &Velocity, &Health)>();
        let mut seen: Vec<_> = query
            .iter()
            .map(|(id, (position, velocity, health))| (id, position.x, velocity.dx, health.0))
            .collect();
        seen.sort_unstable();

        assert_eq!(seen, vec![(full, 1, 1, 50)]);
    }

    #[test]
    fn a_query_over_three_components_can_write_two_of_them() {
        let mut world = World::new();

        let entity = world.spawn();
        world
            .insert(entity, Position { x: 0, y: 0 })
            .expect("alive");
        world
            .insert(entity, Velocity { dx: 3, dy: 4 })
            .expect("alive");
        world.insert(entity, Health(10)).expect("alive");

        {
            let mut query = world.query::<(&mut Position, &Velocity, &mut Health)>();
            for (_id, (position, velocity, health)) in query.iter() {
                position.x += velocity.dx;
                position.y += velocity.dy;
                health.0 -= 1;
            }
        }

        assert_eq!(
            *world.get::<Position>(entity).expect("alive"),
            Position { x: 3, y: 4 }
        );
        assert_eq!(*world.get::<Health>(entity).expect("alive"), Health(9));
    }

    #[test]
    fn a_query_guard_can_be_iterated_through_into_iterator() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, Health(4)).expect("alive");

        let mut query = world.query::<&Health>();
        let mut count = 0;
        for (id, health) in &mut query {
            assert_eq!(id, entity);
            assert_eq!(health.0, 4);
            count += 1;
        }

        assert_eq!(count, 1);
    }

    /// Spells a handle the way a canonical dump does, so a failure reads like
    /// the artefact it is about.
    fn spell(ids: &[crate::EntityId]) -> Vec<String> {
        ids.iter()
            .map(|id| format!("{}v{}", id.index(), id.generation()))
            .collect()
    }

    /// A world with two holes, freed in an order that is not ascending.
    ///
    /// Both properties matter and neither is decoration: one hole would not show
    /// that the *order* of the free list is load bearing, and freeing in
    /// ascending order would agree with a rebuild that sorted.
    fn two_holes() -> (World, Vec<crate::EntityId>) {
        let mut world = World::new();
        let ids: Vec<_> = (0..5).map(|_| world.spawn()).collect();
        world.despawn(ids[3]).expect("alive");
        world.despawn(ids[1]).expect("alive");
        (world, ids)
    }

    #[test]
    fn the_freelist_is_what_the_next_spawns_take() {
        let (mut world, _) = two_holes();

        assert_eq!(spell(&world.freelist()), vec!["3v2", "1v2"]);

        // Consumed from the back, and with the generation the despawn raised.
        let taken: Vec<_> = (0..4).map(|_| world.spawn()).collect();
        assert_eq!(spell(&taken), vec!["1v2", "3v2", "5v1", "6v1"]);
        assert!(world.freelist().is_empty());
    }

    #[test]
    fn a_reconstituted_world_hands_out_the_handles_the_original_would_have() {
        let (mut original, _) = two_holes();

        let mut rebuilt = World::reconstitute(&original.entity_ids(), &original.freelist())
            .expect("a world's own table describes a world");

        assert_eq!(rebuilt.entity_ids(), original.entity_ids());
        assert_eq!(rebuilt.freelist(), original.freelist());
        assert_eq!(rebuilt.len(), original.len());

        let from_original: Vec<_> = (0..4).map(|_| original.spawn()).collect();
        let from_rebuilt: Vec<_> = (0..4).map(|_| rebuilt.spawn()).collect();
        assert_eq!(from_rebuilt, from_original);
    }

    /// The trap this whole seam exists for, asserted rather than described.
    ///
    /// A table listing only the live entities is what a save that trusted the
    /// canonical dump would write - the dump prints live entities and nothing
    /// else. The world it describes is not the world it came from, and the
    /// difference is invisible until something spawns. Here it is refused
    /// instead, by name and by the slot.
    #[test]
    fn a_table_of_live_entities_alone_is_refused_rather_than_silently_smaller() {
        let (world, _) = two_holes();

        let error = World::reconstitute(&world.entity_ids(), &[])
            .expect_err("slots 1 and 3 are named by neither list");

        match error {
            EcsError::SlotMissing { index, highest } => {
                assert_eq!((index, highest), (1, 4));
            }
            other => panic!("expected a missing slot, got {other:?}"),
        }
    }

    #[test]
    fn a_table_naming_one_slot_twice_is_refused() {
        let mut world = World::new();
        let ids: Vec<_> = (0..2).map(|_| world.spawn()).collect();

        let doubled = vec![ids[0], ids[0], ids[1]];
        assert!(matches!(
            World::reconstitute(&doubled, &[]),
            Err(EcsError::SlotNamedTwice { index: 0 })
        ));

        // ... and the same rule on the other list.
        let mut with_hole = World::new();
        let ids: Vec<_> = (0..2).map(|_| with_hole.spawn()).collect();
        with_hole.despawn(ids[0]).expect("alive");
        let free = with_hole.freelist();
        let twice = vec![free[0], free[0]];
        assert!(matches!(
            World::reconstitute(&with_hole.entity_ids(), &twice),
            Err(EcsError::SlotNamedTwice { index: 0 })
        ));
    }

    #[test]
    fn a_table_naming_one_slot_live_and_free_is_refused() {
        let mut world = World::new();
        let ids: Vec<_> = (0..2).map(|_| world.spawn()).collect();
        world.despawn(ids[0]).expect("alive");

        // Slot 1 is alive; claiming it is also waiting to be handed out is the
        // case the storage engine answers with a debug assertion, so it has to
        // be refused before it gets there.
        let live = world.entity_ids();
        assert!(matches!(
            World::reconstitute(&live, &live),
            Err(EcsError::SlotLiveAndFree { index: 1 })
        ));
    }

    #[test]
    fn an_empty_table_reconstitutes_an_empty_world() {
        let world = World::reconstitute(&[], &[]).expect("nothing to contradict");

        assert!(world.is_empty());
        assert!(world.freelist().is_empty());
        assert_eq!(world.entity_ids(), Vec::new());
    }

    #[test]
    fn a_reconstituted_entity_carries_no_components_and_takes_them_back() {
        let mut original = World::new();
        let entity = original.spawn();
        original.insert(entity, Health(7)).expect("alive");

        let mut rebuilt = World::reconstitute(&original.entity_ids(), &original.freelist())
            .expect("a world's own table describes a world");

        assert!(rebuilt.contains(entity));
        assert!(!rebuilt.has::<Health>(entity));

        rebuilt.insert(entity, Health(7)).expect("alive");
        assert_eq!(
            *rebuilt.get::<Health>(entity).expect("just inserted"),
            Health(7)
        );
    }

    #[test]
    fn a_query_knows_how_many_entities_it_will_yield() {
        let mut world = World::new();
        for value in 0..5 {
            let entity = world.spawn();
            world.insert(entity, Health(value)).expect("alive");
        }

        let mut query = world.query::<&Health>();
        assert_eq!(query.iter().len(), 5);
    }
}
