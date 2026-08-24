//! `narvo-mcp` — an MCP server that puts a running Narvo engine in front of an
//! agent.
//!
//! ```text
//! narvo-mcp --engine 127.0.0.1:7777
//! ```
//!
//! It speaks **MCP 2026-07-28** over stdio to its host, and this workspace's own
//! agent protocol over a TCP connection to one `narvo` process that was started
//! with `--ipc`. Seven tools, one per protocol command; see `tools.rs` for what
//! each says about itself and `server.rs` for the protocol.
//!
//! # This file is the shell, and it is the whole of the shell
//!
//! Everything that can be wrong in an interesting way is in `server.rs`,
//! `tools.rs` and `jsonrpc.rs`, which are text in and text out (M6.5b's S2). This
//! file is the only one in the crate that names `std::io`, `std::env`,
//! `std::process` or a socket, and it does three things: read the arguments, open
//! a connection, and move bytes between `stdin`, [`server::pump`] and `stdout`.
//!
//! # What goes where, which the stdio binding is strict about
//!
//! **`stdout` carries MCP messages and nothing else.** "The server **MUST NOT**
//! write anything to its `stdout` that is not a valid MCP message." So every
//! diagnostic this program has goes to `stderr`, which the same binding leaves
//! free: "The server **MAY** write UTF-8 strings to `stderr` for any logging
//! purposes".
//!
//! # How it ends
//!
//! **On end-of-file from `stdin`, and that is the point rather than a detail.**
//! "Servers **SHOULD** exit promptly when their standard input is closed or reads
//! return end-of-file. This is the primary graceful-shutdown signal and the only
//! portable one." It is also what makes this process impossible to orphan: when
//! whatever launched it goes away, the operating system closes its end of the
//! pipe, this read returns zero, and the loop stops. No signal handler and no
//! supervisor is involved, which is why `tests/agent_over_mcp.rs` can rely on it
//! even for a test that is killed rather than failed.
//!
//! # No argument-parsing dependency
//!
//! One option and one flag, so the parsing is thirty lines — the same reading
//! `narvo-cli` arrived at in M4.2 with `clap` measured at 21 crates and 3.88 s.
//! S1 of M6.5b measured the same question one layer up and reached the same
//! answer for the protocol itself; ADR-0034 records it.

mod jsonrpc;
mod server;
mod tools;

