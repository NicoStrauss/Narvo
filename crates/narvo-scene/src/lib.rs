//! Two RON formats over a [`World`]: the scene an author writes, and the save a
//! run produces.
//!
//! Responsibility: turning text into a world and a world back into text. What a
//! world *is* belongs to `narvo-ecs`; what a component's text looks like
//! belongs to that component's `serde` implementation and to the registry's
//! type-erased path (ADR-0006). This crate owns exactly the layer between them —
//! the entity list, the spawn order, the names entities may refer to each other
//! by, and the errors an author gets back.
//!
//! **One mechanism, two contracts.** Both formats put a component's body through
//! the registry byte for byte, and neither has a second rendering of a component
//! to keep in step. What differs is everything above that: a scene is content and
//! grows additively (ADR-0018 Decision 6), a save is state and grows by version;
//! a scene names entities symbolically and reaches only the worlds a fresh world
//! can spawn into, a save names them by slot and generation and reaches every
//! world. The save lives in [`save`] and ADR-0043 records the split; this module
//! is the scene.
//!
//! Read ADR-0018 before changing the format. The three decisions it records and
//! this crate implements:
//!
//! 1. **File order is spawn order.** The *n*-th entity in the file is the *n*-th
//!    entity spawned, so it occupies slot *n* of a fresh world. Nothing in the
//!    file names a slot.
//! 2. **Components go through the registry, never through a list here.** The
//!    format is component-open: a new component type is a new registry entry and
//!    no line of this crate changes. That is the condition under which M8+'s 3D
//!    types are an extension rather than a rewrite (`ProjektPlan.md` §6/M4).
//! 3. **Entity references are symbolic.** An entity may carry a `name`, and
//!    another entity's `refs` points a component's field at it. Names are scene
//!    syntax: no component holds one, the world does not know them, and they do
//!    not survive [`to_string`].
//!
//! # Determinism
//!
//! Nothing here reads a clock or a source of entropy, and every order is fixed:
//! entities in file order, components within an entity in the registry's
//! stable-name order, references in field-name order. Two loads of one file
//! produce worlds with identical canonical dumps, which is the property
//! `the_example_scene_loads_into_the_reference_world` and the round-trip
//! property test both rest on.
//!
//! # The file
//!
//! ```ron
//! Scene(
//!     entities: [
//!         (
//!             name: "player",
//!             components: {
//!                 "transform": (x: 0.0, y: 0.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),
//!                 "layer": (depth: 1.0),
//!             },
//!         ),
//!         (
//!             components: {
//!                 "camera": (x: 0.0, y: 0.0, zoom: 1.0),
//!                 "follow": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false),
//!             },
//!             refs: { "follow": { "target": "player" } },
//!         ),
//!     ],
//! )
//! ```
//!
//! `name` is optional and `refs` is optional. A component's body is the
//! component's own RON — byte for byte what
//! [`ComponentInfo::serialize`](narvo_ecs::ComponentInfo::serialize) produces —
//! so there is one rendering of a component in this project and not two.
//!
//! # Examples
//!
//! ```
//! use narvo_ecs::{ComponentRegistry, Transform, canonical_dump};
//!
//! let mut registry = ComponentRegistry::new();
//! registry.register_component::<Transform>("transform")?;
//!
//! let world = narvo_scene::from_str(
//!     r#"Scene(entities: [(components: {
//!         "transform": (x: 1.5, y: -2.0, rotation: 0.0, scale_x: 1.0, scale_y: 1.0),
//!     })])"#,
//!     &registry,
//! )?;
//!
//! assert_eq!(world.len(), 1);
//!
//! // ... and back again. The strings need not match; the worlds must.
//! let written = narvo_scene::to_string(&world, &registry)?;
//! let reloaded = narvo_scene::from_str(&written, &registry)?;
//! assert_eq!(
//!     canonical_dump(&world, &registry)?,
//!     canonical_dump(&reloaded, &registry)?
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod error;
mod file;
mod finding;
mod location;
mod prefab;
pub mod save;
mod walk;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use narvo_ecs::{ComponentRegistry, World, canonical_dump};

