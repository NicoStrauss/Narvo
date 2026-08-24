//! Comparing rendered output against blessed reference images.
//!
//! A golden-image test renders something, loads the reference PNG that was
//! blessed for it, and fails if the two have drifted apart. Reference images
//! freeze the orientation convention of ADR-0004 as surely as they freeze the
//! colours, which is why they are treated as authoritative: a red golden test
//! means the renderer changed, until somebody proves otherwise.
//!
//! Nothing in this module ever writes a reference image. A missing reference is
//! an error with instructions, not an invitation to create one - see
//! [`GoldenError::ReferenceMissing`].

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::offscreen::Pixels;

/// Where a failing golden comparison writes what it rendered.
///
/// `<cargo target directory>/[<target triple>/]<profile>/golden` — the directory
/// cargo builds this profile's output into — derived from the path of the
/// running test binary. Every golden test in this repository passes this to
/// [`Golden::new`], so there is one rule and one implementation of it rather
/// than one per test file.
///
/// # Why the binary's own path, and not an environment variable
///
/// Cargo offers `CARGO_TARGET_TMPDIR` — measured as `<target>/tmp` — but sets it
/// **only for integration tests and benches**. That is the whole trouble: the
/// golden test of `narvo-app` has to be a unit test, because that crate has no
/// library target and `placements_of` is reachable from nowhere else (ADR-0015).
/// M3.10 answered the gap with [`std::env::temp_dir`], which put the one
/// artifact only a human can act on — the image to be blessed — into
/// `%TEMP%\amboss-golden-app\`, a directory Windows Search does not index and the
/// maintainer did not find.
///
/// A test binary always knows its own path, so this derivation has no exception,
/// and cargo builds that binary inside whatever target directory it was told to
/// use — so the result follows `CARGO_TARGET_DIR` without reading it. Measured
/// rather than assumed (M3.11): with `CARGO_TARGET_DIR` pointing at a scratch
/// directory, [`std::env::current_exe`] returned
/// `<scratch>\debug\deps\<name>-<hash>.exe` and `CARGO_TARGET_TMPDIR` was
/// `<scratch>\tmp`. Both follow it; only one is available everywhere.
///
/// The directory holding a test binary is `deps`, so the artifacts go one level
/// up, beside the profile's other output. Naming the profile directory rather
/// than the target root is what keeps a `release` run from overwriting what a
/// `debug` run left for somebody to look at.
///
/// # Panics
///
/// If the running executable's path cannot be determined. There is deliberately
/// no fallback: falling back to the system temp directory is the behaviour this
/// function exists to remove, and it would do it silently.
#[must_use]
pub fn golden_artifact_dir() -> PathBuf {
    let executable = std::env::current_exe().expect(
        "a test needs its own path to place failure artifacts under the cargo \
         target directory, and there is deliberately no system-temp fallback",
    );

    let directory = executable
        .parent()
        .expect("an executable's path has a parent directory");

    // `<target>/<profile>/deps/<binary>` for anything cargo builds as a test.
    // Anything run from elsewhere keeps its artifacts beside itself rather than
    // one level up into a directory this function did not put it in.
    let base = if directory.file_name() == Some(OsStr::new("deps")) {
        directory.parent().unwrap_or(directory)
    } else {
        directory
    };

    base.join("golden")
}

/// How far a rendered image may drift from its reference before it counts as a
/// regression.
///
/// Three thresholds, because no single number separates "a CPU rasteriser
/// rounded differently" from "the renderer broke":
///
/// - [`channel`](Self::channel) is the noise floor. A pixel whose every channel
///   is within it is treated as matching and is not counted at all.
/// - [`max_differing_ratio`](Self::max_differing_ratio) bounds how much of the
///   image may exceed that floor. Without it, an image that is uniformly a
///   little wrong - a shifted colour space, a changed clear value - would pass
///   on the strength of each individual pixel being nearly right.
/// - [`max_channel_deviation`](Self::max_channel_deviation) bounds how wrong any
///   single pixel may be. Without it, the ratio budget would forgive a handful
///   of catastrophically wrong pixels, which is exactly what a broken sampler or
///   an off-by-one in an atlas looks like.
///
/// A single displaced pixel and a completely wrong image therefore fail for
/// different reasons and say so in the message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// Per-channel difference that still counts as a match, in counts of 255.
    pub channel: u8,
    /// Largest per-channel difference any single pixel may show.
    pub max_channel_deviation: u8,
    /// Fraction of pixels, `0.0..=1.0`, that may exceed [`channel`](Self::channel).
    pub max_differing_ratio: f64,
}

