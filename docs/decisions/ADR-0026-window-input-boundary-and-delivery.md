# ADR-0026: One translation boundary, and what a window's input does at a tick edge

Status: accepted · Date: 2026-08 · Scope: `narvo-app` (`input`, `window`, `cli`,
`frame`), and every later runner that feeds a world from a device

## Context

M5.1 built the mapping (ADR-0025) and M5.2 decided that a recording stays above
it (ADR-0012's M5.2 amendment). M5.3 gives the mapping its first consumer outside
a test: winit → a table → `Mapping::map` → the ECS event pipeline, in the window
runner.

Most of what that needs was already decided. **The delivery semantics is not a
new decision and this ADR does not make one** — it is ADR-0012 Decision 5 applied
unchanged at the window's seam, and saying so is the finding rather than a
shortcut. What *is* new is three things with no existing home, which is why this
is a new ADR rather than an amendment: where winit is allowed to be visible, what
a reload does to input in flight, and what happens when a world cannot receive
input at all.

## Decision 1 — exactly one function sees a winit type

> **Overtaken in M6b.8 (D23, Weg C) — the number, not the rule.** The sentence
> below is left exactly as written. M6b.1's survey measured **seven** functions
> naming a winit type in a signature, not one, and M6b.8 re-measured the same
> seven and added none. What the amendment keeps is the boundary — *one place*,
> `input.rs` and `window.rs` — and what it discards is "one function". See
> **Amendment (M6b.8)** at the foot of this document.

`crate::input::translate` is the only place in this workspace that names a winit
type in a signature. Everything downstream of it speaks `narvo-input`'s
vocabulary: `Control`, `Edge`, `DeviceEvent`.

**This is the acceptance criterion of the design, not a tidiness preference.**
`ProjektPlan.md` §7 has no way to verify anything that needs a window, so a
second winit-aware place would put the delivery rule permanently out of reach of
the test suite. With one boundary, `InputFeed` — which owns the rule — is
compiled and tested in the headless configuration against synthetic device
events, and the module is gated `#[cfg(any(feature = "render", test))]` exactly
as `watch` is and for the same reason.

The split is measurable: of the module's seventeen tests, **eight run in the
`--no-default-features` configuration** and the nine that need `KeyCode` do not.

### The boundary is two functions, and winit is why

`translate` takes a `winit::event::KeyEvent` and reads three fields off it;
`device_event(key, state, repeat)` does the work. That looks like indirection for
its own sake and is not: **a `KeyEvent` cannot be constructed outside winit.**
Its `platform_specific` field is `pub(crate)` (`winit-0.30.13/src/event.rs:654`)
and the module holding that field's type is private (`src/lib.rs:213`), so no
test can build one — and on Windows the type has no `Default` either. Splitting
puts every branch in the half a test can reach and leaves three field reads in
the half it cannot.

### The table is data, and the names are a convergence rather than a dependency

`TABLE` is a slice of `(KeyCode, Control)` rather than a `match`, so that
completeness and collision-freedom are properties a test can walk. All
twenty-one entries are a straight rename, and that was checked against the
vendored source rather than assumed: `narvo_input::Control` follows the W3C UI
Events `code` convention by ADR-0025's own decision, and winit's `KeyCode`
documents itself as conforming to the same specification
(`winit-0.30.13/src/keyboard.rs:285-292`). Two independent choices of one public
convention.

**The gap is named rather than closed.** winit spells the numeric keypad with
variants of its own — `Numpad0` … `Numpad9`, `NumpadEnter` — which are not the
digits and not `Enter`, so a numpad key produces nothing. That follows ADR-0025's
deliberately small vocabulary and is pinned by a test so it cannot become a
silent alias.

**Auto-repeat is dropped here**, which discharges an obligation `narvo-input`
states and declines: a pure function over a slice cannot know whether a press is
a repeat, and "whoever reads the device knows". winit hands the answer over as
`KeyEvent::repeat`. Letting repeats through would re-fire an `OnPress` binding
for as long as a key was held — a click that buys one upgrade would buy thirty a
second.

**Escape is not routed through the mapping.** It closes the window, and it is the
runner's key rather than the game's: a binding that captured it would take away
the only way to close a window that is not responding.

## Decision 2 — the delivery rule is ADR-0012's, unchanged

Input is written into the world's `Events<InputEvent>` buffer **between ticks**,
at `Runner::draw`, after the reload check and before `FrameLoop::step` — which is
between the last tick of the previous frame and the first of this one, the same
boundary ADR-0022 Decision 4 established for the reload. `rotate_events` runs
first in the tick, so the input is readable throughout it.

Everything the brief asked for then falls out of ADR-0011's rotation rather than
being invented:

| asked for | what provides it |
|---|---|
| delivered exactly once | `rotate` drops what was readable before |
| in the first tick of the next advance that ticks | rotation happens per tick, and the buffer is filled between them |
| a zero-tick advance keeps the queue | no tick means no rotation; the next frame's events are appended |
| catch-up ticks after the first get nothing | the second rotation of an advance finds `pending` empty |

An advance runs 0 to 8 ticks (`FixedTimestep::DEFAULT_MAX_TICKS_PER_ADVANCE`), so
all four cases are reachable in an ordinary frame.

**Rejected: a queue of mapped events held in the runner and released when a tick
is known to be due.** Its best argument is directness — the rule would be written
where it is read, instead of resting on a rotation two crates away. Against it:
the runner would have to ask the timestep whether it will tick before it ticks,
which is a second scheduling decision in a second place; and the window would
then have a delivery semantics of its own, which is precisely the failure mode
ADR-0011's Context describes — one channel with two behaviours, and a bug nobody
can find afterwards. One semantics for both runners was the goal.

## Decision 3 — a world swap discards the undelivered queue

A reload reconstitutes the world and restarts its tick counter (ADR-0022
Decision 1). Input collected for the world that was running is therefore
discarded, not carried across: it was aimed at a world that no longer exists, and
delivering it would put a keystroke into a different world at a tick number that
has just been reset.

Only the *undelivered* queue needs the rule. Anything already handed to the old
world went into that world's buffer and is dropped with it, for free.

A **refused** reload does not discard, and that is the same rule rather than an
exception: the world that was running is still running, so input aimed at it is
still aimed at it. `reload_if_changed` returns whether it actually swapped, which
is what makes the two cases distinguishable at the call site.

**Rejected: replaying the queue into the new world.** Best argument: a keystroke
the player has already made should not vanish because a file happened to be
saved. Against it: the tick counter has restarted, so the event would arrive at a
tick that means something different; and a scene reload usually changes what the
entities *are*, so an action aimed at the old world is not obviously meaningful
in the new one.

## Decision 4 — a world with no buffer drops the input, and says so once

A scene-constituted world carries no `Events<InputEvent>` today, so a window
pointed at one has nowhere to put input. It is dropped, and a note is printed
**once per run** — not per frame, which at sixty frames a second is not a
diagnostic, and not never, because a window whose keys silently do nothing is the
failure class this project spends its time removing.

**It drops rather than inserts, and that is load-bearing.** The obvious "fix" —
insert a buffer into a world that lacks one — would be a defect with a delay on
it. `World::insert` does not consult the registry
(`crates/narvo-ecs/src/world.rs:199-213`), so the insert would *succeed*, and
the failure would surface later and elsewhere, when something asked that world
for a canonical dump and got `EcsError::UnregisteredComponent`. A window never
dumps, so it would not even be the window that broke.

Giving scene worlds an input buffer is a registry decision and belongs to **M5.4**,
where the dump can see it. This ADR deliberately leaves it alone.

The other obvious reuse is also refused: `sim::feed` is mode-dispatched and
reaches `unreachable!` for every mode but `Input`, so a window pointed at a scene
would have **panicked** rather than dropped.

## Decision 5 — `--mapping` is a window flag, and there is no default

`--mapping <path>` binds the window's keyboard. Without it the window behaves
exactly as it did before: no key produces an input event, and only the built-in
Escape does anything.

**No default mapping ships.** A file the engine invented would be a set of
bindings nobody wrote, which `ProjektPlan.md` §2 excludes.

The flag is refused for every run that has no keyboard — headless, `--screenshot`,
and an unattended `--frames` measurement — as one check rather than three
conflict pairs, because the rule is "only the window has a keyboard" and one
check says that where three would only imply it.

A mapping that does not load **refuses the start**, before a window opens, with
the file named and the position `ron` computed. That is `RunPlan::from_scene`'s
existing pre-flight discipline applied to the second file a window can be given.

The load is **one read**: `std::fs::read_to_string` followed by
`narvo_input::from_str`, not `from_file`. ADR-0019's rule that hash and load come
from one read has no anchor to protect here, but M5.2 recorded `from_file`'s own
second read as a hazard, and this path does not inherit it.

## Consequences

- **The window can now change simulation state.** Until M5.3 a window only read a
  world; it now writes into one component of it, between ticks. The render path
  is untouched and still only reads.
- **`SceneHost::world_mut` exists**, and is the one writer outside the scheduler.
- **No window recorder was built, so ADR-0022's cut rule still has no site.** That
  decision remains decided-and-not-built, its revision condition unchanged. M5.3
  approached the coexistence gap and did not close it: recording is still
  headless-only and watching is still window-only.
- **The `input` demo is still not reachable from a window.** Its world is built in
  code by the headless runner, and `--mode` implies `--headless`. So the first
  end-to-end *window* consumer of a mapping is a scene world that can carry a
  buffer — M5.4's business.
- **`Command::Window` grew a payload struct**, `WindowOptions`, following
  `HeadlessOptions` and `FrameOptions`.

## Revision condition

Reopen when a window can record, which is the ADR-0022 revision condition this
task deliberately did not trigger: a recorder at this seam would have to decide
whether it records what `deliver` produced or what the device did, and ADR-0012's
M5.2 amendment already answers that — above — but the *site* would be new.

> **This condition fired, and the amendment below answers it.** The pointer
> arrived; the sentence is kept as written because its second half turned out to
> be the wrong instruction and the amendment says why.

Reopen when a second device class arrives — a pointer, a gamepad — because
Decision 1's "one function" is a claim about a boundary rather than about a
keyboard, and a second device must widen that function rather than add a second
one.

Reopen if auto-repeat filtering ever needs to be configurable, which would move
it from a table into state and out of this module's pure half.

## Amendment (M6b.8): the boundary is a place, and the number was never one

Answers the revision condition above — *"reopen when a second device class
arrives — a pointer"*. It arrived, and this records which of D23's three routes
was taken, why, and what it bought.

### The condition had already fired, quietly

The pointer did not arrive with this amendment. It arrived in M5.4, and by M5.7
the whole click path was running: a `CursorMoved` remembered a position, a
`MouseInput` turned it into an action through `Projection::screen_to_world` and
`hit_test`. What did **not** happen was the reopening this document asked for, so
the path was built entirely inside `window.rs` — which carries no tests, and
carried none then.

That is the measurement this amendment rests on, and it was taken before anything
moved: `window.rs` held **zero** `#[test]` and **zero** `#[cfg(test)]`. The
pointer path's *components* were covered — `hit_test` through the blessed click
scene, `screen_to_world` against its own inverse in `crate::frame` — and the
**composition** was covered by nothing. `Runner::click` had exactly one caller and
it was the winit event arm; `Live::cursor` had exactly one writer and one reader,
both in the same untestable file.

### Weg C, and what the other two would have cost

Three routes were on the table. The human decided C on 15.08.2026.

- **Weg A — widen `translate`.** Refused because ADR-0025's device vocabulary has
  no term for a position. A pointer spelled as a `Control` and an `Edge` would put
  a device coordinate into a `.ron` mapping and, through it, into every recording
  — which is exactly what ADR-0012's M5.2 amendment exists to keep out. The
  revision condition above *recommends* this route ("a second device must widen
  that function"), and that recommendation is the half of the sentence this
  amendment discards.
- **Weg B — a second boundary in `window.rs`.** Refused because it changes
  nothing: the code is already there, and leaving it there is what produced a
  path with no coverage.
- **Weg C — rename the boundary.** Taken. Decision 1's real content is *"exactly
  one place knows winit"*, and `input.rs` is that place. The pointer's state and
  its resolution move there; the unwrapping of a winit `CursorMoved` stays in the
  event arm, because that is what a boundary is for.

### What moved, and what did not

`input::Pointer` holds the remembered position and answers what a press at it
means. `Runner::click` keeps this runner's own three decisions — which projection
the press is answered through, that a window with no `--mapping` has no queue to
put the result in, and what to print when a rectangle names an illegal action.

**No behaviour changed.** The four steps are the same four steps in the same
order, through the same `Projection` and the same `hit_test`.

**No eighth winit signature was added**, measured rather than assumed: the seven
M6b.1 counted are the seven that exist after this change. `Pointer::moved_to`
takes two `f32`, which is the point — the position crosses the boundary as
scalars, so everything downstream of it still speaks no winit.

### What it bought, which is the only reason C was worth taking

Eleven tests that could not have been written before. Not *hard* to write —
**impossible**: the state was a private field of a struct that only exists inside
an `ActiveEventLoop`, and the resolution was a private method of the type that
handles winit callbacks.

They cover the composition and both of its directions: a press on the button
resolves to its action and value, a press away from it resolves to nothing, a
press before the pointer ever moved resolves to nothing, the projection is what
decides where a pixel lands, a camera moves what a press hits, the front-most
rectangle answers, and a rectangle naming an illegal action reports rather than
resolves. They live in `input.rs` under `#[cfg(all(test, feature = "render"))]`.

### The correction to Decision 1's number

Decision 1 says `translate` "is the only place in this workspace that names a
winit type in a signature". **That was already false when it was written**, and
the document contradicts it one heading lower under *"The boundary is two
functions, and winit is why"*.

Measured in M6b.1 and re-measured here, it is **seven**:

| # | Where | Why it is there |
|---|---|---|
| 1 | `input.rs` `translate` | the table's front door |
| 2 | `input.rs` `device_event` | a `KeyEvent` cannot be built outside winit |
| 3 | `input.rs` `control_of` | the table itself |
| 4 | `window.rs` `fail` | passes `&ActiveEventLoop` through |
| 5 | `window.rs` `resumed` | winit's `ApplicationHandler`, signature not ours |
| 6 | `window.rs` `window_event` | as above |
| 7 | `window.rs` `draw` | passes `&ActiveEventLoop` through |

Four of the seven are the event loop and three are the keyboard table. **Two
files, and no third**: outside `input.rs` and `window.rs` no line in the
workspace names a winit type, and the only other occurrence of the string is
`FORBIDDEN_IN_HEADLESS` in `xtask/src/main.rs`.

So the rule that survives is *the boundary is a place*, and the count that
matters is **two files, not seven functions and not one**.

### Revision condition

Reopen when a device arrives that a `(f32, f32)` cannot describe — a gamepad
axis, a pen with pressure and tilt, a touch point with an identity that persists
across frames. `Pointer` is deliberately not a device abstraction; it is the
pointer, and a second device gets its own type beside it rather than a widening
of this one.

Reopen if a pointer position ever has to reach a recording, which would put
ADR-0012's M5.2 amendment back on the table. Nothing here does that today: a
press becomes an action and only the action travels.
