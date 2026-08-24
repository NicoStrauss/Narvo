# ADR-0043: A save is not a scene, and what it carries that a scene cannot

Status: accepted · Date: 2026-08 · Scope: `narvo-scene` (the save format, its
reader and its writer) and `narvo-ecs` (a world's free list, and rebuilding a
world from an entity table)

## Context

`ProjektPlan.md` §6/M6b's sixth post asks one question in its title — *is a save
a scene file?* — and M4 had already answered a neighbouring one. §6/M4 booked
**"Szene und Save teilen den Kern"**: scene as authoring format, save as state
dump, *both over the registry serialization*. ADR-0018 Decision 5 then drew the
line from the scene's side and handed the rest over by name:

> `to_string` therefore **refuses** such a world (`NotSceneShaped`) rather than
> emitting a file that would load back into a differently shaped world while
> reporting success. […] The round-trip property is stated over the scene
> domain, and the save task inherits the wider one along with the question of
> how a recycled slot is written down.

**ADR-0018 is not reopened by this.** Its revision condition names three
triggers — prefabs and overrides, a scene identity anchor, a consumer for a JSON
export — and a save format is none of them. It is read here verbatim rather than
paraphrased, because saying a condition has occurred when it has not is how a
decision gets reversed without anybody deciding to.

What was open: the file. Whether one exists at all, what it holds, and what
happens when a build is handed one it cannot read.

## Decision 1 — one mechanism, two contracts

**A save is not a scene file.** Both formats put a component's body through the
component registry byte for byte, so there is one rendering of a component in
this project and nothing to keep in step — that is the *mechanism*, and §6/M4's
booking is honoured rather than revised. Everything above it differs:

| | a scene | a save |
|---|---|---|
| is | written | produced |
| holds | content | state |
| names entities | symbolically, by `name` | explicitly, by slot and generation |
| reaches | worlds a fresh world can spawn into | every world, despawn history included |
| grows | additively, no version (ADR-0018 Decision 6) | by version, refused when unknown |
| outlives | nothing — git is its version | the build that wrote it |

The last row decides the rest, and it is ADR-0018 Decision 6's own reasoning
applied to the other artefact. That decision put no version in the scene because
*"a scene is content in this repository, shipped in the same commit as the engine
that reads it, and git is its version"*, and offered `#[serde(default)]` as the
growth path. Both halves fail for a save. A save is opened by a build its author
never saw, so git is not its version; and a default field can absorb a new field
but cannot say **which** build wrote the file it is silently absorbing.

The crate is shared and the module is not: `narvo-scene` now owns two formats,
and its own documentation says so. That is one crate rather than two on the same
ground ADR-0041 used — the obvious cut runs through machinery both sides need.
Concretely: the verbatim component body (`RawPairs`, `file.rs`) and the byte
offset that turns a borrowed fragment into a line and a column (`location.rs`).
Splitting the formats into two crates would have duplicated both or invented a
third crate to hold them.

## Decision 2 — a save carries three things a canonical dump does not

This is the load-bearing decision and the only one that was in real doubt. A
canonical dump prints live entities and their components. Three things decide the
next tick and are not in it.

### 2a — the handle of every live entity

Slot and generation, written out. A scene has nowhere to put one: file order *is*
spawn order there, so slot *n* is position *n* and every generation is 1
(ADR-0018 Decision 2). A save names them, which is what lets it describe a world
with gaps and with recycled slots.

### 2b — the free list, and this is the one a round trip cannot see

A despawn frees a slot and raises its generation; the next spawn takes a freed
slot before it reaches for a fresh one. **A world's future handles are as much
its state as its present ones**, and nothing in a dump shows them.

Measured against `hecs` 0.11.1 rather than argued. A world of five entities with
slots 3 and 1 freed, in that order:

| world | live | free list | next four spawns |
|---|---|---|---|
| the original | `0v1 2v1 4v1` | `3v2 1v2` | `1v2 3v2 5v1 6v1` |
| rebuilt from its live handles alone | `0v1 2v1 4v1` | `1v1 3v1` | **`3v1 1v1 5v1 6v1`** |
| rebuilt with its free list too | `0v1 2v1 4v1` | `3v2 1v2` | `1v2 3v2 5v1 6v1` |

**The middle row is why this ADR exists.** Its canonical dump is byte-identical
to the original's — same live entities, same components — so it passes every
round-trip test that can be written, and its next spawn lands in a different slot
at a different generation. Both halves of the divergence have a cause in the
source: `hecs::Entities::alloc_at` fills the gap below a handle by pushing the
skipped ids onto `pending` in **ascending** order (`entities.rs:342-352`) rather
than in the order they were freed, and it leaves them at `EntityMeta::EMPTY`'s
generation, which is **1** (`entities.rs:584-588`) rather than the raised one
`free` left behind (`entities.rs:376-397`).

