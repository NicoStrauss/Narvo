# ADR-0047: The engine is renamed to Narvo

Status: accepted · Date: 2026-08 · Scope: the whole workspace (fourteen crate
directories, every manifest, every import, `Cargo.lock`, both workflows)

## Context

D30 in the plan's §11 is the decision and a human made it on 20.08.2026; this ADR
records it and the measurement behind it. **This file is the one place in the
engine repository where the old name is written on purpose**, so that the
decision stays findable after the name it replaced has gone from everywhere
else.

> **Corrected in U2 (24.08.2026): the sentence above is no longer true, and it is
> marked rather than deleted because it was true when it was written.** D31 —
> decided by the human on 20.08.2026, after this ADR — settled that the twelve
> blessed reference images are **not** re-blessed. They are test instruments of
> the text renderer rather than product identity, and which word their glyphs
> spell is irrelevant to what they check. The consequence is that **six strings
> across three files** (`text_lines.rs`, `linear_blessed_margin.rs`,
> `msaa_blessed_margin.rs`) and one PNG carry the old name **permanently**,
> public repository included. They have carried a comment saying why since U1a —
> without it the next reader "fixes" them and moves a blessed reference. So the
> old name is written on purpose in **two** kinds of place in the engine: here,
> and in that fixture. The section *"And one kind that was found by a failing
> test"* below is the measurement this correction rests on; what changed is not
> the finding but the claim of exclusivity in this paragraph.
>
> **Re-checked in U3 (24.08.2026), and the correction above still holds.** A
> case-insensitive sweep of the tracked tree finds the old name in **eight**
> files. A line count is deliberately left out: this paragraph sits in one of
> the eight and would keep invalidating its own number. The eight are this
> ADR; the three test files whose six fixture strings are named above;
> `%TEMP%\amboss-golden-app\` in `narvo-render2d/src/golden.rs`;
> `Amboss-Gauntlet` and `Uebergabe-Amboss.md` in `CLAUDE.md`;
> `Uebergabe-Amboss.md` again in `.github/workflows/ci.yml`; and the
> `AmbossGameEngine` URL in `README.md`'s badge comment. Everything after the
> fixture is a name of something that is not this engine, so the two kinds the
> paragraph above settled on are unchanged. What U3 changed is narrower than
> exclusivity and is stated exactly: after the filesystem paths moved, this
> file is the only place in the engine that spells the old **paths**. It is
> still not the only place that spells the old **name**.

### The occasion

`Amboss` was a working title, and §1 of the plan has said so since v0.1 — the
choice was always outstanding rather than made. Two decisions turned it from
outstanding into due:

- **D29 (publication).** Privately the name cost nothing. A public repository
  under a name somebody else owns is a different object.
- **D5 (monetisation).** The direction the human set is a royalty model on
  Unreal's shape: free up to a threshold, then a small percentage, with the
  threshold and the rate fixed and never retroactive. That makes the project a
  commercial offering, and a commercial offering under a contested mark is the
  case trademark law is actually about. D5 already carried the note in its own
  words — *"«Amboss» ist markenrechtlich prominent besetzt — Namensprüfung vor
  jedem kommerziellen Schritt"* — and this is that step.

### The collision, measured rather than supposed

**AMBOSS is a well-known German software brand.** AMBOSS SE, Berlin: over 400
employees, €240 million in funding, over a million users. The product is
explicitly *learning software*. That is the same market, the same language and
Nice class 9. This is not a distant homonym in another field; it is a software
company whose name is spelled the same way, in the country the project is
written in.

## Decision

**The engine is called `Narvo`.** Every crate, every binary, every identifier,
every string and every document that names the engine says `Narvo`. The old name
survives in exactly four kinds of place, each for a reason that is not inertia:

1. **This ADR**, so the decision is findable.
2. **Filesystem paths** — `/mnt/d/Amboss`, `D:\Amboss`, `$HOME/.cache/amboss` —
   because the working directory is still called that. A document naming a path
   that does not exist is worse than one naming the old path. The directory is
   renamed in U2, when a fresh clone happens anyway.
3. **Names of things that are not this engine**: the GitHub repository
   `AmbossGameEngine` (replaced in U2), the frozen clone `Amboss-Gauntlet` that
   D25's evidence is about, the plan-side file `Uebergabe-Amboss.md`, and
   `%TEMP%\amboss-golden-app\` — a directory M3.10 really created, so renaming it
   would make a recorded measurement false.
4. **The German word for anvil**, which is what `Amboss` means and what
   `games/forge-loop` is about. `lang/en.ron` says `"verb.melt": "The Anvil"`
   where `lang/de.ron` says `"verb.melt": "Der Amboss"`. That is the game's
   furniture, not this engine, and a blind replacement would have renamed the
   anvil.

> **Corrected in U3 (24.08.2026): two sentences in the list above name the
> wrong task, and they are marked rather than deleted because they were the
> plan when this ADR was written.** Item 2 says the directory "is renamed in
> U2"; item 3 says the GitHub repository was "replaced in U2". **U2 did
> neither.** The boundary between the tasks moved after this ADR had frozen
> it: U2 separated the tree — the engine stayed, the plan and the game left —
> and both renames went past it.
>
> What actually happened, in order. The **working directory** was renamed by
> hand before U3 began, and U3 pulled the **thirteen** path occurrences
> behind it — across `CLAUDE.md`, `crates/narvo-app/src/watch.rs`, ADR-0007,
> ADR-0019, ADR-0022 and `docs/perf/BASELINE.md`. Item 2's premise, *because
> the working directory is still called that*, has therefore expired:
> filesystem paths are **no longer** one of the places the old name survives,
> and the enumeration above now has three live kinds rather than four. Item
> 2's own line is the one place in the engine that still spells the three old
> forms, which is exactly why they are left standing in it.
>
> The **GitHub repository** is still `AmbossGameEngine` as this is written.
> U3 prepares the public tree and proves it — a copy without `.git`, checked
> byte for byte against `HEAD` — and deliberately creates no repository,
> pushes to no new remote and changes no visibility. Those are a human's
> handgriffe, and they are what item 3's parenthesis is still waiting for.

### And one kind that was found by a failing test

**Rendered content is not a name.** `text_lines.rs` lays out the strings
`"Amboss 16 gjpq!"` and `"Amboss 32"`, and `text_lines_ascii_192x80.png` — one of
the twelve blessed references — holds those glyphs *as pixels*. Renaming them
with everything else moved the model's lit-pixel count from **1588 to 1819** and
the reference stopped matching. A blessed reference moving is a HALT and never an
occasion to re-bless (ADR-0008), so the six strings across three files were put
back and now carry a comment saying why. Making the sample text read `Narvo`
means re-blessing that reference deliberately, as its own task.

## Three candidates fell first, and the rejections are recorded so they do not recur

| Candidate | Free where | Fell on |
|---|---|---|
| **Scriber** | crates.io | **EUIPO hits in classes 9 and 42 at an identical name**, plus a US application for downloadable software |
| **Syntaxis** | registers empty | a **near-word pointing at the wrong category** (parser, compiler); `.com` taken; and the ordinary dictionary word in Dutch and Spanish |
| **Instrux** | crates.io | **six active companies** carry the name and every good domain is gone |

### The selection rule that came out of the three

**Near-words are the worst class.** They carry an association that is almost
never one's own, *and* they are crowded — because a near-word is exactly what
everybody reaches for. Only two classes carry:

- **an obscure real term from an unmined domain** (Groma, Dioptra) — costs one
  sentence of explanation;
- **a coined word** — costs build-up work, which Godot, Bevy and Fyrox all paid.

`Narvo` is the second class.

### Narvo, checked four ways

| Check | Result |
|---|---|
| crates.io | free — `narvo`, `narvo-core`, `narvo-render2d` |
| web search | **no software hit**; what exists is two British company registrations with no trademark bearing (a commercial agency and a property business) |
| DPMAregister | **0 hits** |
| EUIPO | **0 hits** |

Each register check covers Nice classes 9, 41 and 42.

**Named, and deliberately not booked as an objection:** `Narvo` is one letter
from `Narco`, and it sits beside the German lamp brand NARVA. Both were weighed
and neither carries; they are written down so the next reader does not have to
rediscover them and wonder whether anybody looked.

**D5's reservation now covers both.** The lawyer's appointment before publication
examines trademark and licence in one sitting rather than two.

## The proof that this changed nothing is the twelve unmoved references

A rename may move names and nothing else, and this repository already owns an
unusually sharp instrument for saying so: **twelve blessed reference images,
compared blob-wise before and after.** All twelve carry the same SHA-1 after the
rename as before it, and git records all twelve as pure renames rather than
edits. Beside them: the four test counts are unmoved — 1 630 / 361 / 450 / 282,
not plus-one — and the fourteen-step verification set is green on both sides.

That the instrument is *live* rather than merely quiet was demonstrated before
the rename began, against the unchanged tree, with three reverted injections: one
offset in `quad::VERTICES` (one reference falls), one in `sprite::sprite_vertices`
(the other eleven fall), and one that makes the simulation genuinely
nondeterministic (six determinism tests fall). Without that, the green at the end
would be a claim rather than a measurement.

The first of those three also refuted a prediction made before it ran. "All
twelve fall" was registered; **one** fell, because `quad::VERTICES` is the
screen-filling quad alone and the other eleven references are drawn through
`sprite_vertices`. The second injection was written to close exactly that gap,
and its prediction — eleven fall, `textured_quad_quadrants_64x64` stays green —
held to the test.

## Consequences

- **`release-determinism.yml` fires**, and correctly: every manifest under
  `crates/` is touched and `Cargo.lock` changes. The workflow triggers on
  `crates/*/Cargo.toml` and on `Cargo.lock`, and either alone is enough.
- **Guards that check a constant against a file had to move with it.** The
  verification set lives in three places by design and two of them name crates.
  Every guard whose expectation moved was shown to fail, on purpose, after it
  moved — a guard adapted to a new reality is green because it was adapted, not
  because anything is true, and that is this repository's most dangerous failure
  class in its most tempting form.
- **rustfmt rewrapped code the rename did not otherwise touch.** `narvo` is one
  character shorter than `amboss`, so lines that had been wrapped now fit. This
  is mechanical, and it is why the diff is larger than the substitution count.
- **A transition state is deliberate and temporary.** Between U1a and U2,
  `CLAUDE.md` and the ADRs say `Narvo` while `ProjektPlan.md`, `docs/history/`,
  `docs/design/` and the M7 documents still say `Amboss`. Those files move to a
  private plan repository in U2 and are not renamed here; the pre-registration
  and the history archive additionally declare themselves unaltered, which is a
  second and independent reason not to touch them.
- **Nothing is decided here about what Narvo is licensed under.** That stays
  D5's, and a human's.

## What is not known

- Whether `Narvo` clears every register in every jurisdiction the project will
  eventually reach. Four checks were run and are recorded above; they are not a
  legal opinion, and D5's appointment is where one is obtained.
- Whether the two British company registrations ever acquire a mark that would
  bear on class 9. Nothing suggests it today.
