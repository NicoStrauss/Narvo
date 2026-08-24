# ADR-0016: Shared test fixtures live in a dev-only crate

- Status: accepted
- Date: 2026-08-08
- Decision: D16 (`ProjektPlan.md` §11), delegated to the chat by the human
- Supersedes: nothing. Constrains: any future shared test building block

## Context

Test fixtures in this workspace were shared by copying. By M3.21 that had
produced, in two crates:

- **seven definitions of `quadrant_texture`**, the four-quadrant texture of M1
  and M3.5, in seven files, in two textually distinct families that emit
  identical pixels — one family using named colour constants and
  `Vec::with_capacity(size as usize * size as usize * 4)`, the other using
  literals and `(size * size * 4) as usize`, so a grep for either form finds only
  half of them;
- **ten definitions of the four-quadrant atlas across nine files** — seven at
  16 x 16 and three at 20 x 20 — counted with
  `git grep -n "fn atlas()\|fn padded_atlas()" HEAD -- '*.rs'` and then read, to
  exclude `camera_pan_steps.rs`'s column-striped atlas, which is a different
  image and is deliberately untouched by this decision. Exactly one pair of them
  was ever compared: `linear_motion.rs` renders its unpadded and padded fixtures
  and asserts the framebuffers equal, which is M3.17's padding measurement;
- **a border guard duplicated in two files**, its body byte-identical and its
  red-demonstration twin differing only in the atlas-size identifier and its line
  wrapping, both created by M3.21 itself.

Copying cost nothing while every copy agreed. M3.21 is where it began to cost:
padding two of the atlases left five others carrying a note that they were the
shape "M3.9 and M3.11 used" — true only because M3.21 corrected the tense by
hand, in five files, one at a time.
Under `Nearest` the two shapes render the same image, so nothing went red; under
`Linear`, which D13 has decided on, they will not. The divergence was invisible
to every test in the repository and was found by reading.

The third atlas is the sharpest case. `layer_order_regions_128x128`'s fixture
lives in `narvo-app`'s `src/sprite_batch.rs`, and its own comment gives the
reason it was copied: *"written out again because a test fixture has no business
in a public API and this crate cannot see that one."* That reason is sound and
this ADR does not overturn it — it answers the question the reason leaves open.

**The question is general, and this ADR answers it generally:** where does shared
test code live in this workspace, without `narvo-app` depending on
`narvo-render2d`'s test internals or the reverse?

## Decision

**A `crates/narvo-testkit` crate, a `dev-dependency` of every crate that uses
it, `publish = false`.**

It holds fixture *data* — texel grids and the geometry that indexes them. It does
not hold rules. `check_region_padding` stays a `narvo-render2d` export, because
it is the rule a later atlas packer (D3/M4) must satisfy rather than a test
helper; the testkit calls it and does not restate it.

Every fixture is offered in two shapes: one returning `Vec<u8>` and one returning
`narvo_render2d::Pixels`. That is not a convenience. `narvo-testkit` depends on
`narvo-render2d` and is a dev-dependency of it — a cycle cargo permits for
dev-dependencies — and the cycle has one consequence, **measured rather than
assumed**:

```
error[E0308]: mismatched types
  expected `offscreen::Pixels`, found `narvo_render2d::offscreen::Pixels`
```

`narvo-render2d`'s own `#[cfg(test)]` modules are a second compilation of that
crate, so a `Pixels` built by the testkit is a different type from theirs.
`Vec<u8>` is `std` and crosses unchanged. So in-crate unit tests of
`narvo-render2d` call the `_rgba` form and wrap it; every other caller —
integration tests in any crate, and every test in `narvo-app`, whose test build
links the same `narvo-render2d` the testkit does — uses the `Pixels` form. **The
pixel truth is the `_rgba` function either way**, which is what makes "one
definition" true rather than nearly true.

## Consequences

- The crate landscape grows by one (`ProjektPlan.md` §5.1). The bar §5.1 sets is
  that a crate is split off "once it justifies its own API boundary"; the
  boundary here is *dev-only, workspace-internal*, marked by `publish = false`
  and by every dependant declaring it under `[dev-dependencies]`.
- **Nothing reaches a production build**, and the phrase to be precise about is
  *production*. The repository's headless guard is `.github/workflows/ci.yml`'s
  `cargo tree -p narvo-app --no-default-features --edges normal --locked`, and
  it stays green because `--edges normal` is what it asks about. Without that
  flag the same command now prints the wgpu stack, correctly: the dev-dependency
  closes a cycle back to `narvo-render2d` with default features, so the *dev*
  graph carries `gpu`. The consequence worth naming is that the headless **test**
  job now builds the graphics stack through that dev edge, even though the
  headless binary still does not link it. No fixture enters any public API.
- **`cargo deny` must stay green**, and the new crate is path-only with a single
  workspace dependency, so it adds no third-party code, no licence and no
  advisory surface.
- One place in the workspace is forced rather than free: `narvo-render2d`'s own
  unit tests, per the type seam above. That is documented at both ends.
- The next shared test building block follows this ADR rather than re-deciding.

## Alternatives considered

**Pad the third copy and leave the structure alone.** Best argument: a
one-file change with no structural question, and D16 could have been deferred
past the whole D13 round. Against: it cements the seventh instance of exactly
the form M3.21 argued against, and the next fixture change pays the same price
again. Recorded as the rejected way in `ProjektPlan.md` §12.

**A module in `narvo-render2d` outside `#[cfg(test)]`, which `narvo-app` sees
as ordinary dependency surface.** Best argument: no new crate at all, §5.1's
landscape untouched, and `narvo-app` already depends on `narvo-render2d`, so
the wiring is free. Against: test fixtures in a production crate's public API are
a statement — they appear in its documentation, they are subject to whatever
stability that crate promises, and the M3.21 comment quoted above is the repo's
own prior objection to it. A `#[cfg(feature = "fixtures")]` gate answers the
shipping half but not the API half, and it puts a feature on the render crate
whose only purpose is tests. Rejected on the API-statement ground, not the
mechanical one.

**`include!` of a shared source file.** Best argument: no crate, no dependency
edge, no cycle, and it reaches `narvo-render2d`'s own unit tests where the
chosen design does not. Against: it is textual, so "one definition" would hold in
the source tree and not in the compiled artefacts — each includer gets its own
copy, with its own `dead_code` warnings for the parts it does not use, and the
same item existing at several coordinates makes a `file:line` citation ambiguous.
This repository's working agreement is built on `file:line` as evidence, so a
sharing mechanism that multiplies coordinates is the wrong trade here. The path
is also relative to the including file, so every consumer encodes its own
distance to the shared file and a move breaks all of them.

**A `tests/common/` module per crate.** Best argument: the ordinary Rust idiom
for sharing between integration tests, needing nothing new. Against: it shares
*within* one crate's `tests/` directory only. It cannot reach `narvo-app`, which
is the case that motivated D16, and it cannot reach `src/` unit tests either — it
would leave the third copy exactly where it is.

**A workspace-level `tests/` directory.** Best argument: `ProjektPlan.md` §5.1
already lists workspace-wide `tests/` in its target picture. Against: cargo has
no such concept — a `tests/` directory is a property of a package, so this would
be a new package under a different name, i.e. this decision with worse
signposting. Noted so the §5.1 target picture is not read as an alternative that
was passed over.
