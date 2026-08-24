//! The request and response vocabulary, and how each crosses the boundary.

use serde::{Deserialize, Serialize};

use crate::entity::EntityName;
use crate::error::ProtocolError;

/// What an agent asks a running engine.
///
/// # Five of the six capabilities, and the one that is not here
///
/// Six capabilities are named for M6 in `ProjektPlan.md` §6: query entities and
/// components, set them, load a scene, step ticks, take a screenshot, start a
/// replay. M6.1 defined the three reads and left the rest alone, because the
/// *shape* of a write was not determinable then — whether it carries a tick,
/// whether it produces a recording line (ADR-0012), where in a tick it lands
/// (ADR-0022's swap is at a tick boundary and nothing else would be
/// deterministic). A read moves no state, so its shape did not depend on any of
/// that; the precedent was M5.1, which built the mapping core while D8 was open.
///
/// D19 answered it in M6.2 (ADR-0012's amendment): a set is **not** written into
/// a recording, and a run that accepts one has its band cut at that tick. So
/// [`SetComponent`](Self::SetComponent) can have a shape, and it is the only one
/// of the five that ever needed that answer — v0.76's §6/M6 measured that the
/// other four never depended on D19 at all.
///
/// M6.4a added the two that redirect a run rather than reading or nudging it:
/// [`LoadScene`](Self::LoadScene) and [`Replay`](Self::Replay). Both carry a
/// **path and nothing else**, because everything else about the world they
/// produce is in the file — which is ADR-0018's and ADR-0012's rule read from
/// the protocol's side, and is why neither carries a mode, a seed or a tick
/// count that a file could contradict. The screenshot is the sixth and is M6.4b;
/// it is the only one of the six that would put bytes on the wire, and whether
/// this line-based protocol carries them is that task's question.
///
/// # What a response says now, and what it used to leave out
///
/// **Which tick it observed. M6.1 left it out of every response, called it
/// additive, and M6.7b put it back** — every answer that came from a world now
/// carries `ticks_run`. M6.3c had rejected it once more, on `Step` alone, and
/// named the condition for revisiting in as many words: *"No client exists to
/// need it, and M6.5 is where one first will."* M6.5b built that client, M6.7a
/// measured that it cannot name the tick it observed something at, and ADR-0036
/// is where the rest of the reasoning lives.
///
/// # On the wire
///
/// Externally tagged, snake_case, unknown fields refused:
///
/// ```json
/// "list_entities"
/// {"get_entity":{"entity":"3v1"}}
/// {"get_component":{"entity":"3v1","component":"transform"}}
/// {"set_component":{"entity":"3v1","component":"layer","value":"(depth:0.5)"}}
/// {"step":{"ticks":5}}
/// {"load_scene":{"path":"scenes/click_counter.ron"}}
/// {"replay":{"path":"bug.rec"}}
/// "dump"
/// ```
///
/// ADR-0030 records why externally tagged rather than a uniform
/// `{"command": …}` object: measured in M6.1, the internally tagged
/// representation loses the position on six of twelve malformed inputs, and this
/// protocol's error messages are its product.
///
/// # Examples
///
/// ```
/// use narvo_ipc::Request;
///
/// let request = Request::from_json(r#"{"get_component":{"entity":"3v1","component":"transform"}}"#)?;
/// assert_eq!(
///     request,
///     Request::GetComponent {
///         entity: "3v1".parse().expect("a well-formed name"),
///         component: "transform".to_owned(),
///     }
/// );
/// assert_eq!(
///     request.to_json(),
///     r#"{"get_component":{"entity":"3v1","component":"transform"}}"#
/// );
/// # Ok::<(), narvo_ipc::ProtocolError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// Which entities the world holds, in canonical order.
    ListEntities,
    /// Every component one entity carries, with the registry's text for each.
    GetEntity {
        /// The entity to read.
        entity: EntityName,
    },
    /// One named component of one entity.
    GetComponent {
        /// The entity to read.
        entity: EntityName,
        /// The component's stable registry name, such as `transform`.
        component: String,
    },
    /// Writes one named component of one entity.
    ///
    /// The value is the registry's own RON, exactly as [`Response::GetComponent`]
    /// hands it back — ADR-0030's rule read in the other direction, so a value
    /// that came out of this protocol can be sent straight back into it, bit for
    /// bit and including the three floats JSON has no number for.
    ///
    /// **It inserts as well as replaces.** The registry's writing path ends in
    /// `World::insert`, which adds the component when the entity carries none, so
    /// a write can change an entity's *shape* and not only a value — and an
    /// entity's shape is in the canonical dump. The answer says which of the two
    /// happened rather than leaving it to be inferred: `previous` is `None`
    /// exactly when the write added the component. Measured in M6.3b, not assumed.
    SetComponent {
        /// The entity to write to.
        entity: EntityName,
        /// The component's stable registry name, such as `transform`.
        component: String,
        /// The registry's own text for the new value.
        value: String,
    },
    /// Grants the run more ticks than it was going to take.
    ///
    /// The one command that is about the *run* rather than about the world: it
    /// touches no entity, changes no component, and is therefore not a write in
    /// D19's sense — it does not cut a band. What it changes is how far the run
    /// goes, and the band's own tick count follows it, so a recording of an
    /// extended run still describes the run that happened.
    ///
    /// **It adds rather than sets.** Two `step` commands of one tick each grant
    /// two ticks, which is what makes "step again" mean stepping again.
    ///
    /// **What happens when the budget runs out is the hinge to M6.3d.** Today the
    /// run ends, which is what a tick budget has always meant. M6.3d turns
    /// exhaustion into a wait, and this crate is contractually neutral on that:
    /// nothing here says whether a run that has used its grant stops or blocks.
    Step {
        /// How many ticks to add to the run's budget.
        ticks: u64,
    },
    /// Replaces the running world with the one a scene file describes.
    ///
    /// **A reconstitution, in ADR-0022's sense**: the file is loaded fresh and
    /// the world that was running is discarded whole, rather than being patched
    /// towards the file's contents. What that costs and what it buys is that
    /// ADR's, and this crate neither performs it nor knows when in a tick it
    /// lands.
    ///
    /// **The path and nothing else.** Which components the world ends up with,
    /// which systems run over it and how many entities it holds are all in the
    /// file (ADR-0018), so a field here for any of them could only ever
    /// contradict it.
    LoadScene {
        /// The scene file, relative to the directory the run was started in.
        ///
        /// Relative because a recording's scene anchor is (ADR-0019: an absolute
        /// path "would travel wrong"), and the answer hands back the anchor's own
        /// normal form, so a client can see exactly which file was taken.
        path: String,
    },
    /// Replaces the run with a replay of a recording.
    ///
    /// **The run becomes the run the file describes**, mode, seed, length and
    /// input all: a recording carries every one of them and a second opinion
    /// about any of them could only disagree, which is the reading
    /// `--replay`'s own command line already takes.
    ///
    /// **It is the one command that puts a run into the state the other four
    /// are refused in.** A replay reproduces; a command that changed what it
    /// reproduces would leave it reproducing nothing, and the engine refuses
    /// each of them by name. This crate does not spell that rule — nothing here
    /// executes anything — but it is why a client that starts a replay should
    /// expect its next `set` to come back as an error.
    Replay {
        /// The recording to replay, relative to the run's working directory.
        path: String,
    },
    /// The whole world as one canonical dump.
    ///
    /// **A unit variant, like [`ListEntities`](Self::ListEntities), and for the
    /// same reason: there is nothing to say.** A dump is of the world, all of it,
    /// in the one canonical form there is. An argument narrowing it would be a
    /// second answer to a question `canonical_dump` has already answered, and a
    /// narrowed dump is not one — `--expect` compares against a whole dump, and a
    /// half of one would fail against it for a reason that has nothing to do with
    /// the simulation.
    ///
    /// # Why this exists
    ///
    /// M6.7a measured that an agent can assemble something *dump-shaped* out of
    /// `list_entities` and `get_entity`, and that what it assembles is its own
    /// text rather than `canonical_dump`'s: the header line, the two-space indent
    /// and the order within an entity all belong to that function. So the repro
    /// runner would reject it, and the gap M6 has to close would be moved rather
    /// than shut. This carries the engine's own bytes instead — ADR-0030's rule,
    /// applied to a whole world rather than to one component value.
    Dump,
}

