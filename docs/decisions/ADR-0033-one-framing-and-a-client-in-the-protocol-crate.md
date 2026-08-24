# ADR-0033: One framing for both ends, and the client lives in the protocol crate

Status: accepted · Date: 2026-08-13 · Scope: `narvo-ipc`, `narvo-app`'s
`transport` module, and every future consumer of the agent protocol —
`tools/narvo-mcp` first

## Context

M6.3d gave the engine a socket. `narvo-app`'s `transport::Endpoint` owns a
loopback listener, and inside it, inlined, sat the whole of the line framing:
bytes accumulated in a `partial: Vec<u8>`, a scan for `\n`, a `\r` dropped if it
preceded one, and on the way out a `line.push('\n')` after `to_json`.

That was right while there was one end. M6.5b adds the second: an MCP server
under `tools/`, which is a client of this protocol and speaks the same framing
from the other side. It **cannot depend on `narvo-app`** in any way that is
worth doing, and the framing was the only thing it needed from there.

`ProjektPlan.md` §6/M6's v0.94 cut names three resolutions and asks for the
decision before the client is written, which is what this records.

## Decision

**The line framing moves to `narvo-ipc`, as `framing::Lines` and
`framing::framed`, and both ends call it. The client half of the transport —
`narvo_ipc::Client` — lives there too. `narvo-ipc` is therefore the protocol
*and how to speak it*, and it is still not what answers it.**

The second sentence is the part that is a decision rather than a consequence.
Until M6.5a this crate's own documentation said, in as many words, that there was
"no socket, no thread, no clock and no `std::net` here". `Client` makes two
thirds of that false — there is a socket, and `ask` reads `Instant::now()` for
its deadline — so the crate's description changed with it rather than quietly
around it.

What did **not** change, and was checked rather than assumed:

- `narvo-ipc` is still a leaf with no workspace dependency (ADR-0030's
  consequence). `std::net` is not a package.
- `cargo deny list` before and after: **byte-identical** (SHA-256
  `1D7EFE38…6CD9C` both times).
- `cargo tree -p narvo-app --no-default-features --edges normal` before and
  after: **byte-identical** (`A7190028…1B69C`). `narvo-ipc` was already in that
  tree, at line 46, since M6.3a — so the answer to "does this change the headless
  tree?" is measured rather than expected.
- `Cargo.lock` and all four manifests: untouched.
- `FORBIDDEN_IN_HEADLESS`: unchanged, and it could not have been — its seven
  names are packages.

## What the rejected paths cost, in numbers

**Export a client type from `narvo-app`.** Its best argument is that the code
already exists there, works, and is under test: nothing moves, and the client is
written beside the endpoint it talks to, where a change to one is visible from
the other.

The price is the dependency edge it forces on every consumer, and it is a
package count rather than an opinion:

| what a tool would depend on | packages in `--edges normal` |
|---|---|
| `narvo-ipc` | **14** |
| `narvo-app`, `--no-default-features` | 93 |
| `narvo-app`, as a dependency takes it by default | **185** |

A tool under `tools/` that named `narvo-app` would take 185 packages — wgpu,
winit, naga, kira, cpal and the rest of the render and audio stacks — to reach a
type that needs 14. It would also make every MCP client carry `hecs`, which is
the same objection ADR-0030 raised against reusing `EntityId`, one layer up.

**Implement the framing a second time.** Its best argument is that it is about
twenty lines and that the two ends are genuinely separate programs, which is a
normal reason for two implementations of a wire format.

It is refused on the shape of the failure rather than the size of the code.
Two framings at the two ends of **one connection** agree in every test that
checks one end against bytes written by hand, and disagree only when the two
meet — in the field, in a message split across two TCP deliveries, which is
exactly the case a loopback test never produces. It is one rule written down
twice, the class `ProjektPlan.md` §6.10 records as the third sha256 copy — which
survived M4.3's placement deliberation *and* a reviewer because nobody grepped.

