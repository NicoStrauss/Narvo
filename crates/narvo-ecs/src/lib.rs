//! Entities, components, queries and the system scheduler.
//!
//! Responsibility: the engine's data model and the order in which code runs over
//! it. Entities and their components live in a [`World`]; a [`Query`] says which
//! components to look at; a [`Scheduler`] runs a fixed sequence of systems over
//! the world once per simulation tick; a [`ComponentRegistry`] gives component
//! types stable names and the type-erased path that turns them into text.
//!
//! API boundary: this crate is the facade ADR-0002 decided on. `hecs` provides
//! archetype storage and query fetching underneath, and appears in exactly one
//! place in the public API - the [`Query`] supertrait, recorded in ADR-0005.
//! There is no re-export of `hecs`, and no `hecs` type appears in any other
//! signature, return type or public field. Code written against this crate does
//! not name the storage engine, which is what keeps the engine free to replace
//! it.
//!
//! Determinism: nothing here reads a clock or a source of entropy, and every
//! observable order is fixed. Query iteration follows archetype layout and is
//! explicitly *not* stable; [`World::entity_ids`] and [`ComponentRegistry::iter`]
//! are the canonical orders, sorted by entity id and by stable component name.
//! Anything that gets serialized, hashed, logged or asserted on goes through
//! those two. [`canonical_dump`] is where they meet: one world, one string, and
//! [`state_hash`] summarises it in 64 bits. ADR-0008 states what that hash
//! guarantees and — just as important — what it does not.
//!
//! Randomness and events live here too, and for one reason: both are state that
//! decides what the simulation does next, so both have to be visible to
//! [`canonical_dump`] or a divergence in them would compare as agreement. An
//! [`Rng`] is a component holding its own state (ADR-0010); an [`Events`] buffer
//! is a component holding one tick of messages (ADR-0011). Neither hides
//! anything in a local, a capture or a `static`.
//!
//! What arrives from outside a tick is **not** here. `narvo_input::InputEvent`
//! is a named action with a magnitude, knowing nothing about keys or devices
//! (ADR-0012), and it travels in an [`Events`] buffer like any other message —
//! which is why the runner can feed it from a live source or from a recorded one
//! without the simulation being able to tell the difference. It lived in this
//! crate until M5.1 and moved to `narvo-input` when that crate was split off,
//! as ADR-0012 said it should. Nothing here had to change for it: an [`Events`]
//! buffer puts no requirement on its payload that a foreign type cannot meet,
//! and `narvo-input` does not depend on this crate (ADR-0025).
//!
//! Not here either: the recording *file*, which is `narvo-app`'s, because it
//! names an application's simulation modes. This crate is the layer those are
//! built on.
//!
//! # Examples
//!
//! One tick of a simulation, end to end:
//!
//! ```
//! use narvo_ecs::{Scheduler, SystemContext, World};
//!
//! struct Position { x: i32, y: i32 }
//! struct Velocity { dx: i32, dy: i32 }
//!
//! // Systems are plain functions. They cannot capture state - see `System`.
//! fn movement(world: &mut World, _context: &SystemContext) {
//!     let mut moving = world.query::<(&mut Position, &Velocity)>();
//!     for (_id, (position, velocity)) in moving.iter() {
//!         position.x += velocity.dx;
//!         position.y += velocity.dy;
//!     }
//! }
//!
//! let mut world = World::new();
//! let entity = world.spawn();
//! world.insert(entity, Position { x: 0, y: 0 })?;
//! world.insert(entity, Velocity { dx: 1, dy: -2 })?;
//!
//! let mut scheduler = Scheduler::new();
//! scheduler.add_system("movement", movement)?;
//!
//! scheduler.run(&mut world, &SystemContext::new(0));
//!
//! let position = world.get::<Position>(entity)?;
//! assert_eq!((position.x, position.y), (1, -2));
//! # Ok::<(), narvo_ecs::EcsError>(())
//! ```

mod burst;
mod camera;
mod entity;
mod error;
mod events;
mod follow;
mod hit;
mod layer;
mod query;
mod registry;
mod rigid_body;
mod rng;
mod sampling;
mod scheduler;
mod shake;
mod sprite;
mod state;
mod tally;
mod tint;
mod transform;
mod world;

pub use burst::{Burst, advance_bursts};
pub use camera::Camera;
pub use entity::EntityId;
pub use error::EcsError;
pub use events::{Events, rotate_events};
pub use follow::{Follow, compose_camera};
pub use hit::HitRect;
pub use layer::Layer;
pub use query::{Query, QueryBorrow, QueryIter};
pub use registry::{ComponentInfo, ComponentRegistry, register_engine_components};
pub use rigid_body::RigidBody;
pub use rng::Rng;
pub use sampling::Sampling;
pub use scheduler::{Scheduler, System, SystemContext};
pub use shake::Shake;
pub use sprite::Sprite;
pub use state::{DumpDifference, canonical_dump, first_difference, state_hash};
pub use tally::Tally;
pub use tint::Tint;
pub use transform::Transform;
pub use world::{Component, Ref, RefMut, World};
