# ADR-0041: The seam between a world and the renderer is a crate

Status: accepted · Date: 2026-08-15 · Scope: `narvo-view2d`, `narvo-app`, and
every future consumer that draws simulation state

## Context

ADR-0015 put the extraction from a `World` in `narvo-app`, and named the
condition under which that would stop working:

> **`narvo-app` is a binary crate with no library target**, so `placements_of`
> is reachable from unit tests inside `src/` and from nothing else. That is
> adequate today and will not stay so: **the moment a second consumer needs the
> extraction, either that crate grows a `[lib]` or the function moves.** Named
> here so the move is a decision rather than a surprise.

M6b.0 built that second consumer — an external probe against the public surface —
and it reached six of seven goals. The seventh stopped at the render edge: the
probe rewrote `hit_test` and `depth_order` by hand and could not reach
`placements_of` at all.

M6b.1's survey (`target/reports/M6b.1-erhebung.md`) measured four candidate
homes. Two findings shaped this decision more than the package counts did:

- **It is seven functions in three dependency groups, not four in one.** Group
  **A** — `placements_of`, `regions_of`, `camera_of`, `region_names_of` — needs
  the ECS *and* the renderer. Group **B** — `hit_test`, `depth_order` — needs
  only the ECS. Group **C** — `count_actions` — needs the ECS and
  `narvo-input`.
- **`depth_order` is the clamp.** Group A calls it four times, group B contains
  it, and `hit.rs`'s own module documentation calls it *"One copy, two
  consumers"* — the single copy is what stops a click landing on the sprite that
  is not in front, for depths of `-0.0` only.

## Decision

**A new crate, `narvo-view2d`, carries groups A and B together. `narvo-app`
becomes its consumer, and takes it as an optional dependency behind `render`.**

Concretely, as built in M6b.1:

- `narvo-view2d` owns `placements_of`, `regions_of`, `camera_of`,
  `region_names_of`, `hit_test`, `depth_order`, the types `Drawn` and
  `DrawnRegion`, and the four private helpers that serve them. Its two edges are
  `narvo-ecs` and `narvo-render2d`, and it has no features.
- `narvo-app` keeps `count_actions` `pub(crate)`, keeps `assets.rs`, and keeps
  the golden tests for the two scenes it blesses, in a new `blessed_scenes`
  module with no production code.

> **Amended by M6b.2a — two sentences above are superseded and left standing.**
> This crate has **three** edges now, not two, and `narvo-app` keeps only
> `assets.rs`'s *policy* half. The wording is deliberately unedited: a Decision
> section records what was decided on its date, and the amendment at the end of
> this file carries the correction, including the argument the third edge was
> required to give.
- `narvo-render2d` is untouched. It still does not depend on `narvo-ecs`, and
  the two crates still meet at `SpritePlacement`.

## Why one crate and not two

The dependency graph suggests a cut: group A needs the renderer, group B does
not, and a consumer wanting only a hit test would rather not carry wgpu. The cut
was rejected because it runs straight through `depth_order`, and both ways of
making the cut cost more than the carried weight:

- **A copy on each side** re-creates exactly the defect M5.4 removed. Two
  normalisations of `-0.0` that are equal today and drift tomorrow is the kind of
  fault that survives every test somebody thinks to write, because it is visible
  only when a depth is written `-0.0` *and* two rectangles overlap.
- **A dependency between the halves** means the hit-test crate becomes a
  dependency of the extraction crate, which is one more crate in the workspace
  and one more edge for the same single function — the thing the split was
  supposed to avoid.

**The price is named rather than hidden.** `narvo-view2d` depends on
`narvo-render2d` with default features, so **a consumer that wants only
`hit_test` pays for the whole graphics stack**: measured, an external consumer of
this crate resolves 147 packages besides itself, where `narvo-ecs` alone
resolves 16.

### Revision condition for the split

Reopen when a third consumer appears that wants group B without group A — a
click consumer that does not draw. That consumer makes the cost a number rather
than an aesthetic judgement, and the two-crate shape can then be decided against
it. Until one exists, the single copy of `depth_order` outranks a saving nobody
has asked for.

## Why group C stays behind

`count_actions` counts input events into a `Tally`. Moving it would put
`narvo-input` under every consumer of the renderer, to serve a system that is
**the demo's, not the seam's**: a game that counts clicks writes its own tally,
and the shape of that counter is a content decision rather than an engine one.
`narvo-ecs` owns `Tally` because it is state; the system that drives it stays in
the crate that already sees both `narvo-ecs` and `narvo-input`, which is the
split M5.4 made for `HitRect` and `hit_test` and the layering ADR-0025 fixed.

