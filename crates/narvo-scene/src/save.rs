//! The save format: a world's state written down so a later run can carry on
//! from it.
//!
//! Read ADR-0043 before changing anything here, and ADR-0018 beside it. A scene
//! and a save share one *mechanism* — the component registry's own RON, byte for
//! byte what `ComponentInfo::serialize` produces — and they are two *contracts*:
//!
//! | | a scene | a save |
//! |---|---|---|
//! | is | written | produced |
//! | holds | content | state |
//! | names entities | symbolically, by `name` | explicitly, by slot and generation |
//! | reaches | worlds a fresh world can spawn into | every world, despawn history included |
//! | grows | additively, no version (ADR-0018 Decision 6) | by version, refused when unknown |
//! | outlives | nothing — git is its version | the build that wrote it |
//!
//! The last row is the one that decides the rest. A scene ships in the same
//! commit as the engine that reads it; a save is opened by a player a year later
//! and by a build that has moved on. `#[serde(default)]` is the right answer for
//! the first and no answer at all for the second, because it cannot say *which*
//! version wrote the file it is silently tolerating.
//!
//! # What a save carries that a scene cannot
//!
//! 1. **The handle of every live entity**, slot and generation. A scene has no
//!    place to put one: file order *is* spawn order there, so slot *n* is
//!    position *n* and generation is always 1 (ADR-0018 Decision 2).
//! 2. **The free list** — the slots a despawn released and the generation each
//!    will carry when it is handed out again. This is the half of a world that
//!    [`canonical_dump`] cannot show, because a dump prints live entities and a
//!    free slot holds none. A save that recorded only what the dump shows would
//!    round-trip byte for byte and hand out different handles from its next
//!    spawn onwards. The format cannot express that world: the entity table has
//!    to name every slot below its highest, and [`World::reconstitute`] refuses
//!    it otherwise.
//! 3. **The tick.** It is not in the world at all — the runner counts it and
//!    passes it to systems through `SystemContext` — and systems compute world
//!    state from it. It is the second thing a dump does not show and the next
//!    tick does.
//!
//! Everything else that decides the next tick is already a component and needs
//! nothing here: the generator's state (ADR-0010), events in flight, both halves
//! of every buffer (ADR-0011), and the rigid bodies the solver rebuilds itself
//! from every tick (ADR-0029).
//!
//! # The file
//!
//! ```ron
//! Save(
//!     version: 1,
//!     tick: 4200,
//!     entities: [
//!         (
//!             id: (index: 0, generation: 1),
//!             components: {
//!                 "transform": (x:1.0,y:2.0,rotation:0.0,scale_x:1.0,scale_y:1.0),
//!             },
//!         ),
//!     ],
//!     free: [
//!         (index: 3, generation: 2),
//!         (index: 1, generation: 2),
//!     ],
//! )
//! ```
//!
//! `free` is in the order [`World::freelist`] reports, which is the order the
//! next spawns consume **from the back**: `(index: 1, …)` above is the slot the
//! next spawn takes.
//!
//! # Examples
//!
//! ```
//! use narvo_ecs::{ComponentRegistry, Transform, World, canonical_dump};
//!
//! let mut registry = ComponentRegistry::new();
//! registry.register_component::<Transform>("transform")?;
//!
//! let mut world = World::new();
//! let entity = world.spawn();
//! world.insert(entity, Transform::default())?;
//!
//! let text = narvo_scene::save::to_string(&world, 4200, &registry)?;
//! let loaded = narvo_scene::save::from_str(&text, &registry)?;
//!
//! assert_eq!(loaded.tick, 4200);
//! assert_eq!(
//!     canonical_dump(&loaded.world, &registry)?,
//!     canonical_dump(&world, &registry)?
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use narvo_ecs::{ComponentRegistry, EcsError, EntityId, World, canonical_dump};
use serde::Deserialize;

use crate::INDENT;
use crate::file::RawPairs;
use crate::location::{self, Location};

/// The version this build writes and the only one it reads.
///
/// It is a number in the file rather than a `#[serde(default)]` field per
/// change, and ADR-0043 records why: a default can absorb a new field, but it
/// cannot tell a reader *which* build wrote the file it is absorbing, and a save
/// is read by builds its author never saw.
pub const FORMAT_VERSION: u32 = 1;

