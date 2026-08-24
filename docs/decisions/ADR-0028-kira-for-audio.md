# ADR-0028: kira for audio, measured against Scope B, with the decoders switched off

Status: accepted · Date: 2026-08 · Scope: D2 in `ProjektPlan.md` §11; the audio
dependency of `narvo-app` and every audio task after it

## Context

`ProjektPlan.md` §6/M5 fixes the requirement profile — **Scope B**, set by the
maintainer: channels, per-channel volume, music layers, and volume tweens as the
candidate criterion. §11's D2 line says the decision falls *"per Messung gegen
dieses Profil"* rather than by recommendation, and names the M4.2 clap
measurement as the precedent for how a dependency's cost is established.

Nothing audio-shaped exists in the workspace: a case-insensitive search for
`audio`, `sound`, `kira`, `rodio`, `cpal` and `symphonia` across every `.rs` and
`.toml` finds three prose mentions in module documentation and nothing else.

The measurement ran in two throwaway crates **outside** the workspace, so
`Cargo.lock` was untouched until the decision was made.

## The rule, registered before the numbers

Written into the task before anything was measured, so the outcome could not be
chosen after the fact:

1. The candidate that covers the **whole** Scope B profile **natively** wins.
2. If both cover it: fewer additional crates.
3. If the crate counts are within ±10 %: the shorter clean build.
4. If neither covers it: **halt** and report the gap. No in-house mixer.

## The measurement

Two configurations were measured, and the difference between them is the finding
that matters most.

### As the crates come by default — and both are unusable here

| | kira 0.12.3 | rodio 0.22.2 |
|---|---|---|
| crates, normal edges | 48 | 54 |
| clean build | 14.655 s | 13.257 s |
| `symphonia` in the tree | yes | yes |

**Both fail `cargo deny check` in this configuration**, and neither number above
describes anything this project could adopt. `symphonia` is `MPL-2.0`
(`symphonia-0.6.0/Cargo.toml:40`), and `deny.toml:23-41` allows permissive
licences only — its comment is explicit: *"Permissive licenses only: no copyleft
anywhere in the tree, so shipping a statically linked game binary stays
unencumbered."* Both crates pull symphonia through their default decoder
features (kira: `mp3, ogg, vorbis, flac, wav, pcm`; rodio: `flac, mp3, mp4,
vorbis, wav`).

### As this project would take them

Decoders are switched off, because M5's sounds are **generated** rather than
loaded — nothing here decodes a file, and ADR-0024's rule against committed
binaries applies to audio as much as to images.

| | kira 0.12.3 | rodio 0.22.2 |
|---|---|---|
| features | `default-features = false, ["cpal"]` | `default-features = false, ["playback"]` |
| crates, normal edges, excluding the probe | **25** | **24** |
| clean build | **12.600 s** | **12.798 s** |
| `symphonia` | absent | absent |

The two are within 4 % on crates and 1.6 % on build time. Neither number decides
anything.

### Scope B coverage — this is what decides

| requirement | kira 0.12.3 | rodio 0.22.2 |
|---|---|---|
| separate channels | `add_sub_track` (`manager.rs:117`, `track/sub/handle.rs:58`), `TrackBuilder` (`track/sub/builder.rs:17`) | multiple players over a `Mixer` (`mixer.rs:46,57`) |
| per-channel volume at runtime | `set_volume` (`track/sub/handle.rs:88`) | `Player::set_volume` (`player.rs:178`) |
| simultaneous music layers | multiple sub-tracks, each playing | the mixer's purpose |
| **volume tweens** | **native, and part of the same call** | **not at this layer** |

The deciding line, quoted:

- kira, `track/sub/handle.rs:87-88` — *"Sets the (post-effects) volume of the
  mixer track"*, `pub fn set_volume(&mut self, volume: impl Into<Value<Decibels>>, tween: Tween)`.
  `Tween` is *"Describes a smooth transition between values"* with `start_time`,
  `duration` and `easing` (`tween.rs:87-96`). A tweened, per-channel, runtime
  volume change is one call.
