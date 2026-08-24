//! The whole of MCP this server speaks, as a function over data.
//!
//! # Where the line between core and shell runs (M6.5b's S2)
//!
//! **Here.** Everything in this file is text in and text out: [`Server::handle`]
//! takes one message and returns the answer to it, and [`pump`] takes whatever
//! bytes arrived and returns whatever bytes should go back. Neither names a file
//! descriptor, a process or a clock.
//!
//! What is left outside is `main.rs`, and it is the only file in this crate that
//! names `std::io`, `std::env`, `std::process` or a socket: it parses arguments,
//! opens a connection to the engine, and moves bytes between `stdin`, [`pump`]
//! and `stdout`. That is three verbs.
//!
//! The division is the workspace's fourth of this shape — M6.1 defined a protocol
//! with no transport, M6.3a an execution seam with no socket, M6.5a a framing with
//! no network — and it buys the same thing each time: the part that can be wrong
//! in an interesting way is the part a test can drive without starting anything.
//!
//! # Which specification, and what is deliberately not implemented
//!
//! MCP **2026-07-28**, and only its `tools` capability. `resources`, `prompts`,
//! `completion`, `logging`, `subscriptions/listen` and every extension are absent
//! and undeclared, which is what capabilities are for — a client that reads
//! `server/discover` learns exactly this. `tools/list` returns every tool in one
//! page and therefore never issues a `nextCursor`.
//!
//! **This server speaks only the modern era.** The 2026-07-28 revision replaced
//! the `initialize` handshake with per-request metadata, and a legacy client's
//! `initialize` is refused here with a message naming the version this server
//! does speak — which is the one thing the specification asks a modern-only
//! server to do for a legacy one, because "legacy clients have no fall-forward
//! mechanism, and this message may be the only diagnostic they can surface to
//! users".
//!
//! # The named limit: no MCP client has ever driven this
//!
//! **Nothing in this repository is an MCP client, nothing in CI is, and no test
//! below or anywhere else establishes that a real agent host can use this
//! server.** That claim needs a client, and a client is a network, a model and a
//! credential — none of which belongs in a verification set whose whole point is
//! that it runs offline and deterministically on two platforms.
//!
//! It is written here rather than left to be discovered because the alternative
//! is worse than the gap: `ProjektPlan.md` §10 carries eight limits of this class,
//! and every one of them exists because somebody preferred a named absence to a
//! test that passes without checking anything. A test that started a server,
//! asked it nothing and asserted that it had not crashed would be the ninth
//! instance and the most expensive outcome this task had available.
//!
//! What **is** checked, and what each of the three is worth:
//!
//! 1. **Every wire shape against the specification's own example JSON.** The
//!    2026-07-28 pages print the whole of a `server/discover` result, a
//!    `tools/list` result, a `tools/call` result, an `UnsupportedProtocolVersion`
//!    error and the two error mechanisms; the tests below compare against those
//!    rather than against a memory of them. That converts "this crate's author
//!    read the specification correctly" into "these bytes are the ones the
//!    specification prints", which is a different and much cheaper claim to
//!    trust.
//! 2. **The whole path through a real process**, in `tests/agent_over_mcp.rs`: a
//!    spawned binary, operating-system pipes, and a TCP connection it opens
//!    itself. That covers the framing at a boundary where the bytes really do
//!    arrive in whatever pieces the kernel chooses.
//! 3. **The engine's own behaviour**, in `narvo-app`'s `agent_socket.rs`, which
//!    drives a real `narvo` over a real socket and is where the replay refusals
//!    and the answering moments are established.
//!
//! What none of the three reaches is the interpretation: whether a host's
//! implementation of this revision agrees with this one about a field it and the
//! specification both mention. Only a client can answer that, and the first one
//! to try is the evidence — not anything here.
//!
//! # Determinism
//!
//! Nothing here reads a clock or a source of entropy, and answering a message is
//! a pure function of the message and of what the engine said. That matters less
//! than it does inside the engine — this process runs no simulation — and it is
//! still what makes every test below a comparison of two strings.

use narvo_ipc::{Lines, Request, Response, framed};
use serde::Serialize;

use crate::jsonrpc::{self, Fault, Id, Incoming};
use crate::tools::{self, Tool};

/// The protocol revision this server speaks.
pub const VERSION: &str = "2026-07-28";

/// Every revision it speaks, newest first, as `server/discover` reports them.
const SUPPORTED: &[&str] = &[VERSION];

/// The methods it answers, in the order an error lists them.
const METHODS: &[&str] = &["server/discover", "tools/list", "tools/call"];

