# ADR-0018: RON is the scene format, and what its file says

Status: accepted · Date: 2026-08 · Scope: `narvo-scene` (the scene file, the
loader and the writer), and the `DeserializeOwned` half of `narvo-ecs`'s
component registry

This ADR does two jobs, because M4 asked for both in one place: it records **D3**,
which was decided outside it, and it decides the **shape of the file**, which was
not decided anywhere before this task.

## Context

`ProjektPlan.md` §6/M4 opened the content pipeline and put the scene format first
— "das Verifizierbare zuerst". Two things were already fixed when this task
started and are inherited rather than re-argued:

- **D3 was decided on 2026-08-10, by delegated judgement**: RON is the truth, a
  JSON export is deferred until a tool consumes it. The reasoning is in §6/M4 and
  is reproduced below because an ADR that only points at a plan is not a record.
- **The format is component-open** (§6/M4, *Strukturbindung*): registry-driven,
  with no closed type list. That is the condition under which M8+'s 3D
  components are an extension of this format rather than a rewrite of it.

What was open: everything about the file itself. How entities are ordered, how a
component is spelled, how one entity points at another, and what a round trip is
allowed to promise.

## Decision 1 — RON is the truth (D3, recorded)

The scene format is RON. A JSON export is deferred until something consumes one,
under the same no-stockpiling rule as §2.

Reasoning, from §6/M4:

1. **ADR-0006 precedent.** The component registry already serializes into RON.
2. **The truth should be the format the registry already speaks.** That is the
   load-bearing half, and this task turned it from an aesthetic into a mechanism:
   a component's body in a scene file is *byte for byte* what
   `ComponentInfo::serialize` produces. There is one rendering of a component in
   this project, not two, so there is nothing to keep in step.

**Rejected: JSON as the truth.** Its best argument is real — far more external
tooling reads JSON natively, and a content format is exactly where external
tooling shows up. Against it: the two reasons above, and the fact that JSON has
no comments, which a hand-authored content format wants and this repository's
own specimen already uses.

**Not decided here:** where a *game's* scenes will live. The specimen this task
commits is `crates/narvo-scene/scenes/example.ron`, beside the crate that
defines the format, because it is the format's own worked example rather than
game content — a test resolves it from `CARGO_MANIFEST_DIR`, so
`cargo test -p narvo-scene` needs nothing outside the crate. A repository-level
`scenes/` would have claimed an answer to a question M4 has not reached.

## Decision 2 — file order is spawn order

The *n*-th entity in the file is the *n*-th entity spawned, so it lands in slot
*n* of a fresh world at generation 1. **Nothing in the file names a slot.**

The property is asserted rather than assumed. The loader does not compute a
handle from a position: it spawns, keeps the handles the world hands back, and
resolves names against those. `file_order_is_spawn_order_and_slots_ascend` is
what makes the *format* able to promise the correspondence, and it would fail
loudly if `hecs` ever stopped handing out ascending slots to a fresh world.

A consequence worth stating: **the file's entity position is also the entity's
slot index**, so it is the number that appears again in a canonical dump, in a
state hash comparison and in a `first_difference` report. Every error message in
this crate locates an entity by that number for exactly that reason.

## Decision 3 — entity references are symbolic, and live beside the body

An entity may carry a `name`. Another entity's `refs` table points one of its
components' fields at that name:

```ron
(
    name: "eye",
    components: {
        "follow": (smoothing: 0.5, x: 0.0, y: 0.0, lost: false),
    },
    refs: { "follow": { "target": "player" } },
)
```

The loader resolves `"player"` to a handle and splices `target` into the body
before the registry reads it. References may point **forwards**: every entity is
spawned before any component is read, because a camera following a player and a
player holding the camera's handle cannot both be satisfied in one pass, and a
format that only allowed backward references would make the author order the file
around the loader's implementation.

**Names are scene syntax, not world state.** No component holds one, no new
component type was added, and the world does not know them. The direct
consequence is that `to_string` cannot emit them — see Decision 5.

### Rejected: the index reference

Writing the target as the entity's ordinal — `target: 2`, or the handle itself,
`(index: 2, generation: 1)`.

**Its best argument, and it is not weak:** it needs no resolution step, no name
table, no duplicate-name rule and no splice; the file would say exactly what the
world holds, and the writer could round-trip it back out unchanged, which would
make byte equality of the two texts achievable instead of explicitly disclaimed.

