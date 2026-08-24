//! Rendering a single frame to a PNG and exiting.
//!
//! A thin wrapper around the offscreen path that the renderer's tests already
//! cover. Nothing here decides what a frame looks like.

use std::path::{Path, PathBuf};

use narvo_render2d::{OffscreenTarget, Pixels, RenderError};

/// Width of a captured frame.
pub const WIDTH: u32 = 256;
/// Height of a captured frame.
pub const HEIGHT: u32 = 256;

/// Where a screenshot taken out of a running window lands.
///
/// Beside `--screenshot`'s own default (`cli::DEFAULT_SCREENSHOT_PATH`) and
/// under `target/` for the same reason: `.gitignore` has `/target/`, so a
/// picture taken to look at something never turns up in `git status`. A
/// different directory would need a second ignore rule and would be one more
/// thing to get wrong.
pub const WINDOW_DIR: &str = "target/screenshots";

/// The file a window screenshot taken at `stamp` is written to.
///
/// `stamp` is milliseconds since the Unix epoch, which is a wall clock and
/// therefore a thing the simulation may not touch — this is the runner, not a
/// tick, and nothing derived from it reaches a world or a state hash. It is
/// passed in rather than read here so the naming is a pure function and can be
/// tested without waiting for a clock to move.
///
/// **Two captures in one millisecond overwrite.** They cannot be produced by
/// hand: a request is served at most once per frame (`SceneHost::present`), and
/// a frame is milliseconds. A caller driving this in a loop could, and none
/// does.
#[must_use]
pub fn window_capture_path(dir: &Path, stamp: u128) -> PathBuf {
    dir.join(format!("window-{stamp}.png"))
}

/// Writes `pixels` as a PNG under `dir` and returns the file it wrote.
///
/// **The return value is what a caller reports.** Recomputing the path for the
/// message would be two derivations of one fact, and the failure that leaves —
/// a line naming a file that was written somewhere else — is the class M6.4a
/// found in an answer computed beside its own effect rather than from it. There
/// is one `PathBuf` here, it is what the encoder was handed, and it is what
/// comes back.
///
/// # Errors
///
/// [`RenderError::PngWrite`] if `dir` cannot be created or the file cannot be
/// written — a read-only directory, a full disk, a path that is not a
/// directory. The error names the file rather than the directory, because that
/// is what a caller was trying to produce.
pub fn write_window_capture(
    dir: &Path,
    stamp: u128,
    pixels: &Pixels,
) -> Result<PathBuf, RenderError> {
    let path = window_capture_path(dir, stamp);

    // Same reason `capture` below creates its parent: the directory is under
    // `target/`, which a fresh checkout does not have.
    std::fs::create_dir_all(dir).map_err(|error| RenderError::PngWrite {
        path: path.clone(),
        source: Box::new(error),
    })?;

    pixels.save_png(&path)?;

    Ok(path)
}

