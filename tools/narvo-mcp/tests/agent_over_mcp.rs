//! An agent drives a real `narvo-mcp` process over real pipes and a real socket.
//!
//! Everything in `src/` is text in and text out and is tested there (M6.5b's S2).
//! What this file adds is the half that cannot be: a child process, an operating
//! system's pipe buffers, a TCP connection opened by somebody else, and the
//! shutdown that ends all of it.
//!
//! # The engine at the far end is a listener this test owns, and that is a named
//! limit
//!
//! **It is not an `narvo` run**, and the reason is a measurement rather than a
//! preference. `--ipc` exists only in a build that asked for the `ipc` feature
//! (D20), and no step of the verification set builds *that* `narvo` together
//! with this crate's tests: step 2 is `cargo nextest run --workspace`, which
//! builds `narvo` with its default features and no socket at all, and step 10 is
//! `cargo nextest run -p narvo-app --features ipc`, which does not build this
//! package. Reaching for `target/debug/narvo` from here would therefore be a
//! test that is green about whichever of the two binaries happened to be built
//! last — the shape CLAUDE.md records for `cargo deny` being green about a tree
//! it could not see.
//!
//! So the split is deliberate and each half claims only its own property:
//!
//! * **this file** — that a real `narvo-mcp` process speaks MCP over stdio,
//!   reaches an engine over TCP, and reports what happens to that connection;
//! * **`narvo-app`'s `agent_socket.rs`** — that a real `narvo` answers, refuses
//!   and steers as it is contracted to, including the replay refusals ADR-0032
//!   states.
//!
//! Neither is evidence for the other's claim, and the seam between them is
//! `narvo_ipc::Response`, which both sides construct with the same types.
//!
//! # Nothing here waits without a bound
//!
//! **The v0.89 rule, applied rather than inherited.** `PATIENCE`, `Serving`'s
//! `Drop` and the bounded waiting in `agent_socket.rs` live in *that* file, and a
//! new test file gets none of them. Every wait below therefore carries its own
//! deadline: the accept loop polls a non-blocking listener, the socket reads have
//! `set_read_timeout`, and the child's `stdout` is drained by a thread into a
//! channel read with `recv_timeout` — because `std` has no timed read on a pipe,
//! which is the same reason `agent_socket.rs` has a reader thread.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long anything here waits before deciding it is never going to happen.
///
/// **A brake, not a synchronisation.** Every wait below is on a loopback round
/// trip or a process exiting, all of which take microseconds, so this is not
/// tuned to any of them and must never be. **If a test ever fails on this bound
/// the answer is not a bigger number** — `ProjektPlan.md` §9.2's rule against
/// tuning a timeout until a flake disappears applies exactly here.
const PATIENCE: Duration = Duration::from_secs(20);

/// The metadata every MCP request carries, as a fragment to paste into one.
const META: &str = r#""_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}"#;

/// A listener standing in for a running engine.
///
/// It speaks the engine's own protocol — `narvo_ipc::Response` rendered by the
/// same `to_json` a real run uses — and it reads requests with `std`'s
/// `read_line` rather than with `narvo_ipc::Lines`. That second half is
/// deliberate and is M6.5a's S5 reasoning: a fake that used the framing under
/// test would make this an agreement between one implementation and itself.
struct FakeEngine {
    listener: TcpListener,
    port: u16,
}

impl FakeEngine {
    /// A listener on a port the operating system picked.
    ///
    /// Port 0 rather than a number written here, for the reason
    /// `agent_socket.rs` records: a fixed port is the classic way a test like
    /// this becomes flaky, through a collision or a socket still in `TIME_WAIT`.
    fn bound() -> Self {
        Self::taking(0)
    }

