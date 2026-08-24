//! Which regions of an atlas are the frames of one animation.
//!
//! This is a **declaration and not a player**. It reads the names a packed
//! atlas already carries and answers a question a consumer could not otherwise
//! answer: *these regions are one clip, and they are its frames in this order.*
//! Nothing here advances a counter, holds a frame for a number of ticks or
//! decides what happens at the end — those are a game's, and `narvo-assets`
//! has no world to keep them in.
//!
//! # The gap this closes, measured rather than assumed (M6b.5)
//!
//! Animating needs no engine change at all: `Sprite` holds a region name, a
//! system writes components each tick, and a system that keeps a counter and
//! rewrites the name **is** an animation — deterministic and replayable,
//! because the counter is world state. M6b.5's survey wrote exactly that as an
//! external consumer and measured the bill: the system's body is eleven lines
//! and the state four, all of it over the public surface, with no engine
//! internal anywhere.
//!
//! What it also measured is the one fact the consumer had to **invent**: how
//! many frames the clip has. That number lives on the disk and in the packed
//! table, and nothing handed it over — so the consumer wrote `6`. With a
//! seventh frame added beside the other six, the atlas carried seven regions,
//! the game showed six, and the load emitted no error, no warning and no
//! failure. The information was present and unreachable.
//!
//! # Frames come out as **names**, never as a count to spell back
//!
//! The obvious shape is to answer with a number and let the consumer write
//! `format!("{stem}_{index}")`. It is rejected, and for a reason that is not
//! taste:
//!
//! - **Padding is the author's.** A sprite sheet exported as `run_00.png` …
//!   `run_11.png` is ordinary, and no speller can know whether to emit `run_0`
//!   or `run_00`. A function that guessed would be wrong for half of the
//!   projects that exist.
//! - **A gap cannot then produce a missing region.** Frames numbered 0, 1 and 3
//!   are three frames; a consumer indexing *by position* into the list names
//!   only regions that are there. One handed the count `3` and left to spell
//!   would ask for `run_2`, which no file carries.
//! - **It removes a double spelling.** The survey's consumer wrote the clip's
//!   name twice — once for the entity's starting region, once inside the system
//!   — and the two had to agree by hand.
//!
//! So there is no speller in this module, deliberately. The names are the truth
//! and they are handed over whole.
//!
//! # It re-interprets nothing
//!
//! Recognition is **additive**. No region is renamed, none is removed, the
//! packed table is untouched, and a region belonging to no clip is a region
//! exactly as it was. A name that happens to end in `_1` gains a second
//! description and loses nothing: `Atlas::region` still answers for it under the
//! name it has.
//!
//! That mattered enough to check before this was written. M6b.5 scanned every
//! `*.rs` and `*.ron` in the workspace for a string literal shaped like a
//! numbered region name and found one hit, `"zoom_2"`, which is a camera-margin
//! test case rather than a region; no tracked image's file stem matches at all.
//! Nothing in the tree changes meaning.

use std::collections::{BTreeMap, BTreeSet};

/// The separator between a clip's name and a frame's number.
const SEPARATOR: char = '_';

/// One animation's frames, as region names in playing order.
///
/// Never empty: a clip exists because at least one name produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    frames: Vec<String>,
}

impl Clip {
    /// Its frames, as region names, in playing order.
    ///
    /// The count is `frames().len()`, and it is the number that had to be
    /// invented before this existed.
    #[must_use]
    pub fn frames(&self) -> &[String] {
        &self.frames
    }

    /// The name of the frame at `index`, counting from the first.
    ///
    /// **By position, not by the number in the name.** With frames numbered 0,
    /// 1 and 3, position 2 is `"…_3"`. That is what keeps every answer a region
    /// the atlas carries: the positions are dense even when the numbering is
    /// not.
    #[must_use]
    pub fn frame(&self, index: usize) -> Option<&str> {
        self.frames.get(index).map(String::as_str)
    }
}

