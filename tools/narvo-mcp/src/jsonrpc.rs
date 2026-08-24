//! The JSON-RPC 2.0 envelope MCP messages travel in, and what a bad one says.
//!
//! Everything here is a function over text. Nothing reads a socket, a file or a
//! clock, which is what makes the whole of the protocol layer above it testable
//! without starting anything (M6.5b's S2).
//!
//! # Which specification, and which revision
//!
//! **MCP `2026-07-28`**, read at
//! <https://modelcontextprotocol.io/specification/2026-07-28/>. The revision is
//! named here and in [`crate::server::VERSION`] rather than left implicit,
//! because the protocol has moved five times in under two years and a server that
//! did not say which one it speaks would be unfalsifiable. Every wire shape in
//! this crate is pinned in a test against the specification's own verbatim
//! example JSON — see `server.rs`.
//!
//! # Absent and `null` are different ids, and serde collapses them
//!
//! A request **MUST** carry a string or numeric `id`; a notification carries
//! none; and MCP says "Unlike base JSON-RPC, the ID **MUST NOT** be `null`". So
//! `{"id":null,…}` is a malformed request and `{…}` with no `id` is a
//! notification to be answered with silence — two different outcomes.
//!
//! `#[derive(Deserialize)]` cannot express that distinction, which is why the
//! envelope below is read out of a [`serde_json::Map`] by key rather than derived.
//! Measured on the pinned `serde_json` 1.0.151 rather than assumed: a field
//! `Option<serde_json::Value>` with `#[serde(default)]` yields `None` for an
//! absent `id` **and** `None` for an explicit `null`, while
//! `Map::contains_key("id")` returns `false` and `true` for the same two inputs.

use std::fmt;

use serde::Serialize;

/// The revision of the JSON-RPC specification every message carries.
const JSONRPC: &str = "2.0";

/// A request's identifier, carried back verbatim.
///
/// Echoed rather than re-rendered: JSON-RPC correlates a response to its request
/// by this value, and a server that normalised `"1"` into `1` would answer a
/// question nobody asked. MCP narrows JSON-RPC's `id` to a string or a number,
/// so anything else — a boolean, an array, an object, `null` — is
/// [`Fault::NotARequest`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Id(serde_json::Value);

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What one line from a client turned out to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A well-formed request: answer it, with this `id` on the answer.
    Request {
        /// The identifier to carry back.
        id: Id,
        /// The method that was asked for.
        method: String,
        /// Its parameters, or `null` when none were sent.
        params: serde_json::Value,
    },
    /// A well-formed notification: **say nothing at all.**
    ///
    /// "The receiver **MUST NOT** send a response." A server that answered one
    /// would put a message on `stdout` that no `id` correlates, which is the one
    /// thing the stdio binding forbids outright.
    Notification {
        /// The method that was announced.
        method: String,
    },
}

/// Reads one line as a JSON-RPC message, or says why it is not one.
///
/// # Errors
///
/// [`Fault::Parse`] when the text is not JSON, and [`Fault::NotARequest`] when it
/// is JSON but not a message this protocol can act on.
pub fn classify(line: &str) -> Result<Incoming, Fault> {
    let value: serde_json::Value =
        serde_json::from_str(line).map_err(|source| Fault::Parse(source.to_string()))?;

    let serde_json::Value::Object(fields) = value else {
        return Err(Fault::NotARequest {
            because: "a JSON-RPC message is an object".to_owned(),
        });
    };

    match fields.get("jsonrpc") {
        Some(serde_json::Value::String(version)) if version == JSONRPC => {}
        Some(other) => {
            return Err(Fault::NotARequest {
                because: format!("\"jsonrpc\" must be \"{JSONRPC}\", and this one is {other}"),
            });
        }
        None => {
            return Err(Fault::NotARequest {
                because: format!("a JSON-RPC message carries \"jsonrpc\": \"{JSONRPC}\""),
            });
        }
    }

    let method = match fields.get("method") {
        Some(serde_json::Value::String(method)) => method.clone(),
        Some(other) => {
            return Err(Fault::NotARequest {
                because: format!("\"method\" must be a string, and this one is {other}"),
            });
        }
        None => {
            return Err(Fault::NotARequest {
                because: "a JSON-RPC message names a \"method\"".to_owned(),
            });
        }
    };

    // The distinction the module documentation is about: the key being absent is
    // a notification, and the key holding `null` is a malformed request.
    let Some(id) = fields.get("id") else {
        return Ok(Incoming::Notification { method });
    };

    match id {
        serde_json::Value::String(_) | serde_json::Value::Number(_) => Ok(Incoming::Request {
            id: Id(id.clone()),
            method,
            params: fields
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        }),
        serde_json::Value::Null => Err(Fault::NotARequest {
            because: "\"id\" must not be null; leave it out entirely to send a notification"
                .to_owned(),
        }),
        other => Err(Fault::NotARequest {
            because: format!("\"id\" must be a string or a number, and this one is {other}"),
        }),
    }
}

