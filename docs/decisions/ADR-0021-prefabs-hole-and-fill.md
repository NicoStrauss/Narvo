# ADR-0021: Prefabs are templates with holes, filled at load time

Status: accepted · Date: 2026-08 · Scope: `narvo-scene` (the `prefabs` block and
the instance syntax), and every scene written from here on

`ProjektPlan.md` §6/M4 lists "Prefabs mit Overrides" and §6/M7 names the consumer
that wants them — enemy types and item variants, "Template plus
Positions-Override, keine Vererbung". This ADR decides what that means in the
file.

## Context

M4.1 built the scene format and, with it, a mechanism that turns out to be the
whole of this feature: **splicing**. A `refs` entry names a component and a
field, and the loader inserts that field into the component's body before the
registry reads it — without parsing the body, because ADR-0018 requires the
author's bytes to reach the component verbatim.

M4.5's survey found that mechanism already does more than M4.1 needed:

- it inserts **any number** of named fields per component, since it iterates a
  map;
- a field it inserts that the body already sets produces a **duplicate field**,
  which the component's own deserializer refuses — precisely, by name;
- since M4.2 every `refs` value is a `RawValue`, so it carries a **position**.

Three properties a prefab feature needs, already built and already tested. What
was missing was one generalisation: splice took `EntityId` values, and a prefab
needs to insert arbitrary RON.

## Decision 1 — a template is a one-entity body with holes

A `prefabs:` block before `entities:` defines named templates. A template is a
component map exactly like an entity's, plus its own `refs`, and **nothing
else** — no name, because a template is not in the world and nothing can point
at one.

A template body may leave fields out. That absence is the **hole**, and it is
not a default: a hole nobody fills is refused by the component's own
deserializer as a missing field. Nothing invents a value.

An instance is one entry in `entities:` that names a template with `from:`. It
stays **one list entry**, so ADR-0018's file-order-is-spawn-order rule is
untouched, and `instances_and_flat_entities_share_one_ascending_slot_order`
checks that with the two kinds interleaved.

## Decision 2 — hole and fill, and the collision is hard

An instance has four operations and no fifth:

| operation | syntax | meaning |
|---|---|---|
| **fill** | `fill: { component: { field: value } }` | splice a value into the template's body |
| **replace** | `components: { component: (…) }` | the instance's body wins verbatim; the template's is not spliced into at all |
| **add** | `components: { component: (…) }` | for a component the template does not have |
| **remove** | `without: ["component"]` | drop a template component |

`fill` deliberately mirrors `refs`: same shape, same splice, and the only
difference is where the value comes from — `refs` resolves a name to a handle,
`fill` carries the author's RON. One mechanism, two sources.

**Omission is inheritance.** An instance that says nothing about a component
still gets it; there is no silent removal, which is why removal has a word.

**A collision is a hard error.** If the template sets a field and the instance
fills it, the assembled body carries that field twice, the component's own
deserializer refuses it, and the loader reports `FieldCollision` naming the
template, the component and the field. There is **no precedence rule and no
silent winner**: a field is set in one place or the other. The message says so
and offers the two ways out rather than picking one.

### How the collision is identified, and why not by reading the message

The deserializer says "duplicate field `x`". It does not say a prefab was
involved, and parsing its text to find out would tie this crate to another
crate's wording.

Instead the cause is established **structurally**: on failure the body is read
again *without* the filled fields. If that succeeds, the fills are what broke it,
and the offending field is found by adding them back one at a time. If it fails
too, the body was already wrong — an unfilled hole, or a value of the wrong type
— and the ordinary error is the honest one. The probes run on a scratch world,
only on the failure path, and cost one parse per filled field of one component.

### Rejected: merging fields by rewriting the body

The obvious alternative — parse the template body, parse the instance's overrides,
merge the two field sets, write the result — and the one every other engine's
prefab system does.

**Its best argument is real:** it makes a collision a non-event. The instance
simply wins, which is what "override" means everywhere else, and an author never
has to think about which half of a value lives where.

**It is closed by M4.1's own measurement chain**, and this is not a preference:

- `ron::Value` cannot carry it — `ron::to_string(&Value)` emits map syntax that a
  struct's `Deserialize` rejects (10 of 10 probes), and `Value` drops struct
  names besides;
- RON will not hand back a struct body's fields verbatim — `ExpectedMap`;
- a custom `deserialize_any` receives `f64`, not source text, so decimal → f64 →
  decimal → f32 double rounding is not provably the identity.

So a merge means writing a RON lexer and re-rendering numbers this project's
state hash is taken over — the exact thing ADR-0018 refused, for the exact
reason. **Hole-and-fill buys the same feature with none of that**, at the price
of a rule an author has to know: a field belongs to the template or to the
instance.

### Rejected: whole-component replacement only

No holes at all; an instance that wants a different position writes the whole
`transform` out.

**Its best argument:** it needs nothing new — M4.1's format already supports it,
and this ADR could have been a page of prose saying so.

**Against it:** the position case is exactly what §6/M7 asks for, and under this
rule every instance repeats `rotation`, `scale_x` and `scale_y` to change `x`.
Repetition is a drift source: change the template's scale and the twenty
instances that copied it keep the old one, silently and with no error anywhere.
Whole replacement survives as one of the four operations, for when it is what an
author means.

## Decision 3 — expansion is a load-time transformation

A template is **scene syntax, like a name** (ADR-0018): it exists while the file
is being read and stops existing when the world is built. A world does not know
one was involved.

That is not a claim to take on trust. It is the property the feature is *defined*
by:

> A scene with prefabs and the hand-written flat scene equivalent to it load to
> worlds with identical `canonical_dump`.

Checked for the committed example, for each of the four operations on its own,
and — over generated combinations of all four with generated field values — by a
`proptest` whose flat side is built programmatically from the same case, so the
two sides are different code producing the same world rather than one string
derived from the other.

**The writer stays prefab-free**, and that follows rather than being decided
separately: `to_string` writes what a `World` holds, a `World` holds no template,
so there is nothing to write. The same consequence ADR-0018 records for names,
and the same test shape.

## Non-goals, each with the condition that would reopen it

- **External prefab files.** A template lives in the scene that uses it. Sharing
  one across scenes means a scene depending on a second file, which is exactly
  **ADR-0019's second revision condition** — the scene identity anchor names one
  file, and a multi-file initial state is that ADR's problem to solve, not this
  one's. Reopens there, when it does.
- **Inheritance.** A template may not be built on another. Reopens if a real
  content set shows two templates that differ in one field *and* the duplication
  is measured to hurt — and even then, the first thing to try is a third
  template, not a chain.
- **Nesting.** An instance may not itself be a template for something else.
  Same condition.
- **Multi-entity prefabs.** A template is one entity. A template that spawned
  three would need a way to say how they relate, which is parent-child
  territory, and this format has no hierarchy at all. Reopens when hierarchy
  does.

## Consequences

- **`components` on an entity became optional.** An instance that only fills
  holes contributes none of its own. A flat entity with no components is
  therefore also legal now — an idle entity, which the world model has always
  allowed.
- **A template's `refs` resolve in the instance's scene**, not in the template.
  They have to: a template is not in the world, and the entity it names need not
  exist until an instance is spawned. Two instances of one template point at
  whatever *their* scene calls that name.
- **A hole points at the template, not at the instance.** The missing-field error
  carries the position of the template's body, because that is where the hole is;
  the instance says nothing about the field. Checked rather than assumed —
  `a_hole_that_nobody_fills_is_the_components_own_missing_field` asserts the
  line.
- **No new warning class.** M4.2 decided on exactly two and this adds none. Every
  prefab finding is an error, because every one of them is a scene that will not
  load.
- **The splice mechanism now takes text rather than handles.** `refs` renders its
  resolved handle with `ron::to_string` at the call site, so this crate still
  never spells a field of `EntityId` by hand.

## Revision condition

Reopen when a content set is large enough to measure — the arguments above about
repetition and drift are structural, and a real enemy roster is what would turn
them into numbers. Reopen if hole-and-fill's one rule proves to be the thing
authors get wrong most, which the validation CLI's findings would show.
