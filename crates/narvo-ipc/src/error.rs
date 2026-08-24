//! What can go wrong reading a protocol message, and how it says so.

use std::error::Error;
use std::fmt;

/// Something that stopped a request or a response from being read.
///
/// # These messages are the product
///
/// The other end of this protocol is an agent, and the message is the whole of
/// the feedback it gets. Both variants answer **where**, **what** and **what to
/// do**, and both are asserted on in a test, wording included — the standard
/// `narvo-scene` set in M4.2 and `narvo-input` was held to in M5.1.
///
/// # Where comes from `serde_json`, not from here
///
/// This crate counts no lines. `serde_json` has already computed a position by
/// the time it rejects anything, and throwing it away would be a loss — the same
/// arrangement `narvo-input` has with `ron`, and the same reason.
///
/// That the position is actually *there* is a property of the representation,
/// not a given. Measured in M6.1: the externally tagged representation carries a
/// real position through all twelve malformed inputs below, including the ones
/// raised from inside [`EntityName`](crate::EntityName)'s own parser. The
/// internally tagged representation reports `0:0` for six of those six shapes —
/// serde buffers the content past the tag, so the position is gone by the time a
/// field is read, and it still locates only the failures decided before the
/// buffering starts. ADR-0030 records the choice and its table;
/// `every_malformed_request_is_located` below is what holds it, so a later switch
/// of representation shows up as a red test rather than as messages that quietly
/// stop saying where.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProtocolError {
    /// The text is not a request.
    Request {
        /// The line the parser stopped on, counting from one.
        line: usize,
        /// The column the parser stopped on, counting from one.
        column: usize,
        /// `serde_json`'s own description of what it found and what it expected.
        message: String,
    },
    /// The text is not a response.
    Response {
        /// The line the parser stopped on, counting from one.
        line: usize,
        /// The column the parser stopped on, counting from one.
        column: usize,
        /// `serde_json`'s own description of what it found and what it expected.
        message: String,
    },
}

impl ProtocolError {
    /// Wraps a `serde_json` failure that happened while reading a request.
    pub(crate) fn request(source: &serde_json::Error) -> Self {
        Self::Request {
            line: source.line(),
            column: source.column(),
            message: strip_position(&source.to_string()),
        }
    }

    /// Wraps a `serde_json` failure that happened while reading a response.
    pub(crate) fn response(source: &serde_json::Error) -> Self {
        Self::Response {
            line: source.line(),
            column: source.column(),
            message: strip_position(&source.to_string()),
        }
    }

    /// The line the parser stopped on, counting from one.
    #[must_use]
    pub fn line(&self) -> usize {
        match self {
            Self::Request { line, .. } | Self::Response { line, .. } => *line,
        }
    }

    /// The column the parser stopped on, counting from one.
    #[must_use]
    pub fn column(&self) -> usize {
        match self {
            Self::Request { column, .. } | Self::Response { column, .. } => *column,
        }
    }
}

/// Removes the trailing `at line L column C` that `serde_json` appends.
///
/// The position is carried as two numbers, and a message that also spelled it
/// out in prose would say it twice — once in a field a caller can act on and
/// once in a sentence it would have to parse back. `serde_json` appends the
/// suffix in its own `Display` for exactly the case this crate does not have:
/// nowhere else to put it.
fn strip_position(message: &str) -> String {
    match message.rfind(" at line ") {
        Some(cut) => message[..cut].to_owned(),
        None => message.to_owned(),
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request {
                line,
                column,
                message,
            } => write!(
                f,
                "{line}:{column}: the text is not a request this protocol can read: {message}"
            ),
            Self::Response {
                line,
                column,
                message,
            } => write!(
                f,
                "{line}:{column}: the text is not a response this protocol can read: {message}"
            ),
        }
    }
}

impl Error for ProtocolError {}

#[cfg(test)]
mod tests {
    use super::ProtocolError;
    use crate::{Request, Response};