impl Request {
    /// Renders this request as one line of JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        // Infallible for this type: every field is a `String`, a `Vec` or an
        // `EntityName`, none of which can fail to serialize, and there is no
        // float anywhere in the protocol for `serde_json` to refuse.
        serde_json::to_string(self).expect("a request holds nothing that can fail to serialize")
    }

    /// Reads a request back out of JSON.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Request`] if `text` is not one, carrying the position
    /// and `serde_json`'s own description of what it expected instead.
    pub fn from_json(text: &str) -> Result<Self, ProtocolError> {
        serde_json::from_str(text).map_err(|source| ProtocolError::request(&source))
    }
}

/// What the engine answers.
///
/// One variant per [`Request`] variant, named the same and spelled the same on
/// the wire, plus [`Error`](Response::Error). That pairing is not decoration:
/// `every_request_has_an_answer_with_the_same_tag` holds it, so a command added
/// without an answer is a red test rather than a protocol with a hole in it.
///
/// # Component values are the registry's own bytes
///
/// A component crosses as the exact string
/// `ComponentRegistry::serialize_component` produced — RON, inside a JSON
/// string. Nothing here parses it, re-renders it or converts it. ADR-0030
/// records why, and the short form is that it is the only measured path that
/// carries what the state hash carries: `serde_json` has no representation for
/// `NaN` or either infinity and writes `null` for all three, which is a silent
/// loss on the way out and a hard failure on the way back.
///
/// The consequence is stated rather than hidden: a client has to parse RON to
/// read a component value.
///
/// # Every answer that came from a world says when it was true
///
/// `ticks_run` is on all of them and on [`Error`](Self::Error) it is not, which
/// is the one exception and the reason for it is measurable: a malformed line is
/// refused by the transport before any world is consulted
/// (`narvo-app`'s `transport.rs`), so an error can exist with no moment at all
/// and a number there would sometimes be a fabrication.
///
/// **It is `ticks_run` and not a tick index**, because the consumer is
/// `--ticks N` on the command line: an answer that says `ticks_run: 7` is an
/// answer about the world a run of seven ticks ends in. ADR-0036 records why the
/// number rides in the answer rather than in a command of its own — measured,
/// four sequential reads of one moving component came back with four different
/// values, so a tick asked for separately is a tick from a different moment.
///
/// # On the wire
///
/// ```json
/// {"list_entities":{"entities":["0v1","1v1"],"ticks_run":7}}
/// {"get_entity":{"entity":"3v1","components":[{"name":"layer","value":"(depth:0.5)"}],"ticks_run":7}}
/// {"get_component":{"entity":"3v1","component":"layer","value":"(depth:0.5)","ticks_run":7}}
/// {"set_component":{"entity":"3v1","component":"layer","previous":"(depth:0.25)","ticks_run":7}}
/// {"step":{"granted":9,"ticks_run":7}}
/// {"load_scene":{"path":"scenes/a.ron","digest":"e3b0c442…","entities":4,"ticks_run":7}}
/// {"replay":{"path":"bug.rec","mode":"input","seed":1,"ticks":600,"ticks_run":7}}
/// {"dump":{"state":"entities 2\nentity 0v1\n  layer (depth:0.5)\n","ticks_run":7}}
/// {"error":{"message":"no entity 3v1"}}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    /// The entities the world holds.
    ListEntities {
        /// In the world's canonical order — ascending by slot, then by
        /// generation. Carried in the order it was given and never sorted here.
        entities: Vec<EntityName>,
        /// How many ticks had run when this was answered.
        ticks_run: u64,
    },
    /// Everything one entity carries.
    GetEntity {
        /// The entity that was read.
        entity: EntityName,
        /// One entry per component the entity has, in the registry's canonical
        /// order — ascending by stable name. A component the entity does not
        /// carry is left out rather than written as absent, which is the same
        /// rule `canonical_dump` follows.
        components: Vec<ComponentValue>,
        /// How many ticks had run when this was answered.
        ticks_run: u64,
    },
    /// One named component of one entity.
    GetComponent {
        /// The entity that was read.
        entity: EntityName,
        /// The component's stable registry name.
        component: String,
        /// The registry's text for it, or `None` when the entity is alive but
        /// does not carry this component.
        ///
        /// `null` on the wire, and this is the one place a JSON `null` means
        /// something in this protocol: an absence, exactly as
        /// `ComponentInfo::serialize` returns `Ok(None)` for it. It is not the
        /// `null` `serde_json` writes for a non-finite float — no float ever
        /// reaches `serde_json` here, which is the point of ADR-0030.
        value: Option<String>,
        /// How many ticks had run when this was answered.
        ticks_run: u64,
    },
    /// One named component was written.
    SetComponent {
        /// The entity that was written to.
        entity: EntityName,
        /// The component's stable registry name.
        component: String,
        /// The registry's text for what was there before, or `None` when the
        /// entity did not carry this component and the write added it.
        ///
        /// The second `null` in this protocol that means something, and it means
        /// something different from [`GetComponent`](Self::GetComponent)'s: there
        /// it says "there is none", here it says "there was none, and now there
        /// is". Both are the same absence — `ComponentInfo::serialize` returning
        /// `Ok(None)` — read at two different moments.
        ///
        /// It is the whole of what a caller learns about the write beyond that it
        /// happened. It cannot distinguish a write that stored a new value from
        /// one that stored the value already there; nothing outside the world
        /// can, which M6.3a measured from the other side.
        previous: Option<String>,
        /// How many ticks had run when this was answered.
        ///
        /// **On a write it is also where D19 cut the band**, which is a fact a
        /// client had no way to learn before M6.7b: a cut band is
        /// byte-indistinguishable from an ordinary recording of a shorter run
        /// (ADR-0032), so the only place the cut point can be said is here.
        ticks_run: u64,
    },
    /// The run's budget was raised.
    Step {
        /// How many ticks the run will take in total, after this command.
        ///
        /// The total rather than the increment, because the increment is what the
        /// request already said and an answer that only echoed it would tell a
        /// client nothing it did not know.
        ///
        /// It is a budget and not a position. **Since M6.7b the position is
        /// beside it**, so "how many ticks are left" is `granted - ticks_run` and
        /// a client no longer has to guess. That is the alternative M6.3c
        /// rejected for want of a consumer, redeemed at the condition it named.
        granted: u64,
        /// How many ticks had run when this was answered.
        ticks_run: u64,
    },
    /// A scene file constituted the running world.
    ///
    /// The three fields are what a client cannot work out for itself and would
    /// otherwise have to ask three more questions to learn: **which** file was
    /// taken, **which bytes** it had, and that the world it produced is not
    /// empty.
    LoadScene {
        /// The scene's path in the normal form a recording's anchor uses —
        /// relative, forward slashes, whatever the platform (ADR-0019).
        path: String,
        /// SHA-256 of the file's bytes, lower-case hex.
        ///
        /// The same digest a recording's `scene-sha256` line carries, from the
        /// same single read the world was built from. It is what lets a client
        /// say *which* version of a file it is now looking at, which a path
        /// alone cannot — the whole reason ADR-0019 hashes at all.
        digest: String,
        /// How many entities the loaded world holds.
        entities: u32,
        /// How many ticks had run when this was answered.
        ///
        /// The counter runs on across a load, so this is where the new world
        /// starts from rather than zero — and it is where the band was cut.
        ticks_run: u64,
    },
    /// The run is now a replay of a recording.
    ///
    /// Everything here was read out of the file rather than supplied by the
    /// asker, which is the point of answering with it: a client that sends a
    /// path learns what the file actually said the run was.
    Replay {
        /// The recording's path, exactly as it was asked for.
        path: String,
        /// Which simulation it is of, by the name the command line uses.
        ///
        /// A string rather than an enum: the mode vocabulary is the runner's
        /// (`narvo-app`'s `sim::Mode`), and naming it here would put a
        /// composition-root decision into a leaf crate — the same layering
        /// ADR-0030 keeps `EntityId` out for.
        mode: String,
        /// The seed the recorded run was made with.
        seed: u64,
        /// How many ticks it covers, which is how long the run now is.
        ticks: u64,
        /// How many ticks had run when this was answered.
        ///
        /// **Of the run that is now over**, not of the replay: a replay starts
        /// its count again at zero, and this answer is given at the last moment
        /// of the run it replaces. Saying so is the point — a client that read
        /// `ticks_run: 40` here knows the forty ticks it was watching are gone.
        ticks_run: u64,
    },
    /// The whole world as one canonical dump.
    ///
    /// **The bytes are `canonical_dump`'s, unparsed and unconverted**, which is
    /// ADR-0030's rule applied to a world rather than to one component value: the
    /// engine has exactly one text for a world, and a second rendering here would
    /// be a second opinion about a format that already has one.
    ///
    /// The property that makes it worth having is byte-identity with what
    /// `narvo --dump` writes to stdout. That is what lets an agent put this in a
    /// file and hand it to `--expect` (ADR-0035), and it is asserted rather than
    /// assumed — in `narvo-app`, against the command-line path, which is the
    /// unmoved reference because nothing in M6.7b touched it.
    Dump {
        /// The canonical dump, exactly as `canonical_dump` produced it.
        ///
        /// Including its trailing newline. A JSON string carries `\n` without
        /// trouble, so nothing here trims, joins or re-wraps: the bytes that come
        /// out of the engine are the bytes that go on the wire.
        state: String,
        /// How many ticks had run when this was answered.
        ///
        /// **The number that makes the dump usable.** A repro is
        /// `--ticks <ticks_run> --expect <this state>`, so an answer without it
        /// would be a state nobody could say when to look for.
        ticks_run: u64,
    },
    /// The request could not be answered.
    ///
    /// The message is free text and this crate writes none of it. What can go
    /// wrong answering a request is a property of executing one, and nothing
    /// here executes anything — M6.3 owns both the execution and the wording,
    /// and whether this grows a taxonomy is its call to make with real failures
    /// in hand.
    ///
    /// **The one response with no `ticks_run`, and the reason is measured rather
    /// than stylistic.** A malformed line never reaches a world at all — the
    /// transport refuses it and answers with this — so there is no moment to
    /// report. A field here would be absent on some errors and present on
    /// others, or invented on the ones that have none; saying "an error is the
    /// answer that has no moment" is the honest shape of that.
    Error {
        /// What went wrong.
        message: String,
    },
}

