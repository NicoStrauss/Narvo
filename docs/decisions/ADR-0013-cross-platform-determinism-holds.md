# ADR-0013: The M2 kill criterion did not occur; cross-platform determinism stays a hard goal

Status: accepted · Date: 2026-08-07 · Scope: the whole project, and every
milestone that rests on reproducible simulation

## Context

`ProjektPlan.md` §6/M2 has carried a pre-registered kill criterion since v0.1:

> If cross-platform floating-point determinism (Windows ↔ Linux) cannot be held
> at acceptable cost, the goal is reduced by ADR to "determinism guaranteed per
> platform, cross-platform best effort" — the decision falls in M2, not later
> and not implicitly.

Pre-registering it was the point. A goal that is quietly relaxed once it becomes
inconvenient was never a goal, and "we'll see how far we get" produces no
falsifiable claim at all. So the condition, the fallback and the deadline were
written down before any of the evidence existed.

M2.2 through M2.4b produced that evidence. This ADR records what was decided
about it. The decision is the human's; this document is the protocol.

## Decision

**The kill criterion did not occur. The documented fallback is not taken.
Cross-platform determinism remains a hard project goal.**

## What is promised, and in which domain

The promise is stated as a named domain rather than as a general claim.
"Narvo is cross-platform deterministic" would reach past the evidence, and an
overreaching promise is worse than a narrow one: it is the sentence a later
session would rely on without going back to what was actually measured.

> For the simulation core at its present extent, Windows and Linux are
> byte-identical — in the `dev` and in the `release` profile, at 10 000 and at
> 1 000 000 ticks, on the same physical CPU and on two separate CI runners, at
> the checkpoints 1 / 100 / 1 000 / 5 000 / 10 000, with `f32` accumulation, a
> seeded RNG, an event buffer and an input stream inside the state hash.
> Verified with a pinned toolchain (identical rustc commit on both sides), a
> pinned `Cargo.lock`, on x86-64.

Two properties of that evidence are what separate it from a coincidence, and
neither is incidental:

- **The `f32` accumulation in `motion` is deliberate.** It advances a phase by
  0.017 per tick — a value with no exact binary representation — and wraps it
  about 170 times over 10 000 ticks. A purely integer simulation would have
  agreed across platforms by construction and demonstrated nothing. The one
  quantity in the whole suite that *could* diverge was put there so that
  agreement would mean something.
- **The checkpoints are part of the promise, not a debugging extra.** A replay
  that agrees only at the end can establish *that* two runs match and never
  *where* they stopped matching. Without intermediate agreement there is no way
  to localise a divergence, and localising one is what the instrument is for
  once a physics solver arrives.

## The evidence

Hashes are 64-bit FNV-1a over the canonical dump (ADR-0008). Every figure below
was re-measured on 2026-08-07 on both platforms before this ADR was written.

| Check | From | Result |
|---|---|---|
| `motion`, 10 000 ticks, `f32` accumulation over ~170 wraps | M2.2 | `425113c82dd30a7b`; dumps byte-identical, 2 629 bytes |
| `chance`, seed 1 and seed 2 — seeded RNG and event buffer inside the hash | M2.2b | `7c62f1f262614edf` and `35833ab0fca80978`; dumps byte-identical, 2 379 and 2 360 bytes |
| `input`, seed 1 and seed 2 — input stream, all four live/replay combinations including cross-replay | M2.3 | `442b307056344d1b` and `9921f95453834f08`; dumps byte-identical, 2 222 and 2 232 bytes |
| Checkpoints 1 / 100 / 1 000 / 5 000 / 10 000, replay against original | M2.3, M2.4 | identical at every one |
| Recordings produced independently on each platform | M2.3 | byte-identical, 51 376 bytes |
| The suite as permanent CI infrastructure, comparing **two separate runners** | M2.4 | 19 of 19 artifacts identical |
| `release` profile, both platforms, all pairings | M2.4b | 19 of 19 identical in each of `release`↔`release`, `dev`↔`release` per platform, and crosswise |
| 1 000 000 ticks — 100× the matrix length, ~100 000 `f32` wraps | M2.4b | `7bfefe116e9fe558`, `beddf442c84e2934`, `8d00b2a2fab0136a`; identical across both platforms and both profiles |

The toolchain pin the last two rows depend on, checked again the day this ADR
was written:

```
windows: rustc 1.97.1 (8bab26f4f 2026-07-14)
linux  : rustc 1.97.1 (8bab26f4f 2026-07-14)
```

