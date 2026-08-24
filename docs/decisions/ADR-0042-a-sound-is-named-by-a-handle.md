# ADR-0042: A sound is named by a handle only a registry can issue

Date: 2026-08-15 (M6b.2b)

Status: accepted

## Context

M6b.2's survey asked whether a game can bring its own content without changing
the engine. For images the answer was yes. For sound it was no, and the survey
measured where the wall stood.

**Not at kira.** `StaticSoundData` carries no feature gate, its four fields are
public (`kira-0.12.3/src/sound/static_sound/data.rs:28-46`), `Frame::from_mono`
is ungated (`src/frame.rs:35`), and the only `#[cfg(feature = "symphonia")]` in
that file guards `mod from_file` — the decoder road D2 deliberately closed.
`kira_sink.rs` already built `StaticSoundData` by struct literal from synthesised
samples. **D2's decoder configuration is untouched by this ADR** and stays
`default-features = false, features = ["cpal"]`.

The wall was this workspace's own surface, measured as four compiler errors
against a probe outside the tracked tree: `sounds::ALL` is a
`const [(&str, Synth); 3]` with no `push`, `KiraSink::sounds` is private, and
there is neither an `add_sound` nor a `with_sounds`. **The sample type was open
and the table was shut.**

Two further measurements shaped what follows.

**A wrong name was writable and nearly silent.** `Cue.sound` was a
`&'static str`, so `Cue::play(0, Channel::Sfx, "blip")` compiled and ran. The
device-backed sink said `no sound named "blip" was synthesised; the run continues
without that sound` once on stderr and carried on. M6b.2 measured the second
half: a *second* miss in the same run said **nothing at all**, because `noted`
was a single `bool` capping every failure class together.

**The check lived where nothing exercises it.** Only `KiraSink` looked a sound
up. `KiraSink::new` has exactly one call site in the workspace — `window.rs`,
production code — and `window.rs` carries 0 `#[test]` and 0 `#[cfg(test)]` across
862 lines. A defect in the resolution would have been caught by nothing.

## Decision

**A cue names its sound with `SoundId`, an opaque `Copy` handle whose only source
is a `SoundLibrary`.** A sound nobody registered cannot be written down: the
field is private, and no function anywhere takes a number and returns a
`SoundId`. §9.2's rule is the reason — where a mistake can be made unwritable,
that beats any guard against it.

**The library is the runner's, and `Sink::submit` takes it**:

```rust
fn submit(&mut self, cue: &Cue, library: &SoundLibrary);
```

so both sinks resolve through `SoundLibrary::get` — one function, one copy — and
neither carries a table that could answer differently. That is ADR-0041's
`depth_order` argument applied to a second seam.

**`SoundLibrary::new` is the only constructor and always registers the three
synthesised sounds**, at 0, 1 and 2, which is what makes `SoundLibrary::CLICK`
and its two siblings valid against every library rather than against one built a
particular way.

**`noted` becomes class-wise.** Four classes — unknown sound, missing channel,
refused playback, refused track — each reported once per run. Capping is still
right, since a cue arrives per tick; capping them *together* was not.

**No new observable state on a sink beyond that.** M6b.2 asked for the miss to
become observable; the answer here is that the common case stops being reachable
rather than becoming countable.

## What this does not decide

**The scene grammar does not grow.** ADR-0018 is untouched and no `.ron` file
carries a sound name. `Cue`'s documentation used to say that *"a content-authored
sound name would be a different decision and would need the scene grammar to
grow, which M5.6c deliberately does not do."* **That sentence is still true.** It
describes a different road: this one opens the vocabulary in *code*, where a game
registers samples and gets a handle back. The two questions are separate and only
one of them is answered here.

What did become false in that comment is its other half — *"the sounds a build can
play are the ones its code synthesises, so the set is closed at compile time."*
The set is open now. The comment says both things, which is why it was rewritten
rather than deleted.

## Why an index is safe here, and the one thing that would change that

A `SoundId` is a position in the library that issued it, so the same number means
a different sound in a library with different registrations. That would be
serious if a cue were ever written down. **It is not, and this was measured
rather than assumed:**

- `Cue` derives `Debug, Clone, Copy, PartialEq` and no serde.
- `narvo-audio` names `serde` nowhere — not in `src/`, not in `Cargo.toml`.
- `recording.rs`, which writes ADR-0012's format, mentions neither `Cue` nor
  `audio`; a recording carries input actions.
- `narvo-ecs` names neither `narvo_audio` nor `Cue`, so no component holds one
  and ADR-0008's hash cannot see one.
- `narvo-ipc` mentions `narvo-audio` only in a doc sentence about staying a
  leaf; the one `Cue` in `ipc.rs` is inside the test module that opens at line
  1378 of a 3562-line file.