impl Response {
    /// How many ticks had run when this was answered, if it came from a world.
    ///
    /// `None` for exactly one variant, [`Error`](Self::Error), and the match
    /// below is exhaustive so a response added without a moment has to say so
    /// here rather than inherit an answer by omission.
    #[must_use]
    pub fn ticks_run(&self) -> Option<u64> {
        match self {
            Self::ListEntities { ticks_run, .. }
            | Self::GetEntity { ticks_run, .. }
            | Self::GetComponent { ticks_run, .. }
            | Self::SetComponent { ticks_run, .. }
            | Self::Step { ticks_run, .. }
            | Self::LoadScene { ticks_run, .. }
            | Self::Replay { ticks_run, .. }
            | Self::Dump { ticks_run, .. } => Some(*ticks_run),
            Self::Error { .. } => None,
        }
    }

    /// Renders this response as one line of JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        // Infallible for the same reason `Request::to_json` is.
        serde_json::to_string(self).expect("a response holds nothing that can fail to serialize")
    }

    /// Reads a response back out of JSON.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Response`] if `text` is not one, carrying the position
    /// and `serde_json`'s own description of what it expected instead.
    pub fn from_json(text: &str) -> Result<Self, ProtocolError> {
        serde_json::from_str(text).map_err(|source| ProtocolError::response(&source))
    }
}

