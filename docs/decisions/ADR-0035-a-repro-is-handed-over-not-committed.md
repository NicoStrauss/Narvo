# ADR-0035: A repro is an artefact handed over, not a file in the repository

Status: accepted · Date: 2026-08 · Scope: `narvo-app`'s `--expect` runner and
its `repro` module, and every repro test an agent produces from here on

## Context

`ProjektPlan.md` §6/M6 closes on three things: an agent inspects a running scene,
changes a state, and **generates a deterministic repro test from an observation**.
The first two have been reachable since M6.3a, M6.3b and M6.4a and over MCP since
M6.5b. The third had no instrument at all — nothing in this workspace compared a
run against an expectation. The determinism suite compares two runs of one build;
`cargo xtask determinism compare` compares two platforms. Both are comparisons,
and a comparison is not an oracle for a single case.

§6/M6 says the recording format of M2.3 is the **output form** of a repro. It
does not say where the file goes, and the two readings lead to different projects:

- a repro that stays is a test set that **grows from an agent**, with everything
  that implies for who blesses it, what the cross-platform matrix does with it,
  and who maintains it;
- a repro that is handed over is a diagnosis, and a human decides whether
  anything permanent comes of it.

The question is not stylistic, because a repro needs an expected value and that
value is a state.

**ADR-0008's headline rule names a hash** — "No hash literal is ever committed to
this repository" — and the step from a hash to a *dump* is a reading rather than
a quotation, so it is made here in the open. Three things carry it. The sentence
following that rule is already general: determinism tests "never compare against
a checked-in value — not in a test, not in documentation, not in a golden file."
The ADR's own one-question test — "what would have to change for this value to
move? If the answer includes a dependency version, a serde field order or a
compiler release, it is the forbidden kind" — answers all three for a canonical
dump, since the dump is RON and the hash's instability is inherited *from* it.
And the third, admitted kind of literal it gained in M3.36 excludes a dump by its
own first condition: a content anchor covers a *generated artefact*, "not
simulation state. Otherwise it is the forbidden kind wearing a different name."

Two measurements, both taken on `da56389` before anything was built:

- `git ls-files | grep '\.rec$'` and the same for `.dump` are **both empty**. Not
  one recording and not one dump is committed; every one is written by the build
  that reads it. M6.2 reported this for `.rec`, and it still holds for both.
- A canonical dump of `--mode chance --seed 1 --ticks 500` is **2 200 bytes**, so
  size decides nothing here. What decides is what the bytes are.

## Decision

**A repro is three things, none of which is committed: a recording, a canonical
dump of the state that recording produced, and the command line that joins them.**

```
narvo --mode input --seed 1 --ticks 500 --record bug.rec --dump > expected.dump
narvo --replay bug.rec --ticks 500 --expect expected.dump
```

The agent produces the first line's two files and hands over the second. A human
decides whether anything about it becomes permanent, and that decision is not
part of any task — a blessing is a human's, in this project, always.

**What is committed is the runner, never a case.** `--expect`, the judgement in
`crates/narvo-app/src/repro.rs` and the tests around them are ordinary code and
live in the tree like anything else. Every expected state those tests use is
written by a run they start moments earlier, from the same build, which is
ADR-0008's two-run rule applied to an instrument whose whole purpose is to
compare against an expectation. What the repository holds is the *procedure*.

**The oracle is a dump and never a hash.** A hash says that two states differ and
never where, and §6/M6 names `first_difference` as the diagnosis basis of this
milestone. A repro for a defect at tick 500 that answers with sixteen hex digits
has discarded the entity and the component before the reader sees them.

**The tick is `--ticks` and nothing else.** A run of *n* ticks is exactly the
prefix of a longer one, so "the state at tick 500" is "the state of a run that
ended at tick 500" — the idiom `cli::USAGE` already documents for bisecting. That
is what lets a repro for a defect at tick 500 go red **at tick 500** instead of at
the end of a longer run.

## Rejected: the repro lands permanently in the repository

The strongest of the three, and it is worth stating at full strength. A repro
that is not kept is a diagnosis that has to be rediscovered; a defect fixed
without a test in the tree is one that comes back; and "the recording format is
the output form" reads like something meant to persist. Nothing about that
argument is weak.

What loses it is that it requires an **ADR-0008-superseding ADR** and gets
nothing for the price. The expected state is simulation state, so committing it
is the forbidden kind exactly; and the property being given up is the one that
makes this repository's determinism instruments trustworthy — a `ron` separator
or a serde field order would turn committed repros red while every simulation in
them was perfectly correct. That is the failure ADR-0008 exists to bound, and
paying it to keep a per-incident file is a bad trade.

Two further costs, neither of which had an owner: a test set growing from an
agent needs a **blessing step**, and this project has none that a task may
perform; and the cross-platform matrix would have to decide whether to carry
agent-written cases, which is a change to §7.3's table that nobody asked for.