impl Default for Tolerance {
    /// Headroom for cases nobody has measured, not a fitted difference.
    ///
    /// The word "tolerance" invites the assumption that these values were
    /// derived from an observed spread between rasterisers. They were not, and
    /// M3.1 is what establishes it: llvmpipe, WARP and a discrete GPU each
    /// render the only reference image blessed so far — an axis-aligned 64 x 64
    /// quad — byte-identical to it and to one another. The figures live in
    /// `docs/perf/BASELINE.md` under "Golden-image margin" and are deliberately
    /// not copied here, because a reference ages better than a copy.
    ///
    /// Every count below is therefore unused, and the case that was measured is
    /// the one least able to produce a disagreement in the first place:
    /// axis-aligned geometry landing on pixel boundaries gives two
    /// implementations nothing to round differently.
    ///
    /// Three things M3 adds would give them something: filtered sampling at
    /// non-integer texture coordinates, a camera transform that puts vertices
    /// between pixel centres, and glyph coverage. The first reference image
    /// containing one of them is when these values are worth arguing about
    /// again, on whatever evidence that image produces. Until then they are
    /// untested in the direction that matters, which is not the same as being
    /// right.
    fn default() -> Self {
        Self {
            channel: 4,
            max_channel_deviation: 24,
            max_differing_ratio: 0.001,
        }
    }
}

/// The single worst pixel of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorstPixel {
    /// Column, counted from the left.
    pub x: u32,
    /// Row, counted from the top - see ADR-0004.
    pub y: u32,
    /// What the reference has there.
    pub reference: [u8; 4],
    /// What was rendered there.
    pub actual: [u8; 4],
    /// Largest single-channel difference at this pixel.
    pub deviation: u8,
}

/// What a comparison measured, whether or not it passed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoldenReport {
    /// Pixels compared.
    pub total_pixels: u64,
    /// Pixels exceeding [`Tolerance::channel`] on at least one channel.
    pub differing_pixels: u64,
    /// [`differing_pixels`](Self::differing_pixels) over
    /// [`total_pixels`](Self::total_pixels).
    pub differing_ratio: f64,
    /// Largest single-channel difference anywhere in the image.
    pub max_channel_deviation: u8,
    /// Where that difference is, if the images differ at all.
    pub worst_pixel: Option<WorstPixel>,
}

impl GoldenReport {
    /// Every threshold beside the number actually measured against it.
    ///
    /// A failing comparison says how far it missed by;
    /// [`GoldenError::Mismatch`]'s message is built from exactly these figures.
    /// A passing one said nothing, and that asymmetry is why M1 could record "0
    /// of 4096 pixels" for Linux and only "passed" for Windows — leaving it
    /// unknown whether the margin on the second platform was comfortable or one
    /// count wide. The numbers were always there; nothing asked for them.
    ///
    /// Printed by a passing golden test, this reaches the CI log on both
    /// platforms: `.config/nextest.toml` gives `narvo-render2d` a
    /// `success-output` override for the neighbouring reason, that a skipped
    /// renderer test must not read like a passing one.
    #[must_use]
    pub fn measured_against(&self, tolerance: Tolerance) -> String {
        let mut summary = format!(
            "{} of {} pixels ({:.4}%) differ by more than {} counts, budget {:.4}%; \
             worst channel deviation {} counts, limit {}",
            self.differing_pixels,
            self.total_pixels,
            self.differing_ratio * 100.0,
            tolerance.channel,
            tolerance.max_differing_ratio * 100.0,
            self.max_channel_deviation,
            tolerance.max_channel_deviation
        );

        match self.worst_pixel {
            Some(worst) => summary.push_str(&format!(
                "\n  worst pixel: ({}, {}) reference {:?} against render {:?}",
                worst.x, worst.y, worst.reference, worst.actual
            )),
            None => summary.push_str("\n  worst pixel: none, the images are identical"),
        }

        summary
    }
}