/// One component of one entity: its stable name, and the registry's text for it.
///
/// The `value` is opaque here on purpose. It is whatever
/// `ComponentRegistry::serialize_component` returned, carried across unchanged,
/// and this crate neither validates it nor knows what shape it has — which is
/// what makes the protocol component-open in the same sense ADR-0018 makes the
/// scene format component-open: a caller may register whatever it likes and this
/// vocabulary does not have to grow a case for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentValue {
    /// The component's stable registry name, such as `transform`.
    pub name: String,
    /// The registry's own text for it, byte for byte.
    pub value: String,
}

impl ComponentValue {
    /// Pairs a stable name with the registry's text for that component.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentValue, Request, Response};
    use crate::EntityName;
    use std::num::NonZeroU32;

    fn name(index: u32, generation: u32) -> EntityName {
        EntityName::new(
            index,
            NonZeroU32::new(generation).expect("test generations are non-zero"),
        )
    }

    /// The tag `serde_json` wrote, whichever of the two shapes it used.
    ///
    /// A unit variant is a bare JSON string; every other variant is an object
    /// with exactly one key. Both are the externally tagged representation.
    fn tag(json: &str) -> String {
        let value: serde_json::Value = serde_json::from_str(json).expect("this crate wrote it");
        match value {
            serde_json::Value::String(tag) => tag,
            serde_json::Value::Object(map) => {
                assert_eq!(
                    map.len(),
                    1,
                    "an externally tagged value has one key: {json}"
                );
                map.keys()
                    .next()
                    .expect("just checked there is one")
                    .clone()
            }
            other => panic!("unexpected shape on the wire: {other}"),
        }
    }

