//! Judging a run against the state somebody expected of it.
//!
//! **This is the third of the three things `ProjektPlan.md` §6/M6 closes on.**
//! Inspecting a running world and changing one have been reachable since M6.3a,
//! M6.3b and M6.4a; running a *repro test* was not, because nothing in this
//! workspace compared a run against an expectation. The determinism suite
//! compares two runs of one build, and `cargo xtask determinism compare`
//! compares two platforms — both are comparisons and neither is an oracle for a
//! single case.
//!
//! # What the oracle is, and where it comes from
//!
//! The expected value is **a canonical dump, produced by an earlier run and
//! handed to this one as a file**. Three properties follow, and each of them is
//! the reason for a rejected alternative:
//!
//! - **It is a dump and not a state hash.** A hash can say *that* two states
//!   differ and never *where*, and §6/M6 names `first_difference` as the
//!   diagnosis basis of the whole milestone. A repro for a defect at tick 500
//!   that reports sixteen hex digits has thrown away the entity and the
//!   component before the reader ever sees them.
//! - **It comes from outside this process.** [`judge`] takes it as a `&str` the
//!   caller read; nothing in this module or in `main.rs` can produce one. That
//!   is what rules out an oracle derived from the specimen it judges, which is
//!   the failure that would make every repro pass. `main.rs` reads the file
//!   **before the run starts**, so the ordering says it too.
//! - **It is never committed.** ADR-0008's headline rule names a hash — "No hash
//!   literal is ever committed to this repository" — and the sentence after it is
//!   the general one this rests on: determinism tests "never compare against a
//!   checked-in value — not in a test, not in documentation, not in a golden
//!   file." A committed dump is such a value, and the ADR's own one-question test
//!   ("what would have to change for this value to move?") answers *a dependency
//!   version, a serde field order, a compiler release*, which is its forbidden
//!   kind; the third, admitted kind excludes it too, by the first of its three
//!   conditions — a content anchor covers a generated artefact and not simulation
//!   state. **The step from "hash" to "dump" is a reading of that test, not a
//!   sentence quoted from the ADR**, and ADR-0035 is where it is made and argued.
//!
//! # Where the tick comes from
//!
//! From `--ticks`, and from nothing here. A run of *n* ticks is exactly the
//! prefix of a longer one (`headless::run`'s own property), so "the state at
//! tick 500" is "the state of a run that ended at tick 500". That is the same
//! idiom `cli::USAGE` already documents for bisecting a divergence, and it is
//! what lets a repro for a defect at tick 500 go red **at tick 500** rather than
//! at the end of a longer run.
//!
//! # Ungated, for the reason `ipc`, `audio` and `physics` are
//!
//! Comparing two strings needs no device, so steps two, eight and ten of the
//! verification set compile and test this in every configuration the repository
//! builds. Putting it behind `render` would place the judgement outside exactly
//! the checks that would catch a mistake in it — the M4.8 shape `CLAUDE.md`
//! records.

use std::fmt;
use std::path::Path;

use narvo_ecs::DumpDifference;

