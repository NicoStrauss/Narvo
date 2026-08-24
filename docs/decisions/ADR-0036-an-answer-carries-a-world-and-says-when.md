# ADR-0036: An answer carries the world's own dump, and says when it was true

Status: accepted · Date: 2026-08 · Scope: `narvo-ipc`'s `Request` and
`Response`, `narvo-app`'s `ipc` seam, `tools/narvo-mcp`, and every command the
protocol gains from here on

## Context

`ProjektPlan.md` §6/M6 closes on an agent generating a deterministic repro test
from an observation. M6.7a built the runner — `--expect <file>`, judging a run
against a canonical dump an earlier run produced (ADR-0035) — and then measured
what an agent still could not do. Six items; **two of them carry the gap**:

- **No command produces a canonical dump.** An agent can assemble something
  dump-shaped from `list_entities` and *n* × `get_entity`, and what it assembles
  is its own text: the header line, the two-space indent and the order within an
  entity all belong to `canonical_dump`. `--expect` compares line for line, so it
  would reject the reconstruction — the gap would move, not close.
- **The observed tick is not readable.** `Response::Step` answers `granted`,
  which is the cumulative *budget* and not the position; its own documentation
  said so. An agent that sees something wrong cannot name the tick it saw it at,
  which is exactly what `--ticks N` needs.

Both were left out deliberately and both deferrals are on the record. M6.1:
*"Which tick it observed … is additive to add later."* M6.3c, rejecting it again
on `Step` alone: *"No client exists to need it, and M6.5 is where one first
will."* M6.5b built that client. **This is redeeming two bookings at the
condition one of them named, not repairing an omission.**

## Decision

**Two things, and they are one decision because a dump is useless without the
second half.**

### 1. A world crosses as `canonical_dump`'s own text

`Request::Dump` is a unit variant; `Response::Dump` carries `state`, the exact
string `canonical_dump` produced, in a JSON string, unparsed and unconverted.

That is **ADR-0030's rule applied**, not extended: the protocol carries the
engine's own bytes and converts nothing. ADR-0030 decided it for a *component
value* and already used the dump as its yardstick — *"the protocol is exactly as
faithful as the canonical dump, and no more."* This is the same sentence with the
subject widened, and it needs no new measurement because the bytes are the same
bytes.

**The property, and it is the whole point of the task: byte-identity with what
`narvo --dump` writes.** Not "equivalent", not "the same information" — the same
bytes, so an agent can write the answer to a file and hand it to `--expect`. It
is asserted against the **command-line path**, which this task did not touch, in
`a_dump_off_the_wire_is_the_command_lines_dump_and_the_repro_runner_takes_it`.
A second computation of the dump would have been no reference at all.

**It fails where the command line fails.** `canonical_dump` calls
`reject_unregistered` for every entity (`crates/narvo-ecs/src/state.rs:72`) and
refuses a world holding a component the registry does not know, while
`get_entity` walks the registry and never sees one. So `dump` is the stricter of
the two reads, deliberately: a dump that quietly left a component out would stop
being byte-identical to the command line's — which fails in exactly that case —
and byte-identity is what makes it usable at all. `RequestError::Dump` is its own
variant rather than `Engine`'s, because there is no entity the caller asked about.

### 2. Every answer that came from a world says how many ticks had run

`ticks_run: u64` on all eight world-answering variants. **`Error` is the one
without it**, and the reason is measurable rather than stylistic: a malformed
line is refused by the transport before any world is consulted
(`crates/narvo-app/src/transport.rs`), so an error can exist with no moment at
all. A field there would be absent on some errors and invented on others.

**It is `ticks_run`, not a tick index.** The consumer is `--ticks N`: an answer
saying `ticks_run: 7` is an answer about the world a run of seven ticks ends in,
so a repro is `--ticks 7 --expect <that state>` with no arithmetic in between.
The number is `Moment::ticks_run()`, which already existed — inside tick *n* it is
*n + 1*, because the drain happens after that tick's systems — and which is the
same number D19 cuts a band at.

**No new observation moment.** The value is *read from* the moment ADR-0031
established; nothing here chooses one. That ADR's one-observation-point rule is
what makes the field meaningful at all, and this is the first thing to spend it.

## Rejected: a `get_tick` command, refuted by measurement

The strongest alternative by far, and it was measured rather than argued. Its
best argument is cost: a command adds nothing to any existing response, so **88
`Response::` construction sites and about thirty pinned wire literals across three
crates stay exactly as they are**, and a client that never asks pays nothing.

