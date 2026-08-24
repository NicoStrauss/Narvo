# ADR-0019: A recording names the scene it was made against

Status: accepted · Date: 2026-08 · Scope: `narvo-app` (the recording format, the
`scene-file` mode and the replay path), and the determinism suite case built on
them

`ProjektPlan.md` §6/M4 named this as an ADR candidate in one sentence — *"eine
geladene Szene konstituiert den Anfangszustand; eine Aufzeichnung braucht deshalb
einen Szenen-Identitätsanker (Pfad + Content-Hash, Replay verweigert bei
Mismatch)"* — and left the decision to the replay task. This is that decision.

## Context

Until M4.3 every initial state in this repository was written in Rust. M4.1 built
a scene format and M4.2 a validator for it, and **nothing loaded a scene**: the
format existed, the loader existed, and no runner called either. A recording
(ADR-0012) therefore carried everything that determined its run — mode, seed,
tick count — because a mode *is* its initial state.

A scene-file run breaks that. Its initial state lives in a file that anybody can
edit between the recording and the replay, so mode-seed-ticks no longer determine
the run. Something has to say **which file, in which state**.

## Decision 1 — the anchor is a path and a SHA-256 of the file's bytes

A recording of a `scene-file` run carries two header fields:

```
scene crates/narvo-app/scenes/determinism-case.ron
scene-sha256 18337bd72b4d4da33b1c8f1ee3f1cadff2b5f87e692ca5a1ae1e19ad87a4dede
```

The digest is over the file's **bytes**, not over anything derived from them.

### Rejected: the canonical dump's hash of the loaded world

The obvious alternative, and the one that looks cheaper because the machinery
exists: load the scene, take `state_hash(canonical_dump(...))` at tick 0, store
that.

**Its best argument is real.** It measures what actually matters — the *world* the
recording started from — so it is immune to changes that do not change the world:
a reordered `components` map, a re-indented body, a corrected comment. Under a
byte hash all of those refuse a replay that would in fact have been faithful. It
also needs no new code at all.

**Against it, and this is what decides:**

1. **It answers a question that is already answered, and not the one being
   asked.** The state hash compares two runs from tick 0 onward. A dump hash at
   tick 0 is that same instrument pointed one tick earlier — so it inherits
   ADR-0008's stability domain, which explicitly does *not* hold across a `ron`
   or `serde` bump. A recording is an artifact that outlives the build that made
   it; anchoring it to a value ADR-0008 says may move for reasons unrelated to
   correctness would make old recordings fail for no reason. **ADR-0008 forbids
   committing a state hash to this repository, and a recording file is a
   committed artifact in every sense that matters.** A byte hash of a file has no
   such domain: SHA-256 of a byte sequence is the same value forever.
2. **It cannot be checked before the file is loaded**, and the load is the
   expensive, failure-prone step. The byte hash is checked on the bytes as they
   are read.
3. **It gives a worse message.** "The world at tick 0 hashes differently" sends a
   reader looking for a simulation bug. "This file is not the file the recording
   was made against" sends them to `git diff`.

The cost is admitted and named in Consequences: a change that does not change the
world still refuses the replay. That is the safe direction of a wrong answer.

## Decision 2 — the path is stored relative, with forward slashes

Always, on every platform. `crates/narvo-app/scenes/case.ron`, never
`crates\narvo-app\scenes\case.ron` and never `D:\Narvo\...`.

**This is not tidiness; it is what keeps the determinism suite meaningful.** That
suite compares recordings **byte for byte between Windows and Linux** (§7.3), and
the new case records. A path written verbatim would differ in its separators
between the two platforms and the comparison would fail — for a reason with
nothing to do with the simulation, which is the exact failure mode ADR-0008 calls
the worst one this project has.

An absolute path is **refused** at record time rather than converted. Converting
would need a base to be relative to, and the only honest candidate is the working
directory the run happened to start in — which is not a property of the recording
and would travel wrong to another machine. On replay the stored path is resolved
against the current working directory, so a recording is replayed from the
directory it was made in; the missing-file message says so.

## Decision 3 — the format grows additively, and `FORMAT_VERSION` stays 1

