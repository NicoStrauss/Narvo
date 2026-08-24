//! The recording file: a run's input stream as editable text.
//!
//! Read ADR-0012 before changing anything here. The format is a decision, and
//! the property it was chosen for is not compactness but that a person can open
//! a ten-thousand-tick repro, cut it down to thirty ticks, and see whether the
//! bug survives.
//!
//! This module does no file I/O. It turns a [`Recording`] into a `String` and
//! back, and `main.rs` is what touches the disk — so every test here runs
//! without a temporary directory, and a parse error can be provoked by writing
//! the broken text inline where the reader can see it.

use std::error::Error;
use std::fmt;

use narvo_input::InputEvent;

use crate::scene_anchor::Anchor;
use crate::sim::Mode;

/// First word of the first line. A file that does not start with it is not a
/// recording, whatever its extension says.
const MAGIC: &str = "narvo-recording";

/// The format version this build writes and is willing to read.
///
/// A recording carries it in its first line, and a mismatch is a hard error
/// rather than a best effort: replaying a file written by a different format
/// silently is the failure this whole feature exists to prevent.
pub const FORMAT_VERSION: u32 = 1;

/// Last line of a complete recording.
///
/// Without it a file cut short by a failed write, a full disk or an interrupted
/// transfer would parse as a valid, shorter recording — and a replay of it would
/// diverge from the original with nothing to say why. Deleting lines from the
/// middle by hand keeps the marker, so shortening a repro stays a two-second
/// job; losing the tail does not.
const TERMINATOR: &str = "end";

/// One recorded input, with the tick it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedInput {
    /// The tick this event is delivered in.
    pub tick: u64,
    /// What arrived.
    pub event: InputEvent,
}

/// A run's input stream, plus everything else needed to reproduce the run.
///
/// # The header holds what determines a run, and nothing that results from one
///
/// Mode, seed and tick count go in. The final state and its hash deliberately do
/// not: a hash stored in a file in this repository is a hash that a `ron` or
/// `serde` release can invalidate without anything being wrong, which ADR-0008
/// forbids for exactly that reason. A replay is checked by running it against
/// the original *now*, in one build, not against a number written down last
/// month.
///
/// # Only ticks with input are stored
///
/// An empty tick is implicit. A recording of ten thousand mostly-idle ticks is
/// then a few hundred lines instead of ten thousand, which is the difference
/// between a diff a person reads and one they skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    mode: Mode,
    seed: u64,
    ticks: u64,
    scene: Option<Anchor>,
    inputs: Vec<RecordedInput>,
}

impl Recording {
    /// Starts an empty recording for a run of `ticks` ticks.
    #[must_use]
    pub fn new(mode: Mode, seed: u64, ticks: u64) -> Self {
        Self {
            mode,
            seed,
            ticks,
            scene: None,
            inputs: Vec::new(),
        }
    }

    /// Records which scene file this run was constituted from.
    ///
    /// Mandatory for a [`Mode::SceneFile`] run and rejected for any other, which
    /// [`parse`](Self::parse) enforces on the way back in: a recording that
    /// named a scene it did not start from would be a recording nothing could
    /// check.
    pub fn anchor_to(&mut self, anchor: Anchor) {
        self.scene = Some(anchor);
    }

    /// The scene this run was constituted from, when it was constituted from
    /// one.
    #[must_use]
    pub fn scene(&self) -> Option<&Anchor> {
        self.scene.as_ref()
    }

    /// Appends an event.
    ///
    /// # Panics
    ///
    /// Panics if `tick` is before the last appended tick, or is not inside the
    /// run this recording describes. Both are programming errors in the caller —
    /// the runner appends as it walks ticks upwards — rather than conditions a
    /// file can be in, and the parser enforces the same two invariants with
    /// errors because there the input is untrusted.
    pub fn push(&mut self, tick: u64, event: InputEvent) {
        assert!(
            tick < self.ticks,
            "tick {tick} is outside a recording of {} ticks",
            self.ticks
        );
        if let Some(last) = self.inputs.last() {
            assert!(
                tick >= last.tick,
                "input arrived for tick {tick} after tick {} was already recorded; \
                 a recording is written in tick order",
                last.tick
            );
        }

        self.inputs.push(RecordedInput { tick, event });
    }

    /// Cuts the band to cover exactly `ticks` ticks and no more.
    ///
    /// **D19's band cut, and the whole of its mechanism.** A run that accepts a
    /// state set from outside is reproducible up to the moment that write landed
    /// and not past it, so the band stops describing what came after.
    /// ADR-0012's M6.2 amendment holds the decision; this is the code it comes
    /// to, and the seam in `crate::ipc` is what calls it.
    ///
    /// **It counts ticks rather than naming one, and M6.3d is why.** M6.3b spelled
    /// this `cut_after(tick)` and computed `tick + 1` inside, because a write
    /// after tick *N*'s systems is reproducible by a run of *N + 1* ticks — so the
    /// count was always the quantity and the tick was its disguise. A run that is
    /// **waiting** then arrived and could not be spelled at all: no tick is
    /// running, and the band has to be closed at however many ticks have already
    /// run, which may be **zero**. The tick form's smallest answer was one.
    ///
    /// **The cut is expressed by this number and by nothing else.** There is no
    /// marker, and the format could not carry one worth having: only ticks that
    /// have input are stored (ADR-0012 Decision 2), so a band that stops early is
    /// byte-indistinguishable from a run that simply had nothing to record after
    /// that point. What *is* checkable — that no recorded input lies at or beyond
    /// the count — is asserted here on the way out and enforced by
    /// [`parse`](Self::parse)'s `EventBeyondEnd` on the way back in. A `cut`
    /// header field would be an unverifiable claim in a format whose stated rule
    /// is that every deviation is an error.
    ///
    /// A cut band is therefore an ordinary, complete recording of a shorter run:
    /// it keeps its `end` marker, it parses, `sim::validate_recording` accepts
    /// it, and a replay runs it to the cut.
    ///
    /// # Panics
    ///
    /// Panics if the cut would lengthen the band, or if an input has already been
    /// recorded at or beyond the new count. Both are programming errors in the
    /// caller — the seam cuts at the count it has reached, and everything recorded
    /// so far is before it — rather than conditions a file can be in.
    pub fn cut_to(&mut self, ticks: u64) {
        assert!(
            ticks <= self.ticks,
            "a band of {} ticks cannot be cut to {ticks}; a cut only ever shortens",
            self.ticks
        );
        if let Some(last) = self.inputs.last() {
            assert!(
                last.tick < ticks,
                "cutting to {ticks} ticks would leave the input recorded at tick {} past \
                 the end, and a band like that does not parse",
                last.tick
            );
        }

        self.ticks = ticks;
    }

