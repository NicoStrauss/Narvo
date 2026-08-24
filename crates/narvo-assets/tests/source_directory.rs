//! The file source against a real directory.
//!
//! Split from `source.rs`'s own tests for one mechanical reason and one design
//! one. The mechanical one: cargo sets `CARGO_TARGET_TMPDIR` for integration
//! tests and benches only, so a unit test inside `src/` cannot ask for a scratch
//! directory the way this file does — the same limitation `narvo-render2d`'s
//! `golden_image.rs` records for its own artifact directory. The design one: the
//! module's own tests are about arithmetic on bytes and need no file system at
//! all, and keeping the two apart says which is which.
//!
//! **Nothing binary is committed** (ADR-0024). Every PNG here is encoded by the
//! test that reads it.

use narvo_assets::{AssetError, regions_from_directory};
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

#[test]
fn a_directory_becomes_regions_named_by_their_stems() {
    let directory = scratch("stems");
    std::fs::write(directory.join("beta.png"), encode(1, 1, &[1, 2, 3, 255]))
        .expect("the file is written");
    std::fs::write(directory.join("alpha.png"), encode(2, 1, &[0; 8]))
        .expect("the file is written");
    // Not a PNG, and not an error either: a directory may hold other things and
    // this takes what it recognises.
    std::fs::write(directory.join("notes.txt"), b"ignored").expect("the file is written");

    let regions = regions_from_directory(&directory).expect("the directory loads");
    let names: Vec<&str> = regions
        .iter()
        .map(narvo_assets::SourceRegion::name)
        .collect();
    assert_eq!(names, vec!["alpha", "beta"]);
    assert_eq!(regions[1].rgba(), &[1, 2, 3, 255]);
}

/// An extension in another case is still a PNG.
#[test]
fn the_extension_is_matched_without_case() {
    let directory = scratch("extension-case");
    std::fs::write(directory.join("icon.PNG"), encode(1, 1, &[7, 7, 7, 255]))
        .expect("the file is written");

    let regions = regions_from_directory(&directory).expect("the directory loads");
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].name(), "icon");
}

/// An empty directory packs to nothing rather than failing.
///
/// The decision ADR-0024 records: "no assets" is a legitimate state, and a scene
/// that then names a region gets the unknown-region error, which is a more
/// useful place to be told than here.
#[test]
fn an_empty_directory_is_not_an_error() {
    let directory = scratch("empty");
    let regions = regions_from_directory(&directory).expect("an empty directory loads");
    assert!(regions.is_empty());
}

#[test]
fn a_missing_directory_says_which_one() {
    let directory = scratch("missing").join("not-there");
    let error = regions_from_directory(&directory).expect_err("a missing directory is refused");
    let message = error.to_string();
    assert!(message.contains("could not be read"), "{message}");
    assert!(message.contains("not-there"), "{message}");
}

/// A file that is not a PNG at all is refused, naming itself.
#[test]
fn a_corrupt_file_in_the_directory_names_itself() {
    let directory = scratch("corrupt");
    std::fs::write(directory.join("broken.png"), b"this is not a PNG")
        .expect("the file is written");

    let error = regions_from_directory(&directory).expect_err("corrupt bytes are refused");
    let message = error.to_string();
    assert!(message.contains("broken.png"), "{message}");
    assert!(
        message.contains("not a PNG this build can read"),
        "{message}"
    );
}

/// Two stems that differ only in case are refused, on both platforms.
///
/// On a case-insensitive file system the two files cannot both exist, so the
/// second write replaces the first and there is nothing to collide. That case
/// **says so** rather than passing silently: a green run here means one thing on
/// Linux and another on Windows, and a test that hid the difference would be
/// worse than one that reports it.
#[test]
fn two_stems_differing_only_in_case_are_refused() {
    let directory = scratch("case");
    std::fs::write(directory.join("hero.png"), encode(1, 1, &[1, 1, 1, 255]))
        .expect("the file is written");
    std::fs::write(directory.join("HERO.png"), encode(1, 1, &[2, 2, 2, 255]))
        .expect("the file is written");

    let listed = std::fs::read_dir(&directory)
        .expect("the directory reads")
        .filter_map(Result::ok)
        .count();

    if listed < 2 {
        println!(
            "this file system folded the two names into one file, so the collision \
             cannot be staged here; the rule is exercised on a case-sensitive file \
             system, and the message itself is checked by \
             the_duplicate_error_names_both_files"
        );
        return;
    }

    let error = regions_from_directory(&directory).expect_err("the collision is refused");
    let message = error.to_string();
    assert!(message.contains("would both be the region"), "{message}");
    assert!(message.contains("compared without case"), "{message}");
    assert!(message.contains("Rename one of them"), "{message}");
}

/// The duplicate message, checked without needing a file system that can stage
/// the collision.
#[test]
fn the_duplicate_error_names_both_files() {
    let error = AssetError::DuplicateStem {
        name: "hero".to_owned(),
        first: PathBuf::from("assets/hero.png"),
        second: PathBuf::from("assets/HERO.png"),
    };
    let message = error.to_string();
    assert!(message.contains("assets/hero.png"), "{message}");
    assert!(message.contains("assets/HERO.png"), "{message}");
    assert!(message.contains("\"hero\""), "{message}");
}

/// A 16-bit file in the directory is refused with the re-export instruction.
#[test]
fn a_sixteen_bit_file_in_the_directory_is_refused() {
    let directory = scratch("deep");
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Sixteen);
        let mut writer = encoder.write_header().expect("a header can be written");
        writer
            .write_image_data(&[0, 1, 0, 2, 0, 3, 255, 255])
            .expect("eight bytes are one 16-bit RGBA pixel");
    }
    std::fs::write(directory.join("deep.png"), bytes).expect("the file is written");

    let error = regions_from_directory(&directory).expect_err("16 bit is refused");
    let message = error.to_string();
    assert!(message.contains("16 bits per sample"), "{message}");
    assert!(
        message.contains("Re-export it at 8 bits per sample"),
        "{message}"
    );
}

/// **The contract, end to end from files**: what a directory packs to is what
/// the generated source would have packed to, and the guards do not know the
/// difference.
///
/// Three statements in one test because they are one claim: the same packer, the
/// same padding guard, the same anchor. If a file source needed any of them
/// changed, ADR-0020's "the source is exchangeable under the contract" would be
/// wrong, and this is where that would show.
#[test]
fn a_packed_directory_is_indistinguishable_from_a_packed_code_source() {
    let directory = scratch("contract");
    std::fs::write(
        directory.join("red.png"),
        encode(4, 4, &[255, 0, 0, 255].repeat(16)),
    )
    .expect("the file is written");
    std::fs::write(
        directory.join("blue.png"),
        encode(2, 6, &[0, 0, 255, 255].repeat(12)),
    )
    .expect("the file is written");

    let from_files =
        narvo_assets::pack(regions_from_directory(&directory).expect("the directory loads"))
            .expect("two small regions pack");

    let from_code = narvo_assets::pack(vec![
        narvo_assets::SourceRegion::solid("red", 4, 4, [255, 0, 0, 255]).expect("a solid region"),
        narvo_assets::SourceRegion::solid("blue", 2, 6, [0, 0, 255, 255]).expect("a solid region"),
    ])
    .expect("two small regions pack");

    assert_eq!(
        from_files.anchor_bytes(),
        from_code.anchor_bytes(),
        "the same pixels from a file and from code produce different atlases, so the \
         source is not exchangeable under the contract after all"
    );
    assert_eq!(from_files.width(), from_code.width());
    assert_eq!(from_files.height(), from_code.height());
}