**A finding, and it contradicts a sentence in ADR-0012.** That ADR's consequences
say: *"Changing the format means bumping `FORMAT_VERSION`, and every recording in
existence stops replaying — which is correct."* That is true of a change to what
an existing field *means*. It is not true of a field that is new and optional,
and the difference is worth having in writing because the sentence would
otherwise have forced a version bump that broke every recording for no gain.

What was actually done, and why it breaks nothing:

- The two scene fields are **written only when there is one**, so a recording of
  a mode-based run is byte for byte what it was before this task. The seventeen
  determinism cases that existed are unchanged, and
  `a_mode_based_recording_renders_exactly_as_it_did_before` pins that against a
  literal.
- A recording written before this task **parses unchanged**, because the fields
  are optional on the way in — `a_recording_from_before_the_anchor_still_parses`.
- An **older build** reading a *newer* recording fails with
  `UnknownHeaderField { field: "scene" }`. That is the correct direction: a build
  that cannot honour a scene anchor must not replay a recording that has one, and
  it says which field it did not understand.

A version bump would additionally have changed the first line of every recording,
including the two the determinism suite already compares — turning a compatible
change into a false regression.

**The rule this leaves behind:** a *new optional* field is additive and does not
bump; a change to an existing field's meaning or a new *required* field does.
ADR-0012's consequence is refined to that, not overturned.

## Decision 4 — the division of labour, and why neither half subsumes the other

| | the anchor | the state hash (ADR-0008) |
|---|---|---|
| asks | is this the same **file**? | did two runs compute the same **thing**? |
| when | before tick 0 | from tick 0 onward |
| over | the scene file's bytes | the canonical dump |
| stable across dependency bumps | yes | explicitly not |

**The anchor cannot replace the state hash**: two runs from an identical file can
still diverge, and finding that out is the whole of the determinism suite.

**The state hash cannot replace the anchor**, and the demonstration is in the M4.3
report: adding a *comment* to a scene changes nothing about the world it
describes, so a tick-0 dump comparison would pass — while every later tick is
computed from a file the recording never saw. Worse, the failure would appear as
a divergence at some tick, which reads as a simulation bug. The anchor turns a
mystery into a sentence.

They also fail in different directions, on purpose. The anchor is **conservative**:
it refuses runs that would have been fine (a comment, a reformat). The state hash
is **exact**: it reports only real divergence. A conservative check before the run
and an exact one during it is the right way round — the cheap check that
over-refuses runs first.

## Decision 5 — refusal, and what it says

A mismatch and a missing file are **two cases with two messages**, and neither is
repaired automatically. There is no `--force`, no "re-anchor" flag and no
best-effort load: a replay that started from a different world would produce a
divergence nothing could explain, which is worse than not running.

Both messages name the file and say what to do. The mismatch names **both**
digests, because the first question a reader has is "did I change it, or did the
recording come from somewhere else", and two hashes plus a path answer it with
`git log`.

## The mechanism, and one property worth stating

**Hash and load come from one read.** `Anchor::read` returns the anchor *and* the
text, and the world is built from that text. Hashing a file and then opening it
again to load it would leave a window in which it changes, and the world would be
built from bytes the anchor never saw. It is a small thing and it is exactly the
kind of small thing this ADR exists to prevent.

**SHA-256 is written out** in `crates/narvo-core/src/sha256.rs` rather than taken
from `sha2`. The reasoning is on the module and is deliberately *not* ADR-0008's
stability argument, which does not transfer: SHA-256 is frozen by FIPS 180-4, so
a crate cannot legitimately change its output. What decides is cost against
checkability — `sha2` brings eight crates new to this workspace and 1.8 s of clean
build for eighty lines used once per run, and the published vectors make the
written-out version checkable in the direction ADR-0008 requires. **This is not a
security boundary**: the anchor answers "is this the same file", not "did somebody
tamper with it". A security use is the module's revision condition.

## Consequences

- **A cosmetic edit to a scene refuses every recording made against it.** A
  comment, a re-indent, a reordered `components` map: the world is identical and
  the replay is refused. That is Decision 1's admitted cost, and the remedy is
  the message's second sentence — record again. It also means a *frozen* scene is
  what a determinism case needs, which is why the case scene is a copy of the
  M4.1 specimen rather than the specimen itself: that file is *meant* to grow, an
  M4.1 test asserts it carries every registered component.