/// Why a request could not be answered at all.
///
/// # The half of M6.5b's S4 that is this crate's own
///
/// **This is not the engine saying no.** A request the engine understood and
/// refused comes back as a [`Response::Error`](narvo_ipc::Response::Error) and
/// becomes a *tool execution error* — `isError: true`, the engine's own sentence,
/// which is what MCP asks for so that a model can self-correct. This type is the
/// other mechanism: MCP's *protocol errors*, for "issues with the request
/// structure itself that models are less likely to be able to fix".
///
/// The two mechanisms land exactly on the two types that already existed before
/// this crate — [`narvo_ipc::Response::Error`] on one side and
/// [`narvo_ipc::ClientError`] on the other — which is why M6.3a's deferred
/// taxonomy needed no new variant on the wire. ADR-0034 records that.
///
/// # These messages are the product
///
/// The same standard `narvo-scene` set in M4.2, `narvo-input` in M5.1,
/// `narvo-ipc` in M6.1 and `narvo-app`'s `ipc` module in M6.3 were held to: the
/// other end is an agent and the message is the whole of its feedback. Every
/// variant is asserted on in a test, wording included.
#[derive(Debug)]
pub enum Fault {
    /// The line is not JSON.
    Parse(String),
    /// The line is JSON and is not a message this protocol can act on.
    NotARequest {
        /// Which rule it broke, in that rule's own terms.
        because: String,
    },
    /// A required `_meta` field is missing.
    MissingMeta {
        /// The reserved key that was not there.
        key: &'static str,
    },
    /// The client asked for a protocol revision this server does not speak.
    UnsupportedVersion {
        /// What it asked for.
        requested: String,
        /// What there is.
        supported: &'static [&'static str],
    },
    /// A client from the handshake era opened with `initialize`.
    ///
    /// Its own variant rather than an [`UnknownMethod`](Self::UnknownMethod),
    /// because the useful thing to say is which *version* to speak rather than
    /// which methods there are: "A server that supports only modern versions
    /// **SHOULD** name the protocol versions it supports in any error it returns
    /// to an `initialize` request … legacy clients have no fall-forward
    /// mechanism, and this message may be the only diagnostic they can surface to
    /// users."
    LegacyHandshake {
        /// The revisions this server does speak.
        supported: &'static [&'static str],
    },
    /// Nothing here answers that method.
    UnknownMethod {
        /// What was asked for.
        method: String,
        /// The methods this server does answer, in the order it lists them.
        known: &'static [&'static str],
    },
    /// The method exists and its parameters are not usable.
    BadParams {
        /// What was wrong with them.
        because: String,
    },
    /// The engine could not be reached or could not answer.
    ///
    /// A *server* error in MCP's sense, so a protocol error rather than a tool
    /// execution error: no adjustment to the arguments makes an absent engine
    /// answer. It carries [`narvo_ipc::ClientError`]'s own words, which name the
    /// address and distinguish a run that is thinking from one that has gone.
    Unreachable {
        /// What the client said.
        cause: String,
    },
}