    /// Lengthens the band to `ticks`, because the run it describes grew.
    ///
    /// The mirror of [`cut_to`](Self::cut_to), and the two are the only
    /// ways this number moves after construction: one only shortens, one only
    /// lengthens. A run's length stopped being fixed before tick 0 when `step`
    /// arrived (M6.3c), and a band whose count did not follow would **under**-claim
    /// — it would describe a shorter run than the one that happened, and a replay
    /// of it would stop early while looking perfectly valid. That is the silent
    /// class M6.3b measured: nothing reading a recording can tell a short band
    /// from a quiet one.
    ///
    /// A **cut** band is never lengthened. The caller checks
    /// (`Inbox::band_is_open`) rather than this function, because "the band is
    /// closed" is the runner's state and not the file's.
    ///
    /// # Panics
    ///
    /// Panics if `ticks` is below the current count. Shortening is
    /// [`cut_to`](Self::cut_to)'s job and has its own invariant to keep.
    pub fn extend_to(&mut self, ticks: u64) {
        assert!(
            ticks >= self.ticks,
            "a band of {} ticks cannot be extended to {ticks}; an extension only ever \
             lengthens, and shortening is what cut_to is for",
            self.ticks
        );

        self.ticks = ticks;
    }

    /// Which simulation this recording belongs to.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The seed the recorded run was started with.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// How many ticks the recorded run covered.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Every recorded input, in tick order.
    #[must_use]
    pub fn inputs(&self) -> &[RecordedInput] {
        &self.inputs
    }

    /// Writes the recording as text.
    ///
    /// The output is canonical: the header fields come in one fixed order and
    /// the events in tick order, so two recordings of the same run are the same
    /// bytes and a diff between two runs shows only what actually differs.
    #[must_use]
    pub fn render(&self) -> String {
        use fmt::Write as _;

        let mut text = String::new();
        writeln!(text, "{MAGIC} {FORMAT_VERSION}").expect("writing into a String cannot fail");
        writeln!(text, "mode {}", self.mode.name()).expect("writing into a String cannot fail");
        writeln!(text, "seed {}", self.seed).expect("writing into a String cannot fail");
        writeln!(text, "ticks {}", self.ticks).expect("writing into a String cannot fail");
        // Written only when there is one, which is what keeps every recording
        // made before scene starts existed byte for byte what it was.
        if let Some(scene) = &self.scene {
            writeln!(text, "scene {}", scene.path()).expect("writing into a String cannot fail");
            writeln!(text, "scene-sha256 {}", scene.digest())
                .expect("writing into a String cannot fail");
        }
        text.push('\n');

        for input in &self.inputs {
            writeln!(
                text,
                "{} {} {}",
                input.tick,
                input.event.action(),
                input.event.value()
            )
            .expect("writing into a String cannot fail");
        }

        writeln!(text, "{TERMINATOR}").expect("writing into a String cannot fail");
        text
    }

    /// Reads a recording back.
    ///
    /// Blank lines are ignored and a line whose first non-blank character is `#`
    /// is a comment, so a repro can be annotated with what it is supposed to
    /// show.
    ///
    /// # Errors
    ///
    /// [`RecordingError`], naming the line and what was wrong with it. Every
    /// deviation is an error: there is no lenient mode, because a replay that
    /// quietly does something other than what was recorded is worse than no
    /// replay at all.
    pub fn parse(text: &str) -> Result<Self, RecordingError> {
        let mut lines = text
            .lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line.trim()))
            .filter(|(_number, line)| !line.is_empty() && !line.starts_with('#'));

        let (_number, first) = lines.next().unwrap_or((1, ""));
        parse_magic(first)?;

        let mut mode = None;
        let mut seed = None;
        let mut ticks = None;
        let mut scene_path = None;
        let mut scene_digest = None;
        let mut inputs: Vec<RecordedInput> = Vec::new();
        let mut terminated = false;

        for (number, line) in lines {
            if terminated {
                return Err(RecordingError::TrailingContent {
                    line: number,
                    text: line.to_owned(),
                });
            }
            if line == TERMINATOR {
                terminated = true;
                continue;
            }

            // A header field is `name value`; an event is `tick action value`
            // and so has one field more. The tick being a number is what tells
            // the two apart, and it is why header names are words.
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields.as_slice() {
                [name, value] => {
                    parse_header_field(
                        number,
                        name,
                        value,
                        Header {
                            mode: &mut mode,
                            seed: &mut seed,
                            ticks: &mut ticks,
                            scene_path: &mut scene_path,
                            scene_digest: &mut scene_digest,
                        },
                    )?;
                }
                [tick, action, value] => {
                    let ticks = ticks.ok_or(RecordingError::EventBeforeHeader { line: number })?;
                    inputs.push(parse_event(number, tick, action, value, ticks, &inputs)?);
                }
                _ => {
                    return Err(RecordingError::MalformedLine {
                        line: number,
                        text: line.to_owned(),
                    });
                }
            }
        }

        if !terminated {
            return Err(RecordingError::Truncated);
        }

        let mode = mode.ok_or(RecordingError::MissingHeaderField { field: "mode" })?;
        let scene = scene_anchor(mode, scene_path, scene_digest)?;

        Ok(Self {
            mode,
            seed: seed.ok_or(RecordingError::MissingHeaderField { field: "seed" })?,
            ticks: ticks.ok_or(RecordingError::MissingHeaderField { field: "ticks" })?,
            scene,
            inputs,
        })
    }
}