Identical commit hash, not merely identical version string. That is what
`rust-toolchain.toml` exists for.

## Why hashes appear in this document and nowhere else

ADR-0008 forbids committing the hash of a state to this repository. That rule is
not broken here, and the distinction matters enough to write down so that a
later reader does not mistake this for a violation.

**An ADR is a protocol, not a test.** Nothing compares against these values.
They are dated observations — "on 2026-08-07, with this dependency set, the
simulation produced this" — and if a `ron` release changes the dump format
tomorrow, these numbers become historical rather than wrong. A hash in a *test*
would turn the same release into a red suite with nothing broken, which is the
failure ADR-0008 exists to prevent. The determinism suite therefore still
compares two runs against each other and stores no expected value anywhere.

## What is explicitly not promised

- **Physics.** rapier2d arrives in M5 and nothing here predicts it. A solver
  brings square roots, trigonometry, iterative methods and sums whose order
  depends on contact ordering; the entire floating-point surface measured above
  is one accumulated addition and subtraction on `f32`. This is the largest gap
  by a wide margin.
- **Other toolchain versions.** Both sides resolve to one pinned rustc commit.
  Determinism *between* compiler versions is untested, and ADR-0008's stability
  table already excludes it.
- **Other dependency sets.** CI builds `--locked`. That a `ron` or serde bump
  preserves the dump format is checked by nothing.
- **Other microarchitectures, and ARM at all.** The local comparison holds the
  CPU constant by construction — WSL runs on the same physical processor as the
  Windows build (ADR-0007). Since M2.4 the CI job does compare two separate
  machines, which is the first evidence from hardware other than the
  maintainer's, but both are x86-64 and neither was chosen for its
  microarchitecture. Apple Silicon and other ARM targets are untouched.
- **Scale.** The demo worlds hold 32 and 33 entities. Nothing is known about
  large worlds, archetype variety, or slot recycling at volume.
- **The `release` profile as a guarded property.** M2.4b measured it once, on
  one day, by hand. It is not watched by anything — see the open point below.
- **Correctness.** The comparison establishes that two platforms compute the
  *same* thing, never that they compute the *right* thing. If both make the same
  mistake, every check here is green. That limit is inherited from ADR-0008's
  "a comparison tool, not an identity tool", one level up.

## What this decision is not

It is **not** a licence to relax the determinism suite later. The suite became
permanent CI infrastructure in M2.4 precisely so that this decision keeps being
re-earned on every push rather than resting on one good afternoon. It is also an
acceptance criterion for M5: `ProjektPlan.md` §6/M5 requires the suite to stay
green with physics active, and that is the real examination.

Nor does it settle anything about determinism as a general property of Rust, of
IEEE-754, or of these two operating systems. It is a statement about this
simulation core, at this size, under these pins.

## Consequences

- The fallback wording — "determinism guaranteed per platform, cross-platform
  best effort" — is not adopted, and §6/M2's kill criterion is closed. A future
  reduction of the goal needs a new ADR superseding this one, with its own
  evidence.
- `enhanced-determinism` is mandatory for rapier2d from M5 (`ProjektPlan.md`
  §6/M5). It was a recommendation; with cross-platform determinism confirmed as
  a hard goal it becomes a requirement, because the alternative is to discover
  in M5 that the goal was abandoned by a default feature flag.
- **Open point, not built here.** *(Amended 2026-08-07 — M2.8; see below.)*
  The `release` comparison should run when its
  answer can change, rather than on every push: on a change to
  `rust-toolchain.toml`, and on a change to the `[profile.*]` sections of
  `Cargo.toml`. Anyone who adds a `[profile.release]` with LTO or a different
  `opt-level` moves the answer exactly as a compiler bump does. Implementation
  is its own task.

  The objection that belongs with it: a job nobody looks at is the same failure
  class as the three scars this project already carries — golden images with no
  adapter, M1.3b, and the WSL mtime trap. "Less often" must not become
  "switched off". Whatever is built has to fail as loudly as an ordinary CI
  failure, and tying it to the two files that can change the answer is what
  makes it fire when it has something to say.

  *Amended 2026-08-07 (M2.8). The point above is no longer open. It is kept as
  written, because it is what was decided at M2 close; this paragraph records
  what became of it. M2.7 built it as
  `.github/workflows/release-determinism.yml`, bound to `rust-toolchain.toml`,
  to `Cargo.toml` and to the workflow file itself, plus `workflow_dispatch` so
  that a failure can be re-run and anybody can look at the answer without
  editing a file. M2.8 added the third trigger, `.cargo/config.toml`
  (D9, `ProjektPlan.md` §11): that file sets `rustflags` and the linker, so a
  change there moves codegen in the same class as a compiler bump.*

  *Two things are deliberately unchanged by that. The filter still matches the
  whole of `Cargo.toml` rather than its `[profile.*]` sections: narrowing it
  would mean parsing TOML in a shell step, and the asymmetry decides it —
  over-firing costs a few runner minutes, under-firing costs a missed
  regression in the profile the game actually ships in. `ProjektPlan.md` §7.3
  names that imprecision as deliberate and it stays so. And the objection above
  still stands and still carries: there is no schedule, the workflow fails as an
  ordinary red CI run, and "less often" must not become "switched off". A third
  trigger widens what makes the job fire; it changes nothing about what happens
  when it does. The decision this ADR records is unchanged and it stays
  accepted.*