/// What comparing a run's state against an expected one concluded.
///
/// Five outcomes and not two, because the three that are neither a plain
/// agreement nor a plain divergence are exactly the ones a coarser type would
/// report as the wrong one of the pair. Two of them were found by measurement
/// rather than by design — see [`ByteOrderMark`](Self::ByteOrderMark).
#[derive(Debug)]
pub enum Verdict {
    /// The two states are the same text.
    Reproduced,
    /// The two states are the same state, written with different bytes.
    ///
    /// Line for line they agree; byte for byte they do not. That is a file whose
    /// line endings were translated on the way to disk, or whose final newline
    /// was dropped, and it is **not** a divergence. Reporting it as one would be a
    /// red result that says nothing about the simulation, which is the failure
    /// mode ADR-0008 exists to bound.
    ///
    /// **PowerShell's `>` is not this case, and the first draft of this comment
    /// said it was.** Measured on Windows PowerShell 5.1 against this binary:
    /// `narvo --mode chance --ticks 3 --dump > x` writes 2 163 bytes beginning
    /// `ef bb bf`, with 101 CR beside 101 LF. So it does translate the line
    /// endings — but it also prepends a byte-order mark, and a mark sits on the
    /// *first line*, which makes it [`ByteOrderMark`](Self::ByteOrderMark)
    /// instead. What reaches this variant is a CRLF translation with no mark: a
    /// checkout under `core.autocrlf`, or an editor that saved the file.
    ///
    /// It is a separate variant rather than folded into
    /// [`Reproduced`](Self::Reproduced) so the difference is *said* instead of
    /// swallowed: state equality in this workspace is `first_difference`
    /// returning `None` (`sim::assert_same_state` and the integration suite's
    /// copy both define it that way, and a second definition here would be a
    /// second definition of "the same state"), and a run that agrees under that
    /// definition while differing in bytes is worth one sentence to the reader.
    ReproducedInLinesOnly,
    /// The two states part company, here.
    Diverged(DumpDifference),
    /// The expected state is empty, so nothing was compared.
    ///
    /// Its own outcome because the alternative is the class this repository keeps
    /// finding: an empty expectation compared against a real dump would report a
    /// difference at line one and read as "the simulation diverged", when what
    /// happened is that the run which was supposed to write the file wrote
    /// nothing. `xtask determinism compare` refuses an empty manifest for the
    /// same reason.
    Nothing,
    /// The expected state begins with a byte-order mark, so its first line is not
    /// the dump's first line.
    ///
    /// **Its own outcome because the alternative is a message that shows a reader
    /// two identical-looking lines.** A mark is three invisible bytes in front of
    /// `entities`, so `first_difference` reports line 1 and prints
    /// `left  ﻿entities 33` against `right  entities 33` — a divergence the reader
    /// cannot see and would go looking for in the simulation.
    ///
    /// It is refused rather than stripped, because stripping would mean comparing
    /// something other than the file's bytes. `xtask determinism record` makes the
    /// same choice one step earlier, writing its dumps with `fs::write` rather
    /// than through a redirect "so no shell can reinterpret the line endings on
    /// the way".
    ///
    /// This is not hypothetical and not rare: it is what Windows PowerShell's `>`
    /// produces, measured on this binary.
    ByteOrderMark,
}

/// The byte-order mark, as the character a UTF-8 decoder hands back for `ef bb bf`.
const BYTE_ORDER_MARK: char = '\u{feff}';

impl Verdict {
    /// Whether the run reproduced the state expected of it.
    ///
    /// This is what the process's exit code is, and it is deliberately the only
    /// thing that decides it: a caller that wants to branch reads this, and
    /// everything else about the outcome is in the message.
    #[must_use]
    pub fn reproduced(&self) -> bool {
        match self {
            Self::Reproduced | Self::ReproducedInLinesOnly => true,
            Self::Diverged(_) | Self::Nothing | Self::ByteOrderMark => false,
        }
    }
}

/// Compares a run's canonical dump against the state expected of it.
///
/// `expected` is the left side and `produced` the right, so a reported
/// difference reads in the order [`Judgement`] labels it: left is the file,
/// right is this run.
///
/// # Why this takes two strings and reads nothing
///
/// So that the expected state cannot come from the run being judged. There is no
/// path through this module that produces one, and the caller that supplies it
/// reads its file before the simulation starts. An oracle derived from its own
/// specimen is the one construction that would make a repro runner pass
/// unconditionally, and it is excluded by shape rather than by care.
#[must_use]
pub fn judge(expected: &str, produced: &str) -> Verdict {
    // Before anything else, because a mark makes every later answer wrong rather
    // than merely unhelpful: it is invisible, it sits on line one, and it would be
    // reported as a divergence between two lines that read the same.
    if expected.starts_with(BYTE_ORDER_MARK) {
        return Verdict::ByteOrderMark;
    }

    // Trimmed rather than `is_empty`, because a file holding one newline is as
    // empty as one holding nothing, and a canonical dump is never either - it
    // always carries its `entities N` header. U+FEFF is not whitespace in current
    // Unicode, so a mark would survive this - which is why it is caught above.
    if expected.trim().is_empty() {
        return Verdict::Nothing;
    }

    match narvo_ecs::first_difference(expected, produced) {
        Some(difference) => Verdict::Diverged(difference),
        // `first_difference` walks lines, so it is blind to a translated line
        // ending and to a missing final newline. Byte equality is asked here,
        // after it, and answers a different question rather than overriding it.
        None if expected == produced => Verdict::Reproduced,
        None => Verdict::ReproducedInLinesOnly,
    }
}