    /// Every request variant, with the wire text it is contracted to produce.
    ///
    /// The `match` below has no `_` arm, which is what makes this an intent gate
    /// rather than a list that can fall behind: a new variant stops this file
    /// compiling, so no command can be added without someone writing an arm here.
    /// The length assertion covers the other direction, a sample quietly dropped
    /// from the list.
    ///
    /// **What this gate does not catch, measured in M6.1 rather than reasoned
    /// about:** a variant added *with* an arm but *without* a sample leaves this
    /// test green, because the loop only walks the samples. What catches that
    /// case is `an_unknown_command_lists_every_command_there_is` in `error.rs` —
    /// `serde_json`'s unknown-variant message enumerates every variant, and that
    /// wording is pinned. So there are two gates, they are independent, and
    /// neither is this assertion on `samples.len()`.
    #[test]
    fn every_request_variant_has_the_wire_text_it_is_contracted_to() {
        let samples = [
            Request::ListEntities,
            Request::GetEntity { entity: name(3, 1) },
            Request::GetComponent {
                entity: name(3, 1),
                component: "transform".to_owned(),
            },
            Request::SetComponent {
                entity: name(3, 1),
                component: "layer".to_owned(),
                value: "(depth:0.5)".to_owned(),
            },
            Request::Step { ticks: 5 },
            Request::LoadScene {
                path: "scenes/a.ron".to_owned(),
            },
            Request::Replay {
                path: "bug.rec".to_owned(),
            },
            Request::Dump,
        ];

        for request in &samples {
            let expected = match request {
                Request::ListEntities => r#""list_entities""#,
                Request::GetEntity { .. } => r#"{"get_entity":{"entity":"3v1"}}"#,
                Request::GetComponent { .. } => {
                    r#"{"get_component":{"entity":"3v1","component":"transform"}}"#
                }
                Request::SetComponent { .. } => {
                    r#"{"set_component":{"entity":"3v1","component":"layer","value":"(depth:0.5)"}}"#
                }
                Request::Step { .. } => r#"{"step":{"ticks":5}}"#,
                Request::LoadScene { .. } => r#"{"load_scene":{"path":"scenes/a.ron"}}"#,
                Request::Replay { .. } => r#"{"replay":{"path":"bug.rec"}}"#,
                Request::Dump => r#""dump""#,
            };

            assert_eq!(request.to_json(), expected);
            assert_eq!(
                &Request::from_json(expected).expect("this crate just wrote it"),
                request
            );
        }

        assert_eq!(samples.len(), 8, "a variant was added without a sample");
    }

