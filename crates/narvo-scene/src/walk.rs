//! The one traversal of a scene, and the two policies that use it.
//!
//! [`from_str`](crate::from_str) stops at the first error, which is right for a
//! loader: there is no world to hand back, so carrying on costs work nobody can
//! use. [`validate`](crate::validate) collects everything, which is right for
//! feedback: an author with four mistakes wants four messages, not four runs.
//!
//! **The checks are written once.** Both entry points call [`walk`] and differ
//! only in what their [`Sink`] does when something is reported — one says stop
//! and keeps the first error, the other says carry on and keeps them all.
//!
//! *Rejected: always collect, and let `from_str` take the first.* It needs no
//! [`Flow`] plumbing and is a smaller diff. Against it: `from_str` would pay for
//! findings nobody reads on every load, and — the deciding half — a check would
//! then run downstream of one that had already failed, against state that
//! failure invalidated. Stopping is not an optimisation here; it is what keeps
//! the loader's behaviour exactly what it was before this module existed.

use std::collections::BTreeMap;

use narvo_ecs::{ComponentRegistry, EntityId, World};
use ron::value::RawValue;

use crate::error::SceneError;
use crate::file::{Prefab, Scene};
use crate::finding::Finding;
use crate::location::{self, Location};
use crate::prefab::{self, Origin};
use crate::splice;

/// The opening of `EntityId`'s serialized form, whitespace removed.
///
/// Not a guess: `the_handle_needles_are_what_an_entity_id_serializes_to` pins
/// both needles against `ron::to_string` of a real handle, so a change to
/// `EntityId`'s serde form turns that test red rather than turning the warning
/// below silently off.
const HANDLE_OPENS: &str = "(index:";

/// What joins that form's two fields, whitespace removed.
const HANDLE_JOINS: &str = ",generation:";

/// Whether the traversal should carry on after a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// Keep walking; more findings are wanted.
    Continue,
    /// Stop here; the caller has what it came for.
    Stop,
}

/// What the traversal reports to.
pub(crate) trait Sink {
    /// Reports an error, and says whether the walk goes on.
    fn error(&mut self, error: SceneError) -> Flow;

    /// Reports a warning. A warning never stops a walk.
    fn warning(&mut self, finding: Finding);

    /// Whether warnings are wanted at all.
    ///
    /// The load path does not want them, and asking first means it does not pay
    /// for a check whose result it would throw away.
    fn wants_warnings(&self) -> bool;
}

/// The loader's policy: keep the first error, stop there.
#[derive(Debug, Default)]
pub(crate) struct FailFast {
    /// The first error reported, which is the one the loader returns.
    pub(crate) first: Option<SceneError>,
}

impl Sink for FailFast {
    fn error(&mut self, error: SceneError) -> Flow {
        self.first.get_or_insert(error);
        Flow::Stop
    }

    fn warning(&mut self, _finding: Finding) {}

    fn wants_warnings(&self) -> bool {
        false
    }
}

/// The validator's policy: keep everything, carry on.
#[derive(Debug, Default)]
pub(crate) struct Collect {
    /// Everything reported, in the order the walk found it.
    pub(crate) findings: Vec<Finding>,
}

impl Sink for Collect {
    fn error(&mut self, error: SceneError) -> Flow {
        self.findings.push(Finding::from_error(&error));
        Flow::Continue
    }