It stays `pub(crate)`. That has one consequence worth stating, because it decided
where a test file lives: `counter_world_at_three` drives the click counter
through `count_actions`, so the golden test that uses it **cannot** be an
integration test under `tests/` — nothing there can reach a `pub(crate)` item,
and `narvo-app` has no library target to reach it through. That is why
`blessed_scenes` is a `src/` module rather than a file beside the reference
images it checks.

## Why `assets.rs` stays behind, knowingly

`crates/narvo-app/src/assets.rs` is the workspace's *other* place that sees a
world and the renderer at once: it reads `narvo_ecs::World` and
`narvo_render2d::{Pixels, TextureRegion}`, and its own header already appeals to
*"the same division of labour ADR-0015 gives `placements_of` and `filter_of`"*.

It did not move, and that is a decision rather than an oversight. It is the atlas
side of the seam — packing, region tables, load-time validation — and it belongs
with whatever M6b.2 decides about the asset boundary. Moving it now would mean
deciding that question inside a task scoped to this one. What did move is
`region_names_of`, the half of it that reads a world and nothing else;
`assets.rs` calls it across the new boundary.

**Named here so the second place is a known cost rather than a discovery.**

## The candidates that were rejected, with their best arguments

Each is stated at its strongest, with the condition that would reopen it.

### (a) `narvo-app` grows a `[lib]`

**Its best argument: it moves nothing.** No type migrates, no crate is created,
no edge changes, `Cargo.lock` does not move, and the prediction that no blessed
reference shifts is true by construction rather than by measurement. ADR-0015
names it as one of its own two answers, so taking it would have been the smaller
departure.

It was rejected on three measurements and one sentence:

- **327 packages against 147.** A consumer wanting the extraction would take the
  whole runner — the CLI, the recording format, the agent transport.
- **Verification step 4 breaks.** In the shared-path form the survey measured,
  `cargo clippy -p narvo-app --lib -- -D warnings` failed with 141 errors, every
  one of them a `dead_code` warning about an item only `main` reaches. (The
  survey named this measurement's limit: a purpose-written `lib.rs` would not
  have those, and would instead have to carry `load_recording` and `scene_for`,
  which `crate::ipc` reaches through crate paths.)
- **Opening `sim` opens six submodules.** `sim.rs` already declares `chance`,
  `input`, `motion`, `physics`, `scene` and `scene_file` as `pub mod`, so making
  `sim` reachable makes all six reachable.
- And the sentence, from `main.rs`'s own module documentation: *"It exposes no
  library API — nothing links against it — which keeps composition-root decisions
  out of `narvo-core`, `narvo-render2d` and `narvo-assets`."*

**Reopen** if a second consumer ever needs something from `narvo-app` that is
*not* the render edge — a recording reader, the CLI parser. At that point the
question is no longer about this seam and the answer may well be different.

### (c) the extraction moves into `narvo-render2d`

**Its best argument is ADR-0015's own, and it is real:** every renderer that
ships in a real engine reads the world, and the indirection chosen here copies
data every frame. At 50 000 sprites that is 50 000 `SpritePlacement` values built,
moved and dropped once per frame. A renderer that iterated the world in place
would not pay it.

ADR-0015 made that argument's revival conditional on a number — whether the copy
*dominates* the cost or merely contributes — and the number says no:

- the copy is **14.6 µs of 1 025 µs** at 50 000 sprites (M3.7);
- the extraction phase is **3.76 ms of a 6.74 ms frame** (M3.32), and the reason
  named in the same paragraph is not the copy but that `camera_of` and
  `placements_of` each call `World::entity_ids`, which allocates and sorts a
  vector of every live id — **twice per frame**.

So (c) falls on a measurement rather than on a quotation. It would also put
`World` and nine component types into `narvo-render2d`'s public API, which
ADR-0015's consequences forbid in as many words.

**Reopen** when the double enumeration has been removed and the copy measured
again. If the copy is then the dominant term, (c) returns with the burden of
proof on its side and needs a new ADR superseding ADR-0015.

### (d) the extraction moves into `narvo-scene`

