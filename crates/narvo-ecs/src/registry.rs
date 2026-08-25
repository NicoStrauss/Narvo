//! The component registry: stable names for component types, and the
//! type-erased path from an entity to its serialized components.

use std::any::{TypeId, type_name};
use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::entity::EntityId;
use crate::error::EcsError;
use crate::world::{Component, World};

/// The type-erased serialization path stored per registered component.
///
/// A plain function pointer, monomorphized once per component type at
/// registration. `Ok(None)` means the entity is alive but does not carry this
/// component - the ordinary case, not a failure.
type SerializeComponent = fn(&World, EntityId) -> Result<Option<String>, EcsError>;

/// The type-erased deserialization path stored per registered component.
///
/// The mirror image of [`SerializeComponent`], and a function pointer for the
/// same reason: one instantiation per component type, monomorphized at
/// registration, with the type parameter consumed there.
///
/// It *inserts* rather than returning the component, because returning it would
/// need the caller to name `T` — which is the one thing an erased path exists to
/// avoid. The component is therefore constructed and placed in one step.
type DeserializeComponent = fn(&mut World, EntityId, &str) -> Result<(), EcsError>;

/// Everything the engine knows about one registered component type.
///
/// Obtained from [`ComponentRegistry::get`] or by iterating a registry.
#[derive(Clone, Copy)]
pub struct ComponentInfo {
    name: &'static str,
    type_id: TypeId,
    type_name: &'static str,
    serialize: SerializeComponent,
    deserialize: DeserializeComponent,
}

impl fmt::Debug for ComponentInfo {
    /// Prints the two names and nothing else.
    ///
    /// A derived `Debug` would print the `serialize` function pointer as an
    /// address and the `TypeId` as an opaque hash, neither of which is the same
    /// from one run to the next. This crate exists to keep observable output
    /// reproducible, and a `Debug` string that quietly is not tends to end up in
    /// an assertion. [`Scheduler`](crate::Scheduler) hand-writes its `Debug` for
    /// the same reason.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentInfo")
            .field("name", &self.name)
            .field("type_name", &self.type_name)
            .finish_non_exhaustive()
    }
}

impl ComponentInfo {
    /// The stable name this component was registered under.
    ///
    /// Stable means: it survives renaming the Rust type, and changing it
    /// invalidates saved state. This is the key serialized state is written
    /// under.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The `TypeId` of the component type.
    ///
    /// Lets a caller that holds a `T` find out whether this entry describes it,
    /// without the registry having to be generic over `T`.
    #[must_use]
    pub fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// The Rust type name of the component type.
    ///
    /// Diagnostics only. Unlike [`name`](Self::name) it is not stable - the
    /// compiler decides how it is spelled - so nothing may key off it.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// Serializes this component of `entity`, without the caller naming the
    /// component's type.
    ///
    /// Returns `Ok(None)` when the entity does not carry this component. That is
    /// what makes a full state dump possible: walk the entities, walk the
    /// registry, and write what comes back.
    ///
    /// The format is RON, matching the engine's text content format. The output
    /// is byte-for-byte what `ron::to_string` produces for the component itself -
    /// there is no wrapper - which is what M2.2 will hash.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if the entity is not alive, and
    /// [`EcsError::ComponentSerialization`] if the component's own `Serialize`
    /// implementation fails.
    pub fn serialize(&self, world: &World, entity: EntityId) -> Result<Option<String>, EcsError> {
        (self.serialize)(world, entity)
    }

