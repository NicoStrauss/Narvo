//! What a simulation asks to be heard, and the things that can hear it.
//!
//! # A cue is a value, and that is the whole design
//!
//! Nothing in this crate knows what an ECS is. A [`Cue`] says *what should be
//! heard and when*, in the simulation's own unit of time, and a [`Sink`] turns
//! cues into sound or into nothing. The extraction that reads a world and
//! produces cues lives in `narvo-app`, which is the only crate that sees both a
//! `World` and this vocabulary — ADR-0015's shape for the renderer, applied to a
//! second reader.
//!
//! ADR-0028 already decided the rule this follows: *"Whatever consumes these
//! cues must not write to the world: cues are extracted from state, and nothing
//! in ADR-0008's hash domain may move because a sound played."*
//!
//! # Time is in ticks, because the core has no clock
//!
//! [`Cue::tick`] and [`Cue::tween_ticks`] are simulation ticks. A cue produced
//! from a fixed-timestep simulation must not depend on how fast frames happen,
//! and a wall-clock duration here would make it depend on exactly that. The
//! translation to a real duration happens once, at the backend seam, from the
//! tick length the runner already knows — see [`KiraSink::new`].
//!
//! [`KiraSink::new`]: kira_sink::KiraSink::new
//!
//! # Two sinks, one of which is always here
//!
//! [`NullSink`] is compiled in every configuration, opens nothing and needs no
//! device. It is what tests use and what a headless run gets. The real backend
//! is behind the `device` feature, which `narvo-app` turns on from `render`.
//! **No test in this workspace opens an audio device.**

#![forbid(unsafe_code)]

pub mod library;
pub mod sounds;

#[cfg(feature = "device")]
pub mod kira_sink;

pub use library::{SoundId, SoundLibrary};
pub use sounds::Samples;

/// Which mixer channel a cue is addressed to.
///
/// Two, because Scope B names two things to control independently: the music bed
/// and the sounds an action makes. A third would be a new variant and a new
/// sub-track, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// The music bed, including any layers on top of it.
    Music,
    /// One-shot sounds an action makes.
    Sfx,
}

impl Channel {
    /// Every channel, in a fixed order.
    ///
    /// Used to build one sub-track per channel, so the tracks exist in the same
    /// order on every run.
    pub const ALL: [Channel; 2] = [Channel::Music, Channel::Sfx];

    /// The channel's name, for a log line.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Channel::Music => "music",
            Channel::Sfx => "sfx",
        }
    }
}

/// What a cue asks for.
///
/// # Which fields each operation reads
///
/// The struct is flat and two of its fields are not meaningful for every
/// operation. That is stated here rather than encoded in the type, because a
/// flat cue is what makes an expected list readable in a test — which is the
/// one place these values are ever compared.
///
/// | operation | reads `sound` | reads `target_db` | reads `tween_ticks` |
/// |---|---|---|---|
/// | [`Play`](Op::Play) | yes | no | no |
/// | [`Volume`](Op::Volume) | no | yes | yes |
/// | [`LayerOn`](Op::LayerOn) | yes | yes | yes |
/// | [`LayerOff`](Op::LayerOff) | yes | no | yes |
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Op {
    /// Play the named sound once, now.
    Play,
    /// Move the channel's volume to `target_db` over `tween_ticks`.
    Volume,
    /// Start the named sound as a looping layer, fading it to `target_db` over
    /// `tween_ticks`.
    LayerOn,
    /// Fade the named layer out over `tween_ticks` and stop it.
    LayerOff,
}

impl Op {
    /// The operation's name, for a log line and for a test's expected list.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Op::Play => "play",
            Op::Volume => "volume",
            Op::LayerOn => "layer_on",
            Op::LayerOff => "layer_off",
        }
    }
}