/// Renders one frame offscreen and writes it to `path` as a PNG.
///
/// # Errors
///
/// Whatever the renderer reports: no adapter, no device, a failed read-back or a
/// PNG that could not be written. The caller prints it; the messages are written
/// to be actionable on their own.
pub fn capture(path: &Path) -> Result<(), RenderError> {
    // The default path lives under `target/`, which may not exist yet on a
    // fresh checkout, and a caller-given path may point somewhere equally new.
    // Creating the directory beats telling the user to.
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| RenderError::PngWrite {
            path: path.to_path_buf(),
            source: Box::new(error),
        })?;
    }

    let target = OffscreenTarget::new(WIDTH, HEIGHT)?;
    println!("adapter in use: {}", target.adapter_summary());

    let frame = target.render_textured_quad(&crate::content::demo_texture())?;
    frame.save_png(path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{WINDOW_DIR, window_capture_path, write_window_capture};
    use narvo_render2d::Pixels;
    use std::path::{Path, PathBuf};

    /// A temp directory of this test process's own. nextest runs every test in
    /// its own process, so the pid makes it unique; the name makes it findable.
    fn scratch(what: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "narvo-app-screenshot-{what}-{}",
            std::process::id()
        ))
    }

    /// Four texels, none of them grey, so a channel that moved is visible.
    fn fixture() -> Pixels {
        Pixels::from_rgba8(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, //
                0, 0, 255, 255, 255, 255, 0, 255,
            ],
        )
        .expect("a 2x2 RGBA image is valid")
    }

    #[test]
    fn the_reported_path_is_the_file_that_was_written() {
        // The idempotence question of M6.4a asked at a screenshot: is the answer
        // computed from the effect, or beside it? Here the only way to find out
        // is to believe the returned path and look there — which is the point.
        // Nothing in this test knows the file name in advance.
        let directory = scratch("written");
        let _ = std::fs::remove_dir_all(&directory);

        let reported = write_window_capture(&directory, 1_234, &fixture())
            .expect("writing into a fresh temp directory must succeed");

        assert!(
            reported.is_file(),
            "the reported path must be a file that exists: {}",
            reported.display()
        );

        let bytes = std::fs::read(&reported).expect("the reported file must be readable");
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .expect("the PNG that was just written must decode");
        let size = reader
            .output_buffer_size()
            .expect("a 2x2 image fits in memory");
        let mut decoded = vec![0_u8; size];
        let info = reader
            .next_frame(&mut decoded)
            .expect("the single frame must decode");

        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(info.color_type, png::ColorType::Rgba);
        assert_eq!(
            &decoded[..info.buffer_size()],
            fixture().rgba(),
            "the bytes in the file must be the bytes that were handed over"
        );

        // And it is the only file there, so "reported one, wrote another" cannot
        // hide behind a directory that happens to contain both.
        let written: Vec<PathBuf> = std::fs::read_dir(&directory)
            .expect("the directory must be readable")
            .map(|entry| entry.expect("the entry must be readable").path())
            .collect();
        assert_eq!(written, vec![reported], "one capture, one file");

        std::fs::remove_dir_all(&directory).expect("the temp directory must be removable");
    }

    #[test]
    fn the_name_is_a_function_of_the_stamp_alone() {
        let directory = Path::new(WINDOW_DIR);

        assert_eq!(
            window_capture_path(directory, 7),
            window_capture_path(directory, 7),
            "the same stamp must name the same file, or the message and the \
             write could part"
        );
        assert_ne!(
            window_capture_path(directory, 7),
            window_capture_path(directory, 8),
            "two captures must not collide"
        );
        assert_eq!(
            window_capture_path(directory, 7),
            Path::new(WINDOW_DIR).join("window-7.png")
        );
    }

    #[test]
    fn a_place_that_cannot_hold_a_directory_is_reported_with_the_file_it_was_for() {
        // A file where the directory should be. `create_dir_all` refuses, which
        // is the reachable half of "the location is not writable" — the other
        // half, a full disk, is the same arm of the same match and cannot be
        // produced on demand.
        let occupied = scratch("occupied");
        let _ = std::fs::remove_dir_all(&occupied);
        let _ = std::fs::remove_file(&occupied);
        std::fs::write(&occupied, b"not a directory").expect("the temp file must be writable");

        let error = write_window_capture(&occupied, 99, &fixture())
            .expect_err("a file cannot be used as a directory");

        let message = error.to_string();
        assert!(
            message.contains("window-99.png"),
            "the message must name the file the caller wanted, not the \
             directory that refused: {message}"
        );
        assert!(
            message.starts_with("writing the PNG to "),
            "and it must read as one sentence: {message}"
        );

        std::fs::remove_file(&occupied).expect("the temp file must be removable");
    }

    #[test]
    fn a_file_that_cannot_be_created_is_reported_the_same_way() {
        // The **other** arm: the directory is fine and the file is not. Two
        // different failures produce this error - `create_dir_all` above and the
        // encoder here - and a caller reads one sentence, so both have to name
        // the file. The first version of this pair named the directory in one
        // arm and the file in the other, which is how a message comes to be
        // about something the user was not doing.
        //
        // A directory standing where the PNG goes, because that is the one way
        // to make the write fail that works the same on both platforms this
        // project verifies on. A full disk is the same arm and cannot be
        // produced on demand.
        let directory = scratch("blocked");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("the temp directory must be creatable");
        std::fs::create_dir_all(window_capture_path(&directory, 55))
            .expect("a directory may stand where the file would go");

        let error = write_window_capture(&directory, 55, &fixture())
            .expect_err("a directory cannot be overwritten with a PNG");

        let message = error.to_string();
        assert!(
            message.contains("window-55.png"),
            "the message must name the file: {message}"
        );
        assert!(
            message.starts_with("writing the PNG to "),
            "and read as one sentence: {message}"
        );

        std::fs::remove_dir_all(&directory).expect("the temp directory must be removable");
    }
}
