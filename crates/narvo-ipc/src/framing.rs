//! One JSON message per line, in one implementation for both ends of a
//! connection.
//!
//! # Why this is here rather than beside the socket
//!
//! It was in `narvo-app`'s `transport` module until M6.5a, inlined into the
//! type that owns the listener, and that was right while there was one end. The
//! client half (M6.5a) and the MCP server that uses it (M6.5b) are the second
//! end, they cannot depend on the application crate, and the third resolution —
//! writing the splitting a second time — would put **two framings at the two
//! ends of one connection**, which disagree only in the field. ADR-0033 records
//! the decision and prices the alternatives.
//!
//! # What a frame is
//!
//! A message is a line and a line is a message. Both directions, both ends.
//!
//! That rests on one property of what this protocol produces:
//! [`Request::to_json`](crate::Request::to_json) and
//! [`Response::to_json`](crate::Response::to_json) are `serde_json`'s compact
//! form, which contains no newline — not even when a component value carries
//! one, because a control character inside a JSON string is escaped rather than
//! written. That is load-bearing, so it is checked rather than assumed:
//! `no_message_this_protocol_produces_contains_a_line_break` below feeds a
//! literal newline through a component value and looks for it in the output.
//!
//! # The two halves
//!
//! [`Lines`] is the reading half: bytes as they arrived, whole lines out, a
//! partial one held. [`framed`] is the writing half, and it is one line of code
//! on purpose — the value is that both ends call *it* rather than each appending
//! a terminator of its own choosing.

/// Bytes as they arrived, and the whole lines in them.
///
/// One buffer per connection. Nothing here knows what a line means, which is
/// what lets the same type serve a reader of requests and a reader of responses.
///
/// # A partial line is held, not guessed at
///
/// TCP delivers a byte stream, so a write of one message can arrive as three
/// reads and three messages can arrive as one. [`feed`](Self::feed) therefore
/// returns only what a newline has ended, and [`unfinished`](Self::unfinished)
/// says whether anything is being held back — which is how a caller tells a
/// stream that ended cleanly from one that stopped in the middle of a message.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Lines {
    /// Bytes received that do not yet end a line.
    partial: Vec<u8>,
}

impl Lines {
    /// A buffer with nothing in it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes bytes as they arrived and returns the lines they completed.
    ///
    /// The terminator is `\n`, and a `\r` immediately before it is dropped as
    /// well. That second half is tolerance rather than symmetry: [`framed`]
    /// never writes a `\r`, but an agent's own client library, or a person with
    /// a terminal, may — and a message refused for a byte nobody meant to send
    /// would be a bad first experience of this protocol.
    ///
    /// An empty line comes back as an empty `Vec`. It is a complete line, so
    /// this reports it; whether an empty message is worth answering is a
    /// question for the end that reads it, and both ends of this transport skip
    /// it.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.partial.extend_from_slice(bytes);

        let mut lines = Vec::new();
        while let Some(end) = self.partial.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<u8> = self.partial.drain(..=end).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            lines.push(line);
        }
        lines
    }

    /// Whether bytes are held that no newline has ended yet.
    ///
    /// **This is what makes "the stream ended" and "the stream ended mid-message"
    /// different answers.** A peer that closes with this false said everything it
    /// meant to; one that closes with it true was cut off, and a caller that
    /// cannot tell them apart reports a truncated answer as an absent one.
    #[must_use]
    pub fn unfinished(&self) -> bool {
        !self.partial.is_empty()
    }
}

