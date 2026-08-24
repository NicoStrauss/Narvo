# ADR-0008: What the state hash guarantees, and what it does not

Status: accepted · Date: 2026-08 · Scope: narvo-ecs (canonical dump and state
hash), and every determinism test built on them

Written before the code it governs. The hash is a promise about behaviour, and
which promise it is has to be decided rather than discovered from whatever the
implementation happens to do.

## Context

M2 turns on a single question: do two runs of the same simulation agree? The
instrument for answering it is a canonical serialization of the world and a
64-bit hash over that text. An instrument needs a stated range before its
readings mean anything — otherwise a red test is as likely to mean "the
dependency tree moved" as "the simulation diverged", and an agent cannot tell
those apart from the failure alone.

The hash runs over RON (ADR-0006), so it depends on RON's formatting and on
serde's field order. Both are stable for a given dependency set and neither is
promised across versions.

## Decision

The stability domain is exactly this:

| Promise | Status |
|---|---|
| Two runs of the same binary, same seed and input | **hard guarantee**, tested |
| Windows ↔ Linux, same toolchain, same `Cargo.lock` | **hard guarantee** — the subject of the kill criterion in `ProjektPlan.md` §6, M2 |
| Across dependency or toolchain bumps | **explicitly no guarantee** |

The third row has a consequence that binds the code, not just the prose:

> **No hash literal is ever committed to this repository.** Determinism tests
> compare two runs against each other. They never compare against a checked-in
> value — not in a test, not in documentation, not in a golden file.

A committed hash would be broken by a `ron` release that changes a separator,
by a serde release that changes how a struct is emitted, by a compiler that
formats a float differently. Every one of those turns the suite red while the
simulation is perfectly correct. That is the worst failure mode this project
has: a red test that says nothing about correctness costs an agent a full
diagnostic cycle and teaches it to distrust the suite. A two-run comparison
cannot fail that way — both sides move together — and it fails exactly when the
simulation actually diverges.

The second row is a guarantee we assert and verify, not one we assume. If it
ever fails, the answer is not to weaken the test: §6/M2 pre-registers the
response, which is a new ADR reducing the goal to per-platform determinism with
cross-platform as best effort. That decision is a human's.

## The hash is a comparison tool, not an identity tool

It answers "did these two runs diverge". It does not answer "is this state
correct", and it cannot: any 64-bit summary of an arbitrarily large state is a
lossy one, and nothing anywhere records what the right answer would have been.
Correctness is what ordinary assertions are for.

That is why the canonical dump is public alongside the hash, and why it is the
more important of the two. An agent that sees two hashes disagree needs a `diff`
of the dumps next, not a second hexadecimal number. The hash exists so that
"did anything change" is one cheap comparison instead of a string compare over
megabytes; the moment the answer is "yes", the dump takes over.

## Which hash

A 64-bit FNV-1a, implemented in `narvo-ecs` itself.

`std::hash::DefaultHasher` is disqualified outright: its own documentation says
the internal algorithm is unspecified and may change between releases, which
would silently move every hash on a toolchain bump — the exact failure this ADR
exists to bound.

The algorithm is written out in this workspace rather than pulled from a crate,
and that is the point: stability comes from the constants being frozen in our
own source, where changing them is a visible diff in a file with this ADR's
name in its comments. A dependency would put that guarantee one version bump
away, and it would put an argument into `cargo deny`'s scope for the sake of
about six lines of arithmetic.

What is required of it: deterministic, non-cryptographic, and sensitive enough
that a one-byte difference in the dump changes the output. A test asserts the
sensitivity directly, because a hash function that returned a constant would
pass a two-run comparison perfectly. It is also exactly the published FNV-1a,
and tests pin it against published vectors — which is a different kind of
literal from the one this ADR forbids, and the next section is where the line
runs.

## Which literals this rule forbids, and which it asks for

*Amended 2026-08-07 (M2.4a). This paragraph previously ended "so no test pins a
magic constant, in keeping with the rule above". That read as a blanket ban on
literals and was contradicted by the code from M2.2b onwards. The rule was never
that broad — the distinction below is what it always meant — but it was written
in a way that could only be read the wrong way, and a rule that has to be
reinterpreted is a rule that will be. The decision itself is unchanged and this
ADR stays accepted.*

**Forbidden: the hash of a state.** Such a value is produced by this repository
and would be checked against itself. It moves when `ron` changes a separator,
when serde changes how a struct is emitted, when a compiler formats a float
differently — every one of those turning the suite red while the simulation is
perfectly correct. That is the failure mode the whole ADR exists to bound, and
it is why determinism tests compare two runs against each other.