**Against it:** an index is a *position*, and positions move. Inserting one entity
at the top of a file silently repoints every reference below it — silently
because the file still parses, the load still succeeds, and the camera now
follows something else. That is precisely the class of failure this project keeps
naming: a green result about the wrong thing. A name breaks loudly instead
(`UnknownReference`, which lists the names the scene does define), and a name is
also what a human or an agent editing content actually has in mind. The cost is
paid where it is cheapest — once, in the loader — and the benefit is paid out on
every edit.

The handle form is still *accepted* on input, because it is what the writer
produces; it is simply not what an author is expected to write.

### Rejected: the marker inside the component body

The natural-reading alternative is to put the reference where the field is:

```ron
"follow": (target: EntityRef("player"), smoothing: 0.5, x: 0.0, y: 0.0, lost: false)
```

**Its best argument:** it reads the way an author thinks, and the reference sits
in the field it fills instead of in a table three lines below.

**Against it — measured, not assumed.** The body must reach the registry
*verbatim* (Decision 4), so the marker would have to be found in raw text. Doing
that safely means re-implementing RON's lexer: `ron` 0.12.2's `parse.rs` skips
escaped strings, raw strings (`r#"…"#`), byte strings, char literals, line
comments and **nested** block comments, and a marker-shaped substring inside any
of them must not be touched. A hand-written scanner that got one of those wrong
would corrupt a body silently. Three cheaper routes were tried first and all
three are closed:

| route | why it is closed |
|---|---|
| walk `ron::Value`, substitute, re-serialize | `ron::to_string(&Value)` emits **map** syntax `{"x":0.1}`, which the component's own `Deserialize` rejects — `ExpectedNamedStructLike`, 10 of 10 probes. `Value` also drops struct names, so `EntityRef("p")` and the 1-tuple `("p")` are the same value |
| let RON hand back the body's fields verbatim | a RON struct body `(x: 1.0)` cannot be deserialized into `BTreeMap<String, &RawValue>`; it fails with `ExpectedMap`. (A *seq* can be decomposed this way; a struct cannot) |
| a custom `Deserialize` keeping leaves verbatim via `deserialize_any` | the visitor receives an `f64`, not the source text, and decimal → f64 → decimal → f32 double rounding is not provably the identity |

The `refs` table needs none of that: the marker never enters the body, so nothing
has to be found in text. It also buys something the inline form does not — **a
scene's reference graph is readable without parsing a single component body**,
which is what a validation CLI (§6/M4, next task but one) and M6's introspection
both want.

The inline form was put to an independent design panel during this task — four
designs from four different briefs, scored by three judges on separate criteria.
Every one of the four chose a marker inside the body, and the judges found two
hazards in that family which the `refs` table does not have: a save side that
must *recognise* a handle by matching its rendered text inside an untyped body,
and — for a design that wrote a bare handle for a dangling target while the
loader refused bare handles — a file that its own writer could produce and its
own loader could not read. One graft from the panel was taken and is in the code:
**the spliced handle is produced by `ron::to_string(&id)` rather than by writing
`(index:…,generation:…)` by hand**, so this crate never spells a field of
`EntityId` and cannot drift from the form `entity.rs` pins as stable API.
`a_spliced_handle_is_the_handles_own_serialization` holds the two together.

## Decision 4 — a component body reaches the registry verbatim

Apart from spliced reference fields, the bytes an author wrote are the bytes the
component's `Deserialize` sees. The body is opened at its first `(` and closed at
its last `)`, the new fields go in at the front, and everything between is copied
through untouched — numbers, strings, comments, nesting, whitespace.

This is a direct consequence of ADR-0008. The state hash is FNV-1a over RON text,
so a number's *spelling* is inside the hash's domain. Anything that read a body
into a value tree and wrote it back would be choosing those spellings itself, on
the load path, for every scene. The `RawValue` capture is what makes "verbatim" a
mechanism rather than an intention.

A body that cannot take a spliced field — a tuple, a seq, a bare literal — is
rejected by name (`NotAStructBody`) rather than mangled.

## Decision 5 — the round trip is over worlds, not over bytes

**Promised:** `World → String → World` reproduces the canonical dump exactly, for
every world a scene can describe. That is M4's closing criterion, and it is a
`proptest` property over generated worlds rather than a list of examples.

**Explicitly not promised:** that a scene file survives `String → World → String`
byte for byte. It cannot, and the reason is Decision 3 rather than a shortcoming:
a name is scene syntax, a `World` does not carry one, so the writer has nothing
to write. The specimen's round trip asserts the texts *differ*, so that the
freedom is a recorded property rather than an accident somebody later tightens.