    /// A listener on `port`, or on one the operating system picked when it is 0.
    ///
    /// The explicit form is for one case only — an engine that went away and
    /// came back at the same address — and it is the one place a test here can
    /// lose a race to another process on the machine, since the port is no
    /// longer being handed out fresh. Bounded by `PATIENCE` like everything
    /// else, and reported as a failure to bind rather than as a mystery.
    fn taking(port: u16) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .unwrap_or_else(|cause| panic!("could not listen on 127.0.0.1:{port}: {cause}"));
        let port = listener.local_addr().expect("it just bound").port();
        listener
            .set_nonblocking(true)
            .expect("a mode change on a socket we own");

        Self { listener, port }
    }

    /// Waits for the server to connect, within the brake.
    ///
    /// Polling a non-blocking listener rather than blocking on `accept`, because
    /// `std` offers no accept deadline and an unbounded one here would be a hang
    /// rather than a red test — which is the whole of what this file's own
    /// doctrine is about.
    fn accepted(&self) -> Talking {
        let deadline = Instant::now() + PATIENCE;

        while Instant::now() < deadline {
            match self.listener.accept() {
                Ok((stream, _peer)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("a mode change on a socket we own");
                    stream
                        .set_read_timeout(Some(PATIENCE))
                        .expect("a read bound on a socket we own");
                    return Talking {
                        reader: BufReader::new(stream),
                    };
                }
                Err(_) => std::thread::yield_now(),
            }
        }

        panic!("the server never connected to the engine, within {PATIENCE:?}");
    }
}

/// One accepted connection, from the engine's side.
struct Talking {
    reader: BufReader<TcpStream>,
}

impl Talking {
    /// The next request the server sent, as it was written on the wire.
    fn heard(&mut self) -> String {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .expect("a request arrives inside the brake");
        assert!(read > 0, "the server closed without asking anything");

        line.trim_end().to_owned()
    }

    /// Answers with one response.
    fn say(&mut self, response: &narvo_ipc::Response) {
        let line = format!("{}\n", response.to_json());
        self.reader
            .get_mut()
            .write_all(line.as_bytes())
            .expect("write to a socket we own");
        self.reader.get_mut().flush().expect("flush");
    }
}

/// The server process, and everything a test needs to talk to it and end it.
struct Serving {
    child: Child,
    /// `Option` so that closing it is something a test can do on purpose — which
    /// is the graceful shutdown the stdio binding names.
    stdin: Option<ChildStdin>,
    /// Lines the server wrote to `stdout`, in order.
    stdout: mpsc::Receiver<String>,
    /// Lines it wrote to `stderr`.
    ///
    /// Drained by a thread rather than left in the pipe: a full pipe buffer
    /// blocks the writer, and a server blocked on a diagnostic would look exactly
    /// like one that had stopped answering.
    stderr: mpsc::Receiver<String>,
}

/// Starts `narvo-mcp` pointed at `engine`.
fn serving(engine: &FakeEngine) -> Serving {
    let mut child = Command::new(env!("CARGO_BIN_EXE_narvo-mcp"))
        .args(["--engine", &format!("127.0.0.1:{}", engine.port)])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary this test was built beside starts");

    let stdin = child.stdin.take().expect("stdin was piped");
    let stdout = drained(child.stdout.take().expect("stdout was piped"));
    let stderr = drained(child.stderr.take().expect("stderr was piped"));

    Serving {
        child,
        stdin: Some(stdin),
        stdout,
        stderr,
    }
}

/// Reads a pipe on its own thread, one line per message.
///
/// The thread is a *test* thread; nothing in this crate grows one. It exists
/// because `std` has no timed read on a pipe, so the alternative to a thread is
/// a blocking read with no bound — the hang this file exists to have removed.
fn drained(pipe: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    receiver
}

