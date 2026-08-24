//! The sounds a run can play, and the only thing that can name one.
//!
//! # A sound is named by a handle, and the handle has no public constructor
//!
//! [`SoundId`]'s field is private, so the only way to hold one is to have been
//! given it by a [`SoundLibrary`] — either from [`SoundLibrary::register`], which
//! took the samples, or from one of the three constants below, which the library
//! always carries. **A sound nobody registered cannot be written down.**
//!
//! That is the whole point rather than a side effect. Until M6b.2b a cue named
//! its sound with a `&'static str`, and `"blip"` was a legal thing to write: it
//! compiled, it ran, and the backend said so once on stderr and carried on. M6b.2
//! measured the second half of that — a *second* typo in the same run said
//! nothing at all, because the note was capped once per run across every failure
//! class. §9.2's rule applies: where a mistake can be made unwritable, that beats
//! any guard against it.
//!
//! # What this does **not** open
//!
//! The scene grammar. ADR-0018 is untouched, and a sound name is still not a
//! thing a `.ron` file can carry. A game brings its samples in **code**, gets a
//! handle back, and emits cues from its own systems. `Cue`'s own documentation
//! explains why the *other* road — a content-authored name — is a different
//! decision that this one does not take.
//!
//! # There is no empty library
//!
//! [`SoundLibrary::new`] always registers the three sounds `sounds::ALL` carries,
//! at indices 0, 1 and 2, which is what makes [`SoundLibrary::CLICK`] and its two
//! siblings valid against *every* library rather than only against one built a
//! particular way. A constructor that skipped them would make those three
//! constants a handle into a library that does not contain them, which is the one
//! way a handle could lie.

use crate::sounds::{self, Samples};

/// A handle to one registered sound.
///
/// Opaque and [`Copy`]: opaque so that only a [`SoundLibrary`] can produce one,
/// and `Copy` so that [`Cue`](crate::Cue) stays `Copy` — a cue is compared and
/// listed in tests, and an owned name would cost that for nothing.
///
/// # It is an index, and that is safe here because a cue is not state
///
/// The value is a position in the library that issued it, so the *same* number
/// means a different sound in a library with different registrations. That would
/// be a serious property if a cue were ever written down, but M6b.2b measured
/// that it is not: `Cue` derives no serde, `narvo-audio` names serde nowhere,
/// the recording format (ADR-0012) carries input actions and no audio, and
/// `narvo-ecs` knows nothing of this crate. A cue exists from the extraction
/// that built it to the sink that took it, inside one tick, in the runner.
///
/// ADR-0042 records the measurement and the condition that would reopen it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SoundId(u32);

impl SoundId {
    /// The handle's position, for a message that has nothing better to say.
    ///
    /// Deliberately not a way back to a sound: it produces a number, and no
    /// function anywhere takes a number and returns a [`SoundId`].
    #[must_use]
    pub fn index(self) -> u32 {
        self.0
    }
}

impl std::fmt::Display for SoundId {
    /// Renders as `#3`, so a message that has lost its library still says which
    /// handle it means.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Every sound a run can play, and the handles that name them.
///
/// Built once by the runner and lent to whichever [`Sink`](crate::Sink) it has;
/// [`Sink::submit`](crate::Sink::submit) takes it, so **both sinks resolve a
/// handle through this same function** rather than each carrying a table of its
/// own. That is the arrangement ADR-0042 argues for, and it is what puts the
/// lookup in a configuration the headless build compiles and tests.
#[derive(Debug, Clone)]
pub struct SoundLibrary {
    /// Name and samples per handle; the handle is the index.
    entries: Vec<(String, Samples)>,
}

impl SoundLibrary {
    /// The click a one-shot action makes.
    pub const CLICK: SoundId = SoundId(0);

    /// The music bed that starts with the run.
    pub const MUSIC_BASE: SoundId = SoundId(1);

    /// The layer that fades in on top of the bed.
    pub const MUSIC_LAYER: SoundId = SoundId(2);

