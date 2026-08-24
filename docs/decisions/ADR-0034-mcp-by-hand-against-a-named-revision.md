# ADR-0034: MCP is implemented by hand, against a named revision

Status: accepted · Date: 2026-08-13 · Scope: `tools/narvo-mcp`, and any later
consumer of the Model Context Protocol in this workspace

## Context

M6.5b puts an MCP server in front of a running engine. M6's cut, written before
the task, said this would be "possibly the first substantial external dependency
since `kira`" and marked that as an assumption to be tested rather than a plan.

Both readings were live. MCP is a real protocol with version negotiation, a
partitioned error-code space and an official Rust SDK; a server written by hand
against it can be silently incompatible in a way no test in this repository would
catch, because no MCP client is in this repository. Against that, the workspace
already holds both halves of what a stdio MCP server needs: `serde_json` since
M6.1, and since M6.5a a line framing that both ends of a connection call
(ADR-0033).

The question is which of the two failure modes is cheaper to carry, and it was
decided by measurement rather than by preference.

## The revision this is decided against

**MCP `2026-07-28`**, read at <https://modelcontextprotocol.io/specification/>
on 13.08.2026, which that page names as the current specification.

Naming it is load-bearing rather than a courtesy. The revision published
2026-07-28 is not a point release: it **removed the `initialize` handshake**.
Every request now carries its protocol version, the client's capabilities and
optionally its identity in `_meta`, servers **MUST** implement a new
`server/discover` method, and the specification calls the two sides of that line
"modern" and "legacy" and prints a compatibility matrix for the four
combinations. A decision taken against "MCP" without a date would have been taken
against two different protocols.

## Decision

**The protocol is implemented by hand in `tools/narvo-mcp`, on
`narvo_ipc::framing` and `serde_json`, with no MCP dependency. The revision is
`2026-07-28`, it is a constant in the code (`server::VERSION`), and every wire
shape is pinned in a test against the specification's own verbatim example
JSON.**

Only the `tools` capability is implemented, and only the modern era.

### Why the surface makes this reasonable

A tools-only stdio server is three methods and six error codes:

| method | why it is here |
|---|---|
| `server/discover` | servers **MUST** implement it |
| `tools/list` | required of anything declaring the `tools` capability |
| `tools/call` | the point of the exercise |

| code | when |
|---|---|
| `-32700` | the line is not JSON |
| `-32600` | it is JSON and not a JSON-RPC message |
| `-32601` | no such method — including a legacy `initialize` |
| `-32602` | a required `_meta` field, an unknown tool, an unusable parameter, a cursor this server never issued |
| `-32022` | `UnsupportedProtocolVersion`, the one code from MCP's own reserved sub-range |
| `-32603` | the engine could not be reached or could not answer |

Plus two rules that are not codes: a notification is answered with silence, and
`stdout` carries nothing that is not an MCP message.

That is roughly the size of `narvo-ipc::protocol`, which this workspace built
and pinned in M6.1.

### And the framing is already here

The stdio binding asks for exactly what ADR-0033 already built, in the same
words: "Messages are delimited by newlines, and **MUST NOT** contain embedded
newlines." So `narvo_ipc::framing::{Lines, framed}` serves the MCP wire as well
as the engine's, in the same process, and an SDK would have brought a second
framing into it — the failure ADR-0033 exists to prevent, one layer further out.

## What the rejected routes cost, in numbers

Measured on the pinned 1.97.1 toolchain, in throwaway crates outside the
workspace so that `Cargo.lock` stayed untouched until the decision was made. That
procedure is ADR-0028's, and it is used here for the same reason: a measurement
that changes the tree it is measuring is not one.

| route | packages new to this workspace | clean debug build | newest revision it carries |
|---|---|---|---|
| `rmcp` 3.1.2 | **28**, `tokio` among them | 0.60 s → **12.34 s** | 2026-07-28 |
| `rust-mcp-schema` 0.10.3 | **1** | not measured | **2025-06-18** |
| by hand | **0** | unchanged | 2026-07-28, by this crate's own reading |

### `rmcp` 3.1.2 — the best rejected alternative, in full strength

It is the official Rust SDK, it is actively maintained (3.1.2 published
2026-08-07, six days before this decision), its licences all pass the policy, and
it does carry the current revision — `ProtocolVersion::V_2026_07_28`, at
`rmcp-3.1.2/src/model.rs:170`, alongside four older ones.

**Its strongest argument is the one this decision has no answer to**: version
negotiation and error codes are exactly where a hand-written server can be wrong
in a way that produces no local symptom, and rmcp's are the SDK's, exercised by
every other Rust MCP server there is. That is a real reduction in risk and it is
not obtained here.

Three things outweigh it:

1. **It is an async runtime.** `tokio` is unconditional (`tokio ^1` in rmcp's own
   dependency list), and this workspace contains no async code at all: the client
   this server talks through is `narvo_ipc::Client`, which is blocking
   `std::net`, and the engine it reaches is a single-threaded tick loop. Adopting
   rmcp means bridging a blocking client into an async runtime for a program whose
   whole job is to move one line at a time.
2. **The cost is measured and it is twenty-fold.** 28 packages and 0.60 s → 12.34 s
   on a clean debug build, against §8.1's budgets, for a program of **800 lines**
   of production code — blank lines, comments and test modules excluded.
3. **The risk it removes is smaller than it looks, and that is honest rather than
   convenient.** No MCP client is in this repository or in CI, so conformance is
   unverified either way — with rmcp it would be *probable* rather than
   *demonstrated*. What a hand-written server can do instead, and does, is pin
   every wire shape to the specification's own printed JSON, which is a
   machine-checkable claim of the kind this project's Definition of Done asks for
   and which an SDK would not have supplied either.

### `rust-mcp-schema` 0.10.3 — the cheap middle, and why it is not available

One package, MIT, types only, no runtime. It is the route that would have given
the specification's own vocabulary for almost nothing.

It cannot be used, and the reason is a measurement rather than a judgement: its
newest generated schema is `2025-06-18`
(`rust-mcp-schema-0.10.3/src/generated_schema/2025_06_18/mcp_schema.rs:16`), it
was last published 2026-06-24 — a month before the revision — and 2025-06-18 is
on the **legacy** side of the era boundary. Its types describe an `initialize`
handshake this server does not have and no `server/discover` at all.

### The precedent

`narvo-cli` measured `clap` at 21 crates and 0.29 s → 3.88 s in M4.2 and
declined it for one subcommand, one positional argument and one flag, with the
measurement written into its own manifest. This is the same question one layer
up, measured the same way, and the numbers are larger in both columns.

## What this costs, stated rather than implied

- **Every future revision is manual work.** MCP has published five revisions in
  under two years (2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25, 2026-07-28),
  and one of those four transitions removed the handshake. rmcp would have
  carried them; this will not. The mitigation is that `server::VERSION` is a
  constant and an unsupported version is a specified error with the supported list
  in it, so a client meeting a stale server is *told* rather than left to guess.
- **Legacy clients cannot use this server.** The specification's own matrix says
  legacy client plus modern server "Fails", and this implements only the modern
  era. What is done about it is the one thing the specification asks of a
  modern-only server: an `initialize` is refused with a message naming the version
  this server does speak, because "legacy clients have no fall-forward mechanism,
  and this message may be the only diagnostic they can surface to users".
  Implementing both eras would roughly double the surface and is additive if
  something needs it.
- **Conformance rests on a reading.** Named as `tools/narvo-mcp`'s standing
  limit in `server.rs`, alongside what is checked instead. The first real client
  to connect is the evidence, and nothing here is.

## What this decision does *not* change

- **The wire the engine speaks.** ADR-0030 is untouched: a component value still
  crosses as the registry's own RON inside a JSON string, and this server carries
  the engine's answer into a tool result verbatim rather than re-rendering it. No
  float reaches `serde_json` at either boundary.
- **The error taxonomy.** M6.3a deferred the question "does the taxonomy belong on
  the wire?" to the first consumer that wanted to branch on it. This is that
  consumer, and the answer is **no**: MCP's own two mechanisms — a JSON-RPC error
  for what a model cannot fix, a result with `isError: true` for what it can —
  land exactly on the two types that already existed, `narvo_ipc::Response::Error`
  on one side and `narvo_ipc::ClientError` on the other. The wire keeps carrying
  a sentence.
- **The tool list.** It is one tool per protocol command, because the curation was
  done in M6.1 and M6.4a. It does not vary with the run's state, and that is the
  specification's requirement rather than a simplification: `tools/list` "**MUST
  NOT** vary per-connection or as a side effect of other requests on the
  connection".

## Revision condition

Re-examine when any of these is true:

1. **A real client fails against this server.** That is the evidence the named
   limit says does not exist yet, and it outranks every measurement above.
2. **MCP publishes a revision this server does not speak** and something needs
   it. The choice is then between a second constant and adopting the SDK, and the
   package count should be re-measured rather than quoted from here.
3. **This workspace acquires an async runtime for another reason.** Two of the
   three arguments against rmcp are about `tokio` being new here; if it stops
   being new, only the package count and the second framing remain.
4. **The surface grows past tools.** Resources, prompts, elicitation,
   subscriptions or the Tasks extension are each larger than everything
   implemented here, and a server that wanted several of them is a different
   arithmetic from the one this ADR did.