/// **Ends the server when the test that started it goes away.**
///
/// Two mechanisms, in order, and the first is the one that matters.
///
/// 1. **Closing `stdin`.** "Servers **SHOULD** exit promptly when their standard
///    input is closed or reads return end-of-file. This is the primary
///    graceful-shutdown signal and the only portable one." This server honours it
///    (`the_server_exits_when_its_stdin_is_closed`), so a dropped `Serving` ends
///    a process rather than abandoning one.
/// 2. **`kill`**, as a backstop, and reaped under the same bound as everything
///    else — a `wait` that never returned would be a fresh version of the hang
///    the brake exists to have removed, sited in the one place that runs during
///    an unwind and could therefore swallow the failure that caused it.
///
/// **Where this stops, stated rather than implied.** `Drop` runs on an ordinary
/// return and during a panic unwind. It does **not** run when the test process is
/// killed from outside — which since v0.96 includes nextest's own
/// `terminate-after`. That case is covered by mechanism 1 without this code
/// running at all: when the test process dies, the operating system closes its
/// end of the pipe, the child's next read returns zero, and it exits. **That is
/// why the shutdown signal is a property worth its own test** — it is the only
/// one of the two that survives the test harness being the thing that dies.
impl Drop for Serving {
    fn drop(&mut self) {
        drop(self.stdin.take());

        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::yield_now(),
            }
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Serving {
    /// Writes `bytes` to the server's `stdin` exactly as given.
    ///
    /// Bytes rather than a line, so a test can put two messages in **one** write
    /// and find out whether the framing on the other side is the one that holds.
    fn write(&mut self, bytes: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is still open");
        stdin
            .write_all(bytes.as_bytes())
            .expect("write to a pipe we own");
        stdin.flush().expect("flush");
    }

    /// One message, framed, into `stdin`.
    fn send(&mut self, message: &str) {
        self.write(&format!("{message}\n"));
    }

    /// The next line the server wrote to `stdout`, within the brake.
    fn answer(&mut self) -> serde_json::Value {
        let line = self
            .stdout
            .recv_timeout(PATIENCE)
            .expect("an answer arrives inside the brake");

        serde_json::from_str(&line).unwrap_or_else(|cause| {
            panic!("the server wrote something that is not JSON to stdout: {line:?} ({cause})")
        })
    }

    /// Whether anything at all is waiting on `stdout` after `grace`.
    ///
    /// The one place a *short* wait is correct: the property under test is an
    /// absence, and an absence cannot be waited for. `grace` is generous for what
    /// it is measuring — a message that was going to be written was written
    /// before the one that came after it — and its failure mode is the safe one:
    /// too short reports silence that is not there, which would be a **red** test
    /// on the assertion that follows, never a green one.
    fn quiet_for(&mut self, grace: Duration) -> Option<String> {
        self.stdout.recv_timeout(grace).ok()
    }

    /// Closes `stdin`, then waits for the server to end.
    ///
    /// Returns whether it ended on its own and successfully. A test that gets
    /// `false` has found a server that ignores the one portable shutdown signal
    /// there is.
    fn ended_on_its_own_after_stdin_closed(&mut self) -> bool {
        drop(self.stdin.take());

        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            match self.child.try_wait().expect("asking after our own child") {
                Some(status) => return status.success(),
                None => std::thread::yield_now(),
            }
        }

        false
    }

    /// Everything the server said on `stderr` so far, within the brake.
    fn said(&mut self, lines: usize) -> Vec<String> {
        (0..lines)
            .map(|_| {
                self.stderr
                    .recv_timeout(PATIENCE)
                    .expect("a diagnostic arrives inside the brake")
            })
            .collect()
    }
}

/// A `tools/call` message for `tool`, with `arguments` written out.
fn call(id: u32, tool: &str, arguments: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{{META},"name":"{tool}","arguments":{arguments}}}}}"#
    )
}

// ---- what the server can say with no engine at all ----------------------