    /// A library carrying the three sounds this build synthesises.
    ///
    /// The order is `sounds::ALL`'s, which is what makes the three constants
    /// above correct. A test holds that rather than a comment.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: sounds::ALL
                .iter()
                .map(|(name, make)| ((*name).to_owned(), make()))
                .collect(),
        }
    }

    /// Adds `samples` under `name` and returns the handle that plays them.
    ///
    /// `name` is a label for messages and for [`Cue::note`](crate::Cue::note); it
    /// is not an identity. Two registrations with the same name are two sounds
    /// with two handles, because the handle is what a cue carries.
    pub fn register(&mut self, name: impl Into<String>, samples: Samples) -> SoundId {
        let index = u32::try_from(self.entries.len()).expect("far fewer than 2^32 sounds");
        self.entries.push((name.into(), samples));
        SoundId(index)
    }

    /// The samples `id` names, or `None` if this library did not issue it.
    ///
    /// **The one lookup both sinks make.** `None` is reachable only with a handle
    /// from a *different* library — see the type's documentation for why that is
    /// the single remaining way to hold a handle this library cannot answer.
    #[must_use]
    pub fn get(&self, id: SoundId) -> Option<&Samples> {
        self.entries.get(id.0 as usize).map(|(_, samples)| samples)
    }

    /// The label `id` was registered under, or `None` as [`Self::get`].
    #[must_use]
    pub fn name_of(&self, id: SoundId) -> Option<&str> {
        self.entries
            .get(id.0 as usize)
            .map(|(name, _)| name.as_str())
    }

    /// Every handle and its samples, in registration order.
    ///
    /// It exists because the private field works: a module outside this one
    /// cannot build a [`SoundId`], so a backend that wants to convert the whole
    /// library at startup has no way to walk it by index. That is the constructor
    /// guarantee doing its job rather than an obstacle to route around — the way
    /// around it is to hand out the handles, which is this.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (SoundId, &Samples)> + '_ {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, (_, samples))| {
                (
                    SoundId(u32::try_from(index).expect("far fewer than 2^32 sounds")),
                    samples,
                )
            })
    }

    /// How many sounds are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the library is empty.
    ///
    /// It never is — [`Self::new`] is the only constructor and it registers
    /// three. The method exists because `len` without it is a clippy warning, and
    /// answering `false` always is more honest than not answering.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SoundLibrary {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::SoundLibrary;
    use crate::sounds::{self, Samples};

    /// Samples that are not any of the three built-in ones.
    fn mine() -> Samples {
        Samples {
            sample_rate: 44_100,
            mono: vec![0.25, -0.25, 0.5, -0.5],
            loops: false,
        }
    }

    /// The three constants are correct because `new` registers in `ALL`'s order.
    /// Written out per constant rather than derived, so adding a fourth built-in
    /// without saying which handle it gets fails here.
    #[test]
    fn the_three_constants_name_the_three_synthesised_sounds() {
        let library = SoundLibrary::new();
        assert_eq!(library.len(), 3);
        assert!(!library.is_empty());

        assert_eq!(library.name_of(SoundLibrary::CLICK), Some(sounds::CLICK));
        assert_eq!(
            library.name_of(SoundLibrary::MUSIC_BASE),
            Some(sounds::MUSIC_BASE)
        );
        assert_eq!(
            library.name_of(SoundLibrary::MUSIC_LAYER),
            Some(sounds::MUSIC_LAYER)
        );
    }

    /// And they resolve to what `sounds` synthesises, not merely to something.
    #[test]
    fn every_built_in_handle_resolves_to_the_samples_sounds_synthesises() {
        let library = SoundLibrary::new();

        for (index, (name, make)) in sounds::ALL.iter().enumerate() {
            let id = [
                SoundLibrary::CLICK,
                SoundLibrary::MUSIC_BASE,
                SoundLibrary::MUSIC_LAYER,
            ][index];
            assert_eq!(library.name_of(id), Some(*name));
            assert_eq!(library.get(id), Some(&make()), "{name}");
        }
    }

    #[test]
    fn a_registered_sample_comes_back_by_its_handle() {
        let mut library = SoundLibrary::new();
        let id = library.register("blip", mine());

        assert_eq!(library.len(), 4);
        assert_eq!(library.name_of(id), Some("blip"));
        assert_eq!(library.get(id), Some(&mine()));
        // And it did not disturb the built-ins.
        assert_eq!(library.name_of(SoundLibrary::CLICK), Some(sounds::CLICK));
    }

    /// A name is a label, not an identity: two registrations under one name are
    /// two sounds, because the handle is what a cue carries.
    #[test]
    fn two_registrations_with_one_name_are_two_handles() {
        let mut library = SoundLibrary::new();
        let first = library.register("blip", mine());
        let second = library.register(
            "blip",
            Samples {
                sample_rate: 8_000,
                mono: vec![1.0],
                loops: true,
            },
        );

        assert_ne!(first, second);
        assert_eq!(library.name_of(first), library.name_of(second));
        assert_ne!(library.get(first), library.get(second));
    }

    /// The one remaining way to hold a handle a library cannot answer, and the
    /// reason [`SoundLibrary::get`] returns an `Option` at all.
    #[test]
    fn a_handle_from_a_larger_library_does_not_resolve_in_a_smaller_one() {
        let mut big = SoundLibrary::new();
        let foreign = big.register("blip", mine());
        let small = SoundLibrary::new();

        assert!(big.get(foreign).is_some());
        assert_eq!(small.get(foreign), None);
        assert_eq!(small.name_of(foreign), None);

        // A handle every library has still resolves in both.
        assert!(small.get(SoundLibrary::CLICK).is_some());
        assert!(big.get(SoundLibrary::CLICK).is_some());
    }

    /// The handle renders as something a message can carry when the library that
    /// would name it is not at hand.
    #[test]
    fn a_handle_renders_as_its_position() {
        let mut library = SoundLibrary::new();
        let id = library.register("blip", mine());

        assert_eq!(SoundLibrary::CLICK.to_string(), "#0");
        assert_eq!(id.to_string(), "#3");
        assert_eq!(id.index(), 3);
        assert_eq!(SoundLibrary::MUSIC_LAYER.index(), 2);
    }
}
