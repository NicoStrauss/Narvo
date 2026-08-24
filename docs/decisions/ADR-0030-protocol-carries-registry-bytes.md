# ADR-0030: The protocol carries the registry's own bytes, and its vocabulary is its own

Status: accepted · Date: 2026-08-12 · Scope: `narvo-ipc`, every client of it,
and anything that later puts a component value on a wire

## Context

M6 exposes a running engine to an agent. `ProjektPlan.md` §6 fixes the encoding
in one clause — *"IPC-Protokoll (JSON über lokalen Socket)"* — and M6.1 builds
the vocabulary that clause describes: what a request and a response are, as
types, with no transport and nothing executing them.

Two questions have to be answered before a single field is written down, and
neither has an obvious answer.

**How does a component value cross?** The engine already has exactly one
type-erased path from an entity to a component's text. It is
`ComponentRegistry`, and its shape is a function pointer stored at registration:

```rust
type SerializeComponent = fn(&World, EntityId) -> Result<Option<String>, EcsError>;
```

`crates/narvo-ecs/src/registry.rs:20`. The body is `ron::to_string` at `:385`,
fixed by ADR-0006, and the type parameter is consumed at registration — which is
the whole of the erasure and also the whole of the constraint. The path returns
a `String`, and there is no second one.

**How is an entity named?** `EntityId` is index plus generation, its serde
representation is documented as stable API (`entity.rs:32-44`), and it derives
`Serialize`/`Deserialize`. But `EntityId::from_parts` is `pub(crate)`, and its
documentation says why in as many words: a handle *read out of text* "is only
meaningful in the world the dump came from", and "exposing a public constructor
would invite fabricating handles and looking them up, which is how a
stale-handle bug gets written" (`entity.rs:92-95`).

That intent turns out to be only half enforced, which M6.1 found by checking it
rather than repeating it. `EntityId` is a public type with a derived
`Deserialize`, so a fabricated handle is one `ron::from_str` away from any crate
in the workspace. The finding is reported to `narvo-ecs` rather than acted on
here, and it does not weaken the decision below — it sharpens it, because a
guarantee that is only a convention is not one a protocol may lean on.

## Decision

**A component value crosses the protocol as the exact string the registry
produced — RON, inside a JSON string, unparsed, unconverted and byte for byte.
`serde_json` never sees a float. And `narvo-ipc` names an entity with its own
type, `EntityName`, so the crate stays a leaf with no workspace dependency.**

The two halves are one decision because they have one shape: the protocol
boundary reuses what the engine already guarantees, and converts nothing.

## The measurement the decision rests on

Measured in M6.1 against `serde_json 1.0.151`, `ron 0.12.2` and `serde 1.0.229`,
over fifteen `f32` values chosen to stress a float path — `0.1` and the value one
mantissa bit above it, both signed zeros, the smallest subnormal,
`MIN_POSITIVE`, `±MAX`, both infinities, `NaN`, and four ordinary magnitudes.

| path | finite values | `NaN`, `+inf`, `-inf` |
|---|---|---|
| `f32` as a JSON number | 12 of 12 bit-identical | written as `null`; reading `null` back as an `f32` is an error |
| `f32` widened to `f64` as a JSON number | 12 of 12 bit-identical after narrowing | same three `null`s |
| `serde_json::Value` as an intermediate | 12 of 12 bit-identical | same three `null`s |
| RON text carried in a JSON string | 15 of 15 byte-identical | carried as `inf`, `-inf`, `NaN` |
| RON text via `ron::Value` to `serde_json::Value` | finite values survive | `inf` and `NaN` become `null` |

Three things in that table decided it.

**The loss is silent going out and loud coming back.** `serde_json` does not
refuse a non-finite float; it writes `null` and returns `Ok`. The failure
surfaces only when somebody tries to read the value again — at which point a
`get` followed by a `set` has written a different state than the one it read,
and that state is in the hash. This is the class M5b.3a caught at
`format!("{:.6}")`, one step further along: there, two neighbouring values
merged; here, three values vanish.

**The transcode is not a way out.** Parsing the registry's RON into a dynamic
value and re-emitting it as JSON avoids touching the registry, so it was
measured rather than dismissed. It loses the same three values, for the same
reason — the destination has no representation for them. It buys nothing and
costs a converter with no oracle.

**Carrying the bytes inherits the registry's fidelity exactly, including its
limit.** RON round-trips all fifteen values bit-exactly *except* that a `NaN`
payload collapses: `0x7fc0002a` and `0x7f800001` both render as `NaN` and both
read back as `0x7fc00000`. Only the sign survives. That limit is the canonical
dump's already — `canonical_dump` calls the same `ron::to_string` — so the
protocol inherits it rather than adding to it, and
`a_nan_payload_is_lost_by_the_registry_before_the_protocol_sees_it` is where it
is written down as a check rather than as a caveat.

The wire shape was measured the same way, by putting malformed requests through
both serde enum representations and reading the position off the error. The two
probe runs did not use identical input lists, so the comparison is stated over
the six shapes they have in common — the ones an agent actually produces:

| malformed request | externally tagged | internally tagged |
|---|---|---|
| the entity field missing | located | `0:0` |
| entity `"3x1"` — no `v` | located | `0:0` |
| entity `"3v0"` — generation zero | located | `0:0` |
| entity `""` | located | `0:0` |
| entity given as a number | located | `0:0` |
| the component field missing | located | `0:0` |

