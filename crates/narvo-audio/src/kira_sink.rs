//! The one place this workspace touches an audio device.
//!
//! Everything above this module is values: a [`Cue`] is a struct, a
//! [`NullSink`](crate::NullSink) is a `Vec`. This is the seam where a cue
//! becomes sound, and it is deliberately the thinnest thing that can be. It is
//! behind the `device` feature, so a headless build does not contain it and no
//! test in this workspace reaches it.
//!
//! # Where the tick becomes a duration
//!
//! A [`Cue`] measures time in simulation ticks, because the core has no clock.
//! [`KiraSink::new`] takes the length of one tick and every tween duration is
//! that length times the cue's `tween_ticks`. This is the only multiplication in
//! the crate that turns simulation time into wall time, and having exactly one
//! is the point.
//!
//! # The tween is kira's
//!
//! ADR-0028 chose kira over rodio on one requirement: a tweened, per-channel,
//! runtime volume change in a single call. That call is used here as it comes —
//! `set_volume(volume, tween)`. Nothing in this file steps a volume by hand, and
//! nothing computes an amplitude ramp.
//!
//! # Handles are kept alive on purpose
//!
//! kira's `TrackHandle` documents that *"When a [`TrackHandle`] is dropped, the
//! corresponding mixer track will be removed"*
//! (`kira-0.12.3/src/track/sub/handle.rs:21-23`), and a removed track takes its
//! audio with it. So every track this sink creates is kept in the sink for as
//! long as the sink lives. Sound handles are the opposite case and are dropped:
//! there is no `Drop` under kira's `src/sound/`, so letting a one-shot's handle
//! go does not stop it.

use std::{collections::BTreeMap, time::Duration};

use kira::{
    AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Frame, ResourceLimitReached,
    Tween,
    sound::static_sound::{StaticSoundData, StaticSoundSettings},
    track::{TrackBuilder, TrackHandle},
};

use crate::{Channel, Cue, Mishap, Notes, Op, Sink, SoundId, SoundLibrary, sounds};

/// What can go wrong while building a sink.
///
/// Both halves are ordinary outcomes rather than bugs: a machine may have no
/// output device at all, which is why `narvo-app` treats a failure here as
/// "run without sound" rather than as a reason not to open a window.
#[derive(Debug)]
pub enum SinkError {
    /// The backend could not be started — no device, or none this process may
    /// have.
    Device(<DefaultBackend as kira::backend::Backend>::Error),
    /// A track could not be created because kira's capacity was reached.
    Track(ResourceLimitReached),
}

impl std::fmt::Display for SinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinkError::Device(error) => write!(f, "no audio output is available: {error:?}"),
            SinkError::Track(error) => write!(f, "an audio channel could not be created: {error}"),
        }
    }
}

impl std::error::Error for SinkError {}

/// A sink that plays cues through kira.
pub struct KiraSink {
    /// Held because dropping it stops everything.
    ///
    /// Never read after construction, and that is the point rather than an
    /// oversight: the manager owns the backend, so letting it go tears down the
    /// audio thread and every track with it. The lint is expected rather than
    /// allowed, so that if a later change does read the field the expectation
    /// becomes unfulfilled and says so.
    #[expect(
        dead_code,
        reason = "held for its Drop: dropping the manager stops all audio"
    )]
    manager: AudioManager<DefaultBackend>,
    /// One sub-track per [`Channel`], created once in [`Channel::ALL`] order.
    channels: BTreeMap<Channel, TrackHandle>,
    /// One sub-track per layer, a child of its channel's track, so a layer's
    /// volume moves without moving the bed underneath it.
    layers: BTreeMap<SoundId, TrackHandle>,
    /// The playable form of each sound, keyed by the handle that names it.
    ///
    /// Filled at construction for everything the library carried then, and
    /// filled in on first use for anything registered afterwards. **Both, and
    /// that is not belt-and-braces:** building at startup is what keeps a music
    /// bed from converting 96 000 frames on the tick it starts, and filling in
    /// later is what stops this table and [`SoundLibrary`] from disagreeing about
    /// which sounds exist. A sink that answered from a stale table would report a
    /// miss where `NullSink` reported none, and the two sinks agreeing is the
    /// point of resolving through one function.
    sounds: BTreeMap<SoundId, StaticSoundData>,
    /// The length of one simulation tick.
    tick: Duration,
    /// One report per failure class per run.
    notes: Notes,
}

impl std::fmt::Debug for KiraSink {
    /// Hand-written because `AudioManager` is not `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KiraSink")
            .field("channels", &self.channels.keys().collect::<Vec<_>>())
            .field("layers", &self.layers.keys().collect::<Vec<_>>())
            .field("sounds", &self.sounds.keys().collect::<Vec<_>>())
            .field("tick", &self.tick)
            .finish()
    }
}