### The domain boundary: despawn history is save territory

A scene *constitutes* a fresh world, so the worlds it can describe are the ones
spawning alone can reach: slots `0..n` in order, every generation 1. A world that
has despawned something is outside that set — its slots have gaps, or a recycled
slot carries a generation above one — and no scene file can produce it, because
nothing in the format asks a world to despawn.

`to_string` therefore **refuses** such a world (`NotSceneShaped`) rather than
emitting a file that would load back into a differently shaped world while
reporting success. This is not a gap: §6/M4 puts scene and save on one mechanism
("Szene und Save teilen den Kern"), and a world with history is the *save* side of
that mechanism. The round-trip property is stated over the scene domain, and the
save task inherits the wider one along with the question of how a recycled slot
is written down.

*A correction to the task that commissioned this ADR, recorded because it is the
kind of detail that outlives the conversation:* the boundary is **generation 1**,
not generation 0. `hecs` starts generations at one and `EntityId::generation` is a
`NonZeroU32`; generation zero is the one bit pattern `hecs` refuses.

## Decision 6 — no version field in the file

The recording format (ADR-0012) carries a format version and a scene does not.
The difference is what the artefact outlives. A recording outlives the run that
produced it and is replayed by a later process; a scene is content in this
repository, shipped in the same commit as the engine that reads it, and git is
its version. Adding one later is a `#[serde(default)]` field, which old files
satisfy without being touched.

## The change to `narvo-ecs`, and why it is not a widening

The registry could write a component and not read one back. `ComponentInfo` gains
a second type-erased function pointer, `deserialize`, alongside `serialize`; the
error enum gains `ComponentDeserialization`. Nothing else moved — in particular
`register_component`'s signature did not, because the `DeserializeOwned` bound it
has demanded since M2.1 was reserved for exactly this and its documentation said
so: *"a component that can be written but not read back is a state format that
cannot be loaded — a defect that would only surface once saving and loading
exist."* Loading exists now.

The alternative was for `narvo-scene` to keep its own name → constructor table,
which would have been a second registry with a second source of truth for stable
names — and a closed type list in the crate whose whole point is not having one.

`narvo-ecs` stays free of file I/O, which `ProjektPlan.md` §5.1 requires of it.
All path handling is two thin shells in `narvo-scene` around the string
functions, so a caller with the text in hand — a test, M4's hot reload, M6's IPC —
never touches a path.

## Relation to the ADRs it sits on

- **ADR-0006** said RON was the registry's *internal* format and that this was
  "not D3". That was correct, and D3 has now landed on the same answer for a
  different reason. ADR-0006's revision condition — "if D3 resolves in a way that
  makes one format across both worth the churn" — is discharged: there is no
  churn, because the two formats coincide by decision rather than by accident.
- **ADR-0008** governs Decision 4 entirely. The stability domain is unchanged:
  this task commits no hash and no dump, and the round-trip property compares two
  worlds produced from one commit.
- **ADR-0010** is why `rng` is in the specimen and in the property test. A
  generator's state is world state, so a scene authors it like any other
  component — its two integers, written out.
- **ADR-0014** is why every component body in a scene is scalars and never a
  foreign type's serde form. The scene format inherits that domain without
  restating it.

## Consequences

- **A new component type costs the scene format nothing.** Register it, and it is
  authorable. No line of `narvo-scene` mentions a component type, and the test
  that would notice a regression compares the specimen's component set against
  the registry's rather than against a list.
  `the_format_carries_component_shapes_the_engine_does_not_have` is what turns
  that from a claim into a check: it registers a **unit struct, a newtype, a
  multi-field tuple struct, an enum with all three variant kinds, and a struct
  containing a `Vec<Option<…>>` and a `String` holding `)` and `"`** — none of
  which the engine has today — and round-trips them beside an engine component.
  The seven engine types could not have shown this on their own, because they are
  all named-field structs and a format that handled only those would pass every
  other test in the file.
- **A `refs` entry is a second place to look.** An author writing a component with
  a reference must omit the field from the body and add it to `refs`. Writing it
  in both is caught (`Unexpected duplicate field named 'target' in 'Follow'`,
  with the position inside the body), but it is a rule to know, and it is the
  price of Decision 3's rejected inline alternative.
