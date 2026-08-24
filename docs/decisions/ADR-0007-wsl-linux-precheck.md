# ADR-0007: WSL 2 is the local Linux pre-check, not the authority

Status: accepted · Date: 2026-08 · Scope: local development on Windows hosts

A record of a decision taken, not a comparison of options.

## Context

M2 carries a pre-registered kill criterion over Windows ↔ Linux float
determinism (`ProjektPlan.md` §6, M2): if cross-platform determinism cannot be
held at acceptable cost, the goal is reduced by ADR to per-platform determinism.
Deciding that needs Linux results, repeatedly and quickly.

Until now Linux existed in this project only as a GitHub Actions runner. That is
the wrong instrument for the job twice over: its round-trip time is minutes, far
outside the iteration budgets of §8.1, and it is not always reachable — the M2.1
verification sat unconfirmed across four commits during an Actions outage.

## Decision

WSL 2 (Ubuntu 24.04) runs the verification set on Linux locally, on the same
working copy under `/mnt/d/Narvo`. No second clone: two copies of a repository
are a source of divergence with nothing to show for it. Git stays on the Windows
side; WSL builds and tests, it does not commit, push or fetch.

Build artifacts go to a Linux-native directory inside the distro
(`$HOME/.cache/narvo/target`), configured in the distro's own
`~/.cargo/config.toml`. Sharing `target/` between the two platforms would make
each invalidate the other's artifacts, turning every platform switch into a full
rebuild.

The toolchain is pinned to an exact version in `rust-toolchain.toml`, so both
platforms compile with the same compiler rather than with two things both called
"stable".

**The rank order is part of the decision: WSL is a pre-check, the CI runner is
the authority.** A green WSL run does not certify a commit and does not replace
a green CI run. It moves Linux-specific failures — a build error, a `--locked`
mismatch, a headless dependency creeping back in — from minutes-later to
seconds-later.

## Where WSL can differ from the runner

Named rather than glossed over, because a pre-check whose divergences are
unknown is a pre-check nobody can calibrate:

- **glibc.** Ubuntu 24.04 here, `ubuntu-latest` on the runner. They track each
  other but are not pinned together, and a glibc difference is exactly the class
  of thing a `--locked` build does not catch.
- **Kernel.** WSL 2 runs a Microsoft-built kernel, not the distribution's.
  Anything touching syscall behaviour, timers or the scheduler sees a different
  kernel than the runner does.
- **Filesystem across the Windows boundary.** Sources are read through drvfs
  from `/mnt/d`, which differs from a native filesystem in case sensitivity,
  permission bits, inode semantics and timestamp resolution. Only the artifacts
  live on the Linux side.
- **GPU stack.** The runner pins Mesa's lavapipe as the Vulkan ICD so the
  adapter is identical from run to run. WSL additionally exposes a
  D3D12-backed path through WSLg, so the adapter has to be pinned deliberately
  here too or golden images are compared against a different rasteriser.
- **CPU.** WSL runs on the same physical machine as the Windows build. That is
  a feature for the determinism experiment — it holds the hardware constant and
  varies only the target platform, which is the comparison M2 actually wants —
  and a limitation for everything else: it cannot surface a difference that only
  appears on the runner's hardware.
- **The system clock, on both sides.** *(Added 2026-08-07 from the M2.2
  incident.)* Cargo decides freshness by mtime, and the two platforms write
  their mtimes with two clocks. On this host the **Windows** clock was two hours
  behind, so source files Windows wrote looked *older* than Linux artifacts
  stamped with the correct time, and WSL treated a changed crate as up to date:
  a run reported green having built code from the previous milestone. Nothing
  was visible on the Windows side, because there sources and artifacts share the
  same wrong clock and stay consistent with each other. It surfaced only because
  a manifest change happened to force a dependent to rebuild.

  The direction matters and was first recorded the wrong way round — as the WSL
  clock running ahead. The offset had been measured correctly; the cause was
  attributed to the wrong side. Either skew produces the same symptom, so the
  rule is symmetric: **both clocks belong to the environment and are compared
  before a run is believed.** `CLAUDE.md` carries the command and the workaround.

For pure computation — the fixed timestep, the ECS, the deterministic
simulation, everything M2 is about — none of these are in the path, so a WSL run
of that work is a usable pre-check.

What that sentence does *not* say, and did until 2026-08-07: that the pre-check
carries in general. What had actually been demonstrated at M2.1e was a WSL run
after a fresh build. The incremental case — the one an agent hits on every
iteration, and the one the clock skew above hides in — was verified only from
M2.2b onwards, by watching real compilation happen in the run's own output
rather than by assuming it had.

**Re-evaluate from M5.** rapier2d brings a physics solver whose determinism
depends on floating-point codegen, and audio brings a device stack; both are
places where "same machine, different kernel and different libc" stops being
obviously irrelevant. The judgement above is scoped to what exists now and is
not a standing exemption.

## Consequences

- Two target directories exist for one working copy. Disk cost is duplicated;
  the alternative is a full rebuild on every platform switch.
- `rust-toolchain.toml` applies to Windows as well. Both platforms resolve to
  the same exact version, which is the intent, at the price of rustup
  materialising that version as a toolchain of its own next to `stable`.
- The Linux tooling — cargo-nextest, cargo-deny, clang, lld, the Vulkan ICD —
  has to be kept in step with what `.github/workflows/ci.yml` installs by hand.
  Nothing checks that automatically; it is a documented manual duty in
  `CLAUDE.md`.