/// One thing that should be heard, at one tick.
///
/// # Why `sound` is a handle and not a name
///
/// It was a `&'static str` from M5.6c until M6b.2b, on the reasoning that *the
/// sounds a build can play are the ones its code synthesises, so the set is
/// closed at compile time*. **That premise is gone**: [`SoundLibrary::register`]
/// takes a consumer's own samples, so the set is open, and a string would make
/// `"blip"` a legal thing to write again — compiling, running, and reaching a
/// sink that has never heard of it. [`SoundId`] has no public constructor, so a
/// sound nobody registered is not something a cue can say.
///
/// What the old note got right and still holds is the `Copy`: a handle is four
/// bytes, so a cue stays `Copy`, and an expected list in a test stays readable.
///
/// # The other road, and why this is not it
///
/// The old note also said *"a content-authored sound name would be a different
/// decision and would need the scene grammar to grow, which M5.6c deliberately
/// does not do."* **That sentence is still true and still describes a road not
/// taken.** This one opens the vocabulary in *code*: a game registers samples and
/// emits cues from its own systems. ADR-0018 is untouched, and no `.ron` file
/// carries a sound name. ADR-0042 records why the two are separate questions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cue {
    /// The simulation tick this cue belongs to.
    pub tick: u64,
    /// Which channel it addresses.
    pub channel: Channel,
    /// Which registered sound, for the operations that name one.
    pub sound: SoundId,
    /// What to do.
    pub op: Op,
    /// The target volume in decibels, for the operations that use one.
    pub target_db: f32,
    /// How long the change should take, in simulation ticks.
    pub tween_ticks: u32,
}

impl Cue {
    /// A one-shot on `channel`.
    #[must_use]
    pub fn play(tick: u64, channel: Channel, sound: SoundId) -> Self {
        Self {
            tick,
            channel,
            sound,
            op: Op::Play,
            target_db: 0.0,
            tween_ticks: 0,
        }
    }

    /// A looping layer faded in to `target_db` over `tween_ticks`.
    #[must_use]
    pub fn layer_on(
        tick: u64,
        channel: Channel,
        sound: SoundId,
        target_db: f32,
        tween_ticks: u32,
    ) -> Self {
        Self {
            tick,
            channel,
            sound,
            op: Op::LayerOn,
            target_db,
            tween_ticks,
        }
    }

    /// The one line a runner prints per cue.
    ///
    /// The bridge between an ear and a machine: the hearing check compares what
    /// it hears against these, so the line names the tick, the channel, the
    /// operation and the sound rather than summarising them.
    ///
    /// # Why it takes the library
    ///
    /// Because the line has to keep saying `click` rather than `#0`. A handle is
    /// a position and the label lives in the library that issued it, so naming it
    /// needs both. A handle the library cannot answer renders as `#3`, which is
    /// what [`SoundId`]'s `Display` is for — the line still exists, and it says
    /// something a reader can act on.
    #[must_use]
    pub fn note(&self, library: &SoundLibrary) -> String {
        let sound = library
            .name_of(self.sound)
            .map_or_else(|| self.sound.to_string(), str::to_owned);

        format!(
            "cue tick {} {} {} {}",
            self.tick,
            self.channel.name(),
            self.op.name(),
            sound
        )
    }
}

/// Something that can receive cues.
///
/// Two implementations: [`NullSink`], which is always compiled and opens
/// nothing, and the kira-backed one behind the `device` feature.
pub trait Sink {
    /// Receives one cue, resolving its sound through `library`.
    ///
    /// # Why the library is a parameter and not a field
    ///
    /// So that both implementations resolve a handle through
    /// [`SoundLibrary::get`] — one function, one copy — rather than each holding
    /// a table of its own that could answer differently. It is the same reasoning
    /// ADR-0041 gives for keeping `depth_order` in one place, and it is what puts
    /// the lookup in a configuration the headless build compiles: `NullSink` is
    /// in every build, so an injection into the lookup fails a test that needs no
    /// audio device and no `device` feature.
    ///
    /// A field would also have forced a choice nobody should have to make: the
    /// runner prints [`Cue::note`] whether or not a device opened, so it needs
    /// the library on a path where there may be no sink at all.
    fn submit(&mut self, cue: &Cue, library: &SoundLibrary);
}

/// One report per failure class per run.
///
/// **Class-wise since M6b.2b, and it was a single `bool` before.** That bool
/// capped *every* class together, so a run that hit two different failures
/// mentioned one of them — M6b.2 measured exactly that: a second unknown sound in
/// the same run printed nothing at all. Capping is still right, because a cue
/// arrives per tick and an uncapped note would be sixty lines a second; capping
/// them *together* was not.
#[derive(Debug, Default)]
pub(crate) struct Notes {
    /// One flag per [`Mishap`], in its declaration order.
    said: [bool; Mishap::COUNT],
}