- **A written scene is not a nice scene, and it is editing-fragile in exactly the
  way this ADR rejected.** `to_string` emits compact component bodies, no names
  and no comments, and its references come out as positional handles —
  `(index:2,generation:1)` — because that is what the component itself holds.
  So a *written* scene inherits the whole weakness of the index alternative
  rejected in Decision 3: insert an entity at the top of one and every reference
  below it silently repoints. That is acceptable for machine output and a trap
  for a human who starts editing it. The rule the format leaves behind: **a
  written scene is an artefact, an authored scene is a source.** Once a person
  edits one, the references should be turned into `refs` entries, and the
  validation CLI of §6/M4 is the natural place to say so. A "reserved shape" rule
  — refusing a bare handle on load, so the fragile form cannot exist — was
  considered and rejected here, because it would make the writer's own output
  unloadable and break the round trip this milestone closes on.
- **The round-trip property cannot see a field that leaves the serialized form.**
  It is stated over the canonical dump, and the dump is built from the same
  serialization, so a component field marked `#[serde(skip)]` disappears from both
  sides and the property stays green. That is demonstrated in the M4.1 report,
  along with the test that *does* catch it — `narvo-ecs`'s own per-component
  round trip. The two guard different things and neither subsumes the other:
  `narvo-ecs` guards what the serialized form contains, `narvo-scene` guards
  that the format preserves whatever it contains.
- **NaN payloads are outside the domain**, and always were. RON renders every
  `NaN` as `NaN` or `-NaN`, so `0x7fc00001` loads back as `0x7fc00000`; the two
  canonical quiet `NaN`s and both infinities survive exactly. This is a property
  of the canonical form ADR-0008 already rests on rather than something the scene
  format introduces, and the property test's strategy excludes payload-carrying
  `NaN`s by name for that reason.
- **`proptest` enters the tree** as a dev-dependency (`cargo deny check` passes;
  it adds no new duplicate-version warning).

## Amendment 2026-08 (M4.2): a fragment's position, and why this is not its own ADR

Decision 4 keeps every component body, entity name and reference as a `RawValue`
borrowed out of the input. M4.2 takes one more thing from that: **a byte offset**,
by subtracting the fragment's address from the text's, which turns into the line
and column every error now carries.

It was worth asking whether depending on that deserves an ADR of its own. It does
not, and the reason is what kind of dependency it is:

1. **The borrow is a lifetime guarantee, not a promise `ron` could quietly
   withdraw.** The value is a `&'a RawValue` where `'a` is the lifetime of the
   `&str` that was parsed. The only buffer it can point into is that string, and
   the compiler is what says so. A `ron` release that stopped borrowing would not
   silently change behaviour — it would fail to compile.
2. **The arithmetic on top of it is checked rather than trusted.** `locate`
   compares the slice at the offset it computed against the fragment before
   returning a position, so a broken assumption produces *no position*, never a
   wrong one. An error that says less is a nuisance; one that points at the wrong
   line is a defect.
3. **A standing test pins it**, `the_offset_mechanism_locates_a_known_fragment`,
   against a hand-counted line and column — and it is the only test that would
   notice, because every other one asserts on messages and a message without a
   position is still the same message. Both failure modes were demonstrated red
   in the M4.2 report: a corrupted line count, which the slice check cannot see
   and the hand-counted position does; and a corrupted offset, which the slice
   check catches and turns into `None`.

What an ADR is for is a decision with alternatives worth recording. This one has
a compiler-enforced premise, a fail-safe implementation and a guard — so it is
recorded here, beside the decision it rests on, rather than given a document that
would only repeat this paragraph.

**One consequence of the amendment changes the grammar's reading, not its
meaning.** An entity's `name` is now kept as a raw value too, so an omitted
`name` and a written `name: ""` are finally distinguishable. The rule is
unchanged — both are anonymous — but `validate` can now warn about the second,
which M4.1 recorded as an accepted wart it could not see.

**A rough edge, named rather than smoothed.** A component body that fails to
parse produces a message with two positions in it: the file position of the body,
and `ron`'s own position *inside* the body. They are not in the same coordinate
system and the message does not say so. Composing them was considered and
rejected: for a body with a spliced reference the text the component's
deserializer saw is not the author's text, so a composed position would be right
for some bodies and wrong for others — which is worse than two honest numbers.

## Revision condition

Reopen when prefabs and overrides land, because an override is a second thing an
entity entry may hold and Decision 3's `refs` table is the nearest neighbour to
it; when a scene identity anchor for replays is decided (§6/M4 names it as an ADR
candidate), because that fixes what a scene's content hash is taken over; or if a
consumer for a JSON export appears, which reopens D3's deferred half and nothing
else.