What loses it is that the number would be wrong precisely when it matters. A
separate command is answered at **its own** moment, and the moments differ.
Measured on a mid-flight `--mode motion` run over a real socket, four sequential
round trips reading one moving component:

```
read 0: …"value":"(x:-34328,y:-17164)"…
read 1: …"value":"(x:-34354,y:-17177)"…
read 2: …"value":"(x:-34368,y:-17184)"…
read 3: …"value":"(x:-34382,y:-17191)"…
→ 4 distinct answers out of 4
```

Two requests written before either was read came back identical — one drain, one
moment — so the ordering guarantee within a drain holds and is not what is at
issue. What is at issue is that an agent doing `get_component` then `get_tick`
gets a tick from a *later* world, silently, and builds a repro that names the
wrong length. That is ADR-0011's concern at a smaller scale, and it is the reason
the number rides in the answer it belongs to.

The churn is real and is accepted. Two things bound it: every one of the 88 sites
is a **compile error** until it is fixed, so nothing can be missed quietly; and
the meaning of the number is pinned by an unmoved external reference — a run of
`ticks_run` ticks on the command line — rather than by an expectation somebody
edited to match the implementation.

## Rejected: the tick on the reads only

Four variants instead of eight — `list_entities`, `get_entity`, `get_component`,
`dump` — on the argument that the field dates an *observation* and an action's
answer is not one. Half the churn, and coherent.

Three things lose it. `Response::Step`'s own documentation asks for the number by
name, and M6.3c rejected it **there**; redeeming that booking everywhere except
where it was made would be a strange reading of it. A write's answer is the only
place D19's cut point can ever be learned, because a cut band is
byte-indistinguishable from an ordinary short one (ADR-0032). And "these four
carry it" is a list to remember, where "everything except an error" is a rule —
`Response::ticks_run()` is an exhaustive match, so a variant added later has to
answer the question rather than inherit an answer by omission.

## Rejected: a JSON-native dump, and a wrapper

A structured `{"entities":[{"name":…,"components":[…]}]}` is ADR-0030's rejected
path one level up, and it loses for the same two reasons plus a third: it would be
a **second definition of the dump format**, free to drift from the one
`canonical_dump` writes and `--expect` reads.

A wrapper — `{"ticks_run":7,"response":{…}}` — would put the moment in one place
instead of eight. It breaks `every_request_has_an_answer_with_the_same_tag`, which
is the guard that a command cannot reach the wire without an answer, and it
changes the shape of every response for the sake of a field that is one word.

## Consequences

- **ADR-0030's additive-growth cost is named and does not bite here.** That ADR
  says the protocol "cannot grow a field additively without a client update"
  because unknown fields are refused. There is a deployed client now
  (`tools/narvo-mcp`, M6.5b) — and it reads `narvo-ipc`'s own `Response` type
  (ADR-0033), so both ends move in one commit and no skew is possible. What would
  make the cost real is an **out-of-tree** client, and the answer then is the
  version handshake ADR-0030 already names.
- **The MCP server needed no work to carry the field.** It passes the engine's
  answer verbatim, so `ticks_run` reaches an agent because nothing intercepts it.
  What it did need is a `dump` tool, and the description says what the number is
  for, because a description is the one place an agent looks before it makes a
  mistake.
- **M6.1's intent gate fired across the crate boundary, as designed.** Adding one
  variant to `narvo-ipc` stopped `narvo-app` and `tools/narvo-mcp` compiling at
  their exhaustive matches, so `dump` could not reach the wire without somebody
  deciding whether an agent may see it and what it does during a replay.
- **A dump is answered during a replay.** ADR-0032's criterion applied rather than
  extended: *"a run that is reproducing a recording answers reads and refuses
  every command that would change what it reproduces."* A dump changes nothing —
  and it is the answer a replay exists to produce.
- **What is still missing for an agent with no shell is unchanged and named**:
  nothing starts a recording over the wire, nothing says whether a run is
  recording or where, and nothing writes a file. M6.7b closed the *content* gap,
  not the carrier one, and ADR-0035's decision that a repro is handed over is why
  the carrier is not this protocol's problem.

## Revision condition

Reopen if a response ever has to be produced with no world behind it beyond
`Error` — a second transport-level answer would make "every answer but an error"
false, and the rule would need restating rather than patching. Or if an
out-of-tree client appears, at which point ADR-0030's version handshake stops
being hypothetical.
