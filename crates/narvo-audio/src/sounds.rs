//! The sounds this build can play, synthesised from parameters.
//!
//! # Why there is no file path here
//!
//! ADR-0028 keeps kira's decoder features off, because every one of them pulls
//! `symphonia` and turns `cargo deny check` red. So nothing in this workspace
//! reads an audio file, and nothing needs to: each sound below is a few lines of
//! arithmetic, and the result goes to the backend as raw samples.
//!
//! ADR-0024's rule against committed binaries applies to audio exactly as it
//! does to images — *"No PNG is committed, here or anywhere"* — so the audible
//! artefact a human can check is written under `target/` by a test, the M5.5a
//! pattern, and never checked in.
//!
//! # What is deliberately not here
//!
//! No mixing, no resampling, no envelope follower, no filter. Each sound is one
//! oscillator and, for the click, one decay envelope. The fade a layer arrives
//! with is **kira's** `Tween` on the channel's volume, not an amplitude ramp
//! computed here: ADR-0028 chose kira precisely because that tween is native,
//! and computing one here would be the in-house DSP the decision exists to
//! avoid.
//!
//! # These samples are outside the determinism domain
//!
//! The arithmetic below is `f32` and uses `sin`, whose last bit is not something
//! this project pins across platforms. That costs nothing here, because samples
//! are never dumped, never hashed and never compared by the determinism suite —
//! what that suite compares is the world, and audio only ever reads it
//! (ADR-0028). The **cue list** is the audio-side thing that is asserted, and it
//! is built from tick numbers and integer counters.

use std::f32::consts::TAU;

/// The sample rate everything here is generated at, in Hz.
///
/// One rate for every sound, so the backend never resamples — kira is told this
/// number and plays the frames as they are.
pub const SAMPLE_RATE: u32 = 48_000;

/// The name of the one-shot a `buy` makes.
pub const CLICK: &str = "click";

/// The name of the music bed that starts with the run.
pub const MUSIC_BASE: &str = "music_base";

/// The name of the layer that fades in once the counter is high enough.
pub const MUSIC_LAYER: &str = "music_layer";

/// A block of mono samples, the rate they are meant to be played at, and
/// whether they repeat.
///
/// Mono rather than stereo because Scope B has no panning and no positional
/// audio; the backend duplicates a sample into both channels at the seam.
#[derive(Debug, Clone, PartialEq)]
pub struct Samples {
    /// Samples per second.
    pub sample_rate: u32,
    /// The samples, nominally in `-1.0..=1.0`.
    pub mono: Vec<f32>,
    /// Whether the block repeats from the start when it reaches the end.
    ///
    /// **A property of the sound, not of the operation that plays it**, and that
    /// placement is the whole of M5.6d. Until then the decision lived in the
    /// backend's `match` on the cue's operation: `Op::LayerOn` played
    /// `sound.loop_region(..)` and `Op::Play` played the sound bare. The music
    /// bed arrives as a `Play`, so it stopped after one pass — audible, and not
    /// something any test could have caught, because nothing device-free knew
    /// what "should loop" meant.
    ///
    /// Here it is a value the backend only reads, so a test can assert it
    /// without a device and without kira.
    pub loops: bool,
}

impl Samples {
    /// How long the block lasts, in seconds.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        self.mono.len() as f32 / self.sample_rate as f32
    }

    /// The largest absolute sample, for a test that wants to know the block is
    /// neither silent nor clipping.
    #[must_use]
    pub fn peak(&self) -> f32 {
        self.mono.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
    }
}

/// How a sound is produced, for the table below.
pub type Synth = fn() -> Samples;

/// Every sound this build can play, by the name a [`Cue`](crate::Cue) uses.
///
/// A table rather than a `match` so that a backend can build all of them at
/// startup and a test can export all of them without either place carrying its
/// own list that could fall out of step.
pub const ALL: [(&str, Synth); 3] = [
    (CLICK, click),
    (MUSIC_BASE, music_base),
    (MUSIC_LAYER, music_layer),
];

/// Looks a sound up by the name a cue carries.
#[must_use]
pub fn by_name(name: &str) -> Option<Samples> {
    ALL.iter()
        .find(|(known, _)| *known == name)
        .map(|(_, make)| make())
}

/// A sine wave of `frequency` Hz lasting `seconds`, at `amplitude`.
///
/// The one generator everything else is built from.
fn sine(frequency: f32, seconds: f32, amplitude: f32) -> Vec<f32> {
    let count = (seconds * SAMPLE_RATE as f32).round() as usize;
    (0..count)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            amplitude * (TAU * frequency * t).sin()
        })
        .collect()
}

/// The click a `buy` makes: a short blip that decays to nothing.
///
/// 60 ms, so two clicks a tick apart are two clicks rather than one smear, and
/// an exponential decay so it reads as a tap rather than as a beep.
#[must_use]
pub fn click() -> Samples {
    const SECONDS: f32 = 0.06;
    const DECAY: f32 = 45.0;

    let mut mono = sine(880.0, SECONDS, 0.5);
    for (i, sample) in mono.iter_mut().enumerate() {
        let t = i as f32 / SAMPLE_RATE as f32;
        *sample *= (-DECAY * t).exp();
    }

    Samples {
        sample_rate: SAMPLE_RATE,
        mono,
        // A tap happens once. Looping it would turn the demo into a metronome.
        loops: false,
    }
}