/// Pairs the two scene header fields with the mode, and refuses every other
/// combination.
///
/// The rule in one place: a `scene-file` recording carries both fields, and
/// anything else carries neither. A recording naming a scene it did not start
/// from would point a replay at a file it never read; one *not* naming the scene
/// it did start from could not be checked at all, which is the whole of
/// ADR-0019.
fn scene_anchor(
    mode: Mode,
    path: Option<String>,
    digest: Option<String>,
) -> Result<Option<Anchor>, RecordingError> {
    match (mode, path, digest) {
        (Mode::SceneFile, Some(path), Some(digest)) => Ok(Some(Anchor::from_parts(path, digest))),
        (Mode::SceneFile, None, _) => Err(RecordingError::MissingHeaderField { field: "scene" }),
        (Mode::SceneFile, _, None) => Err(RecordingError::MissingHeaderField {
            field: "scene-sha256",
        }),
        (_, None, None) => Ok(None),
        (mode, _, _) => Err(RecordingError::SceneAnchorOnModeWithoutOne { mode }),
    }
}

/// The header slots a `name value` line can fill.
struct Header<'a> {
    mode: &'a mut Option<Mode>,
    seed: &'a mut Option<u64>,
    ticks: &'a mut Option<u64>,
    scene_path: &'a mut Option<String>,
    scene_digest: &'a mut Option<String>,
}

/// Checks the first line and the format version on it.
fn parse_magic(line: &str) -> Result<(), RecordingError> {
    let mut fields = line.split_whitespace();

    if fields.next() != Some(MAGIC) {
        return Err(RecordingError::NotARecording {
            first_line: line.to_owned(),
        });
    }

    let found = fields.next().unwrap_or("");
    match found.parse::<u32>() {
        Ok(FORMAT_VERSION) => Ok(()),
        _ => Err(RecordingError::UnsupportedVersion {
            found: found.to_owned(),
        }),
    }
}

/// Reads one `name value` header line into the slot it belongs to.
fn parse_header_field(
    line: usize,
    name: &str,
    value: &str,
    header: Header<'_>,
) -> Result<(), RecordingError> {
    /// Fills a header slot, refusing a second value for it.
    fn once<T>(
        slot: &mut Option<T>,
        line: usize,
        name: &str,
        value: T,
    ) -> Result<(), RecordingError> {
        if slot.is_some() {
            return Err(RecordingError::DuplicateHeaderField {
                line,
                field: name.to_owned(),
            });
        }
        *slot = Some(value);
        Ok(())
    }

    match name {
        "mode" => {
            let parsed = Mode::parse(value).ok_or_else(|| RecordingError::UnknownMode {
                line,
                mode: value.to_owned(),
            })?;
            once(header.mode, line, name, parsed)
        }
        "seed" => once(header.seed, line, name, parse_number(line, "seed", value)?),
        "ticks" => once(
            header.ticks,
            line,
            name,
            parse_number(line, "ticks", value)?,
        ),
        "scene" => once(header.scene_path, line, name, value.to_owned()),
        "scene-sha256" => {
            if value.len() != 64 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(RecordingError::MalformedDigest {
                    line,
                    value: value.to_owned(),
                });
            }
            once(header.scene_digest, line, name, value.to_owned())
        }
        _ => Err(RecordingError::UnknownHeaderField {
            line,
            field: name.to_owned(),
        }),
    }
}

/// Reads one `tick action value` event line.
fn parse_event(
    line: usize,
    tick: &str,
    action: &str,
    value: &str,
    ticks: u64,
    so_far: &[RecordedInput],
) -> Result<RecordedInput, RecordingError> {
    let tick = parse_number(line, "tick", tick)?;

    if tick >= ticks {
        return Err(RecordingError::EventBeyondEnd { line, tick, ticks });
    }
    if let Some(last) = so_far.last()
        && tick < last.tick
    {
        return Err(RecordingError::OutOfOrder {
            line,
            tick,
            previous: last.tick,
        });
    }

    let value = value
        .parse::<i64>()
        .map_err(|_| RecordingError::MalformedNumber {
            line,
            field: "value",
            text: value.to_owned(),
        })?;

    let event = InputEvent::new(action, value).map_err(|_| RecordingError::InvalidActionName {
        line,
        name: action.to_owned(),
    })?;

    Ok(RecordedInput { tick, event })
}

/// Reads an unsigned field, naming it if it is not a number.
fn parse_number(line: usize, field: &'static str, text: &str) -> Result<u64, RecordingError> {
    text.parse().map_err(|_| RecordingError::MalformedNumber {
        line,
        field,
        text: text.to_owned(),
    })
}

/// Why a recording could not be read.
///
/// Every variant names the line it happened on, because the file is expected to
/// be hand-edited and "somewhere in this file" is not a useful thing to tell
/// whoever just edited it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingError {
    /// A `scene-sha256` value is not sixty-four hex digits.
    MalformedDigest {
        /// The line it was on.
        line: usize,
        /// What was written instead.
        value: String,
    },
    /// A recording names a scene for a mode that is not constituted from one.
    SceneAnchorOnModeWithoutOne {
        /// The mode the recording names.
        mode: Mode,
    },
    /// The first line is not the recording magic.
    NotARecording {
        /// What the first line said instead.
        first_line: String,
    },
    /// The format version is not the one this build reads.
    UnsupportedVersion {
        /// The version found in the file.
        found: String,
    },
    /// A line is neither a header field nor an event.
    MalformedLine {
        /// Line number, counting from one.
        line: usize,
        /// The line itself.
        text: String,
    },
    /// A header field that this format does not have.
    UnknownHeaderField {
        /// Line number, counting from one.
        line: usize,
        /// The name that was given.
        field: String,
    },
    /// A header field appeared twice.
    DuplicateHeaderField {
        /// Line number of the second occurrence.
        line: usize,
        /// The repeated name.
        field: String,
    },
    /// A required header field is absent.
    MissingHeaderField {
        /// Which one.
        field: &'static str,
    },
    /// The header names a simulation mode that does not exist.
    UnknownMode {
        /// Line number, counting from one.
        line: usize,
        /// The name that was given.
        mode: String,
    },
    /// A number could not be read.
    MalformedNumber {
        /// Line number, counting from one.
        line: usize,
        /// Which field it was.
        field: &'static str,
        /// The text that was there.
        text: String,
    },
    /// An event line appeared before the header said how long the run is.
    EventBeforeHeader {
        /// Line number, counting from one.
        line: usize,
    },
    /// An event is at a tick the run never reaches.
    EventBeyondEnd {
        /// Line number, counting from one.
        line: usize,
        /// The tick the event claims.
        tick: u64,
        /// How many ticks the header says the run has.
        ticks: u64,
    },
    /// Events are not in tick order.
    OutOfOrder {
        /// Line number, counting from one.
        line: usize,
        /// The tick that came too late.
        tick: u64,
        /// The tick before it.
        previous: u64,
    },
    /// An action name is not an identifier.
    InvalidActionName {
        /// Line number, counting from one.
        line: usize,
        /// The name that was rejected.
        name: String,
    },
    /// Something follows the end marker.
    TrailingContent {
        /// Line number, counting from one.
        line: usize,
        /// What was found there.
        text: String,
    },
    /// The end marker is missing.
    Truncated,
}