/// A reference directory, an output directory and a tolerance.
///
/// Reference images are read from `reference_dir` and never written. Failure
/// artifacts - what was rendered, and a diff highlighting where it differs - go
/// to `output_dir`, which belongs under `target/` so that a failed run can never
/// leave anything behind that could be committed by accident.
///
/// [`golden_artifact_dir`] is that directory, and every golden test in this
/// repository passes it here. The field stays a parameter rather than becoming
/// the fixed answer, because a test that wants its own scratch subdirectory -
/// this module's own do - needs to say so.
#[derive(Debug, Clone, Copy)]
pub struct Golden<'paths> {
    /// Directory holding blessed reference PNGs, one per test name.
    pub reference_dir: &'paths Path,
    /// Directory for the artifacts a failing comparison writes.
    pub output_dir: &'paths Path,
    /// How much drift is accepted.
    pub tolerance: Tolerance,
}

impl<'paths> Golden<'paths> {
    /// Uses [`Tolerance::default`].
    #[must_use]
    pub fn new(reference_dir: &'paths Path, output_dir: &'paths Path) -> Self {
        Self {
            reference_dir,
            output_dir,
            tolerance: Tolerance::default(),
        }
    }

    /// Path the reference for `name` is expected at.
    #[must_use]
    pub fn reference_path(&self, name: &str) -> PathBuf {
        self.reference_dir.join(format!("{name}.png"))
    }

    /// Compares `actual` against the blessed reference for `name`.
    ///
    /// On failure the rendered image, and where it makes sense a diff image, are
    /// written to the output directory and named in the error, so a maintainer
    /// can look at what happened without reproducing the run.
    ///
    /// # Errors
    ///
    /// - [`GoldenError::ReferenceMissing`] if no reference has been blessed yet.
    ///   This is never resolved by writing one here.
    /// - [`GoldenError::ReferenceUnreadable`] if the file exists but will not
    ///   decode.
    /// - [`GoldenError::SizeMismatch`] if the dimensions disagree, which makes a
    ///   pixel comparison meaningless.
    /// - [`GoldenError::Mismatch`] if the images differ beyond the tolerance.
    /// - [`GoldenError::OutputWrite`] if a failure artifact could not be written.
    pub fn verify(&self, name: &str, actual: &Pixels) -> Result<GoldenReport, GoldenError> {
        let reference_path = self.reference_path(name);

        if !reference_path.is_file() {
            return Err(GoldenError::ReferenceMissing {
                name: name.to_owned(),
                expected_at: reference_path,
                rendered_to: self.write_artifact(name, "actual", actual)?,
            });
        }

        let reference = load_reference(&reference_path)?;

        if (reference.width(), reference.height()) != (actual.width(), actual.height()) {
            return Err(GoldenError::SizeMismatch {
                name: name.to_owned(),
                reference_size: (reference.width(), reference.height()),
                actual_size: (actual.width(), actual.height()),
                reference_at: reference_path,
                rendered_to: self.write_artifact(name, "actual", actual)?,
            });
        }

        let (report, diff) = compare(&reference, actual, self.tolerance);

        let too_many = report.differing_ratio > self.tolerance.max_differing_ratio;
        let too_far = report.max_channel_deviation > self.tolerance.max_channel_deviation;
        if !too_many && !too_far {
            return Ok(report);
        }

        Err(GoldenError::Mismatch(Box::new(MismatchDetails {
            name: name.to_owned(),
            report,
            tolerance: self.tolerance,
            too_many,
            too_far,
            reference_at: reference_path,
            rendered_to: self.write_artifact(name, "actual", actual)?,
            diff_at: self.write_artifact(name, "diff", &diff)?,
        })))
    }

    /// Writes one failure artifact and returns where it landed.
    fn write_artifact(
        &self,
        name: &str,
        kind: &str,
        image: &Pixels,
    ) -> Result<PathBuf, GoldenError> {
        std::fs::create_dir_all(self.output_dir).map_err(|error| GoldenError::OutputWrite {
            path: self.output_dir.to_path_buf(),
            source: Box::new(error),
        })?;

        let path = self.output_dir.join(format!("{name}.{kind}.png"));
        image
            .save_png(&path)
            .map_err(|error| GoldenError::OutputWrite {
                path: path.clone(),
                source: Box::new(error),
            })?;

        Ok(path)
    }
}

