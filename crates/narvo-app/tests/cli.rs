//! Drives the binary through its actual command line.
//!
//! These spawn `narvo` rather than calling into it, so what is tested is the
//! path a user takes: argument parsing, dispatch, exit code and the file that
//! comes out the other end.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A directory for one test's artifacts, under the Cargo target directory —
/// which the repository already ignores.
///
/// One per test rather than one shared: nextest runs tests in parallel
/// processes, and a test that tidies up after itself would otherwise delete a
/// file another one is still using.
fn artifact_dir(test: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(test);
    std::fs::create_dir_all(&directory).expect("the output directory must be creatable");
    directory
}

/// Runs the binary and returns success, stdout and stderr.
fn narvo(arguments: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .args(arguments)
        .output()
        .expect("the narvo binary must be runnable");

    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Helpers only the screenshot test needs. Gated so a build without `render`
/// does not carry them as dead code - the headless configuration has to pass
/// `-D warnings` too.
#[cfg(feature = "render")]
mod render_support {
    use std::path::{Path, PathBuf};

    /// Printed when a test cannot run for lack of an adapter.
    pub const SKIP_MARKER: &str = "NARVO-GPU-TEST-SKIPPED";

    /// Set this, to anything, and a missing adapter fails instead of skipping.
    pub const REQUIRE_GPU_VAR: &str = "NARVO_REQUIRE_GPU";

    /// Where test artifacts go: under the Cargo target directory, which the
    /// repository already ignores.
    pub fn output_dir() -> PathBuf {
        Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli")
    }

    /// Reads a PNG's dimensions straight out of its IHDR chunk.
    ///
    /// Structural rather than a full decode, and deliberately dependency-free:
    /// the headless build must not grow a decoder just so a test can look at a
    /// file, and its `cargo tree` should stay as short as it is. That the
    /// encoder produces genuinely decodable PNGs is already covered by the
    /// renderer's own round-trip and golden-image tests.
    pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
        const MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

        // Eight bytes of signature, then the IHDR chunk: four bytes of length,
        // four of type, then width and height as big-endian u32s.
        if bytes.len() < 24 || !bytes.starts_with(MAGIC) || &bytes[12..16] != b"IHDR" {
            return None;
        }

        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        Some((width, height))
    }
}

#[test]
fn help_is_printed_and_exits_successfully() {
    // Runs in every configuration, including a build without `render`: naming a
    // command never depends on being able to carry it out.
    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .arg("--help")
        .output()
        .expect("the narvo binary must be runnable");

    assert!(output.status.success(), "--help must exit successfully");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--screenshot") && stdout.contains("--headless"),
        "the usage text must list the flags, got: {stdout}"
    );
    assert!(
        stdout.contains("target/screenshots"),
        "the usage text must say where a screenshot lands without a path, got: {stdout}"
    );
}

#[test]
fn an_unknown_flag_fails_and_names_itself() {
    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .arg("--definitely-not-a-flag")
        .output()
        .expect("the narvo binary must be runnable");

    assert!(!output.status.success(), "an unknown flag must fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--definitely-not-a-flag"),
        "the error must name the argument, got: {stderr}"
    );
}

#[test]
fn the_headless_flag_runs_without_a_gpu() {
    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .arg("--headless")
        .output()
        .expect("the narvo binary must be runnable");

    assert!(
        output.status.success(),
        "--headless must not need a GPU: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ticks"),
        "the headless run must report what it did, got: {stdout}"
    );
}

