//! The socket, and nothing else: bytes in, [`Request`] out, [`Response`] in,
//! bytes out.
//!
//! **This module knows no `World`, no registry, no tick and no band.** That is
//! not tidiness — D20's decision is localhost TCP *on the condition that it stays
//! cheaply reversible*, and reversibility is exactly this: the thing above it is
//! [`crate::ipc::Channel`], four methods wide, and swapping the transport is
//! writing a second implementation of them. Nothing in `headless` or `ipc` names
//! `std::net`.
//!
//! # What it is, measured rather than assumed
//!
//! D20 chose localhost TCP because it is the only transport `std` offers on both
//! platforms on the pinned toolchain: `std::os::windows::net::UnixStream` exists
//! and is behind the unstable feature `windows_unix_domain_sockets`
//! (rust-lang/rust#150487), which ADR-0009's reasoning excludes. The D20 survey
//! measured the consequence rather than asserting it, and it is the reason for
//! the gate: **a loopback listener has no access check.** A second, unrelated
//! process connects with no prompt and no credential on both platforms, and under
//! WSL a *different user* connected to a root-owned listener with nothing in the
//! way. An AF_UNIX socket at mode 0755 refused that same user with `EACCES`.
//!
//! So the transport is behind a feature that is **off by default**, and a release
//! binary does not contain it.
//!
//! # Framing
//!
//! One JSON message per line, in both directions — and **the code that does the
//! splitting is not here.** It lives in `narvo_ipc::framing`, as
//! [`Lines`](narvo_ipc::Lines) and [`framed`](narvo_ipc::framed), because
//! M6.5a gave this connection a second end: a client cannot depend on the
//! application crate, and a second implementation of the framing would be two
//! framings at the two ends of one connection. ADR-0033 records the move and
//! prices what was rejected.
//!
//! What that leaves here is the socket and nothing else: this module decides
//! *when* to read and what a read means, and `Lines` decides where a message
//! stops.
//!
//! # What is not watched here, and cannot be
//!
//! - **Which tick a message lands on.** It is decided by the race between a
//!   client and the tick loop. The wait removes the race only while the run is
//!   waiting; during ticks it stands.
//! - **The order two clients' messages arrive in.** Only one client is served at
//!   a time (see [`Endpoint::arrived`]), so the question is deferred rather than
//!   answered.
//! - **Platform differences in socket behaviour.** CI running green on both is
//!   evidence about those two runners, not a test.
//! - **That a client which vanished mid-request left nothing behind.** The
//!   observable is an absence, and a slow client produces the same one.

use std::io::{ErrorKind, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};

use narvo_ipc::{Lines, Request, Response, framed};

use crate::ipc::Channel;

/// A listener on loopback, and the one client it is talking to.
///
/// **One client at a time, deliberately.** A second connection is left in the
/// accept queue until the first goes away. Serving several would need an order
/// between their messages, and that order is the operating system's — the first
/// thing on this module's own list of what cannot be watched. One client is what
/// an agent session is; more is a question M6.4 can ask when something needs it.
#[derive(Debug)]
pub struct Endpoint {
    listener: TcpListener,
    client: Option<Client>,
    /// Whether a client has ever been accepted here.
    ///
    /// It is what makes `--ipc` mean *serve an agent* rather than *offer a
    /// socket to whoever happens to be quick*: a run that is serving waits for
    /// its first client, and stops waiting for good once one has been and gone.
    /// Without it every end-to-end test would be a race between a connecting
    /// client and a run that finishes in milliseconds — which is precisely the
    /// finding that halted the first M6.3c.
    served: bool,
}