/// The reserved `_meta` key carrying the protocol version. Required on every
/// request.
const PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";

/// The reserved `_meta` key carrying what the client can do. Required on every
/// request.
const CLIENT_CAPABILITIES_KEY: &str = "io.modelcontextprotocol/clientCapabilities";

/// What `server/discover` tells an agent this server is for.
///
/// The specification's own words for the field: "Optional natural-language
/// guidance for LLMs on how to use this server effectively." So it is written for
/// a model rather than for a person, and it says the two things that are not
/// derivable from the tool list — that there is one engine behind all seven
/// tools, and that a run can be *waiting*.
const INSTRUCTIONS: &str = "\
Drives one running instance of the Narvo game engine over its agent socket. The seven \
tools read and steer a single simulation: names come from list_entities, component values \
are the engine's own RON carried inside JSON strings, and every answer is the world as the \
tick that produced it left it. A headless run that has used its tick budget waits for a \
command rather than exiting, so step is both how the simulation advances and how a stopped \
run is released. A run reproducing a recording answers every read and refuses every command \
that would change what it reproduces.";

/// How this server names itself, self-reported and unverified.
#[derive(Debug, Clone, Copy, Serialize)]
struct Implementation {
    name: &'static str,
    version: &'static str,
}

/// The `_meta` every result carries.
///
/// "Servers **SHOULD** include the following `io.modelcontextprotocol/*` field in
/// every result's `_meta` … to identify themselves without relying on any prior
/// connection state."
#[derive(Debug, Clone, Copy, Serialize)]
struct ResultMeta {
    #[serde(rename = "io.modelcontextprotocol/serverInfo")]
    server_info: Implementation,
}

impl ResultMeta {
    fn new() -> Self {
        Self {
            server_info: Implementation {
                name: env!("CARGO_PKG_NAME"),
                version: env!("CARGO_PKG_VERSION"),
            },
        }
    }
}

/// What `server/discover` answers with.
#[derive(Debug, Serialize)]
struct Discovered {
    #[serde(rename = "resultType")]
    result_type: &'static str,
    #[serde(rename = "supportedVersions")]
    supported_versions: &'static [&'static str],
    capabilities: Capabilities,
    instructions: &'static str,
    #[serde(rename = "_meta")]
    meta: ResultMeta,
}

/// What this server can do, which is tools and nothing else.
///
/// `listChanged` is deliberately absent: the tool list is a constant, so there is
/// nothing to notify about, and declaring the capability would promise a
/// notification stream this server does not have.
#[derive(Debug, Clone, Copy, Serialize)]
struct Capabilities {
    tools: Empty,
}

/// An empty object, which is how a capability with no settings is spelled.
#[derive(Debug, Clone, Copy, Serialize)]
struct Empty {}

/// What `tools/list` answers with.
///
/// No `nextCursor`: every tool fits in one page, and "Clients **SHOULD** treat a
/// missing `nextCursor` as the end of results."
#[derive(Debug, Serialize)]
struct Listed {
    #[serde(rename = "resultType")]
    result_type: &'static str,
    tools: Vec<Tool>,
    #[serde(rename = "_meta")]
    meta: ResultMeta,
}

/// What `tools/call` answers with.
#[derive(Debug, Serialize)]
struct Called {
    #[serde(rename = "resultType")]
    result_type: &'static str,
    content: Vec<Content>,
    #[serde(rename = "isError")]
    is_error: bool,
    #[serde(rename = "_meta")]
    meta: ResultMeta,
}

/// One block of a tool result. Text only here.
#[derive(Debug, Serialize)]
struct Content {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

impl Called {
    /// A call that worked, carrying `text`.
    fn ok(text: String) -> Self {
        Self::new(text, false)
    }

    /// A call the engine refused, or one whose arguments were not usable.
    ///
    /// MCP's *tool execution error*: the model is meant to read this and try
    /// again with something else, which is why it is a result rather than a
    /// JSON-RPC error.
    fn refused(text: String) -> Self {
        Self::new(text, true)
    }

