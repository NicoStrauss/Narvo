# ADR-0012: The recording is line-based text, and the input source lives outside the world

Status: accepted · Date: 2026-08 · Scope: narvo-ecs (`InputEvent`), narvo-app
(`recording`, `sim::input`), and every replay built on them

## Context

M2's closing criterion asks that a recorded run reproduce its end state byte for
byte. The point of it is not the feature: it is that a bug becomes an artifact
somebody can attach to a report, and that whoever receives it can cut it from ten
thousand ticks down to thirty and watch it still fail.

That purpose settles more than "write the inputs to a file". Four things had to
be decided, and three of them have an answer that looks obviously right and is
wrong.

## Decision

### 1. The file is line-based text, not RON

```
narvo-recording 1
mode input
seed 1
ticks 10000

6 turn 1
7 select 18
8 thrust -1
14 select 29
end
```

A magic-and-version line, `name value` header lines in any order, a blank line,
then one input per line as `tick action value`, then `end`. Blank lines are
skipped and `#` starts a comment.

**This is not a decision about D3.** The scene format is open until M4 and this
ADR does not touch it, exactly as ADR-0006 does not for the registry's internal
RON. A recording is a debugging artifact with one producer and one consumer; a
scene format is content a person authors. They have no reason to be the same
thing, and the fact that this one is *not* RON should not be read as an argument
against RON there either.

### 2. Only ticks with input are written

An empty tick is implicit. A ten-thousand-tick recording of this project's demo
is 3,749 lines rather than 10,000, and — more to the point — a diff between two
recordings shows the inputs that differ instead of ten thousand identical blank
lines with a few changes buried in them.

### 3. Neither the final state nor its hash is in the file

ADR-0008 forbids committing the hash of a state, because such a value moves when
`ron`, `serde` or the compiler changes while nothing is actually wrong. A hash in
a recording would be the same value living in a file instead of in a test, with
the same failure mode and a worse one on top: every recording ever made would
become unreplayable at once on a dependency bump. A replay is checked by running
it against the original *now*, in one build.

The file does carry an `end` marker, which is a different thing: it says the file
is complete, not what the run produced. Without it a file cut short by a failed
write parses as a perfectly valid recording of a run that never happened.

### 4. The input source lives in the runner, never in the world

This is the one that decides whether any of the rest works.

`sim::input`'s world contains no generator and no decision of its own. Every
entity starts at rest and the only thing that can change one is an `InputEvent`
that arrived from outside the tick. The synthetic stand-in for a player —
`sim::input::pilot`, seeded — lives in the runner beside the loop, and on replay
it is not constructed at all.

It has to be that way round. ADR-0010 requires that anything affecting the
simulation be visible in the canonical dump, and by that rule a generator inside
the world would have to advance identically in both runs. But a replay does not
draw at all: it reads the file. A generator in the world would therefore be
advanced during recording and untouched during replay, and the two dumps would
differ in exactly that component while everything else matched. Outside the
world, it is not simulation state at all — it is what the simulation is
reproducible *against*, and the recording is the complete account of what it
produced.

### 5. Input is fed between ticks and is readable in the tick that follows

The runner writes a tick's input into the world's `Events<InputEvent>` buffer
before the tick runs; `rotate_events` is the first system, so the input is
readable throughout that same tick.

That is not a loosening of ADR-0011, which had events sent in tick *N* become
readable in *N+1*. Its argument was about a sender that is a *system*: with
same-tick delivery, whether a reader sees the event depends on where it sits in
the run order relative to the sender, so moving a system silently changes
behaviour. The runner is not a system and does not send from inside a tick.
There is no relative order to depend on, and every system in the tick sees the
same input set whatever its position — which is the property ADR-0011 wanted, not
an exception to it.

### 6. An input names an action, not a key

`InputEvent` is an action name and an `i64` magnitude. Nothing about keyboards,
mice or gamepads reaches the deterministic core.

A recording that stored `KeyW` would stop meaning what it meant the moment a
binding changed, and it would tie the simulation to a window library it must not
depend on. M5 brings the device-to-action mapping; what it hands the runner per
tick is a list of these, and nothing in this format has to change for it.

## Consequences

- **The format has a version and it is enforced.** A recording written by a
  different version is refused, not read as best effort. Changing the format
  means bumping `FORMAT_VERSION`, and every recording in existence stops
  replaying — which is correct, and is why the sparse, line-based shape was worth
  getting right now rather than later.