/// What one attempt to read found — and, separately, whether there will be more.
///
/// **The two are different events and were once the same value.** Until M6.3d's
/// fifth session `read_lines` returned only the messages, and the wait treated an
/// empty list as *the client has gone*: "nothing has arrived yet" and "the
/// connection is closed" were spelled identically, so a read that came back
/// empty for any other reason — a `WouldBlock` from a mode switch that had
/// silently failed, whose error was discarded — would have ended a run whose
/// client was still there and still waiting for an answer.
///
/// That defect was found by reading in the fourth session's R1 and is **not** the
/// intermittent failure this milestone has been chasing: its signature is a run
/// that *exits*, which gives a client end-of-file, while the observed failure is
/// a run that is alive and blocked. It is fixed here on its own merits.
#[derive(Debug, Default)]
struct Read {
    /// Complete messages, in arrival order.
    lines: Vec<Vec<u8>>,
    /// Whether the peer has closed, so nothing more can ever arrive.
    ///
    /// **Only this ends a wait.** An empty `lines` with `ended` false means the
    /// read found nothing yet, which is a reason to keep waiting and never a
    /// reason to hang up.
    ended: bool,
}

impl Read {
    /// Nothing arrived and nothing ever will.
    fn over() -> Self {
        Self {
            lines: Vec::new(),
            ended: true,
        }
    }
}

/// A connected client and the bytes of its half-finished line.
#[derive(Debug)]
struct Client {
    stream: TcpStream,
    /// The framing, holding whatever does not yet end a line.
    lines: Lines,
    /// Set when the peer has closed its half of the connection.
    closed: bool,
}

impl Endpoint {
    /// Listens on `address`, which is expected to be a loopback address.
    ///
    /// The listener is non-blocking from the start, so [`arrived`](Self::arrived)
    /// never waits for a connection that may not come — which is what keeps a run
    /// with the transport on and nobody attached identical to a run without it.
    ///
    /// # Errors
    ///
    /// Whatever binding can raise. The caller names the address in the message;
    /// this returns the operating system's own words.
    pub fn bind(address: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;

        Ok(Self {
            listener,
            client: None,
            served: false,
        })
    }

    /// Which address the listener actually took.
    ///
    /// Worth having because port 0 means "any free port", which is how a test
    /// avoids picking a number that another process on the machine already has.
    ///
    /// # Errors
    ///
    /// Whatever asking the socket can raise.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Takes a client if one is waiting, without blocking.
    fn accept_if_waiting(&mut self) {
        if self.client.is_some() {
            return;
        }
        // Nobody waiting is the ordinary answer here, and every other error is
        // treated the same way: there is no client this tick and the next tick
        // asks again. A listener that has genuinely broken therefore looks like
        // a client that never came — one of this module's named limits rather
        // than something it can tell apart.
        if let Ok((stream, _peer)) = self.listener.accept() {
            // Non-blocking while ticks are running, so no tick ever waits for a
            // client to say something. `awaited` switches it back.
            let _ = stream.set_nonblocking(true);
            self.served = true;
            self.client = Some(Client {
                stream,
                lines: Lines::new(),
                closed: false,
            });
        }
    }

    /// Reads whatever is available and returns the complete lines in it.
    ///
    /// `blocking` decides how the read behaves; the parsing is the same either
    /// way, which is what keeps a request that arrived during a tick and one that
    /// arrived during a wait indistinguishable.
    fn read_lines(&mut self, blocking: bool) -> Read {
        let Some(client) = self.client.as_mut() else {
            return Read::over();
        };

        // **Not discarded.** A mode this function could not set is a connection
        // it cannot honour, and saying so is the difference between "the socket
        // is broken" and "the client said goodbye" - which is the distinction
        // the whole of this type exists for.
        if client.stream.set_nonblocking(!blocking).is_err() {
            client.closed = true;
            self.client = None;
            return Read::over();
        }

        let mut lines = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            match client.stream.read(&mut buffer) {
                Ok(0) => {
                    client.closed = true;
                    break;
                }
                Ok(n) => {
                    lines.extend(client.lines.feed(&buffer[..n]));
                    // A blocking read stops as soon as it has something whole;
                    // otherwise it would wait for bytes nobody is going to send.
                    if blocking && !lines.is_empty() {
                        break;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {
                    client.closed = true;
                    break;
                }
            }
        }

        // Back to non-blocking for the tick loop, whatever this call did.
        let _ = client.stream.set_nonblocking(true);

        let ended = client.closed;
        if ended && !client.lines.unfinished() && lines.is_empty() {
            self.client = None;
        }

        Read { lines, ended }
    }

    /// Turns lines into requests, answering the malformed ones itself.
    ///
    /// **A line that is not a request never reaches the world**, so the world has
    /// no say in what it is told. The message is `narvo-ipc`'s own — position
    /// included — which is why this adds no wording of its own: M6.1 already made
    /// those messages the product and pinned them.
    fn requests_from(&mut self, lines: Vec<Vec<u8>>) -> Vec<Request> {
        let mut requests = Vec::new();
        let mut refusals = Vec::new();

        for line in lines {
            if line.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&line);
            match Request::from_json(&text) {
                Ok(request) => requests.push(request),
                Err(error) => refusals.push(Response::Error {
                    message: error.to_string(),
                }),
            }
        }

        if !refusals.is_empty() {
            self.answered(&refusals);
        }

        requests
    }
}