/// **A host can find out what this server is before the engine exists.**
///
/// The reason the connection is lazy: `server/discover` and `tools/list` are
/// answered out of constants, so a host that launches this process gets a usable
/// description whether or not an `narvo` is running. Nothing accepts a
/// connection in this test and nothing needs to.
#[test]
fn a_host_discovers_the_server_and_lists_its_tools_with_no_engine_connected() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    assert_eq!(
        server.said(2),
        vec![
            "narvo-mcp: serving MCP 2026-07-28 on stdio".to_owned(),
            format!("narvo-mcp: engine at 127.0.0.1:{}", engine.port),
        ]
    );

    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{{{META}}}}}"#
    ));
    let discovered = server.answer();
    assert_eq!(discovered["id"], 1);
    assert_eq!(
        discovered["result"]["supportedVersions"],
        serde_json::json!(["2026-07-28"])
    );
    assert_eq!(
        discovered["result"]["capabilities"],
        serde_json::json!({"tools": {}})
    );

    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{{META}}}}}"#
    ));
    let listed = server.answer();
    assert_eq!(listed["id"], 2);
    assert_eq!(
        listed["result"]["tools"]
            .as_array()
            .expect("tools are an array")
            .len(),
        8
    );
}

// ---- the whole path, through a process and a socket ---------------------

/// **An agent calls a tool, the engine sees the request, and the answer comes
/// back.**
///
/// The end-to-end claim of M6.5b in one test: a real process, real pipes, a real
/// socket, and no in-process double anywhere on the path between them. The
/// assertion is on what the *engine* received as well as on what the agent got,
/// because a dispatch that sent another command's request would still produce a
/// well-formed answer — M6.5b's red edge (a), here at the layer that has a wire
/// to look at.
#[test]
fn an_agent_calls_a_tool_the_engine_sees_the_request_and_the_answer_returns() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    server.send(&call(
        7,
        "get_component",
        r#"{"entity":"3v1","component":"layer"}"#,
    ));

    let mut talking = engine.accepted();
    assert_eq!(
        talking.heard(),
        r#"{"get_component":{"entity":"3v1","component":"layer"}}"#,
        "the tool call reached the engine as a different request"
    );

    talking.say(&narvo_ipc::Response::GetComponent {
        entity: "3v1".parse().expect("a well-formed name"),
        component: "layer".to_owned(),
        value: Some("(depth:0.5)".to_owned()),
        ticks_run: 7,
    });

    let answered = server.answer();
    assert_eq!(answered["id"], 7);
    assert_eq!(answered["result"]["isError"], false);
    assert_eq!(
        answered["result"]["content"][0]["text"],
        r#"{"get_component":{"entity":"3v1","component":"layer","value":"(depth:0.5)","ticks_run":7}}"#
    );
}

/// **Two requests in one write are two answers**, across a real pipe.
///
/// M6.5b's red edge (b). A pipe delivers bytes, not messages: this write is one
/// `write_all` of two lines, and an implementation that treated one read as one
/// message would answer the first and lose the second. It is `narvo-ipc`'s
/// framing doing the work (ADR-0033), in its second consumer and at the boundary
/// where the operating system decides how the bytes arrive.
#[test]
fn two_requests_in_one_write_are_two_answers() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    server.write(&format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\",\"params\":{{{META}}}}}\n\
         {{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"server/discover\",\"params\":{{{META}}}}}\n"
    ));

    assert_eq!(server.answer()["id"], 1);
    assert_eq!(
        server.answer()["id"],
        2,
        "the second request in one write was lost"
    );
}

/// **A message split across two writes is still one message.**
///
/// The other half of the framing property, and the one a `read`-per-message
/// implementation gets wrong in the opposite direction: half a request must not
/// be answered, and the rest of it must complete the same one.
#[test]
fn a_request_split_across_two_writes_is_still_one_request() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    let whole = format!(r#"{{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{{{META}}}}}"#);
    let (head, tail) = whole.split_at(24);

    server.write(head);
    assert_eq!(
        server.quiet_for(Duration::from_millis(250)),
        None,
        "half a request was answered"
    );

    server.write(&format!("{tail}\n"));
    assert_eq!(server.answer()["id"], 3);
}