/// The music bed: a steady tone that loops.
///
/// Two seconds at 220 Hz is exactly 440 whole cycles, so the end meets the
/// beginning at zero and the loop has no click in it. That is why the length and
/// the frequency are chosen together rather than separately.
#[must_use]
pub fn music_base() -> Samples {
    Samples {
        sample_rate: SAMPLE_RATE,
        mono: sine(220.0, LOOP_SECONDS, 0.25),
        loops: true,
    }
}

/// The layer that arrives at three: a fifth above the bed, and the same length.
///
/// 330 Hz for two seconds is 660 whole cycles, so this loops as cleanly as the
/// bed and the two stay in step for as long as they both play.
///
/// It is generated at full amplitude on purpose. What makes it *fade* in is
/// kira's `Tween` on the channel volume, not a ramp baked into these samples.
#[must_use]
pub fn music_layer() -> Samples {
    Samples {
        sample_rate: SAMPLE_RATE,
        mono: sine(330.0, LOOP_SECONDS, 0.22),
        loops: true,
    }
}

/// How long both looping sounds are, in seconds.
const LOOP_SECONDS: f32 = 2.0;

#[cfg(test)]
mod tests {
    use super::{
        ALL, CLICK, LOOP_SECONDS, MUSIC_BASE, MUSIC_LAYER, SAMPLE_RATE, by_name, click, music_base,
        music_layer,
    };

    #[test]
    fn every_sound_in_the_table_is_reachable_by_its_name() {
        for (name, _) in ALL {
            assert!(
                by_name(name).is_some(),
                "{name} is in the table but not found"
            );
        }
        assert!(by_name("no such sound").is_none());
    }

    #[test]
    fn the_table_names_are_the_exported_constants_and_are_distinct() {
        let names: Vec<&str> = ALL.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, vec![CLICK, MUSIC_BASE, MUSIC_LAYER]);

        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "names collide: {names:?}");
    }

    #[test]
    fn every_sound_is_audible_and_none_of_them_clips() {
        for (name, make) in ALL {
            let samples = make();
            assert_eq!(samples.sample_rate, SAMPLE_RATE, "{name}");
            assert!(!samples.mono.is_empty(), "{name} is empty");

            let peak = samples.peak();
            assert!(peak > 0.05, "{name} is effectively silent: peak {peak}");
            assert!(peak <= 1.0, "{name} clips: peak {peak}");
            assert!(
                samples.mono.iter().all(|s| s.is_finite()),
                "{name} carries a non-finite sample"
            );
        }
    }

    #[test]
    fn the_click_is_short_and_actually_decays() {
        let samples = click();
        assert!(samples.seconds() < 0.1, "{} s", samples.seconds());

        // The tail must be far quieter than the head, or "decay" is a claim the
        // envelope does not keep.
        let quarter = samples.mono.len() / 4;
        let head = samples.mono[..quarter]
            .iter()
            .fold(0.0_f32, |p, s| p.max(s.abs()));
        let tail = samples.mono[samples.mono.len() - quarter..]
            .iter()
            .fold(0.0_f32, |p, s| p.max(s.abs()));
        assert!(tail < head * 0.2, "head {head}, tail {tail}");
    }

    /// Both loops must start and end near zero, or the seam clicks every two
    /// seconds — which is the one defect a listener would notice immediately and
    /// a cue-list test could never catch.
    #[test]
    fn both_looping_sounds_join_end_to_start_without_a_step() {
        for (name, samples) in [(MUSIC_BASE, music_base()), (MUSIC_LAYER, music_layer())] {
            assert!(
                (samples.seconds() - LOOP_SECONDS).abs() < 1e-6,
                "{name} is {} s",
                samples.seconds()
            );

            let first = samples.mono[0];
            let last = *samples.mono.last().expect("not empty");
            let step = (first - last).abs();
            assert!(
                step < 0.02,
                "{name} steps by {step} across the loop seam (first {first}, last {last})"
            );
        }
    }

    /// The music loops over its whole length and the click does not.
    ///
    /// **The test M5.6d exists to make possible.** Before it, whether a sound
    /// repeated was decided inside the backend's `match` on the cue operation,
    /// where nothing device-free could see it — so the music bed playing once
    /// and stopping was audible to a human and invisible to the suite. The
    /// expectation is written out per sound rather than derived, so adding a
    /// fourth sound fails here until somebody says which it is.
    ///
    /// This runs in the headless configuration, which has no kira in it at all.
    #[test]
    fn the_music_loops_over_its_whole_length_and_the_click_does_not() {
        let expected = [(CLICK, false), (MUSIC_BASE, true), (MUSIC_LAYER, true)];

        assert_eq!(
            expected.len(),
            ALL.len(),
            "a sound was added or removed without saying whether it loops"
        );

        for (name, should_loop) in expected {
            let samples = by_name(name).unwrap_or_else(|| panic!("{name} is in the table"));
            assert_eq!(
                samples.loops, should_loop,
                "{name}: loops = {}, expected {should_loop}",
                samples.loops
            );
        }
    }

    /// A looping sound has to be worth looping: the seam assertion below only
    /// means something for the sounds that actually repeat.
    #[test]
    fn everything_that_loops_is_one_of_the_two_second_music_beds() {
        for (name, make) in ALL {
            let samples = make();
            if samples.loops {
                assert!(
                    (samples.seconds() - LOOP_SECONDS).abs() < 1e-6,
                    "{name} loops but is {} s rather than {LOOP_SECONDS} s",
                    samples.seconds()
                );
            }
        }
    }

    #[test]
    fn synthesis_is_repeatable_within_a_run() {
        for (name, make) in ALL {
            assert_eq!(make(), make(), "{name} differs between two calls");
        }
    }
}