impl Channel for Endpoint {
    /// Accepts a waiting client and returns whatever whole messages have arrived.
    ///
    /// Never blocks: the listener and the stream are both non-blocking here, so a
    /// tick costs one `accept` and one `read` that return immediately when there
    /// is nothing.
    fn arrived(&mut self) -> Vec<Request> {
        self.accept_if_waiting();
        let read = self.read_lines(false);
        self.requests_from(read.lines)
    }

    /// Blocks until a request arrives, or until there is nobody to wait for.
    ///
    /// **A blocking read rather than a poll with a sleep**, and that is the whole
    /// reason there is no timeout constant anywhere in this file: a duration
    /// chosen here would be a number that is right on this machine and wrong on a
    /// slower runner, and the project has a standing rule against exactly that.
    /// The operating system does the waiting.
    ///
    /// The price is stated rather than hidden: while this blocks, no second
    /// client is accepted and no tick runs. That is what waiting *is* — the
    /// simulation is stopped, which is also what makes the wait invisible to
    /// everything a run produces.
    fn awaited(&mut self) -> Option<Request> {
        loop {
            // Waiting for the *first* client blocks on the accept, which is what
            // makes `--ipc --ticks 0` a deterministic "attach and drive" rather
            // than a race a client loses on a fast machine. Once one has been
            // served, a later wait with nobody connected is over: `attached`
            // returns false and the runner never gets here.
            if self.client.is_none() && !self.served {
                let _ = self.listener.set_nonblocking(false);
                let accepted = self.listener.accept();
                let _ = self.listener.set_nonblocking(true);
                match accepted {
                    Ok((stream, _peer)) => {
                        let _ = stream.set_nonblocking(true);
                        self.served = true;
                        self.client = Some(Client {
                            stream,
                            lines: Lines::new(),
                            closed: false,
                        });
                    }
                    Err(_) => return None,
                }
            }

            self.accept_if_waiting();
            self.client.as_ref()?;

            let read = self.read_lines(true);
            if read.ended {
                // The peer closed, or the connection could not be honoured.
                // **Only this ends the wait** - an empty read on a live
                // connection means nothing has arrived yet, which is what
                // waiting is for.
                self.client = None;
                return None;
            }
            if read.lines.is_empty() {
                // Nothing whole yet on a connection that is still open. Wait
                // again rather than treating absence as departure.
                continue;
            }

            let mut requests = self.requests_from(read.lines);
            if !requests.is_empty() {
                // Anything beyond the first goes back on the queue by being
                // returned one at a time; the runner pushes what it gets and
                // comes back here for the rest.
                let first = requests.remove(0);
                if let Some(client) = self.client.as_mut() {
                    // Whatever else came in the same read is still held by the
                    // framing only if it was incomplete; complete extras are re-queued by
                    // pushing their bytes back, which cannot happen here because
                    // `read_lines` stops at the first whole line in blocking mode.
                    let _ = client;
                }
                return Some(first);
            }
            // Everything that arrived was malformed and has been refused. Wait
            // again rather than treating a bad line as the end of the client.
        }
    }

    fn attached(&self) -> bool {
        self.client.is_some() || !self.served
    }