/// A loaded save: the world it holds, and the tick it was taken at.
///
/// The two travel together because neither reproduces a run on its own. The
/// world without the tick continues at the wrong moment for every system that
/// reads one; the tick without the world says nothing.
#[derive(Debug)]
pub struct Savepoint {
    /// The world as it stood.
    pub world: World,
    /// The tick the run had reached. Feed it back to the loop that continues.
    pub tick: u64,
}

/// Something that went wrong writing or reading a save.
///
/// Separate from [`SceneError`](crate::SceneError) rather than folded into it,
/// because the two formats are two contracts: a caller handling a failed load
/// should not have to consider a missing prefab, and a caller handling a failed
/// scene should not have to consider a version.
#[derive(Debug)]
#[non_exhaustive]
pub enum SaveError {
    /// The file is not valid RON, or does not have the shape of a save.
    ///
    /// Also what a truncated file produces: RON stops where the text stops and
    /// says what it expected there.
    Syntax {
        /// Where the parser stopped.
        location: Location,
        /// What it said.
        message: String,
    },
    /// The file names a format version this build does not read.
    ///
    /// Read *before* the file is parsed strictly, which is the whole point of
    /// the field: a save from a later build carries fields this one has never
    /// heard of, and reporting the first of those as an unknown field would hide
    /// the reason behind a symptom.
    UnsupportedVersion {
        /// The version the file names.
        found: u32,
        /// The version this build reads.
        supported: u32,
    },
    /// An entity carries a component name the registry does not know.
    ///
    /// The ordinary shape of "a save from an older build": a component that
    /// existed when the file was written and does not exist now. There is no
    /// silent skip — a component left out of a load is state the run continues
    /// without, and it would continue plausibly.
    UnknownComponent {
        /// Where the body sits in the file, when it can be located.
        location: Option<Location>,
        /// The entity the component was written on.
        entity: EntityId,
        /// The name the file used.
        component: String,
        /// The names the registry does know, in its own order.
        known: Vec<String>,
    },
    /// One entity names the same component twice.
    DuplicateComponent {
        /// Where the second one sits, when it can be located.
        location: Option<Location>,
        /// The entity that carries the repeat.
        entity: EntityId,
        /// The repeated name.
        component: String,
    },
    /// A component's body is not a valid rendering of that component.
    Component {
        /// Where the body sits in the file, when it can be located.
        location: Option<Location>,
        /// The entity the component was being put on.
        entity: EntityId,
        /// The stable name of the component.
        component: String,
        /// What the registry said.
        source: EcsError,
    },
    /// The entity table does not describe a world.
    ///
    /// Carried through from [`World::reconstitute`], which names the slot.
    Table(EcsError),
    /// The world could not be written, or a component could not be read back.
    Ecs(EcsError),
    /// The file could not be read or written.
    Io {
        /// The path that was tried.
        path: PathBuf,
        /// What the file system said.
        source: std::io::Error,
    },
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax { location, message } => {
                write!(f, "{location}: the save is not valid RON: {message}")
            }
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "this save says it is version {found} and this build reads version {supported}; \
                 a save is read by builds its author never saw, so an unknown version is \
                 refused rather than guessed at"
            ),
            Self::UnknownComponent {
                location,
                entity,
                component,
                known,
            } => {
                write_location(f, *location)?;
                write!(
                    f,
                    "{entity:?} carries \"{component}\", which no component is registered under; \
                     the known ones are {}. A save names components the registry knows, and a \
                     name it does not know is usually a save from a build that had it",
                    quoted(known)
                )
            }
            Self::DuplicateComponent {
                location,
                entity,
                component,
            } => {
                write_location(f, *location)?;
                write!(
                    f,
                    "{entity:?} names \"{component}\" twice; one entity holds one component of a \
                     type, so a save naming it twice says two different things about one place"
                )
            }
            Self::Component {
                location,
                entity,
                component,
                source,
            } => {
                write_location(f, *location)?;
                write!(
                    f,
                    "the \"{component}\" of {entity:?} could not be read back: {source}"
                )
            }
            Self::Table(source) => write!(f, "the save's entity table is not a world: {source}"),
            Self::Ecs(source) => source.fmt(f),
            Self::Io { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
        }
    }
}

impl Error for SaveError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Syntax { .. }
            | Self::UnsupportedVersion { .. }
            | Self::UnknownComponent { .. }
            | Self::DuplicateComponent { .. } => None,
            Self::Component { source, .. } | Self::Table(source) | Self::Ecs(source) => {
                Some(source)
            }
            Self::Io { source, .. } => Some(source),
        }
    }
}