use std::io::{ErrorKind, Read as _, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use narvo_ipc::{Client, ClientError, Lines, Request, Response};

use crate::server::{Engine, Server, pump};

/// What `--help` prints, and what a usage mistake prints after its complaint.
const USAGE: &str = "\
narvo-mcp — an MCP server for a running Narvo engine

USAGE:
    narvo-mcp --engine <host:port>

OPTIONS:
    --engine <host:port>    The address an `narvo --ipc <host:port>` run is
                            listening on. Required.
    -h, --help              Print this message.

The MCP conversation is on stdin and stdout, one JSON-RPC message per line.
Diagnostics go to stderr. The server exits when stdin reaches end of file.

EXIT CODES:
    0    stdin reached end of file and the server stopped
    2    bad arguments, or stdin could not be read
";

/// How long a single exchange with the engine may take before it is reported as
/// silence.
///
/// **A brake, not a synchronisation**, and the same twenty seconds
/// `agent_socket.rs`, `transport.rs` and `narvo-ipc`'s own client tests use.
/// Every exchange here is a loopback round trip answered at a tick boundary, so
/// this is not tuned to any of them. **If it ever fires against a healthy engine
/// the answer is not a bigger number** — `ProjektPlan.md` §9.2's rule against
/// tuning a timeout until a flake disappears applies exactly here.
///
/// It is deliberately **not** an option. A knob for it would be a knob for
/// exactly the mistake that rule forbids, and a run that genuinely takes longer
/// than twenty seconds to reach a tick boundary is a finding rather than a
/// configuration.
const PATIENCE: Duration = Duration::from_secs(20);

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    let address = match parse(&arguments) {
        Ok(Parsed::Serve { address }) => address,
        Ok(Parsed::Usage) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(complaint) => {
            eprintln!("narvo-mcp: {complaint}");
            eprint!("\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    eprintln!("narvo-mcp: serving MCP {} on stdio", server::VERSION);
    eprintln!("narvo-mcp: engine at {address}");

    let mut server = Server::new(Dialled::at(address));
    match serve(&mut server) {
        Ok(()) => ExitCode::SUCCESS,
        Err(cause) => {
            eprintln!("narvo-mcp: {cause}");
            ExitCode::from(2)
        }
    }
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    /// Serve an engine at this address.
    Serve {
        /// `host:port`, exactly as it was written.
        address: String,
    },
    /// Print the usage and stop.
    Usage,
}

/// Reads the command line, or says what is wrong with it.
///
/// # Errors
///
/// A sentence naming what was missing, unknown or doubled.
fn parse(arguments: &[String]) -> Result<Parsed, String> {
    let mut address: Option<String> = None;
    let mut arguments = arguments.iter().map(String::as_str);

    while let Some(argument) = arguments.next() {
        match argument {
            "-h" | "--help" | "help" => return Ok(Parsed::Usage),
            "--engine" if address.is_some() => {
                return Err("`--engine` was given twice".to_owned());
            }
            "--engine" => {
                let value = arguments
                    .next()
                    .ok_or("`--engine` needs an address, such as `127.0.0.1:7777`")?;
                address = Some(value.to_owned());
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }

    match address {
        Some(address) => Ok(Parsed::Serve { address }),
        None => Err(
            "no engine to serve. Start one with `narvo --headless --ticks 0 --ipc \
             127.0.0.1:7777`, then pass the same address to `--engine`"
                .to_owned(),
        ),
    }
}

/// Moves bytes between the host and the protocol until `stdin` ends.
///
/// # Errors
///
/// Whatever reading `stdin` or writing `stdout` raised. A write failure is fatal
/// rather than skipped: a host that cannot receive an answer is not one to keep
/// answering.
fn serve<E: Engine>(server: &mut Server<E>) -> std::io::Result<()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut lines = Lines::new();
    let mut buffer = [0_u8; 4096];

    loop {
        match input.read(&mut buffer) {
            // End of file. The graceful shutdown the stdio binding names, and the
            // only portable one.
            Ok(0) => return Ok(()),
            Ok(read) => {
                let out = pump(&mut lines, server, &buffer[..read]);
                if !out.is_empty() {
                    output.write_all(&out)?;
                    // **Flushed on every answer, not on a schedule.** The host is
                    // waiting for this line before it sends the next request, so a
                    // buffered answer is a deadlock rather than a delay.
                    output.flush()?;
                }
            }
            Err(cause) if cause.kind() == ErrorKind::Interrupted => {}
            Err(cause) => return Err(cause),
        }
    }
}

/// The engine, connected to when there is first something to ask it.
///
/// # Why the connection is lazy
///
/// A host launches this process and then asks it what it can do. `server/discover`
/// and `tools/list` are answered out of constants and need no engine at all, so
/// connecting at startup would make a server that cannot describe itself unless
/// the engine happens to be up — and would turn "the engine is not running" into
/// a process that exits before its host can read the reason.
///
/// # Why it redials
///
/// The client is dropped whenever a conversation fails, so the next call opens a
/// fresh connection. MCP is stateless and its stdio binding expects a server to
/// outlive individual failures; without this, one engine restart would leave this
/// process holding a broken socket and answering every later call with the same
/// stale complaint.
struct Dialled {
    address: String,
    client: Option<Client>,
}

impl Dialled {
    /// A server for the engine at `address`, not yet connected to it.
    fn at(address: String) -> Self {
        Self {
            address,
            client: None,
        }
    }
}

impl Engine for Dialled {
    fn ask(&mut self, request: &Request) -> Result<Response, ClientError> {
        if self.client.is_none() {
            self.client = Some(Client::connect(&self.address, PATIENCE)?);
        }

        let client = self.client.as_mut().expect("just connected");
        let answered = client.ask(request);

        if answered.is_err() {
            self.client = None;
        }
        answered
    }
}

#[cfg(test)]
mod tests {
    use super::{Parsed, parse};

    fn arguments(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn an_engine_address_is_what_this_needs() {
        assert_eq!(
            parse(&arguments(&["--engine", "127.0.0.1:7777"])).expect("well formed"),
            Parsed::Serve {
                address: "127.0.0.1:7777".to_owned()
            }
        );
    }

    #[test]
    fn help_is_asked_for_in_the_three_usual_ways() {
        for spelling in ["-h", "--help", "help"] {
            assert_eq!(
                parse(&arguments(&[spelling])).expect("well formed"),
                Parsed::Usage
            );
        }
    }

    /// **No engine is not a default**, and the message says how to start one.
    #[test]
    fn no_engine_says_how_to_start_one() {
        assert_eq!(
            parse(&[]).expect_err("there is nothing to serve"),
            "no engine to serve. Start one with `narvo --headless --ticks 0 --ipc \
             127.0.0.1:7777`, then pass the same address to `--engine`"
        );
    }

    #[test]
    fn an_engine_without_an_address_says_what_one_looks_like() {
        assert_eq!(
            parse(&arguments(&["--engine"])).expect_err("no address"),
            "`--engine` needs an address, such as `127.0.0.1:7777`"
        );
    }

    /// Two engines would be a choice this program cannot make for the caller.
    #[test]
    fn two_engines_are_refused_rather_than_one_of_them_winning() {
        assert_eq!(
            parse(&arguments(&["--engine", "a:1", "--engine", "b:2"])).expect_err("two"),
            "`--engine` was given twice"
        );
    }

    #[test]
    fn an_unknown_argument_is_named() {
        assert_eq!(
            parse(&arguments(&["--ipc", "a:1"])).expect_err("no such argument"),
            "unknown argument `--ipc`"
        );
    }

    /// The usage text names every option this program has.
    ///
    /// A cheap gate against the drift that makes a `--help` a lie: an option
    /// added to [`parse`] and not to [`super::USAGE`] fails here.
    #[test]
    fn the_usage_names_every_option_there_is() {
        for option in ["--engine", "-h", "--help"] {
            assert!(
                super::USAGE.contains(option),
                "{option} is accepted and undocumented"
            );
        }
    }
}