- **How an analogue axis is spelled in `value` is open.** An `i64` can carry
  fixed-point units, and something has to decide which — milli-units, or a
  device-native range. That is M5's call, made when there is a device to be
  faithful to. It is named here rather than guessed at; whatever is chosen, it
  must be exact under a text round trip, which is why the field is an integer and
  not a float.
- **Shortening a recording by hand means editing the header too.** Cutting the
  tail off with `head` loses the `end` marker and is refused; deleting lines from
  the middle keeps it and works. Reducing `ticks` is a separate edit. That is the
  price of detecting truncation, and it is one line.
- **A recording is not a save file.** It records inputs, never world state, so
  replaying always re-simulates from the start. Loading a world from a file is
  M4's business and a different artifact.
- **`InputEvent` sits in `narvo-ecs` for now**, because it is a component
  payload and needs serde, by ADR-0010's argument. Its natural home is the
  `narvo-input` crate the workspace target picture has for M5, and it should
  move there when that crate exists.
- Action names are restricted to an identifier charset. That is what lets the
  line format hold them without a quoting rule, and quoting is one more thing to
  get wrong in a file whose whole value is that it is obvious.

## Revision condition

Reopen when M5 brings real devices, on two counts: the analogue-axis
representation above, and whether recording *actions* rather than raw device
events is still the right layer once a mapping file exists that could itself
change between a recording and its replay. That second question is genuinely
open — recording below the mapping would make a repro survive a mapping change,
and recording above it makes the file readable. It should be answered with a
real mapping in hand rather than now.

Also reopen if a recording ever needs to carry something that is not an input —
a window resize, a pause, a mid-run configuration change. Those are not inputs
and should not be squeezed into the action namespace without a decision.

---

## Amendment (M5.2): D8 is decided — the recording stays above the mapping

Answers the revision condition above, both counts. M5.1 built the mapping
(ADR-0025), so the question can be answered with one in hand rather than guessed
at, which is what the condition asked for.

**The decision: a recording stores actions, not device events. The mapping lies
wholly outside the reproduction closure.** Decision 6 above is confirmed rather
than replaced, and nothing in the format moves — no field, no version, no line.
It is written down because "we looked and nothing had to move" is a result, and
ADR-0019's own M4.6 amendment established that such a result is recorded like
one.

### Why, in the order the evidence decided it

**1. Headless, there is nothing below the mapping to tap.** The runner's input
source has exactly two variants — `Source::Pilot(Rng)` and `Source::Recorded`
(`crates/narvo-app/src/headless.rs:189-198`) — and both hand back
`Vec<InputEvent>`. `sim::input::pilot` (`crates/narvo-app/src/sim/input.rs:149`)
builds those straight from the seeded generator. No device event exists anywhere
in a headless run, so a recorder below the mapping would have nothing to observe.

The precise version, because the overclaim is tempting: device events are not
*impossible* headless — `narvo-input` is graphics-free and its own integration
test maps them with no window. What is missing is a **producer**. Recording below
would therefore mean building a synthetic device layer whose only consumer is the
recorder, which is stock in the sense `ProjektPlan.md` §2 rules out.

**2. The mapping is a pure function and is already covered where it lives.**
`Mapping::map` takes `&self`, reads no clock, no entropy, no environment and no
global, holds nothing between calls and returns no `Result`
(`crates/narvo-input/src/mapping.rs`). Its order guarantee, its
one-binding-per-control rule and its repeatability each have their own test. A
replay that re-ran the mapping would re-derive a function a unit test already
pins, and would pin it *worse* — through a ten-thousand-tick state hash instead
of an assertion that names the property.

**3. Below costs a file, an anchor, a header pair and a flag — but *not* a
version bump, which is a correction to this amendment's own first draft.** The
draft asserted that recording device events would force `FORMAT_VERSION` past 1.
That was wrong, it was refuted against the precedent it cited, and it is written
out here rather than quietly deleted:

- The scene pair is **"Mandatory for a `Mode::SceneFile` run and rejected for any
  other"** (`crates/narvo-app/src/recording.rs:88-93`) — required for its kind
  and absent otherwise — and ADR-0019 Decision 3 declined a bump for exactly that
  shape. A `mapping` / `mapping-sha256` pair written only for device recordings
  is the same shape. "Required" in that rule means required of *every*
  recording, and the scene pair is the standing counter-example.
