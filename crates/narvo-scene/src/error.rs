//! What can go wrong reading or writing a scene, and how it says so.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use narvo_ecs::{EcsError, EntityId};

use crate::location::Location;

/// Something that stopped a scene from being read or written.
///
/// # These messages are the product
///
/// `ProjektPlan.md` §6/M4 names error message quality as a feature of this
/// milestone rather than a nicety, and gives the reason: a scene is written by
/// hand, by a person or by an agent, and the message is the whole of the
/// feedback either of them gets. So every variant here answers three questions —
/// **where**, **what** and **what to do** — and every one of them is asserted on
/// in a test, wording included, because a message nobody checks drifts into
/// uselessness one refactor at a time.
///
/// **Where** is spelled two ways, and the difference is not arbitrary.
/// [`Syntax`](Self::Syntax) carries a line and a column, because `ron` knows
/// them and throwing them away would be a loss. Every other variant names the
/// entity by its **position in the file's entity list**, counting from zero,
/// plus its name when it has one. That position is not a consolation prize for a
/// missing line number: it is also the entity's slot index in the loaded world
/// (see [`crate::from_str`]), so it is the number that appears again in a
/// canonical dump, in a state hash diff and in a `first_difference` report. A
/// line number would locate the mistake in the file; this locates it in the file
/// *and* in the world.
#[derive(Debug)]
#[non_exhaustive]
pub enum SceneError {
    /// The file is not valid RON.
    ///
    /// Position and cause both come from `ron`'s own spanned error, unchanged.
    Syntax {
        /// Where the parser stopped.
        location: Location,
        /// `ron`'s own description of what it expected.
        message: String,
    },
    /// Two templates in the `prefabs` block share a name.
    DuplicatePrefabName {
        /// Where the repeat was written, when it could be located.
        location: Option<Location>,
        /// The contested name.
        name: String,
        /// Position of the template that claimed it first.
        first: usize,
        /// Position of the template that was turned away.
        second: usize,
    },
    /// An entity instantiates a template the scene does not define.
    UnknownPrefab {
        /// Where the name was written, when it could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The name that was not found.
        name: String,
        /// Every template the scene does define, in name order.
        known: Vec<String>,
    },
    /// An instance fills a field its template already sets.
    ///
    /// The hard error of ADR-0021's hole-and-fill rule. There is no precedence
    /// and no silent winner: a field is set in one place or the other.
    FieldCollision {
        /// Where the component's body is, when it could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The template it came from.
        prefab: String,
        /// The component whose field is set twice.
        component: String,
        /// The field in question.
        field: String,
    },
    /// An instance removes a component its template does not have.
    RemovingAbsentComponent {
        /// Where the removal was written, when it could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The component it asked to remove.
        component: String,
        /// What the template does carry, in template order.
        present: Vec<String>,
    },
    /// A `fill` was given for a component the entity does not carry.
    FillForAbsentComponent {
        /// Where in the file this is, when it could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The component named by `fill` and carried by nothing.
        component: String,
    },
    /// `fill` or `without` was used on an entity that instantiates nothing.
    PrefabOnlyField {
        /// Position of the entity in the file's entity list.
        index: usize,
        /// Which of the two it was.
        field: &'static str,
        /// The component it named.
        component: String,
    },
    /// An entity names the same component twice.
    DuplicateComponent {
        /// Where in the file this is, when the fragment could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list, counting from zero.
        index: usize,
        /// The name that appears more than once.
        component: String,
    },
    /// Two entities claim the same name.
    DuplicateEntityName {
        /// Where the repeat was written, when it could be located.
        ///
        /// The *second* name, not the first: the first one is where the name
        /// legitimately lives, and the one an author has to go and change is the
        /// one that arrived too late.
        location: Option<Location>,
        /// The contested name.
        name: String,
        /// Position of the entity that claimed it first.
        first: usize,
        /// Position of the entity that was turned away.
        second: usize,
    },
    /// An entity's `name` is written, but is not a string.
    InvalidEntityName {
        /// Where the name was written, when it could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The value as written.
        written: String,
        /// The deserializer's own complaint.
        message: String,
    },
    /// A component name in the file is not in the registry.
    UnknownComponent {
        /// Where in the file this is, when the fragment could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// That entity's name, when it has one.
        entity_name: Option<String>,
        /// The name that was not found.
        component: String,
        /// Every name the registry does know, in its canonical order.
        ///
        /// A `Box<[String]>` rather than a `Vec`: the list is built once and
        /// never grows, so the capacity field a `Vec` carries is eight bytes of
        /// nothing. That is not tidiness — it is what keeps this enum under
        /// `clippy::result_large_err`'s 128-byte threshold, which it sat exactly
        /// on once the variants gained positions.
        known: Box<[String]>,
    },
    /// A reference names an entity the scene does not contain.
    UnknownReference {
        /// Where in the file this is, when the fragment could be located.
        location: Option<Location>,
        /// Position of the referring entity in the file's entity list.
        index: usize,
        /// The component the reference belongs to.
        component: String,
        /// The field the reference fills.
        field: String,
        /// The name that was not found.
        name: String,
        /// Every name the scene does define, for the same reason and in the
        /// same shape as [`UnknownComponent`](Self::UnknownComponent)'s.
        known: Box<[String]>,
    },
    /// A reference was given for a component the entity does not carry.
    ReferenceForAbsentComponent {
        /// Where in the file this is, when the fragment could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The component named by `refs` and missing from `components`.
        component: String,
    },
    /// A component that takes a reference is not written as a struct body.
    ///
    /// A reference is spliced into the body as an extra field, which needs the
    /// body to be `(…)` or `Name(…)`. A tuple, a number or a string has nowhere
    /// to put a named field.
    NotAStructBody {
        /// Where in the file this is, when the fragment could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The component whose body cannot take a field.
        component: String,
        /// The body as written.
        body: String,
    },
    /// A resolved entity handle could not be written into a component's body.
    ///
    /// Only reachable if `EntityId`'s own `Serialize` fails, which it has no way
    /// of doing today — two integers. It exists rather than an `expect` because
    /// this crate's job is turning failures into messages, and a panic on a load
    /// path is the one shape of failure a scene author cannot act on.
    HandleSerialization {
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The component the reference belongs to.
        component: String,
        /// The field the reference fills.
        field: String,
        /// The serializer's own error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// A component's body is not a valid rendering of its type.
    Component {
        /// Where in the file this is, when the fragment could be located.
        location: Option<Location>,
        /// Position of the entity in the file's entity list.
        index: usize,
        /// The component that would not read back.
        component: String,
        /// The registry's own error, which carries the position inside the body.
        source: EcsError,
    },
    /// A world being written out is not shaped like a loaded scene.
    ///
    /// Raised by [`crate::to_string`] and only there. See that function's
    /// documentation for the domain boundary and why it is drawn here.
    NotSceneShaped {
        /// The entity whose handle is outside the scene domain.
        entity: EntityId,
        /// The slot index a scene-shaped world would have had at this position.
        expected_index: u32,
    },
    /// The world facade or the registry refused an operation.
    Ecs(EcsError),
    /// A scene file could not be read or written.
    Io {
        /// The path that was being read or written.
        path: PathBuf,
        /// The operating system's own error.
        source: std::io::Error,
    },
}

impl From<EcsError> for SceneError {
    fn from(error: EcsError) -> Self {
        Self::Ecs(error)
    }
}

/// Renders an entity as the file locates it, with its name when it has one.
fn at(index: usize, name: Option<&String>) -> String {
    match name {
        Some(name) => format!("entity {index} (\"{name}\")"),
        None => format!("entity {index}"),
    }
}

/// Renders a list of known names for a "did you mean" tail.
///
/// The whole list rather than the nearest match: an edit-distance guess is right
/// often enough to be trusted and wrong often enough to mislead, and these lists
/// are short — a registry has single digits of entries and a scene has as many
/// names as it has named entities.
fn listing(known: &[String]) -> String {
    if known.is_empty() {
        return "none are".to_owned();
    }

    let quoted: Vec<String> = known.iter().map(|name| format!("\"{name}\"")).collect();
    format!("the known ones are {}", quoted.join(", "))
}

impl SceneError {
    /// Where in the input this error is, when it is bound to a place at all.
    ///
    /// `None` has two meanings and they are worth telling apart. For a
    /// writer-side variant — [`NotSceneShaped`](Self::NotSceneShaped),
    /// [`Ecs`](Self::Ecs), [`Io`](Self::Io) — there *is* no input text, so there
    /// is nothing a position could refer to. For an input-bound variant it means
    /// the fragment could not be located, which
    /// [`locate`](crate::location::locate) reports rather than guessing; the
    /// message is then one line poorer and never wrong.
    #[must_use]
    pub fn location(&self) -> Option<Location> {
        match self {
            Self::Syntax { location, .. } => Some(*location),
            Self::DuplicateComponent { location, .. }
            | Self::DuplicatePrefabName { location, .. }
            | Self::UnknownPrefab { location, .. }
            | Self::FieldCollision { location, .. }
            | Self::RemovingAbsentComponent { location, .. }
            | Self::FillForAbsentComponent { location, .. }
            | Self::DuplicateEntityName { location, .. }
            | Self::InvalidEntityName { location, .. }
            | Self::UnknownComponent { location, .. }
            | Self::UnknownReference { location, .. }
            | Self::ReferenceForAbsentComponent { location, .. }
            | Self::NotAStructBody { location, .. }
            | Self::Component { location, .. } => *location,
            Self::PrefabOnlyField { .. }
            | Self::HandleSerialization { .. }
            | Self::NotSceneShaped { .. }
            | Self::Ecs(_)
            | Self::Io { .. } => None,
        }
    }