- M2 closes with this ADR.

## Revision condition

**M5, when rapier2d arrives**, and that is a date rather than a sentiment. If
determinism breaks under the solver, this decision is taken again on the new
evidence — deliberately, in a new ADR — and not quietly softened by widening
what "deterministic" was supposed to mean.

Reopen also on any of the excluded conditions becoming a project requirement: an
ARM target, an unpinned toolchain, or a simulation whose scale leaves the range
measured here.

## Amendment 2026-08-12 (M5b.1 / M5b.2): the pre-measurement half of the M5 revision

*Everything above is unchanged and stays accepted. This section records what
became of the revision condition, in the form ADR-0004 uses for its amendments:
the standing text is what was decided then, and this is what happened next.*

**The revision condition has been met for its pre-measurement half, and the kill
criterion did not occur.** "M5, when rapier2d arrives" was written when rapier
was expected inside M5. `ProjektPlan.md` §6/M5 moved it out and §6/M5b brought it
back as its own block, so the condition is discharged in two parts: M5b.1
measured rapier against a synthetic scene, and M5b.3 will measure the suite with
physics active on two CI runners. **This amendment discharges the first part
only.**

What M5b.1 measured, on the configuration this project would ship
(`rapier2d 0.35.1` with `enhanced-determinism`, pinned toolchain, one
`Cargo.lock`): a 32-body scene at `narvo-core`'s own 16 666 666 ns tick,
recorded in four configurations — Windows and WSL, `dev` and `release` — and
compared byte for byte at the checkpoints 1 / 100 / 1 000 / 5 000 / 10 000 over
1 000 and 10 000 ticks. **Sixteen of sixteen pairings identical**:
self-reproducibility per configuration, Windows against WSL per profile, and
`dev` against `release` per platform. A one-ULP perturbation of a single start
coordinate reached 30 of 32 bodies within 100 ticks, so the instrument was
sensitive enough for the agreement to mean something. Figures in
`target/reports/M5b_1.md`.

**`enhanced-determinism` stays mandatory — with a qualification that has to
travel with it.** The requirement in the Consequences above is unchanged, and
M5b.2 gave it the enforcement point it never had:
`the_manifest_still_asks_for_enhanced_determinism` in `narvo-physics2d` reads
the manifest at run time and fails if the feature leaves, which matters because
the flag is not one of rapier's defaults and dropping it would otherwise be
silent.

The qualification is this. M5b.1 also built the *default* configuration, without
the flag, and its dumps were **byte-identical to the flagged ones** in all eight
compared cases, and byte-identical across Windows and WSL in both profiles by
itself. The two builds genuinely differ — five resolved feature flags, and
`indexmap` present in one tree and absent from the other — but in that scene the
flag changed no bit of output. **So the probe measured that the mandated
configuration agrees. It did not measure that the mandate is what produces the
agreement.** The requirement is therefore kept on the argument it was always
kept on — that the alternative is discovering in a later milestone that the goal
was abandoned by a default feature flag — and not on evidence that it was load-
bearing here, because there is none.

**Revision condition for that qualification specifically: a measurement that
resolves the flag's contribution.** A scene, a body count or a platform pair
where the two configurations produce different bytes would turn the requirement
from a precaution into a finding. Until one exists, the honest statement is the
one above, and it belongs in any later text that cites this ADR for the
requirement.

**What stays open, explicitly.** The named domain in "What is promised, and in
which domain" is *not* widened by this amendment. In particular:

- **Two separate runners.** M5b.1's evidence is Windows against WSL on one
  physical CPU, which is the same limit the fourth bullet of "What is explicitly
  not promised" already carries. The two-runner evidence for physics arrives with
  M5b.3, not here.