impl KiraSink {
    /// Opens the default output device and builds one sub-track per channel.
    ///
    /// `tick` is how long one simulation tick lasts; it is what every
    /// `tween_ticks` is multiplied by. `library` is converted to kira's own
    /// form here, once, rather than on the tick a sound first plays.
    ///
    /// # Errors
    ///
    /// [`SinkError::Device`] if no output could be opened — an ordinary outcome
    /// on a machine without one — and [`SinkError::Track`] if a channel could
    /// not be created.
    pub fn new(tick: Duration, library: &SoundLibrary) -> Result<Self, SinkError> {
        let mut manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(SinkError::Device)?;

        let mut channels = BTreeMap::new();
        for channel in Channel::ALL {
            let track = manager
                .add_sub_track(TrackBuilder::new())
                .map_err(SinkError::Track)?;
            channels.insert(channel, track);
        }

        Ok(Self {
            manager,
            channels,
            layers: BTreeMap::new(),
            sounds: playable(library),
            tick,
            notes: Notes::default(),
        })
    }

    /// The playable form of `id`, converting and caching it if this is the first
    /// time it is asked for.
    ///
    /// `None` means the library did not issue the handle, which is the same
    /// answer [`NullSink`](crate::NullSink) gives and for the same reason: the
    /// lookup is [`SoundLibrary::get`], here and there.
    fn playable_sound(&mut self, id: SoundId, library: &SoundLibrary) -> Option<StaticSoundData> {
        if let Some(sound) = self.sounds.get(&id) {
            return Some(sound.clone());
        }

        let samples = library.get(id)?;
        let sound = static_sound(samples);
        self.sounds.insert(id, sound.clone());
        Some(sound)
    }

    /// The tween a cue asks for, in wall time.
    ///
    /// A `tween_ticks` of zero becomes [`Duration::ZERO`], which kira treats as
    /// "immediately" rather than as a division by zero: `calculate_new_raw_value`
    /// returns early when `tween.duration.is_zero()`
    /// (`kira-0.12.3/src/parameter.rs:155-157`).
    fn tween(&self, cue: &Cue) -> Tween {
        Tween {
            duration: self.tick * cue.tween_ticks,
            ..Tween::default()
        }
    }

    /// Says once per run **per class** that a cue could not be played.
    fn note_failure(&mut self, class: Mishap, what: &str) {
        self.notes.once(class, what);
    }
}

/// Every sound the library carries now, in kira's own form.
///
/// Separate from [`KiraSink::new`] so that the conversion has a name and the
/// constructor reads as what it is: open a device, make the tracks, convert the
/// sounds.
fn playable(library: &SoundLibrary) -> BTreeMap<SoundId, StaticSoundData> {
    library
        .iter()
        .map(|(id, samples)| (id, static_sound(samples)))
        .collect()
}

/// Wraps synthesised samples in the value kira plays.
///
/// Built by struct literal, which is the whole no-decoder path: every field of
/// `StaticSoundData` is public (`kira-0.12.3/src/sound/static_sound/data.rs:28-46`)
/// and the type carries no feature gate, so samples computed in
/// [`sounds`](crate::sounds) become playable audio without any format,
/// any file, or any decoder in the dependency tree.
///
/// # The loop is read here, not decided here
///
/// `samples.loops` is the sound's own property, so this function is the single
/// place a loop region is ever set and the `submit` match below has nothing to
/// say about it. `..` is the whole sound:
/// `impl From<RangeFull> for Region` yields
/// `Region { start: PlaybackPosition::Samples(0), end: EndPosition::EndOfAudio }`
/// (`kira-0.12.3/src/sound.rs:167-173`), and a `None` region means play once —
/// `StaticSoundSettings::new()` starts with `loop_region: None`
/// (`settings.rs:14`, `:39`).
fn static_sound(samples: &sounds::Samples) -> StaticSoundData {
    let data = StaticSoundData {
        sample_rate: samples.sample_rate,
        frames: samples.mono.iter().map(|s| Frame::from_mono(*s)).collect(),
        settings: StaticSoundSettings::new(),
        slice: None,
    };

    if samples.loops {
        data.loop_region(..)
    } else {
        data
    }
}