- rodio, `player.rs:178` — `pub fn set_volume(&self, value: Float)`. Immediate,
  no duration.

**rodio is not without fades, and the fair statement of the gap matters.** It has
`fade_in(duration)` (`source/mod.rs:441`), `fade_out`, `crossfade`, and
`linear_gain_ramp(duration, start_value, end_value, clamp_end)`
(`source/mod.rs:517-523`). They are **`Source` adapters**: applied when a source
is wrapped, with the parameters known at that moment. What Scope B asks for is a
*channel's* volume moving smoothly *at runtime* — "duck the music while a sound
plays", decided after both are already playing. To get that from rodio one would
either pre-wrap every source with a ramp whose shape was known in advance, or
step `set_volume` from the frame loop by hand, which is the in-house tween the
halt rule exists to prevent.

## Decision

**kira, with `default-features = false` and the decoders off.**

Rule 1 settles it: kira covers all four requirements natively and rodio covers
three. The tiebreaks never fire, and it is worth recording that they would not
have contradicted the result — the crate counts are within ±10 %, so rule 3 would
have applied, and kira's clean build is the shorter of the two.

## Rejected: rodio

**Its best argument is real.** It is the smaller tree by one crate, it is the
more widely used of the two, its API is smaller and easier to hold in the head,
and for a game whose audio is "play this sound now" it would be entirely
adequate — which describes M5's click sound exactly.

What it does not do is the one requirement the maintainer named as the candidate
criterion. Scope B lists tweens explicitly, and the M7 slice is an incremental
where a music layer fading in as a threshold is crossed is the ordinary case
rather than an exotic one. Choosing rodio would mean writing that fade by hand
in the frame loop on the first day, which is precisely the in-house DSP §2 and
this task's halt rule exclude.

## Consequences

- **The decoders stay off, and that is a standing constraint rather than a
  default.** Turning on any of kira's format features pulls `symphonia` and
  turns `cargo deny check` red. A future task that wants to load an audio *file*
  is a licence decision before it is an audio decision.
- **Sounds are generated, never committed.** ADR-0024's rule — *"No PNG is
  committed, here or anywhere"* — is about binaries in the repository, and audio
  is the same class. The M5.5a demo generator is the precedent for where
  generated media goes: written under `target/` by a test.
- **`glam` enters the dependency tree**, as a transitive dependency of kira. That
  does **not** touch ADR-0014, which forbids a maths library's types inside a
  *registered component*, not in the tree. No component gains a `glam` field.
- **A device is never required by a test.** `cpal`'s
  `default_output_device(&self) -> Option<Self::Device>`
  (`cpal-0.17.3/src/traits.rs:69`) makes "no device" an ordinary outcome rather
  than an error, and `AudioManager::new` returns a `Result` (`manager.rs:70`).
  The design consequence is independent of what any particular machine has: the
  real backend is feature-gated like `render`, and a null backend serves tests,
  headless runs and CI.
- **Audio reads only.** Whatever consumes these cues must not write to the world:
  cues are extracted from state, and nothing in ADR-0008's hash domain may move
  because a sound played. That is the render path's rule applied to a second
  reader.

## Revision condition

Reopen if a measured need arises for something Scope B does not name — spatial
audio, effects, or loading a compressed file — because the first two would be a
new profile and the third is the licence question above.

Reopen if kira's tween model turns out not to cover a case the slice actually
has. The measurement above establishes that the API exists and is per-channel and
runtime; it does not establish that its easing set is sufficient for content
nobody has written yet.

Reopen if `cpal` stops being the portable output layer under both candidates,
since the two trees agree on it and this decision therefore does not choose it.

## Amendment, 2026-08-11 (M5.6b/M5.6c): the licence premise, and what it costs

M5.6b asked `cargo deny check` the question this decision had never actually put
to it, and the answer was no. **The measurement above is untouched and the
decision stands; one consequence below it was wrong and is replaced here.** This
ADR stays `accepted`.

### (i) The finding: `triple_buffer`, and why no feature configuration escapes it