- **Real game scenes.** The probe is a toy: 32 bodies, two shapes, no joints, no
  CCD event, no sensors. `ProjektPlan.md` §6/M5b names this and keeps the
  measurement against real scenes at rung 2 of the slice ladder.
- **The solver's own retained state was not part of that measurement at all.**
  M5b.2 found it carries behaviour — a rebuilt world and a retained one part
  company at the first tick that resolves a contact — and decided in **ADR-0029**
  that the facade keeps none of it. That decision is what keeps physics inside
  the domain this ADR governs; without it, physics state would sit outside the
  canonical dump and the promise above would not reach it.

## Amendment 2026-08-12 (M5b.3b / M5b.3c / M5b.4): the two-runner half, and what it does not reach

*Everything above is unchanged and stays accepted. This section closes the
revision condition the amendment above left half open, and states the limits the
measurements that closed it also produced.*

**The revision condition is discharged, and the kill criterion did not occur.**
M5b.3b put the solver into the cross-platform matrix as its own mode — five
checkpoints from `t1` to `t10000`, the same table the other modes use — and the
comparison job answered on the evidence that was missing before, two CI runners
rather than WSL beside Windows:

```
xtask: 26 files identical across determinism/linux and determinism/windows
```

Twenty-six payload files, twenty-three dumps and three recordings, byte for byte.
The release workflow's three comparisons were green in the same run: release
against itself, `dev` against `release` on each platform, and release across the
two platforms. Figures in `target/reports/M5b.3b.md`.

That is the answer to the largest gap the *"What is explicitly not promised"*
section names — **"Physics. rapier2d arrives in M5 and nothing here predicts
it."** It is now measured rather than unpredicted. The bullet stays as written,
because what it says about the floating-point surface a solver brings is still
true and is why the measurement was worth taking.

### Three limits the same measurements produced, and none of them is a caveat added afterwards

**The sensitivity is bracketed from one side.** M5b.3c perturbed the physics mode
by a single ULP and measured what the matrix does with it. A recurring one-ULP
difference, and a one-off one introduced after the first contact, both reach every
checkpoint from `t100` on, at 12 of 13 bodies. **That shows a difference of that
size gets through; it does not show the smallest difference that would.** Nothing
was run to find the floor, and no statement here should be read as if something
had been.

**The pre-contact phase is normalised, and `t1` carries that region alone.** The
*same* one-ULP perturbation applied before the first contact is absorbed
completely: it shows at `t1` and is gone by `t100`, out to `t10000`. So for a
divergence that lands while the bodies are still settling, the matrix's later
checkpoints are weaker evidence than M5b.1's figure — 30 of 32 bodies within 100
ticks — would suggest, because that scene had no attractor and this one does. The
pair is pinned in `sim::physics`'s own tests since M5b.4
(`a_one_ulp_perturbation_after_the_first_contact_spreads` and its sibling), so the
property is a test rather than a paragraph.

**The two runners are the same processor model.** Both CI jobs reported
`Intel(R) Xeon(R) Platinum 8370C CPU @ 2.80GHz`. Twenty-six files identical
across two *machines* is not twenty-six files identical across two
*microarchitectures*; it is strictly stronger than M5b.1's one-CPU comparison and
strictly weaker than the fourth bullet of *"What is explicitly not promised"*
being retired. **That bullet is not retired.**

### Not the other open condition

The first amendment left a revision condition of its own — a measurement that
resolves what `enhanced-determinism` contributes, since M5b.1 found the flagged
and unflagged builds byte-identical in the scene it had. **Nothing here touches
it.** That question is still open and the flag is still kept on the argument the
first amendment gives, not on evidence that it is load-bearing.

### One consequence for the render path (M5b.4)

Physics reaches a *picture* from M5b.4 on: `scenes/physics_drop.ron` is drawn from
its bodies and blessed as a reference image. That image is rendered on both
platforms and compared against one committed file, which makes it a second place
where a platform difference would show — and it is **outside** the domain this ADR
promises anything about, because the render path only reads (ADR-0005) and its
arithmetic never enters a state hash.

So the seam was built to keep the promise from being leaned on where it does not
reach: `SpritePlacement` carries the rotation as the `(cos, sin)` pair a
`RigidBody` already holds, and there is no trigonometric operation between a body
and its pixels. The rejected alternative — projecting the pair to an angle with
`atan2` so the placement could keep carrying one — would have put standard-library
trigonometry on that path, which M5b.2 measured to be lossy on Windows and exact
on Linux for one and the same value, and `enhanced-determinism` does not reach
`std`.