    fn warning(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    fn wants_warnings(&self) -> bool {
        true
    }
}

/// Walks a scene, reporting everything wrong with it to `sink`.
///
/// Returns the world it built — which the loader keeps when nothing was
/// reported and the validator discards. The `Err` case is the syntax stop
/// alone, because a file that is not RON has no entity list to walk; everything
/// else goes to the sink, because everything else is something a walk can carry
/// on past.
pub(crate) fn walk(
    text: &str,
    registry: &ComponentRegistry,
    sink: &mut dyn Sink,
) -> Result<World, SceneError> {
    let scene: Scene<'_> = ron::from_str(text).map_err(|error| SceneError::Syntax {
        location: Location::new(error.span.start.line, error.span.start.col),
        message: error.code.to_string(),
    })?;

    let mut world = World::new();

    // Pass one: every entity exists before any component is read, which is what
    // makes a forward reference legal.
    let mut ids = Vec::with_capacity(scene.entities.len());
    for _ in &scene.entities {
        ids.push(world.spawn());
    }

    let names = match bind_names(text, &scene, &ids, sink) {
        Bound::Stopped => return Ok(world),
        Bound::Names(names) => names,
    };

    let prefabs = match collect_prefabs(text, &scene, sink) {
        Prefabs::Stopped => return Ok(world),
        Prefabs::Ready(prefabs) => prefabs,
    };

    // Pass three: the components, with every reference already resolvable and
    // every template expanded.
    for (index, entity) in scene.entities.iter().enumerate() {
        let id = ids[index];

        // Expansion first, so that what follows walks one list of components
        // whether they came from a template, from the entity, or from both.
        // Everything downstream of here is exactly what it was before prefabs
        // existed, which is the shape ADR-0021's "load-time transformation"
        // means in practice.
        let mut problems = Vec::new();
        let expansion = prefab::expand(text, index, entity, &prefabs, &mut problems);
        for problem in problems {
            if sink.error(problem) == Flow::Stop {
                return Ok(world);
            }
        }

        let mut seen: Vec<&str> = Vec::with_capacity(expansion.components.len());

        for component in &expansion.components {
            let name = component.name;
            let location = prefab::body_location(text, component);

            if seen.contains(&name) {
                if sink.error(SceneError::DuplicateComponent {
                    location,
                    index,
                    component: name.to_owned(),
                }) == Flow::Stop
                {
                    return Ok(world);
                }
                continue;
            }
            seen.push(name);

            if !registry.contains(name) {
                if sink.error(SceneError::UnknownComponent {
                    location,
                    index,
                    entity_name: names.name_of(index),
                    component: name.to_owned(),
                    known: registry.iter().map(|info| info.name().to_owned()).collect(),
                }) == Flow::Stop
                {
                    return Ok(world);
                }
                continue;
            }

            if sink.wants_warnings() {
                warn_about_positional_handles(
                    location,
                    index,
                    name,
                    component.body.get_ron(),
                    sink,
                );
            }

            let Some(values) = resolve_all(text, index, component, &names, sink)? else {
                if sink.wants_warnings() {
                    continue;
                }
                return Ok(world);
            };
            let Resolved::Fields(values) = values else {
                continue;
            };

            let body_text = if values.is_empty() {
                component.body.get_ron().trim().to_owned()
            } else {
                match splice(location, index, name, component.body.get_ron(), &values) {
                    Ok(text) => text,
                    Err(error) => {
                        if sink.error(error) == Flow::Stop {
                            return Ok(world);
                        }
                        continue;
                    }
                }
            };

            if let Err(source) = registry.deserialize_component(name, &mut world, id, &body_text) {
                let error = explain(
                    registry, location, index, component, &values, &expansion, source,
                );
                if sink.error(error) == Flow::Stop {
                    return Ok(world);
                }
            }
        }
    }

    Ok(world)
}

/// The names of a scene, and the handles they stand for.
pub(crate) struct Names {
    /// Name per entity index, for messages that quote it.
    per_entity: Vec<Option<String>>,
    /// Name to handle and to the index that claimed it first.
    by_name: BTreeMap<String, (EntityId, usize)>,
}

impl Names {
    /// The name of the entity at `index`, when it has one.
    fn name_of(&self, index: usize) -> Option<String> {
        self.per_entity.get(index).cloned().flatten()
    }

    /// Every name the scene defines, in alphabetical order.
    fn known(&self) -> Box<[String]> {
        self.by_name.keys().cloned().collect()
    }
}

/// Either the bound names, or the signal that the sink asked to stop.
enum Bound {
    Names(Names),
    Stopped,
}

/// Pass two: reads every `name`, binds it, and reports what cannot be bound.
///
/// A pass of its own because a reference in pass three may point at any entity,
/// including one declared later — so every name has to exist before the first
/// component is read.
fn bind_names(text: &str, scene: &Scene<'_>, ids: &[EntityId], sink: &mut dyn Sink) -> Bound {
    let mut per_entity: Vec<Option<String>> = vec![None; scene.entities.len()];
    let mut by_name: BTreeMap<String, (EntityId, usize)> = BTreeMap::new();

    for (index, entity) in scene.entities.iter().enumerate() {
        let Some(raw) = entity.name.0 else {
            continue;
        };
        let location = location::locate(text, raw.get_ron());

        let name: String = match ron::from_str(raw.get_ron()) {
            Ok(name) => name,
            Err(error) => {
                if sink.error(SceneError::InvalidEntityName {
                    location,
                    index,
                    written: raw.get_ron().trim().to_owned(),
                    message: error.code.to_string(),
                }) == Flow::Stop
                {
                    return Bound::Stopped;
                }
                continue;
            }
        };

        if name.is_empty() {
            // ADR-0018: an empty name is anonymous, and that has not changed.
            // Writing one is legal and almost certainly not what was meant.
            if sink.wants_warnings() {
                sink.warning(Finding::warning(
                    location,
                    format!(
                        "entity {index} is named with the empty string, which means anonymous - \
                         nothing can refer to it. Omit the `name` field for an anonymous entity, \
                         or give it a name something can point at"
                    ),
                ));
            }
            continue;
        }

        // Recorded before the duplicate check, and only for messages: a later
        // finding about this entity should quote the name its author wrote, even
        // when that name belongs to somebody else. What the repeat does *not*
        // get is the binding below.
        per_entity[index] = Some(name.clone());

        if let Some((_, first)) = by_name.get(&name) {
            if sink.error(SceneError::DuplicateEntityName {
                location,
                name,
                first: *first,
                second: index,
            }) == Flow::Stop
            {
                return Bound::Stopped;
            }
            // The first claimant keeps the binding, so a reference to the name
            // resolves to the entity that asked for it first.
            continue;
        }

        by_name.insert(name, (ids[index], index));
    }

    Bound::Names(Names {
        per_entity,
        by_name,
    })
}