impl fmt::Display for RecordingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedDigest { line, value } => write!(
                f,
                "line {line}: \"{value}\" is not a SHA-256; a scene anchor is sixty-four \
                 lower-case hex digits, which is what this build writes"
            ),
            Self::SceneAnchorOnModeWithoutOne { mode } => write!(
                f,
                "this recording names a scene but its mode is \"{mode}\", which is built from \
                 code rather than from a file; only a \"scene-file\" run is constituted from a \
                 scene, and an anchor on anything else would point a replay at a file it never \
                 read"
            ),
            Self::NotARecording { first_line } => write!(
                f,
                "this is not an Narvo recording: the first line should start with \"{MAGIC}\", \
                 and it says \"{first_line}\""
            ),
            Self::UnsupportedVersion { found } => write!(
                f,
                "this recording is format version \"{found}\" and this build reads version \
                 {FORMAT_VERSION}; replaying it would risk doing something other than what was \
                 recorded, so it is refused rather than attempted"
            ),
            Self::MalformedLine { line, text } => write!(
                f,
                "line {line} is neither a header field (`name value`) nor an input \
                 (`tick action value`): \"{text}\""
            ),
            Self::UnknownHeaderField { line, field } => write!(
                f,
                "line {line}: \"{field}\" is not a header field of this format; \
                 it has mode, seed, ticks, and for a scene-file recording \
                 scene and scene-sha256"
            ),
            Self::DuplicateHeaderField { line, field } => write!(
                f,
                "line {line}: \"{field}\" is given twice, and which one would apply is not \
                 something to guess at"
            ),
            Self::MissingHeaderField { field } => write!(
                f,
                "the header has no \"{field}\"; a recording needs mode, seed and ticks to \
                 reproduce a run, and a scene-file recording needs scene and \
                 scene-sha256 as well"
            ),
            Self::UnknownMode { line, mode } => write!(
                f,
                "line {line}: this recording is for simulation mode \"{mode}\", which this build \
                 does not have"
            ),
            Self::MalformedNumber { line, field, text } => write!(
                f,
                "line {line}: {field} should be a number and is \"{text}\""
            ),
            Self::EventBeforeHeader { line } => write!(
                f,
                "line {line} is an input, but the header has not said how many ticks the run has \
                 yet; the header comes first"
            ),
            Self::EventBeyondEnd { line, tick, ticks } => write!(
                f,
                "line {line}: an input is recorded for tick {tick}, but the run is {ticks} ticks \
                 long and so ends at tick {}; the input would never be delivered",
                ticks.saturating_sub(1)
            ),
            Self::OutOfOrder {
                line,
                tick,
                previous,
            } => write!(
                f,
                "line {line}: tick {tick} comes after tick {previous}, so the inputs are out of \
                 order; they are delivered in file order and a replay would drop this one"
            ),
            Self::InvalidActionName { line, name } => write!(
                f,
                "line {line}: \"{name}\" is not a usable action name; it has to be a non-empty \
                 run of ASCII letters, digits, `_`, `-` and `.`"
            ),
            Self::TrailingContent { line, text } => write!(
                f,
                "line {line} comes after the \"{TERMINATOR}\" marker: \"{text}\". \
                 The marker is what says the file is complete, so nothing may follow it"
            ),
            Self::Truncated => write!(
                f,
                "this recording has no \"{TERMINATOR}\" marker, so it is incomplete - most likely \
                 cut short by a failed write or a partial copy. Replaying it would silently \
                 deliver fewer inputs than were recorded"
            ),
        }
    }
}

impl Error for RecordingError {}

#[cfg(test)]
mod tests {
    use super::{FORMAT_VERSION, Recording, RecordingError};
    use crate::scene_anchor::Anchor;
    use crate::sim::Mode;
    use narvo_input::InputEvent;

    fn event(action: &str, value: i64) -> InputEvent {
        InputEvent::new(action, value).expect("the test names are valid")
    }

    /// **A cut band is byte-identical to a band that was always that long.**
    ///
    /// The whole of D19's cut, and the whole of what it costs: the format has one
    /// place to say how far a run got, so a band cut at tick 3 and a band from a
    /// run that only ever asked for four ticks render to the same bytes. Nothing
    /// reading the file can tell them apart, which is why no marker was added —
    /// a `cut` field would be a claim about a run that the format could not
    /// check, and this test is what says the claim would be uncheckable rather
    /// than merely unwanted.
    #[test]
    fn a_cut_band_is_byte_identical_to_a_band_that_was_always_that_short() {
        let mut cut = Recording::new(Mode::Input, 7, 100);
        let mut short = Recording::new(Mode::Input, 7, 4);
        for tick in [0, 2, 3] {
            cut.push(tick, event("thrust", 1));
            short.push(tick, event("thrust", 1));
        }
        cut.cut_to(4);

        assert_eq!(cut.ticks(), 4);
        assert_eq!(cut.render(), short.render());
        assert_eq!(cut, short);
    }