/// A verdict together with what it was about, ready to be written out.
///
/// The message is this type's rather than the caller's because it is product:
/// a person reads it in a terminal and an agent reads it off stderr, and §6/M4's
/// rule that an error message is agent feedback applies to a verdict at least as
/// much as to an error.
pub struct Judgement<'a> {
    /// What the comparison concluded.
    pub verdict: &'a Verdict,
    /// The file the expected state was read from.
    pub expected: &'a Path,
    /// Ticks this run executed before its state was taken.
    pub ticks: u64,
}

impl fmt::Display for Judgement<'_> {
    /// One verdict line, then whatever the reader needs after it.
    ///
    /// **The first word of the first line is the verdict, and the two verdict
    /// words share no substring**: `reproduced` and `diverged`. A wording test
    /// holds that, because the obvious phrasing — "reproduced" against "not
    /// reproduced" — makes an agent that greps for the first one match both, and
    /// a machine-readable answer that is wrong half the time is worse than none.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path = self.expected.display();
        let ticks = self.ticks;

        match self.verdict {
            Verdict::Reproduced => write!(
                f,
                "reproduced - after {ticks} ticks this run's state is the one {path} records.\n    \
                 That says this build computes that state, and nothing about whether the state is \
                 right: a repro shows a state again, it does not judge it."
            ),
            Verdict::ReproducedInLinesOnly => write!(
                f,
                "reproduced - after {ticks} ticks this run's state is the one {path} records.\n    \
                 The two agree line for line and differ in bytes, so {path} has translated line \
                 endings or no final newline. The state is the same; the file is not what a \
                 `--dump` redirect wrote."
            ),
            Verdict::Diverged(difference) => write!(
                f,
                "diverged - after {ticks} ticks this run's state is not the one {path} records.\n\
                 {difference}\n    \
                 left is {path}, right is this run. Which of the two is correct is not a question \
                 this can answer; it says only that they are two states and where they part."
            ),
            Verdict::Nothing => write!(
                f,
                "{path} is empty, so this run was compared against nothing.\n    \
                 An expected state is what an earlier run wrote - `narvo ... --dump > {path}` - \
                 and an empty file means that run wrote none. Nothing was checked here."
            ),
            Verdict::ByteOrderMark => write!(
                f,
                "{path} begins with a byte-order mark, so this run was compared against nothing.\n    \
                 Three invisible bytes sit in front of `entities`, which makes the first line \
                 differ from the dump's first line while reading exactly like it. Windows \
                 PowerShell's `>` writes one; a redirect from a shell that does not, or \
                 `--dump` into a file written by anything else, does not. Nothing was checked here."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Judgement, Verdict, judge};
    use std::path::Path;

    /// A dump with two entities, in the shape `canonical_dump` writes.
    fn sample() -> String {
        "entities 2\nentity 0v1\n  position (x:1,y:2)\nentity 1v1\n  position (x:5,y:6)\n"
            .to_owned()
    }

    /// The same, with one number changed on the line given.
    fn sample_with(line: usize, replacement: &str) -> String {
        let mut lines: Vec<String> = sample().lines().map(str::to_owned).collect();
        lines[line - 1] = replacement.to_owned();
        lines.join("\n") + "\n"
    }

    /// The message a verdict produces about a file called `expected.dump`.
    fn message(verdict: &Verdict, ticks: u64) -> String {
        Judgement {
            verdict,
            expected: Path::new("expected.dump"),
            ticks,
        }
        .to_string()
    }

    #[test]
    fn a_run_that_lands_on_the_expected_state_reproduces_it() {
        let verdict = judge(&sample(), &sample());

        assert!(matches!(verdict, Verdict::Reproduced));
        assert!(verdict.reproduced());
    }

    #[test]
    fn a_changed_number_diverges_and_names_the_entity_and_the_component() {
        // The whole reason the oracle is a dump rather than a hash: what comes
        // back is where, not that.
        let verdict = judge(&sample(), &sample_with(5, "  position (x:5,y:7)"));

        let Verdict::Diverged(difference) = &verdict else {
            panic!("a changed number is a divergence, got {verdict:?}");
        };
        assert_eq!(difference.line(), 5);
        assert_eq!(difference.component(), Some("position"));
        assert_eq!(difference.entity().map(|entity| entity.index()), Some(1));
        assert_eq!(difference.left(), Some("  position (x:5,y:6)"));
        assert_eq!(difference.right(), Some("  position (x:5,y:7)"));
        assert!(!verdict.reproduced());
    }

    #[test]
    fn the_expected_state_is_the_left_side_and_the_run_is_the_right() {
        // Not cosmetic: the message tells a reader which side to change, and the
        // two sides are a file on disk and a simulation. Swapping them sends the
        // reader to fix the wrong one.
        let verdict = judge(&sample(), &sample_with(3, "  position (x:1,y:9)"));

        let Verdict::Diverged(difference) = &verdict else {
            panic!("expected a divergence");
        };
        assert_eq!(difference.left(), Some("  position (x:1,y:2)"));
        assert_eq!(difference.right(), Some("  position (x:1,y:9)"));
    }

    #[test]
    fn a_shorter_run_diverges_rather_than_agreeing_about_the_part_it_reached() {
        // The state after fewer ticks is a prefix of the state after more only in
        // the sense that the *run* is; the dump is not, and a comparison that
        // stopped at the shorter one would pass every truncated repro.
        let mut short = sample();
        short.truncate(
            short
                .find("entity 1v1")
                .expect("the sample has two entities"),
        );

        let verdict = judge(&short, &sample());

        assert!(!verdict.reproduced());
        assert!(matches!(verdict, Verdict::Diverged(_)));
    }

    #[test]
    fn an_empty_expected_state_is_not_a_pass_and_is_not_a_divergence() {
        // Two separate claims. It must not be green - nothing was checked - and
        // it must not read as a divergence, because the finding is that a file
        // was never written rather than that a simulation moved.
        for empty in ["", "\n", "   \n  \n"] {
            let verdict = judge(empty, &sample());

            assert!(matches!(verdict, Verdict::Nothing), "{empty:?}");
            assert!(!verdict.reproduced(), "{empty:?}");
        }
    }

    #[test]
    fn a_byte_order_mark_is_refused_by_name_rather_than_reported_as_a_divergence() {
        // Measured, not supposed: Windows PowerShell's `>` writes `ef bb bf` in
        // front of the dump. Without this the verdict is a divergence at line one
        // between `entities 2` and `entities 2`, and a reader goes looking for a
        // simulation defect that is not there.
        let expected = format!("{}{}", super::BYTE_ORDER_MARK, sample());
        let verdict = judge(&expected, &sample());

        assert!(matches!(verdict, Verdict::ByteOrderMark));
        assert!(!verdict.reproduced());

        let message = message(&verdict, 500);
        assert!(
            message.contains("begins with a byte-order mark"),
            "{message}"
        );
        assert!(message.contains("Nothing was checked"), "{message}");
        // Neither verdict, and it must read as neither.
        assert!(!message.contains("reproduced"), "{message}");
        assert!(!message.contains("diverged"), "{message}");
    }

    #[test]
    fn a_mark_is_caught_before_the_emptiness_check_that_cannot_see_it() {
        // U+FEFF is not whitespace in current Unicode, so `trim` leaves it: a file
        // holding nothing but a mark would otherwise fall through to the
        // comparison rather than being named. Order, not luck.
        assert!(!super::BYTE_ORDER_MARK.is_whitespace());
        assert!(matches!(
            judge(&super::BYTE_ORDER_MARK.to_string(), &sample()),
            Verdict::ByteOrderMark
        ));
    }

    #[test]
    fn a_translated_line_ending_is_the_same_state_and_says_so() {
        // The trap a redirect on Windows sets. `first_difference` walks lines and
        // cannot see it, so byte equality is asked separately - and the answer is
        // a sentence to the reader, not a red run.
        let expected = sample().replace('\n', "\r\n");
        let verdict = judge(&expected, &sample());

        assert!(matches!(verdict, Verdict::ReproducedInLinesOnly));
        assert!(verdict.reproduced());
    }

    #[test]
    fn a_missing_final_newline_is_the_same_state_too() {
        let expected = sample().trim_end().to_owned();
        let verdict = judge(&expected, &sample());

        assert!(matches!(verdict, Verdict::ReproducedInLinesOnly));
        assert!(verdict.reproduced());
    }

    #[test]
    fn the_two_verdict_words_share_no_substring() {
        // The wording rule this whole message shape is built around. "reproduced"
        // against "not reproduced" would make an agent grepping for the first one
        // match both, and a machine-readable answer that is wrong half the time is
        // worse than no answer.
        let passed = message(&Verdict::Reproduced, 500);
        let failed = message(
            &judge(&sample(), &sample_with(5, "  position (x:5,y:7)")),
            500,
        );

        assert!(passed.contains("reproduced"), "{passed}");
        assert!(!passed.contains("diverged"), "{passed}");
        assert!(failed.contains("diverged"), "{failed}");
        assert!(!failed.contains("reproduced"), "{failed}");
    }

    #[test]
    fn a_pass_names_the_tick_the_file_and_what_it_does_not_show() {
        let passed = message(&Verdict::Reproduced, 500);

        assert!(passed.contains("500 ticks"), "{passed}");
        assert!(passed.contains("expected.dump"), "{passed}");
        assert!(
            passed.contains("nothing about whether the state is right"),
            "the boundary belongs in the message a reader actually sees: {passed}"
        );
    }

    #[test]
    fn a_divergence_names_the_tick_the_file_the_place_and_which_side_is_which() {
        let failed = message(
            &judge(&sample(), &sample_with(5, "  position (x:5,y:7)")),
            500,
        );

        assert!(failed.contains("500 ticks"), "{failed}");
        assert!(failed.contains("expected.dump"), "{failed}");
        assert!(failed.contains("entity    1v1"), "{failed}");
        assert!(failed.contains("component position"), "{failed}");
        assert!(
            failed.contains("left is expected.dump, right is this run"),
            "a reader has to know which side to go and look at: {failed}"
        );
    }

    #[test]
    fn an_empty_expectation_says_the_file_is_empty_and_how_one_is_written() {
        let nothing = message(&Verdict::Nothing, 500);

        assert!(nothing.contains("expected.dump is empty"), "{nothing}");
        assert!(nothing.contains("--dump >"), "{nothing}");
        assert!(nothing.contains("Nothing was checked"), "{nothing}");
        // It is neither of the two verdicts, and must not read as either.
        assert!(!nothing.contains("reproduced"), "{nothing}");
        assert!(!nothing.contains("diverged"), "{nothing}");
    }

    #[test]
    fn a_byte_difference_that_is_not_a_divergence_says_which_file_to_look_at() {
        let expected = sample().replace('\n', "\r\n");
        let message = message(&judge(&expected, &sample()), 500);

        assert!(message.contains("reproduced"), "{message}");
        assert!(
            message.contains("line for line and differ in bytes"),
            "{message}"
        );
        assert!(message.contains("expected.dump"), "{message}");
    }
}
