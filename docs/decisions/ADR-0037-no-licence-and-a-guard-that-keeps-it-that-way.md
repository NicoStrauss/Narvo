# ADR-0037: The repository grants nothing, and a test keeps it that way

Status: accepted · Date: 2026-08 · Scope: the root `LICENSE`,
`[workspace.package]` and all fourteen member manifests, `deny.toml`'s
`[licenses.private]`, `README.md`, `CLAUDE.md`, and one test in `xtask`

## Context

M0 wrote a dual `MIT OR Apache-2.0` licence into this repository — two files at
the root, one `license` key in `[workspace.package]`, and `license.workspace =
true` in every member manifest. It was the customary Rust arrangement and it was
chosen before the project had any direction for its licensing.

D5 reopened the licence half on 11.08.2026 with a direction that is incompatible
with it (`ProjektPlan.md` §11): a royalty model of the Unreal shape, free below a
threshold and a small percentage above it. The plan's own sentence is the one
that matters here — *ein einmal so publizierter Stand wäre unwiderruflich frei.*
A permissive grant cannot be withdrawn from a copy already published under it, so
the dual licence stops being a decision that can be revisited the moment anything
ships under it.

The repository is private, which contains that and does not prevent it.
Visibility is one click, and D5 records the private setting as a decision about
*Actions spending*, not as a safety mechanism. So the exposure is: a repository
that grants a permissive licence in five places, one click away from granting it
irrevocably to everyone.

**This ADR decides nothing about what Narvo will be licensed under.** That is
D5's open question, it belongs to a human, and D5 already routes it through legal
advice before any release. What is decided here is only that the old grant stops
being made while that question is open.

## Decision

### 1. The licence surface is emptied, and the same places say why

Every place that declared a licence now withholds one instead, and it is the same
places so that a reader checks one set of locations rather than two:

- `LICENSE-MIT` and `LICENSE-APACHE` are **deleted**. Their content was a grant;
  there is no correction to leave visible in the body of a permission that is no
  longer given.
- A new root `LICENSE` carries the notice: all rights reserved, no licence chosen
  yet, none granted, and third-party material untouched. It sits where a licence
  sits, and says there is none.
- `[workspace.package]` loses its `license` key and all fourteen members lose
  `license.workspace = true`. A comment in the root manifest says the absence is
  the statement.
- `README.md` and `CLAUDE.md` say it in prose, in the sections that used to say
  the opposite.

**No SPDX headers were involved** — a survey found none anywhere in the tree, so
that surface was empty already.

`crates/narvo-testkit/assets/DejaVuSansMono-LICENSE.txt` is deliberately
untouched. It is a *foreign* licence, and the font's own terms require it to
travel beside the font. The same reasoning covers every dependency: their
licences are their authors' to give, and `deny.toml`'s `allow` list, its
`triple_buffer` exception and D5's standing lawyer item are all unchanged.

### 2. `cargo deny` is told these crates are ours, via `publish`

Removing the fields turns the licence check red. Measured, in this tree rather
than inherited from M5b.1's throwaway probe: `cargo deny check licenses` reported
exactly **fourteen** `error[unlicensed]` lines, one per workspace crate, and no
foreign crate.

`deny.toml` gains `[licenses.private] ignore = true`. cargo-deny's own generated
template (0.20.2) describes the key as *"ignores workspace crates that aren't
published, or are only published to private registries"* — so the exemption is
not "trust our code", it is "these do not go to a registry", and it reads the
`publish` field and nothing else.

**That coupling was measured, not assumed.** With `publish = true` set on
`narvo-core` alone and nothing else changed, `cargo deny check licenses` failed
with exactly one `error[unlicensed]`, naming `narvo-core`; the other thirteen
stayed exempt. The exemption is per crate and bound to the field. It therefore
fails in the right direction: a crate that becomes publishable loses the
exemption by itself, with nobody having to remember this file.

`publish = false` was already on all fourteen — inherited from
`[workspace.package]`, with `narvo-testkit` also stating it directly — and is
confirmed resolved by `cargo metadata`. It was not added by this decision; it was
found in place and is now load-bearing for a second reason.

### 3. The guard lives in `xtask`'s tests, and needs no new verification step

A one-off removal rots. The guard asserts, reading the tree at run time:

- no `Cargo.toml` in the workspace has a line beginning `license` (which covers
  `license`, `license-file` and `license.workspace`);
- every manifest says something about `publish`;
- no `.rs` file carries an SPDX header;
- `LICENSE-MIT`, `LICENSE-APACHE` and `COPYING` are absent **from the root only**,
  so the font's licence is out of scope by construction;
- `LICENSE` still reserves rights and contains neither licence's opening grant,
  so replacing the notice with a real licence under the same file name is caught;
- `README.md` still says it too — the v0.96 rule that a fact can be guarded while
  the text asserting it is not.

Both file counts are asserted as floors (≥ 15 manifests, ≥ 100 Rust files) so
that a walk which found nothing cannot pass by not looking.

It is **not an eleventh verification step**, and that was measured rather than
assumed: `xtask` is a workspace member precisely so its drift guards run in
`cargo nextest run --workspace`, which is step 2. The full workspace run went
from 1109 tests to 1110.

## Alternatives rejected

- **A placeholder `LicenseRef-…` expression in `[workspace.package]`, added to
  `allow`.** The close one. It keeps the licence check switched on for our own
  crates instead of exempting them, and puts the reservation into the metadata
  where tooling reads it. It loses on what it leaves behind: a `license` key,
  which is the exact artefact being removed, so the next reader finds a field
  that looks like a licence and needs prose to learn that it is not one — and it
  would need an entry in `allow`, the list whose own comment reserves it for
  licences that genuinely permit something.
- **Fourteen `exceptions` entries.** That table maps a crate to a licence it may
  use. There is no licence here to name, so it cannot express this at all.
- **Changing only the prose and leaving the fields.** The half machines do not
  read, and the half that gets copied into published metadata.
- **Choosing a replacement licence now.** Out of scope by D5 and by this task. A
  licence proposed in a commit would anticipate a decision the plan assigns to a
  human with legal advice.

## Consequences

- The workspace declares no licence. Anyone reading the manifests, the root or
  the README is told that nothing is granted.
- `cargo deny`'s licence policy no longer applies to our own crates. That is the
  intended trade and its scope is exactly the fourteen: foreign crates are
  checked as before. The cost is that a licence field coming *back* is invisible
  to `cargo deny` — measured, with the injected field in place the check reported
  `licenses ok` — which is why the guard exists and is not redundant with it.
- One re-introduction shape does not reach the guard at all, and this was found
  by trying it: `license.workspace = true` in a member now fails **cargo's own
  manifest parse**, because `[workspace.package]` has no `license` to inherit. No
  test runs; the build stops. The guard catches the literal form.
- Outside contributions have nothing to be made under, and the README says so.

## Revision condition

D5 deciding the licence. When that happens this ADR is superseded rather than
amended: the guard's assertions become false by design, and changing them is part
of the decision rather than a repair. Nothing here should be read as making that
decision easier or harder in any particular direction.
