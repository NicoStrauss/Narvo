# CLAUDE.md

Working agreement for AI agents (and humans) in this repo. This file stays
short on purpose — persistent context lives here and in `docs/`, not in chat.

## Project

Narvo is an AI-first 2D game engine in Rust (3D path planned later). It is
optimized for agent workflows: deterministic headless simulation, text-based
content, fast iteration. Verification beats plausibility: no feature is done
without a machine-checkable proof.

## Claims need evidence

A statement about how a tool, an environment or an API behaves needs a source:
a `file:line`, the output of a command that was actually run, or the source of
the version pinned in `Cargo.lock`. Without one, write **uncertain**. That is a
complete answer and not a failure; a confident wrong one costs a session. This
applies to reports, commit messages and code comments alike — and the comment is
the worst place to get it wrong, because it outlives the conversation that
produced it and the next reader has no way to tell a checked claim from a
plausible one.

Three times so far an unchecked assumption was written in the tone of a checked
one:

- M1: `wgpu`'s `SurfaceTexture` was assumed to present when dropped. It
  discards. The window stayed blank while every other signal said the frame was
  drawn correctly.
- M2.1: integration tests under `tests/` were assumed to see only
  `[dev-dependencies]`. They see the package's `[dependencies]` too, which made
  a comment claiming the facade was mechanically sealed against `hecs` simply
  false.
- M2.1c: the repository was assumed to be public, from a recommendation in
  `ProjektPlan.md` §11 read as a decision. It is private.

Each was plausible, cheap to check, and stated as fact. Check it, or mark it
uncertain.

**Undoing a temporary change is itself a claim.** `git checkout -- <file>`
restores the whole file, not just the lines that were injected into it — so a
demonstration and the real work living in one file means reverting the first
silently discards the second. In M2.6 that happened: the caller this task
existed to change was reverted along with the injection, and only a positive
check caught it. So after every revert, verify **the presence of the intended
work**, not the absence of the injection:

```
grep -n "<the thing that was supposed to change>" <file>
```

Absence proves nothing about what else went with it. Prefer putting an injection
in a file the task does not otherwise touch.

**`gh` answers an unanswerable question with an empty one.** Two instances, both
found in M5.5a and both after the empty answer had already been reported as
evidence:

- `gh run list --commit <sha>` needs the **full** SHA. Given a short one it
  returns `[]` — the same answer it gives for a commit that genuinely has no
  run. Three "no CI run on this commit" findings were gathered that way before
  anyone checked. Use the full SHA, and confirm against a scan of
  `gh run list --limit N`.
- `gh run watch` can return before every job's fields have settled, and a
  `conclusion` then comes back **empty** rather than absent. Read `status`
  beside `conclusion`, or read the run twice. **Never report a job green on a
  blank field.**

The shape is the same both times: a query returning something that looks like
data and is not. It costs nothing to read a workflow result twice.

**`cargo deny check` is green about the tree it can see, which is not always the
tree you changed.** Third instance of the same class, found in M5.6b. A
dependency that is declared `optional` and whose feature nothing activates is
**absent from cargo-deny's graph**: `cargo deny check` reported
`advisories ok, bans ok, licenses ok, sources ok` on a workspace that had just
gained `kira`, because the crate enabling it defaulted the feature off. The green
was true and answered a question nobody had asked.

So: **a deny green counts only once `cargo deny list` names the crate you
added.**

```
cargo deny list | grep -i <the crate you just added>
```

Empty output means the check never looked at it. The same reasoning applies to
the design of a feature gate — nothing in this repository runs `--all-features`,
so a dependency reachable only through an opt-in feature is one the licence
policy never inspects.

## Verification set

One command:

```
cargo xtask ci
```

It runs the eleven commands below in exactly this order, passes their output
through unfiltered, and stops at the first one that fails — naming it in a form
that can be pasted straight back into a shell. All eleven must exit 0 before any
task counts as complete ("green"):

```
cargo build --workspace
cargo nextest run --workspace              # tests; CI uses --profile ci
cargo test --doc --workspace               # nextest does not run doctests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check
cargo build -p narvo-app --no-default-features
cargo nextest run -p narvo-app --no-default-features
cargo tree -p narvo-app --no-default-features --edges normal
cargo nextest run -p narvo-app --features ipc
cargo clippy -p narvo-audio --no-default-features --all-targets -- -D warnings
```

The eleven stay documented individually because the full run is often not what is
wanted: iterating on one crate is `cargo nextest run -p narvo-ecs`, and a
failure `cargo xtask ci` reports is reproduced by rerunning that one line.

**Steps seven to nine are the headless configuration**, and they were CI-only until
M4.9. The gap shipped a defect: M4.8 added an integration test importing a
render-gated crate without the matching `#![cfg(feature = "render")]`, the local
run was green, and CI failed on both platforms. A local "green" that has never
executed three of the checks is exactly what this command exists to prevent.

**The tenth is the agent transport**, added in M6.3d. D20 put it behind a
feature that is off by default, so neither of the two configurations above
compiles it and the one that does was built by nothing — the same shape as the
M4.8 gap the headless trio closes, one gate further along. It is a test run
rather than a build because the socket tests need a binary that has a socket.

**The eleventh is audio without a device**, added in V4, and it is the fourth
step here that exists because a configuration was checked by nothing. `cargo
clippy --workspace` unifies the features of the members it builds: `narvo-app`'s
`render` turns `narvo-audio/device` on, so every workspace-wide step above sees
that crate *with* a device and the configuration without one is linted by nobody.
What that hid was measured rather than argued — three `Mishap` variants are
constructed only in the device-gated `kira_sink.rs`, so `narvo-audio` without
`device` carried a `dead_code` warning while step four reported green.

It is scoped to one crate on purpose. A crate with n optional features has 2^n
configurations, this set now checks five, and the boundary is a measurement
rather than a preference: V4 linted every non-default configuration in the
workspace and `narvo-audio` was the only unclean one. The wider cut was priced
in the same measurement — linting the whole headless configuration is 93 packages
and 34.7 s cold against this step's 1 package and 3.3 s, and once `narvo-audio`
is clean it catches nothing further. Appended rather than filed beside the
headless trio so that the step numbers above keep meaning what they say.

**Three steps stood after the eleventh from M7.1 until U2, and they are gone
rather than superseded.** They were the game without a window: a headless test
run of `forge-loop`, a read of its dependency tree, and a lint of the same
configuration. All three named a package this workspace no longer has — U2 moved
the game out to a standalone crate beside the repository — and a step naming a
package the workspace lacks is a build error, not a weaker check.