    /// Reads this component back out of `text` and puts it on `entity`, without
    /// the caller naming the component's type.
    ///
    /// The exact inverse of [`serialize`](Self::serialize): what that function
    /// wrote, this one reads. `text` is the component's own RON with no wrapper
    /// — the same string [`serialize`](Self::serialize) hands back — which is
    /// what lets a caller that has both round trip a component it cannot name.
    ///
    /// An existing `T` on the entity is replaced, matching
    /// [`World::insert`](crate::World::insert). There is no "already present"
    /// error, because the caller that would want one is loading a fresh entity
    /// and can see for itself what it has already put there.
    ///
    /// # The bound that was reserved for this
    ///
    /// [`register_component`](ComponentRegistry::register_component) has
    /// required `DeserializeOwned` since M2.1 and its documentation says why:
    /// *"a component that can be written but not read back is a state format
    /// that cannot be loaded — a defect that would only surface once saving and
    /// loading exist."* This is that moment. Nothing about the registration
    /// signature changes; the bound simply stops being unused.
    ///
    /// # Errors
    ///
    /// [`EcsError::NoSuchEntity`] if the entity is not alive, and
    /// [`EcsError::ComponentDeserialization`] if `text` is not a valid RON
    /// rendering of this component type.
    pub fn deserialize(
        &self,
        world: &mut World,
        entity: EntityId,
        text: &str,
    ) -> Result<(), EcsError> {
        (self.deserialize)(world, entity, text)
    }
}

/// Stable names for component types, and the serialization path that goes with
/// them.
///
/// hecs stores components by `TypeId` and offers no reflection, so a component
/// type cannot be named, listed or serialized without something that was told
/// about it first. That something is this registry, and it exists at this
/// milestone rather than with the state hash it serves, because it decides what
/// the hash can even see.
///
/// # Canonical order
///
/// Iteration is sorted by stable name, always. Both indexes are [`BTreeMap`]s
/// rather than hash maps for that reason: a hash map's iteration order varies
/// between processes, and a state hash built on top of one would differ between
/// two runs of the same simulation - the precise failure this crate exists to
/// prevent.
///
/// # Examples
///
/// ```
/// use narvo_ecs::{ComponentRegistry, World};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct Position { x: i32, y: i32 }
///
/// let mut registry = ComponentRegistry::new();
/// registry.register_component::<Position>("position")?;
///
/// let mut world = World::new();
/// let entity = world.spawn();
/// world.insert(entity, Position { x: 3, y: -1 })?;
///
/// let info = registry.get("position").expect("just registered");
/// assert_eq!(info.serialize(&world, entity)?.as_deref(), Some("(x:3,y:-1)"));
///
/// // An entity without the component is not an error - it is an absence.
/// let bare = world.spawn();
/// assert_eq!(info.serialize(&world, bare)?, None);
/// # Ok::<(), narvo_ecs::EcsError>(())
/// ```
#[derive(Default, Clone)]
pub struct ComponentRegistry {
    /// Keyed by stable name; the source of the canonical iteration order.
    by_name: BTreeMap<&'static str, ComponentInfo>,
    /// Reverse index, so registering a type twice is caught even under a
    /// different name. Ordered for the same reason as `by_name`.
    name_by_type: BTreeMap<TypeId, &'static str>,
}

