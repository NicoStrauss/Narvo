# ADR-0022: Hot reload reconstitutes the world, and polls to notice

Status: accepted · Date: 2026-08 · Scope: `narvo-app` (the window runner, the
scene watcher), and the closing criterion of `ProjektPlan.md` §6/M4

## Context

§6/M4's closing criterion is that a change to a scene file appears in the running
game in under a second, without a restart. M4.3 gave the runner a scene-file
mode, headless. M4.6 gives the *window* one and makes the file live.

## Decision 1 — reconstitution, not patching (recorded)

Decided in §6/M4 and implemented here: a change reloads the scene **fresh**. The
process and the GPU context live on — the window, the swapchain, the device and
the atlas are untouched — and the runtime state is gone.

The rejection is §6/M4's own and is reproduced because an ADR that points at a
plan is not a record. **Rejected: patching the running world's state.** Its best
argument is the editor feeling — tuning a value without losing the situation you
were looking at. Against it: an editor is a stated non-goal (§2); entity identity
across reloads and the diff semantics that would define a patch are open
determinism fronts; and for the M7 slice the state worth keeping is in a save
anyway.

The swap is one assignment of world and scheduler into the host, so it cannot be
seen half-done. The tick counter restarts, because a reconstituted world is a new
run rather than a continuation.

## Decision 2 — polling, not `notify`, on a measurement

The plan line said "Hot-Reload über `notify`". M4.6 was given leave to revise it
with numbers, and did.

**The deciding measurement.** `notify` selects the Inotify backend on Linux. On a
`/mnt` path — drvfs, which is where **this repository's entire Linux workflow
lives** (CLAUDE.md puts the working copy at `/mnt/d/Narvo`) — it delivered **no
event at all** within three seconds for a write that had definitely happened.
The control: the same file's `mtime` and length *did* change, and its contents
were readable. A watcher that is silently dead in the configuration the project
prescribes is worse than no watcher, because nothing announces it.

The rest of the ledger, secondary to that: `notify` costs two crates new to this
workspace (`notify`, `notify-types`; the rest of its tree is already here) and
2.1 s of clean build, and would pull a fourth `windows-sys` version into a lock
file that already carries three.

**What is compared is the contents, hashed** — not `mtime` and length. The
cheaper design has a hole that this module's own first test fell into: `"one"`
and `"two"` are the same length, and two writes inside the file system's
timestamp resolution carry the same `mtime`, so the change is invisible. Editing
one character and saving twice quickly is not exotic. At a two-kilobyte scene,
five times a second, against a window redrawing sixty times in the same second,
hashing every poll is not a cost worth a blind spot.

**The interval is 200 ms**, a fifth of the criterion, which leaves the load and
the swap the rest of the budget.

## Decision 3 — coalescing, and a torn file is the normal case

An editor writes a file in pieces and often more than once. So a change is
reported only once the file has **stopped changing** for one interval: a new
digest starts a wait, and only a second poll seeing the same digest settles it. A
read that fails outright — the file is being replaced, or is briefly gone — is
treated the same way, as "not settled", and retried.

That makes a burst of writes one reload rather than one per write, and makes a
half-written file unobservable to the reload path. It is not error handling
bolted on; it is what the mechanism is.

## Decision 4 — the swap is at a tick boundary, and a failure keeps the old world

The window's frame callback runs zero or more ticks inside `FrameLoop::step`, so
the reload check sits **before** that call: between the last tick of the previous
frame and the first of this one.

**A scene that does not load is logged and the running world continues,
unchanged.** §7 is explicit that a component may produce a wrong picture and must
never stop the simulation, and a half-saved scene is the ordinary case rather
than the exceptional one. The message carries the position M4.2 gives it. The
text that failed is *accepted* by the watcher afterwards, so the same broken file
is not re-judged five times a second; the next different text is tried.

## Decision 5 — the anchor belongs to the band, the watcher to the window

M4.3's `Anchor::read` refuses an absolute path, so that a **recording** travels
between machines. The window records nothing, and going through the anchor to
open a scene made that rule forbid an ordinary way to point at a file. So the
window reads the file directly, and the anchor stays what ADR-0019 made it: a
recording's statement about the file it was made against.

## Decision 6 — the cut rule, decided and not built

Decided: if a recording is running when a reload happens, the reload **finalises**
it — a valid, replayable band up to the cut tick — and does **not** resume
recording afterwards. The message names the cut and why.

*Rejected: the reload as an event in the band.* A replay would need both versions
of the scene file, and the first is the one the reload just replaced.
*Rejected: restarting the recording automatically.* That creates artefacts nobody
asked for.

**It is not built, and that is the honest state.** The survey found that the
window runner cannot record — recording lives in the headless runner — and the
headless runner does not watch. **There is no site today at which a recording and
a watcher coexist**, so a cut rule would be a mechanism with no caller, which
§2's no-stockpiling rule excludes. The decision is recorded here so that whoever
gives the window a recorder implements it rather than inventing a different
answer. That is its revision condition.

## Decision 7 — the display rule, and how narrow it is

A scene cannot say which region of an atlas an entity draws: that is the M4.4
contract's job, and referencing it from a scene is explicitly **the next step**.
So the window derives an appearance from world data alone — a sprite's position
in draw order, which `placements_of` fixes by depth and then by entity id, picks
one of the four quadrants of the existing demo texture, each a solid colour.

Enough for the one thing a picture is needed for here: seeing a reload happen.
No component, no format surface and no asset path was invented to get it. The
sprite-field demo is untouched and still draws the whole texture.

## Consequences

- **The watcher runs only while frames are drawn.** The poll lives in the frame
  callback, so a window that stops being asked to redraw stops noticing changes.
  Acceptable — a window nobody is looking at is not a window anybody is editing
  for — and named rather than discovered.
- **Reload is window-only.** `Watcher::new` is called in exactly one place, and
  headless never mentions it. A replay therefore cannot reload: the file is
  pinned by its anchor and the band dictates, which is what ADR-0019 already
  guaranteed.
- **A reload restarts the tick counter**, so anything derived from tick number
  starts over. That is what reconstitution means and is not a defect.
- **`--scene` selects the scene-file mode**, so `narvo --scene x.ron` opens a
  window on it. Naming `--mode scene-file` as well is still allowed and still
  implies `--headless`, which is what a recording's header needs.

## Measured

| | |
|---|---|
| load of a specimen-sized scene | under a millisecond, asserted under 10 ms |
| write → swap, windowed, measured runs | **220 ms, 259 ms, 270 ms** |
| criterion | 1 s |

The three parts of the budget are: up to one interval before the first poll sees
it (≤ 200 ms), one interval to settle (200 ms), and the load and swap (under a
millisecond). The floor is therefore about 200 ms and the ceiling about 400 ms,
which is what the measurements show.

## Revision condition

Reopen when the window can record, which is what creates the cut rule's site.
Reopen if a scene ever pulls in a second file, because the watcher watches one —
that is ADR-0019's multi-file question and is answered there. Reopen if the poll
interval ever becomes measurable against a frame, which at a two-kilobyte file it
is not.