## Rejected: express a repro in a form the repository already commits

A case in `xtask`'s determinism matrix, or a test in
`crates/narvo-app/tests/determinism.rs`. Its argument is real: no new kind of
artefact at all, and both places already drive the binary.

What loses it is that neither is an oracle. The xtask matrix compares two
directories produced from one commit and holds **no expectation whatsoever** —
giving it one would make it a different instrument answering a different
question. And the in-process suite would have to carry the expected state in
source, which is ADR-0008's forbidden literal reached by a longer road. That
suite also cannot host `scene-file` at all, because its loop passes only
`--mode/--seed/--ticks` (§7.3, measured in M5b.3b), and a scene run is precisely
where a repro is most likely to be wanted.

## Consequences

- **The expected state is read before the simulation starts.** `main.rs` reads
  the file, then builds the run. An oracle a runner could derive from the run it
  judges would make every repro pass, and the ordering rules it out by shape
  rather than by care. `repro::judge` takes two `&str` and performs no I/O, so
  there is no path through it that could produce one either.
- **The two verdict words share no substring**: `reproduced` and `diverged`. The
  obvious phrasing — "reproduced" against "not reproduced" — makes an agent
  grepping for the first one match both, and a machine-readable answer that is
  wrong half the time is worse than none. A wording test holds it.
- **State equality is `first_difference` returning `None`**, which is what
  `sim::assert_same_state` and the integration suite's copy already mean by it. A
  second definition here would be a second definition of "the same state". A pair
  that agrees line for line and differs in bytes — a translated line ending, a
  dropped final newline — is therefore reproduced, and the message says so rather
  than swallowing it.
- **A byte-order mark on the expected state is neither a pass nor a divergence
  either, and this one was found by measuring rather than by design.** The first
  draft of this runner's own comment said PowerShell's `>` produces the
  line-endings case. It does not: measured on Windows PowerShell 5.1 against this
  binary, `narvo --mode chance --ticks 3 --dump > x` writes 2 163 bytes
  beginning `ef bb bf` with 101 CR beside 101 LF. The mark sits on line one, so
  `first_difference` reported a divergence between `entities 33` and
  `entities 33` — two lines that read identically, sending a reader after a
  simulation defect that was not there. It is now its own outcome, refused by
  name and never stripped, because stripping would mean comparing something other
  than the file's bytes. Since the commonest way to write a repro on this
  project's own platform is a shell redirect, this is the ordinary path and not
  an edge.
- **An empty expected state is neither a pass nor a divergence.** It is its own
  outcome, because "the run that should have written this file wrote nothing" is
  a different finding from "the state moved", and reporting it as a divergence at
  line one would send a reader after a simulation that never misbehaved.
  `xtask determinism compare` refuses an empty manifest for the same reason.
- **No new step in the verification set.** The runner is ungated code in
  `narvo-app`, so steps two, eight and ten already build and test it in every
  configuration this repository has. That is the opposite of M6.3d's tenth step,
  which existed because a feature that is off by default was built by nothing;
  `--expect` adds no feature and therefore no configuration.
- **A repro has a shelf life, and that is acceptable because it is handed over.**
  It is valid for the build and the `Cargo.lock` it was made against — ADR-0008's
  third row. A dependency bump may move the dump while the simulation is correct.
  Because no repro is committed, that expiry costs nobody a red CI run; it costs
  the person holding the repro one re-record, which is what they would do anyway.

## What the runner proves, and what it does not

It proves exactly one thing: **this build, fed this recording, reaches this state
after this many ticks — or it does not, and here is the first line where the two
part company.**

It does not prove that the state is correct. Nothing anywhere records what the
right answer would have been (ADR-0008 says this of the hash and it is equally
true of the dump), so "reproduced" is a statement about a build and a state and
never a judgement of either.

It does not prove that a **defect** was reproduced. A repro shows a state again;
whether that state exhibits the defect is a claim in the repro's prose and not
something any comparison can check. The distinction matters for M6.7b, where the
temptation is to read a red repro as "the bug is back" when what it says is "the
state is not the recorded one".

It cannot say that a band was **cut**. D19's cut leaves an ordinary, complete
recording of a shorter run — byte-indistinguishable from one that simply had
nothing left to record (ADR-0032, and `Recording::cut_to`'s own documentation) —
so a repro built over a cut band reproduces only as far as the cut. What the
runner can do is name the tick it reached, and it always does. Measured on a real
cut band: a run that executed 50 ticks left a band saying 8, and the repro over
that band reported `diverged - after 8 ticks`. Nothing false, and less than the
whole truth, which is the most a reader can be given here.

## Revision condition

Reopen if a repro ever has to survive a dependency bump — that needs a stability
story ADR-0008's third row does not offer — or if agent-written tests are wanted
in the tree. The second is a human's decision and needs two things this ADR
deliberately does not invent: who blesses a case, and what the cross-platform
matrix does with it.