`hecs` offers the fix and says what it is for
(`world.rs:938-947`): *"Entity handles will be allocated deterministically
between different worlds (e.g. across serialization) if they have the same live
entities and the same freelist."* `World::freelist` and `World::reconstitute` in
`narvo-ecs` are that pair behind the facade, and
`a_reconstituted_world_hands_out_the_handles_the_original_would_have` pins the
table above through it.

**The order is passed through, not normalised.** `freelist()` reports the
storage engine's own order, in which the **last** entry is the slot the next
spawn takes; the file writes it that way and `reconstitute` hands it back
unchanged. Reversing it at the boundary and again on the way in would be two
places to get it wrong, and a sorted free list is a plausible, wrong world.

### 2c — the tick

The tick counter is not in the world. The runner holds it
(`crates/narvo-app/src/headless.rs:294`) and passes it to systems through
`SystemContext`. Two systems in this tree read it and write world state from it:
`sim::scene::wander` computes an entity's whole `Transform` as a function of it
(`crates/narvo-app/src/sim/scene.rs:196`) and `sim::scene::arm_shakes` fires on
`tick % SHAKE_EVERY` (`…/scene.rs:226`). A save that forgets the tick round-trips
perfectly and diverges on the very next tick.

So a load returns a `Savepoint`, which is a world **and** a tick, and the two
travel together because neither reproduces a run alone. It is the one field in
the file that is not in the `World`, and it is named here rather than smuggled
in.

**ADR-0022 is the contrast that makes this legible.** Its Decision 1 records
that a hot reload restarts the tick, *"because a reconstituted world is a new run
rather than a continuation"*. A save is a continuation. Same mechanism —
reconstitution — opposite answer about the tick, for a stated reason.

### What needed nothing, checked rather than assumed

- **The generator.** `Rng` is a registered component (ADR-0010), so its state is
  already in the dump and already in the save.
- **Events in flight.** `Events<E>` serializes *both* halves, `pending` and
  `readable` (`crates/narvo-ecs/src/events.rs:100-105`), so ADR-0011's one-tick
  delivery survives with nothing added. The input buffer is an `Events` and is
  covered by the same sentence.
- **Physics.** ADR-0029 still holds: `physics::simulate` constructs a fresh
  `Physics2d` inside the system on every tick
  (`crates/narvo-app/src/physics.rs:84`), so the solver keeps nothing between
  ticks and there is nothing to write down.

### What is knowingly outside, and it is a limit rather than an oversight

A save is a world and a tick. The audio cue memory, the recording's read
position and the input source live in the runner, not in the world, and none of
them is written here. A run resumed from a save may therefore re-emit a cue the
saved run had already emitted. That is named rather than closed because closing
it means deciding what a *run* is, which is a larger object than a world and has
no consumer yet (§2).

## Decision 3 — a failed load cannot touch the running world, structurally

ADR-0022's posture is the requirement: a failed load leaves the running state
standing and says what is wrong. The question this task asked is whether that can
be **built** rather than **guarded**, which §9.2 ranks above any guard.

It can, and the shape is one signature. `World::reconstitute` is an *associated
function*: it builds a world and is handed none. `save::from_str` is the same —
it takes text and a registry, and returns a `Savepoint` or an error. **There is
no running world in either signature, so there is nothing for a failure to
damage.** A caller that keeps its own world until the call returns `Ok` cannot
lose it, and it does not have to remember to.

The rejected shape is the natural one: `fn load_into(&mut self, text: &str)`. Its
best argument is that it saves the caller a move and reads like the operation it
performs. Against it: it can clear a world and then fail on the third entity, and
the test that would catch that is a test somebody has to think of.
**Reopen** if a world ever becomes expensive enough to build that reusing one is
measured to matter — nothing has measured it, and a world is a `hecs::World`
behind a `BTreeMap`.

## Decision 4 — the version is asked for before a failure is reported

`version: 1` is the first field. A file naming another version is refused with
both numbers in the message.

The mechanism is worth recording because the obvious implementation does not
work. The file struct carries `#[serde(deny_unknown_fields)]`, so a save from a
*later* build — which has fields this one has never heard of — fails on the first
unknown field and reports that, burying the reason under a symptom. So when the
strict parse fails, the file is asked what version it claims by a second,
tolerant deserializer, and a version mismatch wins. The tolerant pass runs only
after the strict one has failed, so an ordinary load parses the text once.

