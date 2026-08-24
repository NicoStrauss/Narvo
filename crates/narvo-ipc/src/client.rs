//! The agent's side of the wire: connect to a running engine and ask it things.
//!
//! # What this is for
//!
//! `narvo-app`'s `transport` module is the engine's end of a connection — a
//! listener, one client at a time, answers written back. This is the other end,
//! and it exists because M6.5b's MCP server needs one and **cannot reach the
//! application crate**: a tool depending on `narvo-app` would take 185 packages
//! for a type that needs 14. ADR-0033 has the measurement and the two rejected
//! paths.
//!
//! Both ends frame with [`crate::framing`], which is the point of the move: one
//! implementation, so the two ends of a connection cannot disagree about where a
//! message stops.
//!
//! # Why there is no feature gate here
//!
//! D20 put the *listener* behind a default-off feature because a loopback
//! listener has no access check — any process on the machine can connect to one.
//! That reasoning is about accepting connections, and it does not reach this
//! file: a client opens an outbound connection and grants nobody access to
//! anything. So this compiles in every configuration, which is also what puts it
//! inside the three headless steps of the verification set rather than outside
//! them (the M4.8 shape `CLAUDE.md` records).
//!
//! # Determinism
//!
//! [`Client::ask`] reads a clock — `Instant::now()`, for the deadline below.
//! Nothing else in this crate does, and nothing here is in the simulation path:
//! this is the agent's side of the wire, where no engine state is computed. The
//! engine's own answer is produced by the run, at a moment ADR-0031 fixes, and
//! how long a client was willing to wait for it changes nothing about it.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;
use std::io::{ErrorKind, Read as _, Write as _};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use crate::framing::{Lines, framed};
use crate::{ProtocolError, Request, Response};

/// A connection to a running engine.
///
/// One request, one answer, in order — which is what the engine offers: it
/// serves one client at a time and answers in the order it was asked.
#[derive(Debug)]
pub struct Client {
    stream: TcpStream,
    /// The reading half of the framing, holding whatever is half-arrived.
    lines: Lines,
    /// Whole lines that have arrived but not yet been handed out.
    ///
    /// A single read can deliver two answers. Dropping the second would lose an
    /// answer to a question that was asked, so it waits here for the next call
    /// rather than for a second delivery that is never coming.
    queued: VecDeque<Vec<u8>>,
    patience: Duration,
}

impl Client {
    /// Connects to an engine that is serving an agent.
    ///
    /// `address` is what `--ipc` was given, in `host:port` form. `patience` is
    /// the read deadline described on [`ask`](Self::ask).
    ///
    /// **`patience` does not bound the connecting**, and that is a named limit
    /// rather than an oversight: `TcpStream::connect_timeout` needs a resolved
    /// `SocketAddr` and this takes text, so how long a refusal takes is the
    /// operating system's. Measured on this machine, connecting to a loopback
    /// port nothing is listening on returns in about 2 s on Windows and at once
    /// on Linux — `connecting_to_nothing_names_the_address` is the slowest test
    /// in this crate for that reason. It is a bound, but it is not this one's.
    ///
    /// # Errors
    ///
    /// [`ClientError::Connect`] if there is nothing there, naming the address
    /// and carrying the operating system's own words.
    pub fn connect(address: &str, patience: Duration) -> Result<Self, ClientError> {
        let stream = TcpStream::connect(address).map_err(|cause| ClientError::Connect {
            address: address.to_owned(),
            cause,
        })?;

        Ok(Self {
            stream,
            lines: Lines::new(),
            queued: VecDeque::new(),
            patience,
        })
    }