pub use error::SceneError;
pub use finding::{Finding, Severity, ValidationReport};
pub use location::Location;

use walk::{Collect, FailFast, walk};

/// Indentation of one nesting level in written output.
const INDENT: &str = "    ";

/// Builds a [`World`] from the text of a scene file.
///
/// # Spawn order, and the identity it fixes
///
/// Entities are spawned in file order into a fresh world, so the *n*-th entity
/// of the file is the *n*-th spawn. This function does not *assume* what handle
/// that produces — it keeps the handles the world hands back and resolves names
/// against those. The assumption a reader would otherwise have to make (slot *n*,
/// generation 1) is asserted instead, by
/// `file_order_is_spawn_order_and_slots_ascend`.
///
/// # References
///
/// Every entity is spawned before any component is read, so a reference may
/// point forwards as freely as backwards. That is not a convenience: a camera
/// following a player and a player carrying the camera's handle cannot both be
/// satisfied in one pass, and a format that only allowed backward references
/// would make the author order the file around the loader's implementation.
///
/// # Errors
///
/// [`SceneError`] naming the entity and, for a syntax error, the line and
/// column. Every variant's message is asserted in this crate's tests.
pub fn from_str(text: &str, registry: &ComponentRegistry) -> Result<World, SceneError> {
    let mut sink = FailFast::default();
    let world = walk(text, registry, &mut sink)?;

    match sink.first {
        Some(error) => Err(error),
        None => Ok(world),
    }
}

/// Checks the text of a scene file and reports **everything** wrong with it.
///
/// # Why this exists beside [`from_str`]
///
/// [`from_str`] stops at the first error, which is right for a loader: there is
/// no world to hand back, so carrying on costs work nobody can use. It is wrong
/// for feedback. An author - a person or an agent - with four mistakes in a file
/// wants four messages, not four runs, because each run of a fail-fast loader
/// hides the next mistake behind the one it just reported.
///
/// So this walks the same checks and collects. It builds a world as it goes and
/// throws it away: a scene with errors has no world worth keeping, and the
/// findings are the product.
///
/// # One implementation, two policies
///
/// The checks are not written twice. This and [`from_str`] call the same private
/// traversal and differ only in what they do when it reports something. That is
/// what makes "the validator agrees with the loader" a property of the code
/// rather than of somebody's diligence, and
/// `the_validator_and_the_loader_agree_on_the_first_error` asserts it over every
/// broken scene in the test file.
///
/// # A syntax error is still a hard stop
///
/// Not by choice: a file that is not RON has no entity list to walk, so there is
/// nothing further to check and the report holds exactly one finding.
///
/// # Warnings
///
/// Two classes, both from traps measured in M4.1, and deliberately no more:
///
/// 1. **A positional handle in a component body** - what [`to_string`] writes.
///    A written scene is editing-fragile in exactly the way ADR-0018 rejected
///    index references for: insert an entity above it and the handle silently
///    points somewhere else.
/// 2. **A written empty name.** `name: ""` is anonymous by ADR-0018, and an
///    author who typed it almost certainly meant something else.
///
/// # Examples
///
/// ```
/// use narvo_ecs::ComponentRegistry;
/// use narvo_scene::Severity;
///
/// let registry = ComponentRegistry::new();
/// let report = narvo_scene::validate(
///     r#"Scene(entities: [(name: "", components: { "nope": (x: 1) })])"#,
///     &registry,
/// );
///
/// // One error for the unregistered component, one warning for the empty name.
/// assert_eq!((report.errors(), report.warnings()), (1, 1));
/// assert!(!report.loads());
/// assert_eq!(report.findings()[0].severity(), Severity::Warning);
/// ```
#[must_use]
pub fn validate(text: &str, registry: &ComponentRegistry) -> ValidationReport {
    let mut sink = Collect::default();

    match walk(text, registry, &mut sink) {
        // A syntax error ends the walk before there is anything else to say.
        Err(syntax) => ValidationReport {
            findings: vec![Finding::from_error(&syntax)],
        },
        Ok(_world) => ValidationReport {
            findings: sink.findings,
        },
    }
}