**Required: a test vector of an algorithm.** Such a value is produced by a
published specification and this repository is checked against it. It depends on
no dependency, no toolchain and no serialization format; the only thing that can
move it is an edit to the algorithm — which is exactly what it is there to
catch. Without one, a transposed digit in a constant would still be
deterministic, still pass every two-run comparison in the workspace, and still
be wrong, because a two-run comparison compares two runs of the same wrong code.
Only a value from outside notices.

The second kind lives at, all in `crates/narvo-ecs/src` until M3.34 added the
last entry below:

- `state.rs:360`, `the_hash_matches_the_published_fnv_1a_64_vectors` — five
  input/output pairs from the FNV reference distribution.
- `state.rs:369`, `the_constants_are_the_published_ones` — the prime and the
  offset basis against draft-eastlake-fnv-21 §5.
- `state.rs:379`, `the_hash_is_fnv_1a_rather_than_fnv_1` — the two differ only
  in the order of the xor and the multiply, and a swap would pass everything else.
- `rng.rs:194` and `rng.rs:213` — the same discipline applied to the generator
  of ADR-0010, against the PCG reference implementation's published seed and
  output.
- `crates/narvo-testkit/src/sha256.rs` — SHA-256 written out for M3.34's glyph
  atlas anchor, against the published FIPS 180-4 vectors, with both constant
  tables re-derived from the primes rather than transcribed. Added here because
  this list said "all in `crates/narvo-ecs/src`" and that stopped being true.

**M3.34 also committed a literal of the first, forbidden kind, knowingly.** The
glyph-atlas anchors in `crates/narvo-testkit/src/glyph_atlas.rs` move when
`ab_glyph` is updated, which is what the test above rules out. They are there
because D10 (`ProjektPlan.md` §11) decided them on a reading this ADR does not
cover: its subject is the *state hash*, where a dependency-driven move is noise
that says nothing about simulation correctness. For a generated asset the move
is the signal — the bytes are the specification, and nothing else in the
repository would notice them changing. The anchor's own doc carries this
argument, and whether this ADR should be amended to admit a third kind is a
question for the maintainer rather than something M3.34 decided. **It was
answered yes, and the next section is the answer.**

The test to apply to a new literal is one question: **what would have to change
for this value to move?** If the answer includes a dependency version, a serde
field order or a compiler release, it is the forbidden kind. If the only answer
is "an edit to the algorithm itself", it is the required kind.

## Admitted: a content anchor, where the movement is the signal

*Added 2026-08-10 (M3.36), applying a decision delegated to the chat and
recorded in `ProjektPlan.md` §12 before this text was written. Nothing above is
withdrawn: the forbidden kind stays forbidden and the required kind stays
required. This section adds a third that the two did not cover, and it exists
because M3.34 committed one before there was a place to write it down.*

The first two kinds are sorted by **where the value comes from** — produced by
this repository and checked against itself, or produced by a published
specification and checked against from outside. The third is sorted by something
the first two never had to ask: **what a movement of the value would mean.**

**Forbidden — the state hash. Stability is the purpose, so movement is noise.**
The value summarises simulation state, and a `ron` separator, a serde field order
or a float's formatting can move it while the simulation is perfectly correct.
The red test then says nothing about correctness, which is the failure mode this
whole ADR exists to bound.

**Admitted — a content anchor. The bytes are the specification, so movement is
the finding.** The value summarises a *generated artefact*: an atlas, a table, a
packed asset. When a dependency bump moves it, that is not noise obscuring the
question — it *is* the question, and a red test is the only way anyone learns
that the artefact the engine ships changed shape. Re-anchoring is therefore not
maintenance to be automated away but a deliberate sighting: somebody looks at the
new artefact, satisfies themselves it is right, and commits the new value as an
act.

Three conditions, all three necessary, and the third is the one that is easy to
skip:

1. **The value covers a generated artefact, not simulation state.** Otherwise it
   is the forbidden kind wearing a different name.
2. **Nothing else in the repository watches the whole of what is anchored.** If
   an ordinary assertion — a count, a padding rule, a property — would catch the
   change, write that assertion instead; it survives dependency bumps and an
   anchor does not. An anchor is for what only the bytes themselves record, and
   the wording is deliberately "the whole of it" rather than "any of it": the
   first instance below is partly covered by other instruments and is still
   worth anchoring.
3. **A way to look at the artefact exists, and it is used before anchoring.** A
   value re-blessed without looking is worse than no value at all: it converts a
   deliberate sighting into a ritual, and the next real regression is committed
   as a re-anchoring.