- Nor need the body line change meaning. The grammar dispatches on **field
  count** (`recording.rs:236-264`): two fields is a header, three is
  `tick action value`, anything else is `MalformedLine`. A four-field device line
  therefore leaves every existing recording byte-identical and makes an older
  build refuse a newer recording — the same safe direction ADR-0019 relied on
  from `UnknownHeaderField`. This ADR's "every recording in existence stops
  replaying" does not fire.

What the cost actually is, once that is subtracted: two header fields and a
sibling of `scene_anchor`'s pairing rule (`recording.rs:293-307`); a `--mapping`
flag the CLI does not have (`crates/narvo-app/src/cli.rs:339-383`); a load
hazard, because `narvo_input::from_file` reads the file a second time where
ADR-0019 requires hash and load to come from one read; and, across crates, an
output form for `Control`, `DeviceEvent` and `Edge` that ADR-0025 §4 withheld on
purpose. Real, bounded, and smaller than the argument that was first written.

**4. Scoping below to the runs that have a device produces two recording layers,
not one.** "Below the mapping" is undefined for a source that already sits above
it: `narvo-app` imports only `InputEvent`, and nothing in it calls `map`. A
below answer would therefore have to be scoped to device-sourced runs — the
window runner of M5.3 — leaving the headless pilot recording actions exactly as
it does now. That is two layers whose choice depends on how a run happened to be
driven.

And they would not be interchangeable, which is the measurement that makes this
concrete: the demo's `select` carries a value in `0..MOVERS` with `MOVERS = 32`
(`crates/narvo-app/src/sim/input.rs:26` and `:159`), while `Control` has
twenty-one variants of which ten are digits
(`crates/narvo-input/src/control.rs:58-101`). Twenty-two of the thirty-two
selections have no device spelling at all. A repro's layer would become a
property of its origin rather than of the format, and the two would not translate
into each other.

**5. Half of Decision 6's own rationale has lapsed, and the half that survives is
the deciding one.** Decision 6 gave two reasons for naming an action rather than
a key. The second — *"it would tie the simulation to a window library it must not
depend on"* — **no longer holds**, and M5.1 is what ended it: `narvo-input`'s
entire dependency set is `ron` and `serde`
(`crates/narvo-input/Cargo.toml:11-20`), and `Control` is that crate's own enum,
never a re-export (ADR-0025). A device recording would carry Narvo's own control
names, not winit's. That reason is subtracted here rather than re-used, because
re-using a lapsed argument is how an ADR rots.

The first reason stands untouched and is what decides: *a recording that stored
`KeyW` would stop meaning what it meant the moment a binding changed.* A repro
that survives a rebinding, that a person can read and cut down by hand, and that
names the simulation's own vocabulary is an *above* property.

### Rejected: record below the mapping

**Its best argument is real and is not answered by the five points above.** A
device-level recording would put mapping faults inside the reproduction closure:
a binding that emits the wrong magnitude, or a future device quirk, would be
reproduced by the replay instead of being re-derived correctly and hiding the
fault. Old recordings would also become regression fixtures for new mappings —
replay the device stream through today's file and see whether the same actions
still come out. That is a genuine capability and this decision gives it up.

Two things reduce it rather than refute it. The mapping is a pure function with
its own tests, so the fault class it would catch is the class a unit test catches
better. And nothing is lost that cannot be built later as a *separate* artifact: a
device-stream fixture for testing mappings is not a recording and would not need
this format.

Two further points in its favour are recorded so they are not lost, because both
were found while trying to refute the case *for* this decision and both make the
rejected option cheaper than it first looked:

- **No `.rec` file is committed to this repository.** Every recording is
  regenerated by the build that reads it (`xtask/src/determinism.rs`, and the
  record helper in `crates/narvo-app/tests/determinism.rs`), so a format change
  would invalidate no stored artifact here at all.
- **Together with point 3, the compatibility cost is close to zero**: no version
  bump, no field whose meaning changes, every existing recording still parsing.
  The pre-decision argument that below "would drag a `FORMAT_VERSION` front into
  every recording" does not survive measurement.

What is left standing against it is points 1, 2, 4 and 5 — no producer to tap, a
pure function already covered where it lives, two non-interchangeable layers, and
the one half of Decision 6 that has not lapsed. Those decide it; the cost
argument does not, and saying so is the difference between a decision and a
rationalisation.

### The analogue-axis question is deferred to its first consumer