#[test]
fn a_recorded_run_replays_to_the_same_state_through_the_command_line() {
    // The unit tests reach the runner directly; this is the only thing that
    // covers the file actually being written, read back and handed to a second
    // process — which is the shape a repro is shared in.
    use std::ffi::OsStr;

    let path = artifact_dir("cli-recording").join("run.rec");
    std::fs::remove_file(&path).ok();

    let (recorded_ok, recorded_hash, recorded_stderr) = narvo(&[
        OsStr::new("--mode"),
        OsStr::new("input"),
        OsStr::new("--seed"),
        OsStr::new("1"),
        OsStr::new("--ticks"),
        OsStr::new("500"),
        OsStr::new("--record"),
        path.as_os_str(),
        OsStr::new("--hash"),
    ]);
    assert!(recorded_ok, "recording a run failed: {recorded_stderr}");

    let text = std::fs::read_to_string(&path).expect("the recording must have been written");
    assert!(
        text.starts_with("narvo-recording "),
        "unexpected file: {text}"
    );
    assert!(text.ends_with("end\n"), "the file must be complete");
    assert!(!text.contains('\r'), "a recording is LF on every platform");

    let (replayed_ok, replayed_hash, replayed_stderr) = narvo(&[
        OsStr::new("--replay"),
        path.as_os_str(),
        OsStr::new("--hash"),
    ]);
    assert!(replayed_ok, "replaying failed: {replayed_stderr}");

    assert_eq!(
        replayed_hash.trim(),
        recorded_hash.trim(),
        "the replay produced a different state than the run it came from"
    );
    assert!(
        !recorded_hash.trim().is_empty(),
        "the run reported no hash at all"
    );

    std::fs::remove_dir_all(artifact_dir("cli-recording")).ok();
}

#[test]
fn replaying_a_truncated_recording_fails_instead_of_running_a_shorter_run() {
    // The M1 scar applied to this feature: a tool that reports success on a
    // damaged input is worse than no tool. Demonstrated in both directions -
    // the test above shows the green case, this one the red.
    use std::ffi::OsStr;

    let directory = artifact_dir("cli-recording-truncated");
    let good = directory.join("full.rec");
    let cut = directory.join("cut.rec");

    let (ok, _stdout, stderr) = narvo(&[
        OsStr::new("--mode"),
        OsStr::new("input"),
        OsStr::new("--ticks"),
        OsStr::new("200"),
        OsStr::new("--record"),
        good.as_os_str(),
    ]);
    assert!(ok, "recording a run failed: {stderr}");

    let text = std::fs::read_to_string(&good).expect("written");
    let truncated = &text[..text.len() - "end\n".len()];
    std::fs::write(&cut, truncated).expect("the truncated copy must be writable");

    let (ok, _stdout, stderr) = narvo(&[
        OsStr::new("--replay"),
        cut.as_os_str(),
        OsStr::new("--hash"),
    ]);

    assert!(!ok, "a truncated recording must not replay");
    assert!(
        stderr.contains("end") && stderr.contains(&cut.display().to_string()),
        "the error must name the file and what is missing, got: {stderr}"
    );

    std::fs::remove_dir_all(&directory).ok();
}

#[cfg(feature = "render")]
#[test]
fn the_screenshot_flag_writes_a_png_of_the_expected_size() {
    use render_support::{REQUIRE_GPU_VAR, SKIP_MARKER, output_dir, png_dimensions};

    let directory = output_dir();
    std::fs::create_dir_all(&directory).expect("the output directory must be creatable");
    let path = directory.join("screenshot.png");
    std::fs::remove_file(&path).ok();

    let output = Command::new(env!("CARGO_BIN_EXE_narvo"))
        .arg("--screenshot")
        .arg(&path)
        .output()
        .expect("the narvo binary must be runnable");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        // Only a missing adapter skips. Anything else is a real failure.
        assert!(
            stderr.contains("no GPU adapter available"),
            "`narvo --screenshot` failed for a reason other than a missing adapter: {stderr}"
        );
        assert!(
            std::env::var_os(REQUIRE_GPU_VAR).is_none(),
            "{REQUIRE_GPU_VAR} is set, so a missing adapter counts as a failure \
             rather than a skip: {stderr}"
        );

        println!("{SKIP_MARKER}: {}", stderr.trim());
        return;
    }

    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the screenshot at {} is unreadable: {error}",
            path.display()
        )
    });

    let dimensions = png_dimensions(&bytes).unwrap_or_else(|| {
        panic!(
            "{} is not a PNG with a readable IHDR chunk ({} bytes)",
            path.display(),
            bytes.len()
        )
    });

    assert_eq!(
        dimensions,
        (256, 256),
        "the screenshot must have the size the runner documents"
    );

    std::fs::remove_dir_all(&directory).ok();
}