- A failure seen only in WSL is still a real finding. A pass seen only in WSL
  is not a result.
- **A verification tool is demonstrated in the incremental case, not only after
  a fresh build.** *(Added 2026-08-07.)* A standing rule, and the general lesson
  of the clock incident rather than a note about clocks. The fresh-build case is
  the one where every tool looks correct: everything is rebuilt, so nothing can
  be stale, and a tool that silently skips work is indistinguishable from one
  that does it. The failure hides in the ordinary case — the one that runs
  hundreds of times between milestones — so that is where a tool has to be shown
  working. Concretely, a WSL run is believed only when its own output shows the
  changed crates compiling.

  This joins the rule the golden-image suite produced in M1: a tool has to be
  shown failing as well as passing. The two together are the same requirement
  seen from different sides — that "green" is only worth something when it was
  possible for it to be red, in the conditions the tool actually runs in.

## Revision condition

Reopen at M5, when physics and audio arrive, and immediately if a divergence
between WSL and the runner is ever observed on the same commit — that would make
the pre-check misleading rather than merely incomplete, and the divergence
itself would be the more interesting finding.

## Amendment, 2026-08-11 (M5.7): re-evaluated at M5, and the answer is status quo

The revision condition above says *"Reopen at M5, when physics and audio
arrive"*. M5 is closed, so this is that re-examination. **The decision stands
unchanged: WSL is the fast local pre-check, the CI runner stays the authority.**
Nothing structural is recommended.

Recording it rather than leaving the trigger silently unfired is the point.
ADR-0019's M4.6 amendment set the precedent that a re-examination whose outcome
is "no change" still gets written down, because the next reader otherwise cannot
tell a considered answer from a forgotten one.

### The trigger fired half-way, and the half that fired argues for the decision

The condition names two arrivals. **Audio arrived** (M5.6–M5.6d). **Physics has
not** — rapier2d is M5b, still ahead — so the floating-point-codegen half of the
worry is untested and stays open. This amendment answers the audio half only,
and the physics half keeps its trigger.

Audio was expected to be the harder case: *"audio brings a device stack; both are
places where 'same machine, different kernel and different libc' stops being
obviously irrelevant."* It turned out to strengthen the pre-check instead, for a
reason worth stating precisely — **the device stack never enters a test.**
ADR-0028's null backend and the `device` feature gate mean the audio path
compiled in every configuration this pre-check builds opens nothing. What WSL
exercises is the *link* dependency, not the device.

### What the evidence since M2 actually says

- **ALSA parity, and it is the first system-header dependency this project has
  had.** `cpal` links against ALSA on Linux, so M5.6c added `libasound2-dev` to
  the `ubuntu-latest` job and M5.6c2 needed it locally. Both sides were measured
  rather than assumed, and they agree: the runner installed
  `libasound2-dev:amd64 (1.2.11-1ubuntu0.3)` and reported
  `pkg-config --modversion alsa` → `1.2.11`; the distro here reports
  `Installed: 1.2.11-1ubuntu0.3` and the same `1.2.11`. A new class of
  divergence appeared and closed at parity on its first measurement.

  It is worth naming what that does *not* prove: the two were equal on one day
  because both track Ubuntu 24.04's archive. Nothing pins them together, so this
  belongs beside the glibc bullet above rather than replacing it — the same
  class, now with one measured instance instead of none.

- **drvfs still delivers no inotify events, and the product no longer cares.**
  M4.6 measured it: notify 8.2.0 on the `/mnt` path selected the inotify
  backend, subscribed, and reported no event in three seconds and no error.
  ADR-0022 then chose settle-polling, which sees the same change immediately.
  So the divergence is designed around rather than tripped over — the watcher a
  WSL run exercises is the watcher the product ships. **The limit is unchanged;
  its bite on the product is gone.**

- **The same-CPU limit is unchanged and is covered elsewhere.** WSL cannot
  surface a difference that only appears on the runner's hardware, and nothing
  since M2 has altered that. It is covered by the two-runner comparison in CI
  rather than by anything local, which is precisely why this ADR calls WSL a
  pre-check and not an authority. *(The §7.3 coverage claim is read from
  `ProjektPlan.md`; this amendment did not re-derive it.)*

- **The clock rule got a correction that made it cheaper, not weaker.** The
  comparison is now made zone-bearing, after M5.2 found that an apparent
  two-hour jump was a difference in how the two sides *displayed* a time rather
  than a skew in the time itself. The rule survived; the false positive did not.

- **The pre-check keeps catching real things.** The formatting slip M4.5 found
  is the example the plan records. *(Read from `ProjektPlan.md` §12; not
  re-derived here.)* M5.7's own run is a smaller instance of the same: the
  compile evidence a WSL run prints is what tells a green run about new code
  apart from a green run about old code.

### One methodological consequence, from M5.6c2

A WSL run's value depends on being able to *read* it. M5.6c2 piped a run through
`tail -30`, which discarded every `Compiling` line and produced a result
indistinguishable from a stale build — the failure this ADR's own "watch real
compilation happen" rule exists to prevent, reintroduced by the capture rather
than by the build. M5.6d captured the full output to a file instead and the
evidence survived. **Redirect the whole run; a missing line and a missing
rebuild look identical.**

### What would change the answer

The physics half of the original trigger, unchanged and still ahead: a solver
whose determinism depends on floating-point codegen is the case where "same
machine, different kernel and different libc" stops being obviously irrelevant,
and M5b is where that gets tested. The standing condition — reopen immediately if
WSL and the runner ever disagree on one commit — is untouched and has still never
fired.