/// One message as one line.
///
/// The other half of [`Lines`], and the reason it is a function rather than a
/// `push('\n')` at each call site: a terminator chosen twice is a terminator
/// that can be chosen differently twice.
///
/// `message` is expected to contain no newline of its own. Everything this
/// protocol produces satisfies that (see the module documentation, and the test
/// that checks it); a caller that frames something else is framing text this
/// protocol did not write.
#[must_use]
pub fn framed(message: &str) -> String {
    let mut line = String::with_capacity(message.len() + 1);
    line.push_str(message);
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::{Lines, framed};
    use crate::{ComponentValue, EntityName, Request, Response};

    /// A whole line in one go is one message.
    #[test]
    fn a_whole_line_is_one_message() {
        let mut lines = Lines::new();
        assert_eq!(
            lines.feed(b"\"list_entities\"\n"),
            vec![b"\"list_entities\"".to_vec()]
        );
        assert!(!lines.unfinished());
    }

    /// **Half a message is not a message**, which is the whole reason this type
    /// exists rather than a `split(b'\n')`.
    #[test]
    fn a_partial_line_is_held_until_the_rest_of_it_arrives() {
        let mut lines = Lines::new();

        assert!(
            lines.feed(b"\"list_ent").is_empty(),
            "half a message came out"
        );
        assert!(lines.unfinished(), "and the half was not kept");

        assert_eq!(
            lines.feed(b"ities\"\n"),
            vec![b"\"list_entities\"".to_vec()]
        );
        assert!(!lines.unfinished());
    }

    /// A message split into single bytes is still exactly one message.
    #[test]
    fn a_message_delivered_one_byte_at_a_time_is_still_one_message() {
        let mut lines = Lines::new();
        let message = b"\"list_entities\"\n";

        let mut collected = Vec::new();
        for byte in message {
            collected.extend(lines.feed(&[*byte]));
        }

        assert_eq!(collected, vec![b"\"list_entities\"".to_vec()]);
        assert!(!lines.unfinished());
    }

    /// Three messages in one delivery are three messages, in order.
    #[test]
    fn several_messages_in_one_delivery_come_out_in_order() {
        let mut lines = Lines::new();
        assert_eq!(
            lines.feed(b"one\ntwo\nthree\n"),
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
        assert!(!lines.unfinished());
    }

    /// A delivery that ends mid-message yields the whole ones and keeps the rest.
    #[test]
    fn a_delivery_that_ends_mid_message_keeps_the_remainder() {
        let mut lines = Lines::new();

        assert_eq!(lines.feed(b"one\ntw"), vec![b"one".to_vec()]);
        assert!(lines.unfinished(), "the remainder was dropped");

        assert_eq!(lines.feed(b"o\n"), vec![b"two".to_vec()]);
        assert!(!lines.unfinished());
    }

    /// A `\r\n` terminator loses the `\r` as well as the `\n`.
    #[test]
    fn a_carriage_return_before_the_newline_is_dropped() {
        let mut lines = Lines::new();
        assert_eq!(
            lines.feed(b"one\r\ntwo\n"),
            vec![b"one".to_vec(), b"two".to_vec()]
        );
    }

    /// A `\r` that is not before a newline is content and stays.
    ///
    /// The tolerance above is for a line ending, not for the byte: a component
    /// value containing a carriage return is a value, and eating it would be a
    /// framing that quietly rewrites what it carries.
    #[test]
    fn a_carriage_return_anywhere_else_is_content() {
        let mut lines = Lines::new();
        assert_eq!(lines.feed(b"on\re\n"), vec![b"on\re".to_vec()]);
    }

    /// An empty line is a complete line and is reported as one.
    #[test]
    fn an_empty_line_is_a_line() {
        let mut lines = Lines::new();
        assert_eq!(
            lines.feed(b"\n\none\n"),
            vec![Vec::new(), Vec::new(), b"one".to_vec()]
        );
    }

    /// Bytes with no terminator at all are never a message, however many arrive.
    ///
    /// The second stage of red edge (a): a framing can split correctly and still
    /// lose the last incomplete line by treating end-of-stream as a terminator.
    /// This is the property that says it does not.
    #[test]
    fn a_line_that_never_ends_is_never_a_message() {
        let mut lines = Lines::new();

        for _ in 0..100 {
            assert!(lines.feed(b"not finished yet ").is_empty());
        }
        assert!(
            lines.unfinished(),
            "the bytes were dropped rather than held"
        );
    }

    /// The writing half puts exactly one terminator on and changes nothing else.
    #[test]
    fn framing_a_message_appends_one_newline_and_nothing_else() {
        assert_eq!(
            framed("{\"step\":{\"granted\":9}}"),
            "{\"step\":{\"granted\":9}}\n"
        );
        assert_eq!(framed(""), "\n");
    }

    /// **The two halves are inverse**, which is the property both ends rely on.
    #[test]
    fn what_the_writing_half_frames_the_reading_half_returns() {
        let messages = ["\"list_entities\"", "{\"step\":{\"ticks\":3}}", ""];

        let mut written = String::new();
        for message in messages {
            written.push_str(&framed(message));
        }

        let mut lines = Lines::new();
        let read = lines.feed(written.as_bytes());

        assert_eq!(
            read,
            messages
                .iter()
                .map(|message| message.as_bytes().to_vec())
                .collect::<Vec<_>>()
        );
        assert!(!lines.unfinished());
    }

    /// **No message this protocol produces contains a line break**, which is what
    /// makes a line a message at all.
    ///
    /// Checked rather than assumed, and checked at the place it would break: a
    /// component value is the one field whose content the protocol does not
    /// choose, and ADR-0030 has it crossing as the registry's own bytes. A
    /// literal newline inside one is escaped by `serde_json` into the two
    /// characters `\` and `n`, so the byte never reaches the wire.
    #[test]
    fn no_message_this_protocol_produces_contains_a_line_break() {
        let entity: EntityName = "3v1".parse().expect("a well-formed name");

        let requests = [
            Request::ListEntities,
            Request::GetEntity { entity },
            Request::SetComponent {
                entity,
                component: "label".to_owned(),
                value: "(text:\"two\nlines\")".to_owned(),
            },
        ];
        for request in &requests {
            let json = request.to_json();
            assert!(!json.contains('\n'), "{json:?}");
            assert!(!json.contains('\r'), "{json:?}");
        }

        let responses = [
            Response::Step {
                granted: 9,
                ticks_run: 4,
            },
            Response::GetEntity {
                entity,
                components: vec![ComponentValue::new("label", "(text:\"two\nlines\")")],
                ticks_run: 4,
            },
            // A canonical dump is the response that is *made* of newlines — one
            // per entity and one per component — so it is the sharpest case this
            // check has, and M6.7b put it here rather than trusting the two above
            // to speak for it.
            Response::Dump {
                state: "entities 1\nentity 0v1\n  layer (depth:0.5)\n".to_owned(),
                ticks_run: 4,
            },
            Response::Error {
                message: "something\nwith a break".to_owned(),
            },
        ];
        for response in &responses {
            let json = response.to_json();
            assert!(!json.contains('\n'), "{json:?}");
            assert!(!json.contains('\r'), "{json:?}");
        }

        // And the escaped form survives the round trip, so the check above is
        // not bought by losing the value.
        let carried = Response::GetEntity {
            entity,
            components: vec![ComponentValue::new("label", "(text:\"two\nlines\")")],
            ticks_run: 4,
        };
        assert_eq!(
            Response::from_json(&carried.to_json()).expect("its own output"),
            carried
        );

        // The same, for the response whose payload is nothing but lines. If a
        // dump did not survive this round trip byte for byte, `--expect` would
        // reject what the wire handed an agent and M6.7b's whole chain would be
        // a longer way of not closing the gap.
        let dumped = Response::Dump {
            state: "entities 2\nentity 0v1\n  layer (depth:0.5)\nentity 1v1\n".to_owned(),
            ticks_run: 4,
        };
        let Ok(Response::Dump { state, .. }) = Response::from_json(&dumped.to_json()) else {
            panic!("a dump has to come back as a dump");
        };
        assert_eq!(
            state,
            "entities 2\nentity 0v1\n  layer (depth:0.5)\nentity 1v1\n"
        );
    }
}
