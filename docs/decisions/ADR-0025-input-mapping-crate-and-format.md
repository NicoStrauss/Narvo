# ADR-0025: `narvo-input` is a leaf crate, and its mapping file binds a control to an action per edge

Status: accepted · Date: 2026-08 · Scope: the new `crates/narvo-input`
(`Control`, `DeviceEvent`, `Mapping`, `InputEvent`, `InputError`), `narvo-ecs`
(which loses `InputEvent` and one error variant), and `narvo-app` (which imports
it from one crate further out)

## Context

ADR-0012 decided that an input names an action and not a key, and left a
consequence standing: `InputEvent` sat in `narvo-ecs` "for now", and "its
natural home is the `narvo-input` crate the workspace target picture has for M5,
and it should move there when that crate exists". M5.1 is where the crate exists.

Two things had to be decided together, because each constrains the other: where
the crate's boundary runs, and what a mapping file looks like. A file format that
needed the ECS to describe a binding would have decided the boundary; a boundary
that put the mapping above the event type would have decided the format.

One thing is deliberately **not** decided here. **D8** — whether a recording
stores device events or the actions they map to — is open in `ProjektPlan.md` §11
and due in M5.2, with a real mapping in hand, which is what ADR-0012's own
revision condition asks for. This ADR is that mapping. It takes no step towards
either answer, and §4 below says how that neutrality is enforced in code rather
than only promised in prose.

## Decision

### 1. `narvo-input` depends on nothing in this workspace

Not on `narvo-ecs`, not on `narvo-core`. Its whole dependency set is `ron` and
`serde`, and `narvo-ecs` is a `dev-dependency` for one integration test.

That is possible because of a fact about the ECS that was surveyed rather than
assumed: `Events<E>` places **no bound at all** on `E`
(`crates/narvo-ecs/src/events.rs:100` and the inherent impl at `:107`), and
`ComponentRegistry::register_component<T>` needs only
`T: Component + Serialize + DeserializeOwned` (`registry.rs:222-225`), where
`Component` is blanket-implemented for everything `Send + Sync + 'static`
(`world.rs:25-27`). Every one of those is a `serde` or `std` trait. A type
defined in a crate `narvo-ecs` has never heard of therefore registers, hashes,
serializes and rotates exactly like one it owns.

So the dependency runs the *other* way, and only where a consumer needs both:
`narvo-app` depends on `narvo-ecs` and on `narvo-input`, and neither of those
two knows about the other. `crates/narvo-input/tests/pipeline.rs` is the
mechanical statement of it — if the bound set ever grew an ECS-defined trait,
that file would stop compiling.

### 2. `InputEvent` moves, and takes its error with it

The type is byte-for-byte the same: two fields, the same names, the same `serde`
rendering, so `Events<InputEvent>` in a canonical dump is identical before and
after. The determinism suite's 18 cases and 22 files compare byte-identical
across the move, which is the evidence rather than the claim.

What could not move unchanged is the error. `InputEvent::new` returned
`Result<Self, EcsError>`, and keeping that would have forced the very dependency
edge §1 exists to avoid. So `EcsError::InvalidActionName` is **removed** from
`narvo-ecs` and `InputError::InvalidActionName` takes its place, carrying the
same charset in the same sentence.

`EcsError` is `#[non_exhaustive]`, and nothing outside `narvo-ecs` matched that
variant: `narvo-app`'s recording parser discarded the error with `map_err(|_| …)`
and raised its own (`crates/narvo-app/src/recording.rs:425`). Removing it is
therefore a change no caller can observe, which is why it is a removal and not a
deprecation.

### 3. The device vocabulary is a closed enum of this crate's own names

`Control` is an enum, not a string, because a name the loader cannot check is a
name a typo survives: `KeyW` misspelt as `Keyw` would bind nothing, silently, and
the file would look right. As an enum it is a load error that names what was
written and lists every control there is.

It is **this crate's** enum and never a re-export. The mapping file's meaning must
not change when the window library changes, and a re-exported foreign enum would
make the format an alias for that library's release notes. Translating winit's key
list into this one is M5.3's job, in the runner, above here.

The set is deliberately small — a movement cluster, four arrows, three
non-printing keys and the ten digits — because that is the smallest vocabulary
that lets a mapping file for this project's own headless demo be written, and
`ProjektPlan.md` §2 rules out building on stock. M5.3 is where its completeness
gets decided against a real device rather than guessed at. The enum is
`#[non_exhaustive]`, so growing it is additive, exactly as ADR-0019 made the
recording header additive rather than versioned.