// ---- what the agent is told when things go wrong ------------------------

/// **An engine that goes away mid-call is reported to the agent as such.**
///
/// M6.5b's red edge (c). The engine reads the request and then vanishes, which is
/// how a run really ends — M6.5a measured that a peer closing with bytes it never
/// read resets the connection rather than ending it, and that both endings reach a
/// client as "nothing more can ever arrive".
///
/// What the agent gets is a **protocol** error rather than a tool result, and
/// that is the taxonomy decision of S4 seen from the outside: no adjustment to
/// the arguments makes an absent engine answer, so there is nothing here for a
/// model to self-correct from. The message is `ClientError`'s own.
#[test]
fn an_engine_that_goes_away_mid_call_reaches_the_agent_as_a_protocol_error() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    server.send(&call(1, "list_entities", "{}"));

    let mut talking = engine.accepted();
    assert_eq!(talking.heard(), "\"list_entities\"");

    // The engine dies with the request read and unanswered, and takes its
    // listener with it so that a redial cannot succeed either.
    drop(talking);
    drop(engine);

    let answered = server.answer();
    assert_eq!(answered["id"], 1);
    assert_eq!(answered["error"]["code"], -32603);
    assert_eq!(
        answered["error"]["message"],
        "the engine could not answer: the engine closed the connection before answering"
    );
    assert!(answered.get("result").is_none(), "{answered}");
}

/// **An engine that comes back is talked to again**, rather than the server
/// holding a broken socket for ever.
///
/// The second stage of red edge (c). The first is what an agent is *told* when
/// the engine goes; this is whether the server is still usable afterwards, and
/// nothing about the shape of the first answer says so. MCP is stateless, its
/// stdio binding expects a server to outlive individual failures, and a host
/// restarts an engine far more readily than it restarts this process.
///
/// The second engine takes the same port deliberately: the client is given the
/// address once, on the command line, so a redial that went anywhere else would
/// be a redial to nothing.
#[test]
fn an_engine_that_comes_back_is_talked_to_again() {
    let engine = FakeEngine::bound();
    let port = engine.port;
    let mut server = serving(&engine);

    server.send(&call(1, "list_entities", "{}"));
    let mut talking = engine.accepted();
    assert_eq!(talking.heard(), "\"list_entities\"");
    drop(talking);
    drop(engine);

    assert_eq!(server.answer()["error"]["code"], -32603);

    // A new engine on the same address, which is the only one this server knows.
    let restarted = FakeEngine::taking(port);
    server.send(&call(2, "list_entities", "{}"));

    let mut talking = restarted.accepted();
    assert_eq!(
        talking.heard(),
        "\"list_entities\"",
        "the server never redialled after the engine went away"
    );
    talking.say(&narvo_ipc::Response::ListEntities {
        entities: Vec::new(),
        ticks_run: 7,
    });

    let answered = server.answer();
    assert_eq!(answered["id"], 2);
    assert_eq!(answered["result"]["isError"], false);
}