/// What can go wrong once.
///
/// Not public: a consumer observes nothing here. M6b.2 asked for the miss to
/// become observable, and the answer this task gives is that the common case
/// stops being reachable rather than becoming countable — a handle nobody was
/// given cannot be written. What is left is reported, once each.
///
/// # Why three of the four are unreachable without `device`
///
/// `MissingChannel`, `Playback` and `Track` are constructed only in
/// `kira_sink.rs`, and `device` gates that module. Without the feature the enum
/// still names them — `Notes` is sized by [`COUNT`](Mishap::COUNT) and the tests
/// below exercise all four — but nothing in the lib target builds one, which is
/// a property of the configuration rather than a defect.
///
/// `expect` rather than `allow`, for the reason `narvo-app`'s `main.rs` gives
/// at length: a variant that later does become reachable without a device makes
/// this annotation stale, and `expect` reports that where `allow` would keep
/// hiding it. `not(test)` is in the gate because the tests below construct all
/// four, so in that target the expectation would go unfulfilled and the lint
/// would fire the other way round.
#[cfg_attr(
    all(not(feature = "device"), not(test)),
    expect(dead_code, reason = "only `kira_sink.rs` constructs these three")
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mishap {
    /// The library did not issue this handle.
    UnknownSound,
    /// A channel has no track.
    MissingChannel,
    /// The backend refused to play something.
    Playback,
    /// The backend refused to create a track.
    Track,
}

impl Mishap {
    /// How many classes there are.
    pub(crate) const COUNT: usize = 4;

    /// The class's position in [`Notes::said`].
    fn slot(self) -> usize {
        match self {
            Mishap::UnknownSound => 0,
            Mishap::MissingChannel => 1,
            Mishap::Playback => 2,
            Mishap::Track => 3,
        }
    }
}

impl Notes {
    /// Says `what` on stderr, unless this class already said something.
    ///
    /// Returns whether it printed, which is what makes the capping testable
    /// without capturing stderr.
    pub(crate) fn once(&mut self, class: Mishap, what: &str) -> bool {
        let slot = class.slot();
        if self.said[slot] {
            return false;
        }
        self.said[slot] = true;
        eprintln!("audio: {what}; the run continues without that sound. This is said once per run");
        true
    }
}

/// A sink that opens no device.
///
/// Keeps what it was given when `recording` is set, so a test can assert the
/// list, and drops it otherwise, so a headless run pays nothing for the seam.
///
/// # It resolves, even though it plays nothing
///
/// Since M6b.2b it calls [`SoundLibrary::get`] on every cue and reports a handle
/// the library cannot answer. That is not scaffolding for a test: a headless run
/// is where the demo's ten thousand ticks happen, and a cue naming a sound the
/// run cannot play is a defect whether or not anyone is listening. Before this,
/// only the device-backed sink looked a sound up, and its one construction site
/// is production code with no test on it — so the check existed in exactly the
/// configuration nothing exercises.
#[derive(Debug, Default)]
pub struct NullSink {
    recording: bool,
    received: Vec<Cue>,
    notes: Notes,
}

impl NullSink {
    /// A sink that discards everything.
    #[must_use]
    pub fn discarding() -> Self {
        Self {
            recording: false,
            received: Vec::new(),
            notes: Notes::default(),
        }
    }

    /// A sink that keeps everything it is given.
    #[must_use]
    pub fn recording() -> Self {
        Self {
            recording: true,
            received: Vec::new(),
            notes: Notes::default(),
        }
    }

    /// What it was given, in the order it was given.
    #[must_use]
    pub fn received(&self) -> &[Cue] {
        &self.received
    }
}