    #[test]
    fn a_bad_request_says_where_what_and_what_was_expected() {
        let error = Request::from_json(r#"{"get_entity":{}}"#).expect_err("no entity field");
        assert_eq!(
            error,
            ProtocolError::Request {
                line: 1,
                column: 16,
                message: "missing field `entity`".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "1:16: the text is not a request this protocol can read: missing field `entity`"
        );
    }

    #[test]
    fn an_unknown_command_lists_every_command_there_is() {
        // The list comes from `serde_json` and costs nothing to keep in step,
        // which is why this crate does not write one of its own.
        let error = Request::from_json(r#""nonsense""#).expect_err("not a command");
        assert_eq!(
            error.to_string(),
            "1:10: the text is not a request this protocol can read: unknown variant `nonsense`, \
             expected one of `list_entities`, `get_entity`, `get_component`, `set_component`, \
             `step`, `load_scene`, `replay`, `dump`"
        );
    }

    #[test]
    fn a_bad_response_says_it_is_a_response_that_could_not_be_read() {
        let error = Response::from_json(r#"{"list_entities":{}}"#).expect_err("no entity list");
        assert_eq!(
            error,
            ProtocolError::Response {
                line: 1,
                column: 19,
                message: "missing field `entities`".to_owned()
            }
        );
        assert_eq!(
            error.to_string(),
            "1:19: the text is not a response this protocol can read: missing field `entities`"
        );
    }

    #[test]
    fn a_bad_entity_name_arrives_with_its_own_message_intact() {
        let error =
            Request::from_json(r#"{"get_entity":{"entity":"3v0"}}"#).expect_err("no generation 0");
        assert_eq!(
            error.to_string(),
            "1:30: the text is not a request this protocol can read: \"3v0\" is not an entity \
             name: generations count from one, so no entity has generation 0. The first use of a \
             slot is `v1`"
        );
    }

    /// Every way a request can be wrong still says *where* it is wrong.
    ///
    /// This is the guard on the representation, not a restatement of the cases
    /// above. `serde_json` reserves line 0 for "no position known", and the
    /// internally tagged representation produces it for six of these — the
    /// content past the tag is buffered and the position is gone by the time a
    /// field is read. Switching representation would therefore make six of this
    /// crate's messages point at `0:0`, and this is what notices.
    #[test]
    fn every_malformed_request_is_located() {
        let malformed = [
            r#""nonsense""#,
            r#"{"nonsense":{}}"#,
            r#"{"get_entity":{}}"#,
            r#"{"get_entity":{"entity":"3x1"}}"#,
            r#"{"get_entity":{"entity":"3v0"}}"#,
            r#"{"get_entity":{"entity":""}}"#,
            r#"{"get_entity":{"entity":3}}"#,
            r#"{"get_entity":{"entity":"3v1","extra":true}}"#,
            r#"{"get_component":{"entity":"3v1"}}"#,
            r#"not json at all"#,
            r#"{"get_entity":{"entity":"3v1"}} trailing"#,
            r#"[]"#,
        ];

        for text in malformed {
            let error = Request::from_json(text).expect_err("every one of these is malformed");
            assert!(
                error.line() >= 1 && error.column() >= 1,
                "{text} was rejected without a position: {error}"
            );
        }
    }

    /// An unknown field is refused rather than ignored.
    ///
    /// A protocol that silently drops what it does not understand answers a
    /// request the sender did not make. The cost is stated in ADR-0030: this is
    /// strict in the direction that has no deployed client yet.
    #[test]
    fn an_unknown_field_is_refused_and_names_what_was_expected() {
        let error = Request::from_json(r#"{"get_entity":{"entity":"3v1","extra":true}}"#)
            .expect_err("extra is not a field of get_entity");
        assert_eq!(
            error.to_string(),
            "1:37: the text is not a request this protocol can read: unknown field `extra`, \
             expected `entity`"
        );
    }
}