/// **A command a replay refuses reaches the agent as a tool error, not as a
/// transport failure.**
///
/// M6.5b's red edge (d), and the question it asks is this crate's: ADR-0032 has
/// the engine refusing four commands during a replay, and the refusal arrives
/// here as an ordinary `Response::Error`. Whether an agent can act on it depends
/// entirely on which of MCP's two mechanisms carries it — `isError: true` with
/// the engine's sentence, which a model is meant to read and retry from, or a
/// JSON-RPC error, which it is not.
///
/// **What the engine's own sentence is, is not this file's claim.** That is
/// `narvo-app`'s, in `a_write_over_the_wire_during_a_replay_is_refused_and_the_replay_is_intact`,
/// against a real run on a real command line. This one holds the mapping: whatever
/// the engine refuses with arrives at the agent intact and marked.
#[test]
fn a_command_a_replay_refuses_reaches_the_agent_as_a_tool_error() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    server.send(&call(2, "step", r#"{"ticks":5}"#));

    let mut talking = engine.accepted();
    assert_eq!(talking.heard(), r#"{"step":{"ticks":5}}"#);

    let refusal = "step is refused during a replay: a replay reproduces the run its recording \
                   describes, and a replay's length is its recording's. A replay answers \
                   questions and takes no orders — let it finish, or start a live run to steer";
    talking.say(&narvo_ipc::Response::Error {
        message: refusal.to_owned(),
    });

    let answered = server.answer();
    assert_eq!(answered["id"], 2);
    assert!(
        answered.get("error").is_none(),
        "a refusal was reported as a transport failure: {answered}"
    );
    assert_eq!(answered["result"]["isError"], true);
    assert_eq!(answered["result"]["content"][0]["text"], refusal);
}

// ---- what reaches stdout, and how it all ends ---------------------------

/// **Nothing but MCP messages reaches `stdout`.**
///
/// The stdio binding's one unconditional prohibition: "The server **MUST NOT**
/// write anything to its `stdout` that is not a valid MCP message." So this feeds
/// it the three inputs that most plausibly produce a stray line — a notification,
/// which is owed silence; a blank line; and text that is not JSON — and reads
/// everything that came back.
///
/// The last request is what makes this an assertion rather than a hope: it is
/// answered, so its answer arriving proves the three before it produced exactly
/// what is asserted and not merely that nothing had arrived yet.
#[test]
fn nothing_but_mcp_messages_reaches_stdout() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    server.write(&format!(
        "{{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{{\"requestId\":9}}}}\n\
         \n\
         this is not json\n\
         {{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"tools/list\",\"params\":{{{META}}}}}\n"
    ));

    // Only the unparseable line is owed an answer of the three, and it gets a
    // parse error with no `id` — the field is left out rather than sent as null.
    let complaint = server.answer();
    assert_eq!(complaint["error"]["code"], -32700);
    assert!(complaint.get("id").is_none(), "{complaint}");

    let listed = server.answer();
    assert_eq!(listed["id"], 99);
    assert!(listed["result"]["tools"].is_array(), "{listed}");
}

/// **The server exits when its stdin is closed**, which is how it can never be
/// orphaned.
///
/// The stdio binding's graceful shutdown, and the only portable one there is. It
/// matters here beyond conformance: this is the mechanism that ends the child
/// when the *test process* dies rather than returns — a case `Drop` cannot reach,
/// because a test killed by nextest's `terminate-after` does not unwind. The
/// operating system closes the pipe either way, and this is the property that
/// says the server acts on it.
#[test]
fn the_server_exits_when_its_stdin_is_closed() {
    let engine = FakeEngine::bound();
    let mut server = serving(&engine);

    // Something normal first, so this is a *running* server being shut down
    // rather than one that never started.
    server.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{{{META}}}}}"#
    ));
    assert_eq!(server.answer()["id"], 1);

    assert!(
        server.ended_on_its_own_after_stdin_closed(),
        "the server did not exit when its stdin reached end of file"
    );
}

/// **A usage mistake is a failure with a message, not a server.**
#[test]
fn starting_without_an_engine_is_refused_with_the_usage() {
    let complaint = Command::new(env!("CARGO_BIN_EXE_narvo-mcp"))
        .output()
        .expect("the binary this test was built beside starts");

    assert_eq!(complaint.status.code(), Some(2));
    assert!(
        complaint.stdout.is_empty(),
        "a usage mistake wrote to stdout: {:?}",
        String::from_utf8_lossy(&complaint.stdout)
    );

    let said = String::from_utf8_lossy(&complaint.stderr);
    assert!(said.contains("no engine to serve."), "{said}");
    assert!(said.contains("narvo-mcp --engine <host:port>"), "{said}");
}