**Their reason did not expire with them, and one half of it left the tree.** The
three existed because a game's `render` feature is on by default, so `cargo
clippy --workspace` unified it on for everybody and the headless configuration
was linted by nobody; that argument now belongs to whoever builds the game.
What travelled with the manifest is sharper and is written down here because
this is where it would be lost: the workspace routed `narvo-audio` with
`default-features = false` **on the game's behalf**, since a member cannot
override an inherited dependency's `default-features` and cargo says so outright.
Outside a workspace there is nothing to inherit from, so the standalone manifest
has to declare it itself — and the step that would have caught the omission is
one of the three that went away. U2 built and tested the extracted crate headless
for exactly that reason.

**The numbers above therefore mean what they say again, with no gap.** The set
was appended to three times (V4, M7.1, M7.9) precisely so the earlier step
numbers would keep their meaning; removing from the end preserves that property,
which is the one advantage of having only ever appended.

`cargo tree` is the odd one out: it succeeds whatever it prints, so its verdict
is its *output*, and the headless dependency tree may not contain `wgpu`,
`winit`, `naga`, `raw-window-handle`, `image`, `kira` or `cpal`. That list lives
in `xtask` as `FORBIDDEN_IN_HEADLESS` and in the workflow as a `grep -Eiw`
pattern, and a test compares the two — **against every such line in the workflow
rather than the first**, a shape M7.1 needed when there were two of them and U2
left in place when there was one again. It costs nothing and it is the half that
survives the next addition.

CI runs the same set with two differences: it appends `--locked` to every cargo
step, and it runs nextest with `--profile ci`. Locally the commands stay
unlocked, so that adding a dependency can update `Cargo.lock` as part of the
work. The failure mode worth memorising: if CI fails on `--locked` while
everything was green locally, `Cargo.lock` was not committed. One further
difference, in the safe direction: CI runs the tree check on Linux only, this
runs it always.

Changing the set means changing three places in one commit: this section,
`xtask/src/main.rs`, and `.github/workflows/ci.yml`. Three tests in `xtask` fail
when they drift apart — one checks every step against this file, one against the
workflow, and one holds the forbidden-crate list against the workflow's grep
pattern. CI still runs *more* than these eleven (the determinism artifacts and
their cross-platform comparison); those are outside the set and outside the
guard.

A further check runs beside them and is not a cargo command:

```
cargo xtask whitespace
```

It reports collapsed line continuations in string literals — a run of six or
more spaces inside an ordinary literal on a line rustfmt could not break, which
is what a lost `\` leaves behind. §9.2 of `ProjektPlan.md` carries the
prevention rule; this is the detection half, added in M4.9 after the class had
been found by eye fourteen times. It is part of `cargo xtask ci`.

## Determinism suite

Two halves, because they answer two different questions.

Everything one machine can check is an ordinary test in
`crates/narvo-app/tests/determinism.rs`, driving the built binary: two runs of
each mode agree over 10 000 ticks, different seeds do not, a replay reproduces
its original at ticks 1 / 100 / 1 000 / 5 000 / 10 000, a tampered recording
does not, and a recording replays in a process that did not produce it. It needs
no separate command — `cargo nextest run --workspace` already runs it, which is
step 2 of the set above, and it costs about a second.

Whether *two platforms* agree is not something a test can answer, since a test
observes only the platform it runs on. That half is:

```
cargo xtask determinism record <dir>          # once per platform
cargo xtask determinism compare <dir-a> <dir-b>
```

Each CI job records its matrix and uploads it; a third job compares the two and
fails naming the case, the entity and the component that differ. Locally the
same two commands compare Windows against WSL.

**No expected hash is ever stored** — not in a test, not in a fixture, not in a
recording header (ADR-0008). Everything compares two runs produced from one
commit and one `Cargo.lock`, so a dependency bump moves both sides together
instead of turning the suite red for nothing.

### The release profile

All of the above runs in `dev`; the game ships in `release`. That case has its
own workflow, `.github/workflows/release-determinism.yml`, which does **not**
run on every push — it runs when its answer can change (ADR-0013):

- a change to `rust-toolchain.toml`, because a compiler bump can move codegen;
- a change to `Cargo.toml`, because `[profile.release]` moves it the same way.
  Only the workspace root can define a profile — cargo warns and ignores one in
  a member manifest;
- a change to `.cargo/config.toml`, because it sets `rustflags` and the linker,
  and those are codegen options in rustc's own taxonomy — `rustc -C help` lists
  `-C link-arg` and `-C linker` beside `-C target-cpu`;
- a change to `crates/*/Cargo.toml`, the manifest of an engine crate, because a
  member manifest is where a *dependency* lands and a dependency reaches the
  shipped binary — even though, as the bullet above says, it cannot define a
  profile (D18, added in V5). The pattern stops one level below `crates/` on
  purpose: a member outside that directory — the tooling, and M7's game — says
  nothing about whether the engine computes the same thing in two profiles, so
  its manifest deliberately fires nothing;
- a change to `Cargo.lock`, because every cargo step in CI is `--locked`, which
  makes the lock what the release binaries are actually built from; a bare
  `cargo update` moves that tree without touching any manifest, and before V5
  nothing fired on it (D18). The named gap beside it: a lock records no feature
  selection, so a manifest that only flips a feature moves no lock — inside
  `crates/` the bullet above still catches it, outside it nothing does;
- **by hand, any time**, via *Actions → release determinism → Run workflow*
  (`workflow_dispatch`). That is how a failure is re-run and how anybody looks
  at the answer without editing a file.

It checks three things per run: that release is reproducible against itself, that
`dev` and `release` agree on each platform (the optimisation level as the only
variable), and that the two platforms agree in `release`.

A failure means the shipped profile computes something the tested profile does
not. It is an ordinary red CI run, not an advisory — treat it as a determinism
regression and go to ADR-0013's revision condition.

## Linux verification (WSL)

The verification set also runs on Linux locally, in WSL 2, on this same working
copy — there is no second clone. From Windows:

```
wsl -d Ubuntu-24.04 -e bash -lc 'cd /mnt/d/Narvo && CARGO_TARGET_DIR=$HOME/.cache/narvo/narvo-main cargo xtask ci'
```

`-l` matters: the login shell is what puts cargo on `PATH` and sets the Vulkan
ICD, so the renderer tests run against the same software rasteriser CI pins
instead of whatever WSLg offers.

**`CARGO_TARGET_DIR` matters for a harder reason, and it is the whole of D25.**
Without it this line can verify a *different repository* and report green. Three
facts compose into that:

- the distro's `~/.cargo/config.toml` sets `target-dir` **user-wide**, so every
  workspace on the machine shares one build directory;
- cargo puts a package's binary at `<target>/debug/<name>`, so two packages both
  called `xtask` are one file and whoever built last owns it;
- `xtask` finds the tree it checks with `env!("CARGO_MANIFEST_DIR")`
  (`workspace_root`, `xtask/src/main.rs:550`), baked in at **compile time**, and
  runs every step with that as the working directory (`run_ci`, `main.rs:309`).
  A binary built from another
  checkout therefore checks *that* checkout, whatever the `cd` said.

Measured twice, on 16.08.2026: M7.0 found it, and V4 reproduced it on this tree
before repairing the line. The old form reported **all ten steps ok, exit 0** —
and ran 1 965 tests with 741 lines naming `games/forge-loop`, which at the time
was a crate this repository did not have. The number decomposes: 1 284 from the
frozen `Amboss-Gauntlet` clone plus 681 of its `forge-loop`, against this tree's
1 289. Two of the three test counts in that run (360 headless, 447 ipc) were
*identical* to this tree's, so only the workspace step distinguished them at all.

**One half of that evidence expired on 17.08.2026 and came back on 24.08.2026,
and both moves are written here rather than left for somebody to trip over.**
M7.1 created `games/forge-loop` in *this* repository, which cost the
discriminator: both trees had one, and both built a binary called `forge-loop`
into whatever target directory they were given. **U2 moved the game out again**,
so a compile line naming `games/forge-loop` is once more a line from the clone
and not from this tree — but do not lean on it, because it is the half that has
already flipped twice and the sibling checkout U2 created still builds a
`forge-loop` binary of its own.

What discriminates without having moved is the **step count** (this tree runs
eleven; the clone's `xtask` knows ten — note that those two are now one apart
rather than four, so read the number, not the impression) and the rule at the end
of this section: compile lines naming `/mnt/d/Narvo/`. The variable in the
command line above is what keeps the two apart in the first place.

**The clock check below does not catch this class; the compile-line rule does.**
The two clocks agreed to the second while the run was checking another
repository — a shared build directory is not a skew, so a clock comparison has
nothing to see. What disqualifies it is the rule at the end of this section: the
old-form run printed **no `Compiling` and no `Checking` line at all**, having
built and checked exactly nothing. The repaired form printed 73 and 179 over 183
packages, every `narvo-*` one naming `/mnt/d/Narvo/crates/`, and 1 289 tests.

**The directory was renamed on 24.08.2026, and every path in this section
moved with it (U3; ADR-0047 item 2 is where that is booked, and it is also
the one place that still spells the three old forms).** The two runs quoted
above were measured on 16.08.2026, under the old directory name, so a
transcript kept from before that date names the old path where the rule
names the new one. The rule carries today's spelling deliberately: it is
matched against a run made now, and a discriminator that matches nothing is
blunt without ever going red.

Linux artifacts stay on the Linux filesystem and out of `/mnt`, for two reasons
that have not changed: Windows and Linux artifacts in one `target/` invalidate
each other, so a shared directory turns every platform switch into a full
rebuild; and a target directory under `/mnt` sits behind drvfs and is far slower.
The distro's `~/.cargo/config.toml` is where `target-dir` is set for anything
that does not say otherwise, and it should name `$HOME/.cache/narvo/target`;
the variable above overrides it per call with a directory only this checkout
writes to.

**Measured on 24.08.2026, and named rather than quietly made true: the distro
has not been edited, and its `~/.cargo/config.toml` still points at the
pre-rename directory** — ADR-0047 item 2 spells it — **which holds 57 GB of
artifacts from before the rename.** The sentence above therefore describes the
configuration this repository expects rather than the one the machine has.
Closing that gap is a handgriff on the machine and not a change in this tree,
so U3 reported it instead of reaching outside the repository to make its own
documentation true. Until it is closed the fallback is *louder* than the D25
failure above rather than quieter: a forgotten `CARGO_TARGET_DIR` lands in a
directory this checkout never writes to, so nothing it builds can be mistaken
for this tree's.

The first run under the new variable also starts from an **empty** build
directory, because `$HOME/.cache/narvo` does not exist until it is created. A
cold full build is the expected cost of the rename and not an anomaly. That the override is per call is the known weakness — a forgotten variable
falls back to the shared directory silently, which is exactly the failure above —
and it is accepted rather than fixed, because the durable fix is a repo-local
`.cargo/config.toml` and that file is one of the six paths triggering
`release-determinism.yml` (D25 weighed and rejected it). The repository's own
`.cargo/config.toml` therefore still says nothing about target directories — it
is shared by both platforms.

The Linux tooling has to be kept in step with what the `ubuntu-latest` job in
`.github/workflows/ci.yml` installs — currently cargo-nextest, cargo-deny, clang,
lld, the lavapipe Vulkan ICD and `libasound2-dev` (the ALSA headers `cpal` links
against on Linux; without them `cargo build --workspace` fails in `alsa-sys`'s
build script rather than anywhere near the audio code). Nothing checks that
automatically. `sudo` in
the distro asks for a password, which a non-interactive session cannot supply;
`wsl -d Ubuntu-24.04 -u root -e bash -c '...'` is the way in, and it needs none.

**Compare both clocks before believing a WSL run.** Cargo decides freshness by
mtime, and the two platforms stamp mtimes with two different clocks. Let them
disagree and a changed crate can look up to date, so a run reports green having
compiled nothing.

It has happened once, in M2.2, and the direction was this: **the Windows clock
was two hours behind.** Source files Windows wrote therefore looked *older* than
Linux artifacts stamped with the correct time, and WSL built `narvo-ecs` from
the previous milestone. It only failed because a manifest change forced its
dependent to rebuild — had only `.rs` files changed, the run would have been
green about the wrong code. Nothing showed on the Windows side, because there
sources and artifacts share the same wrong clock and stay consistent with each
other.

That cause was first recorded the other way round, as the distro running ahead.
The offset had been measured correctly and attributed to the wrong side, which
is why the check below compares the two clocks instead of testing one against an
assumption about which of them drifts.

```
date                               # Windows
wsl -d Ubuntu-24.04 -e date        # must agree, to the second
```

If they disagree, correct whichever clock is wrong. Until the mtimes are
trustworthy again, `cargo clean` inside the distro before the run is the only
thing that reliably discards fingerprints stamped with the wrong clock. The
durable fix is a host or distro configuration change and has not been made.

Whichever way a skew runs, do not take a green WSL run at face value unless its
own output shows the changed crates compiling. That rule is in ADR-0007 as a
standing consequence, and it is what catches this class of failure rather than
just this instance of it.

**Rank order: WSL is the fast pre-check, the CI runner stays the authority.** A
green WSL run does not certify a commit and does not replace a green CI run. It
moves Linux-only failures — a build error, a `--locked` mismatch, a graphics
dependency creeping into the headless tree — from minutes later to seconds
later. ADR-0007 records where WSL can diverge from the runner and when that has
to be re-examined.

## CI troubleshooting

**Every push to `main` carries a CI run of its own.** `ci.yml`'s push trigger
filters on branch and nothing else — there is no `paths-ignore` — so a commit
touching only documentation is built and tested like any other.
`release-determinism.yml` is the workflow that filters by path, and its filter
is an allow-list of six entries rather than an ignore-list.

> **Corrected in U3 (24.08.2026), and marked rather than deleted because it was
> true when it was written.** The paragraph that stood here said a push changing
> only `ProjektPlan.md` or `Uebergabe-Amboss.md` carried no run, because
> `ci.yml` ignored those two paths — and so `gh run list` showing nothing for
> such a commit was expected rather than alarming. **U2 removed that filter**,
> on the reasoning that its two entries named files which had left this
> repository and a filter naming a path that cannot exist ignores nothing while
> reading as though it did. The paragraph here was not moved with it, so both
> facts it rested on had become false: there is no ignore list, and neither file
> is in this tree. **Nothing guards this** — `xtask`'s drift guards hold
> `release-determinism.yml`'s trigger list against this file and say nothing
> about `ci.yml`'s push filter — which is why it survived U2 and had to be found
> by reading.

- A failed job with no logs and an empty step list, conclusion `cancelled`,
  never ran. That is infrastructure, not a code fault. Do not debug it.
- GitHub cancels jobs that wait longer than roughly 15 minutes for a runner.
- On an infrastructure failure, rerun at most twice. Then check
  githubstatus.com and report what it says instead of retrying further.
- A rerun of an older run is cleared away by any new push to the same branch:
  the workflow's concurrency group cancels in progress.

## Workspace

| Crate | Owns |
|---|---|
| `crates/narvo-core` | time, fixed timestep, the frame loop and its phase timing, error types |
| `crates/narvo-ecs` | ECS facade over hecs, component registry, system scheduler, seeded RNG, events; since M6b.6 also a world's *future* handles — the free list, and rebuilding a world from an entity table (ADR-0043); since M6b.7 the `Burst` emitter and the closed form that computes its particles without storing one (ADR-0044) |
| `crates/narvo-render2d` | wgpu-based 2D rendering, and since M6.6b the glyph atlas and single-line text layout that feed it (ADR-0038; depends on core) |
| `crates/narvo-assets` | asset handles, loading, hot reload; since M6b.5 also which region names are the frames of one clip, as a declaration and not a player (depends on core) |
| `crates/narvo-input` | device vocabulary, RON action mapping, `InputEvent` (depends on nothing in this workspace) |
| `crates/narvo-audio` | cue vocabulary, synthesised sounds, null sink; the kira backend behind `device` (depends on nothing in this workspace) |
| `crates/narvo-physics2d` | 2D rigid bodies as bare scalars over rapier2d; rebuilt every tick, keeps no state between ticks (ADR-0029; depends on nothing in this workspace) |
| `crates/narvo-ipc` | the agent protocol as data: request and response vocabulary, their JSON spelling, their parse errors. No transport and no execution (ADR-0030; depends on nothing in this workspace) |
| `crates/narvo-scene` | two RON formats over a `World`, one mechanism and two contracts (ADR-0043): the **scene** an author writes — file order is spawn order, symbolic entity references, prefabs as hole and fill (ADR-0018, ADR-0021) — and, since M6b.6, the **save** a run produces, which names entities by slot and generation, carries the free list and the tick, and refuses a version it does not read (depends on `narvo-ecs`) |
| `crates/narvo-view2d` | the seam between a world and the renderer: the sprite and camera extraction — which since M6b.7 also draws a `Burst`'s computed particles, in the same draw order and through the same sort key (ADR-0044) — and the hit test that mirrors that order (ADR-0041; depends on `narvo-ecs` and `narvo-render2d`, and on nothing else). It moved out of `narvo-app` in M6b.1 — a binary has no lib target, so nothing outside could call it |
| `crates/narvo-testkit` | the workspace's shared test fixtures, in one place; `publish = false`, and a **dev**-dependency everywhere it is used, which is what closes the cycle back to `narvo-render2d` (ADR-0016). Since M6.6b it re-exports the text path rather than owning it, and keeps the CPU model a golden scene is checked against (ADR-0038) |
| `crates/narvo-app` | binary `narvo`: windowed and headless runner |

**This repository is the engine, and nothing else.** `games/forge-loop` was a
member of this table from M7.1 until U2 — the M7 slice, and the first consumer of
the engine living outside `crates/`. It now sits outside the repository as a
standalone crate with a path dependency on these crates, which is the same
relationship any other consumer has. The M7.1 report's census of what a consumer
outside the engine has to write for itself still holds; it is simply no longer
written from inside.

A typical task touches exactly one crate. If a change wants to span crates,
stop and report it — that is an architecture signal, not something to push
through. The exception that used to sit here went with the game: a task on the
game could not touch `crates/`, and a gap it found there was reported rather than
closed. That rule now needs no wording, because the game cannot reach `crates/`
except through the same public surface everyone else uses — which is what U2 was
for.

## Architecture rules

- The render path only *reads* simulation state. It never mutates it.
- Simulation stays deterministic: fixed timestep; seeded RNG only (no OS
  entropy, no wall clock inside sim logic); stable iteration order for
  anything that gets hashed or serialized.
- Content is text in the repo. No binary formats, no editor-only state. That much
  is settled. The concrete format is not: RON is the working assumption and the
  recommendation in D3, and D3 is only decided by M4 (`ProjektPlan.md` §11).
  Write RON today, and do not treat the choice as closed.
- New components and serializable types ship with serde support and a
  roundtrip test (applies once serialization lands, M2+).
- Error messages are agent feedback: precise, actionable, and worth testing.
- Headless builds must not depend on wgpu/winit/egui (feature-gated).
- Orientation (NDC, framebuffer, texture and image row order) is fixed by
  ADR-0004. Read it before any render work; a second, silent y-flip anywhere
  invalidates every golden image.
- The window and present path has no automated coverage, deliberately: the
  handover to the compositor is not observable below a compositor. Changes
  there need a human to look at the window. What *is* machine-covered is the
  surface configuration around it — `choose_format`, `choose_present_mode` and
  `choose_alpha_mode` in `narvo-render2d`, plus a test rendering the quad
  through a surface-typical BGRA format. Those catch a wrong format or a
  positional default; they cannot catch a frame that is never handed over.

## Definition of Done

1. Verification set green (all eleven commands, plus `cargo xtask whitespace`).
2. A machine-checkable verification path exists for the change: unit or
   integration test, determinism hash, golden image, or benchmark budget.
   "Looks right" is not acceptance.
3. Code, comments, identifiers, commit messages in English. LF line endings
   (enforced via `.gitattributes`). No licence: all rights reserved, none chosen
   yet, none granted — `LICENSE` at the root, and a guard in `xtask` that fails
   if a `license` field, an SPDX header or a licence file comes back.
4. Architectural decisions are recorded as an ADR in `docs/decisions/`,
   not left in chat history.

## Iteration budgets

Measured on the local reference machine; enforced from the milestone listed.
Baseline and measurement procedure: `docs/perf/BASELINE.md` (re-run at every
milestone close).

| Metric | Budget | from |
|---|---|---|
| Incremental debug rebuild (one crate changed) | < 5 s | M2 |
| Headless start to tick 1 | < 200 ms | M2 |
| Unit test run of a single crate | < 5 s | M2 |
| Window start to first frame | < 1.5 s | M3 |
| Full local verification run | < 5 min | M3 |
| Scene load (demo scene, dev mode) | < 100 ms | M4 |
| Asset change visible via hot reload | < 1 s | M4 |

## ADRs

Read the relevant ADRs before any architecture-touching task:

- `docs/decisions/ADR-0001-language.md` — Rust
- `docs/decisions/ADR-0002-ecs.md` — hecs behind an engine-owned facade
- `docs/decisions/ADR-0003-fixed-timestep-catch-up.md` — catch-up ticks are
  discarded, not deferred
- `docs/decisions/ADR-0004-orientation-conventions.md` — y axes and image row
  order across the render path
- `docs/decisions/ADR-0005-query-facade-seam.md` — the one place hecs is visible
  in a public API, and why queries need a borrow guard
- `docs/decisions/ADR-0006-ron-for-internal-serialization.md` — RON as the format
  the component registry serializes into, and why that is not the scene format
- `docs/decisions/ADR-0007-wsl-linux-precheck.md` — WSL 2 as the local Linux
  pre-check, where it can diverge from the CI runner, and why it is not an
  authority
- `docs/decisions/ADR-0008-state-hash-stability-domain.md` — what the state hash
  guarantees, and the rule that no hash value is ever committed; since M3.36 also
  the third kind of literal — a content anchor over a generated artefact, where a
  dependency moving the value is the finding rather than noise
- `docs/decisions/ADR-0009-no-cranelift-codegen.md` — debug builds keep LLVM;
  the rebuild budget holds and the toolchain pin outranks a nightly backend
- `docs/decisions/ADR-0010-seeded-rng-is-world-state.md` — the generator's state
  is a component and therefore in the state hash; the algorithm is written out
  rather than depended on, and lives in `narvo-ecs` rather than `narvo-core`
- `docs/decisions/ADR-0011-event-delivery-semantics.md` — an event sent in a tick
  is readable in the next one and in no other; the buffer is in the state hash
- `docs/decisions/ADR-0012-input-recording-format.md` — the recording is
  line-based text with only the ticks that carry input, holds no state hash, and
  the input source lives in the runner rather than in the world; amended in M5.2
  with D8's answer — the recording stays *above* the mapping, at the action
  level, so the device is outside the closure
- `docs/decisions/ADR-0013-cross-platform-determinism-holds.md` — M2's
  pre-registered kill criterion did not occur; cross-platform determinism stays
  a hard goal, stated as a named domain with its gaps listed. Re-examined at M5;
  amended in M5b.2 with the pre-measurement half of that re-examination — rapier
  16/16 identical, the criterion again not triggered, `enhanced-determinism`
  still mandatory but with the measured qualification that the probe could not
  show the flag to be what produces the agreement
- `docs/decisions/ADR-0014-bare-f32-in-serialized-components.md` — a registered
  component holds bare scalars; a maths library's type never enters the
  canonical dump, because its serde format would then govern every state hash;
  amended in M6.1 with the half that lived in a source comment — a discriminant
  is a scalar too, and the four RON spellings one serde attribute can choose
  between
- `docs/decisions/ADR-0015-renderer-takes-scalars-not-a-world.md` — the renderer
  takes an explicit buffer of scalars and never the ECS; the extraction from a
  `World` lives in the crate that already sees both sides
- `docs/decisions/ADR-0016-shared-test-fixtures-crate.md` — shared test fixtures
  live in a dev-only `narvo-testkit` crate rather than being copied per file;
  what the dev-dependency cycle back to `narvo-render2d` does to types, and why
  the headless `cargo tree` check needs `--edges normal`
- `docs/decisions/ADR-0017-camera-has-one-composition-point.md` — contributors
  own their state as components and exactly one system writes the camera, as
  `base + Σ offsets` from the state the tick left behind; the guard that
  recomputes it, and what that guard cannot catch
- `docs/decisions/ADR-0018-ron-scene-format.md` — RON as the scene format,
  component-open through the registry, the author's bytes reaching a component
  verbatim, and symbolic entity references resolved through a `refs` table
- `docs/decisions/ADR-0019-scene-identity-anchor.md` — a recording names the
  scene it was made against by path and SHA-256 of the file's bytes; the format
  grows additively rather than bumping its version; amended in M4.6 with why hot
  reload leaves the anchor alone
- `docs/decisions/ADR-0020-asset-contract-and-packer.md` — the atlas, the region
  table and the padding rule as one contract, deterministically packed, with the
  source exchangeable underneath it
- `docs/decisions/ADR-0021-prefabs-hole-and-fill.md` — a template is a one-entity
  body with holes an instance fills; a field belongs to one side or the other and
  a collision is a hard error, identified structurally rather than by message text
- `docs/decisions/ADR-0022-hot-reload-by-reconstitution.md` — a changed scene
  file reloads the world fresh rather than patching it; detection polls and
  hashes because `notify` is silently dead on the project's own WSL path; the
  swap is at a tick boundary and a failed load keeps the running world
- `docs/decisions/ADR-0023-premultiplied-blending.md` — one pipeline that blends,
  premultiplied `OVER` on both components, alpha from the texture alone; what the
  change did to the blessed stock, and that draw order is now composition
- `docs/decisions/ADR-0024-png-file-source.md` — PNG files as a second source
  under the M4.4 contract, premultiplied at load in byte space; `Sprite`, the
  eighth registered component, names a region rather than a rectangle; assets sit
  outside the recording anchor, so a replay guarantees simulation fidelity and
  not image fidelity; amended in M6b.6 with M6b.5's owed booking — the frame
  convention (`stem_<digits>` is a frame of `stem`) belongs beside Decision 5's
  file-stem rule rather than in a module header, and it marks no sentence above
  as overtaken because recognition is additive and that was checked before the
  code was written
- `docs/decisions/ADR-0025-input-mapping-crate-and-format.md` — `narvo-input` as
  a leaf crate depending on nothing in the workspace, `InputEvent` moved into it
  from `narvo-ecs`, a closed device vocabulary of the crate's own names, and a
  binding that says what it emits on each edge; D8 left open on purpose, in code
  as well as in prose
- `docs/decisions/ADR-0026-window-input-boundary-and-delivery.md` — exactly one
  function in the workspace names a winit type, so the delivery rule stays
  compilable and testable in the headless configuration; what a hot reload does
  to input in flight, and what happens when a world cannot receive input at all
- `docs/decisions/ADR-0027-hit-testing-and-the-input-buffer-for-scenes.md` — a
  click is world state: `HitRect` is a registered component holding an
  axis-parallel rectangle and the action name it sends, answered in draw order;
  the inverse of the render transform lives beside the transform, and a scene
  world gets its input buffer from the runner
- `docs/decisions/ADR-0028-kira-for-audio.md` — D2 decided by measurement against
  the Scope B profile with the rule registered before the numbers, in throwaway
  crates outside the workspace so `Cargo.lock` stayed untouched until the
  decision was made; kira 0.12.3 with the decoders switched off
- `docs/decisions/ADR-0029-physics-state-is-rebuilt-every-tick.md` — rapier's
  retained state carries behaviour and is not reconstructible from body scalars,
  so the facade rebuilds the world every tick and keeps nothing; what that costs,
  measured; the snapshot path reported rather than built because it would need to
  supersede ADR-0014
- `docs/decisions/ADR-0030-protocol-carries-registry-bytes.md` — a component
  crosses the agent protocol as the registry's own RON inside a JSON string, so
  no float ever reaches `serde_json` and the boundary is exactly as faithful as
  the canonical dump, its `NaN`-payload limit included; `narvo-ipc` names an
  entity with its own type rather than `EntityId` and stays a leaf; externally
  tagged, decided by the position the internally tagged representation throws
  away
- `docs/decisions/ADR-0031-two-answering-moments-and-the-gated-socket.md` — a run
  answers an agent after a tick and again while it waits, because the command
  that ends a wait takes effect *by being answered* and one moment plus a queue
  is a deadlock; the test that says both moments see the same world; `cut_to`
  replacing `cut_after` because the tick form could not express a cut at zero;
  the socket behind a default-off feature, from the measured fact that a loopback
  listener has no access check; and the duration-leak guard's reach, which is its
  own 250 ms delay and no finer
- `docs/decisions/ADR-0032-a-replay-answers-questions-and-takes-no-orders.md` — a
  run reproducing a recording answers every read and refuses every command that
  would change what it reproduces, in one check with the consequence as its
  parameter; measured first, because `--replay … --ipc …` was already a legal
  command line and a write over that socket produced a replay reporting a state
  hash that was not the recorded run's; why D19's band cut cannot report that
  case at all; and what it settles in advance for the commands that come next
- `docs/decisions/ADR-0033-one-framing-and-a-client-in-the-protocol-crate.md` —
  the line framing moves out of `Endpoint` into `narvo-ipc` and both ends call
  it, because a second implementation would be two framings at the two ends of
  one connection; the client half lives there too, so the crate is the protocol
  *and how to speak it*; the rejected placements priced at 185 packages against
  14, and the `std::net` behaviour the client rests on measured on both
  platforms — including that a peer closing with bytes it never read resets the
  connection rather than ending it, which arrived as a failing test
- `docs/decisions/ADR-0034-mcp-by-hand-against-a-named-revision.md` — MCP is
  written by hand in `tools/narvo-mcp` against the named revision `2026-07-28`,
  with no MCP dependency, because the surface is three methods and six error codes
  and the framing is already `narvo-ipc`'s; the rejected routes priced at 28
  packages and a twenty-fold clean build for `rmcp` 3.1.2 against one package that
  tops out two revisions back; and the answer to the taxonomy M6.3a deferred —
  MCP's two error mechanisms land on the two types that already existed, so the
  wire keeps carrying a sentence
- `docs/decisions/ADR-0035-a-repro-is-handed-over-not-committed.md` — a repro is a
  recording, a canonical dump of the state it produced and the command line that
  joins them, written under `target/` and handed to a human; nothing about a case
  is committed, because the expected value is simulation state and ADR-0008
  forbids exactly that literal. The oracle is a dump and never a hash, so the
  answer names the entity and the component; it is read before the run starts, so
  it cannot come from the run it judges; and the two verdict words share no
  substring, so an agent grepping for one does not match both

- `docs/decisions/ADR-0036-an-answer-carries-a-world-and-says-when.md` — the two
  posts M6.7a measured to be carrying the gap: a whole world crosses as
  `canonical_dump`'s own text, byte-identical to what `narvo --dump` writes and
  checked against that unmoved reference; and every answer that came from a world
  carries `ticks_run`, `Error` being the one that never did. A separate
  `get_tick` was refuted by measurement — four sequential reads of a mid-flight
  run came back with four different values, so a tick asked for separately is a
  tick from another moment; the 88 construction sites that cost are all compile
  errors, and the number's meaning is pinned by `--ticks` on the command line

- `docs/decisions/ADR-0037-no-licence-and-a-guard-that-keeps-it-that-way.md` —
  the `MIT OR Apache-2.0` grant M0 wrote is removed rather than exchanged, in
  the same five places that used to make it, because a permissive grant cannot
  be withdrawn from a copy published under it and D5's direction is incompatible
  with it; the root `LICENSE` is a notice that there is none. It decides nothing
  about what Narvo will be licensed under — that stays D5's, and a human's.
  `cargo deny` is told these crates are ours through `[licenses.private]`, which
  reads the `publish` field and nothing else, measured: `publish = true` on one
  crate alone brought exactly that one back as `error[unlicensed]`. The guard is
  a test in `xtask` rather than a verification step of its own (the set stood at
  ten when M6b.2b wrote this), since `xtask` is a workspace member and step 2
  already runs it; and it is not redundant with
  `cargo deny`, which reports `licenses ok` with the field put back

- `docs/decisions/ADR-0038-text-is-a-production-capability.md` — the glyph atlas
  and the layout move out of `narvo-testkit` into `narvo-render2d`, because the
  human decided the native text path over egui (v1.06) and a dev-only crate
  cannot be reached from a production build. The condition for the move was
  registered in the moving file's own header and had occurred; egui was priced
  first and measured at **105 packages** the lock does not have, against
  ADR-0034's 28. `model_image` and `over` stay behind — they are the CPU model a
  golden scene is checked against, and a normal edge back to `narvo-testkit` is
  the direction ADR-0016's cycle forbids. No package moves (346 → 346), the
  headless tree and `cargo deny list` are byte-identical either side, and the
  re-export is what kept every test file — and therefore the three references
  drawn through this path — unmoved; amended in M7.0, which moved `srgb` the
  same way and for the same reason and therefore spent the rejected
  alternative's first argument against moving `model_image` and `over` — the
  second one, a renderer grading its own homework, is untouched and holds them
  there alone, and the distinction is measured: no production code in
  `narvo-render2d` performs an sRGB conversion in Rust or in WGSL, so a
  prediction and the frame it is checked against stay on opposite sides of the
  GPU

- `docs/decisions/ADR-0039-a-frame-draws-two-batches.md` — a frame draws two
  batches in one pass, because a draw call binds one texture and an overlay needs
  a second; `encode_runs` already bound per run, so nothing in `quad.rs`, the
  blend state or the `LoadOp` moved. **Two, not `n`** — a list would be stock. An
  empty second batch **produces** nothing rather than drawing nothing, which is
  what keeps "the shared code emits the same command sequence" as the regression
  evidence for the ten references instead of "the overlay is off"; three tests
  hold it, on both sides of the trait. The batch limit counts the sum. One
  measured guard gap is recorded rather than closed: a bind group built for an
  empty batch is caught by nothing, and the fix would be production
  instrumentation existing only for a test; amended in M6b.5 with M6b.4's owed
  decision — a batch carries its own camera, as a **field** because two adjacent
  same-typed parameters are a swap nothing catches and an `Option` overlay would
  admit a camera without a batch; "screen-fixed" is `CameraView::IDENTITY` under
  `Projection::for_target` and is **centre-anchored**, with edge anchoring booked
  as a named limit belonging to M6b.8; the rejected camera-per-sprite carries its
  own best argument and a sharp reopening condition

- `docs/decisions/ADR-0040-a-screenshot-is-the-presented-frame.md` — a screenshot
  is a copy of the swapchain image between the encode and the handover, so it is
  the frame that was presented rather than a second render that agrees with it by
  argument; the surface asks for `COPY_SRC` only where the adapter offers it,
  because `wgpu` guarantees `RENDER_ATTACHMENT` and nothing else and an unoffered
  usage would cost the window rather than the screenshot. The copy is
  `offscreen.rs`'s own, moved out unchanged, and a new normalisation makes
  `Pixels`' RGBA promise true for the BGRA surface WSL's lavapipe measurably
  offers — the oracle for that needs no window and is byte equality between the
  two formats, with its own limit measured: a defect symmetric in both is
  invisible to it. Nothing in `quad.rs`, the blend state, the `LoadOp` or the
  drawing order moved; the ten references never build a `WindowTarget`

- `docs/decisions/ADR-0041-the-seam-is-a-crate.md` — the extraction and the hit
  test move out of `narvo-app` into `narvo-view2d`, because ADR-0015's second
  revision condition occurred: M6b.0's probe was the second consumer, and a
  binary crate has no lib target to reach. **One crate and not two**, because the
  obvious cut along the dependency graph runs straight through `depth_order`,
  whose single copy is what stops a click landing on the sprite that is not in
  front; the price — a consumer wanting only `hit_test` carries the graphics
  stack, 147 packages against `narvo-ecs`'s 16 — is named with the condition
  that reopens it. `count_actions` stays `pub(crate)` because a tally is the
  demo's system and not the seam's, and that is what forces the two golden tests
  to stay unit tests in `src/`; `assets.rs` stays knowingly and belongs to M6b.2.
  The rejected candidates carry their best argument: (a) fell at 327 packages
  against 147 and 141 dead-code errors, (c) at a **measurement** rather than a
  quotation — ADR-0015's revival number is not met, the copy is 14.6 µs of
  1 025 µs and the double enumeration dominates — and (d) at verification step 9,
  which was made to fire on purpose. **No guard was added** to
  `FORBIDDEN_IN_HEADLESS`: step 9 already catches the boundary through the four
  crates a bypass drags in, measured

- `docs/decisions/ADR-0042-a-sound-is-named-by-a-handle.md` — a cue names its
  sound with a handle only the registry can issue, so a name that resolves to
  nothing is unwritable rather than merely unplayed. **This entry was missing
  from this list from M6b.2b until M6b.7**, reported at the time rather than
  fixed, and filled in by the first task that had reason to open this file

- `docs/decisions/ADR-0043-a-save-is-not-a-scene.md` — a save and a scene share
  one mechanism, the registry's own RON, and are two contracts: a scene is
  written content that grows additively with git as its version (ADR-0018
  Decision 6), a save is produced state that outlives the build that wrote it and
  grows by a version it refuses when unknown. A save carries three things a
  canonical dump cannot show — every live handle, **the free list**, and the
  tick — and the middle one is the finding: a world rebuilt from its live handles
  alone has a byte-identical dump and hands out different slots at different
  generations from its next spawn, measured against `hecs` 0.11.1 in both
  directions. A failed load cannot touch the running world **structurally** —
  `World::reconstitute` and `save::from_str` are handed no world, so there is
  nothing to damage — and a save that dropped its free list is not a wrong file
  but an unreadable one, because the entity table has to account for every slot
  below its highest. The version is asked for by a *second, tolerant* pass only
  after the strict one fails, so a save from a later build is refused by its
  version rather than by whichever new field came first. Candidate (c), the scene
  file with a version field, is stood up at full strength — it saves a whole
  format — and falls on the fields a save needs being fields a scene must not
  have, since ADR-0018 Decision 2 says nothing in a scene names a slot.
  Meta-progression is explicitly not decided, and the halt branch did not fire

- `docs/decisions/ADR-0044-a-world-stores-the-emitter.md` — D22's answer:
  particles **are** simulation state, and the state is the *emitter* — six bare
  scalars in one registered component, constant in the particle count, with
  `Burst::particle` computing where each spark is. The plan's counter-argument
  was measured and does not hold at two thousand: one entity per particle grows
  the dump by **5.55 ×** the largest this repository produces, not by two orders
  of magnitude, and the whole repro cost of that growth is 3.7 ms on a 76.85 ms
  baseline — so the rule registered before the numbers **excluded nothing**, and
  the decision fell to §2's rule against building on spec. The emitter carries
  its **age** and not a birth tick, because M6b.6 measured that the tick is not
  world state and deriving a picture from it would be candidate (d) rebuilt; the
  age saturates, so a finished burst stops moving the hash. Draw order stays in
  the one place that decides it: `regions_of` sorts by *(depth, entity, slot)*,
  which degenerates to the old key for every world without a burst.
  Candidate (b) falls on a measurement — one wrong particle of two thousand is
  reported as **81 723 bytes on one line** — and each rejected candidate carries
  its best argument and a sharp reopening condition. The price is named in a
  table: a computed particle cannot be clicked, addressed, collided or killed on
  its own. The halt branch did not fire, and no tint above 1.0 is reached

- `docs/decisions/ADR-0045-widgets-are-not-an-engine-concept.md` — the bill for
  v1.06's egui decision, measured by building first and counting after: a probe
  outside the tree built a hover-and-press button, a bordered panel, a progress
  bar and an abbreviated big number **with zero engine changes**, and **nothing
  was blocked**. So no widget type is added and §2's building-block direction
  survives its own test — `hit_test` is the whole of what hover needs, and the
  27 lines a `Hover`/`Press` component pair would have replaced were measured to
  be mostly tint values, which are policy. What *was* missing is one function:
  `Projection::anchor` turns a corner and an inward inset into a world point,
  closing the limit ADR-0039 booked here and correcting its word for it — edge
  anchoring is **not** layout, it holds no state and owns no tree. It shares
  `screen_to_world` rather than restating the arithmetic, and pays a measured
  rounding for that (`8.000004` for `8.0` on a 192-wide target) rather than
  letting a second copy drift from the click path. The estimate registered before
  the survey was 220 production lines against a measured **50**, below its own
  band — small because the bill was paid in instalments by M6.6b, M6b.1, M6b.3
  and M6b.4, which is the honest reading of what v1.06 overestimated. No package
  moves: an external HUD consumer carries the same **99** M6b.5 and M6b.7 record

- `docs/decisions/ADR-0046-the-window-picks-its-own-backend.md` — the window's
  `wgpu::Instance` asks for DX12 on Windows and the offscreen one is left alone,
  which is possible because there were **two instances all along** and only the
  second draws anything a test compares. M7.1c had rejected the fix on the
  assumption that a backend move would carry the twelve blessed references with
  it; the census refutes that, and all twelve report `deviation 0 counts` either
  side. Fifteen blocked frames become zero and the game's worst `acquire` over
  1 400 frames falls from 2012.763 ms to **33.107 ms** — one dropped frame, not
  "fixed". The price is named: **two of twenty-four** DX12 runs died in a
  validation panic whose trigger is unknown, and `WGPU_BACKEND` is the escape
  from either side. Three guards, of which the load-bearing one reads
  `offscreen.rs`'s source, because the leak that threatens the references is one
  edited line and every image comparison is blind to it — both sides would move
  together. The runtime choice is **unguarded** and that is recorded rather than
  fixed: forcing the window back onto Vulkan leaves 1371 of 1371 green

- `docs/decisions/ADR-0047-the-engine-is-renamed.md` — D30's rename, recorded
  where it stays findable: **this is the one file in the engine repository where
  the old name is written on purpose**, so a decision does not become unsearchable
  the moment the name it replaced leaves everywhere else. Three candidate
  approaches fell first and the rejections are written down so they do not recur,
  and the proof that a rename of fourteen crate directories, every manifest, every
  import and both workflows changed nothing is the twelve unmoved references.
  **This entry was missing from this list from U3 until M8.2**, the same way
  ADR-0042's was from M6b.2b until M6b.7, and it is filled in here by the next
  task that had reason to open this file

- `docs/decisions/ADR-0048-the-offscreen-path-names-its-adapter.md` — ADR-0046
  settled the window's backend and said in its title that the references keep
  theirs, which left the offscreen side decided by nobody: `select_adapter`
  compares no vendor, device or name and returns the first rung that answers, and
  `offscreen_backends` returned `Backends::default()`, which is `all()` and so
  carries wgpu's own second tier — `SECONDARY` is exactly `GL`
  (`wgpu-types-30.0.0/src/backend.rs:132-136`, `:140-144`). Harmless while every
  reference is an 8-bit image three rasterisers draw with `deviation 0`; an error
  from the first GI reference, which is a field of `f32`. Two changes and a named
  third that was **not** made: the set narrows to `PRIMARY`, and `summarize`
  carries the adapter's `vendor:device`, so a run can *say* what produced it —
  while selecting by id is refused, because the list would have to be right on
  every runner and the next machine. GL was the only backend measured to disagree
  with the others **on the same machine**: it refuses `Rg32Float` as storage where
  Vulkan and DX12 accept it, and lavapipe's GL refuses `Rgba32Float` as a render
  attachment where its own Vulkan accepts it. The cost was measured *before* the
  change and is nil — same adapter on both platforms, screenshot byte-identical at
  `sha256 f598a3a2…`; the real price is at the edge, where a GL-only machine now
  gets a loud `NoAdapter` instead of a quiet substitution

- `docs/decisions/ADR-0049-a-chain-is-passes-over-two-fields.md` — the multi-pass
  compute path M8.3–M8.6 consume, and it is four pieces because each of the four
  has a named consumer. **A general render graph was rejected** (v1.51): nothing
  here would exercise its generality, so it would be measured against a toy;
  reopens at a third consumer with a different pass structure, in practice the
  first 3D slice. The format is `Rgba32Float` and the usage set has four flags,
  both measured on eight adapter/backend pairs rather than quoted from the
  specification — `Rgba8UnormSrgb` carries no `STORAGE_BINDING` at all, `Rg32Float`
  is refused on both GL adapters, and adding `RENDER_ATTACHMENT` (which no
  consumer needs) is what makes `Rgba32Float` refused under lavapipe's GL. The
  rule it exists to state is M8.5's: **a merge may not be written
  order-dependently** — an atomic CAS reduction was reproducible in **0 of 32**
  cells while a barrier-and-shared-memory tree and an order-independent control
  were reproducible in 32 of 32 *and returned one value across every backend,
  adapter, platform and profile*. The oracle is a pattern with negative channels
  that no draw path can produce, moved through a kernel written in `+` alone, and
  three injections were shown red — including one where the arithmetic guards fell
  while the pass **count stayed green**, which is why both exist. ADR-0039 is
  amended rather than superseded, on a measurement: after M8.2 a frame still draws
  exactly two batches, because nothing records a compute pass yet

- `docs/decisions/ADR-0050-a-comparison-over-coordinates-is-written-in-integers.md`
  — M8.3a measured this and **reported it rather than writing it**, on its brief's
  instruction; M8.3b wrote it, which is why it carries a later date than the
  measurement it records. A comparison over coordinates is written in `i32`, and
  the decision is the *bound* rather than the reasoning: M8.2's magnitude defence
  of `f32` is correct at M8.2's sizes and at every size the tests use — six of
  seven arrangements were **byte-identical under both variants** — and it breaks
  at `MAX_DIMENSION`, which is a limit this crate publishes. Measured over 448
  chain executions: `f32` names the wrong seed from a squared distance of 2^24 up
  (first wrong texel at x = 4096 on all eight adapter/backend pairs) and, worse,
  is wrong **differently per rasteriser family** — 3 248 texels on the three AMD
  paths against 4 096 on the three software ones, one input and two fields —
  while `i32` returned one field in 16 of 16 cells. Its guard **has to be a source
  read**, and that shape follows from the same measurement rather than from taste:
  no output comparison can see the difference below the magnitude at which `f32`
  stops being exact. M8.3b showed it red against an `f32` injection and **14 of
  the 15 tests, every GPU oracle among them, stayed green**. Reopens at a value
  that cannot be an integer, which is M8.5's radiance

Decisions that have *not* been made live elsewhere: `ProjektPlan.md` §11 holds
the D table — the open questions, the recommendation for each, and the milestone
it is due by. Check it before treating anything as settled. An ADR records a
decision that was taken; the D table records one that was not, and a
recommendation there is not a decision.

The finished milestones are no longer written up in the plan: since v0.67 the
§12 bookings, the changelog entries and the §6 detail prose of M0–M4 live in
`docs/history/`, moved there line for line, and §12 keeps only the running
block.

**Neither of those lives in this repository, and no reference to them is a
link.** `ProjektPlan.md`, `docs/history/` and `docs/design/` moved to a
private plan repository in U2, and D32 settled in v1.56 what happens to the
references left behind: the links fall, the prose stays. U3 measured the
split before cutting — **211 references, of which exactly one was a Markdown
link** (`README.md`, removed there) and 210 are prose citing a file by name
inside a code span. Those 210 stay, and they are deliberately dead ends for a
reader outside the private repository: a named source for a decision is worth
more than a decision with no provenance. The precedent is beside them — the
references to `target/reports/…` have been dead on purpose since ADR-0035,
because a repro is handed over and never committed.