**The cost is accepted here rather than avoided, and saying so is the point.** A
content anchor *can* go red for a reason that is not a defect — the thing this
ADR forbids elsewhere. Two things bound it. It goes red rarely, because
`Cargo.lock` is committed and every cargo invocation in CI carries `--locked`
(`.github/workflows/ci.yml:129` and following), so a pinned version
moves only when a commit moves it — by `cargo update`, or by an ordinary manifest
edit that re-resolves, which the deliberately unlocked local build applies. And
the diagnosis is one line rather than a cycle: an anchor that moves in the same
commit as a lock change has one obvious candidate cause, and the artefact is
there to be looked at. A toolchain bump is a second lever and a less obvious one
— `CLAUDE.md` triggers the release-determinism workflow on `rust-toolchain.toml`
"because a compiler bump can move codegen" — and no anchor has yet been through
one, so that half is **untested** rather than established.

**First instance: M3.34's glyph-atlas anchors** in
`crates/narvo-testkit/src/glyph_atlas.rs:568-577`, two SHA-256 values covering
each atlas's pixels *and* its glyph table — advances, bearings and regions —
in one blob (`:247`), so that a changed advance and a changed pixel are both
visible where either alone would leave half the generation unwatched. They were
committed knowingly against the text above, with the argument made in their own
doc; this section is that argument accepted as a rule rather than tolerated as an
exception. The third condition is not left to prose there either: the assertion's
own failure message says *"If `ab_glyph`, the font or the generator changed on
purpose, re-anchor deliberately: look at the atlas, then put this value in"*, and
a preview test writes both atlases under `target/` so that looking is possible.

**Rejected: delete them and watch the atlas with properties only** — count,
padding, "only the space has no outline". Its argument is real and is the reason
those properties exist: a property test never goes red for a dependency bump, so
nobody is ever tempted to re-bless without looking. What loses it is that those
properties all hold for a *wrongly rendered* glyph: a changed rasteriser moves
pixels inside regions that are still correctly counted, padded and placed.

**Other instruments do see part of it, and naming them is what keeps this
honest.** Since M3.35 the text golden renders its scene from
`glyph_atlas::rasterize` (`crates/narvo-render2d/tests/text_lines.rs:55`,
`:138-139`); a CPU-side test in the same file pins committed coverage counts and
probe texels (`the_models_numbers_are_the_predicted_ones`, `:220`); and the
advances are watched outside the anchor entirely, by
`a_monospace_font_gives_every_glyph_the_same_advance`
(`crates/narvo-testkit/src/glyph_atlas.rs:842`).

So the honest form of condition 2 is not "nothing else would notice" but
**"nothing else watches the whole of it"** — which is what an anchor is for: 95
glyphs at both sizes, pixels and table in one byte-exact value, where the golden
compares under a tolerance and skips altogether when no adapter is available
(`text_lines.rs:96-104`). Exactly which instrument would catch which change is
**not enumerated here and should not be guessed at**: three drafts of this
paragraph each got a different part of that division wrong, and two of the three
were written while correcting the draft before.

**Rejected: keep the anchors and leave this ADR silent**, on the ground that its
subject is the state hash and it therefore has nothing to say. What loses it is
the amendment of 2026-08-07 further up: this ADR is where the literal rule is
looked up, so a rule stated here in a form that does not fit the code will be
applied to the code anyway. That amendment fixed a rule phrased too broadly;
leaving a known, live exception outside the rule is the same mistake with the
pieces swapped, and one instance of the shape is enough to decide against a
second.

So the one-question test above gains a **second question, asked only when the
first says "forbidden"**: *would that movement be noise, or would it be the
finding?* Noise means the value summarises state and the first answer stands — do
not commit it. The finding means the value summarises a generated artefact whose
bytes nothing else watches whole, and then the three conditions above apply and
it may be anchored.

## Consequences

- Determinism tests are always comparisons between two things produced in the
  same build. That makes them immune to dependency drift and blind to it: a
  `ron` bump that changed the dump format would go unnoticed here. It would be
  caught by ordinary tests that assert on serialized content, which exist.
- The dump format itself is part of the observable surface. Changing it changes
  every hash, which is allowed — the third row of the table says so — but it is
  a deliberate act, not a refactor.
- Anything that is not in the registry is not in the hash. A component the hash
  cannot see makes a divergence invisible, which is why the canonical dump
  refuses to serialize a world containing an unregistered component type rather
  than skipping it quietly.
- Cross-platform equality is checked by running the binary on both platforms and
  comparing, not by storing a reference value. There is nothing to store.
- **A content anchor is not a determinism instrument and does not weaken one.**
  It watches a generated artefact, never simulation state, so nothing above
  changes: no state hash is committed, and the determinism suite still compares
  two runs of one build. What the third kind adds is a place to put the other
  question — see the section above it.

## Revision condition

Reopen if the second row ever fails — that is the kill-criterion path and needs
its own ADR — or if a state large enough to make a 64-bit summary's collision
probability interesting arrives, which is not a concern at any size M2 through
M7 will produce.