    fn new(text: String, is_error: bool) -> Self {
        Self {
            result_type: "complete",
            content: vec![Content { kind: "text", text }],
            is_error,
            meta: ResultMeta::new(),
        }
    }
}

/// Whatever can answer an engine request.
///
/// **The seam that keeps the socket out of this file.** One method, because one
/// is what the conversation is: an agent asks a running engine something and the
/// engine answers. `narvo_ipc::Client` implements it in `main.rs`, and the tests
/// below implement it with a script — the same arrangement `narvo-app`'s
/// `ipc::Channel` has with `Silent`, `Collected` and the real endpoint.
pub trait Engine {
    /// Sends one request and reads the one answer to it.
    ///
    /// # Errors
    ///
    /// Whatever stopped the conversation. The engine *refusing* a request is not
    /// one of those — that comes back as `Ok(Response::Error)`, and the two are
    /// different things an agent has to be told apart (M6.5b's S4).
    fn ask(&mut self, request: &Request) -> Result<Response, narvo_ipc::ClientError>;
}

/// One MCP conversation, over one engine.
#[derive(Debug)]
pub struct Server<E> {
    engine: E,
}

impl<E: Engine> Server<E> {
    /// A server that will talk to `engine`.
    pub const fn new(engine: E) -> Self {
        Self { engine }
    }

    /// Answers one message, or says nothing because nothing is owed.
    ///
    /// `None` is returned for exactly one input — a well-formed notification —
    /// and it is a rule rather than a convenience: "The receiver **MUST NOT**
    /// send a response." On stdio there is no second channel, so an unbidden
    /// answer would be a line on `stdout` that no `id` correlates.
    pub fn handle(&mut self, line: &str) -> Option<String> {
        let incoming = match jsonrpc::classify(line) {
            Ok(incoming) => incoming,
            // No `id`: either it was not readable, or the message that carried it
            // was not a request. Left out rather than sent as `null`, which is
            // what this revision's error shape asks for.
            Err(fault) => return Some(jsonrpc::failure(None, &fault)),
        };

        match incoming {
            Incoming::Notification { .. } => None,
            Incoming::Request { id, method, params } => Some(match self.answer(&method, &params) {
                Ok(render) => render(&id),
                Err(fault) => jsonrpc::failure(Some(&id), &fault),
            }),
        }
    }