/// Writes a [`World`] as the text of a scene file.
///
/// # The domain boundary, and why it is a hard error
///
/// A scene *constitutes* a fresh world, so the worlds it can describe are the
/// ones a fresh world can reach by spawning: slots `0..n` in order, every
/// generation 1. A world that has despawned something is outside that set — its
/// slots have gaps, or a recycled slot carries a higher generation — and no
/// scene file can produce it, because nothing in the format asks a world to
/// despawn.
///
/// That case is not unsupported by oversight: it is **the save side of the same
/// round trip**, and `ProjektPlan.md` §6/M4 puts scene and save on one
/// mechanism deliberately ("Szene und Save teilen den Kern"). A save records a
/// state that a simulation reached; a scene records a state an author wrote.
/// This function refuses the first rather than emitting a file that would load
/// back into a differently-shaped world — which would be the failure this
/// project keeps naming: a round trip that reports success while the two sides
/// differ.
///
/// Since M6b.6 the other side exists: [`save::to_string`] takes exactly the
/// worlds this one turns away, and `a_world_the_scene_writer_refuses_is_one_the_save_writer_takes`
/// holds the two halves against each other so the handover is a property of the
/// code rather than of two documents agreeing.
///
/// # Names do not come back
///
/// A name is scene syntax and a world does not carry one, so written output has
/// no `name` and no `refs`: an entity handle is written as the component's own
/// RON, which is what the registry produces for it. Loading that output gives an
/// identical world and a different file, which is why the round-trip property is
/// stated over canonical dumps and byte equality of the two texts is explicitly
/// not claimed.
///
/// # Errors
///
/// [`SceneError::NotSceneShaped`] for a world with despawn history, and
/// [`SceneError::Ecs`] if an entity carries a component the registry does not
/// know — the same refusal [`canonical_dump`] makes, and made by asking it, so
/// the two rules cannot drift apart.
pub fn to_string(world: &World, registry: &ComponentRegistry) -> Result<String, SceneError> {
    // Asked rather than re-implemented: `canonical_dump` already refuses a world
    // carrying an unregistered component, and a component outside the dump is a
    // component outside the state hash. Paying one extra serialization pass buys
    // the guarantee that this writer and that rule can never disagree.
    canonical_dump(world, registry)?;

    let entities = world.entity_ids();
    for (index, entity) in entities.iter().enumerate() {
        let expected_index = u32::try_from(index).unwrap_or(u32::MAX);
        if entity.index() != expected_index || entity.generation() != 1 {
            return Err(SceneError::NotSceneShaped {
                entity: *entity,
                expected_index,
            });
        }
    }

    let mut out = String::new();
    out.push_str("Scene(\n");
    let _ = writeln!(out, "{INDENT}entities: [");

    for entity in &entities {
        let _ = writeln!(out, "{INDENT}{INDENT}(");
        let _ = writeln!(out, "{INDENT}{INDENT}{INDENT}components: {{");
        for info in registry.iter() {
            if let Some(body) = info.serialize(world, *entity)? {
                let _ = writeln!(
                    out,
                    "{INDENT}{INDENT}{INDENT}{INDENT}\"{}\": {body},",
                    info.name()
                );
            }
        }
        let _ = writeln!(out, "{INDENT}{INDENT}{INDENT}}},");
        let _ = writeln!(out, "{INDENT}{INDENT}),");
    }

    let _ = writeln!(out, "{INDENT}],");
    out.push_str(")\n");

    Ok(out)
}