- **The scene path is resolved from the working directory**, so a recording is
  replayed from where it was made. For the suite that is the workspace root on
  both platforms. A recording moved elsewhere gets the missing-file message,
  which says this.
- **`Mode::SceneFile` is the first mode `sim::build` cannot build.** Its world
  comes from a file, so `headless::run` dispatches on the loaded scene before it
  consults the mode, and `sim::build`'s arm for it is unreachable by construction.
  The mode loops in the runner's tests now iterate a `CODE_BUILT` list instead of
  `Mode::ALL`, and the coverage that gave up is given back by four tests that
  hand the mode a scene.
- **One list of engine component types, in `narvo-ecs`.** The validation CLI and
  the scene-file mode both need exactly the set a scene may carry, and two
  hand-kept copies is one too many. `register_engine_components` is the engine
  making a statement about its own components — **not** the thing ADR-0018
  forbids, which is a type list inside the *format* crate. A caller may still
  register these, some of them, or none, plus its own; `narvo-app`'s demo
  simulations are untouched.
- **The suite grew from seventeen cases to eighteen**, and the new one records.
  The recording is where a path-normal-form mistake would surface, in the
  cross-platform comparison job.
- **The window runner cannot show a scene file.** Nothing in this task touches
  `window.rs`; `scene-file` is a headless mode. Wiring it to the window is a
  later task's business and needs no decision here.

## Revision condition

Reopen when hot reload lands, because a file that changes *while the process
runs* is the case this ADR does not cover — the anchor is checked once, before
tick 0, and a reload replaces the world underneath a run that may be recording.
**Answered by the amendment below; the other two stand.** Reopen when a scene
gains prefabs that pull in *other* files, because the anchor then names one file
and the initial state depends on several. And reopen if a scene ever needs to be
replayed from somewhere other than where it was recorded, which would turn
Decision 2's relative path into a search path question.

---

## Amendment (M4.6): one band, one anchor — the reload cuts instead

Answers revision condition 1. Hot reload landed in M4.6 (ADR-0022); the anchor is
**unchanged** and this records why, because "we looked and nothing had to move" is
a result and has to be written down like one.

**The condition feared a recording whose initial state changes mid-band.** It
cannot happen, and it cannot happen for two independent reasons.

The first is a rule. ADR-0022's cut rule says a reload **finalises** a running
recording at the tick it happens and does not resume. So a band always describes
one world from one file: the file the anchor names, in the state the anchor
hashes. What comes after the cut is a different run, and if it is recorded it is
a different recording with its own anchor. The anchor keeps meaning exactly what
Decision 1 gave it — *this recording started from these bytes* — with no clause
added for reload.

The second is that today the case does not arise at all. Reload is window-only
(`Watcher::new` has one call site, in `window.rs`) and recording is headless-only,
and a replay never watches. **No site exists at which a recording and a watcher
coexist**, which is why the cut rule is decided and not built — ADR-0022,
Decision 6, with the revision condition that creates its site.

**Rejected: re-anchoring the running recording on reload.** Its best argument:
the band would keep going and stay replayable, which is what a recorder is for.
Against it: a recording would then carry two initial states and a tick at which
one became the other, and a replay would need both files — the first of which the
reload just overwrote. The anchor's whole value is that one recording answers one
question with one hash.

**Rejected: refusing to reload while recording.** Its best argument: it makes the
invariant structural rather than procedural. Against it: it puts the recorder in
charge of the editing loop, and silently ignoring an edit is the failure mode
hot reload exists to remove. Cutting says what happened; refusing hides it.

**What did move, and it is small:** the *window* opens a scene file directly
rather than through `Anchor::read`, because that function refuses an absolute
path (Decision 2) — a rule that exists so a *recording* travels, and that has no
business constraining how a window is pointed at a file. The anchor type, its
normal form and its checks are untouched, and the recording path still goes
through it.

**Unchanged and re-checked:** the eighteen determinism cases are byte-identical
before and after M4.6, `FORMAT_VERSION` is still 1, and no header field was added.