    /// The message without its position.
    ///
    /// [`Display`](fmt::Display) puts the two together; a consumer that renders
    /// the position itself — the validation CLI does, in its own column — wants
    /// them apart, and printing `4:9:` twice on one line is what happens when
    /// only the joined form exists.
    #[must_use]
    pub fn message(&self) -> String {
        Bare(self).to_string()
    }
}

/// Renders a [`SceneError`]'s message without its position.
struct Bare<'a>(&'a SceneError);

impl fmt::Display for Bare<'_> {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per variant, and each arm is one message; splitting it \
                  would put the wording further from the variant it belongs to"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            SceneError::Syntax { message, .. } => {
                write!(f, "the scene is not valid RON: {message}")
            }
            SceneError::DuplicatePrefabName {
                name,
                first,
                second,
                ..
            } => write!(
                f,
                "two templates in the prefabs block are both called \"{name}\": entry \
                 {first} and entry {second}. A prefab name identifies exactly one \
                 template, or an entity instantiating it would have no answer"
            ),
            SceneError::UnknownPrefab {
                index, name, known, ..
            } => write!(
                f,
                "entity {index} instantiates \"{name}\", which the prefabs block does \
                 not define; {}",
                listing(known)
            ),
            SceneError::FieldCollision {
                index,
                prefab,
                component,
                field,
                ..
            } => write!(
                f,
                "entity {index} fills {component}.{field}, and the template \
                 \"{prefab}\" already sets it. A field is set by the template or by \
                 the instance, never by both: there is no precedence rule and no \
                 silent winner. Either take {field} out of the template so it is a \
                 hole every instance fills, or drop it from this instance's fill and \
                 take the template's value"
            ),
            SceneError::RemovingAbsentComponent {
                index,
                component,
                present,
                ..
            } => write!(
                f,
                "entity {index} removes \"{component}\", which its template does not \
                 have; {}",
                listing(present)
            ),
            SceneError::FillForAbsentComponent {
                index, component, ..
            } => write!(
                f,
                "entity {index} fills a field of \"{component}\" and carries no such \
                 component - neither from its template nor of its own; a fill \
                 completes a component that is there, it does not add one"
            ),
            SceneError::PrefabOnlyField {
                index,
                field,
                component,
            } => write!(
                f,
                "entity {index} uses `{field}` for \"{component}\" without \
                 instantiating a template; `fill` completes a template's holes and \
                 `without` drops a template's components, so both need `from`"
            ),
            SceneError::DuplicateComponent {
                index, component, ..
            } => write!(
                f,
                "entity {index} names the component \"{component}\" twice; an entity carries \
                 one of each component, so one of the two would be silently discarded"
            ),
            SceneError::InvalidEntityName {
                index,
                written,
                message,
                ..
            } => write!(
                f,
                "entity {index} has a `name` of `{written}`, which is not a string: {message}. \
                 A name is what a reference points at, so it has to be text"
            ),
            SceneError::DuplicateEntityName {
                name,
                first,
                second,
                ..
            } => write!(
                f,
                "two entities are both named \"{name}\": entity {first} and entity {second}. \
                 A name has to identify exactly one entity, or a reference to it would have \
                 no answer"
            ),
            SceneError::UnknownComponent {
                index,
                entity_name,
                component,
                known,
                ..
            } => write!(
                f,
                "{} carries \"{component}\", which no component is registered under; {}. \
                 A scene can only name components the registry knows, because the registry \
                 is the only thing that can turn a name back into a type",
                at(*index, entity_name.as_ref()),
                listing(known)
            ),
            SceneError::UnknownReference {
                index,
                component,
                field,
                name,
                known,
                ..
            } => write!(
                f,
                "entity {index} points its {component}.{field} at an entity named \
                 \"{name}\", and no entity in this scene has that name; {}",
                listing(known)
            ),
            SceneError::ReferenceForAbsentComponent {
                index, component, ..
            } => write!(
                f,
                "entity {index} gives a reference for \"{component}\" but does not carry that \
                 component; a reference fills a field of a component that is there, it does \
                 not add one"
            ),
            SceneError::NotAStructBody {
                index,
                component,
                body,
                ..
            } => write!(
                f,
                "entity {index} takes a reference into its \"{component}\", whose body is \
                 `{body}`; a reference is spliced in as a named field, so the body has to be \
                 written as `(…)` or `Name(…)`"
            ),
            SceneError::HandleSerialization {
                index,
                component,
                field,
                source,
            } => write!(
                f,
                "entity {index} could not write the handle for {component}.{field}: {source}"
            ),
            SceneError::Component {
                index,
                component,
                source,
                ..
            } => write!(
                f,
                "entity {index} could not read its \"{component}\": {source}"
            ),
            SceneError::NotSceneShaped {
                entity,
                expected_index,
            } => write!(
                f,
                "{entity:?} cannot be written as a scene: a scene constitutes a fresh world, \
                 so its entities occupy slots 0.. in order at generation 1, and this one is \
                 where slot {expected_index} generation 1 was expected. A world that has \
                 despawned something is a saved state rather than a scene; that is the \
                 save side of the same round trip and not this function"
            ),
            SceneError::Ecs(source) => write!(f, "{source}"),
            SceneError::Io { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
        }
    }
}

impl fmt::Display for SceneError {
    /// `line:column: message` when the error is bound to a place, and the bare
    /// message when it is not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(location) = self.location() {
            write!(f, "{location}: ")?;
        }
        Bare(self).fmt(f)
    }
}

impl Error for SceneError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Syntax { .. }
            | Self::DuplicatePrefabName { .. }
            | Self::UnknownPrefab { .. }
            | Self::FieldCollision { .. }
            | Self::RemovingAbsentComponent { .. }
            | Self::FillForAbsentComponent { .. }
            | Self::PrefabOnlyField { .. }
            | Self::DuplicateComponent { .. }
            | Self::DuplicateEntityName { .. }
            | Self::InvalidEntityName { .. }
            | Self::UnknownComponent { .. }
            | Self::UnknownReference { .. }
            | Self::ReferenceForAbsentComponent { .. }
            | Self::NotAStructBody { .. }
            | Self::NotSceneShaped { .. } => None,
            Self::Component { source, .. } | Self::Ecs(source) => Some(source),
            Self::HandleSerialization { source, .. } => Some(source.as_ref()),
            Self::Io { source, .. } => Some(source),
        }
    }
}