Names follow the W3C UI Events `code` convention (`KeyW`, `Digit0`, `ArrowUp`),
and a control is named by its **position** on the device rather than by the
character it produces. A binding is about where a finger goes; a layout that moves
the letters should move the letters and not the movement cluster.

### 4. A binding says what it emits on each edge, and there is no default

```ron
Mapping(
    bindings: [
        (control: KeyW, action: "thrust", emit: OnPressAndRelease(press: 1, release: 0)),
        (control: Digit1, action: "select", emit: OnPress(1)),
    ],
)
```

`OnPressAndRelease` is for a control that is *held*: thrust has to stop when the
key comes up, and only a second event can say so. That is the shape
`InputEvent`'s own documentation table already described (down/up → `1`/`0`).

`OnPress` is for an action that *happens*: a click that buys an upgrade, a
selection, a jump. Emitting `buy 0` on release as well would hand every consumer
an event it has to know to ignore, and "an event the consumer must ignore" is a
bug that announces nothing and that the first forgetful consumer pays for by
buying twice. It is not free either: the pending buffer is part of the state hash
(ADR-0011), so an ignored half is one more piece of simulation state per release.

Neither is a default. Both are written where they are used, so a reader never has
to know a rule to know what a line does.

**D8 is untouched by all of this.** No device term can leave this crate as text:
`Control` derives `Deserialize` and not `Serialize`, and `DeviceEvent` and `Edge`
have no `serde` at all. The only reason to give them an output form would be to
write device terms into a file, which is the question itself. Until M5.2 answers
it, this crate cannot drift towards an answer by accident.

### 5. One control is bound once, and that is what makes the order a fact

Two bindings naming one control is a hard error. The alternatives are both worse:
last-wins makes a file's meaning depend on line order in a way nothing announces,
and firing both makes one press produce two events whose order is a property of a
container rather than of the file.

With that rule, `Mapping::map` emits **in the order of the device events it was
given, and nothing else** — one device event produces at most one input event, so
there is never a set of bindings to put in some order and no container's iteration
order can reach the output. The lookup is a `BTreeMap` anyway, for the reason one
layer down: the iteration order of one is a property of the data and of the other
a property of a seed.

Two controls may share an *action* — `KeyW` and `ArrowUp` both sending `thrust` is
the ordinary case, and nothing about it is a duplicate.

### 6. Everything is checked at load, so mapping cannot fail

`Mapping::map` returns a `Vec<InputEvent>` and not a `Result`. The action-name
charset is checked while the file is read, so an event built from a binding cannot
be rejected; the duplicate rule is checked there too, so a lookup cannot be
ambiguous. A `Result` no caller can trigger is a `Result` every caller unwraps.

The crate-private `InputEvent::from_checked` is what makes that safe to say: the
invariant is held by the module boundary rather than by a comment asking callers
to be careful. That is a second reason the mapping and the event belong in **one**
crate rather than two.

## Rationale for what is not here

**Where an error is, spelled two ways.** Everything `ron` rejects — malformed RON,
an unknown field, an unknown control, a missing field — arrives with a line and a
column already computed, and this crate hands them straight back. This crate's own
two rules name the **binding's index** in the list instead. That is the same split
`narvo-scene` made in M4.2 and for the same reason, with one honest difference
recorded here: there an entity's index is also its slot in the loaded world, so it
locates the mistake in the file *and* in the world; here it locates it in the file
alone.

A line and column for those two is reachable. It was **measured**, not assumed:
`ron` 0.12.2 attaches a position to a `serde::de::Error::custom` raised inside a
`Deserialize` impl (`3:34-3:35: an action name must not be empty` in the M5.1
probe). It is not done, because carrying a *structured* variant back out of a
deserializer needs a side channel, and a variant a test can match on structurally
is worth more than a column. Revisit if a mapping file ever gets long enough that
counting bindings is work.

**No copy of `locate`.** `narvo-scene`'s position machinery is half reusable —
`Location` is public, `locate` is `pub(crate)` in a private module. Reusing the
type alone would mean `narvo-input` → `narvo-scene`, an edge with no reason
behind it, and copying the function would be the second copy of a location
mechanism in a repository whose §9.2 already records what the third copy of
`sha256` cost. So this crate carries `line` and `column` as two fields on its own
`Syntax` variant, renders them in the same `line:column: ` form, and owns no
line-counting code. Moving `Location` (and `locate`) to `narvo-core` — the
precedent is `sha256`, moved there in M4.4 once it had two consumers — is the
right fix and is **reported as a follow-up rather than made here**, because it
would put a fifth and sixth crate into a task already spanning three.

