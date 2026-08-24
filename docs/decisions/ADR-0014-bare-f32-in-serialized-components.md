# ADR-0014: Serialized components hold bare scalars; a maths library is never in the state

Status: accepted · Date: 2026-08-07 · Scope: every component that is registered
and therefore appears in the canonical dump

## Context

`Transform` is the first component M3 adds, and the first one that is
unambiguously engine vocabulary rather than a demo type. It is also the first
addition to the stability domain since ADR-0013 gave that domain a name.

That matters more than the type does. ADR-0008 fixed *what* the state hash
guarantees and ADR-0013 stated *for which domain* Windows and Linux are promised
to agree — but neither says what a component may be made of. Until now the
question did not arise: the registered types were `Rng` (two `u64`), `Events<T>`
(a buffer of the caller's type), and the demo components, all built from scalars
because nothing else was available. `Transform` is the first type for which a
ready-made library representation exists and is tempting.

`glam` is the library in question. It is the obvious choice for 2D and 3D maths
in Rust, it has serde support, and `glam::Vec2` would express two of
`Transform`'s five fields more directly than two named `f32` do.

## Decision

**A component that is registered holds bare scalars — `f32`, `u64`, `bool` and
the like — and no type from a computation library. `glam`, if and when it
arrives, is a tool for computing and is converted to at the point of use. It is
never a field of a serialized component and never appears in the canonical
dump.**

This is D11 in `ProjektPlan.md` §11, decided by the maintainer. This ADR records
it and states what it means for the components that come after `Transform`.

## Why

A foreign type in a registered component puts that library's serde
representation inside the domain ADR-0008 governs. The consequence is concrete
rather than theoretical: `glam` chooses how a `Vec2` is emitted — as a sequence,
as a struct with named fields, with or without a wrapper — and the next version
that changes that choice moves every state hash in the project. Nothing would be
wrong when it happened. No simulation would have changed. The determinism suite
would go red, the release-profile workflow would go red, and the cause would be
a dependency doing something it is entitled to do.

That is precisely the failure ADR-0008 exists to bound. Its rule that no expected
hash is ever committed exists so that a `ron` or `serde` bump moves both sides of
a comparison together. A `glam` type in a component would reintroduce the same
hazard one level down, where the rule cannot reach it: the two sides would still
move together, but every *recorded* observation — ADR-0013's evidence table, a
`BASELINE.md` row, a bisect across the bump — would silently stop referring to
the same thing.

Bare scalars have no such owner. `f32`'s serde representation is a number, and
what a number looks like in RON is settled by the format, which the engine
already depends on and already tracks.

## The alternative that was rejected, and its argument

**Store `glam` types and convert at the serialization boundary.** Components
would hold `Vec2`; the registry's type-erased path would translate to and from a
scalar form on the way into and out of the dump. The state would then be exactly
as stable as this decision makes it, *and* systems would get vector arithmetic
without writing `t.x` and `t.y` twice.

The argument against it is not that it would fail. It is a conversion layer at
every system boundary, written and maintained now, for a problem that has not
occurred: no `glam` is in the tree, no system does vector arithmetic, and the
first one that wants to can convert in three lines at the call site. The layer
would have to be correct for every component type, would need its own tests, and
would be the sort of infrastructure that is easy to add and hard to remove once
callers depend on its shape.

The honest cost of rejecting it is also worth writing down: systems will convert
by hand, and a system that forgets to convert one field of five gets a plausible
result rather than a compile error. That is a real risk and it is accepted,
because it is visible in a diff, whereas a hash that moves under a dependency
bump is not.

## Consequences

- **The rule for the next component, not just for this one.** Any type that is
  passed to `ComponentRegistry::register_component` holds only scalars,
  `String`, and other registered-component types. Adding a dependency's type to
  a registered component needs an ADR superseding this one, with an argument for
  why that dependency's serde format is more stable than the state hash it would
  govern.
- **A round-trip test on bit patterns is part of registering a component that
  holds floats.** `==` calls `0.0` and `-0.0` equal and two NaNs unequal, so it
  cannot see what a hash sees. `f32::to_bits` can. CLAUDE.md already requires a
  round-trip test for serializable types; for floats it has to be this one.
  Measured for `Transform` in M3.1's successor task and recorded there: the RON
  round trip is bit-exact on values that stress it, including a subnormal, a
  magnitude at `f32::MAX`, `-0.0`, and a pair one mantissa bit apart.
- **The stability domain of ADR-0013 now includes a component nothing renders.**
  `Transform` is in the hash from the moment it is registered, which is before
  anything draws it. That order is deliberate: the component and its place in
  the hash are verifiable on their own, and a render path that reads it later
  cannot quietly change what is hashed.
- **ADR-0008's rule is untouched and applies here.** No hash of a `Transform`
  world is committed to this repository. The tests that prove the hash sees the
  component compare two worlds against each other, never against a stored value.
- **`glam` remains a live option as a computation dependency.** Nothing here
  argues against adding it; the decision is only about what crosses the
  serialization boundary. It is D10 in `ProjektPlan.md` §11 and not settled by
  this ADR.

## Amendment, 2026-08-12 (M6.1): a discriminant is a scalar too

This rule has had a second half since M3.23 and it lived only in a source
comment — `crates/narvo-ecs/src/sampling.rs:18-43`, where M5b.3a left it. A rule
in one type's documentation is a rule the author of the *next* component has no
reason to read, so it is written down here. Nothing above changes; this states
what was already being done.

**A choice between named alternatives is stored in a registered component as a
bare integer, with the mapping written out in that component's own
documentation. The enum type, if there is one, lives above the component and is
converted to at the point of use — the same relationship the decision above gives
`glam`.**

`Sampling` is the instance: one `pub filter: u8`, with `0` and `1` given meaning
in a table on the type, and the renderer-side mapping in `narvo-app`, which is
the crate that sees both the world and the renderer (ADR-0015).

### Why, measured rather than asserted

Because an enum's serialized spelling is a *serde attribute's* choice, not the
format's, and the attribute can be added later by someone who has no idea a state
hash depends on it. Measured against `ron 0.12.2` in M6.1:

| what is stored | RON |
|---|---|
| `enum { Nearest, Linear }`, unit variant | `Nearest` |
| the same enum with `#[serde(rename_all = "snake_case")]` | `nearest` |
| a newtype variant `Nearest(3)` | `Nearest(3)` |
| a struct variant `Linear { steps: 3 }` | `Linear(steps:3)` |
| `filter: u8 = 0` | `0` |

Four spellings for one concept, chosen by attributes. And the change is not
merely a moved hash: after adding `rename_all`, the old spelling no longer
parses at all —

```text
1:1-1:8: Unexpected variant named `Nearest` in enum `Renamed`, expected `nearest` instead
```

— so every stored dump written before the attribute becomes unreadable, which is
the failure the decision above describes for `glam` with a sharper edge on it. An
integer has no attribute that can move it.

### The reserved-code rule, which is part of the same decision

An unknown code is **not** rejected by the component. `Sampling` says so and
gives the reason: a component is storage, and rejecting inside it would mean
deciding what a partially valid world is — the same reason `Layer` does not
reject a `NaN` depth. A reader maps an unknown code to the default, and that
mapping lives with the consumer rather than with the storage.

The cost is the one this ADR already accepts in its main text: the mapping is
written by hand, and a mapping that forgets a code produces a plausible result
rather than a compile error. It is visible in a diff, which is the trade already
made.

## Revision condition

When a component genuinely cannot be expressed in scalars without losing meaning
— a matrix whose element order is the thing being stored, say — or when the
hand-conversion cost this decision accepts is shown to have produced a real
defect rather than a hypothetical one. Either is an argument for a new ADR with
its own evidence, not for widening this one in place.