impl ComponentRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T` under the stable name `name`.
    ///
    /// The `DeserializeOwned` bound was required from M2.1 and unused until
    /// M4.1, on the reasoning that dropping it later is a breaking change for
    /// every caller, while a component that can be written but not read back is
    /// a state format that cannot be loaded - a defect that would only surface
    /// once saving and loading exist. Loading exists now:
    /// [`ComponentInfo::deserialize`] is what the bound was being held for, and
    /// this signature did not have to move to get it.
    ///
    /// # Errors
    ///
    /// [`EcsError::ComponentAlreadyRegistered`] if `T` is already registered,
    /// under this or any other name, and [`EcsError::ComponentNameTaken`] if
    /// another type already holds `name`. Neither case overwrites: a silently
    /// replaced registration would change the meaning of every state dump
    /// written afterwards.
    pub fn register_component<T>(&mut self, name: &'static str) -> Result<(), EcsError>
    where
        T: Component + Serialize + DeserializeOwned,
    {
        let type_id = TypeId::of::<T>();

        if let Some(registered_as) = self.name_by_type.get(&type_id) {
            return Err(EcsError::ComponentAlreadyRegistered {
                component: type_name::<T>(),
                registered_as,
                requested: name,
            });
        }

        if let Some(existing) = self.by_name.get(name) {
            return Err(EcsError::ComponentNameTaken {
                name,
                registered: existing.type_name,
                rejected: type_name::<T>(),
            });
        }

        self.by_name.insert(
            name,
            ComponentInfo {
                name,
                type_id,
                type_name: type_name::<T>(),
                serialize: serialize_component::<T>,
                deserialize: deserialize_component::<T>,
            },
        );
        self.name_by_type.insert(type_id, name);

        Ok(())
    }

    /// Looks a component up by its stable name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ComponentInfo> {
        self.by_name.get(name)
    }

    /// The stable name `T` is registered under, if it is registered.
    #[must_use]
    pub fn name_of<T: Component>(&self) -> Option<&'static str> {
        self.name_by_type.get(&TypeId::of::<T>()).copied()
    }

    /// The same lookup for a caller that holds an erased type rather than a `T`.
    ///
    /// The canonical dump is exactly that caller: it asks the world which
    /// component types an entity carries and gets `TypeId`s back, with no way
    /// to name them again except through here.
    #[must_use]
    pub fn name_of_type(&self, type_id: TypeId) -> Option<&'static str> {
        self.name_by_type.get(&type_id).copied()
    }

    /// Whether a component is registered under this stable name.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Every registered component, in stable-name order.
    ///
    /// This order is canonical and is the order a state dump walks in.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &ComponentInfo> {
        self.by_name.values()
    }

    /// Serializes the component registered as `name` of `entity`.
    ///
    /// Shorthand for [`get`](Self::get) followed by
    /// [`ComponentInfo::serialize`], with the missing-registration case turned
    /// into an error rather than an `Option` the caller has to unwrap.
    ///
    /// # Errors
    ///
    /// [`EcsError::UnknownComponent`] if nothing is registered under `name`,
    /// plus everything [`ComponentInfo::serialize`] can return.
    pub fn serialize_component(
        &self,
        name: &str,
        world: &World,
        entity: EntityId,
    ) -> Result<Option<String>, EcsError> {
        self.by_name
            .get(name)
            .ok_or_else(|| EcsError::UnknownComponent {
                name: name.to_owned(),
            })?
            .serialize(world, entity)
    }

    /// Reads the component registered as `name` out of `text` and puts it on
    /// `entity`.
    ///
    /// Shorthand for [`get`](Self::get) followed by
    /// [`ComponentInfo::deserialize`], and the mirror of
    /// [`serialize_component`](Self::serialize_component) down to the
    /// missing-registration case being an error rather than an `Option`.
    ///
    /// # Errors
    ///
    /// [`EcsError::UnknownComponent`] if nothing is registered under `name`,
    /// plus everything [`ComponentInfo::deserialize`] can return.
    pub fn deserialize_component(
        &self,
        name: &str,
        world: &mut World,
        entity: EntityId,
        text: &str,
    ) -> Result<(), EcsError> {
        self.by_name
            .get(name)
            .ok_or_else(|| EcsError::UnknownComponent {
                name: name.to_owned(),
            })?
            .deserialize(world, entity, text)
    }

    /// How many component types are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

impl fmt::Debug for ComponentRegistry {
    /// Lists the registered stable names, in the registry's canonical order.
    ///
    /// Hand-written rather than derived for the reason given on
    /// [`ComponentInfo`]'s own `Debug`, and with the same effect: the output is
    /// identical from one run to the next, and it says the one thing worth
    /// seeing - what is registered, in the order a state dump will walk it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ComponentRegistry")
            .field("components", &self.by_name.keys())
            .finish()
    }
}

/// The body behind every [`SerializeComponent`] pointer, one instantiation per
/// registered component type.
///
/// This is the whole of the type erasure: the type parameter is consumed here,
/// at registration, and what the registry stores afterwards mentions no
/// component type at all.
fn serialize_component<T>(world: &World, entity: EntityId) -> Result<Option<String>, EcsError>
where
    T: Component + Serialize,
{
    match world.get::<T>(entity) {
        Ok(component) => {
            let text =
                ron::to_string(&*component).map_err(|source| EcsError::ComponentSerialization {
                    component: type_name::<T>(),
                    entity,
                    source: Box::new(source),
                })?;
            Ok(Some(text))
        }
        // An entity that simply does not carry this component is the normal case
        // when walking a registry, so it is an absence rather than an error.
        Err(EcsError::MissingComponent { .. }) => Ok(None),
        Err(other) => Err(other),
    }
}

/// The body behind every [`DeserializeComponent`] pointer, one instantiation per
/// registered component type.
///
/// The mirror of [`serialize_component`], and it uses `ron::from_str` against
/// that function's `ron::to_string` deliberately: ADR-0006 fixed RON as the
/// registry's format, and a reader in any other format would not be able to read
/// what the writer writes.
///
/// A malformed `text` is an error and never a silently skipped component. The
/// asymmetry with the serializing side — where a missing component is `Ok(None)`
/// rather than a failure — is intentional: an absence is a fact about the world,
/// while unreadable text is a fact about the input, and quietly dropping a
/// component the caller asked for would produce a world that is missing state
/// nobody was told about.
fn deserialize_component<T>(world: &mut World, entity: EntityId, text: &str) -> Result<(), EcsError>
where
    T: Component + DeserializeOwned,
{
    // Checked before parsing, so that a dead entity is reported as a dead entity
    // rather than as whatever the text happens to be.
    if !world.contains(entity) {
        return Err(EcsError::NoSuchEntity { entity });
    }

    let component: T =
        ron::from_str(text).map_err(|source| EcsError::ComponentDeserialization {
            component: type_name::<T>(),
            entity,
            source: Box::new(source),
        })?;

    world.insert(entity, component)
}

/// Registers every component type the engine itself offers, under the stable
/// names the whole workspace uses.
///
/// # Why this list lives here
///
/// Because two things outside this crate need it and neither may own it. The
/// validation CLI checks scenes against it; the runner's scene-file mode
/// constitutes a world from it. A third copy would be a third place to forget a
/// new type, and the failure it produces is not a compile error — it is a scene
/// that suddenly reports "unknown component" for something the engine plainly
/// has.
///
/// **This is not the thing ADR-0018 forbids.** That ADR keeps a component type
/// list out of `narvo-scene`, because the *format* must stay component-open: a
/// scene may carry whatever its caller registered, and the format crate never
/// names a type. This is the opposite direction — the engine saying which
/// components *it* provides — and a caller is free to register these, some of
/// them, or none, plus its own. `narvo-app`'s demo simulations do exactly that
/// and are untouched by it.
///
/// # What it is not
///
/// It is not "the components a world must have", and registering a type does not
/// put it in a world. It is also not a promise that the set is stable: a type
/// added to the engine is added here, and the state hash of anything that
/// registers the whole set moves accordingly. That is the same rule
/// `ProjektPlan.md` §12 already states — being in the hash is a property of a
/// scenario, not of a type.
///
/// # Errors
///
/// [`EcsError::ComponentAlreadyRegistered`] or [`EcsError::ComponentNameTaken`]
/// if `registry` already holds one of these types or names. Passing a fresh
/// registry cannot fail.
pub fn register_engine_components(registry: &mut ComponentRegistry) -> Result<(), EcsError> {
    registry.register_component::<crate::Transform>("transform")?;
    registry.register_component::<crate::Layer>("layer")?;
    registry.register_component::<crate::Burst>("burst")?;
    registry.register_component::<crate::Sampling>("sampling")?;
    registry.register_component::<crate::Camera>("camera")?;
    registry.register_component::<crate::Follow>("follow")?;
    registry.register_component::<crate::Shake>("shake")?;
    registry.register_component::<crate::Rng>("rng")?;
    registry.register_component::<crate::Sprite>("sprite")?;
    registry.register_component::<crate::HitRect>("hitrect")?;
    registry.register_component::<crate::Tally>("tally")?;
    registry.register_component::<crate::RigidBody>("rigidbody")?;
    registry.register_component::<crate::Tint>("tint")?;
    registry.register_component::<crate::Occluder>("occluder")?;
    registry.register_component::<crate::LightSource>("lightsource")?;
    registry.register_component::<crate::Lit>("lit")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ComponentRegistry;
    use crate::{EcsError, World};
    use serde::{Deserialize, Serialize};
    use std::any::TypeId;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Position {
        x: i32,
        y: i32,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Velocity {
        dx: i32,
        dy: i32,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Health(i32);

    /// The engine's own set registers, in canonical order, on a fresh registry.
    ///
    /// The order is the registry's, not the call order, and it is asserted so
    /// that a type added to `register_engine_components` shows up here as a
    /// deliberate edit rather than as a surprise in a scene file somewhere.
    #[test]
    fn the_engine_component_set_registers_under_its_stable_names() {
        let mut registry = ComponentRegistry::new();
        super::register_engine_components(&mut registry).expect("a fresh registry accepts them");

        let names: Vec<&str> = registry.iter().map(|info| info.name()).collect();
        assert_eq!(
            names,
            vec![
                "burst",
                "camera",
                "follow",
                "hitrect",
                "layer",
                "lightsource",
                "lit",
                "occluder",
                "rigidbody",
                "rng",
                "sampling",
                "shake",
                "sprite",
                "tally",
                "tint",
                "transform"
            ]
        );
    }

    /// It refuses to run twice over one registry rather than half-registering.
    #[test]
    fn registering_the_engine_set_twice_is_an_error() {
        let mut registry = ComponentRegistry::new();
        super::register_engine_components(&mut registry).expect("the first time");

        assert!(super::register_engine_components(&mut registry).is_err());
    }

    /// A caller may add its own components beside the engine's.
    ///
    /// The property that keeps this function from being a closed world: a
    /// simulation registers the engine set and then whatever else it has, which
    /// is what `narvo-app`'s scene mode does with its own `Wander`.
    #[test]
    fn a_caller_can_register_its_own_components_as_well() {
        let mut registry = ComponentRegistry::new();
        super::register_engine_components(&mut registry).expect("a fresh registry accepts them");
        registry
            .register_component::<Position>("position")
            .expect("a name the engine does not use");

        assert_eq!(registry.len(), 17);
    }

    #[test]
    fn a_fresh_registry_is_empty() {
        let registry = ComponentRegistry::new();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert_eq!(registry.iter().count(), 0);
    }

    #[test]
    fn a_component_is_found_under_its_stable_name() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("the first registration of a type always succeeds");

        let info = registry.get("position").expect("just registered");
        assert_eq!(info.name(), "position");
        assert_eq!(info.type_id(), TypeId::of::<Position>());
        assert!(info.type_name().ends_with("Position"));

        assert!(registry.contains("position"));
        assert_eq!(registry.name_of::<Position>(), Some("position"));

        assert!(registry.get("nothing").is_none());
        assert!(!registry.contains("nothing"));
        assert_eq!(registry.name_of::<Velocity>(), None);
    }

    #[test]
    fn registering_the_same_type_twice_is_an_error() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("the first registration of a type always succeeds");

        let error = registry
            .register_component::<Position>("position_again")
            .expect_err("a type may only be registered once");

        match error {
            EcsError::ComponentAlreadyRegistered {
                component,
                registered_as,
                requested,
            } => {
                assert!(component.ends_with("Position"));
                assert_eq!(registered_as, "position");
                assert_eq!(requested, "position_again");
            }
            other => panic!("expected a double registration error, got {other:?}"),
        }

        // The rejected registration left nothing behind.
        assert_eq!(registry.len(), 1);
        assert!(!registry.contains("position_again"));
        assert_eq!(registry.name_of::<Position>(), Some("position"));
    }

    #[test]
    fn two_types_cannot_share_one_stable_name() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("thing")
            .expect("the first registration of a type always succeeds");

        let error = registry
            .register_component::<Velocity>("thing")
            .expect_err("a stable name belongs to exactly one type");

        match error {
            EcsError::ComponentNameTaken {
                name,
                registered,
                rejected,
            } => {
                assert_eq!(name, "thing");
                assert!(registered.ends_with("Position"));
                assert!(rejected.ends_with("Velocity"));
            }
            other => panic!("expected a name collision error, got {other:?}"),
        }

        // The name still belongs to the type that claimed it first.
        assert_eq!(registry.len(), 1);
        assert_eq!(
            registry.get("thing").expect("still registered").type_id(),
            TypeId::of::<Position>()
        );
        assert_eq!(registry.name_of::<Velocity>(), None);
    }

    #[test]
    fn iteration_is_sorted_by_stable_name_not_by_registration_order() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Velocity>("velocity")
            .expect("first");
        registry
            .register_component::<Health>("health")
            .expect("first");
        registry
            .register_component::<Position>("position")
            .expect("first");

        let names: Vec<_> = registry.iter().map(|info| info.name()).collect();
        assert_eq!(names, vec!["health", "position", "velocity"]);
        assert_eq!(registry.iter().len(), 3);
    }

    #[test]
    fn debug_output_carries_nothing_that_changes_between_runs() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Velocity>("velocity")
            .expect("first");
        registry
            .register_component::<Position>("position")
            .expect("first");

        // Function pointer addresses and TypeId hashes are the two things in
        // here that differ per process and per compiler version. Asserting the
        // exact strings is what keeps them out.
        assert_eq!(
            format!("{registry:?}"),
            r#"ComponentRegistry { components: ["position", "velocity"] }"#
        );

        let info = registry.get("position").expect("just registered");
        let rendered = format!("{info:?}");
        assert!(
            rendered.starts_with(r#"ComponentInfo { name: "position", type_name: "#),
            "unexpected ComponentInfo debug output: {rendered}"
        );
        assert!(
            !rendered.contains("0x"),
            "the serialize function pointer leaked into the debug output: {rendered}"
        );
    }

    #[test]
    fn the_type_erased_path_writes_what_serde_writes() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let entity = world.spawn();
        let position = Position { x: 3, y: -1 };
        let direct = ron::to_string(&position).expect("a Position always serializes");
        world.insert(entity, position).expect("the entity is alive");

        let erased = registry
            .serialize_component("position", &world, entity)
            .expect("the entity is alive and the component is registered");

        assert_eq!(erased.as_deref(), Some(direct.as_str()));
        assert_eq!(direct, "(x:3,y:-1)");
    }

    /// The erased pair is an inverse, checked through the registry both ways.
    ///
    /// This is what M4.1 needed and what the `DeserializeOwned` bound was
    /// reserved for since M2.1: a component the caller cannot name goes to text
    /// and comes back unchanged. Compared through the text rather than through
    /// `PartialEq`, because the text is what a scene file and a state dump
    /// hold: an equality that agreed while the two renderings differed would be
    /// agreement about the wrong thing.
    #[test]
    fn the_erased_path_reads_back_what_the_erased_path_wrote() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let written = world.spawn();
        world
            .insert(written, Position { x: 3, y: -1 })
            .expect("the entity is alive");
        let text = registry
            .serialize_component("position", &world, written)
            .expect("alive and registered")
            .expect("the entity carries one");

        let read_back = world.spawn();
        registry
            .deserialize_component("position", &mut world, read_back, &text)
            .expect("what the registry wrote, the registry reads");

        assert_eq!(
            *world.get::<Position>(read_back).expect("just inserted"),
            Position { x: 3, y: -1 }
        );
        assert_eq!(
            registry
                .serialize_component("position", &world, read_back)
                .expect("alive and registered")
                .as_deref(),
            Some(text.as_str())
        );
    }

    /// Reading onto an entity that already carries one replaces it.
    ///
    /// The same rule `World::insert` has, stated here because a loader's whole
    /// job is putting components on entities and "what happens to the one that
    /// was there" is the first question it raises.
    #[test]
    fn deserializing_replaces_a_component_that_is_already_there() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Position { x: 1, y: 1 })
            .expect("alive");

        registry
            .deserialize_component("position", &mut world, entity, "(x:9,y:8)")
            .expect("alive and registered");

        assert_eq!(
            *world.get::<Position>(entity).expect("just inserted"),
            Position { x: 9, y: 8 }
        );
    }

    /// Text that is not this component names the component and the entity.
    ///
    /// And keeps the deserializer's own error as a source, because that is what
    /// carries the position inside the text; flattening it into a sentence would
    /// throw away the only part that says *where*.
    #[test]
    fn deserializing_malformed_text_names_the_component_and_the_entity() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let entity = world.spawn();

        let error = registry
            .deserialize_component("position", &mut world, entity, "(x:1,y:)")
            .expect_err("`y:` has no value");

        match &error {
            EcsError::ComponentDeserialization {
                component,
                entity: reported,
                source,
            } => {
                assert!(component.ends_with("Position"), "got {component}");
                assert_eq!(*reported, entity);
                // The position inside the text survives into the message.
                assert!(
                    source.to_string().contains("1:"),
                    "the deserializer's own error should carry a position: {source}"
                );
            }
            other => panic!("expected a deserialization error, got {other:?}"),
        }

        assert!(
            !world.has::<Position>(entity),
            "a failed read must leave nothing behind"
        );
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn deserializing_for_a_dead_entity_is_an_error_not_an_insert() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let entity = world.spawn();
        world.despawn(entity).expect("the entity is alive");

        assert!(matches!(
            registry.deserialize_component("position", &mut world, entity, "(x:1,y:2)"),
            Err(EcsError::NoSuchEntity { .. })
        ));
    }

    #[test]
    fn deserializing_an_unregistered_name_names_the_name() {
        let registry = ComponentRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();

        match registry.deserialize_component("position", &mut world, entity, "(x:1,y:2)") {
            Err(EcsError::UnknownComponent { name }) => assert_eq!(name, "position"),
            other => panic!("expected an unknown component error, got {other:?}"),
        }
    }

    #[test]
    fn a_component_the_entity_does_not_have_serializes_to_nothing() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let entity = world.spawn();

        assert_eq!(
            registry
                .serialize_component("position", &world, entity)
                .expect("the entity is alive"),
            None
        );
    }

    #[test]
    fn serializing_for_a_dead_entity_is_an_error_not_an_absence() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let entity = world.spawn();
        world
            .insert(entity, Position { x: 0, y: 0 })
            .expect("the entity is alive");
        world.despawn(entity).expect("the entity is alive");

        assert!(matches!(
            registry.serialize_component("position", &world, entity),
            Err(EcsError::NoSuchEntity { .. })
        ));
    }

    #[test]
    fn serializing_an_unregistered_name_names_the_name() {
        let registry = ComponentRegistry::new();
        let mut world = World::new();
        let entity = world.spawn();

        match registry.serialize_component("position", &world, entity) {
            Err(EcsError::UnknownComponent { name }) => assert_eq!(name, "position"),
            other => panic!("expected an unknown component error, got {other:?}"),
        }
    }

    #[test]
    fn a_full_dump_walks_entities_and_the_registry_in_canonical_order() {
        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Velocity>("velocity")
            .expect("first");
        registry
            .register_component::<Position>("position")
            .expect("first");

        let mut world = World::new();
        let moving = world.spawn();
        world
            .insert(moving, Position { x: 1, y: 2 })
            .expect("alive");
        world
            .insert(moving, Velocity { dx: 3, dy: 4 })
            .expect("alive");
        let still = world.spawn();
        world.insert(still, Position { x: 5, y: 6 }).expect("alive");

        let mut dump = Vec::new();
        for entity in world.entity_ids() {
            for info in registry.iter() {
                let serialized = info
                    .serialize(&world, entity)
                    .expect("every entity in the list is alive");
                dump.push((entity, info.name(), serialized));
            }
        }

        assert_eq!(
            dump,
            vec![
                (moving, "position", Some("(x:1,y:2)".to_owned())),
                (moving, "velocity", Some("(dx:3,dy:4)".to_owned())),
                (still, "position", Some("(x:5,y:6)".to_owned())),
                (still, "velocity", None),
            ]
        );
    }
}
