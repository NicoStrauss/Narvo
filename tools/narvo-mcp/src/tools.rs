//! What the server offers an agent, and how a call becomes an engine request.
//!
//! # One tool per protocol command, and the curation happened once already
//!
//! Seven tools for `narvo-ipc`'s seven commands, named the way the wire names
//! them. **The alternative — a curated subset, or tools that combine several
//! commands — was rejected because the curation has already been done**: M6.1
//! decided which capabilities belong on the wire and M6.4a added the last two,
//! each with its own reasoning about what a client can and cannot work out for
//! itself. A second selection here would be a second opinion about a decision
//! that has one, and it would put the two vocabularies out of step the first time
//! either moved.
//!
//! # The list does not depend on the run's state, and that is the specification's
//!
//! ADR-0032 makes four of the seven refusable: a run reproducing a recording
//! takes no orders. A tool list that shrank during a replay would be a better
//! list to look at — and MCP forbids it in as many words. `tools/list` "**MUST
//! NOT** vary per-connection or as a side effect of other requests on the
//! connection" (2026-07-28, Server Features/Tools), and starting a replay *is*
//! another request on the connection.
//!
//! So the refusal is a **tool execution error** instead, which is the mechanism
//! MCP names for exactly this: "actionable feedback that language models can use
//! to self-correct". The engine's own sentence already ends "A replay answers
//! questions and takes no orders — let it finish, or start a live run to steer",
//! which is that feedback written in M6.4a before there was anything to read it.
//! The four descriptions below say it in advance as well, so an agent can see the
//! condition before it spends a call finding out.
//!
//! # Descriptions are the product
//!
//! `ProjektPlan.md` §6/M4's rule — error message quality is a feature — read at
//! the one place an agent looks *before* it makes a mistake rather than after.

use narvo_ipc::{EntityName, Request};
use serde::Serialize;

/// One tool, in the shape `tools/list` puts it on the wire.
///
/// `title`, `icons`, `outputSchema` and `annotations` are all optional in the
/// specification and none is here. That is a named limit rather than an
/// oversight: an `outputSchema` would have to describe seven response shapes and
/// would then be a second statement of `narvo-ipc`'s `Response`, free to drift
/// from it. What a call returns is the engine's own answer, verbatim — see
/// [`crate::server`].
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    /// The tool's name, which is the protocol command's own tag.
    pub name: &'static str,
    /// What it does, what it answers with, and when it is refused.
    pub description: &'static str,
    /// JSON Schema for the arguments, in the 2020-12 dialect MCP defaults to.
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// How an entity is spelled everywhere in this engine, quoted into a schema.
const ENTITY_DESCRIPTION: &str = "An entity name as this engine spells one: the slot index, the letter v, and the \
     generation — for example 3v1. Names come from list_entities; a name from before a \
     despawn never addresses whatever took its place.";

/// How a component value is spelled, quoted into a schema.
///
/// The consequence ADR-0030 states, said where an agent will meet it.
const VALUE_DESCRIPTION: &str = "The component's value as the engine's own registry writes it: RON, carried inside \
     this JSON string. It is byte-for-byte what get_component hands back, so a value \
     read out of this server can be sent straight back into it — including the three \
     floats JSON has no number for.";