    /// Works out what one request is owed, without yet knowing its `id`.
    ///
    /// The answer comes back as a closure over the `id` so that every result type
    /// keeps its own shape here and the envelope is added in one place.
    #[expect(
        clippy::type_complexity,
        reason = "a boxed renderer is what lets each result type keep its own \
                  Serialize shape while the envelope is written once"
    )]
    fn answer(
        &mut self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<Box<dyn FnOnce(&Id) -> String + '_>, Fault> {
        // Checked before `_meta` is, and deliberately: a legacy client sends no
        // `_meta` at all, so the ordinary complaint would be about a missing
        // field rather than about the era it is from. This one names the version
        // it should be speaking, which is what the specification asks a
        // modern-only server to do for a client that has no way forward.
        if method == "initialize" {
            return Err(Fault::LegacyHandshake {
                supported: SUPPORTED,
            });
        }

        // Every request carries these, whatever it asks for, because there is no
        // handshake in which they could have been agreed once.
        self.check_meta(params)?;

        match method {
            "server/discover" => Ok(Box::new(|id: &Id| {
                jsonrpc::answer(
                    id,
                    &Discovered {
                        result_type: "complete",
                        supported_versions: SUPPORTED,
                        capabilities: Capabilities { tools: Empty {} },
                        instructions: INSTRUCTIONS,
                        meta: ResultMeta::new(),
                    },
                )
            })),

            "tools/list" => {
                // A cursor this server never issued cannot name a position in a
                // result set it never paginated. "Invalid cursors **SHOULD**
                // result in an error with code -32602 (Invalid params)."
                if let Some(cursor) = params.get("cursor") {
                    return Err(Fault::BadParams {
                        because: format!(
                            "{cursor} is not a cursor this server issued; it returns every tool \
                             in one page and never sends a \"nextCursor\""
                        ),
                    });
                }

                Ok(Box::new(|id: &Id| {
                    jsonrpc::answer(
                        id,
                        &Listed {
                            result_type: "complete",
                            tools: tools::catalogue(),
                            meta: ResultMeta::new(),
                        },
                    )
                }))
            }

            "tools/call" => {
                let called = self.call(params)?;
                Ok(Box::new(move |id: &Id| jsonrpc::answer(id, &called)))
            }

            other => Err(Fault::UnknownMethod {
                method: other.to_owned(),
                known: METHODS,
            }),
        }
    }

    /// Runs one tool call against the engine.
    ///
    /// # The two mechanisms, kept apart
    ///
    /// A [`Fault`] here becomes a JSON-RPC error — MCP's *protocol* error, for
    /// "issues with the request structure itself that models are less likely to
    /// be able to fix". Everything else becomes a result, `isError` saying which
    /// kind, because those are the ones a model can act on.
    ///
    /// The split lands on types that already existed: `Ok(Response::Error)` is
    /// the engine refusing something it understood, and `Err(ClientError)` is the
    /// conversation not happening at all.
    fn call(&mut self, params: &serde_json::Value) -> Result<Called, Fault> {
        let Some(name) = params.get("name") else {
            return Err(Fault::BadParams {
                because: "a tools/call names the tool to call in \"name\"".to_owned(),
            });
        };
        let Some(name) = name.as_str() else {
            return Err(Fault::BadParams {
                because: format!("\"name\" must be a string, and this one is {name}"),
            });
        };

        let arguments = match params.get("arguments") {
            None => serde_json::json!({}),
            Some(object @ serde_json::Value::Object(_)) => object.clone(),
            Some(other) => {
                return Err(Fault::BadParams {
                    because: format!("\"arguments\" must be an object, and this one is {other}"),
                });
            }
        };

        // An unknown tool is a protocol error and a bad argument is not, which is
        // the specification's own division: "Unknown tool" is listed under
        // protocol errors, "Input validation errors" under tool execution errors.
        // So the name is checked here and the arguments below.
        if !tools::catalogue().iter().any(|tool| tool.name == name) {
            return Err(Fault::BadParams {
                because: tools::unknown(name),
            });
        }

        let request = match tools::request_for(name, &arguments) {
            Ok(request) => request,
            Err(complaint) => return Ok(Called::refused(complaint)),
        };

        match self.engine.ask(&request) {
            // The engine's own sentence, carried across unchanged. It is written
            // to be read by whoever sent the request — every one of them names
            // what was wrong and what to do instead — so putting anything in
            // front of it would be this crate talking over the engine.
            Ok(Response::Error { message }) => Ok(Called::refused(message)),

            // Everything else is the engine's answer as it wrote it. **Verbatim,
            // and that is ADR-0030's rule read one layer out:** a component value
            // is the registry's own RON inside a JSON string, and a second
            // rendering here would be a second opinion about a format that
            // already has one. The consequence is the one that ADR states — an
            // agent parses RON to read a component value — and the tool
            // descriptions say so.
            Ok(answer) => Ok(Called::ok(answer.to_json())),

            Err(cause) => Err(Fault::Unreachable {
                cause: cause.to_string(),
            }),
        }
    }

    /// Refuses a request that does not carry the metadata every request carries.
    fn check_meta(&self, params: &serde_json::Value) -> Result<(), Fault> {
        let meta = params.get("_meta");

        let Some(requested) = meta.and_then(|meta| meta.get(PROTOCOL_VERSION_KEY)) else {
            return Err(Fault::MissingMeta {
                key: PROTOCOL_VERSION_KEY,
            });
        };
        if meta
            .and_then(|meta| meta.get(CLIENT_CAPABILITIES_KEY))
            .is_none()
        {
            return Err(Fault::MissingMeta {
                key: CLIENT_CAPABILITIES_KEY,
            });
        }

        // **Checked on every request, `server/discover` included.** The
        // specification is unqualified about it — "If the server does not
        // implement the requested version … it **MUST** respond with an
        // `UnsupportedProtocolVersionError`" — and a client loses nothing by it,
        // because the error carries the same `supported` list discovery would
        // have answered with.
        let requested = requested.as_str().unwrap_or_default();
        if !SUPPORTED.contains(&requested) {
            return Err(Fault::UnsupportedVersion {
                requested: requested.to_owned(),
                supported: SUPPORTED,
            });
        }

        Ok(())
    }
}