    /// A cut band parses back, keeps its `end` marker, and replays to the cut.
    #[test]
    fn a_cut_band_is_an_ordinary_complete_recording() {
        let mut band = Recording::new(Mode::Input, 7, 100);
        band.push(1, event("thrust", 1));
        band.push(5, event("thrust", -1));
        band.cut_to(10);

        let text = band.render();
        assert!(
            text.ends_with("end\n"),
            "the cut dropped the terminator:\n{text}"
        );

        let parsed = Recording::parse(&text).expect("a cut band is a recording like any other");
        assert_eq!(parsed, band);
        assert_eq!(parsed.ticks(), 10);
        assert_eq!(parsed.inputs().len(), 2);
    }

    /// A cut that would leave an input past the end is refused rather than
    /// producing a band that does not parse.
    #[test]
    #[should_panic(expected = "past the end")]
    fn a_cut_before_a_recorded_input_is_refused() {
        let mut band = Recording::new(Mode::Input, 7, 100);
        band.push(9, event("thrust", 1));
        band.cut_to(4);
    }

    /// A cut only ever shortens.
    #[test]
    #[should_panic(expected = "a cut only ever shortens")]
    fn a_cut_cannot_lengthen_a_band() {
        let mut band = Recording::new(Mode::Input, 7, 10);
        band.cut_to(50);
    }

    /// **A band follows a run that grew**, which is the other half of the cut.
    ///
    /// Since M6.3c a run's length is not settled before tick 0 — a `step` raises
    /// it — and a band whose count did not follow would describe a shorter run
    /// than the one that happened. It would still parse, still replay, and still
    /// look valid, which is what makes it worth a test rather than a comment.
    #[test]
    fn a_band_follows_a_run_that_grew() {
        let mut band = Recording::new(Mode::Input, 7, 4);
        band.push(1, event("thrust", 1));
        band.extend_to(9);

        assert_eq!(band.ticks(), 9);

        // Still an ordinary recording afterwards, and the input it already held
        // is untouched.
        let parsed = Recording::parse(&band.render()).expect("what it wrote, it reads");
        assert_eq!(parsed, band);
        assert_eq!(parsed.inputs().len(), 1);

        // And it now accepts input at a tick the original budget did not cover,
        // which is the assertion `push` would otherwise have failed.
        band.push(8, event("thrust", -1));
        assert_eq!(band.inputs().len(), 2);
    }

    /// **The cut checked against something that did not move with it.**
    ///
    /// `cut_after(N)` computed `N + 1` inside; `cut_to(ticks)` takes the number
    /// as given. Code and tests were converted together in M6.3d, so an
    /// off-by-one in the conversion would have been invisible in both — this
    /// project's most-repeated failure class in its purest form.
    ///
    /// So neither assertion here derives its expectation from the cut. The first
    /// reads the **rendered header**, which is the format's own output and was
    /// `ticks N` long before any of this; the second is a **literal** that a
    /// reader can check against the file format by eye. A test that computed
    /// `4` the way `cut_to` computes it would be checking arithmetic against
    /// itself.
    #[test]
    fn a_cut_bands_header_says_the_number_it_was_cut_to() {
        let mut band = Recording::new(Mode::Input, 7, 100);
        band.push(1, event("thrust", 1));
        band.cut_to(4);

        let text = band.render();
        assert!(
            text.contains("\nticks 4\n"),
            "the header does not say the number the band was cut to:\n{text}"
        );
        assert!(
            !text.contains("ticks 100"),
            "the old count survived:\n{text}"
        );

        // And back through the parser, which never saw the cut at all.
        let parsed = Recording::parse(&text).expect("a cut band is a recording");
        assert_eq!(parsed.ticks(), 4);
    }

    /// An extension only ever lengthens; shortening is the cut's job.
    #[test]
    #[should_panic(expected = "an extension only ever lengthens")]
    fn an_extension_cannot_shorten_a_band() {
        let mut band = Recording::new(Mode::Input, 7, 100);
        band.extend_to(10);
    }

    /// A small recording with an empty tick between two busy ones.
    fn sample() -> Recording {
        let mut recording = Recording::new(Mode::Motion, 7, 100);
        recording.push(3, event("thrust", 2));
        recording.push(3, event("turn", -1));
        recording.push(41, event("select", 9));
        recording
    }

    #[test]
    fn a_recording_renders_as_a_header_and_only_the_ticks_that_have_input() {
        assert_eq!(
            sample().render(),
            "narvo-recording 1\n\
             mode motion\n\
             seed 7\n\
             ticks 100\n\
             \n\
             3 thrust 2\n\
             3 turn -1\n\
             41 select 9\n\
             end\n"
        );
    }

    #[test]
    fn writing_and_reading_a_recording_gives_the_same_recording() {
        let original = sample();
        let restored = Recording::parse(&original.render()).expect("what was written reads back");

        assert_eq!(restored, original);
    }

    #[test]
    fn reading_and_writing_a_recording_gives_the_same_text() {
        // The other half of the round trip, and the one that pins the rendering
        // as canonical: two recordings of the same run are the same bytes.
        let text = sample().render();
        let again = Recording::parse(&text).expect("valid").render();

        assert_eq!(again, text);
    }

    #[test]
    fn a_run_with_no_input_at_all_is_a_valid_recording() {
        let empty = Recording::new(Mode::Motion, 0, 10);
        let text = empty.render();

        assert_eq!(
            text,
            "narvo-recording 1\nmode motion\nseed 0\nticks 10\n\nend\n"
        );
        let restored = Recording::parse(&text).expect("an empty recording is still a recording");
        assert_eq!(restored, empty);
        assert!(restored.inputs().is_empty());
    }

