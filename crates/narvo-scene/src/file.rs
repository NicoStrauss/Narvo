//! The scene file's outer shape: what RON reads before any component does.
//!
//! Everything here is *scene syntax*. None of it is world state, and none of it
//! reaches the canonical dump: an entity's name exists so that another entity
//! can point at it while the file is being read, and stops existing the moment
//! the world is built. ADR-0018 records why that is a decision rather than an
//! omission.

use std::collections::BTreeMap;
use std::fmt;

use ron::value::RawValue;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// A whole scene file.
///
/// The struct is called `Scene` so that a file may open with `Scene(` and say
/// what it is. RON checks a struct name when one is written and accepts the
/// bare `(` when none is, so both spellings parse and the writer picks the
/// self-describing one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Scene<'a> {
    /// One-entity templates, by name.
    ///
    /// Optional and empty by default, which is what keeps every scene written
    /// before M4.5 — the specimen and the frozen determinism case among them —
    /// parsing unchanged.
    #[serde(default, borrow)]
    pub(crate) prefabs: PrefabList<'a>,
    /// The entities, in the order they are spawned.
    #[serde(borrow)]
    pub(crate) entities: Vec<SceneEntity<'a>>,
}

/// The `prefabs` block: named templates, in file order.
///
/// A pair list rather than a map for the reason [`RawPairs`] gives — a map would
/// swallow a repeated name, and two templates under one name is a mistake an
/// author has to be told about rather than have resolved for them.
#[derive(Debug, Default)]
pub(crate) struct PrefabList<'a>(pub(crate) Vec<(String, Prefab<'a>)>);

impl<'de: 'a, 'a> Deserialize<'de> for PrefabList<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Pairs<'a>(std::marker::PhantomData<&'a ()>);

        impl<'de: 'a, 'a> Visitor<'de> for Pairs<'a> {
            type Value = PrefabList<'a>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map from a prefab name to its template")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(entry) = map.next_entry::<String, Prefab<'a>>()? {
                    pairs.push(entry);
                }
                Ok(PrefabList(pairs))
            }
        }

        deserializer.deserialize_map(Pairs(std::marker::PhantomData))
    }
}

/// One template: the components an instance of it starts from.
///
/// Exactly the shape of an entity's own contribution, minus everything that is
/// about *being* an entity — no name, because a template is not in the world and
/// nothing can point at one, and no `from`, because ADR-0021 rules out a prefab
/// built on another.
///
/// A body here may leave fields out. That is the hole an instance fills, and it
/// is why a template body is a [`RawValue`] like every other: what the instance
/// splices into is the author's own bytes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Prefab<'a> {
    /// The components an instance starts from.
    #[serde(borrow)]
    pub(crate) components: RawPairs<'a>,
    /// References the template declares, resolved in each instance's own scene.
    ///
    /// A template cannot resolve them itself: it is not in the world, and the
    /// entity it points at may not exist until an instance is spawned.
    #[serde(default, borrow)]
    pub(crate) refs: BTreeMap<String, RawPairs<'a>>,
}