kira 0.12.3 depends on `triple_buffer` 9.0.0, which is `MPL-2.0`
(`triple_buffer-9.0.0/Cargo.toml:40`, `license = "MPL-2.0"`). The declaration in
kira's manifest is the whole of it (`kira-0.12.3/Cargo.toml:176-177`):

```
[dependencies.triple_buffer]
version = "9.0.0"
```

One declaration, no `optional`, no `[target.…]`, no feature — `grep -n
triple_buffer` over that manifest returns that single line. So **every** kira
0.12.3 configuration carries it.

**The consequence recorded above under "The decoders stay off" is therefore
false as written.** It says that turning a format feature on "turns `cargo deny
check` red", which is true, and it was read — including by the M5.6b prompt,
which pre-registered "green with decoders off" — as meaning that turning them off
makes it green. It does not. Switching the decoders off removes `symphonia` and
does exactly that much: measured on the resolved tree, `grep -c symphonia
Cargo.lock` is `0` while the licence check is still red on `triple_buffer`.

The gap is not a lapse of attention and the record shows it: M5.6's own report
states that `cargo-deny` had passed only "against the **unchanged** tree — it has
not yet been asked about kira". The question was known to be open and was
answered at the first opportunity, which is the mechanism working.

This is a **fourth** reopening trigger, none of the three listed above: the
licence premise the decision rested on did not survive contact with the tree.

### (ii) Correction: the `cpal` anchor points at the rejected candidate

The consequence "A device is never required by a test" anchors on
`cpal-0.17.3/src/traits.rs:69`. kira 0.12.3 requires **cpal 0.18.1**
(`kira-0.12.3/Cargo.toml:182-184`), and the resolved lock confirms `cpal@0.18.1`;
0.17.3 is what **rodio** 0.22.2 requires — the candidate this ADR rejected.

The argument is unharmed, because the line is identical in both versions. The
correct anchor is `cpal-0.18.1/src/traits.rs:81`:

```rust
/// Returns `None` if no output device is available.
fn default_output_device(&self) -> Option<Self::Device>;
```

Recorded rather than silently edited, because the class of the mistake is worth
keeping visible: the evidence pointed at the wrong specimen while saying the
right thing.

### (iii) The decision: a named exception, not a change of course

Taken by the maintainer on 2026-08-11, in the D5 monetisation context rather than
as a technical call, and recorded here because it changes what this ADR obliges:
**D2 stays kira.** `deny.toml` carries a single-crate exception for
`triple_buffer` and nothing else.

The reasoning is the licence *class*. MPL-2.0 is file-level copyleft, and its own
text provides for the case this project is in — `triple_buffer-9.0.0/LICENSE:185-189`,
§3.3 *Distribution of a Larger Work*:

> You may create and distribute a Larger Work under terms of Your choice,
> provided that You also comply with the requirements of this License for
> the Covered Software.

The old policy — "no copyleft anywhere in the tree" — protected the right thing
with too coarse a raster. The new one distinguishes: strong copyleft stays
forbidden outright, file-level copyleft is admissible only as a named exception
with its reasoning beside it. A blanket `"MPL-2.0"` in `allow` was rejected
precisely because it would wave through the next such crate unexamined.

**None of this is legal advice, and the project does not treat it as such.** D5
carries two standing items that this amendment creates: every release ships a
third-party notices file carrying the MPL notice for this crate, and a lawyer
reviews the exception before any publication.

**The exit is documented so it is not rediscovered later.**
`triple_buffer-9.0.0/README.md:272-274`: *"More relaxed licensing (Apache, MIT,
BSD...) may also be negociated, in exchange of a financial contribution."*
Buying that removes the exception rather than extending it.

**What limits the exposure** is that kira stays behind an engine-owned facade,
the ADR-0002 pattern: a later exchange of the backend is a change of substructure
rather than a rebuild. The in-house mixer remains a documented fallback that is
deliberately not built.

### What this amendment does not change

The Scope B coverage table, the crate counts, the build times, and rule 1's
verdict of four-of-four against three-of-four. Every API citation in this ADR was
re-verified character for character in M5.6b, including the ones this amendment
does not touch.