**Its best argument: one call from a file to a drawable.** `narvo-scene` already
turns text into a world, `Sprite.region` is a field of the scene format, and a
consumer today has to bring two crates together to get from a `.ron` file to a
picture. (d) is the only candidate where that is one crate's job, and it creates
no new workspace member.

It was rejected because **it breaks a check that actually runs**. `narvo-scene`
is an ungated normal dependency of `narvo-app`, so an edge from it to
`narvo-render2d` puts wgpu, naga, `raw-window-handle` and image into the
headless tree, and verification step 9 says so. Making the edge optional would
give `narvo-scene` a second configuration that nothing builds — the M5.6b shape,
where an optional dependency no feature activates is absent from cargo-deny's
graph and the green answers a question nobody asked. It would also cost a scene
consumer 18 packages → 148.

**Reopen** if `narvo-scene` ever acquires a feature gate for an independent
reason, since the second configuration would then already be paid for.

## Consequences

- **A consumer of the render edge takes 147 packages besides itself**, measured
  against `narvo-ecs`'s 16 and `narvo-app`'s 327. The seam is reachable from
  outside for the first time.
- **`narvo-view2d` has no features, deliberately.** Everything it names from
  `narvo-render2d` is behind that crate's `gpu` feature, so a gate here would
  mirror `gpu` exactly and nothing would ever switch it off.
- **`narvo-app` takes it optionally, behind `render`.** That is not the
  arrangement `narvo-audio`, `narvo-physics2d` and `narvo-ipc` sit in — those
  are normal dependencies precisely so the headless steps compile and test them.
  The difference is that this crate carries a device stack, and the M4.8 shape
  those three avoid does not apply here: every test that moved runs in step 2.
- **The headless configuration loses nine tests it used to run, and only nine.**
  `hit` was gated `any(feature = "render", test)`, so `cargo nextest run -p
  narvo-app --no-default-features` compiled it and ran its nine tests;
  `sprite_batch` was gated on `render` alone and never ran there at all, so its
  thirty-four travellers cost the headless configuration nothing. Step 8 goes
  from 363 tests to 354. The nine did not disappear — they run in step 2 as part
  of `narvo-view2d`, where all 1 154 workspace tests pass on both platforms —
  but they no longer run in the *headless* configuration.

  Named rather than repaired, and the repair was considered: a feature on
  `narvo-view2d` that gates the renderer half would let `narvo-app` take the
  crate headlessly and keep the nine. It is the shape this ADR rejects two
  paragraphs up — a second configuration nothing in the workspace would build —
  and the nine tests are pure `narvo-ecs` logic with no platform surface, so
  what the headless run would be re-proving is that a sort is a sort.
- **No guard was added to `FORBIDDEN_IN_HEADLESS`.** Measured rather than
  assumed: making `narvo-view2d` a normal dependency of `narvo-app` fails step
  9 with `found device crates in the headless tree: wgpu, naga,
  raw-window-handle, image` and `the render feature gate has been bypassed`. The
  list already catches this boundary through the crates it drags in, and adding
  an entry for exactly the new case is the shape that lets the next one through.
  What an entry *would* catch is a future in which `narvo-render2d` is reachable
  without those four — which cannot happen while everything this crate names from
  it is `gpu`-gated.
- **The module name `sprite_batch` did not change**, though the file moved
  crates. Thirty-three references across eighteen files name it, six of them in
  ADRs and in `docs/history/`, and those are records rather than documents to be
  rewritten.

## Revision condition

Reopen when a third consumer wants group B without group A (above), or when
`assets.rs` is decided in M6b.2 — that decision may pull the atlas half of the
seam into this crate, or may put both halves somewhere else again.

Reopen also if `narvo-view2d` ever acquires a third edge. Two edges are what
make it a seam; a third would make it a layer, and a layer wants a different
argument than this one.

## Amendment, 2026-08-15 (M6b.2a): the atlas half moves in, the convention does not

**Both halves of the revision condition above occurred at once**, and they are
quoted here rather than summarised because the second one is the awkward half:

> Reopen when a third consumer wants group B without group A, or **when
> `assets.rs` is decided in M6b.2** — that decision may pull the atlas half of
> the seam into this crate […]
>
> Reopen also **if `narvo-view2d` ever acquires a third edge.**

`assets.rs` was decided in M6b.2, the atlas half was pulled in, and the edge to
`narvo-assets` is the third.

### The trigger is not the move — it is that a diagnosis could not be reached

