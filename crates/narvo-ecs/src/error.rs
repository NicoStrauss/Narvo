//! The error type this crate returns.

use std::error::Error;
use std::fmt;

use crate::entity::EntityId;

/// Something that went wrong in the world facade, the component registry or the
/// scheduler.
///
/// No `hecs` error type crosses this boundary. hecs' own `NoSuchEntity` and
/// `ComponentError` are translated here, which is what lets the messages name
/// the entity and the component type that were actually involved - hecs knows
/// neither at the point where it fails.
///
/// The messages are written to be read by an agent as much as by a person: they
/// say what was asked for, what was in the way, and where that leaves the
/// caller. A bare "not found" costs a round trip that a message naming the
/// handle does not.
#[derive(Debug)]
#[non_exhaustive]
pub enum EcsError {
    /// The entity is not alive in this world.
    ///
    /// Either it never was, or it was despawned. If its slot has since been
    /// recycled, the newer entity carries a higher generation, so this handle
    /// stays invalid rather than starting to address the newer one.
    NoSuchEntity {
        /// The handle that was used.
        entity: EntityId,
    },
    /// The entity is alive but does not carry the requested component.
    MissingComponent {
        /// The entity that was accessed.
        entity: EntityId,
        /// Rust type name of the component that was asked for.
        component: &'static str,
    },
    /// A component type was registered a second time.
    ComponentAlreadyRegistered {
        /// Rust type name of the component.
        component: &'static str,
        /// The stable name it already carries.
        registered_as: &'static str,
        /// The stable name the rejected registration asked for.
        requested: &'static str,
    },
    /// Two different component types asked for the same stable name.
    ComponentNameTaken {
        /// The contested stable name.
        name: &'static str,
        /// Rust type name of the component that holds the name.
        registered: &'static str,
        /// Rust type name of the component that was turned away.
        rejected: &'static str,
    },
    /// No component is registered under this stable name.
    UnknownComponent {
        /// The name that was looked up.
        name: String,
    },
    /// An entity carries a component type the registry does not know about.
    ///
    /// Raised while building the canonical dump. Skipping the component instead
    /// would be worse than failing: it would leave a part of the state outside
    /// the hash, so a run that diverged in exactly that component would compare
    /// as identical.
    UnregisteredComponent {
        /// The entity that carries it.
        entity: EntityId,
        /// Rust type name of the component, or its `TypeId` when the world has
        /// never seen the type by name.
        component: String,
    },
    /// A second system was registered under a name already in use.
    DuplicateSystem {
        /// The contested system name.
        name: &'static str,
    },
    /// Serializing a component through the registry failed.
    ComponentSerialization {
        /// Rust type name of the component that would not serialize.
        component: &'static str,
        /// The entity whose component was being written.
        entity: EntityId,
        /// The serializer's own error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// Reading a component back through the registry failed.
    ///
    /// The counterpart of [`ComponentSerialization`](Self::ComponentSerialization),
    /// raised by [`ComponentInfo::deserialize`](crate::ComponentInfo::deserialize)
    /// when the text is not a valid rendering of that component type. The
    /// deserializer's own error carries the position inside the text, which is
    /// why it is kept as a source rather than flattened into a message.
    ComponentDeserialization {
        /// Rust type name of the component that would not parse.
        component: &'static str,
        /// The entity the component was being put on.
        entity: EntityId,
        /// The deserializer's own error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// An entity table names one slot twice.
    ///
    /// Raised by [`World::reconstitute`](crate::World::reconstitute). A slot is
    /// one place in the world, so naming it twice describes no world at all —
    /// and the storage engine would answer it by silently letting the second
    /// entry win, which is the shape of failure this project keeps refusing.
    SlotNamedTwice {
        /// The slot index that appears more than once.
        index: u32,
    },
    /// An entity table names one slot as both live and free.
    ///
    /// Raised by [`World::reconstitute`](crate::World::reconstitute). The two
    /// lists partition the slots: a slot holds an entity or is waiting to be
    /// handed out, never both.
    SlotLiveAndFree {
        /// The contested slot index.
        index: u32,
    },
    /// An entity table skips a slot below its own highest one.
    ///
    /// Raised by [`World::reconstitute`](crate::World::reconstitute). A world
    /// reached by spawning and despawning holds every slot below its highest
    /// either live or free, so a table with a hole in it is one no world could
    /// have produced. Accepting it would build a world whose skipped slots can
    /// never be handed out — smaller than the one the table meant to describe,
    /// and silently so.
    SlotMissing {
        /// The slot index no list names.
        index: u32,
        /// The highest slot index either list names.
        highest: u32,
    },
}

impl fmt::Display for EcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchEntity { entity } => write!(
                f,
                "{entity:?} is not alive in this world; it was despawned or never spawned here, \
                 and a handle from before a despawn stays invalid even once its slot is reused"
            ),
            Self::MissingComponent { entity, component } => write!(
                f,
                "{entity:?} does not have a {component} component; \
                 insert one before reading it"
            ),
            Self::ComponentAlreadyRegistered {
                component,
                registered_as,
                requested,
            } => write!(
                f,
                "the component type {component} is already registered as \"{registered_as}\" and \
                 cannot also be registered as \"{requested}\"; register each component type once, \
                 under one stable name"
            ),
            Self::ComponentNameTaken {
                name,
                registered,
                rejected,
            } => write!(
                f,
                "the stable component name \"{name}\" is already held by {registered}, \
                 so {rejected} cannot take it; stable names are what serialized state is keyed by \
                 and two types sharing one would make that state ambiguous"
            ),
            Self::UnknownComponent { name } => write!(
                f,
                "no component is registered under the stable name \"{name}\"; \
                 register it before looking it up"
            ),
            Self::UnregisteredComponent { entity, component } => write!(
                f,
                "{entity:?} carries a {component}, which is not registered, so the canonical \
                 state dump cannot serialize it; register it with \
                 ComponentRegistry::register_component. Leaving it out is not an option: a \
                 component the state hash cannot see makes a divergence in it invisible"
            ),
            Self::DuplicateSystem { name } => write!(
                f,
                "a system named \"{name}\" is already registered; \
                 system names identify a system in the run order and have to stay unambiguous"
            ),
            Self::ComponentSerialization {
                component,
                entity,
                source,
            } => write!(
                f,
                "serializing the {component} component of {entity:?} failed: {source}"
            ),
            Self::ComponentDeserialization {
                component,
                entity,
                source,
            } => write!(
                f,
                "reading a {component} component for {entity:?} failed: {source}"
            ),
            Self::SlotNamedTwice { index } => write!(
                f,
                "slot {index} is named twice in the entity table; a slot is one place in a \
                 world, so a table naming it twice describes no world"
            ),
            Self::SlotLiveAndFree { index } => write!(
                f,
                "slot {index} is named as both a live entity and a free one; the two lists \
                 partition the slots, so a slot either holds an entity or waits to be handed out"
            ),
            Self::SlotMissing { index, highest } => write!(
                f,
                "slot {index} is named neither live nor free, and the table reaches up to slot \
                 {highest}; a world holds every slot below its highest one, so the table is \
                 missing an entry rather than describing a smaller world"
            ),
        }
    }
}

impl Error for EcsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::NoSuchEntity { .. }
            | Self::MissingComponent { .. }
            | Self::ComponentAlreadyRegistered { .. }
            | Self::ComponentNameTaken { .. }
            | Self::UnknownComponent { .. }
            | Self::UnregisteredComponent { .. }
            | Self::DuplicateSystem { .. }
            | Self::SlotNamedTwice { .. }
            | Self::SlotLiveAndFree { .. }
            | Self::SlotMissing { .. } => None,
            Self::ComponentSerialization { source, .. }
            | Self::ComponentDeserialization { source, .. } => Some(source.as_ref()),
        }
    }
}