A cue exists from the extraction that built it to the sink that took it, inside
one tick, in the runner. `CueMemory`, the only thing that survives a tick, is
`pub(crate)` in `narvo-app` and held by the frame loop — outside the world,
outside the canonical dump, outside the hash.

**The consequence was measured too, not only argued.** The determinism artifacts
recorded from the pre-change commit `c8702c6` and from this change compare as
*26 files identical*.

**Revision condition.** Reopen the moment a cue is written anywhere that outlives
the process that made it — a recording, a dump, a protocol message, a save file.
An index is a handle into one run's registry, and none of those is one run.

## The remaining way a handle can fail, and why it is an `Option`

Two libraries in one process. A handle from a library with more registrations
does not resolve in a smaller one, which is why `SoundLibrary::get` returns an
`Option` at all rather than being total. A test holds exactly that case.

It is reported through the unknown-sound class and is the only path that still
reaches it. **A branded handle that made even this unwritable was considered and
rejected**: it needs either a lifetime brand or a per-library tag checked at
lookup, and the hazard it removes has no consumer — every runner in this
workspace builds one library. The cost of being wrong is one reported class on a
path that already reports.

## Alternatives, at their strongest

### (a) `Cue.sound` stays a name, as `String`

**The best argument, in full strength.** No registration step: a game writes
`Cue::play(tick, Channel::Sfx, "blip".into())` and is done. Names survive
everything — a log line reads without a library, a recording could carry one, a
scene file could name one later, and two libraries can never disagree about what
a cue meant because the cue carries the meaning. The engine would resolve a name
against whatever table it has, and an unknown one would be reported exactly as
today. **It is genuinely simpler**, and it is what most engines do.

**Why not.** It gives up `Copy` on `Cue`, since a `String` is owned: `NullSink`'s
`submit` does `*cue`, cue lists are compared whole in tests, and every expected
list in `audio.rs` would grow a `.to_owned()` per entry. That is a cost, not a
refutation. The refutation is that it leaves the defect exactly where M6b.2 found
it — `"blip"` stays writable, and the only thing standing between a typo and
silence is a guard that reports. This task exists because that guard reported
once per run across four classes and a second typo said nothing.

**Reopen if** a cue ever has to be written down (see the revision condition
above). A name is the right shape for a value that outlives its run, and at that
point the `Copy` argument is the smaller one.

### (b) The library lives inside each sink

Sinks would own what they play, and `submit` would keep its one-argument
signature. **Rejected on a measurement rather than on taste:** the runner prints
`Cue::note` whether or not a device opened — the comment at `window.rs` says so
in as many words, because the hearing check depends on that line existing on a
silent machine. A library inside the sink leaves the silent path with nothing to
turn a handle back into `click`. Two copies, one in the runner and one in the
sink, would be two tables that can disagree.

**Reopen if** the note ever stops being printed on the silent path.

### (c) `KiraSink` converts the whole library lazily

Simpler than the arrangement chosen: no conversion at construction, every sound
built on first play. **Rejected** because a two-second music bed is 96 000 frames
and the tick it starts on is not the place to build them. The sink converts at
construction *and* fills in on first use, which is not belt-and-braces: the first
half keeps the bed cheap, and the second keeps the sink's table from disagreeing
with the library about which sounds exist.

### (d) Leave the vocabulary closed and improve the message

**The best argument:** it is the smallest change, it touches no signature, and
M6b.2's complaint — that a miss is nearly invisible — is answerable with a
counter and a better sentence. **Rejected** because M7's scope sentence names
sound as something a game brings, and a run that reports a typo well is still a
run in which a game cannot ship its own audio.

## Consequences

- `Cue` stays `Copy`; a handle is four bytes.
- Four production sites moved, which is what the survey counted before the
  change: two `Sink::submit` calls, one `KiraSink::new`, one `Cue::note`.
- **The resolution is now exercised by the headless configuration.** Injecting a
  failure into `SoundLibrary::get` fails 2 tests in
  `cargo nextest run -p narvo-app --no-default-features` — a build with no
  `device` feature and no kira anywhere in its tree — and 7 across the workspace.
  Before this change the same defect was caught by nothing.
- The class-wise capping is held by one test, in `narvo-audio`. Injecting a
  collapse back to a single flag fails that one and **nothing in step 8**. That
  is a measured limit rather than a claim of coverage; the previous single `bool`
  had no test at all.
- `sounds::by_name` still exists and still takes a `&str`. It returns `Samples`
  and not a handle, so it cannot produce a playable cue; it is how
  `SoundLibrary::new` and the wav export walk `sounds::ALL`.
- `SoundLibrary::iter` exists because the private field works: a module outside
  `library` cannot build a `SoundId`, so the backend had no way to walk the
  library at startup. Handing out the handles is the way through, and it is the
  constructor guarantee doing its job rather than an obstacle routed around.
- `Cargo.lock` did not move. No dependency, production or dev, was added.