impl Sink for NullSink {
    fn submit(&mut self, cue: &Cue, library: &SoundLibrary) {
        // The same call the device-backed sink makes, and the reason this one
        // takes a library at all: a handle the run cannot answer is a defect
        // here too, and this is the sink every configuration compiles.
        if library.get(cue.sound).is_none() {
            self.notes.once(
                Mishap::UnknownSound,
                &format!("no sound is registered as {}", cue.sound),
            );
        }

        if self.recording {
            self.received.push(*cue);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Channel, Cue, Mishap, Notes, NullSink, Op, Samples, Sink, SoundLibrary};

    #[test]
    fn a_discarding_sink_keeps_nothing_and_a_recording_one_keeps_order() {
        let library = SoundLibrary::new();
        let cues = [
            Cue::play(0, Channel::Music, SoundLibrary::MUSIC_BASE),
            Cue::play(7, Channel::Sfx, SoundLibrary::CLICK),
            Cue::layer_on(9, Channel::Music, SoundLibrary::MUSIC_LAYER, 0.0, 60),
        ];

        let mut discarding = NullSink::discarding();
        let mut recording = NullSink::recording();
        for cue in &cues {
            discarding.submit(cue, &library);
            recording.submit(cue, &library);
        }

        assert!(discarding.received().is_empty());
        assert_eq!(recording.received(), &cues);
    }

    #[test]
    fn a_note_names_the_tick_the_channel_the_operation_and_the_sound() {
        let library = SoundLibrary::new();

        assert_eq!(
            Cue::play(12, Channel::Sfx, SoundLibrary::CLICK).note(&library),
            "cue tick 12 sfx play click"
        );
        assert_eq!(
            Cue::layer_on(30, Channel::Music, SoundLibrary::MUSIC_LAYER, 0.0, 60).note(&library),
            "cue tick 30 music layer_on music_layer"
        );
    }

    /// A sound the consumer brought is named by the label it registered, which
    /// is what keeps the hearing check's bridge readable for a game's own audio.
    #[test]
    fn a_note_names_a_consumer_registered_sound_by_its_label() {
        let mut library = SoundLibrary::new();
        let blip = library.register(
            "blip",
            Samples {
                sample_rate: 48_000,
                mono: vec![0.5, -0.5],
                loops: false,
            },
        );

        assert_eq!(
            Cue::play(4, Channel::Sfx, blip).note(&library),
            "cue tick 4 sfx play blip"
        );

        // And a handle this library cannot answer still produces a line, with
        // the handle in place of the name rather than nothing at all. It has to
        // come from a library with *more* registrations than this one, which is
        // the only way to hold a handle this one cannot resolve — the first
        // attempt at this assertion used a second library of the same length and
        // the handle resolved, to `blip`.
        let mut bigger = SoundLibrary::new();
        let samples = library.get(blip).expect("just registered").clone();
        let _ = bigger.register("blip", samples.clone());
        let foreign = bigger.register("elsewhere", samples);
        assert_eq!(foreign.index(), 4);
        assert_eq!(library.len(), 4);
        assert_eq!(
            Cue::play(4, Channel::Sfx, foreign).note(&library),
            "cue tick 4 sfx play #4"
        );
    }

    /// The capping is per class since M6b.2b, and the two halves are asserted
    /// separately: the same class twice says nothing the second time, and a
    /// different class is not silenced by the first.
    #[test]
    fn a_note_is_capped_per_class_rather_than_once_in_total() {
        let mut notes = Notes::default();

        assert!(notes.once(Mishap::UnknownSound, "first"));
        assert!(!notes.once(Mishap::UnknownSound, "second of the same class"));

        assert!(notes.once(Mishap::MissingChannel, "a different class still speaks"));
        assert!(!notes.once(Mishap::MissingChannel, "and is then capped too"));

        assert!(notes.once(Mishap::Playback, "third class"));
        assert!(notes.once(Mishap::Track, "fourth class"));
        assert!(!notes.once(Mishap::Track, "capped"));
    }

    #[test]
    fn the_channel_list_is_every_variant_once() {
        let mut all = Channel::ALL.to_vec();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), Channel::ALL.len());
        assert!(Channel::ALL.contains(&Channel::Music));
        assert!(Channel::ALL.contains(&Channel::Sfx));
    }

    #[test]
    fn every_operation_has_a_distinct_name() {
        let names: Vec<&str> = [Op::Play, Op::Volume, Op::LayerOn, Op::LayerOff]
            .iter()
            .map(|op| op.name())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "names collide: {names:?}");
    }
}
