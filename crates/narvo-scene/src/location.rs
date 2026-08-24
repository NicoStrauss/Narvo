//! Where in a scene file something is, recovered from a borrowed fragment.
//!
//! # The mechanism, and the one assumption under it
//!
//! The scene grammar keeps every component body, every entity name and every
//! reference as a [`RawValue`](ron::value::RawValue) borrowed out of the input
//! (ADR-0018, Decision 4). "Borrowed out of the input" is a statement the
//! *borrow checker* enforces: the value is a `&'a RawValue` where `'a` is the
//! lifetime of the `&str` that was parsed, so the only buffer it can point into
//! is that string. That is what makes [`locate`] possible — subtract the two
//! addresses and the difference is a byte offset into the file.
//!
//! It is still arithmetic on a raw address, so it is **not trusted, it is
//! checked**: [`locate`] compares the slice at the offset it computed against
//! the fragment it was given, and returns `None` if they differ. The failure
//! mode of a broken assumption is therefore *no position*, never a wrong one —
//! an error message that says less, rather than one that sends a reader to the
//! wrong line.
//!
//! `the_offset_mechanism_locates_a_known_fragment` in `tests/validation.rs` is
//! the standing guard: it pins a fragment's line and column against a
//! hand-counted position, so if `ron` ever stopped borrowing, the suite goes red
//! instead of the positions going quietly missing. Both halves are demonstrated
//! red in the M4.2 report — a corrupted line count, which the slice check cannot
//! see and the guard's hand-counted position does; and a corrupted offset, which
//! the slice check catches, turning the position into `None` rather than a lie.

use std::fmt;

/// A position in a scene file. Both counts start at one.
///
/// Line and column are counted exactly as `ron` counts them — lines by `\n`,
/// columns in **characters** rather than bytes — so a position this crate
/// computes and a position `ron` reports mean the same thing in the same file.
/// A tool that renders both (the validation CLI does) would otherwise be
/// reporting in two different coordinate systems without saying so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Location {
    line: usize,
    column: usize,
}

impl Location {
    /// A position at `line` and `column`, both counting from one.
    #[must_use]
    pub const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }

    /// The line, counting from one.
    #[must_use]
    pub const fn line(self) -> usize {
        self.line
    }

    /// The column, counting from one, in characters.
    #[must_use]
    pub const fn column(self) -> usize {
        self.column
    }
}

impl fmt::Display for Location {
    /// `line:column` — the form an editor and a compiler both understand.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// Finds where `fragment` sits inside `text`, given that it is a slice of it.
///
/// Leading whitespace is skipped before the position is taken, because a
/// `RawValue`'s text may begin with the space between a `:` and the value, and
/// what a reader wants pointed at is the value.
///
/// Returns `None` when `fragment` is not a slice of `text` — which is the honest
/// answer for a caller holding an owned copy, and the safety net if the borrow
/// assumption above ever stops holding.
#[must_use]
pub(crate) fn locate(text: &str, fragment: &str) -> Option<Location> {
    let fragment = fragment.trim_start();

    let offset = (fragment.as_ptr() as usize).checked_sub(text.as_ptr() as usize)?;

    // The check that turns address arithmetic into a fact. Without it a
    // fragment from a different buffer that happened to sit at a higher address
    // would produce a plausible, wrong line number.
    if text.get(offset..offset + fragment.len()) != Some(fragment) {
        return None;
    }

    let before = text.get(..offset)?;
    Some(Location {
        line: before.bytes().filter(|byte| *byte == b'\n').count() + 1,
        column: before
            .rsplit('\n')
            .next()
            .map_or(0, |last| last.chars().count())
            + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::{Location, locate};

    #[test]
    fn a_fragment_of_the_text_is_located_by_line_and_column() {
        let text = "one\ntwo three\nfour";
        let three = &text[8..13];
        assert_eq!(three, "three");

        assert_eq!(locate(text, three), Some(Location::new(2, 5)));
        assert_eq!(locate(text, &text[0..3]), Some(Location::new(1, 1)));
        assert_eq!(locate(text, &text[14..]), Some(Location::new(3, 1)));
    }

    #[test]
    fn leading_whitespace_is_skipped_so_the_position_is_the_value() {
        // What a `RawValue` hands back: the space after the colon comes with it.
        let text = "name:  \"eye\"";
        assert_eq!(locate(text, &text[5..]), Some(Location::new(1, 8)));
    }

    #[test]
    fn a_fragment_that_is_not_a_slice_of_the_text_has_no_location() {
        // The safety net: an owned copy is not in the buffer, and the slice
        // comparison is what notices. Both orders are tried, because one of them
        // makes the subtraction underflow and the other makes it succeed with a
        // meaningless value.
        let text = "one\ntwo three\nfour".to_owned();
        let copy = "three".to_owned();

        assert_eq!(locate(&text, &copy), None);
        assert_eq!(locate(&copy, &text), None);
    }

    #[test]
    fn columns_are_counted_in_characters_not_bytes() {
        // `ron` counts columns in characters, and a position this crate computes
        // has to mean the same thing as one `ron` reports, or a tool rendering
        // both would be mixing two coordinate systems.
        let text = "äöü x";
        assert_eq!(locate(text, &text[6..]), Some(Location::new(1, 5)));
    }

    #[test]
    fn an_empty_fragment_at_the_end_still_has_a_position() {
        let text = "abc";
        assert_eq!(locate(text, &text[3..]), Some(Location::new(1, 4)));
    }
}