Two things about it are measured against `ron` 0.12.2, not assumed:

- the tolerant struct **must** be named `Save`, because `ron` checks a written
  struct name against the type and answers `ExpectedDifferentStructName`
  otherwise;
- the tolerant pass does read `version: 2` out of a file the strict pass rejects,
  which is the whole claim.

**`deny_unknown_fields` on the envelope is itself a decision**, and it goes the
other way from the component bodies inside it. Measured on the scene format
first: an unknown field at the file or entity level is a hard error there
(`crates/narvo-scene/src/file.rs:23,101`), and an unknown field **inside a
component body is silently accepted and dropped**, because a component's derived
`Deserialize` has no such attribute. For a save the asymmetry is the wrong way
round and only half of it can be fixed here: a field the envelope cannot place is
state the run would carry on without, so the envelope refuses. The component-body
half is left as it is — changing it would change every component in the engine
and belongs to whoever owns that, not to this file — and it is booked below as a
named limit.

## The rejected candidates, each with its best argument

### (a) A save format of its own, with its own serializer

**Its best argument, in full:** total freedom. A save could pack, could store the
whole entity table as one column-major block, could version its component
encoding independently of the scene's, and would owe the scene format nothing at
all. It is also the only candidate under which the scene format could later
change without a thought for saves.

**Against it, and it is one measurement rather than a preference:** the state
hash is FNV-1a over RON text, so a number's *spelling* is inside the hash's
domain (ADR-0008, ADR-0018 Decision 4). A second serializer is a second set of
spellings, and a save written through it would restore a world whose dump differs
from the one that was saved — in the digits, not in the values. Every candidate
that keeps `ComponentInfo::serialize` as the one renderer avoids that by
construction; this one has to re-derive it and stay in step with it forever.

**Reopen** if a measured case shows the registry's RON to be too large or too
slow for a save — a saved world of thousands of entities is the shape that would
show it, and nothing has measured one. The reopening would be about the
*encoding*, and Decision 2 would survive it unchanged.

### (c) The scene file itself, with a version field

**Its best argument, and it is the strongest of the three, so it is written out
at length rather than summarised.** It saves a format. One grammar, one reader,
one writer, one error type, one set of tests, one thing to document and one thing
for an agent to learn — and *one mechanism less* is a real good in a project whose
§2 refuses capability without a consumer. The scene format is already
component-open, already puts bodies through the registry verbatim, already has
positioned error messages, already has a validator, already has path shells and
a round-trip property test in exactly the build form this task was asked to
follow. ADR-0018 even *accepts a positional handle on input* today, so an entity
that carries `(index:2,generation:1)` already loads. The engineering distance
from the scene format to a save is not large, and the ADR-0018 amendment that
would have carried it — a `version` field with `#[serde(default)]` — is one line.
§6/M4's own booking, read plainly, points here: if scene and save share the core,
sharing the file is the least surprising reading of that sentence.

**Against it, three things, in ascending order of weight:**

1. *A written scene is already an artefact and a trap.* ADR-0018's consequences
   say it in its own words: a written scene "is editing-fragile in exactly the
   way this ADR rejected", and leaves behind the rule **"a written scene is an
   artefact, an authored scene is a source."** Making saves the same file type
   would make that distinction load-bearing for players' data while it remains
   invisible in the file.
2. *The two grow in incompatible directions.* ADR-0018 Decision 6 chose additive
   growth deliberately and gave the reason: git is a scene's version. A save
   needs the opposite, and a format cannot have both — a `version` field that
   scenes ignore and saves enforce is two formats sharing a grammar, which is the
   candidate below rather than this one.
3. *The deciding one: the fields a save needs are fields a scene must not have.*
   A save has to name a slot and a generation per entity, and it has to carry a
   free list. Put those into the scene grammar and ADR-0018 Decision 2 — *"Nothing
   in the file names a slot"* — is gone, together with the argument that produced
   it: an index is a position, positions move, and an insert silently repoints
   everything below. An author would then be able to write a slot number in a
   scene, and the format would have no way to say they should not.

**Reopen** if the save format's grammar and the scene's ever converge to the
point where the two readers differ only in a version field — which would mean
Decision 2's three additions had been given a scene-side meaning, and that is a
change to ADR-0018 rather than to this record.

### (b) is what was built