/// Reads a reference PNG into [`Pixels`].
fn load_reference(path: &Path) -> Result<Pixels, GoldenError> {
    let decoded = image::open(path)
        .map_err(|error| GoldenError::ReferenceUnreadable {
            path: path.to_path_buf(),
            source: Box::new(error),
        })?
        .to_rgba8();

    let (width, height) = (decoded.width(), decoded.height());
    Pixels::from_rgba8(width, height, decoded.into_raw()).map_err(|error| {
        GoldenError::ReferenceUnreadable {
            path: path.to_path_buf(),
            source: Box::new(error),
        }
    })
}

/// Measures the difference and builds the diff image in one pass.
///
/// The diff dims the reference to a dark grey wherever the images agree and
/// paints magenta wherever they do not, so the shape of a regression is legible
/// at a glance instead of having to be read out of coordinates.
fn compare(reference: &Pixels, actual: &Pixels, tolerance: Tolerance) -> (GoldenReport, Pixels) {
    let width = reference.width();
    let height = reference.height();

    let mut differing_pixels = 0_u64;
    let mut max_channel_deviation = 0_u8;
    let mut worst_pixel = None;
    let mut diff = Vec::with_capacity(width as usize * height as usize * 4);

    for (index, (reference_pixel, actual_pixel)) in reference
        .rgba()
        .chunks_exact(4)
        .zip(actual.rgba().chunks_exact(4))
        .enumerate()
    {
        let deviation = reference_pixel
            .iter()
            .zip(actual_pixel.iter())
            .map(|(left, right)| left.abs_diff(*right))
            .max()
            .unwrap_or(0);

        if deviation > max_channel_deviation {
            max_channel_deviation = deviation;

            let index = index as u32;
            worst_pixel = Some(WorstPixel {
                x: index % width,
                y: index / width,
                reference: [
                    reference_pixel[0],
                    reference_pixel[1],
                    reference_pixel[2],
                    reference_pixel[3],
                ],
                actual: [
                    actual_pixel[0],
                    actual_pixel[1],
                    actual_pixel[2],
                    actual_pixel[3],
                ],
                deviation,
            });
        }

        if deviation > tolerance.channel {
            differing_pixels += 1;
            diff.extend_from_slice(&[255, 0, 255, 255]);
        } else {
            let grey = reference_pixel[0] / 4 + reference_pixel[1] / 4 + reference_pixel[2] / 4;
            diff.extend_from_slice(&[grey / 3, grey / 3, grey / 3, 255]);
        }
    }

    let total_pixels = u64::from(width) * u64::from(height);
    let report = GoldenReport {
        total_pixels,
        differing_pixels,
        #[expect(
            clippy::cast_precision_loss,
            reason = "a ratio only needs to be accurate enough to compare against a budget"
        )]
        differing_ratio: differing_pixels as f64 / total_pixels as f64,
        max_channel_deviation,
        worst_pixel,
    };

    let diff = Pixels::from_rgba8(width, height, diff)
        .expect("the diff buffer is built pixel by pixel from the reference dimensions");

    (report, diff)
}

/// Everything a comparison recorded when it found the images too far apart.
///
/// Carried boxed inside [`GoldenError::Mismatch`].
#[derive(Debug)]
pub struct MismatchDetails {
    /// Test name.
    pub name: String,
    /// What the comparison measured.
    pub report: GoldenReport,
    /// What it was measured against.
    pub tolerance: Tolerance,
    /// Whether the differing-pixel budget was exceeded.
    pub too_many: bool,
    /// Whether a single pixel deviated further than the cap.
    pub too_far: bool,
    /// Where the reference lives.
    pub reference_at: PathBuf,
    /// Where the rendered image was written.
    pub rendered_to: PathBuf,
    /// Where the diff image was written.
    pub diff_at: PathBuf,
}