    #[test]
    fn blank_lines_and_comments_are_ignored() {
        let text = "\
# what this repro shows: the ship stops turning after tick 3
narvo-recording 1

mode motion
# the seed matters, the tick count does not
seed 7
ticks 100

3 thrust 2

# and then nothing happens for a while
41 select 9
end
";
        let recording = Recording::parse(text).expect("comments are allowed");

        assert_eq!(recording.inputs().len(), 2);
        assert_eq!(recording.seed(), 7);
    }

    #[test]
    fn the_header_fields_may_come_in_any_order() {
        let text = "narvo-recording 1\nticks 9\nseed 3\nmode chance\nend\n";
        let recording = Recording::parse(text).expect("order is not significant");

        assert_eq!(recording.mode(), Mode::Chance);
        assert_eq!((recording.seed(), recording.ticks()), (3, 9));
    }

    #[test]
    fn a_file_that_is_not_a_recording_says_so() {
        let error = Recording::parse("hello\nend\n").expect_err("no magic line");

        assert!(matches!(error, RecordingError::NotARecording { .. }));
        assert!(error.to_string().contains("hello"));
    }

    #[test]
    fn an_empty_file_is_rejected() {
        assert!(matches!(
            Recording::parse(""),
            Err(RecordingError::NotARecording { .. })
        ));
    }

    #[test]
    fn a_different_format_version_is_refused_rather_than_attempted() {
        let text = format!(
            "narvo-recording {}\nmode motion\nseed 1\nticks 5\nend\n",
            FORMAT_VERSION + 1
        );

        let error = Recording::parse(&text).expect_err("a future version cannot be read");

        assert!(matches!(error, RecordingError::UnsupportedVersion { .. }));
        let message = error.to_string();
        assert!(
            message.contains(&(FORMAT_VERSION + 1).to_string()),
            "{message}"
        );
    }

    #[test]
    fn a_truncated_file_is_detected_rather_than_read_as_a_shorter_run() {
        // The failure this marker exists for: cut the tail off a valid file and
        // every remaining line still parses. Without the marker this would be a
        // perfectly good recording of a run that never happened.
        let full = sample().render();
        let cut = &full[..full.len() - "41 select 9\nend\n".len()];

        assert!(
            Recording::parse(cut).is_err(),
            "a truncated recording must not parse"
        );
        assert_eq!(Recording::parse(cut), Err(RecordingError::Truncated));
    }