impl From<EcsError> for SaveError {
    fn from(source: EcsError) -> Self {
        Self::Ecs(source)
    }
}

/// Writes `location` as the `line:column ` prefix a message opens with, or
/// nothing when there is no position to give.
fn write_location(f: &mut fmt::Formatter<'_>, location: Option<Location>) -> fmt::Result {
    match location {
        Some(location) => write!(f, "{location}: "),
        None => Ok(()),
    }
}

/// Renders a list of names as `"a", "b"`, for a message that lists them.
fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads a save's version and nothing else.
///
/// Deliberately tolerant of every other field, which is what lets a file from a
/// later build be refused *by its version* rather than by whichever new field
/// happens to come first. The struct is named `Save` because `ron` checks a
/// written struct name against the type it is deserializing into and reports
/// `ExpectedDifferentStructName` when they differ — measured against
/// `ron` 0.12.2, not assumed.
mod probe {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    pub(super) struct Save {
        pub(super) version: u32,
    }
}

/// A save file's outer shape.
///
/// `deny_unknown_fields` on both structs, unlike a component body, which serde
/// lets ignore what it does not know. A save is state: a field this build cannot
/// place is state the run would carry on without, and carrying on without state
/// is the failure this format exists to make impossible.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Save<'a> {
    /// Which version of this format wrote the file.
    version: u32,
    /// The tick the run had reached.
    tick: u64,
    /// Every live entity, with its handle.
    #[serde(borrow)]
    entities: Vec<SavedEntity<'a>>,
    /// The slots waiting to be handed out again.
    ///
    /// Defaulted rather than required, because a world that has never despawned
    /// has none and writing `free: []` into every save of one would be noise.
    #[serde(default)]
    free: Vec<EntityId>,
}

/// One entity of a save.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedEntity<'a> {
    /// The slot and generation this entity occupies.
    id: EntityId,
    /// Its components, keyed by stable registry name, each body verbatim.
    ///
    /// [`RawPairs`] rather than a map for the reason the scene format gives: a
    /// map would swallow a repeated key, and every body has to reach the
    /// registry as the bytes that were written, because the state hash is taken
    /// over exactly those spellings (ADR-0008, ADR-0018 Decision 4).
    #[serde(default, borrow)]
    components: RawPairs<'a>,
}