    /// The same gate for the answering half.
    #[test]
    fn every_response_variant_has_the_wire_text_it_is_contracted_to() {
        let samples = [
            Response::ListEntities {
                entities: vec![name(0, 1), name(1, 2)],
                ticks_run: 7,
            },
            Response::GetEntity {
                entity: name(3, 1),
                components: vec![ComponentValue::new("layer", "(depth:0.5)")],
                ticks_run: 7,
            },
            Response::GetComponent {
                entity: name(3, 1),
                component: "layer".to_owned(),
                value: Some("(depth:0.5)".to_owned()),
                ticks_run: 7,
            },
            Response::SetComponent {
                entity: name(3, 1),
                component: "layer".to_owned(),
                previous: Some("(depth:0.25)".to_owned()),
                ticks_run: 7,
            },
            Response::Step {
                granted: 9,
                ticks_run: 7,
            },
            Response::LoadScene {
                path: "scenes/a.ron".to_owned(),
                digest: "e3b0c442".to_owned(),
                entities: 4,
                ticks_run: 7,
            },
            Response::Replay {
                path: "bug.rec".to_owned(),
                mode: "input".to_owned(),
                seed: 1,
                ticks: 600,
                ticks_run: 7,
            },
            Response::Dump {
                state: "entities 1\nentity 0v1\n  layer (depth:0.5)\n".to_owned(),
                ticks_run: 7,
            },
            Response::Error {
                message: "no entity 3v1".to_owned(),
            },
        ];

        for response in &samples {
            let expected = match response {
                Response::ListEntities { .. } => {
                    r#"{"list_entities":{"entities":["0v1","1v2"],"ticks_run":7}}"#
                }
                Response::GetEntity { .. } => {
                    r#"{"get_entity":{"entity":"3v1","components":[{"name":"layer","value":"(depth:0.5)"}],"ticks_run":7}}"#
                }
                Response::GetComponent { .. } => {
                    r#"{"get_component":{"entity":"3v1","component":"layer","value":"(depth:0.5)","ticks_run":7}}"#
                }
                Response::SetComponent { .. } => {
                    r#"{"set_component":{"entity":"3v1","component":"layer","previous":"(depth:0.25)","ticks_run":7}}"#
                }
                Response::Step { .. } => r#"{"step":{"granted":9,"ticks_run":7}}"#,
                Response::LoadScene { .. } => {
                    r#"{"load_scene":{"path":"scenes/a.ron","digest":"e3b0c442","entities":4,"ticks_run":7}}"#
                }
                Response::Replay { .. } => {
                    r#"{"replay":{"path":"bug.rec","mode":"input","seed":1,"ticks":600,"ticks_run":7}}"#
                }
                // The newlines a canonical dump carries, as JSON spells them.
                // Nothing trims or re-wraps them, which is the whole property
                // `--expect` rests on.
                Response::Dump { .. } => {
                    r#"{"dump":{"state":"entities 1\nentity 0v1\n  layer (depth:0.5)\n","ticks_run":7}}"#
                }
                Response::Error { .. } => r#"{"error":{"message":"no entity 3v1"}}"#,
            };

            assert_eq!(response.to_json(), expected);
            assert_eq!(
                &Response::from_json(expected).expect("this crate just wrote it"),
                response
            );
        }

        assert_eq!(samples.len(), 9, "a variant was added without a sample");
    }