/// The prefab table, or the signal that the sink asked to stop.
enum Prefabs<'a> {
    Ready(BTreeMap<String, (Prefab<'a>, usize)>),
    Stopped,
}

/// Reads the `prefabs` block, refusing a name used twice.
///
/// The index travels with each template so a duplicate can say which entry came
/// first — the same shape the duplicate entity name uses.
fn collect_prefabs<'a>(text: &str, scene: &Scene<'a>, sink: &mut dyn Sink) -> Prefabs<'a>
where
    Prefab<'a>: Clone,
{
    let mut table: BTreeMap<String, (Prefab<'a>, usize)> = BTreeMap::new();

    for (index, (name, prefab)) in scene.prefabs.0.iter().enumerate() {
        if let Some((_, first)) = table.get(name) {
            let location = prefab
                .components
                .0
                .first()
                .and_then(|(_, body)| location::locate(text, body.get_ron()));
            if sink.error(SceneError::DuplicatePrefabName {
                location,
                name: name.clone(),
                first: *first,
                second: index,
            }) == Flow::Stop
            {
                return Prefabs::Stopped;
            }
            // The first definition keeps the name, as with entity names.
            continue;
        }
        table.insert(name.clone(), (prefab.clone(), index));
    }

    Prefabs::Ready(table)
}

/// What resolving a component's spliced fields came to.
enum Resolved {
    /// The field values, ready to splice.
    Fields(BTreeMap<String, String>),
    /// Something did not resolve; the component is skipped.
    Skip,
}

/// Resolves every field a component splices, references and fills alike.
///
/// A reference becomes the handle it names, rendered by `ron::to_string` so this
/// crate never spells a field of `EntityId`. A fill is the author's own bytes and
/// travels verbatim, which is what keeps ADR-0018's rule intact through a
/// template.
fn resolve_all(
    text: &str,
    index: usize,
    component: &prefab::Component<'_>,
    names: &Names,
    sink: &mut dyn Sink,
) -> Result<Option<Resolved>, SceneError> {
    let mut values = BTreeMap::new();
    let mut failed = false;

    for spliced in &component.spliced {
        if spliced.is_reference {
            match resolve(
                text,
                index,
                component.name,
                spliced.field,
                spliced.value,
                names,
            ) {
                Ok(id) => {
                    let handle =
                        ron::to_string(&id).map_err(|source| SceneError::HandleSerialization {
                            index,
                            component: component.name.to_owned(),
                            field: spliced.field.to_owned(),
                            source: Box::new(source),
                        })?;
                    values.insert(spliced.field.to_owned(), handle);
                }
                Err(error) => {
                    failed = true;
                    if sink.error(error) == Flow::Stop {
                        return Ok(None);
                    }
                }
            }
        } else {
            values.insert(
                spliced.field.to_owned(),
                spliced.value.get_ron().trim().to_owned(),
            );
        }
    }

    if failed {
        return Ok(Some(Resolved::Skip));
    }

    Ok(Some(Resolved::Fields(values)))
}

/// Turns a component read failure into the most specific thing it can be.
///
/// # Why a collision is found by retrying rather than by reading the message
///
/// When a template sets a field and an instance fills it, the assembled body
/// carries that field twice and the component's own deserializer refuses it —
/// which is exactly the mechanism M4.5 chose, because it needs no lexer and
/// cannot disagree with the component's own idea of its fields. What that
/// mechanism does *not* say is that a prefab was involved.
///
/// So the cause is established structurally: read the body again **without** the
/// filled fields. If that succeeds, the fills are what broke it and the field
/// that collided is found by adding them back one at a time. If it fails too,
/// the body was already wrong — an unfilled hole, or a value of the wrong type —
/// and the ordinary error is the honest one.
///
/// The probes run on a scratch world, only on the failure path, and their cost
/// is one parse per filled field of one component.
fn explain(
    registry: &ComponentRegistry,
    location: Option<Location>,
    index: usize,
    component: &prefab::Component<'_>,
    values: &BTreeMap<String, String>,
    expansion: &prefab::Expansion<'_>,
    source: narvo_ecs::EcsError,
) -> SceneError {
    let ordinary = |source| SceneError::Component {
        location,
        index,
        component: component.name.to_owned(),
        source,
    };

    let Some(prefab) = expansion.prefab else {
        return ordinary(source);
    };
    let filled: Vec<&String> = values
        .keys()
        .filter(|field| {
            component
                .spliced
                .iter()
                .any(|spliced| !spliced.is_reference && spliced.field == field.as_str())
        })
        .collect();
    if filled.is_empty() || component.origin != Origin::Template {
        return ordinary(source);
    }

    let mut probe = World::new();
    let entity = probe.spawn();
    let mut attempt = |subset: &BTreeMap<String, String>| -> bool {
        let Ok(body) = splice(
            location,
            index,
            component.name,
            component.body.get_ron(),
            subset,
        ) else {
            return false;
        };
        registry
            .deserialize_component(component.name, &mut probe, entity, &body)
            .is_ok()
    };

    // Without the fills. If that is broken too, the fills are not the cause.
    let without: BTreeMap<String, String> = values
        .iter()
        .filter(|(field, _)| !filled.contains(field))
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect();
    if !attempt(&without) {
        return ordinary(source);
    }

    // One fill at a time, to name the field rather than the set.
    for field in &filled {
        let mut one = without.clone();
        if let Some(value) = values.get(*field) {
            one.insert((*field).clone(), value.clone());
        }
        if !attempt(&one) {
            return SceneError::FieldCollision {
                location,
                index,
                prefab: prefab.to_owned(),
                component: component.name.to_owned(),
                field: (*field).clone(),
            };
        }
    }

    ordinary(source)
}

/// Turns one reference into a handle, or says where the name that had no answer
/// was written.
fn resolve(
    text: &str,
    index: usize,
    component: &str,
    field: &str,
    raw: &RawValue,
    names: &Names,
) -> Result<EntityId, SceneError> {
    let location = location::locate(text, raw.get_ron());
    let unknown = |name: String| SceneError::UnknownReference {
        location,
        index,
        component: component.to_owned(),
        field: field.to_owned(),
        name,
        known: names.known(),
    };

    // A reference that is not a string names nothing, which is the same mistake
    // from the author's side as a string naming nothing - so it is one message
    // rather than two variants a reader would have to tell apart.
    let name: String =
        ron::from_str(raw.get_ron()).map_err(|_| unknown(raw.get_ron().trim().to_owned()))?;

    names
        .by_name
        .get(&name)
        .map(|(id, _)| *id)
        .ok_or_else(|| unknown(name))
}

/// Raises the F4 warning when a body carries a handle written out positionally.
///
/// # How the shape is recognised, and what that costs
///
/// By looking for the two field names `EntityId` serializes into, in the body
/// with its whitespace removed. It is a **substring scan on purpose**: finding a
/// value properly inside a component body would need the RON lexer ADR-0018
/// rejected, and here the cost of being wrong is a warning rather than a
/// corrupted body. What it can do is fire on a string literal containing both
/// words — noise a reader dismisses in a second, in exchange for no lexer. What
/// it cannot do is drift: the needles are pinned to `EntityId`'s actual serde
/// form by a test.
fn warn_about_positional_handles(
    location: Option<Location>,
    index: usize,
    component: &str,
    body: &str,
    sink: &mut dyn Sink,
) {
    let squeezed: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    if !squeezed.contains(HANDLE_OPENS) || !squeezed.contains(HANDLE_JOINS) {
        return;
    }

    sink.warning(Finding::warning(
        location,
        format!(
            "entity {index} writes a positional entity handle into its \"{component}\", which is \
             what a machine-written scene looks like. Editing one breaks references: inserting an \
             entity above this one silently repoints the handle at something else. For a scene \
             kept by hand, give the target a name and point at it through the refs table"
        ),
    ));
}

#[cfg(test)]
mod tests {
    use super::{HANDLE_JOINS, HANDLE_OPENS};
    use narvo_ecs::World;

    /// The two needles are what a handle actually serializes to.
    ///
    /// The warning in this module recognises a positional handle by looking for
    /// these two strings. Spelling them out is only safe while they are what
    /// `EntityId` writes, and that is a fact about a type in another crate whose
    /// serde form `entity.rs` pins as stable API. If it ever moves, this fails —
    /// which is the point. Without it the warning would simply stop firing, and
    /// a silent lint is worse than none.
    #[test]
    fn the_handle_needles_are_what_an_entity_id_serializes_to() {
        let mut world = World::new();
        let entity = world.spawn();

        let text = ron::to_string(&entity).expect("two integers serialize");
        let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();

        assert!(
            squeezed.contains(HANDLE_OPENS),
            "a handle no longer opens with {HANDLE_OPENS:?}: {text}"
        );
        assert!(
            squeezed.contains(HANDLE_JOINS),
            "a handle no longer joins its fields with {HANDLE_JOINS:?}: {text}"
        );
    }
}