/// What this server offers, in a fixed order.
///
/// **Deterministic, which the specification asks for**: "Servers **SHOULD** return
/// tools in a deterministic order". The order is the reading tools, then the
/// writing one, then the three that steer the run — which is also the order
/// `narvo-ipc`'s own `Request` declares them in, so the two lists are read
/// side by side.
#[must_use]
pub fn catalogue() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_entities",
            description: "List every entity in the running world, in the engine's canonical \
                          order. Answers with the same names the canonical state dump uses, \
                          and is the way to find a name for the other tools. Allowed during a \
                          replay: reading a world changes nothing about what it reproduces.",
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false
            }),
        },
        Tool {
            name: "get_entity",
            description: "Read every component one entity carries, in the registry's canonical \
                          order. A component the entity does not carry is left out rather than \
                          reported as absent. Allowed during a replay.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "entity": { "type": "string", "description": ENTITY_DESCRIPTION }
                },
                "required": ["entity"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "get_component",
            description: "Read one named component of one entity. The answer's value is null \
                          exactly when the entity is alive and does not carry that component. \
                          Component names are the engine's stable registry names, such as \
                          transform or layer; an unknown one is answered with the list of the \
                          names there are. Allowed during a replay.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "entity": { "type": "string", "description": ENTITY_DESCRIPTION },
                    "component": {
                        "type": "string",
                        "description": "The component's stable registry name, such as transform."
                    }
                },
                "required": ["entity", "component"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "set_component",
            description: "Write one named component of one entity. It inserts as well as \
                          replaces: the answer's previous field is null exactly when the write \
                          added a component the entity did not carry. A run that accepts a \
                          write has its recording cut at that tick, because a recording \
                          promises to reproduce only what came before. Refused during a \
                          replay.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "entity": { "type": "string", "description": ENTITY_DESCRIPTION },
                    "component": {
                        "type": "string",
                        "description": "The component's stable registry name, such as layer."
                    },
                    "value": { "type": "string", "description": VALUE_DESCRIPTION }
                },
                "required": ["entity", "component", "value"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "step",
            description: "Grant the run more ticks than it was going to take. It adds rather \
                          than sets, so asking for one tick twice runs two. The answer is the \
                          run's total budget afterwards, not the increment. A run that has used \
                          its budget waits for a command, so this is also what releases a \
                          waiting run. Refused during a replay, whose length is its \
                          recording's.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "ticks": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "How many ticks to add to the run's budget."
                    }
                },
                "required": ["ticks"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "load_scene",
            description: "Replace the running world with the one a scene file describes. The \
                          world is constituted afresh rather than patched towards the file, and \
                          the run's tick counter carries on where it was. The answer names the \
                          file in its normal form, the SHA-256 of the bytes that were taken, \
                          and how many entities the new world holds. A scene that does not load \
                          leaves the running world untouched. Refused during a replay.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The scene file, relative to the directory the engine \
                                        was started in. An absolute path is refused."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "replay",
            description: "Replace the run with a replay of a recording. The run becomes the run \
                          the file describes — mode, seed, length and input all — and the \
                          answer says what the file said those were. Afterwards the run takes \
                          no orders: set_component, step, load_scene and replay are all \
                          refused until it ends, while the reading tools keep working. Refused \
                          during a replay, for that same reason.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The recording, relative to the directory the engine was \
                                        started in."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        },
        Tool {
            name: "dump",
            description: "Read the whole world as one canonical state dump — the same text \
                          `narvo --dump` writes, byte for byte. This is what a repro test is \
                          made of: write the answer's state to a file, then run \
                          `narvo --replay <recording> --ticks <the answer's ticks_run> \
                          --expect <that file>`, and the runner says whether the state comes \
                          back. Every answer from this server carries ticks_run, which is how \
                          many ticks had run when it was given; for this one it is the number \
                          that --ticks needs. Refused only if the world holds a component the \
                          registry does not know, which is the same case the command line \
                          fails in. Allowed during a replay — reading changes nothing about \
                          what it reproduces.",
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false
            }),
        },
    ]
}