**No analogue axes and no positions.** Excluded from the M5 core by decision
(`ProjektPlan.md` §6/M5, delegated decision 3), not by oversight. ADR-0012 leaves
the spelling of an analogue magnitude in `value` open until there is a device to
be faithful to, and this ADR does not guess at it.

## Alternatives considered

**Leave `InputEvent` in `narvo-ecs` and put only the mapping in the new crate.**
Best argument: it is the smaller change, and the survey's premise about the move
was explicitly flagged uncertain, so declining it was a legitimate outcome. Against:
the survey answered the uncertainty in the affirmative and cheaply — the wiring
carries the move, and `narvo-app` was the only consumer. Leaving it would also
have split the two halves of one invariant: the mapping would validate action names
against a rule owned by a crate it does not depend on, and `from_checked` could not
exist. ADR-0012's consequence would have stayed open for a third milestone.

**An open string for a control name, resolved by the runner.** Best argument: the
mapping file would need no vocabulary at all, and M5.3 could accept whatever winit
names a key, so the file format would never need extending. Against: a typo becomes
a silent no-op, and the loader — the one place that can catch it — is handed nothing
to check against. The whole reason error paths are a feature of this milestone is
that a mapping file is edited by hand.

**Re-export winit's `KeyCode` as the vocabulary.** Best argument: no translation
table to write in M5.3, so no chance of mistranslating one. Against: it would put
`winit` in `narvo-input`'s dependency tree and therefore in the headless one, which
`cargo tree -p narvo-app --no-default-features` fails the build over; and the file
format would then change whenever winit's enum did.

**A binding emits on both edges, always.** Best argument: one concept instead of
two, and the consumer decides what to do with a `0`. Against: §4. Recorded because
it was the first design and it is the one that looks obviously right.

**A `release` value that is optional, absent meaning silence.** Best argument: one
variant instead of two, and the common case (`release: 0`) stays one word shorter.
Against: RON spells an absent `Option` as `None` and a present one as `Some(0)`
unless a file-level extension is enabled, so the readable form needs a parser option
that the scene format does not use — two RON dialects in one repository, to save a
word. And an absent field would make "emits nothing on release" a rule a reader has
to know rather than a thing the line says.

**Validate inside `Deserialize` so every error gets a line and column.** Best
argument: measured to work, and every message would then carry a position.
Against: the structured variant cannot come back out without a side channel, and
a `DuplicateBinding` a test can match on beats a `Syntax` a test has to read.
Named here with its measurement so that a later revision starts from a fact.

## Consequences

- **`narvo-ecs` no longer owns anything about input.** Its crate documentation
  says so, and CLAUDE.md's crate table lost "input events" from that row in the
  same commit. The row for the new crate says it depends on nothing in this
  workspace, which is a claim `cargo tree` can check.
- **The mapping file format has no version field and no header.** Deliberately:
  ADR-0019's amendment established that this project grows a format additively
  rather than versioning it, and `#[serde(deny_unknown_fields)]` means an
  unrecognised field is a load error rather than a silent acceptance. A file that
  needs a version is a file whose meaning changed, and that is a different
  decision.
- **A mapping is read and never written.** There is no `to_string`, so no
  round-trip property test of the kind ADR-0018 requires of scenes. A mapping is
  authored, not generated, and a writer with no consumer is stock.
- **`Mapping::map` is a pure function and holds no state, so it does not filter
  auto-repeat.** An operating system that reports a held key as a stream of
  presses produces a stream of events, and every one of them is mapped. Whoever
  reads the device knows whether a press is a repeat; giving a pure function the
  state to find out would make it stateful for one caller's convenience. M5.3
  inherits that.
- **The first consumer outside a test arrives in M5.3.** M5.1 ships the core and
  the test that drives it end to end; nothing in `narvo-app` calls `map` yet,
  and the `input` demo still gets its events from the seeded pilot. That is the
  cut `ProjektPlan.md` §6/M5 asked for and it is why this task is D8-neutral.

## Revision condition

Reopen when **M5.3** brings a real device, on two counts: whether `Control`'s set
is the right one once winit's key list has to be translated into it, and whether
the edge model survives contact with a device that reports more than two states.

Reopen if **M5.2** decides D8 below the mapping. Recording device events would
make `DeviceEvent` and `Edge` part of a file format, which means they would need
a serialized form, an identifier-safe spelling and a stability promise — none of
which they have today, and all of which are refused here on purpose so that the
decision is made rather than inherited.

Also reopen if a mapping file ever needs to bind a *combination* — two controls
held together — because §5's one-binding-per-control rule is what makes the
ordering statement in §5 true, and a combination is a second thing that could
claim a control.