/// Something that stopped a golden comparison from passing.
#[derive(Debug)]
#[non_exhaustive]
pub enum GoldenError {
    /// No reference image has been blessed for this test yet.
    ///
    /// Deliberately an error rather than a skip or a silent write. A test that
    /// created its own reference would pass forever afterwards while asserting
    /// only that the renderer still agrees with whatever it happened to produce
    /// the first time, which is not a test.
    ReferenceMissing {
        /// Test name that was looked up.
        name: String,
        /// Where the reference was expected.
        expected_at: PathBuf,
        /// Where the rendered image was written for inspection.
        rendered_to: PathBuf,
    },
    /// The reference file exists but could not be read or decoded.
    ReferenceUnreadable {
        /// File that could not be read.
        path: PathBuf,
        /// The decoder's error.
        source: Box<dyn Error + Send + Sync>,
    },
    /// The rendered image is a different size than the reference.
    SizeMismatch {
        /// Test name.
        name: String,
        /// Reference dimensions.
        reference_size: (u32, u32),
        /// Rendered dimensions.
        actual_size: (u32, u32),
        /// Where the reference lives.
        reference_at: PathBuf,
        /// Where the rendered image was written.
        rendered_to: PathBuf,
    },
    /// The images differ beyond the tolerance.
    ///
    /// Boxed because it carries by far the most: leaving it inline would make
    /// every `Result` in this module wide enough for clippy's `result_large_err`
    /// to object, and rightly so. The failure path can afford one allocation;
    /// the success path should not pay for it in stack width.
    Mismatch(Box<MismatchDetails>),
    /// A failure artifact could not be written.
    OutputWrite {
        /// Path that could not be written.
        path: PathBuf,
        /// The filesystem's or encoder's error.
        source: Box<dyn Error + Send + Sync>,
    },
}

impl fmt::Display for GoldenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReferenceMissing {
                name,
                expected_at,
                rendered_to,
            } => write!(
                f,
                "golden reference for \"{name}\" is missing.\n  \
                 expected at: {}\n  \
                 rendered image written to: {}\n  \
                 reference missing, ask the maintainer to bless it. Do not create the file: \
                 reference images are blessed by a human who has looked at them, and a \
                 self-blessed reference asserts only that the renderer still agrees with \
                 itself.",
                expected_at.display(),
                rendered_to.display()
            ),
            Self::ReferenceUnreadable { path, source } => write!(
                f,
                "golden reference {} could not be read: {source}",
                path.display()
            ),
            Self::SizeMismatch {
                name,
                reference_size,
                actual_size,
                reference_at,
                rendered_to,
            } => write!(
                f,
                "golden comparison for \"{name}\" cannot run: the reference is {}x{} but the \
                 render is {}x{}.\n  \
                 reference: {}\n  \
                 rendered:  {}\n  \
                 Either the test asked for a different target size than the reference was \
                 blessed at, or the reference belongs to another test.",
                reference_size.0,
                reference_size.1,
                actual_size.0,
                actual_size.1,
                reference_at.display(),
                rendered_to.display()
            ),
            Self::Mismatch(details) => {
                let MismatchDetails {
                    name,
                    report,
                    tolerance,
                    too_many,
                    too_far,
                    reference_at,
                    rendered_to,
                    diff_at,
                } = details.as_ref();

                write!(f, "golden comparison for \"{name}\" failed: ")?;

                if *too_many {
                    write!(
                        f,
                        "{} of {} pixels ({:.4}%) differ by more than {} counts, budget is {:.4}%",
                        report.differing_pixels,
                        report.total_pixels,
                        report.differing_ratio * 100.0,
                        tolerance.channel,
                        tolerance.max_differing_ratio * 100.0
                    )?;
                }
                if *too_many && *too_far {
                    write!(f, "; and ")?;
                }
                if *too_far {
                    write!(
                        f,
                        "the worst pixel is off by {} counts, limit is {}",
                        report.max_channel_deviation, tolerance.max_channel_deviation
                    )?;
                }

                if let Some(worst) = report.worst_pixel {
                    write!(
                        f,
                        ".\n  worst pixel: ({}, {}) reference {:?} against render {:?}",
                        worst.x, worst.y, worst.reference, worst.actual
                    )?;
                }

                write!(
                    f,
                    "\n  reference: {}\n  rendered:  {}\n  diff:      {}\n  \
                     A red golden test means the renderer changed. Treat the reference as \
                     correct until proven otherwise; if it really is outdated, ask the \
                     maintainer to re-bless it rather than overwriting it.",
                    reference_at.display(),
                    rendered_to.display(),
                    diff_at.display()
                )
            }
            Self::OutputWrite { path, source } => write!(
                f,
                "the golden comparison could not write {}: {source}",
                path.display()
            ),
        }
    }
}