    /// Writes each answer as one line.
    ///
    /// A failed write closes the client rather than failing the run: an agent
    /// that went away mid-answer is a fact about the agent, and a simulation that
    /// stopped because of it would be a worse answer to a smaller problem — the
    /// same reading `Inbox::answer_pending` applies to a request it cannot serve.
    fn answered(&mut self, answers: &[Response]) {
        let Some(client) = self.client.as_mut() else {
            return;
        };

        for answer in answers {
            let line = framed(&answer.to_json());
            if client.stream.write_all(line.as_bytes()).is_err() {
                self.client = None;
                return;
            }
        }

        if client.stream.flush().is_err() {
            self.client = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Endpoint;
    use crate::ipc::Channel as _;
    use narvo_ipc::{Request, Response};
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::TcpStream;

    /// How long a read here waits before deciding the answer is never coming.
    ///
    /// **A brake, not a synchronisation**, exactly as in
    /// `tests/agent_socket.rs`, and the same rule applies: if a test ever fails
    /// on this bound the answer is not a bigger number, it is that something is
    /// genuinely stuck.
    ///
    /// It is here because it was **missing**, and the absence was measured
    /// rather than noticed. M6.3d spent three sessions on a suite that would
    /// stop talking instead of going red, and red edge (a2) — an endpoint that
    /// takes requests and writes no answers — reproduced it: the three
    /// end-to-end tests failed at their own twenty-second bound, while
    /// `a_malformed_line_is_answered_with_the_protocols_own_words` and
    /// `an_answer_crosses_back_as_one_line` blocked in `read_line` **for ever**,
    /// on client sockets that had no deadline of any kind. A ten-minute run with
    /// no output was two unit tests waiting on an answer that had been cancelled
    /// by the injection. The brake was written for the integration harness and
    /// never applied to the unit tests that talk over the same loopback.
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);

    /// An endpoint on a port the operating system picked, and a client on it.
    ///
    /// Port 0 rather than a number: a fixed port is a collision with whatever
    /// else on the machine already has it, which is the classic way a socket test
    /// becomes flaky for reasons that have nothing to do with the code.
    ///
    /// The client is handed back with [`PATIENCE`] on it, so that a test which
    /// reads an answer that never arrives **fails** instead of hanging the suite.
    fn connected() -> (Endpoint, TcpStream) {
        let mut endpoint = Endpoint::bind("127.0.0.1:0").expect("loopback binds");
        let address = endpoint.local_addr().expect("it just bound");
        let client = TcpStream::connect(address).expect("connecting to our own listener");
        client
            .set_read_timeout(Some(PATIENCE))
            .expect("a read bound on a socket we own");

        // One poll to take the connection out of the accept queue.
        assert!(endpoint.arrived().is_empty());
        assert!(endpoint.attached(), "the client was not accepted");

        (endpoint, client)
    }

    #[test]
    fn a_line_of_json_becomes_a_request() {
        let (mut endpoint, mut client) = connected();

        client
            .write_all(b"\"list_entities\"\n")
            .expect("write to our own socket");
        client.flush().expect("flush");

        // The read is non-blocking, so give the loopback a moment to deliver.
        let requests = poll_until(&mut endpoint, 1);
        assert_eq!(requests, vec![Request::ListEntities]);
    }

    /// Polls until `want` requests have arrived, or a generous number of tries
    /// have passed.
    ///
    /// **Not a timing assumption dressed as a helper.** Loopback delivery is not
    /// instantaneous and nothing in this design says it should be; what the tests
    /// below assert is *what* arrives, never *how fast*. The bound exists so a
    /// broken endpoint fails the test instead of hanging it.
    fn poll_until(endpoint: &mut Endpoint, want: usize) -> Vec<Request> {
        let mut collected = Vec::new();
        for _ in 0..2_000 {
            collected.extend(endpoint.arrived());
            if collected.len() >= want {
                break;
            }
            std::thread::yield_now();
        }
        collected
    }

    #[test]
    fn a_partial_line_waits_for_the_rest_of_itself() {
        let (mut endpoint, mut client) = connected();

        client.write_all(b"\"list_ent").expect("write");
        client.flush().expect("flush");
        for _ in 0..200 {
            assert!(
                endpoint.arrived().is_empty(),
                "half a message was read as a whole one"
            );
            std::thread::yield_now();
        }

        client.write_all(b"ities\"\n").expect("write");
        client.flush().expect("flush");
        assert_eq!(poll_until(&mut endpoint, 1), vec![Request::ListEntities]);
    }

    #[test]
    fn two_messages_in_one_write_are_two_requests() {
        let (mut endpoint, mut client) = connected();

        client
            .write_all(b"\"list_entities\"\n\"list_entities\"\n")
            .expect("write");
        client.flush().expect("flush");

        assert_eq!(
            poll_until(&mut endpoint, 2),
            vec![Request::ListEntities, Request::ListEntities]
        );
    }

    /// A line that is not a request is refused **by the transport**, with
    /// `narvo-ipc`'s own message, and never reaches anything else.
    #[test]
    fn a_malformed_line_is_answered_with_the_protocols_own_words() {
        let (mut endpoint, client) = connected();
        let mut writer = client.try_clone().expect("clone for writing");
        let mut reader = BufReader::new(client);

        writer.write_all(b"not json at all\n").expect("write");
        writer.flush().expect("flush");

        for _ in 0..2_000 {
            if !endpoint.arrived().is_empty() {
                panic!("a malformed line became a request");
            }
            // The refusal is written back by `arrived` itself.
            std::thread::yield_now();
        }

        let mut line = String::new();
        reader.read_line(&mut line).expect("an answer came back");
        let answer = Response::from_json(line.trim()).expect("it is a response");
        match answer {
            Response::Error { message } => {
                assert!(
                    message.starts_with("1:1: the text is not a request this protocol can read:"),
                    "{message}"
                );
            }
            other => panic!("expected an error answer, got {other:?}"),
        }
    }

    /// **A fresh endpoint waits for its first client; one that has served does
    /// not wait for a second.**
    ///
    /// This assertion was the other way round when it was first written, and the
    /// end-to-end test is what changed it: with `attached` false until somebody
    /// connected, an `--ipc` run reached its budget and ended before any client
    /// could arrive, so every socket test would have been a race against a run
    /// that finishes in milliseconds — the finding that halted the first M6.3c.
    ///
    /// `--ipc` therefore means *serve an agent*, and a run that is serving waits
    /// for one. The named cost: an `--ipc` run that nobody connects to waits for
    /// ever. That is what asking to serve means, and it is not the case the
    /// project's standing requirement is about — a run **without** the flag has
    /// no listener at all, whatever the feature says, and ends as it always did.
    #[test]
    fn an_endpoint_waits_for_its_first_client_and_not_for_a_second() {
        let mut endpoint = Endpoint::bind("127.0.0.1:0").expect("loopback binds");
        assert!(
            endpoint.attached(),
            "a fresh endpoint would not wait for anybody, so `--ipc` would be a race"
        );

        let address = endpoint.local_addr().expect("it just bound");
        let client = TcpStream::connect(address).expect("connecting to our own listener");
        assert!(endpoint.arrived().is_empty());
        assert!(endpoint.attached());

        drop(client);
        assert_eq!(
            endpoint.awaited(),
            None,
            "a departed client is still waited on"
        );
        assert!(
            !endpoint.attached(),
            "the endpoint would wait again for a client that has been and gone"
        );
    }

    /// A client that goes away ends the wait rather than extending it forever.
    #[test]
    fn a_client_that_disconnects_ends_the_wait() {
        let (mut endpoint, client) = connected();
        drop(client);

        assert_eq!(endpoint.awaited(), None);
        assert!(
            !endpoint.attached(),
            "the endpoint kept a client that had gone"
        );
    }

    /// **Absence and departure are different answers**, which is the whole of
    /// the M6.3d fix.
    ///
    /// A live connection that has sent nothing yields no messages and `ended`
    /// false; a peer that has closed yields no messages and `ended` **true**.
    /// Before the fix both were the same value — an empty list — and the wait
    /// read either of them as "the client has gone", so a run whose client was
    /// still there could be ended by a read that merely came back early.
    ///
    /// Its failure state was measured rather than argued: restoring the old
    /// conflation (`ended: lines.is_empty()`) turns this test red on its first
    /// assertion, because a live silent connection then reports departure.
    #[test]
    fn a_silent_connection_is_not_a_closed_one() {
        let (mut endpoint, client) = connected();

        // Nothing sent. Non-blocking, so this returns at once.
        let quiet = endpoint.read_lines(false);
        assert!(
            quiet.lines.is_empty(),
            "nothing was sent, so nothing arrived"
        );
        assert!(
            !quiet.ended,
            "a live connection that has said nothing was reported as closed, \
             which is the confusion this type exists to prevent"
        );
        assert!(endpoint.attached(), "and the client is still there");

        // Now it goes away. Same empty list, opposite verdict.
        drop(client);
        let gone = loop {
            let read = endpoint.read_lines(false);
            if read.ended {
                break read;
            }
            std::thread::yield_now();
        };
        assert!(gone.lines.is_empty());
        assert!(gone.ended, "a closed peer has to be reported as closed");
    }

    /// A mode that cannot be set is a connection that cannot be honoured, and it
    /// is reported as an ending rather than as an empty read.
    ///
    /// Reached here through the one case a test can construct without breaking a
    /// socket by hand: with no client at all there is nothing to set a mode on,
    /// and the answer is `over` — ended, with nothing in it. The `is_err` branch
    /// above it takes the same exit, which is the point of them sharing one.
    #[test]
    fn a_read_with_no_client_is_over_rather_than_merely_empty() {
        let mut endpoint = Endpoint::bind("127.0.0.1:0").expect("loopback binds");

        let read = endpoint.read_lines(true);
        assert!(read.lines.is_empty());
        assert!(read.ended, "no client at all is an ending, not a pause");
    }

    /// **The two ends of one connection understand each other**, which is the
    /// property the shared framing exists for.
    ///
    /// This is the only test in the workspace where `narvo_ipc::Client` and
    /// [`Endpoint`] meet: the client frames a request with `framed`, the endpoint
    /// unframes it with `Lines`, and the answer goes back the same way in the
    /// other direction. A second implementation of the framing at either end
    /// would still pass every test that checks one end against bytes written by
    /// hand; it would fail here.
    ///
    /// **The thread is bounded like everything else that waits.** `ask` writes
    /// and then blocks, so somebody has to pump the endpoint meanwhile; the
    /// client's wait is bounded by [`PATIENCE`] and the pump's by a deadline of
    /// its own, because the M6.3d rule applies to the second end of a test as
    /// much as to the first.
    #[test]
    fn the_client_and_the_endpoint_understand_each_other() {
        use narvo_ipc::Client;

        let mut endpoint = Endpoint::bind("127.0.0.1:0").expect("loopback binds");
        let address = endpoint.local_addr().expect("it just bound").to_string();
        let mut client = Client::connect(&address, PATIENCE).expect("our own listener");

        let pump = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + PATIENCE;
            while std::time::Instant::now() < deadline {
                if let Some(request) = endpoint.arrived().first() {
                    let answer = match request {
                        Request::Step { ticks } => Response::Step {
                            granted: *ticks,
                            ticks_run: 1,
                        },
                        other => Response::Error {
                            message: format!("unexpected request {other:?}"),
                        },
                    };
                    endpoint.answered(&[answer]);
                    return;
                }
                std::thread::yield_now();
            }
        });

        assert_eq!(
            client.ask(&Request::Step { ticks: 5 }).expect("an answer"),
            Response::Step {
                granted: 5,
                ticks_run: 1,
            }
        );
        pump.join().expect("the pump thread did not panic");
    }

    /// An answer comes back as one line of JSON.
    #[test]
    fn an_answer_crosses_back_as_one_line() {
        let (mut endpoint, client) = connected();
        let mut reader = BufReader::new(client);

        endpoint.answered(&[Response::Step {
            granted: 9,
            ticks_run: 1,
        }]);

        let mut line = String::new();
        reader.read_line(&mut line).expect("an answer came back");
        assert_eq!(line, "{\"step\":{\"granted\":9,\"ticks_run\":1}}\n");
    }
}