/// One entity of a scene.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SceneEntity<'a> {
    /// What other entities in this file may call it.
    ///
    /// Absent means anonymous, and so does the empty string — that is ADR-0018's
    /// rule and it has not changed. What *has* changed since M4.1 is that the
    /// two are now told apart: the field keeps the author's raw value when it is
    /// present, so `name: ""` is a written empty name rather than an
    /// indistinguishable default. Two things need that distinction — the
    /// warning `validate` raises for a written empty name, and the position of
    /// the name in the file, which a duplicate has to be able to point at.
    #[serde(default, borrow)]
    pub(crate) name: MaybeRaw<'a>,
    /// The components, keyed by the stable registry name.
    ///
    /// Every value is kept as [`RawValue`] — the author's own bytes, borrowed
    /// straight out of the file. That is the load-bearing choice of this module
    /// and ADR-0018 argues it: a component body must reach the registry
    /// **verbatim**, because re-rendering it through any intermediate
    /// representation would put a number's spelling in the hands of something
    /// other than the author, and the state hash is taken over exactly those
    /// spellings.
    ///
    /// Optional since M4.5: an instance that only fills its template's holes
    /// contributes no component of its own, and an entity with no components at
    /// all is an ordinary — if idle — entity.
    #[serde(default, borrow)]
    pub(crate) components: RawPairs<'a>,
    /// Entity references, by component and by field.
    ///
    /// `refs: { "follow": { "target": "player" } }` fills the `target` field of
    /// this entity's `follow` component with the handle of the entity named
    /// `player`. The field must **not** also appear in the component's own body.
    ///
    /// The referenced name is a [`RawValue`] rather than a `String` for the same
    /// reason the entity's own name is: an unresolvable reference has to be able
    /// to say *where* it was written, and only the borrowed slice knows that.
    /// The `String` it holds is parsed out of that slice when it is needed.
    #[serde(default, borrow)]
    pub(crate) refs: BTreeMap<String, RawPairs<'a>>,
    /// The template this entity is an instance of, if it is one.
    ///
    /// Absent means the entity is written out in full, which is every entity
    /// before M4.5 and most of them after it.
    #[serde(default, borrow)]
    pub(crate) from: MaybeRaw<'a>,
    /// Field values for holes the template left open, by component and field.
    ///
    /// The same shape as [`refs`](Self::refs) on purpose: both name a component
    /// and a field and both are spliced into the effective body by the same
    /// mechanism. What differs is where the value comes from — a `refs` entry
    /// names an entity and is resolved to a handle, a `fill` entry carries the
    /// author's own RON.
    #[serde(default, borrow)]
    pub(crate) fill: BTreeMap<String, RawPairs<'a>>,
    /// Components of the template this instance does **not** take.
    ///
    /// Explicit, because omission means inheritance: an instance that simply
    /// does not mention a component still gets it. There is no silent removal.
    ///
    /// Raw values rather than strings, so that removing something the template
    /// does not have can be reported where it was asked for.
    #[serde(default, borrow)]
    pub(crate) without: Vec<&'a RawValue>,
}

/// An optional field that remembers whether it was written at all, verbatim.
///
/// `#[serde(default)]` alone cannot answer that question: an omitted field and a
/// written empty one both arrive as the default. A type whose `Default` means
/// *absent* and whose `Deserialize` means *present* answers it, because serde
/// only calls the second when the key is in the file.
///
/// Two fields use it — an entity's `name` and, since M4.5, its `from` — and both
/// use it for the same second reason: the raw value is a slice of the input, so
/// a finding about either can say where it was written.
#[derive(Debug, Default)]
pub(crate) struct MaybeRaw<'a>(pub(crate) Option<&'a RawValue>);

impl<'de: 'a, 'a> Deserialize<'de> for MaybeRaw<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Self(Some(<&'a RawValue>::deserialize(deserializer)?)))
    }
}

/// A RON map read as a list of key/value pairs, values kept verbatim.
///
/// Used twice: for an entity's components, and for one component's references.
///
/// A `BTreeMap` would have been one line and is wrong here: serde's map visitor
/// inserts, so a file naming the same key twice would quietly keep the last one
/// and the author would never learn that half of what they wrote was discarded.
/// `a_duplicate_component_on_one_entity_is_rejected_rather_than_silently_dropped`
/// is the test; the behaviour it guards is that the pairs arrive here **as
/// written**, and the loader is what rejects the repeat.
#[derive(Debug, Default, Clone)]
pub(crate) struct RawPairs<'a>(pub(crate) Vec<(String, &'a RawValue)>);

impl<'de: 'a, 'a> Deserialize<'de> for RawPairs<'a> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Pairs<'a>(std::marker::PhantomData<&'a ()>);

        impl<'de: 'a, 'a> Visitor<'de> for Pairs<'a> {
            type Value = RawPairs<'a>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map from a name to a RON value")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut pairs = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((name, body)) = map.next_entry::<String, &'a RawValue>()? {
                    pairs.push((name, body));
                }
                Ok(RawPairs(pairs))
            }
        }

        deserializer.deserialize_map(Pairs(std::marker::PhantomData))
    }
}
