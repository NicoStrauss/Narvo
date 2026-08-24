//! Clips against a real directory: the number comes off the disk.
//!
//! `clip.rs`'s own tests decide the rule on names and need no file system.
//! This file exists for the one claim they cannot make — that the frame count a
//! consumer is handed is a fact about **the files that are there**, and not a
//! constant anywhere in the path from a directory to an answer.
//!
//! The shape is deliberately the negative control M6b.5 was written for: the
//! same code, run twice, over directories that differ by one file. A count that
//! came from a literal would answer the same twice.
//!
//! **Nothing binary is committed** (ADR-0024). Every PNG here is encoded by the
//! test that reads it.

use narvo_assets::{pack, regions_from_directory};
use std::path::{Path, PathBuf};

/// A scratch directory under the cargo target directory, named for the case.
fn scratch(case: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(case);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("the scratch directory");
    directory
}

/// Straight-alpha RGBA8 as PNG bytes, the way an image editor would write them.
fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("a header can be written");
        writer
            .write_image_data(rgba)
            .expect("the data is the size the header says");
    }
    bytes
}

/// Writes `name.png` as one opaque texel, since only the name is under test.
fn put(directory: &Path, name: &str) {
    std::fs::write(
        directory.join(format!("{name}.png")),
        encode(1, 1, &[255, 0, 0, 255]),
    )
    .expect("the file is written");
}

/// The whole path a consumer walks: a directory, packed, asked for its clips.
fn frames_on_disk(directory: &Path, clip: &str) -> Vec<String> {
    let atlas = pack(regions_from_directory(directory).expect("the directory loads"))
        .expect("the regions pack");
    atlas
        .clips()
        .get(clip)
        .map(|clip| clip.frames().to_vec())
        .unwrap_or_default()
}

/// A directory of numbered files is a clip of that many frames.
#[test]
fn a_directory_of_numbered_files_is_a_clip() {
    let directory = scratch("clip-six");
    for index in 0..6 {
        put(&directory, &format!("hero_run_{index}"));
    }
    // A region belonging to no clip, to show it neither joins nor disturbs one.
    put(&directory, "backdrop");

    let frames = frames_on_disk(&directory, "hero_run");
    assert_eq!(frames.len(), 6);
    assert_eq!(
        frames,
        [
            "hero_run_0",
            "hero_run_1",
            "hero_run_2",
            "hero_run_3",
            "hero_run_4",
            "hero_run_5",
        ]
    );
}

/// **One frame more on the disk, one frame more in the answer, no code change.**
///
/// The oracle M6b.5 exists for. Its survey measured the failure in an external
/// consumer that had written the count by hand: seven files on the disk, seven
/// regions in the atlas, six frames drawn, and not one diagnostic. This is the
/// same directory in both halves and the same three lines reading it.
#[test]
fn one_more_file_is_one_more_frame_with_no_code_change() {
    let directory = scratch("clip-grows");
    for index in 0..6 {
        put(&directory, &format!("hero_run_{index}"));
    }
    let before = frames_on_disk(&directory, "hero_run");

    // The only thing that happens between the two reads.
    put(&directory, "hero_run_6");
    let after = frames_on_disk(&directory, "hero_run");

    assert_eq!(before.len(), 6);
    assert_eq!(after.len(), 7);
    assert_eq!(after.last().map(String::as_str), Some("hero_run_6"));
    // The first six are the ones they were; growing appended rather than
    // renumbered.
    assert_eq!(after[..6], before[..]);
    // And the same claim with no number in it at all, so this half cannot be
    // satisfied by a constant that happens to agree with the ones above.
    assert_eq!(after.len(), before.len() + 1);
}

/// The frame count equals the number of files, with no number written anywhere.
///
/// The other count tests name the size of the directory they built, which is
/// honest — the count is the output and the file list is the input — but it
/// leaves them checkable against a literal. This one is not: both sides are
/// read back off the file system at the same moment, at four different sizes,
/// and nothing in it would have to change if the sizes did.
#[test]
fn the_frame_count_equals_the_files_on_disk_at_every_size() {
    for size in [1_u32, 2, 5, 13] {
        let directory = scratch(&format!("clip-counted-{size}"));
        for index in 0..size {
            put(&directory, &format!("hero_run_{index}"));
        }
        put(&directory, "backdrop");

        // Counted off the directory rather than from `size`, so the two sides
        // of the comparison come from the file system and not from each other.
        let on_disk = std::fs::read_dir(&directory)
            .expect("the directory lists")
            .filter(|entry| {
                entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with("hero_run_"))
            })
            .count();

        assert_eq!(
            frames_on_disk(&directory, "hero_run").len(),
            on_disk,
            "the clip and the directory disagree about how many frames there are"
        );
    }
}

