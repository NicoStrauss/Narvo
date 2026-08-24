# ADR-0005: The query facade seals hecs behind one sealed trait

Status: accepted · Date: 2026-08 · Scope: narvo-ecs (`Query`)

This ADR records a decision that was already taken, so it documents where the
seam sits rather than weighing alternatives against each other.

## Context

ADR-0002 chose hecs as the storage and query engine and required an
engine-owned facade around it: no hecs type in the public API of any other
crate. `narvo-ecs` is that facade.

Entities, components and scheduling wrap cleanly — hecs' handles, its component
access and its errors all have Narvo counterparts, and nothing of hecs shows
through. Queries do not. A query is written as the type it fetches,
`(&Position, &Velocity)`, and the machinery that turns that type into a fetch
over archetype storage *is* hecs' `Query` trait. Re-declaring it would mean
re-implementing hecs' component fetching, which is the thing ADR-0002 decided
not to build.

## Decision

`narvo_ecs::Query` is a sealed marker trait with hecs' `Query` as its
supertrait and a blanket implementation over it:

```rust
mod sealed {
    pub trait Sealed {}
    impl<Q: hecs::Query> Sealed for Q {}
}

pub trait Query: hecs::Query + sealed::Sealed {}
impl<Q: hecs::Query> Query for Q {}
```

That supertrait bound, in `crates/narvo-ecs/src/query.rs`, is the seam — the
single place where hecs is visible in this workspace's public API. Everything
around it is Narvo': `World::query` takes `Q: narvo_ecs::Query` and returns
`narvo_ecs::QueryBorrow`, which hands out an `narvo_ecs::QueryIter` over
`(EntityId, Item)`.

The guard is part of the decision. hecs' `QueryBorrow` holds the borrow that the
handed-out references live in, so it has to outlive the iterator; a
`fn query(&self) -> impl Iterator` cannot express that, because the guard would
be a temporary of the call. Hence two steps at the call site, with the guard in
its own binding.

## Rationale

1. **A caller never names hecs.** The turbofish contains only the caller's own
   types, the iterator yields the facade's `EntityId`, and errors are
   `EcsError`. hecs is not re-exported, so nothing downstream can reach it
   without adding its own dependency on it — which review catches, because it
   shows up in a `Cargo.toml`.
2. **The seam cannot widen.** The trait is sealed, so no outside type can
   implement it, and the blanket implementation means the set of queries is
   exactly the set hecs supports — no more, and no place to smuggle anything in.
3. **ADR-0002 stays intact.** Its requirement is that the *agent-facing* API is
   ours and that hecs stays swappable. Replacing the storage engine means
   rewriting `narvo-ecs`, which is what the crate is for; it does not mean
   touching a single system, because no system names the engine.

## Consequences

- Rustdoc for `narvo_ecs::Query` links into hecs' documentation. That is the
  seam being honest about itself, and it is why the trait's own documentation
  states what does and does not cross it.
- **`cargo doc` cannot render this crate on rustc 1.97.1.** hecs yields no entity
  alongside query items — its `QueryIter<'q, Q>::Item` is `Q::Item<'q>` — so the
  only way to satisfy "iteration is over `(EntityId, Item)`" is to fetch the
  entity as part of the query, which makes the guard hold
  `hecs::QueryBorrow<'w, (hecs::Entity, Q)>`. Rustdoc's synthetic auto-trait pass
  crashes on exactly that shape:
  `internal compiler error: librustdoc/clean/auto_trait.rs:196: unexpected region
  kind` from hecs' HRTB `unsafe impl … Send where for<'a> Q::Item<'a>: Send`.
  Reproducible in fifteen lines with hecs and no Narvo code; the same struct
  parameterised over a bare `Q` documents fine. It is an upstream rustdoc bug on
  valid code, and it is recorded rather than worked around: the two workarounds
  are a hand-written `unsafe impl Send`, which the workspace's `unsafe_code =
  "deny"` forbids, and dropping the guard type, which the facade needs. Nothing
  in the verification set or in CI runs `cargo doc`, `cargo test --doc` is
  unaffected (doctest collection does not run that pass), and `publish = false`
  means there is no docs.rs build. Revisit on the next toolchain bump.
- Query iteration order is hecs' archetype order and is not stable. The facade
  does not sort it — sorting every query would cost the traversal its point.
  Instead everything observable goes through `World::entity_ids`, which is
  sorted by `EntityId`, and `ComponentRegistry::iter`, which is sorted by stable
  component name.
- Mutable queries (`&mut T`) are reachable through a shared `&World`, because
  hecs checks those borrows dynamically. Splitting read-only from writing
  queries statically would need hecs' `QueryShared` as a second supertrait, and
  a second seam is not worth buying that: `World::get_mut` already requires
  `&mut World`, so the read-only discipline of the render path has a compiler
  guarantee where it is cheap and a documented convention where it is not.
- A conflicting borrow panics at iteration rather than failing to compile. The
  facade does not convert that into an error type: it is a programming mistake
  in system code, not a condition a caller can handle.

## Revision condition

Reopen if the seam stops being singular — if a later milestone needs a second
hecs trait in a public signature (`QueryShared` for a statically read-only query
API is the plausible one), or if hecs' `Query` interface changes in a way the
blanket implementation cannot absorb. Replacing the storage engine outright is
ADR-0002's revision condition, not this one's.
