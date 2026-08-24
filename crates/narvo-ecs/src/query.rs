//! Queries: which components to fetch, and the guard that borrows the world
//! while they are iterated.
//!
//! This module holds the one place where `hecs` is visible in this crate's
//! public API. ADR-0005 records why, and [`Query`] repeats it at the point where
//! it matters.

use crate::entity::EntityId;

/// Keeps [`Query`] closed to implementations outside this crate.
///
/// The blanket implementation below would already make an outside implementation
/// impossible by coherence. The explicit seal is here so the intent is stated
/// rather than inferred from an overlap error.
mod sealed {
    /// Implemented for every hecs query, and nameable only from inside this
    /// crate.
    pub trait Sealed {}

    impl<Q: hecs::Query> Sealed for Q {}
}

/// The set of components a query fetches, written as a tuple of references.
///
/// A caller names only their own types:
///
/// ```text
/// world.query::<(&Position, &Velocity)>()      // read two components
/// world.query::<(&mut Position, &Velocity)>()  // write one, read the other
/// ```
///
/// # The hecs seam
///
/// [`hecs::Query`] as a supertrait is the single point at which the storage
/// engine is visible in this crate's public API - the seam ADR-0002 asks for and
/// ADR-0005 records. It is a supertrait rather than an implementation detail
/// because the tuple-of-references notation *is* hecs': re-declaring it would
/// mean re-implementing hecs' component fetching, which is the opposite of what
/// ADR-0002 decided.
///
/// Nothing else crosses. hecs is not re-exported, no hecs type appears in any
/// signature, return type or public field of this crate, and a caller writes
/// queries without ever naming hecs. The trait is sealed, so the seam cannot be
/// widened from outside either.
///
/// # Iteration order
///
/// Query iteration order is **not** stable and must never be observed. hecs
/// groups entities by archetype - by their exact set of component types - and
/// visits archetypes in the order they came into existence, so inserting a
/// component, removing one, or spawning entities in a different order rearranges
/// the result even though the set of results is identical.
///
/// This is not a defect to be worked around at the call site with a lucky
/// ordering. Anything observable - a serialized dump, a state hash, a log an
/// assertion reads - is sorted by [`EntityId`] first. [`World::entity_ids`] hands
/// out exactly that order, and [`EntityId`]'s own documentation explains why it
/// is index-major.
///
/// [`World::entity_ids`]: crate::World::entity_ids
pub trait Query: hecs::Query + sealed::Sealed {}

impl<Q: hecs::Query> Query for Q {}

/// A borrow of the world, held for as long as a query is being iterated.
///
/// Returned by [`World::query`](crate::World::query). It exists as its own type
/// because the borrow has to outlive the iterator: components are handed out as
/// references into the world's storage, and something has to keep the world
/// borrowed - and, for a query that writes, keep hecs' dynamic borrow flags
/// raised - while those references are alive. That rules out a bare
/// `fn query(&self) -> impl Iterator`: the guard would be a temporary of the
/// call and get dropped before the first item is read.
///
/// So it is two steps at the call site, and the guard needs its own binding:
///
/// ```
/// # use narvo_ecs::World;
/// # struct Position { x: i32 }
/// # let mut world = World::new();
/// # let entity = world.spawn();
/// # world.insert(entity, Position { x: 1 }).unwrap();
/// let mut positions = world.query::<&Position>();
/// for (id, position) in positions.iter() {
///     assert_eq!(position.x, 1);
///     let _ = id;
/// }
/// ```
///
/// # Panics
///
/// Iterating a query that asks for `&mut T` panics if another live query or
/// component reference already borrows `T` in a conflicting way. hecs checks
/// this dynamically, at [`iter`](Self::iter), rather than at compile time. The
/// fix is always the same: drop the earlier guard first.
pub struct QueryBorrow<'w, Q: Query> {
    /// `hecs::Entity` is fetched alongside `Q` so the facade can pair every item
    /// with its [`EntityId`] without a second lookup.
    inner: hecs::QueryBorrow<'w, (hecs::Entity, Q)>,
}

impl<'w, Q: Query> QueryBorrow<'w, Q> {
    /// Starts a query against `world`. Nothing is borrowed until
    /// [`iter`](Self::iter) is called.
    pub(crate) fn new(world: &'w hecs::World) -> Self {
        Self {
            inner: world.query::<(hecs::Entity, Q)>(),
        }
    }

    /// Executes the query.
    ///
    /// The iterator borrows this guard, which is what keeps the world borrowed
    /// for as long as items are alive.
    ///
    /// # Panics
    ///
    /// See the type-level documentation: a conflicting outstanding borrow of a
    /// component this query writes panics here.
    pub fn iter(&mut self) -> QueryIter<'_, Q> {
        QueryIter {
            inner: self.inner.iter(),
        }
    }
}

impl<'q, Q: Query> IntoIterator for &'q mut QueryBorrow<'_, Q> {
    type Item = (EntityId, Q::Item<'q>);
    type IntoIter = QueryIter<'q, Q>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterator over the entities matching a query.
///
/// Yields `(EntityId, Item)`, where `Item` is the tuple of references the query
/// asked for. The order is the archetype order described on [`Query`] and is not
/// stable - sort by the [`EntityId`] before anything observes it.
pub struct QueryIter<'q, Q: Query> {
    inner: hecs::QueryIter<'q, (hecs::Entity, Q)>,
}

impl<'q, Q: Query> Iterator for QueryIter<'q, Q> {
    type Item = (EntityId, Q::Item<'q>);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|(entity, item)| (EntityId::from_hecs(entity), item))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<Q: Query> ExactSizeIterator for QueryIter<'_, Q> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}
