//! Prefab expansion: turning an instance plus a template into the components an
//! entity actually gets.
//!
//! Read ADR-0021. The decision this module implements in one sentence:
//! **expansion is a load-time transformation and nothing else** — a template is
//! scene syntax like a name, and a world built from a prefab scene is
//! indistinguishable from a world built from the flat scene an author could have
//! written instead. `a_prefab_scene_and_its_flat_twin_load_to_one_world` is that
//! sentence as a test, and the round-trip property makes it a statement about
//! generated combinations rather than about one example.
//!
//! # Four operations, and why omission is not one of them
//!
//! An instance may **fill** a hole the template left, **replace** a whole
//! component, **add** one the template does not have, and **remove** one it
//! does. It may not remove by saying nothing: omission means inheritance, which
//! is what a template is for. ADR-0021 argues it; the shape of this module is
//! that argument in code, because there is no branch anywhere below in which a
//! component silently disappears.

use std::collections::BTreeMap;

use ron::value::RawValue;

use crate::error::SceneError;
use crate::file::{Prefab, RawPairs, SceneEntity};
use crate::location::{self, Location};

/// Where a component's body came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// Written on the entity itself — added, or replacing a template's.
    Instance,
    /// Taken from the template.
    Template,
}

/// A field an instance splices into a body, and where its value came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Spliced<'a> {
    /// The field's name.
    pub(crate) field: &'a str,
    /// Its value, verbatim.
    pub(crate) value: &'a RawValue,
    /// Whether it names an entity (a `refs` entry) or carries RON (a `fill`).
    pub(crate) is_reference: bool,
}

/// One component of an entity, after expansion.
#[derive(Debug)]
pub(crate) struct Component<'a> {
    /// The stable registry name.
    pub(crate) name: &'a str,
    /// The body to read, before anything is spliced into it.
    pub(crate) body: &'a RawValue,
    /// Where that body came from.
    pub(crate) origin: Origin,
    /// What to splice into it, in field order.
    pub(crate) spliced: Vec<Spliced<'a>>,
}

/// What an entity expanded to.
#[derive(Debug)]
pub(crate) struct Expansion<'a> {
    /// The components, template ones first and in template order.
    pub(crate) components: Vec<Component<'a>>,
    /// The template this came from, when there was one. For messages.
    pub(crate) prefab: Option<&'a str>,
}

/// Expands one entity against the scene's templates.
///
/// Every problem found is appended to `problems` rather than returned, because
/// the caller is a walk that may want all of them (M4.2's collecting validator)
/// or only the first (the loader). What comes back is the best expansion that
/// could still be made, so a scene with one bad `without` still reports what is
/// wrong with the rest of its components.
pub(crate) fn expand<'a>(
    text: &str,
    index: usize,
    entity: &'a SceneEntity<'a>,
    prefabs: &'a BTreeMap<String, (Prefab<'a>, usize)>,
    problems: &mut Vec<SceneError>,
) -> Expansion<'a> {
    let template = match entity.from.0 {
        None => None,
        Some(raw) => {
            let location = location::locate(text, raw.get_ron());
            match ron::from_str::<String>(raw.get_ron()) {
                Err(_) => {
                    problems.push(SceneError::UnknownPrefab {
                        location,
                        index,
                        name: raw.get_ron().trim().to_owned(),
                        known: prefabs.keys().cloned().collect(),
                    });
                    None
                }
                Ok(name) => match prefabs.get(&name) {
                    Some((prefab, _)) => Some((name, prefab)),
                    None => {
                        problems.push(SceneError::UnknownPrefab {
                            location,
                            index,
                            name,
                            known: prefabs.keys().cloned().collect(),
                        });
                        None
                    }
                },
            }
        }
    };

    // `fill` and `without` only mean something against a template. Saying either
    // without `from` is a mistake with no sensible reading, so it is refused
    // rather than ignored.
    if template.is_none() && entity.from.0.is_none() {
        for component in entity.fill.keys() {
            problems.push(SceneError::PrefabOnlyField {
                index,
                field: "fill",
                component: component.clone(),
            });
        }
        for raw in &entity.without {
            problems.push(SceneError::PrefabOnlyField {
                index,
                field: "without",
                component: ron::from_str::<String>(raw.get_ron())
                    .unwrap_or_else(|_| raw.get_ron().trim().to_owned()),
            });
        }
    }

    let mut components: Vec<Component<'a>> = Vec::new();

    if let Some((_, prefab)) = template {
        let removed = removals(text, index, entity, prefab, problems);

        for (name, body) in &prefab.components.0 {
            if removed.contains(&name.as_str()) {
                continue;
            }
            // An instance body for the same component replaces this one
            // wholesale; the template's own body is then not spliced into at
            // all, which is what "replace" means.
            if entity
                .components
                .0
                .iter()
                .any(|(instance, _)| instance == name)
            {
                continue;
            }

            components.push(Component {
                name,
                body,
                origin: Origin::Template,
                spliced: template_refs(prefab, name),
            });
        }
    }

    for (name, body) in &entity.components.0 {
        components.push(Component {
            name,
            body,
            origin: Origin::Instance,
            spliced: Vec::new(),
        });
    }

    attach(text, index, entity, &mut components, problems);

    Expansion {
        components,
        prefab: template.map(|(name, _)| {
            // The borrow has to outlive the expansion, and the map's key does.
            prefabs
                .get_key_value(&name)
                .map_or("", |(key, _)| key.as_str())
        }),
    }
}

