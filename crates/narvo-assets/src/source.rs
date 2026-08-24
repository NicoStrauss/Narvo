//! The file source: PNG on disk becomes [`SourceRegion`]s, and nothing else
//! changes.
//!
//! Read ADR-0024. This module is the **second producer** of the same contract
//! `atlas` already defines, and that is the whole of its design: bytes become
//! RGBA8, RGBA8 is premultiplied, and the result goes into the same
//! [`SourceRegion`](crate::SourceRegion) that a code-generated region uses. From
//! there the packer, the padding rule and the anchor are the ones that already
//! existed. **There is no second regime** — nothing downstream of
//! [`pack`](crate::pack) can tell that a file was involved, which is the
//! property §6/M4's "Contract-Weg" was chosen for.
//!
//! # PNG only, and the two conversions that are allowed
//!
//! A decoder that accepted more formats would be a decision about content
//! interchange this project has not taken. What it does accept is every PNG
//! whose expansion to RGBA8 is **exact**:
//!
//! | input | conversion | exact? |
//! |---|---|---|
//! | RGBA, 8 bit | none | yes |
//! | RGB, 8 bit | alpha set to 255 | yes |
//! | grayscale, 1/2/4/8 bit | expanded to 8 bit, then `r = g = b = grey` | yes |
//! | grayscale + alpha | as above, alpha kept | yes |
//! | palette | palette lookup, `tRNS` to alpha | yes |
//! | **anything 16 bit** | would have to discard the low byte | **refused** |
//!
//! The 16-bit refusal is the one that costs something, so it is worth being
//! plain about the alternative: `png` will happily strip 16-bit samples to 8
//! with `Transformations::STRIP_16`, and it would work. It is refused because
//! **a silent loss of half the precision in a file the author chose to author at
//! 16 bits is the kind of thing nobody discovers** — and the message says what
//! to do instead.
//!
//! # Premultiplication happens here, once
//!
//! ADR-0023 made the renderer's pipeline consume premultiplied colour. A file
//! from an ordinary image editor is straight-alpha, so somebody has to multiply,
//! and doing it at load is what keeps every later stage — packer, padding guard,
//! anchor, upload, blend — looking at one representation. See [`premultiply`]
//! for the formula and the rounding.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::atlas::{PackError, SourceRegion};

/// The file extension this module loads. Lower-case; the comparison is not.
const EXTENSION: &str = "png";

/// Something that stopped a file from becoming a region.
///
/// Every variant names the file it is about. A load walks a directory, so "it
/// failed" without a path is an error message that sends its reader to look at
/// every asset they have.
#[derive(Debug)]
pub enum AssetError {
    /// The assets directory could not be read at all.
    UnreadableDirectory {
        /// The directory that was asked for.
        path: PathBuf,
        /// What the file system said.
        detail: String,
    },
    /// One file could not be read.
    UnreadableFile {
        /// The file that was asked for.
        path: PathBuf,
        /// What the file system said.
        detail: String,
    },
    /// The bytes are not a PNG this decoder can read.
    Undecodable {
        /// The file the bytes came from.
        path: PathBuf,
        /// What the decoder said.
        detail: String,
    },
    /// The file is 16 bits per sample, which cannot be expanded exactly.
    TooDeep {
        /// The file.
        path: PathBuf,
    },
    /// The decoder produced a form this module does not know how to expand.
    ///
    /// Unreachable through the transformations set here, and kept because "we
    /// asked for an expansion and got something else" is a state worth naming
    /// rather than a state worth panicking in.
    UnexpectedForm {
        /// The file.
        path: PathBuf,
        /// What came out.
        detail: String,
    },
    /// Two files claim the same region name.
    DuplicateStem {
        /// The name they agree on, compared without case.
        name: String,
        /// The file seen first, in directory order.
        first: PathBuf,
        /// The file that collided with it.
        second: PathBuf,
    },
    /// The packer refused what the files produced.
    Pack(PackError),
}

impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnreadableDirectory { path, detail } => write!(
                f,
                "the assets directory {} could not be read: {detail}",
                path.display()
            ),
            Self::UnreadableFile { path, detail } => {
                write!(f, "{} could not be read: {detail}", path.display())
            }
            Self::Undecodable { path, detail } => write!(
                f,
                "{} is not a PNG this build can read: {detail}",
                path.display()
            ),
            Self::TooDeep { path } => write!(
                f,
                "{} has 16 bits per sample, and expanding it to 8 would discard half of \
                 every sample. Re-export it at 8 bits per sample; this decoder refuses a \
                 lossy conversion rather than making one quietly",
                path.display()
            ),
            Self::UnexpectedForm { path, detail } => write!(
                f,
                "{} decoded to {detail}, which this build cannot expand to RGBA8 exactly",
                path.display()
            ),
            Self::DuplicateStem {
                name,
                first,
                second,
            } => write!(
                f,
                "{} and {} would both be the region \"{name}\": file stems name regions and \
                 are compared without case, so two that differ only in case cannot travel \
                 between a case-sensitive file system and a case-insensitive one. Rename one \
                 of them",
                first.display(),
                second.display()
            ),
            Self::Pack(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AssetError {}

impl From<PackError> for AssetError {
    fn from(error: PackError) -> Self {
        Self::Pack(error)
    }
}

/// `colour` multiplied by `alpha`, in bytes.
///
/// # The formula, and the rounding named
///
/// ```text
/// out = (colour * alpha + 127) / 255        // integer division
/// ```
///
/// Both operands are bytes, the product fits in a `u32`, and the division is
/// integer division — so the result is a pure function of two bytes with no
/// floating point anywhere. That matters more than it looks: an atlas built from
/// files is hashed into ADR-0020's anchor, so the arithmetic that produces its
/// pixels has to give the same answer on every machine, and integer arithmetic
/// is the only kind this project is willing to promise that about.
///
/// The `+ 127` is **round to nearest**, and it is exactly that rather than
/// approximately: `the_rounding_is_round_to_nearest_on_all_65536_pairs` checks
/// every possible input against `f64` rounding. The cheaper `colour * alpha /
/// 255` truncates, which darkens every partially transparent pixel by up to one
/// count — small, systematic, and in the direction nobody notices until an edge
/// looks dirty.
///
/// # `alpha == 0` gives zero, and that is the formula rather than a special case
///
/// A fully transparent pixel contributes nothing, so its colour is unobservable
/// and premultiplied form requires it to be zero. Image editors routinely store
/// something else there — a leftover colour under a fully erased pixel, the
/// "dirty transparent" case — and under `Linear` filtering that leftover bleeds
/// into its neighbours.
///
/// This needs no branch: `colour * 0 + 127` is 127, and `127 / 255` is 0 for
/// every colour. `a_dirty_transparent_pixel_loads_as_zero` is the proof, and
/// M4.8's end-to-end test probes it through a real file and a real render.
#[must_use]
pub fn premultiply(colour: u8, alpha: u8) -> u8 {
    let product = u32::from(colour) * u32::from(alpha) + 127;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the numerator is at most 65152, so the quotient is at most 255"
    )]
    let byte = (product / 255) as u8;
    byte
}