    #[test]
    fn content_after_the_end_marker_is_an_error() {
        let text = "narvo-recording 1\nmode motion\nseed 1\nticks 5\n1 go 1\nend\n2 go 1\n";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::TrailingContent { line: 7, .. })
        ));
    }

    #[test]
    fn a_missing_header_field_names_the_one_that_is_missing() {
        let text = "narvo-recording 1\nmode motion\nticks 5\nend\n";

        assert_eq!(
            Recording::parse(text),
            Err(RecordingError::MissingHeaderField { field: "seed" })
        );

        // The wording too, and that is M5.7's addition. Both header messages
        // listed three fields while the parser had accepted five since M4.3,
        // and nothing noticed because only the variant was ever asserted. An
        // error message is product; a test that stops at its variant lets its
        // text rot.
        assert_eq!(
            RecordingError::MissingHeaderField { field: "seed" }.to_string(),
            "the header has no \"seed\"; a recording needs mode, seed and ticks to reproduce \
             a run, and a scene-file recording needs scene and scene-sha256 as well"
        );
    }

    #[test]
    fn a_repeated_header_field_is_not_guessed_at() {
        let text = "narvo-recording 1\nmode motion\nseed 1\nseed 2\nticks 5\nend\n";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::DuplicateHeaderField { line: 4, .. })
        ));
    }

    #[test]
    fn an_unknown_header_field_is_rejected() {
        let text = "narvo-recording 1\nmode motion\nseed 1\nticks 5\nweather rain\nend\n";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::UnknownHeaderField { line: 5, .. })
        ));

        assert_eq!(
            RecordingError::UnknownHeaderField {
                line: 5,
                field: "weather".to_owned()
            }
            .to_string(),
            "line 5: \"weather\" is not a header field of this format; it has mode, seed, \
             ticks, and for a scene-file recording scene and scene-sha256"
        );
    }

    #[test]
    fn a_mode_this_build_does_not_have_is_an_error() {
        let text = "narvo-recording 1\nmode teleport\nseed 1\nticks 5\nend\n";

        let error = Recording::parse(text).expect_err("there is no teleport mode");

        assert!(matches!(error, RecordingError::UnknownMode { line: 2, .. }));
        assert!(error.to_string().contains("teleport"));
    }

    #[test]
    fn an_input_past_the_end_of_the_run_is_an_error() {
        let text = "narvo-recording 1\nmode motion\nseed 1\nticks 5\n5 go 1\nend\n";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::EventBeyondEnd {
                line: 5,
                tick: 5,
                ticks: 5
            })
        ));
    }

    #[test]
    fn inputs_out_of_tick_order_are_an_error() {
        let text = "narvo-recording 1\nmode motion\nseed 1\nticks 50\n9 go 1\n4 go 1\nend\n";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::OutOfOrder {
                line: 6,
                tick: 4,
                previous: 9
            })
        ));
    }

    #[test]
    fn a_malformed_number_names_the_field_and_the_line() {
        let text = "narvo-recording 1\nmode motion\nseed later\nticks 5\nend\n";

        let error = Recording::parse(text).expect_err("`later` is not a number");
        assert!(matches!(
            error,
            RecordingError::MalformedNumber {
                line: 3,
                field: "seed",
                ..
            }
        ));
        assert!(error.to_string().contains("later"));
    }

    #[test]
    fn a_line_with_the_wrong_number_of_fields_is_an_error() {
        let text = "narvo-recording 1\nmode motion\nseed 1\nticks 5\n1 go 1 extra\nend\n";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::MalformedLine { line: 5, .. })
        ));
    }

    #[test]
    fn an_input_before_the_tick_count_is_an_error() {
        // The tick count is what an event line is validated against, so an event
        // above it in the file cannot be checked at all.
        let text = "narvo-recording 1\nmode motion\nseed 1\n1 go 1\nticks 5\nend\n";

        assert_eq!(
            Recording::parse(text),
            Err(RecordingError::EventBeforeHeader { line: 4 })
        );
    }

    #[test]
    fn an_action_name_that_could_not_be_written_back_is_rejected_on_reading() {
        let text = "narvo-recording 1\nmode motion\nseed 1\nticks 5\n1 gö! 1\nend\n";

        let error = Recording::parse(text).expect_err("`gö!` is not an identifier");
        assert!(matches!(
            error,
            RecordingError::InvalidActionName { line: 5, .. }
        ));
    }

    #[test]
    fn the_rendered_text_uses_no_carriage_returns() {
        // A recording is copied between Windows and Linux, and a line ending
        // that changed in transit would change what parses - and, if it ever
        // reached the state, what a replay produced.
        let text = sample().render();

        assert!(!text.contains('\r'));
        assert!(text.ends_with('\n'));
    }

    #[test]
    #[should_panic(expected = "outside a recording")]
    fn recording_an_input_past_the_end_of_the_run_is_a_programming_error() {
        Recording::new(Mode::Motion, 1, 5).push(5, event("go", 1));
    }

    #[test]
    #[should_panic(expected = "in tick order")]
    fn recording_an_input_out_of_order_is_a_programming_error() {
        let mut recording = Recording::new(Mode::Motion, 1, 50);
        recording.push(9, event("go", 1));
        recording.push(4, event("go", 1));
    }

    // ---- the scene anchor, added additively in M4.3 ---------------------

    /// A recording written before scene starts existed still parses.
    ///
    /// **The premise of M4.3's part two, checked rather than assumed.**
    /// ADR-0012's consequences say that "changing the format means bumping
    /// `FORMAT_VERSION`, and every recording in existence stops replaying". That
    /// is true of a change to what a field *means*; it is not true of a field
    /// that is simply new and optional, and this is the difference. The text
    /// below is exactly what version 1 wrote before this task, byte for byte.
    #[test]
    fn a_recording_from_before_the_anchor_still_parses() {
        let old = concat!(
            "narvo-recording 1\n",
            "mode input\n",
            "seed 1\n",
            "ticks 3\n",
            "\n",
            "0 thrust 1\n",
            "2 thrust 0\n",
            "end\n"
        );

        let recording = Recording::parse(old).expect("an older recording still replays");

        assert_eq!(recording.mode(), Mode::Input);
        assert_eq!(recording.ticks(), 3);
        assert_eq!(recording.inputs().len(), 2);
        assert_eq!(recording.scene(), None);
    }

    /// A recording of a mode-based run is byte for byte what it always was.
    ///
    /// The other half of the same premise, and the one the determinism suite
    /// depends on: the seventeen cases recorded before this task have to stay
    /// identical, and they would not if the anchor were written unconditionally
    /// or the version bumped.
    #[test]
    fn a_mode_based_recording_renders_exactly_as_it_did_before() {
        let mut recording = Recording::new(Mode::Input, 1, 3);
        recording.push(0, InputEvent::new("thrust", 1).expect("an identifier"));

        assert_eq!(
            recording.render(),
            concat!(
                "narvo-recording 1\n",
                "mode input\n",
                "seed 1\n",
                "ticks 3\n",
                "\n",
                "0 thrust 1\n",
                "end\n"
            )
        );
    }

    /// An anchored recording round-trips through its own text.
    #[test]
    fn a_scene_anchor_survives_the_round_trip() {
        let mut recording = Recording::new(Mode::SceneFile, 0, 4);
        recording.anchor_to(Anchor::from_parts(
            "crates/narvo-app/scenes/case.ron".to_owned(),
            "a".repeat(64),
        ));

        let text = recording.render();
        assert!(
            text.contains(
                "scene crates/narvo-app/scenes/case.ron
"
            ),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "scene-sha256 {}
",
                "a".repeat(64)
            )),
            "{text}"
        );

        assert_eq!(Recording::parse(&text).expect("reads back"), recording);
    }

    /// A `scene-file` recording without its anchor is refused.
    #[test]
    fn a_scene_file_recording_must_carry_its_anchor() {
        let text = "narvo-recording 1
mode scene-file
seed 0
ticks 1

end
";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::MissingHeaderField { field: "scene" })
        ));

        let half = concat!(
            "narvo-recording 1
",
            "mode scene-file
",
            "seed 0
",
            "ticks 1
",
            "scene a.ron
",
            "
",
            "end
"
        );
        assert!(matches!(
            Recording::parse(half),
            Err(RecordingError::MissingHeaderField {
                field: "scene-sha256"
            })
        ));
    }

    /// An anchor on a mode built from code is refused, by name.
    #[test]
    fn a_mode_built_from_code_may_not_name_a_scene() {
        let text = format!(
            "narvo-recording 1
mode motion
seed 0
ticks 1
scene a.ron
scene-sha256 {}

end
",
            "b".repeat(64)
        );

        let error = Recording::parse(&text).expect_err("motion is built from code");
        assert!(matches!(
            error,
            RecordingError::SceneAnchorOnModeWithoutOne { .. }
        ));
        assert!(error.to_string().contains("built from"), "{error}");
    }

    /// A digest that is not sixty-four hex digits is refused where it is read.
    #[test]
    fn a_malformed_digest_is_refused_with_its_line() {
        let text = concat!(
            "narvo-recording 1\n",
            "mode scene-file\n",
            "seed 0\n",
            "ticks 1\n",
            "scene a.ron\n",
            "scene-sha256 nothex\n",
            "\n",
            "end\n"
        );

        let error = Recording::parse(text).expect_err("that is not a digest");
        match &error {
            RecordingError::MalformedDigest { line, value } => {
                assert_eq!(*line, 6);
                assert_eq!(value, "nothex");
            }
            other => panic!("expected a malformed digest, got {other:?}"),
        }
        assert!(error.to_string().contains("sixty-four"), "{error}");
    }

    /// A build that does not know the anchor refuses a recording carrying one.
    ///
    /// Not something this build can demonstrate against itself — it knows the
    /// field — so what is pinned is the direction: an unknown header field is a
    /// hard error rather than something skipped. That is what makes an *older*
    /// build refuse a newer recording instead of replaying it without its scene.
    #[test]
    fn an_unknown_header_field_is_refused_rather_than_skipped() {
        let text = "narvo-recording 1
mode motion
seed 0
ticks 1
future 3

end
";

        assert!(matches!(
            Recording::parse(text),
            Err(RecordingError::UnknownHeaderField { .. })
        ));
    }

    // ---- what this format does *not* carry: state changes (M6.2, D19) ----
    //
    // ADR-0012's amendment of this milestone decided that a state change set
    // from outside the simulation — an IPC write — is **not** written into a
    // recording, and that a run which takes one has its band cut at that tick.
    // Nothing produces such a write yet; the transport is M6.3.
    //
    // These three hold the premises that decision rests on, so that a later
    // reader cannot mistake this file for something that already carries a
    // mutation, and so that building one is a red test rather than a quiet
    // extension. Each was measured here rather than inherited from the M5.2
    // amendment's reading of the same grammar.

    /// A mutation line written into a recording by hand is refused, by line.
    ///
    /// The direct guard on the decision: today there is no way to express "set
    /// this component to this value at this tick" in this format, and an
    /// attempt to is an error that quotes the line rather than something
    /// skipped. Whoever gives the format a mutation line has to make this test
    /// red on the way past, which is the point of it.
    #[test]
    fn a_state_change_line_is_not_something_this_format_carries() {
        let text = concat!(
            "narvo-recording 1\n",
            "mode input\n",
            "seed 1\n",
            "ticks 5\n",
            "\n",
            "1 thrust 1\n",
            "2 set 3v1 layer (depth:0.5)\n",
            "end\n"
        );

        let error = Recording::parse(text).expect_err("this format has no mutation line");
        match &error {
            RecordingError::MalformedLine { line, text } => {
                assert_eq!(*line, 7);
                assert_eq!(text, "2 set 3v1 layer (depth:0.5)");
            }
            other => panic!("expected the line to be refused as malformed, got {other:?}"),
        }
        // The message quotes the line, which is what makes the refusal usable
        // to whoever hand-wrote it.
        assert!(error.to_string().contains("2 set 3v1 layer"), "{error}");
    }

    /// The grammar dispatches on field count, and a three-field line is an
    /// input whatever its second word says.
    ///
    /// The sharp edge, and the reason the test above uses a five-field line: a
    /// mutation line may not be spelled with three fields, because this arm
    /// would read it as `tick action value` instead. `2 set 7` does not fail —
    /// it parses, as the action `set` with value 7. Any future mutation line
    /// has to avoid the arm rather than be added beside it.
    #[test]
    fn the_grammar_dispatches_on_field_count_and_three_fields_is_an_input() {
        let three = "narvo-recording 1\nmode input\nseed 1\nticks 5\n2 set 7\nend\n";
        let recording = Recording::parse(three).expect("three fields is an input line");
        assert_eq!(recording.inputs().len(), 1);
        assert_eq!(recording.inputs()[0].event.action(), "set");
        assert_eq!(recording.inputs()[0].event.value(), 7);

        // Two fields is a header slot, so a mutation cannot hide there either.
        let two = "narvo-recording 1\nmode input\nseed 1\nticks 5\nset 7\nend\n";
        assert!(matches!(
            Recording::parse(two),
            Err(RecordingError::UnknownHeaderField { .. })
        ));

        // Four and more is the arm the line above lands in.
        let four = "narvo-recording 1\nmode input\nseed 1\nticks 5\n2 set a b\nend\n";
        assert!(matches!(
            Recording::parse(four),
            Err(RecordingError::MalformedLine { line: 5, .. })
        ));
    }

    /// A registry value fits on a line but not into a whitespace-split field.
    ///
    /// **Measured here rather than reasoned about, and it went one way and not
    /// the other.** ADR-0030 has a component cross the agent protocol as the
    /// registry's own RON, so that is the text a mutation line would have to
    /// carry. Three of the eleven components in the engine registration set hold
    /// a string — `Sprite.region`, `HitRect.action` and `Tally.action` — and none
    /// of them restricts its charset where it is stored. Measured on `Sprite`,
    /// whose region name is unrestricted by construction:
    ///
    /// - a name with a space serializes to `(region:"with a space")`, which
    ///   `split_whitespace` sees as **three** fields. So a mutation line cannot
    ///   carry a value as trailing fields the way `tick action value` does;
    /// - a name with a newline serializes with a `\n` **escape**, two
    ///   characters, so the value stays on **one** line. `ron` escaping is what
    ///   makes that true, and it is why a line-based format is not ruled out
    ///   here — only the field-splitting is.
    ///
    /// Together those two say what a mutation line would cost: a rest-of-line
    /// rule for the value, and therefore a dispatch on something other than the
    /// field count the grammar uses today. That is the measurement ADR-0012's
    /// M6.2 amendment rests on, and it lives beside the parser it is about.
    #[test]
    fn a_registry_value_holds_spaces_but_never_a_line_break() {
        use narvo_ecs::{ComponentRegistry, Sprite, World};

        let mut registry = ComponentRegistry::new();
        registry
            .register_component::<Sprite>("sprite")
            .expect("a fresh registry accepts the type");

        let value = |name: &str| {
            let mut world = World::new();
            let entity = world.spawn();
            world
                .insert(entity, Sprite::new(name))
                .expect("just spawned");
            registry
                .serialize_component("sprite", &world, entity)
                .expect("the registry knows the name")
                .expect("the entity carries the component")
        };

        let spaced = value("with a space");
        assert_eq!(spaced, r#"(region:"with a space")"#);
        assert_eq!(
            spaced.split_whitespace().count(),
            3,
            "a value that splits into one field would not need a rest-of-line rule"
        );

        let broken = value("with\na newline");
        assert_eq!(broken, r#"(region:"with\na newline")"#);
        assert_eq!(
            broken.lines().count(),
            1,
            "a value spanning two lines could not be carried by a line-based format at all"
        );
    }
}