impl Sink for KiraSink {
    fn submit(&mut self, cue: &Cue, library: &SoundLibrary) {
        let tween = self.tween(cue);

        match cue.op {
            Op::Play => {
                let Some(sound) = self.playable_sound(cue.sound, library) else {
                    self.note_failure(
                        Mishap::UnknownSound,
                        &format!("no sound is registered as {}", cue.sound),
                    );
                    return;
                };
                let Some(track) = self.channels.get_mut(&cue.channel) else {
                    self.note_failure(Mishap::MissingChannel, "a channel is missing its track");
                    return;
                };
                // The handle is dropped here on purpose: a finite sound is not
                // stopped by dropping it.
                if track.play(sound).is_err() {
                    self.note_failure(
                        Mishap::Playback,
                        "kira refused a sound, its per-track limit is reached",
                    );
                }
            }

            Op::Volume => {
                let Some(track) = self.channels.get_mut(&cue.channel) else {
                    self.note_failure(Mishap::MissingChannel, "a channel is missing its track");
                    return;
                };
                track.set_volume(Decibels(cue.target_db), tween);
            }

            Op::LayerOn => {
                if !self.layers.contains_key(&cue.sound) {
                    let Some(sound) = self.playable_sound(cue.sound, library) else {
                        self.note_failure(
                            Mishap::UnknownSound,
                            &format!("no sound is registered as {}", cue.sound),
                        );
                        return;
                    };
                    let Some(parent) = self.channels.get_mut(&cue.channel) else {
                        self.note_failure(Mishap::MissingChannel, "a channel is missing its track");
                        return;
                    };

                    // Starts silent, so the tween below is what makes it
                    // audible rather than a jump followed by a fade.
                    let mut track =
                        match parent.add_sub_track(TrackBuilder::new().volume(Decibels::SILENCE)) {
                            Ok(track) => track,
                            Err(_) => {
                                self.note_failure(Mishap::Track, "kira refused a layer track");
                                return;
                            }
                        };

                    // No `loop_region` here any more: the sound already carries
                    // its own, set once in `static_sound`. A layer that did not
                    // loop would now be a wrong `loops` flag on the descriptor,
                    // which a device-free test can see, rather than a missing
                    // call in one arm of this match, which none could.
                    if track.play(sound).is_err() {
                        self.note_failure(Mishap::Playback, "kira refused a looping sound");
                        return;
                    }
                    self.layers.insert(cue.sound, track);
                }

                let Some(track) = self.layers.get_mut(&cue.sound) else {
                    return;
                };
                track.set_volume(Decibels(cue.target_db), tween);
            }

            Op::LayerOff => {
                let Some(track) = self.layers.get_mut(&cue.sound) else {
                    return;
                };
                track.set_volume(Decibels::SILENCE, tween);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::static_sound;
    use crate::sounds;
    use kira::sound::{EndPosition, PlaybackPosition, Region};

    /// Builds the value kira would play, without a manager and therefore without
    /// a device. This is the whole no-decoder claim, exercised.
    #[test]
    fn synthesised_samples_become_sound_data_without_a_decoder_or_a_device() {
        for (name, make) in sounds::ALL {
            let samples = make();
            let sound = static_sound(&samples);

            assert_eq!(sound.sample_rate, samples.sample_rate, "{name}");
            assert_eq!(sound.frames.len(), samples.mono.len(), "{name}");
            assert!(sound.slice.is_none(), "{name}");

            // `from_mono` puts the same sample in both channels.
            let first = sound.frames[0];
            assert_eq!(first.left, samples.mono[0], "{name}");
            assert_eq!(first.right, samples.mono[0], "{name}");
        }
    }

    /// The seam half of the loop assertion: what the descriptor says becomes
    /// exactly the region kira is given, and the region is the whole sound.
    ///
    /// No manager, no device, no playback — just the value that would be handed
    /// over. The descriptor half of the same claim is in `sounds`, where it also
    /// runs in the headless configuration that has no kira at all.
    #[test]
    fn the_descriptors_loop_flag_becomes_the_region_kira_is_given() {
        let whole = Region {
            start: PlaybackPosition::Samples(0),
            end: EndPosition::EndOfAudio,
        };

        for (name, make) in sounds::ALL {
            let samples = make();
            let sound = static_sound(&samples);

            if samples.loops {
                assert_eq!(
                    sound.settings.loop_region,
                    Some(whole),
                    "{name} should loop over its whole length"
                );
            } else {
                assert_eq!(
                    sound.settings.loop_region, None,
                    "{name} should not loop at all"
                );
            }
        }
    }

    /// And the flag is the only thing that decides it — the same samples with
    /// the flag flipped produce the opposite region, which is what makes this a
    /// seam rather than a coincidence.
    #[test]
    fn flipping_the_flag_flips_the_region_and_nothing_else() {
        let mut samples = sounds::click();
        let once = static_sound(&samples);
        samples.loops = true;
        let looping = static_sound(&samples);

        assert_eq!(once.settings.loop_region, None);
        assert!(looping.settings.loop_region.is_some());
        assert_eq!(once.frames, looping.frames, "the audio must be untouched");
        assert_eq!(once.sample_rate, looping.sample_rate);
    }
}