The other half of the revision condition — how an analogue magnitude is spelled
in `value` — is **not** answered here, and that is a decision rather than an
omission. `ProjektPlan.md` §6/M5 excludes analogue axes from the M5 core and the
M7 slice is click-driven, so an axis has no consumer before rung 2 of the slice
ladder (a physics genre after M7). Choosing milli-units or a device-native range
now would be guessing at a device and a genre that do not exist yet, and the guess
would be frozen into a format that refuses to change quietly.

What stands unchanged: the field is an `i64` so that whatever is chosen is exact
under a text round trip.

**Revision condition for this half:** decide it in the task that gives an axis its
first real consumer, with the device in hand — not in a format task, and not
before. If an axis arrives sooner than rung 2, that task inherits the question.

### What this amendment does not change

`FORMAT_VERSION` stays 1. No header field is added or removed. No line of
`recording.rs`, of the replay path or of any recording changes, and the eighteen
determinism cases are byte-identical across this decision — the decision *is* that
the status quo is right. M5.1 had already left the layer untouched: its whole diff
in the recording path is two `use` lines.

### Revision condition

Reopen if a **window** recorder is built and turns out to need the device stream
for a reason this decision did not foresee. M5.3 is where a window recorder first
becomes possible, and `ProjektPlan.md` §6/M5 already notes that a D8-driven window
recorder would fire ADR-0022's cut rule for the first time.

Reopen if mapping faults ever become a class this project is losing time to. The
measurement that would say so is a bug a replay reproduced incorrectly *because*
the mapping had changed underneath it. Until such a bug exists, the rejected
option's best argument stays theoretical, and this project prefers a measured
consumer to a plausible one.

---

## Amendment (M6.2): D19 — a state change set from outside is not in the band

Answers the **second** half of the original revision condition above, verbatim:
*"Also reopen if a recording ever needs to carry something that is not an input —
a window resize, a pause, a mid-run configuration change. Those are not inputs
and should not be squeezed into the action namespace without a decision."* An
IPC write is exactly that case, so this is an amendment to this ADR and not a new
one. **The halt branch M6.2 was given did not fire:** nothing in the Decision
section above is replaced, so no ADR-0012-superseding ADR is required, and the
number reserved for one stays unassigned.

**The decision: a state change set from outside the simulation is not written
into a recording. A run that takes one has its band cut at that tick — valid and
replayable up to it, and not resumed afterwards.**

`ProjektPlan.md` §11 carries it as **D19**. It is a delegated decision; the human
abort right covers it.

### Why, in the order the evidence decided it

**1. The shape is already decided for the sibling case, by ADR-0022 Decision 6.**
A hot reload is also a state change that happens outside the band while a
recording runs, and that decision is: the reload **finalises** the recording — a
valid, replayable band up to the cut tick — and does not resume it. It rejected
*the reload as an event in the band* explicitly. This amendment takes the same
answer for the same reason rather than inventing a second one, which is what
keeps one artifact from having two rules about what may interrupt it.

The difference between the two cases is real and does not change the answer: a
reload **replaces** the world, a write **changes** it. What both share is the
property the band depends on — that the file plus the build is the whole account
of how the final state was reached. A write breaks that for every tick after it,
whichever way it got in.

**2. There is no producer, and §2 excludes building for one that does not exist.**
Measured rather than assumed: the only path from outside into a running world is
`Source::take` → `sim::feed` (`crates/narvo-app/src/headless.rs:205-223` and
`:325-329`), and it carries `Vec<InputEvent>` and nothing else. `sim::feed` is
mode-dispatched and reaches `unreachable!` for every mode but `Input`
(`crates/narvo-app/src/sim.rs:262-276`). There is no site at which a write could
be applied today, and there will not be one before M6.3 builds the transport.
ADR-0022 Decision 6 was itself *decided and not built* on this same reasoning.

**3. What a mutation line would cost, measured at the parser rather than
inherited.** The M5.2 amendment above read this grammar for the device-event
question and corrected its own first draft in the process; that reading is not
carried over here, because the value a write carries is a different thing from a
device event. ADR-0030 has a component cross the agent protocol as the
registry's own RON, so that is the text a line would hold. **Three** of the
eleven components in the engine registration set hold a string — `Sprite.region`,
`HitRect.action` and `Tally.action` (`crates/narvo-ecs/src/registry.rs:468-478`)
— and none of the three restricts its charset where it is stored; `Tally`'s own
doc says the identifier rule lives in a crate it cannot see. Measured on
`Sprite`, whose region name is unrestricted by construction:

| measured | result |
|---|---|
| `Sprite { region: "with a space" }` | `(region:"with a space")` — **three** whitespace fields |
| `Sprite { region: "with\na newline" }` | `(region:"with\na newline")` — `ron` escapes it, **one** line |