/// The components the instance removes, with a finding for each that is not
/// there to remove.
fn removals<'a>(
    text: &str,
    index: usize,
    entity: &'a SceneEntity<'a>,
    prefab: &'a Prefab<'a>,
    problems: &mut Vec<SceneError>,
) -> Vec<&'a str> {
    let mut removed = Vec::new();

    for raw in &entity.without {
        let location = location::locate(text, raw.get_ron());
        let Ok(name) = ron::from_str::<String>(raw.get_ron()) else {
            problems.push(SceneError::RemovingAbsentComponent {
                location,
                index,
                component: raw.get_ron().trim().to_owned(),
                present: prefab
                    .components
                    .0
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect(),
            });
            continue;
        };

        match prefab
            .components
            .0
            .iter()
            .find(|(component, _)| *component == name)
        {
            Some((component, _)) => removed.push(component.as_str()),
            None => problems.push(SceneError::RemovingAbsentComponent {
                location,
                index,
                component: name,
                present: prefab
                    .components
                    .0
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect(),
            }),
        }
    }

    removed
}

/// The template's own references for one component.
fn template_refs<'a>(prefab: &'a Prefab<'a>, component: &str) -> Vec<Spliced<'a>> {
    prefab
        .refs
        .get(component)
        .map(|fields| entries(fields, true))
        .unwrap_or_default()
}

/// Turns a `RawPairs` into spliced entries.
fn entries<'a>(fields: &'a RawPairs<'a>, is_reference: bool) -> Vec<Spliced<'a>> {
    fields
        .0
        .iter()
        .map(|(field, value)| Spliced {
            field: field.as_str(),
            value,
            is_reference,
        })
        .collect()
}

/// Attaches the instance's own `refs` and `fill` to the components they name.
///
/// A `refs` or `fill` entry for a component the entity does not carry — neither
/// from the template nor of its own — is a finding rather than a no-op, for the
/// reason M4.1 gave for `refs`: it would otherwise do nothing at all, silently.
fn attach<'a>(
    text: &str,
    index: usize,
    entity: &'a SceneEntity<'a>,
    components: &mut [Component<'a>],
    problems: &mut Vec<SceneError>,
) {
    for (source, is_reference) in [(&entity.refs, true), (&entity.fill, false)] {
        for (component, fields) in source {
            match components
                .iter_mut()
                .find(|effective| effective.name == component)
            {
                Some(effective) => effective.spliced.extend(entries(fields, is_reference)),
                None if is_reference => {
                    let location = fields
                        .0
                        .first()
                        .and_then(|(_, raw)| location::locate(text, raw.get_ron()));
                    problems.push(SceneError::ReferenceForAbsentComponent {
                        location,
                        index,
                        component: component.clone(),
                    });
                }
                None => {
                    let location = fields
                        .0
                        .first()
                        .and_then(|(_, raw)| location::locate(text, raw.get_ron()));
                    problems.push(SceneError::FillForAbsentComponent {
                        location,
                        index,
                        component: component.clone(),
                    });
                }
            }
        }
    }
}

/// Where a component's body sits, for a message.
pub(crate) fn body_location(text: &str, component: &Component<'_>) -> Option<Location> {
    location::locate(text, component.body.get_ron())
}