    /// Sends one request and reads the one answer to it.
    ///
    /// # The deadline is a brake, not a synchronisation
    ///
    /// **This is the v0.89 rule applied on purpose rather than inherited.**
    /// M6.3d spent seven sessions learning that a blocking read with no bound
    /// turns a failing test suite into a silent one, and the lesson lives in
    /// `agent_socket.rs`'s `PATIENCE` and in `transport.rs`'s test helper — in
    /// *those* files, and in no file beside them. This is a new file, so it
    /// carries the bound explicitly.
    ///
    /// `patience` is therefore set far above anything a working exchange needs.
    /// It is not tuned to a round trip and must never be: **if this ever returns
    /// [`ClientError::Silent`] on a healthy engine, the answer is not a bigger
    /// number** — it is that something is genuinely stuck, and `ProjektPlan.md`
    /// §9.2's rule against tuning a timeout until a flake disappears applies
    /// exactly here.
    ///
    /// The bound is on the whole exchange rather than on one read, so a peer
    /// dribbling one byte at a time cannot extend it indefinitely.
    ///
    /// # Nothing arrived and the engine went away are different answers
    ///
    /// This is M6.3d's own defect, and a client that repeated it would repeat it
    /// mirrored. A read that finds nothing within the deadline is
    /// [`ClientError::Silent`]: the connection is open and the engine simply has
    /// not answered — which is what a long `step` looks like from here. A read
    /// that finds end-of-stream is [`ClientError::Closed`]: nothing more can ever
    /// arrive. Conflating them would report a run that is still thinking as a run
    /// that has gone.
    ///
    /// # Errors
    ///
    /// [`ClientError::Send`] if the request could not be written,
    /// [`ClientError::Silent`] or [`ClientError::Closed`] as above,
    /// [`ClientError::Receive`] for any other read failure, and
    /// [`ClientError::Malformed`] if what came back is not a response.
    pub fn ask(&mut self, request: &Request) -> Result<Response, ClientError> {
        let line = framed(&request.to_json());
        self.stream
            .write_all(line.as_bytes())
            .map_err(|cause| ClientError::Send { cause })?;
        self.stream
            .flush()
            .map_err(|cause| ClientError::Send { cause })?;

        self.answer()
    }

    /// Reads until one whole answer is in hand, or until the deadline.
    fn answer(&mut self) -> Result<Response, ClientError> {
        let deadline = Instant::now() + self.patience;
        let mut buffer = [0_u8; 4096];

        loop {
            if let Some(line) = self.queued.pop_front() {
                let text = String::from_utf8_lossy(&line);
                return Response::from_json(&text).map_err(ClientError::Malformed);
            }

            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ClientError::Silent {
                    patience: self.patience,
                });
            }

            // **The zero check above is load-bearing**, and it was measured
            // rather than assumed: on the pinned toolchain
            // `set_read_timeout(Some(Duration::ZERO))` returns
            // `InvalidInput`/"cannot set a 0 duration timeout" on both platforms,
            // so a deadline that has just run out would come back as a fault
            // rather than as the timeout it is.
            //
            // A *sub-millisecond* remainder needs no such guard, which was the
            // other half of the same measurement: Windows counts this timeout in
            // milliseconds, so 1 us rounding down to 0 would mean "wait for ever"
            // — the M6.3d failure exactly. It does not. A 1 us bound expired in
            // 19.5 ms on Windows and 11.1 ms on Linux; std rounds up rather than
            // truncating.
            self.stream
                .set_read_timeout(Some(left))
                .map_err(|cause| ClientError::Receive { cause })?;

            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    return Err(ClientError::Closed {
                        mid_answer: self.lines.unfinished(),
                    });
                }
                Err(cause) if is_gone(&cause) => {
                    return Err(ClientError::Closed {
                        mid_answer: self.lines.unfinished(),
                    });
                }
                Ok(read) => {
                    // An empty line is a complete line the framing reports and
                    // neither end of this transport answers. Skipping it here
                    // keeps a peer that sends blank lines from producing an
                    // answer nobody could parse.
                    self.queued.extend(
                        self.lines
                            .feed(&buffer[..read])
                            .into_iter()
                            .filter(|line| !line.is_empty()),
                    );
                }
                Err(cause) if cause.kind() == ErrorKind::Interrupted => {}
                Err(cause) if is_deadline(&cause) => {
                    return Err(ClientError::Silent {
                        patience: self.patience,
                    });
                }
                Err(cause) => return Err(ClientError::Receive { cause }),
            }
        }
    }
}

/// Whether a failed read is the deadline rather than a fault.
///
/// **Two kinds, because the two platforms spell it differently**, and that was
/// measured rather than read: an expired `set_read_timeout` comes back as
/// `TimedOut` (os error 10060) on Windows and as `WouldBlock` (os error 11) on
/// Linux, on the pinned 1.97.1 toolchain. Recognising only one of them would
/// make this client report a plain timeout as an I/O fault on exactly one of the
/// two platforms the project builds for — the kind of difference that shows up
/// in CI and not locally.
fn is_deadline(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock)
}