/// Turns a call into the engine request it names, or says what is wrong with it.
///
/// # Why a bad argument is not a protocol error
///
/// MCP splits failures in two, and an argument that does not match a tool's own
/// `inputSchema` falls on the *tool execution* side: "Input validation errors
/// (e.g., date in wrong format, value out of range)" are listed there, and
/// clients "**SHOULD** provide tool execution errors to language models to enable
/// self-correction". A malformed `CallToolRequest` — no `name`, or `arguments`
/// that are not an object — is the protocol error, and that is checked one layer
/// up in [`crate::server`].
///
/// So this returns a plain `String` for the failure: it becomes the text of a
/// result with `isError: true`, not a JSON-RPC error.
///
/// # Errors
///
/// A sentence naming the argument and what is wrong with it, for an unknown tool
/// or an argument that is missing or of the wrong type.
pub fn request_for(name: &str, arguments: &serde_json::Value) -> Result<Request, String> {
    match name {
        "list_entities" => Ok(Request::ListEntities),
        "get_entity" => Ok(Request::GetEntity {
            entity: entity(arguments)?,
        }),
        "get_component" => Ok(Request::GetComponent {
            entity: entity(arguments)?,
            component: text(arguments, "component")?,
        }),
        "set_component" => Ok(Request::SetComponent {
            entity: entity(arguments)?,
            component: text(arguments, "component")?,
            value: text(arguments, "value")?,
        }),
        "step" => Ok(Request::Step {
            ticks: count(arguments, "ticks")?,
        }),
        "load_scene" => Ok(Request::LoadScene {
            path: text(arguments, "path")?,
        }),
        "replay" => Ok(Request::Replay {
            path: text(arguments, "path")?,
        }),
        "dump" => Ok(Request::Dump),
        other => Err(unknown(other)),
    }
}

/// The message an unknown tool name gets.
///
/// Split out because it is reported through the *protocol* mechanism rather than
/// this one — MCP lists "Unknown tool" under protocol errors, with `-32602` and
/// the example message `Unknown tool: invalid_tool_name`.
#[must_use]
pub fn unknown(name: &str) -> String {
    let known: Vec<&str> = catalogue().into_iter().map(|tool| tool.name).collect();
    format!(
        "no tool here is called \"{name}\"; this server offers {}",
        known.join(", ")
    )
}

/// One string argument.
fn text(arguments: &serde_json::Value, field: &str) -> Result<String, String> {
    match arguments.get(field) {
        Some(serde_json::Value::String(value)) => Ok(value.clone()),
        Some(other) => Err(format!(
            "\"{field}\" must be a string, and this one is {other}"
        )),
        None => Err(format!("this tool needs a \"{field}\" argument")),
    }
}

/// One non-negative whole-number argument.
///
/// `u64` rather than "any number": a tick count is a count. `serde_json` reports
/// a whole number as `as_u64` only when it fits, so a negative one and a
/// fractional one are both refused here rather than silently truncated.
fn count(arguments: &serde_json::Value, field: &str) -> Result<u64, String> {
    match arguments.get(field) {
        Some(value) => value.as_u64().ok_or_else(|| {
            format!("\"{field}\" must be a whole number of zero or more, and this one is {value}")
        }),
        None => Err(format!("this tool needs a \"{field}\" argument")),
    }
}