/// And one fewer, so the answer tracks the disk in both directions.
///
/// Without this half the test above is satisfied by anything that only ever
/// grows — the same vacuity M6b.3's identity tint and M6b.2's region negative
/// control are written against.
#[test]
fn one_fewer_file_is_one_fewer_frame() {
    let directory = scratch("clip-shrinks");
    for index in 0..6 {
        put(&directory, &format!("hero_run_{index}"));
    }
    let before = frames_on_disk(&directory, "hero_run");

    std::fs::remove_file(directory.join("hero_run_5.png")).expect("the frame is removed");
    let after = frames_on_disk(&directory, "hero_run");

    assert_eq!(before.len(), 6);
    assert_eq!(after.len(), 5);
    assert!(!after.iter().any(|frame| frame == "hero_run_5"));
}

/// Every frame a clip names is a region the atlas really carries.
///
/// The property that makes indexing by position safe, checked against the
/// packed table rather than against the list the clip came from. A gap is in
/// this directory on purpose: `hero_run_2` is missing, so the clip is three
/// frames and its third is `hero_run_3`.
#[test]
fn every_frame_a_clip_names_is_in_the_atlas() {
    let directory = scratch("clip-gap");
    for index in [0, 1, 3] {
        put(&directory, &format!("hero_run_{index}"));
    }

    let atlas = pack(regions_from_directory(&directory).expect("the directory loads"))
        .expect("the regions pack");
    let clips = atlas.clips();
    let clip = &clips["hero_run"];

    assert_eq!(clip.frames().len(), 3);
    assert_eq!(clip.frame(2), Some("hero_run_3"));
    for frame in clip.frames() {
        assert!(
            atlas.region(frame).is_some(),
            "the clip names {frame}, which the atlas does not carry"
        );
    }
    // The name a count-and-spell consumer would have asked for at position 2.
    assert!(atlas.region("hero_run_2").is_none());
}

/// Asking for the clips does not change the atlas.
///
/// The claim `clip.rs` makes in prose — that recognition is additive — against
/// the two things that would show it were not: the packed bytes and the anchor.
#[test]
fn asking_for_clips_moves_no_pixel_and_no_placement() {
    let directory = scratch("clip-additive");
    for index in 0..4 {
        put(&directory, &format!("hero_run_{index}"));
    }
    put(&directory, "backdrop");

    let regions = regions_from_directory(&directory).expect("the directory loads");
    let untouched = pack(regions.clone()).expect("the regions pack");
    let asked = pack(regions).expect("the regions pack");
    let clips = asked.clips();

    assert_eq!(clips.len(), 1);
    assert_eq!(asked.anchor(), untouched.anchor());
    assert_eq!(asked.rgba(), untouched.rgba());
    assert_eq!(asked.len(), untouched.len());
    // The region that is in no clip is still in the table under its own name.
    assert!(asked.region("backdrop").is_some());
    assert!(!clips.contains_key("backdrop"));
}

/// Zero-padded numbering off a real file system orders numerically.
///
/// The case that decides whether parsing was worth it: a directory listing and
/// a name sort both put `_10` next to `_1`.
#[test]
fn zero_padded_files_order_numerically() {
    let directory = scratch("clip-padded");
    for index in 0..12 {
        put(&directory, &format!("walk_{index:02}"));
    }

    let frames = frames_on_disk(&directory, "walk");
    assert_eq!(frames.len(), 12);
    assert_eq!(frames[9], "walk_09");
    assert_eq!(frames[10], "walk_10");
    assert_eq!(frames[11], "walk_11");
}

/// A directory whose names are the ones this repository actually uses has no
/// clips at all.
///
/// M6b.5's survey scanned the tree for a region name that would be read as a
/// frame and found none. This is that finding as a test rather than as a
/// sentence in a report: the names below are the ones committed scenes and
/// fixtures name today, and none of them becomes a clip.
#[test]
fn the_region_names_this_repository_uses_are_in_no_clip() {
    let directory = scratch("clip-none-of-ours");
    for name in [
        "hero", "coin", "backdrop", "button", "ground", "box_a", "box_b", "box_c", "box_d", "icon",
        "red", "green", "blue", "dot",
    ] {
        put(&directory, name);
    }

    let atlas = pack(regions_from_directory(&directory).expect("the directory loads"))
        .expect("the regions pack");
    assert_eq!(atlas.len(), 14);
    assert!(
        atlas.clips().is_empty(),
        "a name this repository uses was read as an animation frame: {:?}",
        atlas.clips().keys().collect::<Vec<_>>()
    );
}