/// Groups `names` into clips.
///
/// A name is a frame when everything after its **last** [`SEPARATOR`] is a
/// non-empty run of ASCII digits that fits in a `u32`; what precedes that
/// separator is the clip's name. Anything else is in no clip.
///
/// # The digit test is explicit, and that is load-bearing
///
/// `str::parse::<u32>` accepts a leading `+`, so `"run_+1"` would parse and
/// become frame 1 of `run` if parsing were the only check. It is not a frame:
/// no file stem produces that shape by numbering, and a name that reached here
/// spelled that way is not one an author meant as a frame. The explicit
/// `is_ascii_digit` pass is what refuses it, and it refuses non-ASCII digits on
/// the same ground.
///
/// # Order, and why it is a set rather than a sort
///
/// Frames are collected into a `BTreeSet<(u32, String)>` per clip, so the order
/// is *(number, name)* — total, reproducible, and the collection's rather than a
/// comparator that could be edited into a partial one. Numeric order is the
/// point: it puts `run_9` before `run_10`, where sorting the names would not.
///
/// Two names that differ only in padding — `run_0` and `run_00` — are two
/// frames at one number, ordered by name. That is total and reproducible rather
/// than an error, because both name a region the atlas really carries and
/// refusing them would mean this function could fail, which nothing about
/// reading a table should.
///
/// The input's order does not reach the output, exactly as it does not reach a
/// packing ([`pack`](crate::pack)): a set is built and then read out.
///
/// # A gap shortens the clip
///
/// Frames 0, 1 and 3 are a clip of three. There is no way to tell an author who
/// deleted a frame from one who numbered in tens to leave room, so neither is
/// called a mistake — and because the frames come out as names, the shorter
/// clip still names only regions that exist. M4.2's two warning classes are not
/// joined by a third for content taste.
///
/// # Examples
///
/// ```
/// let clips = narvo_assets::clips(["hero_run_0", "hero_run_1", "coin"]);
///
/// assert_eq!(clips["hero_run"].frames(), ["hero_run_0", "hero_run_1"]);
/// assert_eq!(clips["hero_run"].frame(1), Some("hero_run_1"));
///
/// // A region in no clip is still a region; it simply has no entry here.
/// assert!(!clips.contains_key("coin"));
/// ```
#[must_use]
pub fn clips<'a>(names: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, Clip> {
    let mut collected: BTreeMap<String, BTreeSet<(u32, String)>> = BTreeMap::new();

    for name in names {
        let Some((stem, tail)) = name.rsplit_once(SEPARATOR) else {
            continue;
        };
        if tail.is_empty() || !tail.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        // Refused rather than clamped: a number too large for a `u32` is not a
        // frame this can order against the others, and inventing a position for
        // it would put a name in a clip at a place nothing chose.
        let Ok(number) = tail.parse::<u32>() else {
            continue;
        };

        collected
            .entry(stem.to_owned())
            .or_default()
            .insert((number, name.to_owned()));
    }

    collected
        .into_iter()
        .map(|(stem, frames)| {
            (
                stem,
                Clip {
                    frames: frames.into_iter().map(|(_number, name)| name).collect(),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::clips;

    /// What a clip's frames are called, in order, as plain strings.
    fn frames_of(names: &[&str], clip: &str) -> Vec<String> {
        clips(names.iter().copied())
            .get(clip)
            .map(|clip| clip.frames().to_vec())
            .unwrap_or_default()
    }

    #[test]
    fn six_files_are_one_clip_of_six_in_order() {
        let names = [
            "hero_run_0",
            "hero_run_1",
            "hero_run_2",
            "hero_run_3",
            "hero_run_4",
            "hero_run_5",
        ];
        let clips = clips(names);

        assert_eq!(clips.len(), 1);
        assert_eq!(clips["hero_run"].frames().len(), 6);
        assert_eq!(clips["hero_run"].frames(), names);
        assert_eq!(clips["hero_run"].frame(0), Some("hero_run_0"));
        assert_eq!(clips["hero_run"].frame(5), Some("hero_run_5"));
        assert_eq!(clips["hero_run"].frame(6), None);
    }

    /// The number comes from the names it was given and from nothing else.
    ///
    /// **The negative control**, and the property this module exists for. A
    /// count that came from a constant would answer six to both of these; the
    /// test is that the *same code* answers differently because the disk did.
    /// M6b.5's survey measured the failure this prevents: a seventh frame beside
    /// six, an atlas carrying seven regions, a game showing six, and no
    /// diagnostic anywhere.
    #[test]
    fn the_count_comes_from_the_names_and_not_from_a_constant() {
        let six = [
            "hero_run_0",
            "hero_run_1",
            "hero_run_2",
            "hero_run_3",
            "hero_run_4",
            "hero_run_5",
        ];
        let mut seven = six.to_vec();
        seven.push("hero_run_6");

        assert_eq!(clips(six)["hero_run"].frames().len(), 6);
        assert_eq!(clips(seven)["hero_run"].frames().len(), 7);

        // And one fewer, so the number is not merely monotone in something.
        assert_eq!(
            clips(["hero_run_0", "hero_run_1"])["hero_run"]
                .frames()
                .len(),
            2
        );
    }

    /// Numbers order numerically, which is the whole reason they are parsed.
    ///
    /// Sorting the names would put `_10` between `_1` and `_2`, and a run cycle
    /// built from that order plays the frames out of sequence — which looks like
    /// an artist's mistake rather than a library's.
    #[test]
    fn nine_comes_before_ten_rather_than_after_one() {
        let names = ["walk_10", "walk_1", "walk_9", "walk_0", "walk_2", "walk_11"];
        assert_eq!(
            frames_of(&names, "walk"),
            ["walk_0", "walk_1", "walk_2", "walk_9", "walk_10", "walk_11"]
        );
    }

    /// Zero padding is the author's and is carried verbatim, still in numeric
    /// order.
    #[test]
    fn zero_padding_is_kept_and_still_orders_numerically() {
        let names = ["run_09", "run_00", "run_10", "run_01"];
        assert_eq!(
            frames_of(&names, "run"),
            ["run_00", "run_01", "run_09", "run_10"]
        );
    }

    /// A name with no numbered tail is in no clip — and is untouched.
    #[test]
    fn a_plain_name_is_in_no_clip() {
        let clips = clips(["hero", "coin", "backdrop", "box_a", "hero_run_0"]);

        assert_eq!(clips.keys().collect::<Vec<_>>(), ["hero_run"]);
        assert!(!clips.contains_key("hero"));
        assert!(!clips.contains_key("box"));
    }

    /// Every shape that looks like a number and is not.
    #[test]
    fn only_ascii_digits_after_the_last_separator_make_a_frame() {
        for name in [
            "run_",    // nothing after the separator
            "run_+1",  // `parse::<u32>` would accept this; the digit test does not
            "run_-1",  // likewise, and it would not fit a `u32` either
            "run_1a",  // digits and then not
            "run_a1",  // not and then digits
            "run_1.0", // a decimal point is not a digit
            "run_ ",   // a space
            "run_١٢٣", // Arabic-Indic digits are digits and are not ASCII ones
            "run1",    // no separator at all
            "runs",    // nothing numeric anywhere
        ] {
            assert!(
                clips([name]).is_empty(),
                "{name:?} was taken for a frame and should not have been"
            );
        }
    }

    /// A number too large for a `u32` leaves the name a plain region.
    ///
    /// `u32::MAX` is 4 294 967 295, so the first of these is the largest number
    /// that is a frame and the second is one past it.
    #[test]
    fn a_number_that_does_not_fit_is_not_a_frame() {
        assert_eq!(frames_of(&["tick_4294967295"], "tick"), ["tick_4294967295"]);
        assert!(clips(["tick_4294967296"]).is_empty());
        assert!(clips(["tick_99999999999999999999"]).is_empty());
    }

    /// A gap shortens the clip, and every frame it answers with is a name it
    /// was given.
    #[test]
    fn a_gap_shortens_the_clip_rather_than_inventing_a_name() {
        let names = ["run_0", "run_1", "run_3"];
        let frames = frames_of(&names, "run");

        assert_eq!(frames, ["run_0", "run_1", "run_3"]);
        assert_eq!(frames.len(), 3);
        // Position 2 is the third frame, which is `run_3`. Spelling the
        // position back out would have asked for `run_2`, which is not here.
        assert_eq!(clips(names)["run"].frame(2), Some("run_3"));
        for frame in &frames {
            assert!(names.contains(&frame.as_str()), "{frame} was invented");
        }
    }

    /// Numbering that starts anywhere is still a clip.
    #[test]
    fn a_clip_need_not_start_at_zero_or_be_more_than_one_frame() {
        assert_eq!(frames_of(&["run_7", "run_8"], "run"), ["run_7", "run_8"]);
        assert_eq!(frames_of(&["blink_4"], "blink"), ["blink_4"]);
    }

    /// Two paddings at one number are two frames, ordered by name.
    ///
    /// Total and reproducible rather than an error: both name a region the
    /// atlas carries, and a table read should not be able to fail.
    #[test]
    fn mixed_padding_at_one_number_is_total_and_reproducible() {
        let names = ["run_0", "run_00", "run_1"];
        assert_eq!(frames_of(&names, "run"), ["run_0", "run_00", "run_1"]);

        // The same answer whichever order it arrives in.
        assert_eq!(
            frames_of(&["run_1", "run_00", "run_0"], "run"),
            frames_of(&names, "run")
        );
    }

    /// The order the names arrive in does not reach the answer.
    ///
    /// The property [`pack`](crate::pack) has for a layout, here for a table
    /// read: `permuting_the_input_does_not_move_anything` is the packer's, and
    /// this is the same guarantee one level up.
    #[test]
    fn permuting_the_input_does_not_move_a_clip() {
        let forward = ["a_0", "a_1", "b_2", "b_10", "plain", "a_2"];
        let mut reversed = forward;
        reversed.reverse();
        let rotated = ["b_10", "plain", "a_2", "a_0", "a_1", "b_2"];

        assert_eq!(clips(forward), clips(reversed));
        assert_eq!(clips(forward), clips(rotated));
        assert_eq!(clips(forward)["a"].frames(), ["a_0", "a_1", "a_2"]);
        assert_eq!(clips(forward)["b"].frames(), ["b_2", "b_10"]);
    }

    /// A repeated name is one frame, not two.
    #[test]
    fn a_repeated_name_is_one_frame() {
        assert_eq!(
            frames_of(&["run_0", "run_0", "run_1"], "run"),
            ["run_0", "run_1"]
        );
    }

    /// Nothing in, nothing out — and a set with no numbered name is the same.
    #[test]
    fn a_set_with_no_numbered_name_has_no_clips() {
        assert!(clips([]).is_empty());
        assert!(clips(["hero", "coin"]).is_empty());
    }

    /// Only the **last** separator splits, so a clip name may contain one.
    #[test]
    fn the_last_separator_is_the_one_that_splits() {
        let names = ["hero_run_left_0", "hero_run_left_1"];
        assert_eq!(frames_of(&names, "hero_run_left"), names);
        assert!(!clips(names).contains_key("hero_run"));
    }

    /// A leading separator gives a clip with an empty name rather than a
    /// special case.
    ///
    /// `_0.png` is a legal file and therefore a legal region ([`Sprite`]'s own
    /// round trip covers the empty name), so this is what the rule says rather
    /// than something written for it.
    ///
    /// [`Sprite`]: https://docs.rs/narvo-ecs
    #[test]
    fn an_empty_stem_is_a_clip_with_an_empty_name() {
        let clips = clips(["_0", "_1"]);
        assert_eq!(clips[""].frames(), ["_0", "_1"]);
    }
}