M6b.2's survey drove an external consumer, outside the tracked tree, from its own
PNG files to a rendered image. **The chain held.** Every part of `load_for`'s body
is public, and the consumer rebuilt all thirty lines of it from those parts
without a wall.

What it could not get was `AssetsError::UnknownRegion` — the sentence naming the
region a scene asked for and the ones that exist. A binary crate has no lib
target to hand it out from.

**What stands in its place was measured rather than reasoned**, because the first
draft of this amendment asserted it and the assertion was one step ahead of the
evidence: M6b.2's probe wrote its own resolution check, so it never met the
failure it was describing. M6b.2a's probe met it on purpose. A consumer that
skips the check and indexes the region table gets

```
thread 'main' panicked at src\main.rs:194:22:
no entry found for key
```

— no region name, no directory, and no list of what does exist. Beside it, what
the same consumer now gets:

```
a sprite asks for the region "villain", which …\game\assets does not carry; the
known ones are "coin", "hero". A region is named by a file stem, so this needs a
file called "villain.png" there
```

CLAUDE.md's *"error messages are agent feedback"* has a consumer it was not
reaching, and that is the whole reason this moved. **The mechanism came along because the message cannot
travel without it**, not because the mechanism was stuck.

The section *"Why `assets.rs` stays behind, knowingly"* above is therefore
superseded in its conclusion and correct in its reasoning: it said the file
*"belongs with whatever M6b.2 decides about the asset boundary"*, and this is that
decision.

### What moved, and what deliberately did not

| | |
|---|---|
| moved to `narvo-view2d` | `SceneAtlas`, `AssetsError` with its `Display`, `Error` and `From` impls, the body of `load_for`, and the four tests that exercise it |
| stayed in `narvo-app` | `ASSETS_DIRECTORY` and `directory_for` — *which* directory is this runner's convention, and a game may choose its own |

The cut was measured before it was made rather than assumed: `ASSETS_DIRECTORY`
is named in exactly two places in the workspace, its own definition and
`directory_for`, and neither the body of `load_for`, nor `SceneAtlas`, nor any
variant of `AssetsError` mentions it. `UnknownRegion.directory` is the argument
the caller passed, echoed back into the message.

**One test did not move, and the plan's figure of five was one too many.**
`the_assets_directory_sits_beside_the_scene` tests `directory_for`, which stays;
a test cannot travel away from its subject. Four moved.

### Is it still a seam, with three edges?

The condition above says a third edge would make this a layer. **It is a third
edge and it is still a seam**, and the argument is the one the condition asks for
rather than a wave at it.

`load_for` does what `placements_of` does: it takes something that is not in the
renderer's vocabulary and produces something that is. `placements_of` takes
components and yields `SpritePlacement`; `load_for` takes a packed `Atlas` and
yields `Pixels` and `TextureRegion`. Both directions end at the renderer, and
neither crate on the far side learns anything — `narvo-assets` still knows
nothing about `TextureRegion`, exactly as `narvo-ecs` still knows nothing about
`SpritePlacement`.

What would make it a layer is an edge whose traffic does **not** end at the
renderer. That has not happened, and it is the thing to watch for.

The price is smaller than the count suggests, measured rather than argued: of the
eleven packages in `narvo-assets`' normal tree, **ten were already here** —
`narvo-render2d` carries `image`, which pulls `png` and its deflate stack. The
crate's normal tree goes from **98 packages to 99**, and the one is
`narvo-assets` itself. No name from `FORBIDDEN_IN_HEADLESS` appears anywhere
under it, and it cannot reach the headless tree in any case: `narvo-app` takes
this crate only behind `render`, and already depended on `narvo-assets` directly.

### Two consequences worth stating plainly

**A dev-dependency on `png` came with the tests.** It adds no package — png 0.18.1
was already in `Cargo.lock` and already under this crate through `image` — but it
is a new edge in this manifest, and the alternative was to leave the four tests in
`narvo-app`, which would have left this crate's own public surface uncovered by
`cargo nextest run -p narvo-view2d`. The manifest says so at the declaration.

**`narvo-app` now has no production use of `narvo-assets` left.** Its four
remaining uses are `frame.rs`'s blessed-scene fixture under `#[cfg(test)]` and two
integration tests. The edge stays a normal dependency and is reported rather than
changed: demoting it would move `narvo-assets` out of the headless tree and
change what verification step nine prints, which is a separate decision from this
one.