impl Fault {
    /// The JSON-RPC error code this fault is reported under.
    ///
    /// The four standard codes plus one of MCP's own. `-32022` is
    /// `UnsupportedProtocolVersion`, and it is quoted from the specification's
    /// own table rather than invented: MCP reserves `-32020` to `-32099` for
    /// itself and forbids emitting an undefined code from that range.
    #[must_use]
    pub fn code(&self) -> i32 {
        match self {
            Self::Parse(_) => -32700,
            Self::NotARequest { .. } => -32600,
            // The specification leaves this one open — "the exact code is
            // implementation-defined (`initialize` is an unknown method and the
            // request also lacks the required `_meta` fields)" — and names
            // `-32601` and `-32602` as what legacy servers commonly answer with.
            // Method-not-found is the more accurate of the two here, because the
            // method really is not one this server has.
            Self::LegacyHandshake { .. } | Self::UnknownMethod { .. } => -32601,
            Self::MissingMeta { .. } | Self::BadParams { .. } => -32602,
            Self::UnsupportedVersion { .. } => -32022,
            Self::Unreachable { .. } => -32603,
        }
    }

    /// The structured half of the error, for the faults that have one.
    ///
    /// Only `UnsupportedProtocolVersion` does, and its shape is the
    /// specification's: `data.supported` is the list a client picks its retry
    /// from, and `data.requested` is what it sent. A client that could only read
    /// the message would have to parse English to recover.
    fn data(&self) -> Option<serde_json::Value> {
        match self {
            Self::UnsupportedVersion {
                requested,
                supported,
            } => Some(serde_json::json!({
                "supported": supported,
                "requested": requested,
            })),
            _ => None,
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(cause) => write!(
                f,
                "that line is not JSON: {cause}. Every message is one JSON-RPC object on one \
                 line, and a line break inside one is not allowed"
            ),
            Self::NotARequest { because } => {
                write!(f, "that is not a JSON-RPC request: {because}")
            }
            Self::MissingMeta { key } => write!(
                f,
                "every request carries \"{key}\" in its \"_meta\"; this one does not. MCP \
                 {} is stateless, so the version and the client's capabilities are sent \
                 again on each request rather than agreed once",
                crate::server::VERSION
            ),
            Self::UnsupportedVersion {
                requested,
                supported,
            } => write!(
                f,
                "this server does not speak protocol version {requested}; it speaks {}",
                supported.join(", ")
            ),
            Self::LegacyHandshake { supported } => write!(
                f,
                "this server speaks MCP {} and nothing older, and that revision has no \
                 initialize handshake: every request carries its own version in \"_meta\". \
                 Send server/discover instead",
                supported.join(", ")
            ),
            Self::UnknownMethod { method, known } => write!(
                f,
                "no method here is called \"{method}\"; this server answers {}",
                known.join(", ")
            ),
            Self::BadParams { because } => write!(f, "those parameters are not usable: {because}"),
            Self::Unreachable { cause } => write!(f, "the engine could not answer: {cause}"),
        }
    }
}

impl std::error::Error for Fault {}

/// One successful answer, rendered as a line of JSON.
///
/// `result` is written by the caller and this adds the envelope, so every result
/// type in this crate keeps its own field order and its own documentation.
pub fn answer<T: Serialize>(id: &Id, result: &T) -> String {
    #[derive(Serialize)]
    struct Envelope<'a, T> {
        jsonrpc: &'static str,
        id: &'a Id,
        result: &'a T,
    }

    // Infallible for this crate's result types: every field is a string, a
    // number, a bool, a `Vec` or a nested one of those, and **no float ever
    // reaches `serde_json` here** — a component value crosses as the registry's
    // own RON inside a JSON string, which is ADR-0030's rule and the reason this
    // boundary carries what the canonical dump carries.
    serde_json::to_string(&Envelope {
        jsonrpc: JSONRPC,
        id,
        result,
    })
    .expect("a result holds nothing that can fail to serialize")
}

/// One failure, rendered as a line of JSON.
///
/// `id` is `None` exactly when the request was too malformed to have one — the
/// case the specification allows: "Error responses **MUST** include the same ID
/// as the request they correspond to (except in error cases where the ID could
/// not be read due a malformed request)." It is left out rather than sent as
/// `null`, which is what the 2026-07-28 revision's error shape asks for.
pub fn failure(id: Option<&Id>, fault: &Fault) -> String {
    #[derive(Serialize)]
    struct Envelope<'a> {
        jsonrpc: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<&'a Id>,
        error: Body,
    }

    #[derive(Serialize)]
    struct Body {
        code: i32,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    }

    serde_json::to_string(&Envelope {
        jsonrpc: JSONRPC,
        id,
        error: Body {
            code: fault.code(),
            message: fault.to_string(),
            data: fault.data(),
        },
    })
    .expect("an error holds nothing that can fail to serialize")
}