Six of six against none of six. Over its own full list, the externally tagged
form located **every one of eleven rejections across twelve inputs**, and that
list is now `every_malformed_request_is_located` in `error.rs`, twelve malformed
inputs, all of which must come back with a line and a column of at least one.

Internally tagged serde buffers the content past the tag, so the position is gone
by the time a field is read — including for every error raised inside
`EntityName`'s own parser. What it still locates are the failures decided
*before* the buffering starts: an unknown variant, a missing `command`, and JSON
that is not JSON.

Nothing else separated the two. `deny_unknown_fields` was expected to be the
second argument and is not: measured against `serde 1.0.229`, it works on the
internally tagged form too and rejects an extra field with the same wording. The
position is the whole of the case, and it is enough. So the protocol is
externally tagged, and `every_malformed_request_is_located` holds it.

## The rejected paths, with their best argument

**A JSON-native component value — `{"x":1.0,"y":2.0}` on the wire.** Its best
argument is real and is the reason it was measured first: the other end of this
protocol is an MCP client, the MCP ecosystem is JSON end to end, and this
decision makes every such client carry a RON parser to read a value the engine
already had in hand. It also has the measurement partly on its side — for the
twelve finite values, `serde_json` is bit-exact, so ADR-0014's round-trip
concern does not bite where a game's numbers actually live.

Two things sank it. It cannot represent three values a registered component is
allowed to hold: the engine's own components document non-finite values as
reachable rather than impossible — `Sampling`'s reserved-code note says that
`Layer` "does not reject a `NaN` depth", because a component is storage and
rejecting inside it would mean deciding what a partially valid world is. A format
that drops what storage admits is the wrong format for reading storage. And
getting a JSON-native value out of the engine at all would need a *second* output
format in
`ComponentRegistry`: a second function pointer stored at registration, which is
a change to `register_component`, which is an ADR superseding ADR-0006. M6.1's
brief stops at that branch rather than crossing it, and this decision does not
need to cross it.

**Reusing `EntityId` and depending on `narvo-ecs`.** Its best argument is that
the type exists, its serde representation is already documented as stable API,
and a handler would then receive a handle with no conversion to write. Against
it: `narvo-input`, `narvo-audio` and `narvo-physics2d` are all leaves, and an
edge here would put `hecs` underneath every future MCP client for the sake of
two `u32`. More sharply — a name that arrived over a socket is a *fabricated*
handle, exactly what `from_parts` is `pub(crate)` to discourage, and typing it as an
`EntityId` would make the fabrication invisible at the type level. Two types mean
the check cannot be skipped, because it is a conversion rather than a move.

**A uniform `{"command": …}` object for every request.** Its best argument is
shape: one key always present, so an agent composing JSON by hand writes the same
skeleton every time, instead of a bare string for one command and an object for
the others. The position measurement above is what decided against it.

## Consequences

- **A client parses RON to read a component value.** Stated plainly because it
  is the price, and it lands on `tools/narvo-mcp` (M6.4) rather than here.
- **The protocol is exactly as faithful as the canonical dump, and no more.**
  That is the whole guarantee, it is checkable, and
  `crates/narvo-ipc/tests/registry_bytes.rs` is where it is checked — the
  registry's own bytes out, across, and back into a world, compared with
  `to_bits` as ADR-0014 requires.
- **No float appears anywhere in the protocol schema.** Not in a component
  value, which is a string, and not in an entity name, which is also a string.
  `to_json` is therefore infallible for both types, which is why it returns a
  `String` and not a `Result`.
- **`narvo-ipc` stays a leaf.** `serde`, `serde_json`, and `narvo-ecs` as a
  **dev**-dependency only, so the integration test can push real registry output
  through the protocol. The same arrangement ADR-0025 gives `narvo-input`, and
  the headless `cargo tree` check does not see it because that check uses
  `--edges normal` (ADR-0016).
- **A future write command inherits this.** Whatever M6.2 decides about
  semantics, the *value* it carries is the registry's text, because a write that
  spelled values differently from a read would be two formats for one thing.
- **Unknown fields are refused rather than ignored.** A protocol that drops what
  it does not understand answers a request nobody made. The cost is that the
  protocol cannot grow a field additively without a client update, and there is
  no deployed client yet to make that a real cost. When there is one, the answer
  is a version handshake, and that belongs with the transport (M6.3).
- **`serde_json`'s `null` means exactly one thing here:** a component the entity
  does not carry, mirroring `ComponentInfo::serialize`'s `Ok(None)`. It never
  means "a number that could not be written", because no number is ever written.

## Revision condition

When a client is measured to be worse off for parsing RON than for the
alternative — a real MCP client, with the cost counted rather than predicted —
or when a component value has to carry something RON cannot spell. Either is an
argument for a new ADR with its own evidence. Note that the first of those
reopens ADR-0006 as well, because a JSON-native value needs a second output
format in the registry, and that is the decision this one deliberately did not
take.

The measurement half has its own trigger, and it is automatic: if `serde_json`
ever gains a representation for `NaN` and the infinities, or `ron` starts
preserving a `NaN` payload, `a_json_number_path_would_have_lost_all_three_of_them`
and `a_nan_payload_is_lost_by_the_registry_before_the_protocol_sees_it` go red.
Both are content anchors over something a dependency generates — ADR-0008's third
kind of literal, where the dependency moving the value is the finding.