    /// The one response that carries no moment, asserted rather than implied.
    ///
    /// A malformed line is refused by the transport before any world is
    /// consulted, so an error can exist with no tick to report. Every other
    /// response came from a world and says which one.
    #[test]
    fn every_response_but_an_error_says_how_many_ticks_had_run() {
        let with_moment = [
            r#"{"list_entities":{"entities":[],"ticks_run":7}}"#,
            r#"{"get_entity":{"entity":"3v1","components":[],"ticks_run":7}}"#,
            r#"{"get_component":{"entity":"3v1","component":"layer","value":null,"ticks_run":7}}"#,
            r#"{"set_component":{"entity":"3v1","component":"layer","previous":null,"ticks_run":7}}"#,
            r#"{"step":{"granted":9,"ticks_run":7}}"#,
            r#"{"load_scene":{"path":"a.ron","digest":"e3","entities":4,"ticks_run":7}}"#,
            r#"{"replay":{"path":"b.rec","mode":"input","seed":1,"ticks":600,"ticks_run":7}}"#,
            r#"{"dump":{"state":"entities 0\n","ticks_run":7}}"#,
        ];

        for text in with_moment {
            let response = Response::from_json(text).expect("this crate just wrote it");
            assert_eq!(
                response.ticks_run(),
                Some(7),
                "{text} has to carry its moment"
            );
            // And the field is required: the same text without it is refused
            // rather than defaulted to zero, which would be a moment nobody
            // observed.
            let without = text.replace(r#","ticks_run":7"#, "");
            assert!(
                Response::from_json(&without).is_err(),
                "a response without its moment must not parse: {without}"
            );
        }

        let error = Response::from_json(r#"{"error":{"message":"no entity 3v1"}}"#)
            .expect("this crate just wrote it");
        assert_eq!(error.ticks_run(), None);
    }

    /// A command with no answer would be a protocol with a hole in it.
    ///
    /// The tags are compared rather than the variant names, because the tag is
    /// what a client sees. `error` is the one response with no request, which is
    /// asserted here rather than left as an unstated exception.
    #[test]
    fn every_request_has_an_answer_with_the_same_tag() {
        let requests = [
            Request::ListEntities,
            Request::GetEntity { entity: name(3, 1) },
            Request::GetComponent {
                entity: name(3, 1),
                component: "transform".to_owned(),
            },
            Request::SetComponent {
                entity: name(3, 1),
                component: "transform".to_owned(),
                value: String::new(),
            },
            Request::Step { ticks: 1 },
            Request::LoadScene {
                path: String::new(),
            },
            Request::Replay {
                path: String::new(),
            },
            Request::Dump,
        ];
        let responses = [
            Response::ListEntities {
                entities: Vec::new(),
                ticks_run: 0,
            },
            Response::GetEntity {
                entity: name(3, 1),
                components: Vec::new(),
                ticks_run: 0,
            },
            Response::GetComponent {
                entity: name(3, 1),
                component: "transform".to_owned(),
                value: None,
                ticks_run: 0,
            },
            Response::SetComponent {
                entity: name(3, 1),
                component: "transform".to_owned(),
                previous: None,
                ticks_run: 0,
            },
            Response::Step {
                granted: 0,
                ticks_run: 0,
            },
            Response::LoadScene {
                path: String::new(),
                digest: String::new(),
                entities: 0,
                ticks_run: 0,
            },
            Response::Replay {
                path: String::new(),
                mode: String::new(),
                seed: 0,
                ticks: 0,
                ticks_run: 0,
            },
            Response::Dump {
                state: String::new(),
                ticks_run: 0,
            },
            Response::Error {
                message: String::new(),
            },
        ];

        let asked: Vec<String> = requests.iter().map(|r| tag(&r.to_json())).collect();
        let answered: Vec<String> = responses.iter().map(|r| tag(&r.to_json())).collect();

        for command in &asked {
            assert!(
                answered.contains(command),
                "the request `{command}` has no response variant with the same tag"
            );
        }
        assert_eq!(
            answered.len(),
            asked.len() + 1,
            "the answering half has exactly one variant that answers no request, `error`"
        );
        assert_eq!(answered.last().map(String::as_str), Some("error"));
    }

    /// An absent component is `null`, and it comes back as an absence.
    #[test]
    fn a_component_the_entity_does_not_carry_crosses_as_null() {
        let response = Response::GetComponent {
            entity: name(3, 1),
            component: "sprite".to_owned(),
            value: None,
            ticks_run: 7,
        };

        assert_eq!(
            response.to_json(),
            r#"{"get_component":{"entity":"3v1","component":"sprite","value":null,"ticks_run":7}}"#
        );
        assert_eq!(
            Response::from_json(&response.to_json()).expect("just written"),
            response
        );
    }

    /// The order of a component list is carried, not recomputed.
    ///
    /// The canonical order is the registry's and is established before a
    /// response is built; a protocol that sorted here would be a second opinion
    /// about an order that already has one.
    #[test]
    fn a_component_list_crosses_in_the_order_it_was_given() {
        let backwards = Response::GetEntity {
            entity: name(3, 1),
            components: vec![
                ComponentValue::new("transform", "(x:0.0)"),
                ComponentValue::new("layer", "(depth:0.5)"),
            ],
            ticks_run: 7,
        };

        let crossed = Response::from_json(&backwards.to_json()).expect("just written");
        match crossed {
            Response::GetEntity { components, .. } => {
                let names: Vec<&str> = components.iter().map(|c| c.name.as_str()).collect();
                assert_eq!(names, vec!["transform", "layer"]);
            }
            other => panic!("expected the entity answer, got {other:?}"),
        }
    }

    /// An entity list crosses in the order it was given, too.
    #[test]
    fn an_entity_list_crosses_in_the_order_it_was_given() {
        let response = Response::ListEntities {
            entities: vec![name(9, 1), name(0, 3), name(4, 1)],
            ticks_run: 7,
        };

        assert_eq!(
            Response::from_json(&response.to_json()).expect("just written"),
            response
        );
        assert_eq!(
            response.to_json(),
            r#"{"list_entities":{"entities":["9v1","0v3","4v1"],"ticks_run":7}}"#
        );
    }
}