So the grep §11's own discipline demands was run before this was decided, and
its result is that after the move the workspace holds **exactly one**
incremental byte-stream line splitter: `framing.rs`'s `while let Some(end) =
… position(|byte| *byte == b'\n')`. Every other line handling in the workspace
is `str::lines()` over text already wholly in memory — a recording being parsed,
a canonical dump being compared — or `BufRead::read_line` in a test. None of
those can end mid-message, so none of them is a framing and none competes with
this one.

That the single implementation is load-bearing is not an argument this ADR asks
to be believed. It was measured — see below.

**A crate of its own, `narvo-ipc-client`.** Its best argument is that it keeps
`narvo-ipc` exactly what M6.1 built, so the sentence quoted above would still be
true.

Against it: it is a crate on spec, which `ProjektPlan.md` §2 rules out, for one
type with one consumer. It would also move the client *out* of the three headless
steps of the verification set, because those are `-p narvo-app` and a crate
`narvo-app` does not depend on is outside them — the M4.8 shape, bought for a
sentence.

## The measurements this decision rests on

**That one framing is load-bearing rather than merely claimed** was measured by
making the two ends disagree, once from each side. A `\r` terminator instead of
`\n`, injected in place of the shared call:

| injection | red in `narvo-ipc` | red in `narvo-app` |
|---|---|---|
| the client frames its own requests | 1 | 2 |
| the endpoint frames its own answers | — | 10 |

Ten of the twenty socket tests in `narvo-app` notice the second, including six
of the nine end-to-end tests that drive a real `narvo` process and read its
answers with `std`'s own `read_line` — an implementation of line reading that did
not move and shares no code with this one. A green would have been the finding.

**Three facts about `std::net` on the pinned 1.97.1 toolchain**, measured on both
platforms in throwaway programs compiled with `rustc` directly, so nothing
entered `Cargo.lock`:

| question | Windows | Linux |
|---|---|---|
| `set_read_timeout(Some(Duration::ZERO))` | `InvalidInput` | `InvalidInput` |
| a read whose timeout expired | `TimedOut` (10060) | `WouldBlock` (11) |
| a 1 µs timeout | expired after 19.5 ms | expired after 11.1 ms |
| peer closed having read everything | `Ok(0)` | `Ok(0)` |
| peer `shutdown(Write)`, anything unread | `Ok(0)`, after its bytes | `Ok(0)`, after its bytes |
| peer closed with bytes it never read | `ConnectionReset` | `ConnectionReset` |
| …and the bytes it had already sent | **discarded** | **delivered** |

Row two is why `is_deadline` matches two error kinds: recognising one would make
the client report a plain timeout as an I/O fault on exactly one platform. Row
three is why there is no minimum-duration clamp — the case that would have needed
one does not arise. Rows six and seven arrived as a **failing test**: the first
version of this client classified a reset as a broken socket, so an engine that
had merely gone away was reported to the agent as a fault, and *that is the
normal way an engine goes*, because a client that has just sent a request has
almost always sent one the engine will now never read.

The last row is the only place the two platforms disagree, and it is why
`ClientError::Closed::mid_answer` documents itself as a report of what the client
holds rather than a claim about what the engine wrote.

## Consequences

- **`narvo-app` compiles a client it never calls.** It is `pub` in a dependency,
  so no lint fires and no gate hides it. The two headless steps that build and
  test `narvo-app` therefore compile it, and one test in that configuration —
  `transport::tests::the_client_and_the_endpoint_understand_each_other`, one of
  the 321 — actually drives it. Nothing in `main.rs` reaches it.
- **`Client` is not behind a feature, and the listener still is.** D20's gate
  exists because *a loopback listener has no access check*. That reasoning is
  about accepting connections. A client makes an outbound connection and grants
  nobody access to anything, so the gate does not reach it — and a gate here
  would need an eleventh verification step to build what nothing else builds.
- **The framing is public API of `narvo-ipc`.** `Lines` and `framed` are
  exported at the crate root, so a third consumer frames the same way rather than
  writing a fourth.
- **`patience` bounds reading and not connecting.** `TcpStream::connect_timeout`
  needs a resolved `SocketAddr` and `Client::connect` takes text, so a refusal
  takes as long as the operating system takes — measured at about 2 s on Windows
  and immediate on Linux for a loopback port with no listener. It is named on
  `connect` rather than left to be discovered.
- **One clock is now read in this crate.** `Client::ask` calls `Instant::now()`.
  It is on the agent's side of the wire, where no engine state is computed, and
  the crate documentation says so under *Determinism* rather than leaving the
  earlier blanket claim standing.
- **This supersedes nothing.** ADR-0030 stands entire — a component still crosses
  as the registry's own bytes, an entity is still named by `EntityName`, and the
  crate is still a workspace leaf. ADR-0031's two answering moments and its gated
  socket are untouched, because nothing here changes when or whether a run
  answers.

## Revision condition

When a second transport exists. D20 chose localhost TCP *on the condition that it
stays cheaply reversible*, and `Client` is now a second thing to rewrite beside
`Endpoint` — still four methods and one type, but no longer one. If that
reversal is ever attempted and the two ends turn out to need different
abstractions, this is the decision to reopen.

Also when a consumer needs framing that is not one message per line — a binary
payload is the obvious candidate, and M6.4b's deferred screenshot is the concrete
one. `no_message_this_protocol_produces_contains_a_line_break` is what would go
red first, and it is a check rather than a caveat for that reason.