/// Whether a failed read means the peer is gone rather than the socket is
/// broken.
///
/// **An engine that goes away does not always send an end-of-stream**, and this
/// was found by a failing test rather than foreseen. Measured on the pinned
/// toolchain, on both platforms:
///
/// | how the peer went | what the reader sees |
/// |---|---|---|
/// | closed having read everything sent to it | `Ok(0)` |
/// | `shutdown(Write)`, whatever is unread | `Ok(0)`, after every byte it wrote |
/// | closed with bytes it never read still queued | `Err(ConnectionReset)` |
///
/// The third row is ordinary TCP — a close with unread data in the receive queue
/// is a reset rather than a FIN — and it is *the normal way an engine dies*,
/// because a client that has just sent a request has almost always sent one the
/// engine will now never read. Windows also produces `ConnectionAborted` for it,
/// which is what the first run of
/// `an_engine_that_goes_away_is_reported_as_closed` reported (os error 10053).
/// Both kinds mean the same thing to a client: nothing more can ever arrive.
///
/// **What the two platforms do not agree on is what survives the reset**, and
/// [`ClientError::Closed::mid_answer`] says what that costs.
fn is_gone(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
    )
}

/// What stopped a client from getting its answer.
///
/// # These messages are the product
///
/// The same standard [`ProtocolError`] is held to: the other end of this is an
/// agent, and the message is the whole of the feedback it gets. Every variant is
/// asserted on in a test, wording included.
///
/// **Except where the operating system supplies the words.** The `cause` of
/// [`Connect`](Self::Connect), [`Send`](Self::Send) and
/// [`Receive`](Self::Receive) is `std::io::Error`'s own text, and that text is
/// **localised** — the same refused connection reads in German on this machine
/// and in English on a CI runner. So the tests below assert the part written
/// here and never the whole string, and a message that embedded an OS
/// description in a checked prefix would be a test that passes in one language.
///
/// # This is not the taxonomy M6.3d deferred
///
/// That one is about what an *engine* tells an agent went wrong with a request
/// it understood, it branches on the engine's side, and it is M6.5b's. This is
/// the client's own account of a conversation that did not happen.
#[derive(Debug)]
#[non_exhaustive]
pub enum ClientError {
    /// There was nothing to connect to.
    Connect {
        /// The address that was tried.
        address: String,
        /// The operating system's own words.
        cause: std::io::Error,
    },
    /// The request could not be written.
    Send {
        /// The operating system's own words.
        cause: std::io::Error,
    },
    /// The answer could not be read, for a reason that is not the deadline and
    /// is not the peer closing.
    Receive {
        /// The operating system's own words.
        cause: std::io::Error,
    },
    /// The deadline passed on a connection that is still open.
    Silent {
        /// How long was waited.
        patience: Duration,
    },
    /// The engine closed the connection.
    Closed {
        /// Whether bytes of an unfinished answer were in hand when it ended.
        ///
        /// True says the answer was cut off rather than never started, which
        /// points at the engine rather than at the question.
        ///
        /// **It reports what this client holds, and does not claim what the
        /// engine wrote.** A peer that closes with unread bytes queued resets
        /// the connection instead of ending the stream, and the two platforms
        /// then disagree about what had already been delivered: measured on the
        /// pinned toolchain, an engine that wrote `{"step":{"gran` and closed
        /// without reading its request leaves those bytes readable on Linux and
        /// discards them on Windows. So `false` is not evidence that nothing was
        /// written; `true` is evidence that something was. A caller that
        /// branched on it as though it were symmetric would be right on one
        /// platform.
        ///
        /// After a graceful end — an end-of-stream rather than a reset, which is
        /// what a run that finishes normally produces — it is exact on both.
        mid_answer: bool,
    },
    /// Something whole arrived and it is not a response.
    Malformed(ProtocolError),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect { address, cause } => {
                write!(f, "could not reach an engine at {address}: {cause}")
            }
            Self::Send { cause } => write!(f, "the request could not be sent: {cause}"),
            Self::Receive { cause } => write!(f, "the answer could not be read: {cause}"),
            Self::Silent { patience } => write!(
                f,
                "no answer arrived within {patience:?}, on a connection that is still open: \
                 the engine is there and has not answered"
            ),
            Self::Closed { mid_answer: false } => {
                write!(f, "the engine closed the connection before answering")
            }
            Self::Closed { mid_answer: true } => write!(
                f,
                "the engine closed the connection in the middle of an answer"
            ),
            Self::Malformed(cause) => {
                write!(f, "the engine answered with something unreadable: {cause}")
            }
        }
    }
}