/// Decodes `bytes` into premultiplied RGBA8, returning `(width, height, rgba)`.
///
/// `path` is used for error messages only; it need not exist.
///
/// # Errors
///
/// [`AssetError::TooDeep`] for 16 bits per sample, [`AssetError::Undecodable`]
/// for anything the decoder rejects, and [`AssetError::UnexpectedForm`] if the
/// expansion produces something other than the four forms this module knows.
pub fn decode(path: &Path, bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), AssetError> {
    // A `Cursor` rather than the slice itself: png 0.18's `Decoder` needs
    // `BufRead + Seek`, and `&[u8]` is only the first of those.
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));

    // `ALPHA` implies `EXPAND`: palettes become colour, sub-byte grayscale
    // becomes 8-bit, and a `tRNS` chunk becomes a real alpha channel. Every one
    // of those is exact. `STRIP_16` is deliberately **not** set — that is the
    // lossy one, and its absence is what makes the refusal below reachable
    // rather than decorative.
    decoder.set_transformations(png::Transformations::ALPHA);

    let mut reader = decoder
        .read_info()
        .map_err(|error| AssetError::Undecodable {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;

    if reader.info().bit_depth == png::BitDepth::Sixteen {
        return Err(AssetError::TooDeep {
            path: path.to_path_buf(),
        });
    }

    let size = reader
        .output_buffer_size()
        .ok_or_else(|| AssetError::Undecodable {
            path: path.to_path_buf(),
            detail: "the decoded size does not fit in memory".to_owned(),
        })?;
    let mut buffer = vec![0_u8; size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| AssetError::Undecodable {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })?;

    let (width, height) = (info.width, info.height);
    let pixels = width as usize * height as usize;

    // Straight-alpha RGBA8, expanded exactly from whatever came out.
    let straight: Vec<[u8; 4]> = match info.color_type {
        png::ColorType::Rgba => buffer[..pixels * 4]
            .chunks_exact(4)
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect(),
        png::ColorType::Rgb => buffer[..pixels * 3]
            .chunks_exact(3)
            .map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer[..pixels * 2]
            .chunks_exact(2)
            .map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buffer[..pixels]
            .iter()
            .map(|grey| [*grey, *grey, *grey, 255])
            .collect(),
        other => {
            return Err(AssetError::UnexpectedForm {
                path: path.to_path_buf(),
                detail: format!("{other:?}"),
            });
        }
    };

    let mut rgba = Vec::with_capacity(pixels * 4);
    for [r, g, b, a] in straight {
        rgba.extend_from_slice(&[premultiply(r, a), premultiply(g, a), premultiply(b, a), a]);
    }

    Ok((width, height, rgba))
}

/// Reads `path` and turns it into a region called `name`.
///
/// # Errors
///
/// Everything [`decode`] returns, plus [`AssetError::UnreadableFile`] and
/// whatever [`SourceRegion::new`](crate::SourceRegion) refuses.
pub fn region_from_file(name: impl Into<String>, path: &Path) -> Result<SourceRegion, AssetError> {
    let bytes = std::fs::read(path).map_err(|error| AssetError::UnreadableFile {
        path: path.to_path_buf(),
        detail: error.to_string(),
    })?;

    let (width, height, rgba) = decode(path, &bytes)?;
    Ok(SourceRegion::new(name, width, height, rgba)?)
}

/// Every `.png` in `directory`, as regions named by their file stems.
///
/// The result is sorted by name, so a caller gets the same list whatever order
/// the file system enumerates in — which is what keeps the packer's input, and
/// therefore ADR-0020's anchor, a function of the files rather than of the
/// machine.
///
/// # Case, and why the comparison is case-insensitive while the name is not
///
/// A region is named by its file stem **verbatim**: `Hero.png` is the region
/// `Hero`, and a scene asking for `hero` will not find it. Normalising the name
/// to lower case was the alternative and is rejected in ADR-0024 — it would make
/// the name in a scene file something other than the name of the file, which is
/// exactly the indirection this design avoids.
///
/// Duplicate *detection*, on the other hand, ignores case. `Hero.png` and
/// `hero.png` can coexist on Linux and cannot on Windows, so a directory holding
/// both describes a project that loads on one platform and not the other. That
/// is refused on both, in the same words, rather than discovered by whoever
/// happens to use the other machine.
///
/// # An empty directory is not an error
///
/// It packs to nothing and the caller gets an empty list. A scene that then
/// names a region gets the unknown-region error, which says what does exist —
/// and "nothing does" is a more useful thing to be told at that point than
/// "the directory was empty" was earlier.
///
/// # Errors
///
/// [`AssetError::UnreadableDirectory`] if the directory cannot be listed,
/// [`AssetError::DuplicateStem`] for two files whose stems differ only in case,
/// and everything [`region_from_file`] returns.
pub fn regions_from_directory(directory: &Path) -> Result<Vec<SourceRegion>, AssetError> {
    let entries =
        std::fs::read_dir(directory).map_err(|error| AssetError::UnreadableDirectory {
            path: directory.to_path_buf(),
            detail: error.to_string(),
        })?;

    // Collected and sorted before anything is decoded, so the order a failure is
    // reported in is the order a reader can reproduce.
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| AssetError::UnreadableDirectory {
                path: directory.to_path_buf(),
                detail: error.to_string(),
            })?
            .path();

        let is_png = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(EXTENSION));
        if is_png && path.is_file() {
            files.push(path);
        }
    }
    files.sort();

    let mut claimed: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut regions = Vec::with_capacity(files.len());

    for path in files {
        let stem = path
            .file_stem()
            .expect("a file with an extension has a stem")
            .to_string_lossy()
            .into_owned();

        if let Some(first) = claimed.insert(stem.to_lowercase(), path.clone()) {
            return Err(AssetError::DuplicateStem {
                name: stem,
                first,
                second: path,
            });
        }

        regions.push(region_from_file(stem, &path)?);
    }

    regions.sort_by(|a, b| a.name().cmp(b.name()));
    Ok(regions)
}

#[cfg(test)]
mod tests {
    use super::{decode, premultiply};
    use std::path::Path;