**What is still unmeasured there, named rather than left to be discovered:** the
blessed reference is the first in this repository whose sprites are *turned*, and
D17's entry lists rotation as an open front for both interpolation qualifiers.
Partial coverage at a rotated edge is resolved by MSAA sample positions, and
whether three rasterisers agree about those to within the golden tolerance is
measured for the two this project can run locally and for nothing beyond them.

## Amendment 2026-08-16 (V5 / D18): the filter gets two more paths

*Everything above is unchanged and stays accepted. This section records a
decision about **when** the release comparison runs. It changes nothing about
what the comparison computes, and the three checks named under the M2.8
amendment are the same three afterwards.*

**Decided by the human on 2026-08-16 (D18, `ProjektPlan.md` §11):**
`.github/workflows/release-determinism.yml` gains `crates/*/Cargo.toml` and
`Cargo.lock` as trigger paths, bringing the filter from four entries to six.

### Why, and it is a measurement rather than an argument

The M2.8 amendment above bound the filter to the files that can move a
*profile*, and it was right about that: a member manifest cannot define one,
because cargo warns "profiles for the non root package will be ignored" and
ignores it. M5b.3a then measured that the same reasoning leaves a different
question unanswered. `e09c5f1` added rapier to the shipped tree through
`crates/narvo-app/Cargo.toml`, moved `Cargo.lock` with it, touched no root
manifest — and this workflow did not fire. V5 reproduced that on the live
system, on both of the paths `CLAUDE.md` requires: the full SHA
`e09c5f1057c859d875f2796e8a206a44ed59aad1` returns one run and it is `CI`, and
a scan of every run this workflow has ever had does not contain the commit. "A
dependency walked into the shipped build" is a different question from "the
profile moved", and only the second one had a trigger.

`Cargo.lock` is the sharper half of the same class, because CI appends
`--locked` to every cargo step: the lock is what the release binaries are built
from, and a bare `cargo update` moves that tree without touching a manifest at
all.

The cost objection is what the D18 measurement answered, and V5 re-ran it over
the last 80 commits: **6 touch the two new paths, 3 touch the four old ones, and
the overlap is 3 — complete.** Every commit that fires the filter today also
moves one of the new paths, because adding a dependency moves the root manifest
and the lock together. So the union is 6 and the growth is exactly **3 extra
runs per 80 commits**, not 6. The three that only the new filter catches are
`ec2412f`, `d675b54` and `a3f2a36`.

### The boundary, which is deliberate

The pattern is `crates/*/Cargo.toml` and **not** `**/Cargo.toml`. A `*` in a
GitHub path filter does not cross a `/`, so it reaches the twelve engine crates
one level below `crates/` and no member manifest outside it — the two under
`tools/`, and xtask. M7's game becomes a workspace member outside `crates/` as
well. A game manifest says nothing about whether the engine computes the same
thing in two profiles, so its not firing this workflow is the intent and not a
hole. Reopen that if a game manifest ever becomes able to change what the
engine builds — a `[patch]` section over an engine dependency would do it.

### The gap, named rather than closed

`Cargo.lock` records no feature selection. Its keys are `version`, `name`,
`source`, `checksum` and `dependencies`; the word "features" occurs in the file
three times and every one is inside the crate name `document-features`, never as
a key. A manifest edit that only adds `features = [...]` or sets
`default-features = false` therefore changes the code that gets built and moves
no lock. Inside `crates/` the new manifest pattern catches it anyway; in a
manifest outside `crates/` nothing does. The sentence lives beside the filter in
the workflow's own header comment, so a later reader finds it without this ADR.

### The candidate that was not built

**Option C — move external dependencies into `[workspace.dependencies]` and add
a guard that keeps them there.** Its best argument is real and worth writing
down: it would put every dependency edit back into the root manifest, where the
old filter already fired, and the gap above would close as a side effect rather
than being documented. It is not built, and V5 was told not to build it. It
stays a named candidate. **Reopening condition:** a measurement showing that
member-manifest dependency edits are frequent enough that the per-crate pattern
over-fires, or a second gap of this class that the pattern cannot reach.

### What this does not touch

The comparison itself. The `record` job still builds `narvo-app` and `xtask`
headless in both profiles, records three matrices and compares release against
itself and dev against release; the `compare` job still diffs the two platforms
in release. A filter that fires more often would be worth nothing if the thing
it fires had quietly moved, and it has not.