impl Error for ClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connect { cause, .. } | Self::Send { cause } | Self::Receive { cause } => {
                Some(cause)
            }
            Self::Malformed(cause) => Some(cause),
            Self::Silent { .. } | Self::Closed { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Client, ClientError};
    use crate::{Request, Response};
    use std::io::Write as _;
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::time::Duration;

    /// How long a client in these tests waits before deciding nothing is coming.
    ///
    /// **A brake, not a synchronisation**, and the same twenty seconds
    /// `agent_socket.rs` and `transport.rs` use. Every exchange below is a
    /// loopback round trip inside one process, so this is not tuned to any of
    /// them; it is here so a test that would hang fails instead.
    const PATIENCE: Duration = Duration::from_secs(20);

    /// A client, and the raw socket standing in for an engine.
    ///
    /// **No thread anywhere in this file, and that is deliberate.** The fake
    /// engine writes its answer *before* the client asks for it, which a TCP
    /// send buffer holds without a reader — so every test below runs on one
    /// thread with no scheduling in it. The one place a real engine, a real
    /// process and a real `Endpoint` meet this client is
    /// `narvo-app`'s own suite, where it belongs.
    fn connected() -> (Client, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let address = listener.local_addr().expect("it just bound");

        let client = Client::connect(&address.to_string(), PATIENCE).expect("our own listener");
        let (engine, _peer) = listener.accept().expect("the connection we just made");

        (client, engine)
    }

    #[test]
    fn an_answer_comes_back_as_a_response() {
        let (mut client, mut engine) = connected();
        engine
            .write_all(b"{\"step\":{\"granted\":9,\"ticks_run\":4}}\n")
            .expect("write to our own socket");

        assert_eq!(
            client.ask(&Request::Step { ticks: 9 }).expect("an answer"),
            Response::Step {
                granted: 9,
                ticks_run: 4,
            }
        );
    }

    /// **What the client writes is what the engine's framing reads.**
    ///
    /// The request is checked on the wire rather than only through a round trip,
    /// because a round trip against a fake engine would prove only that this
    /// file agrees with itself.
    #[test]
    fn a_request_goes_out_as_one_framed_line() {
        use std::io::{BufRead as _, BufReader};

        let (mut client, engine) = connected();
        engine
            .set_read_timeout(Some(PATIENCE))
            .expect("a read bound on a socket we own");
        let mut reader = BufReader::new(engine);

        // The answer is queued first so `ask` returns rather than waiting.
        reader
            .get_mut()
            .write_all(b"{\"step\":{\"granted\":1,\"ticks_run\":4}}\n")
            .expect("write");
        client
            .ask(&Request::Step { ticks: 1 })
            .expect("the queued answer");

        let mut line = String::new();
        reader.read_line(&mut line).expect("the request arrived");
        assert_eq!(line, "{\"step\":{\"ticks\":1}}\n");
    }

    /// Two answers in one delivery are two answers, not one and a loss.
    #[test]
    fn two_answers_in_one_delivery_are_both_kept() {
        let (mut client, mut engine) = connected();
        engine
            .write_all(b"{\"step\":{\"granted\":1,\"ticks_run\":4}}\n{\"step\":{\"granted\":2,\"ticks_run\":4}}\n")
            .expect("write");

        assert_eq!(
            client.ask(&Request::Step { ticks: 1 }).expect("first"),
            Response::Step {
                granted: 1,
                ticks_run: 4,
            }
        );
        assert_eq!(
            client.ask(&Request::Step { ticks: 2 }).expect("second"),
            Response::Step {
                granted: 2,
                ticks_run: 4,
            },
            "the second answer in one delivery was dropped"
        );
    }

    /// An answer delivered in pieces is one answer.
    #[test]
    fn an_answer_split_across_deliveries_is_still_one_answer() {
        let (mut client, mut engine) = connected();
        engine.write_all(b"{\"step\":{\"gr").expect("write");
        engine
            .write_all(b"anted\":7,\"ticks_run\":4}}\n")
            .expect("write");

        assert_eq!(
            client.ask(&Request::Step { ticks: 7 }).expect("an answer"),
            Response::Step {
                granted: 7,
                ticks_run: 4,
            }
        );
    }

    /// **Nothing arrived is not the same as the engine went away.**
    ///
    /// The connection is open and the engine says nothing, which is what a long
    /// `step` looks like from here. The patience is short because the *waiting*
    /// is what this test is about — it is the subject, not a brake, and no
    /// answer is ever coming, so a slower machine can only make it slower.
    #[test]
    fn a_silent_engine_is_reported_as_silent_and_not_as_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let address = listener.local_addr().expect("it just bound");
        let mut client = Client::connect(&address.to_string(), Duration::from_millis(250))
            .expect("our own listener");
        let _engine = listener.accept().expect("the connection we just made");

        match client.ask(&Request::ListEntities) {
            Err(ClientError::Silent { patience }) => {
                assert_eq!(patience, Duration::from_millis(250));
            }
            other => panic!("a silent engine was reported as {other:?}"),
        }
    }

    /// A peer that ends the stream is reported as closed, and as not mid-answer.
    ///
    /// `shutdown(Write)` rather than a drop, and the difference was measured
    /// rather than chosen for style: a drop with the request still unread resets
    /// the connection instead of ending it, which is the case below. A half-close
    /// is the graceful ending, and it produces `Ok(0)` on both platforms whatever
    /// is unread.
    #[test]
    fn an_engine_that_ends_the_stream_is_reported_as_closed() {
        let (mut client, engine) = connected();
        engine.shutdown(Shutdown::Write).expect("our own socket");

        match client.ask(&Request::ListEntities) {
            Err(ClientError::Closed { mid_answer }) => {
                assert!(!mid_answer, "nothing had arrived, so nothing was cut off");
            }
            other => panic!("a departed engine was reported as {other:?}"),
        }
    }

    /// **An engine that vanishes mid-answer says so**, which is red edge (d).
    ///
    /// The partial answer is written first and the stream ended after it, so
    /// this is the case where `mid_answer` is exact on both platforms — the
    /// reset path, where it is not, is the test below.
    #[test]
    fn an_engine_that_vanishes_mid_answer_says_it_was_mid_answer() {
        let (mut client, mut engine) = connected();
        engine.write_all(b"{\"step\":{\"gran").expect("write");
        engine.flush().expect("flush");
        engine.shutdown(Shutdown::Write).expect("our own socket");

        match client.ask(&Request::ListEntities) {
            Err(ClientError::Closed { mid_answer }) => {
                assert!(
                    mid_answer,
                    "a truncated answer was reported as an absent one"
                );
            }
            other => panic!("a truncated answer was reported as {other:?}"),
        }
    }

    /// **A reset is the engine going away, not the socket breaking.**
    ///
    /// This is how an engine normally dies as seen from here: it is dropped with
    /// a request in its receive queue that it will now never read, and TCP sends
    /// a reset rather than an end-of-stream. It arrived as a *failing test*
    /// rather than as a design note — the first version of this file classified
    /// it as [`ClientError::Receive`], and an engine that had simply gone was
    /// reported to the agent as a broken connection.
    ///
    /// Only the variant is asserted. `mid_answer` is deliberately not, because
    /// the two platforms disagree about what survives a reset, and a test that
    /// pinned it here would be green on one runner and red on the other.
    #[test]
    fn an_engine_dropped_with_the_request_unread_is_closed_rather_than_broken() {
        let (mut client, engine) = connected();
        drop(engine);

        match client.ask(&Request::ListEntities) {
            Err(ClientError::Closed { .. }) => {}
            other => panic!("a reset connection was reported as {other:?}"),
        }
    }

    /// A whole line that is not a response is refused with the protocol's own
    /// words, position included.
    #[test]
    fn a_line_that_is_not_a_response_is_refused_where_it_went_wrong() {
        let (mut client, mut engine) = connected();
        engine.write_all(b"not json at all\n").expect("write");

        match client.ask(&Request::ListEntities) {
            Err(ClientError::Malformed(cause)) => {
                assert_eq!(cause.line(), 1);
                assert!(cause.column() >= 1);
            }
            other => panic!("a malformed answer was reported as {other:?}"),
        }
    }

    /// **Bytes that are not text at all are refused, not panicked on.**
    ///
    /// Red edge (d), measured rather than assumed: the framing works on bytes
    /// and nothing between the socket and here promises they are UTF-8. The
    /// lossy conversion turns what arrived into replacement characters, which
    /// `serde_json` then rejects with a position — so a hostile or confused peer
    /// gets an error and the client stays usable.
    #[test]
    fn bytes_that_are_not_text_are_refused_rather_than_panicked_on() {
        let (mut client, mut engine) = connected();
        engine
            .write_all(&[0xff, 0xfe, 0x80, 0x00, b'\n'])
            .expect("write");

        match client.ask(&Request::ListEntities) {
            Err(ClientError::Malformed(cause)) => assert!(cause.column() >= 1),
            other => panic!("bytes that are not text were reported as {other:?}"),
        }
    }

    /// Blank lines are skipped rather than answered.
    #[test]
    fn blank_lines_before_an_answer_are_skipped() {
        let (mut client, mut engine) = connected();
        engine
            .write_all(b"\n\n{\"step\":{\"granted\":3,\"ticks_run\":4}}\n")
            .expect("write");

        assert_eq!(
            client.ask(&Request::Step { ticks: 3 }).expect("an answer"),
            Response::Step {
                granted: 3,
                ticks_run: 4,
            }
        );
    }

    /// Connecting to nothing is an error that names the address.
    ///
    /// The port is one the operating system had just handed out and nothing is
    /// listening on it any more. What this cannot rule out is another process
    /// taking the same ephemeral port in between — the same limit
    /// `agent_socket.rs`'s `Drop` names for its own liveness check, and the
    /// reason the assertion is on the wording rather than on the failure kind.
    #[test]
    fn connecting_to_nothing_names_the_address() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let address = listener.local_addr().expect("it just bound").to_string();
        drop(listener);

        let error = Client::connect(&address, PATIENCE).expect_err("nobody is listening");
        match &error {
            ClientError::Connect { address: named, .. } => assert_eq!(named, &address),
            other => panic!("expected a connect failure, got {other:?}"),
        }
        assert!(
            error
                .to_string()
                .starts_with(&format!("could not reach an engine at {address}: ")),
            "{error}"
        );
    }

    /// **Every message this type can produce, in its own words.**
    ///
    /// The three that carry an `std::io::Error` are checked by prefix and no
    /// further: that text is the operating system's and is localised, so an
    /// equality assertion here would pass on an English runner and fail on this
    /// German machine.
    ///
    /// The `250ms` in the silent message is `Duration`'s own `Debug`, which makes
    /// it a content anchor over something std generates (ADR-0008's third kind):
    /// if std ever spells a quarter second differently, this goes red and that is
    /// the finding.
    #[test]
    fn every_client_error_says_what_happened_in_its_own_words() {
        let refused = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);

        let connect = ClientError::Connect {
            address: "127.0.0.1:7".to_owned(),
            cause: refused,
        };
        assert!(
            connect
                .to_string()
                .starts_with("could not reach an engine at 127.0.0.1:7: "),
            "{connect}"
        );

        let send = ClientError::Send {
            cause: std::io::Error::from(std::io::ErrorKind::BrokenPipe),
        };
        assert!(
            send.to_string()
                .starts_with("the request could not be sent: "),
            "{send}"
        );

        let receive = ClientError::Receive {
            cause: std::io::Error::from(std::io::ErrorKind::ConnectionReset),
        };
        assert!(
            receive
                .to_string()
                .starts_with("the answer could not be read: "),
            "{receive}"
        );

        assert_eq!(
            ClientError::Silent {
                patience: Duration::from_millis(250)
            }
            .to_string(),
            "no answer arrived within 250ms, on a connection that is still open: the engine is \
             there and has not answered"
        );

        assert_eq!(
            ClientError::Closed { mid_answer: false }.to_string(),
            "the engine closed the connection before answering"
        );
        assert_eq!(
            ClientError::Closed { mid_answer: true }.to_string(),
            "the engine closed the connection in the middle of an answer"
        );

        let malformed = ClientError::Malformed(
            Response::from_json("not json at all").expect_err("it is not json"),
        );
        assert_eq!(
            malformed.to_string(),
            "the engine answered with something unreadable: 1:1: the text is not a response this \
             protocol can read: expected value"
        );
    }
}