    /// Encodes straight-alpha RGBA8 as a PNG, which is what an image editor
    /// would have written.
    ///
    /// **Nothing binary is committed** (ADR-0024), so every test PNG is made
    /// here. That the encoder and the decoder are the same crate is a real limit
    /// and is named rather than hidden: a symmetric defect in both would be
    /// invisible to these tests. What is not invisible to them is everything
    /// this module does *between* the two — the expansion, the premultiply and
    /// the rounding — which is the part that belongs to this project.
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
    fn the_rounding_is_round_to_nearest_on_all_65536_pairs() {
        for colour in 0..=u8::MAX {
            for alpha in 0..=u8::MAX {
                let exact = f64::from(colour) * f64::from(alpha) / 255.0;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "the product of two bytes over 255 is in 0..=255"
                )]
                let nearest = exact.round() as u8;
                assert_eq!(
                    premultiply(colour, alpha),
                    nearest,
                    "premultiply({colour}, {alpha}) is not the nearest byte to {exact}"
                );
            }
        }
    }

    /// The hand-computed table the formula is documented by.
    #[test]
    fn the_premultiply_table_is_what_the_formula_says() {
        // (colour, alpha, expected)
        for (colour, alpha, expected) in [
            (255, 255, 255),
            (255, 128, 128),
            (128, 128, 64),
            (255, 0, 0),
            (200, 0, 0),
            (0, 255, 0),
            (255, 51, 51),
            (100, 51, 20),
            (1, 1, 0),
        ] {
            assert_eq!(
                premultiply(colour, alpha),
                expected,
                "premultiply({colour}, {alpha})"
            );
        }
    }

    /// The row the whole "dirty transparent" case rests on.
    #[test]
    fn a_dirty_transparent_pixel_loads_as_zero() {
        // Bright magenta under a fully erased pixel, which is what an editor
        // leaves behind and what would bleed under `Linear`.
        let bytes = encode(1, 1, &[255, 0, 255, 0]);
        let (_, _, rgba) = decode(Path::new("dirty.png"), &bytes).expect("decodes");
        assert_eq!(rgba, vec![0, 0, 0, 0]);
    }

    #[test]
    fn an_opaque_pixel_keeps_its_colour_exactly() {
        let bytes = encode(1, 1, &[12, 34, 56, 255]);
        let (width, height, rgba) = decode(Path::new("opaque.png"), &bytes).expect("decodes");
        assert_eq!((width, height), (1, 1));
        assert_eq!(rgba, vec![12, 34, 56, 255]);
    }

    #[test]
    fn a_half_transparent_pixel_is_multiplied_through() {
        let bytes = encode(1, 1, &[255, 128, 0, 128]);
        let (_, _, rgba) = decode(Path::new("half.png"), &bytes).expect("decodes");
        assert_eq!(rgba, vec![128, 64, 0, 128]);
    }

    #[test]
    fn a_sixteen_bit_file_is_refused_by_name() {
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

        let error = decode(Path::new("deep.png"), &bytes).expect_err("16 bit is refused");
        let message = error.to_string();
        assert!(message.contains("16 bits per sample"), "{message}");
        assert!(
            message.contains("Re-export it at 8 bits per sample"),
            "{message}"
        );
        assert!(message.contains("deep.png"), "{message}");
    }

    #[test]
    fn corrupt_bytes_are_refused_with_what_the_decoder_said() {
        let mut bytes = encode(2, 2, &[0; 16]);
        // Break the header rather than the payload, so the failure is in
        // reading the info rather than in a frame.
        bytes[1] = b'X';

        let error = decode(Path::new("broken.png"), &bytes).expect_err("corrupt bytes are refused");
        let message = error.to_string();
        assert!(message.contains("broken.png"), "{message}");
        assert!(
            message.contains("not a PNG this build can read"),
            "{message}"
        );
    }

    #[test]
    fn an_rgb_file_gains_an_opaque_alpha() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("a header can be written");
            writer
                .write_image_data(&[9, 8, 7])
                .expect("three bytes are one RGB pixel");
        }

        let (_, _, rgba) = decode(Path::new("rgb.png"), &bytes).expect("decodes");
        assert_eq!(rgba, vec![9, 8, 7, 255]);
    }

    #[test]
    fn a_grayscale_file_expands_to_three_equal_channels() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("a header can be written");
            writer
                .write_image_data(&[0, 200])
                .expect("two bytes are two grey pixels");
        }

        let (_, _, rgba) = decode(Path::new("grey.png"), &bytes).expect("decodes");
        assert_eq!(rgba, vec![0, 0, 0, 255, 200, 200, 200, 255]);
    }

    /// A palette image with a `tRNS` chunk expands to colour and alpha exactly.
    ///
    /// The row of the table nothing else covers, and the one where "exact"
    /// carries the most weight: a palette lookup is a table read, so the result
    /// is the palette entry byte for byte.
    #[test]
    fn a_palette_file_expands_through_its_table_and_its_transparency() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            // Two entries: opaque orange, and one the tRNS chunk makes fully
            // transparent while leaving a colour behind it — the dirty
            // transparent case arriving through a palette.
            encoder.set_palette(vec![255, 128, 0, 0, 255, 0]);
            encoder.set_trns(vec![255, 0]);
            let mut writer = encoder.write_header().expect("a header can be written");
            writer
                .write_image_data(&[0, 1])
                .expect("two bytes are two palette indices");
        }

        let (_, _, rgba) = decode(Path::new("palette.png"), &bytes).expect("decodes");
        assert_eq!(
            rgba,
            vec![255, 128, 0, 255, 0, 0, 0, 0],
            "the opaque entry keeps its colour and the transparent one loses its"
        );
    }
}