/// Bytes as they arrived, and the bytes that should go back.
///
/// **The framing is `narvo-ipc`'s, in its second consumer.** MCP's stdio binding
/// asks for exactly what ADR-0033 already built — "Messages are delimited by
/// newlines, and **MUST NOT** contain embedded newlines" — so this crate calls
/// [`Lines`] and [`framed`] rather than splitting on `\n` again. Two framings at
/// the two ends of one connection is the failure that ADR exists to prevent, and
/// a third implementation here would be the same failure one process further out.
///
/// An empty line produces nothing. It is a complete line and the framing reports
/// it as one; both ends of the engine's own transport skip it, and answering a
/// blank line with a parse error would be this server disagreeing with the two
/// that already exist.
pub fn pump<E: Engine>(lines: &mut Lines, server: &mut Server<E>, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();

    for line in lines.feed(bytes) {
        if line.is_empty() {
            continue;
        }

        // Lossy rather than a refusal: what arrives is bytes, and nothing between
        // a pipe and here promises they are UTF-8. The replacement characters
        // then fail to parse as JSON with a position, which is an answer the
        // client can read — the same reading `narvo-ipc`'s client takes of the
        // same problem.
        let text = String::from_utf8_lossy(&line);
        if let Some(reply) = server.handle(&text) {
            out.extend_from_slice(framed(&reply).as_bytes());
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{Engine, METHODS, SUPPORTED, Server, VERSION, pump};
    use narvo_ipc::{ClientError, ComponentValue, Lines, Request, Response};
    use std::collections::VecDeque;

    /// An engine that answers from a script and remembers what it was asked.
    ///
    /// The counterpart of `narvo-app`'s `Collected`: no socket, no process, no
    /// thread, so every test below is two strings compared. What it cannot stand
    /// in for is a real engine's behaviour, which is what
    /// `tests/agent_over_mcp.rs` drives two processes for.
    #[derive(Debug, Default)]
    struct Scripted {
        asked: Vec<Request>,
        answers: VecDeque<Result<Response, ClientError>>,
    }

    impl Scripted {
        /// An engine that will answer with `answers`, in order.
        fn saying(answers: impl IntoIterator<Item = Response>) -> Self {
            Self {
                asked: Vec::new(),
                answers: answers.into_iter().map(Ok).collect(),
            }
        }

        /// An engine that has gone away.
        fn gone() -> Self {
            let mut scripted = Self::default();
            scripted
                .answers
                .push_back(Err(ClientError::Closed { mid_answer: false }));
            scripted
        }
    }

    impl Engine for Scripted {
        fn ask(&mut self, request: &Request) -> Result<Response, ClientError> {
            self.asked.push(request.clone());
            self.answers
                .pop_front()
                .unwrap_or_else(|| panic!("the script ran out at {request:?}"))
        }
    }

    /// The metadata every request carries, as a fragment to paste into one.
    const META: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

    /// One request in, one answer out, through a server with no engine behind it.
    fn ask(line: &str) -> String {
        let mut server = Server::new(Scripted::default());
        server.handle(line).expect("a request is owed an answer")
    }

    /// The `result` of an answer, as a JSON value.
    fn result(answer: &str) -> serde_json::Value {
        let value: serde_json::Value = serde_json::from_str(answer).expect("this crate wrote it");
        value
            .get("result")
            .unwrap_or_else(|| panic!("no result in {answer}"))
            .clone()
    }

    /// The `error` of an answer, as a JSON value.
    fn error(answer: &str) -> serde_json::Value {
        let value: serde_json::Value = serde_json::from_str(answer).expect("this crate wrote it");
        value
            .get("error")
            .unwrap_or_else(|| panic!("no error in {answer}"))
            .clone()
    }

    // ---- discovery -------------------------------------------------------

    /// **`server/discover` answers with everything the specification's own
    /// example carries**, field for field.
    ///
    /// The example is at `/specification/2026-07-28/server/discover` and is quoted
    /// as the whole response. This asserts the keys it shows rather than the
    /// bytes, because two of its fields — `ttlMs` and `cacheScope` — are the
    /// caching utility, which this server does not implement and therefore does
    /// not send.
    #[test]
    fn discovery_answers_with_the_fields_the_specification_shows() {
        let answered = result(&ask(&format!(
            r#"{{"jsonrpc":"2.0","id":"discover-1","method":"server/discover","params":{{{META}}}}}"#
        )));

        assert_eq!(answered["resultType"], "complete");
        assert_eq!(answered["supportedVersions"], serde_json::json!([VERSION]));
        assert_eq!(answered["capabilities"], serde_json::json!({"tools":{}}));
        assert_eq!(
            answered["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "narvo-mcp"
        );
        assert!(
            answered["instructions"]
                .as_str()
                .expect("instructions are a string")
                .contains("Narvo"),
            "{answered}"
        );
    }

    /// **`server/discover` is what MUST be implemented**, so it is the one method
    /// asserted to exist by name rather than only by behaviour.
    #[test]
    fn every_method_this_server_lists_is_a_method_it_answers() {
        for method in METHODS {
            // One answer in the script, for the one method that reaches an
            // engine. The other two are answered out of constants.
            let mut server = Server::new(Scripted::saying([Response::ListEntities {
                entities: Vec::new(),
                ticks_run: 7,
            }]));
            let answer = server
                .handle(&format!(
                    r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{{META},"name":"list_entities"}}}}"#
                ))
                .expect("a request is owed an answer");

            assert!(
                !answer.contains("\"code\":-32601"),
                "{method} is listed and not answered: {answer}"
            );
        }
        assert!(METHODS.contains(&"server/discover"));
    }

    // ---- the tool list ---------------------------------------------------

    /// The tool list comes back whole, in a page with no cursor after it.
    #[test]
    fn the_tool_list_is_one_page_with_no_cursor_after_it() {
        let answered = result(&ask(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{META}}}}}"#
        )));

        assert_eq!(answered["resultType"], "complete");
        assert_eq!(
            answered["tools"]
                .as_array()
                .expect("tools are an array")
                .len(),
            8
        );
        assert!(answered.get("nextCursor").is_none(), "{answered}");
        assert_eq!(answered["tools"][0]["name"], "list_entities");
        assert!(answered["tools"][0]["inputSchema"]["type"] == "object");
    }

    /// **The list does not depend on anything**, which is what MCP requires.
    ///
    /// S3's answer asserted rather than only argued: the same question asked
    /// twice with a replay started in between gets the same list. The
    /// specification's words are "**MUST NOT** vary per-connection or as a side
    /// effect of other requests on the connection", and starting a replay is
    /// such a request.
    #[test]
    fn the_tool_list_does_not_change_when_the_run_starts_a_replay() {
        let mut server = Server::new(Scripted::saying([Response::Replay {
            path: "bug.rec".to_owned(),
            mode: "input".to_owned(),
            seed: 1,
            ticks: 600,
            ticks_run: 7,
        }]));

        let listing =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{META}}}}}"#);
        let before = server.handle(&listing).expect("an answer");

        server
            .handle(&format!(
                r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{{META},"name":"replay","arguments":{{"path":"bug.rec"}}}}}}"#
            ))
            .expect("an answer");

        let after = server.handle(&listing).expect("an answer");
        assert_eq!(before, after, "the tool list moved under a replay");
    }

    /// A cursor this server never issued is refused where the specification says.
    #[test]
    fn a_cursor_this_server_never_issued_is_invalid_params() {
        let refused = error(&ask(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{META},"cursor":"eyJwYWdlIjogM30="}}}}"#
        )));

        assert_eq!(refused["code"], -32602);
        assert_eq!(
            refused["message"],
            "those parameters are not usable: \"eyJwYWdlIjogM30=\" is not a cursor this server \
             issued; it returns every tool in one page and never sends a \"nextCursor\""
        );
    }

    // ---- calling a tool --------------------------------------------------

    /// **A call reaches the engine as the request it names**, and the answer
    /// comes back as the engine wrote it.
    ///
    /// Red edge (a) of M6.5b at the layer that catches it: the assertion is on
    /// what the engine was *asked*, not only on what came back, because a
    /// dispatch that built another command's request would still produce a
    /// well-formed answer.
    #[test]
    fn a_call_reaches_the_engine_as_the_request_it_names() {
        let mut server = Server::new(Scripted::saying([Response::GetComponent {
            entity: "3v1".parse().expect("a well-formed name"),
            component: "layer".to_owned(),
            value: Some("(depth:0.5)".to_owned()),
            ticks_run: 7,
        }]));

        let answer = server
            .handle(&format!(
                r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{{META},"name":"get_component","arguments":{{"entity":"3v1","component":"layer"}}}}}}"#
            ))
            .expect("an answer");

        assert_eq!(
            server.engine.asked,
            vec![Request::GetComponent {
                entity: "3v1".parse().expect("a well-formed name"),
                component: "layer".to_owned(),
            }],
            "the call reached the engine as a different request"
        );

        let answered = result(&answer);
        assert_eq!(answered["resultType"], "complete");
        assert_eq!(answered["isError"], false);
        assert_eq!(answered["content"][0]["type"], "text");
        assert_eq!(
            answered["content"][0]["text"],
            r#"{"get_component":{"entity":"3v1","component":"layer","value":"(depth:0.5)","ticks_run":7}}"#
        );
    }

    /// **A component value crosses as the registry's bytes**, through this layer
    /// too.
    ///
    /// ADR-0030's rule read at the second boundary. The value carries a `NaN`,
    /// which `serde_json` has no number for — and never sees, because it is text
    /// here and stays text.
    #[test]
    fn a_component_value_crosses_this_layer_as_the_registrys_own_bytes() {
        let mut server = Server::new(Scripted::saying([Response::GetEntity {
            entity: "3v1".parse().expect("a well-formed name"),
            components: vec![ComponentValue::new("transform", "(x:NaN,y:0.0)")],
            ticks_run: 7,
        }]));

        let answer = server
            .handle(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"get_entity","arguments":{{"entity":"3v1"}}}}}}"#
            ))
            .expect("an answer");

        let text = result(&answer)["content"][0]["text"]
            .as_str()
            .expect("text content")
            .to_owned();
        assert!(text.contains("(x:NaN,y:0.0)"), "{text}");
        assert!(!text.contains("null"), "{text}");
    }

    /// **The engine refusing is a tool execution error**, not a protocol error.
    ///
    /// The second stage of red edge (a): the right answer, delivered without the
    /// marking that says it is a refusal, is a call an agent reads as having
    /// worked. `isError` is what says otherwise, and the engine's own sentence is
    /// what a model self-corrects from.
    #[test]
    fn the_engine_refusing_comes_back_as_a_tool_execution_error() {
        let mut server = Server::new(Scripted::saying([Response::Error {
            message: "there is no entity 9v1 in this world".to_owned(),
        }]));

        let answer = server
            .handle(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"get_entity","arguments":{{"entity":"9v1"}}}}}}"#
            ))
            .expect("an answer");

        let answered = result(&answer);
        assert_eq!(answered["isError"], true, "{answered}");
        assert_eq!(
            answered["content"][0]["text"],
            "there is no entity 9v1 in this world"
        );
    }

    /// **An argument that is not usable is a tool execution error too**, because
    /// a model can fix it.
    #[test]
    fn an_unusable_argument_is_a_tool_execution_error() {
        let answer = ask(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"step","arguments":{{"ticks":-4}}}}}}"#
        ));

        let answered = result(&answer);
        assert_eq!(answered["isError"], true, "{answered}");
        assert_eq!(
            answered["content"][0]["text"],
            "\"ticks\" must be a whole number of zero or more, and this one is -4"
        );
    }

    /// **An unknown tool is a protocol error**, which is where MCP puts it.
    #[test]
    fn an_unknown_tool_is_a_protocol_error_naming_the_tools_there_are() {
        let refused = error(&ask(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"get_weather","arguments":{{}}}}}}"#
        )));

        assert_eq!(refused["code"], -32602);
        assert_eq!(
            refused["message"],
            "those parameters are not usable: no tool here is called \"get_weather\"; this server \
             offers list_entities, get_entity, get_component, set_component, step, load_scene, \
             replay, dump"
        );
    }

    /// **An engine that has gone is a protocol error**, and it says which kind of
    /// gone.
    ///
    /// Red edge (c) at the deterministic layer: no argument the model could
    /// change makes an absent engine answer, so this is not something to
    /// self-correct from. The message is `ClientError`'s own, which M6.5a wrote
    /// to tell a run that is still thinking from one that has ended.
    #[test]
    fn an_engine_that_has_gone_is_reported_as_a_protocol_error() {
        let mut server = Server::new(Scripted::gone());

        let answer = server
            .handle(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"list_entities"}}}}"#
            ))
            .expect("an answer");

        let refused = error(&answer);
        assert_eq!(refused["code"], -32603);
        assert_eq!(
            refused["message"],
            "the engine could not answer: the engine closed the connection before answering"
        );
    }

    /// A call with no `arguments` at all is a call with no arguments.
    #[test]
    fn a_call_without_arguments_is_a_call_with_none() {
        let mut server = Server::new(Scripted::saying([Response::ListEntities {
            entities: Vec::new(),
            ticks_run: 7,
        }]));

        server
            .handle(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META},"name":"list_entities"}}}}"#
            ))
            .expect("an answer");

        assert_eq!(server.engine.asked, vec![Request::ListEntities]);
    }

    /// A `tools/call` with no `name` is malformed rather than unknown.
    #[test]
    fn a_call_without_a_name_is_a_malformed_request() {
        let refused = error(&ask(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{{META}}}}}"#
        )));

        assert_eq!(refused["code"], -32602);
        assert_eq!(
            refused["message"],
            "those parameters are not usable: a tools/call names the tool to call in \"name\""
        );
    }

    // ---- the envelope every request carries ------------------------------

    /// A request with no `_meta` is refused, naming the key it needs.
    #[test]
    fn a_request_without_the_protocol_version_is_refused() {
        let refused = error(&ask(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#));

        assert_eq!(refused["code"], -32602);
        assert!(
            refused["message"]
                .as_str()
                .expect("a message")
                .contains("io.modelcontextprotocol/protocolVersion"),
            "{refused}"
        );
    }

    /// The capabilities key is required as well, and is named separately.
    #[test]
    fn a_request_without_the_client_capabilities_is_refused() {
        let refused = error(&ask(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}"#,
        ));

        assert_eq!(refused["code"], -32602);
        assert!(
            refused["message"]
                .as_str()
                .expect("a message")
                .contains("io.modelcontextprotocol/clientCapabilities"),
            "{refused}"
        );
    }

    /// **An unsupported version is refused on every method, discovery included.**
    #[test]
    fn an_unsupported_version_is_refused_on_every_method() {
        for method in METHODS {
            let refused = error(&ask(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{"_meta":{{"io.modelcontextprotocol/protocolVersion":"1900-01-01","io.modelcontextprotocol/clientCapabilities":{{}}}}}}}}"#
            )));

            assert_eq!(refused["code"], -32022, "{method}: {refused}");
            assert_eq!(
                refused["data"],
                serde_json::json!({"requested":"1900-01-01","supported":SUPPORTED}),
                "{method}"
            );
        }
    }

    /// **A legacy client's `initialize` is told which version to speak.**
    ///
    /// The one thing the specification asks a modern-only server to do for a
    /// legacy one: "A server that supports only modern versions **SHOULD** name
    /// the protocol versions it supports in any error it returns to an
    /// `initialize` request."
    #[test]
    fn a_legacy_initialize_is_told_which_version_this_server_speaks() {
        let refused = error(&ask(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"old","version":"1"}}}"#,
        ));

        assert_eq!(refused["code"], -32601);
        assert_eq!(
            refused["message"],
            "this server speaks MCP 2026-07-28 and nothing older, and that revision has no \
             initialize handshake: every request carries its own version in \"_meta\". Send \
             server/discover instead"
        );
    }

    /// An unknown method names the methods there are.
    #[test]
    fn an_unknown_method_names_the_methods_there_are() {
        let refused = error(&ask(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"resources/list","params":{{{META}}}}}"#
        )));

        assert_eq!(refused["code"], -32601);
        assert_eq!(
            refused["message"],
            "no method here is called \"resources/list\"; this server answers server/discover, \
             tools/list, tools/call"
        );
    }

    /// **A notification is answered with silence.**
    #[test]
    fn a_notification_gets_no_answer_at_all() {
        let mut server = Server::new(Scripted::default());

        assert_eq!(
            server.handle(
                r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}"#
            ),
            None
        );
        assert_eq!(
            server.handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            None
        );
    }

    // ---- the framing, in its second consumer -----------------------------

    /// **Two requests in one delivery are two answers**, which is red edge (b).
    ///
    /// The case a hand-written split on `\n` gets right and a `read` that treats
    /// one delivery as one message gets wrong. It is `narvo-ipc`'s framing doing
    /// the work — this test is what says the second consumer of it holds the same
    /// property the first one does.
    #[test]
    fn two_requests_in_one_delivery_are_two_answers() {
        let mut server = Server::new(Scripted::default());
        let mut lines = Lines::new();

        let both = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\",\"params\":{{{META}}}}}\n\
             {{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"server/discover\",\"params\":{{{META}}}}}\n"
        );

        let out = pump(&mut lines, &mut server, both.as_bytes());
        let text = String::from_utf8(out).expect("this crate wrote it");

        let answers: Vec<&str> = text.lines().collect();
        assert_eq!(answers.len(), 2, "an answer was lost: {text}");
        assert!(answers[0].contains("\"id\":1"), "{text}");
        assert!(answers[1].contains("\"id\":2"), "{text}");
        assert!(!lines.unfinished());
    }

    /// **Half a request is not a request**, and the rest of it completes one.
    #[test]
    fn a_request_split_across_deliveries_is_still_one_request() {
        let mut server = Server::new(Scripted::default());
        let mut lines = Lines::new();

        let whole =
            format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{META}}}}}"#);
        let (head, tail) = whole.split_at(20);

        assert!(
            pump(&mut lines, &mut server, head.as_bytes()).is_empty(),
            "half a request was answered"
        );
        assert!(lines.unfinished());

        let out = pump(&mut lines, &mut server, format!("{tail}\n").as_bytes());
        let text = String::from_utf8(out).expect("this crate wrote it");
        assert!(text.contains("\"id\":1"), "{text}");
        assert!(text.ends_with('\n'), "{text:?}");
    }

    /// Every answer this server writes is one line, terminated.
    #[test]
    fn every_answer_is_one_framed_line() {
        let mut server = Server::new(Scripted::default());
        let mut lines = Lines::new();

        let out = pump(
            &mut lines,
            &mut server,
            b"not json\n\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\nalso not json\n",
        );
        let text = String::from_utf8(out).expect("this crate wrote it");

        // Two parse failures. The blank line and the notification produce
        // nothing, which is two different reasons for the same silence.
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.ends_with('\n'), "{text:?}");
        for line in text.lines() {
            assert!(line.contains("\"code\":-32700"), "{line}");
        }
    }
}