An envelope carrying a version, and inside it the world in the shape the scene
already uses. One mechanism, two contracts. It is the candidate the plan
described, and the survey's job was to find out whether the two contracts really
separate. They do, and Decision 2 is the measurement of how far: three fields a
scene has no place for, one of which no round trip can see.

## What this does **not** decide

**Meta-progression.** §6/M6b names it as this post's largest unknown — whether
permanent upgrades live in a second world or in a second area of one. Nothing
here answers it, and nothing here forecloses it: the engine saves and loads *a
world*, and how many worlds a game keeps between runs is the game's arrangement,
built out of this the way any consumer is. §2 forbids building for a consumer
that does not exist, and M7's game does not.

The halt branch the task registered — *report and stop if it turns out a world
cannot be saved without deciding what lives between runs* — **did not fire**, and
the reason is worth one sentence: everything a save needs is either already in
the world or is the number of ticks that world has been run for. Neither says
anything about what a second run inherits.

**Where a save file lives, what it is called, how many slots a game keeps, and
when one is written.** All of that is a game's, and none of it is in this crate.
There is no CLI flag and no runner wiring; the capability is a library
capability, and its consumer is M7.

## Named limits

1. **An unknown field inside a component body is still accepted and dropped.**
   Measured, above. The envelope refuses one; a component body cannot, because
   that is serde's default for a derived `Deserialize` and changing it would mean
   an attribute on every registered component in the engine. The consequence for
   a save is real and bounded: a *newer* save loaded by an *older* build loses
   fields added to a component, silently, while the envelope's version check
   would have caught the same file if the format itself had moved. Closing it is
   a decision about `narvo-ecs`'s component contract, with the whole registered
   set in its blast radius.
2. **A save is a world and a tick, not a run.** Cue memory, the recording read
   position and the input source are the runner's and are not in the file. See
   Decision 2's last paragraph.
3. **`u32::MAX` generations.** `hecs` wraps a generation past `u32::MAX` back to
   1 (`entities.rs:384-386`). A save round-trips whatever generation it is given,
   so it inherits that and adds nothing to it. Reaching it takes four billion
   despawns of one slot.
4. **Nothing enforces that a save is loaded with the registry that wrote it.** A
   component the registry does not know is refused by name, which catches the
   common case; a component whose *shape* changed under one name is caught only
   if the body no longer parses. That is the same exposure ADR-0019's scene
   anchor exists to close for scenes, and no anchor is claimed here.

## Consequences

- **`narvo-ecs` gains two methods and three error variants**, and this task
  therefore touched two crates. CLAUDE.md treats that as an architecture signal
  to report rather than a thing to push through, and this is the report: the free
  list can only be read through `hecs`, ADR-0005 puts the one seam to `hecs` in
  `narvo-ecs`, and the file format cannot live there because `narvo-ecs` stays
  free of file I/O (`ProjektPlan.md` §5.1). The split falls where those two rules
  put it.
- **The entity table has to account for every slot below its highest.** A world
  reached by spawning and despawning holds every slot either live or free, so a
  table with a hole in it is one no world produced. `reconstitute` refuses it
  (`SlotMissing`, naming the slot). The consequence is the one worth having:
  **a save that dropped its free list is not a wrong file, it is an unreadable
  one.** The format cannot express the world in the middle row of Decision 2's
  table.
- **A save can hold a dangling handle** — a `Follow` pointing at an entity that
  was despawned — and it round-trips, because the body reaches the registry
  verbatim and nothing resolves it. That is correct: `Follow` has a `lost` field
  and the engine already treats a dead target as ordinary.
- **The round-trip property is not the oracle**, and the test file says so in its
  first paragraph. The oracle is the continuation: save at tick *n*, load, run to
  *m*, and compare against a run that was never interrupted — including the free
  lists, which the dump does not show.
- **An external consumer costs 17 packages besides itself** (18 including it),
  measured with `cargo tree --edges normal` over a crate outside this workspace
  depending on `narvo-ecs`, `narvo-scene` and `serde`. For scale, M6b.5
  measured 11 for a declaration-only consumer and 99 for world-and-renderer.
  Saving does not drag in a renderer.

## Revision condition

Reopen Decision 2 if a world ever gains state that is neither a component nor the
tick — a second such field would mean "a world" has stopped being the unit, and
that is a larger question than a file format. Reopen Decision 4 if a version 2 is
ever written, because the migration path — refuse, or read and upgrade — is
decided then and is not decided here; refusing is what version 1 does and is the
only behaviour that needs no second reader. Reopen Decision 1 if the scene format
and this one converge as described under candidate (c). Named limit 1 is reopened
by whoever decides `narvo-ecs`'s component contract, not here.
