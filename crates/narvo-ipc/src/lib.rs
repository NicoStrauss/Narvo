//! The protocol an agent and a running engine exchange, and how to speak it.
//!
//! Responsibility: what a request and a response *are*, how each is spelled in
//! JSON, what a malformed one says, where one message stops and the next begins,
//! and — from the agent's side only — how to hold a conversation over a socket.
//!
//! **Nothing here answers a request**, and nothing here touches a `World`. The
//! code that answers one against a running world is `narvo-app`'s `ipc` module,
//! built above this in M6.3a, M6.3b and M6.3c; the listener that carries one is
//! M6.3d's `transport`, and the MCP server that speaks this protocol to an agent
//! is M6.5b.
//!
//! # What M6.5a changed about this crate, deliberately
//!
//! Until M6.5a this crate held no socket, no clock and no `std::net`, and said
//! so. It now holds [`Client`] and the framing both ends share, so the sentence
//! above is one clause longer: the crate is the protocol **and how to speak it**,
//! and it is still not what answers it. That was a decision rather than a
//! consequence, and **ADR-0033** records it, with the price of the two rejected
//! placements measured rather than argued.
//!
//! What did *not* change: this is still a leaf with no workspace dependency
//! (ADR-0030), `serde` and `serde_json` are still the whole of its normal
//! dependency list, and `std::net` is not a package, so nothing enters
//! `Cargo.lock`, the headless dependency tree or `cargo deny`'s graph.
//!
//! Read ADR-0030 before changing how a component value is spelled, ADR-0033
//! before moving the framing or the client, and `ProjektPlan.md` §6/M6 for what
//! the milestone is for.
//!
//! # The three rules that make this layer worth having
//!
//! 1. **A component crosses as the registry's own bytes.** The string in a
//!    response is what `ComponentRegistry::serialize_component` returned — RON,
//!    carried inside a JSON string, unparsed and unconverted. So the boundary is
//!    exactly as faithful as the canonical dump is, no more and no less, and
//!    `serde_json`'s three unrepresentable float values never arise because no
//!    float ever reaches it (ADR-0030).
//! 2. **A name is not a handle.** [`EntityName`] is this crate's own type, not
//!    `narvo_ecs::EntityId`, because a name that arrived from outside has not
//!    been checked against any world. Turning one into the other is a
//!    conversion somebody has to write, which is the point.
//! 3. **Nothing here executes anything.** [`Request`] is a value, and it says
//!    nothing about which tick answers it, which world it is answered against, or
//!    what a write does to a recording in flight. Those are decisions, they were
//!    taken above this crate — D19 in M6.2, the tick boundary in M6.3a, the band
//!    cut in M6.3b, the run's budget in M6.3c — and none of them is spelled
//!    anywhere in this file. Whether an exhausted budget stops a run or makes it
//!    wait is M6.3d's, and this crate is neutral on that too. See
//!    [`Request`] for which of M6's six capabilities are here and why the other
//!    four are not.
//!
//! # Determinism
//!
//! Nothing here is in the simulation path, and nothing here reads a source of
//! entropy. Serializing a request is a pure function of the request, and so is
//! framing one.
//!
//! One clock is read, and it is named rather than left to be discovered:
//! [`Client::ask`] takes `Instant::now()` for its deadline. That is the agent's
//! side of the wire, where no engine state is computed — how long a client was
//! willing to wait changes nothing about the answer, which the run produced at a
//! moment ADR-0031 fixes.
//!
//! # Examples
//!
//! ```
//! use narvo_ipc::{ComponentValue, EntityName, Request, Response};
//!
//! let request = Request::from_json(r#"{"get_entity":{"entity":"3v1"}}"#)?;
//! let entity: EntityName = "3v1".parse().expect("a well-formed name");
//! assert_eq!(request, Request::GetEntity { entity });
//!
//! // What an answer looks like. The component value is the registry's RON,
//! // carried across untouched, and `ticks_run` says how many ticks had run when
//! // the engine gave this answer — the number `narvo --ticks N` takes.
//! let answer = Response::GetEntity {
//!     entity,
//!     components: vec![ComponentValue::new("layer", "(depth:0.5)")],
//!     ticks_run: 7,
//! };
//! assert_eq!(
//!     answer.to_json(),
//!     r#"{"get_entity":{"entity":"3v1","components":[{"name":"layer","value":"(depth:0.5)"}],"ticks_run":7}}"#
//! );
//! assert_eq!(answer.ticks_run(), Some(7));
//! # Ok::<(), narvo_ipc::ProtocolError>(())
//! ```
//!
//! The whole world at once, which is what a repro test is made of:
//!
//! ```
//! use narvo_ipc::{Request, Response};
//!
//! assert_eq!(Request::Dump.to_json(), r#""dump""#);
//!
//! // The engine answers with `canonical_dump`'s own text, byte for byte what
//! // `narvo --dump` writes, so it can be written to a file and handed to
//! // `narvo --replay … --ticks 7 --expect <that file>` (ADR-0035, ADR-0036).
//! let answered = Response::from_json(
//!     r#"{"dump":{"state":"entities 1\nentity 0v1\n  layer (depth:0.5)\n","ticks_run":7}}"#,
//! )?;
//! let Response::Dump { state, ticks_run } = answered else { unreachable!() };
//! assert_eq!(state, "entities 1\nentity 0v1\n  layer (depth:0.5)\n");
//! assert_eq!(ticks_run, 7);
//! # Ok::<(), narvo_ipc::ProtocolError>(())
//! ```

mod client;
mod entity;
mod error;
pub mod framing;
mod protocol;

pub use client::{Client, ClientError};
pub use entity::{EntityName, ParseEntityNameError};
pub use error::ProtocolError;
pub use framing::{Lines, framed};
pub use protocol::{ComponentValue, Request, Response};