/// Reads a scene file from disk.
///
/// A thin shell around [`from_str`] and nothing more: the whole format lives in
/// the string functions, so a caller that has the text — a test, a hot reload
/// watching a buffer, an IPC request in M6 — never touches a path. `narvo-ecs`
/// stays free of file I/O either way, which `ProjektPlan.md` §5.1 requires of
/// it.
///
/// # Errors
///
/// [`SceneError::Io`] naming the path, plus everything [`from_str`] returns.
pub fn from_file(path: &Path, registry: &ComponentRegistry) -> Result<World, SceneError> {
    let text = std::fs::read_to_string(path).map_err(|source| SceneError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    from_str(&text, registry)
}

/// Writes a scene file to disk.
///
/// The counterpart of [`from_file`], and as thin.
///
/// # Errors
///
/// [`SceneError::Io`] naming the path, plus everything [`to_string`] returns.
pub fn to_file(path: &Path, world: &World, registry: &ComponentRegistry) -> Result<(), SceneError> {
    let text = to_string(world, registry)?;

    std::fs::write(path, text).map_err(|source| SceneError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Puts the resolved fields into the component's body as extra named fields.
///
/// # Why the body is cut open rather than parsed
///
/// The body has to reach the registry **verbatim** apart from the fields added
/// here, because every number in it is a number whose exact spelling the state
/// hash is taken over. Anything that read the body into a value tree and wrote
/// it back would be choosing those spellings itself; ADR-0018 records the
/// measurement that closed that route off for good.
///
/// So the body is opened at its first `(` and closed at its last `)`, and the
/// new fields go in at the front. Everything between them — numbers, strings,
/// comments, nesting, whitespace — is copied through untouched, and the author's
/// own bytes are what the component's `Deserialize` sees.
///
/// The one thing checked is that there *is* a struct body to open: a prefix that
/// is not a plain identifier, or a body that does not end in `)`, is rejected by
/// name rather than mangled.
///
/// # Two sources, one mechanism
///
/// `fields` maps a field name to the **RON text** of its value, and two things
/// produce those: a `refs` entry, whose value is a resolved handle rendered by
/// `ron::to_string`, and — since M4.5 — a prefab instance's `fill`, whose value
/// is the author's own bytes. Taking text rather than a handle is what let the
/// second arrive without a second splicer; ADR-0021 records it.
///
/// A field spliced here that the body already sets produces a duplicate field,
/// which the component's own deserializer refuses. That is deliberate and is the
/// whole of M4.5's collision rule: no precedence, no silent winner.
pub(crate) fn splice(
    location: Option<Location>,
    index: usize,
    component: &str,
    body: &str,
    fields: &BTreeMap<String, String>,
) -> Result<String, SceneError> {
    let trimmed = body.trim();
    let refuse = || SceneError::NotAStructBody {
        location,
        index,
        component: component.to_owned(),
        body: trimmed.to_owned(),
    };

    let open = trimmed.find('(').ok_or_else(refuse)?;
    let prefix = &trimmed[..open];
    if !prefix.is_empty() && !is_identifier(prefix) {
        return Err(refuse());
    }
    if !trimmed.ends_with(')') {
        return Err(refuse());
    }

    let interior = &trimmed[open + 1..trimmed.len() - 1];

    let mut out = String::with_capacity(trimmed.len() + fields.len() * 32);
    out.push_str(prefix);
    out.push('(');
    for (field, value) in fields {
        let _ = write!(out, "{field}:{value},");
    }
    // A trailing comma before nothing is legal RON, but writing one into an
    // otherwise empty body would turn `()` into `(,)`, which is not.
    if !interior.trim().is_empty() {
        out.push_str(interior);
    }
    out.push(')');

    Ok(out)
}

/// Whether `text` is a plain RON identifier, which is all a struct name in front
/// of a body may be.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}