impl Error for GoldenError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReferenceMissing { .. } | Self::SizeMismatch { .. } | Self::Mismatch(_) => None,
            Self::ReferenceUnreadable { source, .. } | Self::OutputWrite { source, .. } => {
                Some(source.as_ref())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Golden, GoldenError, GoldenReport, Tolerance, golden_artifact_dir};
    use crate::offscreen::Pixels;

    /// A `size` x `size` image filled with one colour.
    fn flat(size: u32, colour: [u8; 4]) -> Pixels {
        let rgba = colour
            .iter()
            .copied()
            .cycle()
            .take(size as usize * size as usize * 4)
            .collect();
        Pixels::from_rgba8(size, size, rgba).expect("a flat buffer matches its dimensions")
    }

    /// A scratch output directory, unique per test process.
    ///
    /// A subdirectory of [`golden_artifact_dir`] rather than the directory
    /// itself: these tests end by removing what they wrote, and the shared
    /// directory holds the artifacts of the golden tests running beside them.
    fn output_dir() -> std::path::PathBuf {
        golden_artifact_dir().join(format!("unit-{}", std::process::id()))
    }

    /// The artifact directory is inside the target directory the test was built
    /// into, whatever that directory is called.
    ///
    /// Asserted through the running binary rather than against a literal path,
    /// because a literal would have to name `target/debug` and would then be
    /// wrong under `CARGO_TARGET_DIR`, under `--target`, and in `release` — the
    /// three cases the derivation exists to survive.
    #[test]
    fn the_artifact_directory_holds_the_test_binary_that_writes_into_it() {
        let directory = golden_artifact_dir();
        println!("golden_artifact_dir() = {}", directory.display());

        assert!(
            directory.is_absolute(),
            "a relative artifact path would depend on the working directory a \
             test runner happens to choose: {}",
            directory.display()
        );
        assert_eq!(
            directory.file_name(),
            Some(std::ffi::OsStr::new("golden")),
            "the directory has to be named for what it holds: {}",
            directory.display()
        );

        let executable = std::env::current_exe().expect("this test is running from a file");
        let enclosing = directory
            .parent()
            .expect("an absolute path ending in `golden` has a parent");
        assert!(
            executable.starts_with(enclosing),
            "the artifacts must land beside the build output of the binary that \
             writes them, which is what puts them inside the cargo target \
             directory. Executable {} is not under {}",
            executable.display(),
            enclosing.display()
        );
    }

    /// The name the golden-image test blessed its reference under.
    const BLESSED: &str = "textured_quad_quadrants_64x64";

    /// Copies the blessed reference into `dir` and returns what it holds.
    ///
    /// `tests/golden/` is read and never written — not by this test and not by
    /// anything automated. The copy is a byte copy rather than a re-encode, so
    /// the comparison below runs against the same pixels the golden-image test
    /// sees.
    fn blessed_reference_copied_into(dir: &std::path::Path) -> Pixels {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/golden")
            .join(format!("{BLESSED}.png"));

        std::fs::create_dir_all(dir).expect("the temp reference directory must be creatable");
        std::fs::copy(&source, dir.join(format!("{BLESSED}.png")))
            .unwrap_or_else(|error| panic!("cannot copy {}: {error}", source.display()));

        super::load_reference(&source).expect("the blessed reference must decode")
    }

    /// Shifts the red channel of the first `count` pixels by exactly `delta`.
    ///
    /// The shift moves away from whatever value is there, so it never clamps at
    /// 0 or 255 and the resulting deviation is `delta` regardless of what the
    /// reference happens to hold. Only one channel moves, so the per-pixel
    /// deviation - a maximum across channels - is `delta` and not something
    /// derived from it.
    fn red_shifted(reference: &Pixels, count: usize, delta: u8) -> Pixels {
        let mut rgba = reference.rgba().to_vec();

        for pixel in rgba.chunks_exact_mut(4).take(count) {
            pixel[0] = if pixel[0] >= delta {
                pixel[0] - delta
            } else {
                pixel[0] + delta
            };
        }

        Pixels::from_rgba8(reference.width(), reference.height(), rgba)
            .expect("perturbing pixels does not change the dimensions")
    }

    /// Each threshold at its own boundary, against the real blessed reference.
    ///
    /// The point is not that a perturbed image fails. It is that each of the
    /// three thresholds can be made to decide the outcome *on its own*, one
    /// count either side of where it is set — so a number this measurement path
    /// reports can be read as "this threshold, this far from tripping" rather
    /// than as an undifferentiated "passed". A figure that is merely non-zero
    /// establishes nothing (M2.5).
    ///
    /// The denominator is the blessed image's own pixel count rather than a
    /// literal 4096, so re-blessing at another size moves the expectation with
    /// it instead of turning this red for the wrong reason.
    #[test]
    fn every_threshold_decides_on_its_own_at_its_boundary() {
        let output = output_dir().join("boundaries");
        let references = output.join("references");
        let reference = blessed_reference_copied_into(&references);
        let golden = Golden::new(&references, &output);
        let tolerance = Tolerance::default();

        let total = u64::from(reference.width()) * u64::from(reference.height());
        let ratio_of = |count: u64| count as f64 / total as f64;

        // Each case prints what it measured, through the same method a passing
        // golden test prints. The assertions pin the numbers; the printout is
        // what lets a reader take them apart afterwards without rerunning.
        let show = |case: &str, report: &GoldenReport| {
            println!("{case}: {}", report.measured_against(tolerance));
        };

        // The noise floor counts a pixel only when it is exceeded, not met.
        let report = golden
            .verify(BLESSED, &red_shifted(&reference, 1, tolerance.channel))
            .expect("a pixel at the noise floor is not a difference");
        show("noise floor, 1 pixel at the floor", &report);
        assert_eq!(report.differing_pixels, 0);
        assert_eq!(report.max_channel_deviation, tolerance.channel);

        let report = golden
            .verify(BLESSED, &red_shifted(&reference, 1, tolerance.channel + 1))
            .expect("one pixel over the floor is still far inside both budgets");
        show("noise floor, 1 pixel one count over", &report);
        assert_eq!(report.differing_pixels, 1);
        assert_eq!(report.max_channel_deviation, tolerance.channel + 1);

        // The pixel budget, one pixel either side. floor(0.001 * 4096) = 4.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a pixel count that fits the image fits u64 by construction"
        )]
        let budget = (tolerance.max_differing_ratio * total as f64) as u64;

        let report = golden
            .verify(BLESSED, &red_shifted(&reference, budget as usize, 10))
            .expect("exactly the budget is within the budget");
        show("pixel budget, exactly the budget", &report);
        assert_eq!(report.differing_pixels, budget);
        assert!(report.differing_ratio <= tolerance.max_differing_ratio);
        assert_eq!(report.max_channel_deviation, 10);

        let error = golden
            .verify(BLESSED, &red_shifted(&reference, budget as usize + 1, 10))
            .expect_err("one pixel past the budget must fail");
        let GoldenError::Mismatch(details) = &error else {
            panic!("expected a mismatch, got {error}")
        };
        show("pixel budget, one pixel over", &details.report);
        assert_eq!(details.report.differing_pixels, budget + 1);
        assert!((details.report.differing_ratio - ratio_of(budget + 1)).abs() < f64::EPSILON);
        assert!(
            details.too_many,
            "the pixel budget is what should have tripped"
        );
        assert!(
            !details.too_far,
            "the per-pixel cap must be untouched at 10 counts"
        );

        // The per-pixel cap, one count either side, on a single pixel so the
        // ratio stays far inside its budget and cannot be what decides.
        let report = golden
            .verify(
                BLESSED,
                &red_shifted(&reference, 1, tolerance.max_channel_deviation),
            )
            .expect("a pixel exactly at the cap is within the cap");
        show("per-pixel cap, 1 pixel exactly at the cap", &report);
        assert_eq!(
            report.max_channel_deviation,
            tolerance.max_channel_deviation
        );

        let error = golden
            .verify(
                BLESSED,
                &red_shifted(&reference, 1, tolerance.max_channel_deviation + 1),
            )
            .expect_err("one count past the cap must fail");
        let GoldenError::Mismatch(details) = &error else {
            panic!("expected a mismatch, got {error}")
        };
        show("per-pixel cap, 1 pixel one count over", &details.report);
        assert_eq!(
            details.report.max_channel_deviation,
            tolerance.max_channel_deviation + 1
        );
        assert_eq!(details.report.differing_pixels, 1);
        assert!(
            details.too_far,
            "the per-pixel cap is what should have tripped"
        );
        assert!(
            !details.too_many,
            "one pixel in {total} is far inside the ratio budget"
        );

        // The whole per-process directory, not just this test's subdirectory of
        // it: since M3.11 the parent is the shared artifact directory a
        // maintainer looks in for a `.actual.png`, and an empty scratch
        // directory left beside that file is noise in the one place that has to
        // stay readable.
        std::fs::remove_dir_all(output_dir()).ok();
    }

    #[test]
    fn a_missing_reference_is_an_error_that_says_what_to_do() {
        let output = output_dir();
        let references = output.join("references-that-do-not-exist");
        let golden = Golden::new(&references, &output);

        let error = golden
            .verify("nothing-blessed-here", &flat(4, [10, 20, 30, 255]))
            .expect_err("a missing reference must fail");

        let message = error.to_string();
        // Printed, not just asserted on: this is the other half of the
        // measurement path. A run that cannot measure has to say so in a form a
        // reader can act on, and the only way to know it still does is to look
        // at it.
        println!("missing reference: {message}");
        assert!(
            matches!(error, GoldenError::ReferenceMissing { .. }),
            "got {message}"
        );
        assert!(
            message.contains("ask the maintainer to bless"),
            "the message must say what to do, got: {message}"
        );

        // The rendered image is still written, so a maintainer can look at it.
        let GoldenError::ReferenceMissing { rendered_to, .. } = &error else {
            unreachable!("checked above")
        };
        assert!(
            rendered_to.is_file(),
            "the rendered image must be written even when the reference is missing"
        );

        // And no reference was created on the way past.
        assert!(
            !references.exists(),
            "verify() must never create the reference directory"
        );

        std::fs::remove_dir_all(&output).ok();
    }

    #[test]
    fn noise_below_the_channel_tolerance_is_not_counted() {
        let (report, _diff) = super::compare(
            &flat(8, [100, 100, 100, 255]),
            &flat(8, [103, 97, 100, 255]),
            Tolerance::default(),
        );

        assert_eq!(report.differing_pixels, 0);
        assert_eq!(report.max_channel_deviation, 3);
    }

    #[test]
    fn a_uniformly_shifted_image_fails_on_the_pixel_budget_not_the_cap() {
        // Every pixel off by 8: under the per-pixel cap of 24, over the noise
        // floor of 4. Only the ratio criterion catches this.
        let (report, _diff) = super::compare(
            &flat(8, [100, 100, 100, 255]),
            &flat(8, [108, 108, 108, 255]),
            Tolerance::default(),
        );

        assert_eq!(report.differing_pixels, 64);
        assert!((report.differing_ratio - 1.0).abs() < f64::EPSILON);
        assert!(report.max_channel_deviation <= Tolerance::default().max_channel_deviation);
    }

    #[test]
    fn one_badly_wrong_pixel_fails_on_the_cap_not_the_budget() {
        let reference = flat(16, [0, 0, 0, 255]);
        let mut rgba = reference.rgba().to_vec();
        rgba[0..4].copy_from_slice(&[255, 255, 255, 255]);
        let actual = Pixels::from_rgba8(16, 16, rgba).expect("same dimensions");

        let (report, _diff) = super::compare(&reference, &actual, Tolerance::default());

        assert_eq!(report.differing_pixels, 1);
        assert!(
            report.differing_ratio <= Tolerance::default().max_differing_ratio + f64::EPSILON
                || report.differing_ratio < 0.005,
            "one pixel in 256 stays a small fraction: {}",
            report.differing_ratio
        );
        assert_eq!(report.max_channel_deviation, 255);

        let worst = report.worst_pixel.expect("there is a worst pixel");
        assert_eq!((worst.x, worst.y), (0, 0));
    }
}
