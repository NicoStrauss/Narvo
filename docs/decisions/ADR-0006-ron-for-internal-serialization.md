# ADR-0006: The component registry serializes into RON

Status: accepted · Date: 2026-08 · Scope: narvo-ecs (`ComponentRegistry`)

A record of a decision already in the code, written down because the Definition
of Done asks for one. It is not a fresh weighing of options.

## Context

hecs stores components by `TypeId` and has no reflection, so nothing can name,
list or serialize a component type unless something was told about it first. That
is what `ComponentRegistry` is: a stable name per component type plus a
type-erased path from an entity to that component's serialized form.

The erased path is a function pointer, one instantiation per registered type. A
function pointer cannot be generic over a `Serializer`, and serde's `Serializer`
is not object safe, so the format cannot be left to the caller without a crate
like `erased-serde`. A concrete format therefore had to be picked at M2.1, when
the registry was written, rather than deferred.

## Decision

The registry serializes with `ron::to_string`, at
`crates/narvo-ecs/src/registry.rs:297`. `ron` is a normal dependency of
`narvo-ecs` for that one call.

**Scope: the internal, type-erased serialization of components inside the
registry.** Nothing else. The output has no wrapper — it is byte for byte what
`ron::to_string` produces for the component itself, which is what makes
"type-erased output equals direct serde" a checkable property, and
`the_type_erased_path_writes_what_serde_writes` checks it.

**This is not D3.** The scene and content format — what a `.ron` or `.json` file
in the repository looks like — is D3 in `ProjektPlan.md` §11, is recommended but
not decided, and falls by M4. This ADR does not pre-empt it and does not count as
having answered it. The two are separate questions that happen to have the same
candidate answer today: one is an implementation detail of a crate, the other is
the format a human and an agent read and write by hand.

## Rationale

1. Some format had to be named for the erased path to exist at all, and adding
   `erased-serde` to avoid naming one would have been a larger commitment than
   naming one.
2. RON is what `ProjektPlan.md` §3.2 lists for serialization and what D3
   recommends. Choosing it costs nothing extra today and leaves the internal
   format aligned with the likely content format.
3. It is text, so a serialized component is readable in a test failure without
   tooling.

## Consequences

- `ron` appears in `[dependencies]` rather than `[dev-dependencies]`, because the
  call is in library code.
- **Open follow-up, M2.2.** The canonical state hash will be built over these
  strings. That makes it depend on RON's formatting details and on serde's field
  order, and a `ron` version bump can move every recorded hash without a single
  line of engine code changing. What the hash guarantees — over what, how
  stable, and for how long — is the *stability domain* of the hash. It is named
  here and deliberately not decided here: it is an M2.2 question and belongs in
  its own ADR alongside the hash itself.
- If D3 lands on JSON, this ADR is revisited but not automatically overturned.
  An internal format and a content format may legitimately differ; that would be
  a decision to take on its own merits, not a consequence.

## Revision condition

Reopen when M2.2 fixes the hash's stability domain, if D3 resolves in a way that
makes one format across both worth the churn, or on a `ron` major version bump,
which can change the output for unchanged input and therefore every hash derived
from it.