/// The `entity` argument, parsed into a name this engine recognises.
///
/// **The parse happens here rather than at the engine**, which is what lets the
/// message say what a name looks like. It is still only a parse: an
/// [`EntityName`] is this crate's word for a slot and a generation and has been
/// checked against no world, which is ADR-0030's second rule and the reason
/// `narvo-app`'s `resolve` looks a name up rather than converting it.
fn entity(arguments: &serde_json::Value) -> Result<EntityName, String> {
    let spelled = text(arguments, "entity")?;

    spelled
        .parse::<EntityName>()
        .map_err(|cause| format!("\"entity\" is not an entity name: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::{Tool, catalogue, request_for, unknown};
    use narvo_ipc::{EntityName, Request};

    fn arguments(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("the tests write well-formed arguments")
    }

    fn name(text: &str) -> EntityName {
        text.parse().expect("the tests write well-formed names")
    }

    /// **Every command the protocol has is offered as a tool**, and the match
    /// below has no `_` arm.
    ///
    /// That is the intent gate the rest of this workspace uses (M6.1's own, and
    /// `narvo-app`'s `answer`): adding a variant to `narvo_ipc::Request` stops
    /// this file compiling, so a command cannot reach the wire without somebody
    /// deciding whether an agent may see it. The length assertion covers the
    /// other direction — a sample quietly dropped from the list.
    #[test]
    fn every_protocol_command_is_offered_as_a_tool() {
        let samples = [
            Request::ListEntities,
            Request::GetEntity {
                entity: name("3v1"),
            },
            Request::GetComponent {
                entity: name("3v1"),
                component: "transform".to_owned(),
            },
            Request::SetComponent {
                entity: name("3v1"),
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

        let offered: Vec<&str> = catalogue().into_iter().map(|tool| tool.name).collect();

        for request in &samples {
            let expected = match request {
                Request::ListEntities => "list_entities",
                Request::GetEntity { .. } => "get_entity",
                Request::GetComponent { .. } => "get_component",
                Request::SetComponent { .. } => "set_component",
                Request::Step { .. } => "step",
                Request::LoadScene { .. } => "load_scene",
                Request::Replay { .. } => "replay",
                Request::Dump => "dump",
            };
            assert!(
                offered.contains(&expected),
                "the command `{expected}` is not offered as a tool"
            );
        }

        assert_eq!(samples.len(), 8, "a command was added without a sample");
        assert_eq!(
            offered.len(),
            8,
            "a tool was added that answers no protocol command"
        );
    }

    /// **Each tool builds the request it is named for, and no other.**
    ///
    /// Red edge (a) of M6.5b: a call that produced another command's request
    /// would answer the wrong question with a well-formed answer, and nothing
    /// about the shape of the reply would say so.
    #[test]
    fn each_tool_builds_the_request_it_is_named_for() {
        assert_eq!(
            request_for("list_entities", &arguments("{}")).expect("no arguments needed"),
            Request::ListEntities
        );
        assert_eq!(
            request_for("get_entity", &arguments(r#"{"entity":"3v1"}"#)).expect("well formed"),
            Request::GetEntity {
                entity: name("3v1")
            }
        );
        assert_eq!(
            request_for(
                "get_component",
                &arguments(r#"{"entity":"3v1","component":"layer"}"#)
            )
            .expect("well formed"),
            Request::GetComponent {
                entity: name("3v1"),
                component: "layer".to_owned()
            }
        );
        assert_eq!(
            request_for(
                "set_component",
                &arguments(r#"{"entity":"3v1","component":"layer","value":"(depth:0.5)"}"#)
            )
            .expect("well formed"),
            Request::SetComponent {
                entity: name("3v1"),
                component: "layer".to_owned(),
                value: "(depth:0.5)".to_owned()
            }
        );
        assert_eq!(
            request_for("step", &arguments(r#"{"ticks":9}"#)).expect("well formed"),
            Request::Step { ticks: 9 }
        );
        assert_eq!(
            request_for("load_scene", &arguments(r#"{"path":"scenes/a.ron"}"#))
                .expect("well formed"),
            Request::LoadScene {
                path: "scenes/a.ron".to_owned()
            }
        );
        assert_eq!(
            request_for("replay", &arguments(r#"{"path":"bug.rec"}"#)).expect("well formed"),
            Request::Replay {
                path: "bug.rec".to_owned()
            }
        );
    }

    /// A missing or mistyped argument says which one and what it should be.
    #[test]
    fn a_bad_argument_says_which_one_and_what_it_should_be() {
        assert_eq!(
            request_for("get_entity", &arguments("{}")).expect_err("no entity"),
            "this tool needs a \"entity\" argument"
        );
        assert_eq!(
            request_for("get_entity", &arguments(r#"{"entity":7}"#)).expect_err("not a string"),
            "\"entity\" must be a string, and this one is 7"
        );
        assert_eq!(
            request_for("step", &arguments(r#"{"ticks":-1}"#)).expect_err("negative"),
            "\"ticks\" must be a whole number of zero or more, and this one is -1"
        );
        assert_eq!(
            request_for("step", &arguments(r#"{"ticks":1.5}"#)).expect_err("fractional"),
            "\"ticks\" must be a whole number of zero or more, and this one is 1.5"
        );
        assert_eq!(
            request_for("step", &arguments("{}")).expect_err("no ticks"),
            "this tool needs a \"ticks\" argument"
        );
    }

    /// **A name that is not a name is refused here**, with what one looks like.
    #[test]
    fn an_entity_that_is_not_a_name_is_refused_with_the_parsers_own_words() {
        let refused =
            request_for("get_entity", &arguments(r#"{"entity":"three"}"#)).expect_err("not a name");

        assert!(
            refused.starts_with("\"entity\" is not an entity name: "),
            "{refused}"
        );
    }

    /// An unknown tool names what there is.
    #[test]
    fn an_unknown_tool_names_the_tools_there_are() {
        assert_eq!(
            unknown("get_wheather"),
            "no tool here is called \"get_wheather\"; this server offers list_entities, \
             get_entity, get_component, set_component, step, load_scene, replay, dump"
        );
        assert_eq!(
            request_for("get_wheather", &arguments("{}")).expect_err("no such tool"),
            unknown("get_wheather")
        );
    }

    /// **Every tool has a description and a schema an agent can act on.**
    ///
    /// Not a spelling check: the three properties asserted are the ones a client
    /// or a model actually depends on — a name it can call, prose it can read,
    /// and an object schema, which is what MCP requires an `inputSchema` to be
    /// ("**MUST** be a valid JSON Schema object (not `null`)").
    #[test]
    fn every_tool_carries_a_description_and_an_object_schema() {
        for Tool {
            name,
            description,
            input_schema,
        } in catalogue()
        {
            assert!(!name.is_empty());
            assert!(
                description.len() > 80,
                "{name} has a description an agent cannot act on: {description:?}"
            );
            assert_eq!(
                input_schema.get("type").and_then(serde_json::Value::as_str),
                Some("object"),
                "{name}'s schema is not an object schema"
            );
            assert_eq!(
                input_schema
                    .get("additionalProperties")
                    .and_then(serde_json::Value::as_bool),
                Some(false),
                "{name}'s schema accepts arguments it does not describe"
            );

            // **The prose is prose**, and this is the detection half of §9.2's
            // lost-continuation rule at a size `cargo xtask whitespace` cannot
            // reach: these literals are continued with a `\` and five spaces of
            // indent, one short of the six that check looks for, so a `\` lost
            // here would leave a line break and an indent inside a sentence an
            // agent reads — and `serde_json` would escape it rather than refuse
            // it, so nothing else would notice.
            let mut prose = vec![description.to_owned()];
            prose.extend(
                input_schema["properties"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(_, property)| {
                        property["description"].as_str().map(str::to_owned)
                    }),
            );
            for sentence in prose {
                assert!(!sentence.contains('\n'), "{name}: {sentence:?}");
                assert!(!sentence.contains("  "), "{name}: {sentence:?}");
            }
        }
    }

    /// **The four commands a replay refuses say so before they are called.**
    ///
    /// S3's answer in a test: the tool list cannot shrink during a replay
    /// (MCP forbids it), so the condition lives in the descriptions instead, and
    /// an agent that reads them sees it without spending a call.
    #[test]
    fn the_tools_a_replay_refuses_say_so_in_their_descriptions() {
        let refused = ["set_component", "step", "load_scene", "replay"];
        let allowed = ["list_entities", "get_entity", "get_component"];

        for tool in catalogue() {
            if refused.contains(&tool.name) {
                assert!(
                    tool.description.contains("Refused during a replay"),
                    "{} does not say a replay refuses it",
                    tool.name
                );
            }
            if allowed.contains(&tool.name) {
                assert!(
                    tool.description.contains("Allowed during a replay"),
                    "{} does not say a replay allows it",
                    tool.name
                );
            }
        }
    }
}