#[cfg(test)]
mod tests {
    use super::{Fault, Id, Incoming, answer, classify, failure};

    /// A request is a request, and its `id` comes back the way it went in.
    #[test]
    fn a_request_carries_its_method_its_params_and_its_id() {
        let classified = classify(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#)
            .expect("that is a request");

        match classified {
            Incoming::Request { id, method, params } => {
                assert_eq!(id.to_string(), "7");
                assert_eq!(method, "tools/list");
                assert_eq!(params, serde_json::Value::Null);
            }
            other => panic!("a request was classified as {other:?}"),
        }
    }

    /// **A string id stays a string**, because that is what correlates an answer.
    #[test]
    fn a_string_id_is_not_normalised_into_a_number() {
        let classified =
            classify(r#"{"jsonrpc":"2.0","id":"1","method":"tools/list"}"#).expect("a request");
        let Incoming::Request { id, .. } = classified else {
            panic!("not a request")
        };

        assert_eq!(
            answer(&id, &"ok"),
            r#"{"jsonrpc":"2.0","id":"1","result":"ok"}"#
        );
    }

    /// **No `id` at all is a notification, and a notification is answered with
    /// silence.**
    #[test]
    fn a_message_without_an_id_is_a_notification() {
        assert_eq!(
            classify(r#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#).expect("valid"),
            Incoming::Notification {
                method: "notifications/cancelled".to_owned()
            }
        );
    }

    /// **`"id": null` is a malformed request, not a notification.**
    ///
    /// The distinction the module documentation records, held here so that the
    /// hand-written envelope reader cannot be replaced by a derive without this
    /// going red.
    #[test]
    fn a_null_id_is_a_malformed_request_rather_than_a_notification() {
        let fault = classify(r#"{"jsonrpc":"2.0","id":null,"method":"tools/list"}"#)
            .expect_err("null is not an id");

        assert_eq!(fault.code(), -32600);
        assert_eq!(
            fault.to_string(),
            "that is not a JSON-RPC request: \"id\" must not be null; leave it out entirely to \
             send a notification"
        );
    }

    /// Every message this type can produce, in its own words.
    ///
    /// The parse variant carries `serde_json`'s own text, which is checked by
    /// prefix and no further — it is another crate's wording and pinning all of
    /// it here would make this test a mirror of `serde_json`'s release notes.
    #[test]
    fn every_fault_says_what_happened_in_its_own_words() {
        let parse = classify("not json at all").expect_err("it is not json");
        assert_eq!(parse.code(), -32700);
        assert!(
            parse.to_string().starts_with("that line is not JSON: "),
            "{parse}"
        );
        assert!(
            parse.to_string().ends_with(
                ". Every message is one JSON-RPC object on one line, and a line break inside one \
                 is not allowed"
            ),
            "{parse}"
        );

        assert_eq!(
            Fault::NotARequest {
                because: "a JSON-RPC message is an object".to_owned()
            }
            .to_string(),
            "that is not a JSON-RPC request: a JSON-RPC message is an object"
        );

        assert_eq!(
            Fault::MissingMeta {
                key: "io.modelcontextprotocol/protocolVersion"
            }
            .to_string(),
            "every request carries \"io.modelcontextprotocol/protocolVersion\" in its \"_meta\"; \
             this one does not. MCP 2026-07-28 is stateless, so the version and the client's \
             capabilities are sent again on each request rather than agreed once"
        );

        assert_eq!(
            Fault::UnsupportedVersion {
                requested: "1900-01-01".to_owned(),
                supported: &["2026-07-28"],
            }
            .to_string(),
            "this server does not speak protocol version 1900-01-01; it speaks 2026-07-28"
        );

        assert_eq!(
            Fault::LegacyHandshake {
                supported: &["2026-07-28"],
            }
            .to_string(),
            "this server speaks MCP 2026-07-28 and nothing older, and that revision has no \
             initialize handshake: every request carries its own version in \"_meta\". Send \
             server/discover instead"
        );

        assert_eq!(
            Fault::UnknownMethod {
                method: "resources/list".to_owned(),
                known: &["server/discover", "tools/list"],
            }
            .to_string(),
            "no method here is called \"resources/list\"; this server answers server/discover, \
             tools/list"
        );

        assert_eq!(
            Fault::BadParams {
                because: "\"name\" must be a string".to_owned()
            }
            .to_string(),
            "those parameters are not usable: \"name\" must be a string"
        );

        assert_eq!(
            Fault::Unreachable {
                cause: "the engine closed the connection before answering".to_owned()
            }
            .to_string(),
            "the engine could not answer: the engine closed the connection before answering"
        );
    }

    /// Each fault reports under the code the specification gives it.
    #[test]
    fn every_fault_reports_under_the_code_the_specification_gives_it() {
        assert_eq!(Fault::Parse(String::new()).code(), -32700);
        assert_eq!(
            Fault::NotARequest {
                because: String::new()
            }
            .code(),
            -32600
        );
        assert_eq!(
            Fault::UnknownMethod {
                method: String::new(),
                known: &[]
            }
            .code(),
            -32601
        );
        assert_eq!(Fault::LegacyHandshake { supported: &[] }.code(), -32601);
        assert_eq!(Fault::MissingMeta { key: "" }.code(), -32602);
        assert_eq!(
            Fault::BadParams {
                because: String::new()
            }
            .code(),
            -32602
        );
        assert_eq!(
            Fault::UnsupportedVersion {
                requested: String::new(),
                supported: &[]
            }
            .code(),
            -32022
        );
        assert_eq!(
            Fault::Unreachable {
                cause: String::new()
            }
            .code(),
            -32603
        );
    }

    /// **The version error carries the list a client retries from**, in the shape
    /// the specification prints.
    ///
    /// Compared against the example JSON at
    /// `/specification/2026-07-28/basic/versioning`, quoted there as the whole
    /// response to an unsupported version.
    #[test]
    fn the_version_error_is_the_shape_the_specification_prints() {
        let id = match classify(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#) {
            Ok(Incoming::Request { id, .. }) => id,
            other => panic!("{other:?}"),
        };

        assert_eq!(
            failure(
                Some(&id),
                &Fault::UnsupportedVersion {
                    requested: "1900-01-01".to_owned(),
                    supported: &["2026-07-28", "2025-11-25"],
                }
            ),
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32022,"message":"this server does not speak protocol version 1900-01-01; it speaks 2026-07-28, 2025-11-25","data":{"requested":"1900-01-01","supported":["2026-07-28","2025-11-25"]}}}"#
        );
    }

    /// **A failure with no readable id leaves the field out**, rather than
    /// sending `null`.
    #[test]
    fn a_failure_with_no_readable_id_omits_the_field() {
        let rendered = failure(None, &Fault::Parse("expected value".to_owned()));

        assert!(!rendered.contains("\"id\""), "{rendered}");
        assert!(
            rendered.starts_with(r#"{"jsonrpc":"2.0","error":{"code":-32700,"#),
            "{rendered}"
        );
    }

    /// Nothing this crate writes contains a line break, which is what makes a
    /// line a message on MCP's stdio binding as well as on the engine's socket.
    ///
    /// The property is `narvo-ipc`'s
    /// (`no_message_this_protocol_produces_contains_a_line_break`) read at the
    /// second boundary: here the free text is a *fault message*, and one of them
    /// carries `serde_json`'s description of a parse failure, which is the string
    /// this crate does not choose.
    #[test]
    fn no_message_this_crate_writes_contains_a_line_break() {
        let parse = classify("{\n\"nope\"\n}").expect_err("it is not a message");
        let rendered = failure(None, &parse);
        assert!(!rendered.contains('\n'), "{rendered:?}");
        assert!(!rendered.contains('\r'), "{rendered:?}");

        let id = Id(serde_json::Value::String("a\nb".to_owned()));
        let rendered = answer(&id, &"two\nlines");
        assert!(!rendered.contains('\n'), "{rendered:?}");
        assert!(!rendered.contains('\r'), "{rendered:?}");
    }
}