/// Writes a [`World`] and the tick it stands at as the text of a save file.
///
/// # What it refuses, and what it does not
///
/// It refuses a world carrying a component the registry does not know, by asking
/// [`canonical_dump`] — the same refusal and made the same way as
/// [`crate::to_string`], so the two writers cannot drift apart on it.
///
/// It does **not** refuse a world with despawn history. That is the difference
/// between the two formats: `crate::to_string` refuses one because no scene file
/// could load it back, and this one exists for it.
///
/// # Errors
///
/// [`SaveError::Ecs`] for a world carrying an unregistered component, or for a
/// component whose own `Serialize` fails.
pub fn to_string(
    world: &World,
    tick: u64,
    registry: &ComponentRegistry,
) -> Result<String, SaveError> {
    // Asked rather than re-implemented, exactly as `crate::to_string` does: a
    // component outside the dump is a component outside the state hash, and a
    // save that quietly left one out would restore a world that agrees with its
    // file and not with the run it came from.
    canonical_dump(world, registry)?;

    let mut out = String::new();
    out.push_str("Save(\n");
    let _ = writeln!(out, "{INDENT}version: {FORMAT_VERSION},");
    let _ = writeln!(out, "{INDENT}tick: {tick},");
    let _ = writeln!(out, "{INDENT}entities: [");

    for entity in world.entity_ids() {
        let _ = writeln!(out, "{INDENT}{INDENT}(");
        let _ = writeln!(
            out,
            "{INDENT}{INDENT}{INDENT}id: (index: {}, generation: {}),",
            entity.index(),
            entity.generation()
        );
        let _ = writeln!(out, "{INDENT}{INDENT}{INDENT}components: {{");
        for info in registry.iter() {
            if let Some(body) = info.serialize(world, entity)? {
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
    let _ = writeln!(out, "{INDENT}free: [");
    for entity in world.freelist() {
        let _ = writeln!(
            out,
            "{INDENT}{INDENT}(index: {}, generation: {}),",
            entity.index(),
            entity.generation()
        );
    }
    let _ = writeln!(out, "{INDENT}],");
    out.push_str(")\n");

    Ok(out)
}

/// Builds a [`Savepoint`] from the text of a save file.
///
/// # A failed load cannot touch a running world
///
/// Structurally, not by care: this function is handed no world. It builds one
/// through [`World::reconstitute`], which is an associated function for exactly
/// this reason, and hands it back only once every component is in place. A
/// caller that keeps its running world until this returns `Ok` cannot lose it,
/// and a caller that does not have to remember to. That is ADR-0022's posture —
/// a failed load leaves the running state standing — obtained by making the
/// other case unwritable rather than by guarding against it.
///
/// # How the version gets the last word
///
/// A save from a later build carries fields this one has never heard of, so the
/// strict pass fails on the first of them and would bury the reason under a
/// symptom. Before that failure is reported, the file is asked what version it
/// claims, by a deserializer that tolerates everything except the version field
/// itself. A save whose *shape* this build still understands — a lower version
/// with the same fields — is caught by the ordinary check afterwards.
///
/// The tolerant pass runs only when the strict one has already failed, so an
/// ordinary load parses the text once.
///
/// # Errors
///
/// [`SaveError`], with a position wherever the file has one.
pub fn from_str(text: &str, registry: &ComponentRegistry) -> Result<Savepoint, SaveError> {
    let save: Save<'_> = match ron::from_str::<Save<'_>>(text) {
        Ok(save) => save,
        Err(strict) => {
            if let Ok(probe) = ron::from_str::<probe::Save>(text)
                && probe.version != FORMAT_VERSION
            {
                return Err(SaveError::UnsupportedVersion {
                    found: probe.version,
                    supported: FORMAT_VERSION,
                });
            }
            return Err(syntax(strict));
        }
    };

    if save.version != FORMAT_VERSION {
        return Err(SaveError::UnsupportedVersion {
            found: save.version,
            supported: FORMAT_VERSION,
        });
    }

    let live: Vec<EntityId> = save.entities.iter().map(|entity| entity.id).collect();
    let mut world = World::reconstitute(&live, &save.free).map_err(SaveError::Table)?;

    for entry in &save.entities {
        let mut seen: Vec<&str> = Vec::with_capacity(entry.components.0.len());

        for (name, body) in &entry.components.0 {
            let location = location::locate(text, body.get_ron());

            if seen.contains(&name.as_str()) {
                return Err(SaveError::DuplicateComponent {
                    location,
                    entity: entry.id,
                    component: name.clone(),
                });
            }
            seen.push(name);

            if !registry.contains(name) {
                return Err(SaveError::UnknownComponent {
                    location,
                    entity: entry.id,
                    component: name.clone(),
                    known: registry.iter().map(|info| info.name().to_owned()).collect(),
                });
            }

            // Trimmed and otherwise untouched: what a `RawValue` hands back may
            // carry the space after the colon, and everything past that is the
            // writer's own bytes.
            let body_text = body.get_ron().trim();
            registry
                .deserialize_component(name, &mut world, entry.id, body_text)
                .map_err(|source| SaveError::Component {
                    location,
                    entity: entry.id,
                    component: name.clone(),
                    source,
                })?;
        }
    }

    Ok(Savepoint {
        world,
        tick: save.tick,
    })
}

/// Turns a `ron` failure into this crate's positioned syntax error.
fn syntax(error: ron::error::SpannedError) -> SaveError {
    SaveError::Syntax {
        location: Location::new(error.span.start.line, error.span.start.col),
        message: error.code.to_string(),
    }
}

/// Reads a save file from disk.
///
/// A thin shell around [`from_str`], as [`crate::from_file`] is around
/// [`crate::from_str`]: the whole format lives in the string functions, so a
/// caller that already has the text never touches a path.
///
/// # Errors
///
/// [`SaveError::Io`] naming the path, plus everything [`from_str`] returns.
pub fn from_file(path: &Path, registry: &ComponentRegistry) -> Result<Savepoint, SaveError> {
    let text = std::fs::read_to_string(path).map_err(|source| SaveError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    from_str(&text, registry)
}

/// Writes a save file to disk.
///
/// # Errors
///
/// [`SaveError::Io`] naming the path, plus everything [`to_string`] returns.
pub fn to_file(
    path: &Path,
    world: &World,
    tick: u64,
    registry: &ComponentRegistry,
) -> Result<(), SaveError> {
    let text = to_string(world, tick, registry)?;

    std::fs::write(path, text).map_err(|source| SaveError::Io {
        path: path.to_path_buf(),
        source,
    })
}