So a value cannot ride as trailing fields, and a line-based format is *not* ruled
out — only the field-splitting is. A mutation line therefore needs a rest-of-line
rule for its value, and with it a dispatch on something other than the field
count the grammar uses today (`recording.rs:236-264`). That is a change to how
every line is read, not an arm added beside the existing ones.

**4. Deciding this way forecloses nothing, and that is what settles a question
whose evidence is otherwise thin on both sides.** The same grammar makes a future
mutation line addable **without** moving `FORMAT_VERSION`: an unknown two-field
line is `UnknownHeaderField`, a five-field line is `MalformedLine`, and both are
refusals rather than skips, so an old build refuses a new recording instead of
replaying it wrongly — the safe direction ADR-0019 Decision 3 relied on. The
choice can therefore be made again against a real producer in M6.3 or later, at
the cost of one amendment and no invalidated artifact. Choosing the smaller thing
now is not deferral of a decision that will get harder.

### Rejected: widen the recording with a mutation line

**Its strongest argument, in full.** A recording that omits the write does not
reproduce the run the agent actually had. This ADR's own Context says the format
exists so that a bug becomes an artifact somebody can attach to a report and cut
down by hand — and under this decision, a bug an agent found only *after* setting
a component has no such artifact: the band stops one tick before the interesting
part. §6/M6 makes the recording format the output form of the repro test, so
widening the format is the only form under which an agent that wrote state can
hand over a repro **in that form at all**. The compatibility cost is close to
zero, and measured: no version bump (point 4 above), no field whose meaning
changes, every existing recording still parsing, and no `.rec` file committed
anywhere in this repository — every one is regenerated by the build that reads it
(`xtask/src/determinism.rs`), so a format change invalidates no stored artifact.

What decides against it is points 2 and 4 together, and not the cost: there is
nothing to record yet, and recording it later is cheap. Point 3 is a cost, not a
refutation — a rest-of-line rule is a day's work, and saying otherwise would be
dressing an unbuilt thing up as an impossible one.

### What a repro test proves under this decision, named narrowly

A repro built from scene, tick count and checkpoint reproduces the run **up to the
first write** and no further. That is a smaller claim than "reproduces the
agent's session" and it is the one that will be written on the artifact.

For M6.7's planted-bug demonstration this is very likely the relevant artifact
anyway — an agent's writes are its investigation, and the planted bug is in the
scene or the code, so the repro that names it needs the scene and the tick rather
than the agent's probing. **That is a claim about M6.7's shape, not a measurement,
and it is stated as one:** if the demonstration turns out to need a write inside
the repro, this amendment's revision condition is what that finding fires.

### Which of M6's deferred capabilities this actually answers

M6.1 deferred five write-or-effect capabilities to this decision. Checked one by
one, **only the first is a D19 question**, and the other four have homes already:

| capability | governed by D19? | why |
|---|---|---|
| set a component | **yes** | this is the case |
| load a scene | no | ADR-0022 Decisions 1 and 6 own reconstitution and its band rule |
| step ticks | no | it moves the clock; the header already carries `ticks`, and a run of *n* ticks is the prefix of a longer one (`headless.rs:226-233`), so how the steps were grouped is invisible to the state |
| take a screenshot | no | the render path only reads simulation state (CLAUDE.md) |
| start a replay | no | a control operation on the runner, not a change to a world |

This narrows what M6.3 and M6.4 have to settle before they can define their
commands, and it is reported to the plan rather than acted on here.

### What this amendment changes in the code

Nothing in the format, the parser or the replay path. `FORMAT_VERSION` stays 1,
no header field is added, and every recording is byte-identical across this
decision. What it adds is three tests in `crates/narvo-app/src/recording.rs`
holding the premises above, so that a later reader cannot mistake the file for
one that already carries a mutation, and so that building one is a red test
rather than a quiet extension: a hand-written mutation line is refused by line
number and quoted back; the field-count dispatch is pinned including the
three-field arm that would silently read `2 set 7` as an input; and the two
measurements in point 3 are assertions rather than a paragraph.

### Revision condition

Reopen at the first producer — M6.3, which builds the transport and is where a
write first becomes possible. Reopen sooner if a finding turns up that an agent
can only reproduce by re-applying its own write; that is the measurement which
would make the rejected option's best argument concrete rather than theoretical,
and it is the same standard the M5.2 amendment set for its own rejected option.
