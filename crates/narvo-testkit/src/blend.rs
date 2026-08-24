//! A generated fixture with defined alpha steps, and two solid grounds.
//!
//! The fixture M4.7's blending work is proved against. Everything here is
//! **derivable**: six uniform cells whose texel values are written down as
//! constants, so a scene built from it has a result that can be predicted on
//! paper before a GPU is asked (the fixture rule — only fully derivable,
//! machine-checkable properties belong in a fixture).
//!
//! # The alpha steps are chosen, not arbitrary
//!
//! `0`, `85`, `170`, `255`. Two of those are the ends and carry the cases that
//! need no model at all: a fully transparent premultiplied source is `[0, 0, 0,
//! 0]` and must leave the target exactly as it was, a fully opaque one must
//! replace it exactly.
//!
//! The two middles are the pair that divides evenly **both ways**: `255 - 85 =
//! 170` and `255 - 170 = 85`, so `dst * (255 - a) / 255` is a whole number for
//! `dst` of `0` and of `255` — which is every channel either ground has. That
//! matters because it removes rounding from the question this fixture exists to
//! settle. See [`Ground::RED`].
//!
//! # Premultiplied, and what that means for the step cells
//!
//! A step cell is **white already multiplied through**: all four channels carry
//! the step. That is the same representation `glyph_atlas` writes coverage in,
//! and it is what the blend state in `quad.rs` consumes. A cell therefore always
//! satisfies `rgb <= a`, which is the premultiplied invariant, with equality
//! because the colour is white.

use narvo_render2d::{Pixels, TextureRegion};

/// Texels along one edge of every cell.
///
/// Eight, like [`AtlasLayout::REGION_TEXELS`](crate::AtlasLayout::REGION_TEXELS),
/// so a 32-pixel panel drawn from one cell magnifies by exactly four and every
/// pixel centre sits strictly inside a texel. M3.15's blessing restriction — a
/// pixel centre exactly on a texel boundary makes three rasterisers disagree by
/// a whole texel — is therefore not approached; and since a cell is uniform,
/// even landing on one would read the same value.
pub const CELL_TEXELS: u32 = 8;

/// The two solid grounds a step is composited over.
///
/// Two, because they answer different questions and neither alone answers both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ground;

impl Ground {
    /// Opaque red, and the ground that **tells the two readings apart**.
    ///
    /// The blend is `src + dst * (1 - a)`, and where that arithmetic happens is
    /// not obvious from the equation. The render target is `Rgba8UnormSrgb`, so
    /// one reading says the hardware decodes the target to linear light, blends
    /// there and re-encodes; the other says it works on the stored bytes.
    ///
    /// Over this ground the two disagree by far more than rounding. With the
    /// step at `85`:
    ///
    /// - **stored bytes:** `r = 85 + 255 * 170 / 255 = 255`, exactly — the red
    ///   channel does not move at all;
    /// - **linear light:** `r = E(D(85/255) + 1 * (1 - 85/255))`, which is not
    ///   255.
    ///
    /// Both are whole numbers, so nothing about the comparison depends on how a
    /// GPU rounds. `blend_proof` renders it and reports which one happened.
    pub const RED: [u8; 4] = [255, 0, 0, 255];

    /// Opaque black, and the ground that needs **no** model.
    ///
    /// Every channel is zero, so `dst * (1 - a)` is zero whatever space the
    /// arithmetic happens in, and the result is the source term alone. A step of
    /// `a` must read back as exactly `[a, a, a, 255]` — the sRGB round trip
    /// `offscreen.rs` documents returns a sampled byte unchanged, and the alpha
    /// arithmetic is `a + 255 * (1 - a/255) = 255` in any reading.
    ///
    /// So this row is a fixed point of the whole question: if it is wrong, the
    /// source term is not arriving, and no colour-space argument can explain it.
    pub const BLACK: [u8; 4] = [0, 0, 0, 255];
}

/// The alpha steps, in the order their cells appear.
///
/// Both ends and the two even middles. See the module documentation for why
/// these four and not others.
pub const ALPHA_STEPS: [u8; 4] = [0, 85, 170, 255];

/// The cells of [`atlas`], left to right.
///
/// Grounds first so that a scene reads in the order it draws: ground, then the
/// steps that go over it.
#[must_use]
pub fn cells() -> Vec<[u8; 4]> {
    let mut cells = vec![Ground::RED, Ground::BLACK];
    cells.extend(ALPHA_STEPS.map(step_texel));
    cells
}

/// White, premultiplied by `alpha`.
///
/// All four channels carry the step, which is what "premultiplied white" means:
/// the colour is `[1, 1, 1]` and has already been multiplied by its own alpha.
#[must_use]
pub const fn step_texel(alpha: u8) -> [u8; 4] {
    [alpha, alpha, alpha, alpha]
}

/// The fixture: [`cells`] in one row, each [`CELL_TEXELS`] square.
///
/// A strip rather than a grid so that a cell's index is its column and nothing
/// has to be divided to find one.
///
/// # Panics
///
/// If the generated buffer does not match its dimensions, which would be a bug
/// in this function rather than in a caller.
#[must_use]
pub fn atlas() -> Pixels {
    let cells = cells();
    let width = CELL_TEXELS * u32::try_from(cells.len()).expect("six cells fit in a u32");
    let mut rgba = Vec::with_capacity((width * CELL_TEXELS * 4) as usize);

    for _row in 0..CELL_TEXELS {
        for cell in &cells {
            for _column in 0..CELL_TEXELS {
                rgba.extend_from_slice(cell);
            }
        }
    }

    Pixels::from_rgba8(width, CELL_TEXELS, rgba)
        .expect("the generated buffer matches its dimensions")
}

/// `glyphs` with this fixture's strip appended to its right, and the red
/// ground's region in the result.
///
/// **A draw call binds exactly one texture**, so a scene that draws glyphs over
/// a solid ground needs both in one image. The strip goes to the right of the
/// glyph texture and its rows are repeated down to the glyph texture's height —
/// a uniform cell stays uniform however far it is repeated, which is what keeps
/// the returned region derivable rather than measured.
///
/// # Why this lives here and not in the scene that draws it
///
/// It is used by `text_over_scene.rs` and transcribed by neither margin file:
/// both of those call it instead. That is a **scoped exception** to the margin
/// files' hand-transcription rule, of exactly the kind M3.35 documented for
/// `crate::text` — and it is scoped the same way. What the exception costs is
/// that a defect in *this* assembly would move both sides of those files'
/// comparison together and go unseen there. What limits the cost is that the
/// assembly is checked directly by this module's own tests, and that the
/// property the scenes actually rest on — a uniform ground column — is
/// asserted rather than assumed.
///
/// # Panics
///
/// If the combined buffer does not match its dimensions.
#[must_use]
pub fn ground_beside(glyphs: &Pixels) -> (Pixels, TextureRegion) {
    let strip = atlas();
    let width = glyphs.width() + strip.width();
    let height = glyphs.height();
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);

    for y in 0..height {
        for x in 0..glyphs.width() {
            rgba.extend_from_slice(&glyphs.pixel(x, y).expect("inside the glyph texture"));
        }
        let source_row = y.min(strip.height() - 1);
        for x in 0..strip.width() {
            rgba.extend_from_slice(&strip.pixel(x, source_row).expect("inside the strip"));
        }
    }

    let combined = Pixels::from_rgba8(width, height, rgba)
        .expect("the combined buffer matches its dimensions");

    // The red ground keeps its column inside the strip and moves right by the
    // glyph texture's width. It spans the full height, because the repeat made
    // the column uniform all the way down.
    let ground = TextureRegion::from_texels(glyphs.width(), 0, CELL_TEXELS, height, &combined);
    (combined, ground)
}

/// The region of [`atlas`] holding cell `index`.
///
/// # Panics
///
/// If `index` is not a cell of this fixture.
#[must_use]
pub fn region(index: usize, texture: &Pixels) -> TextureRegion {
    assert!(
        index < cells().len(),
        "cell {index} is not one of this fixture's {} cells",
        cells().len()
    );

    let left = CELL_TEXELS * u32::try_from(index).expect("a cell index fits in a u32");
    TextureRegion::from_texels(left, 0, CELL_TEXELS, CELL_TEXELS, texture)
}

/// The region holding the ground cell for `ground`.
///
/// # Panics
///
/// If `ground` is neither [`Ground::RED`] nor [`Ground::BLACK`].
#[must_use]
pub fn ground_region(ground: [u8; 4], texture: &Pixels) -> TextureRegion {
    let index = match ground {
        Ground::RED => 0,
        Ground::BLACK => 1,
        other => panic!("{other:?} is not one of this fixture's grounds"),
    };
    region(index, texture)
}

/// The region holding the step cell for `alpha`.
///
/// # Panics
///
/// If `alpha` is not one of [`ALPHA_STEPS`].
#[must_use]
pub fn step_region(alpha: u8, texture: &Pixels) -> TextureRegion {
    let step = ALPHA_STEPS
        .iter()
        .position(|candidate| *candidate == alpha)
        .unwrap_or_else(|| panic!("{alpha} is not one of this fixture's steps {ALPHA_STEPS:?}"));
    region(step + 2, texture)
}

#[cfg(test)]
mod tests {
    use super::{ALPHA_STEPS, CELL_TEXELS, Ground, atlas, cells, ground_region, step_region};

    #[test]
    fn the_atlas_is_one_row_of_uniform_cells() {
        let texture = atlas();
        assert_eq!(texture.height(), CELL_TEXELS);
        assert_eq!(
            texture.width(),
            CELL_TEXELS * u32::try_from(cells().len()).expect("six cells fit in a u32")
        );

        for (index, cell) in cells().iter().enumerate() {
            let left = CELL_TEXELS * u32::try_from(index).expect("a cell index fits in a u32");
            for y in 0..CELL_TEXELS {
                for x in left..left + CELL_TEXELS {
                    assert_eq!(
                        texture.pixel(x, y).expect("inside the atlas"),
                        *cell,
                        "texel ({x}, {y}) is not cell {index}'s colour"
                    );
                }
            }
        }
    }

    /// The invariant the blend state's account in `quad.rs` depends on.
    ///
    /// A premultiplied texel can never be brighter than its own coverage. Both
    /// grounds are opaque so they satisfy it trivially; the steps satisfy it
    /// with equality, because the colour being multiplied through is white.
    #[test]
    fn every_cell_is_premultiplied() {
        for cell in cells() {
            for channel in 0..3 {
                assert!(
                    cell[channel] <= cell[3],
                    "{cell:?} has more colour than coverage in channel {channel}"
                );
            }
        }
    }

    /// The property the steps were chosen for, stated as arithmetic rather than
    /// as prose.
    ///
    /// For every step and every channel value either ground has, the stored-byte
    /// reading of `dst * (255 - a) / 255` divides without a remainder. That is
    /// what makes `blend_proof`'s two candidate predictions both whole numbers,
    /// so the measurement decides between them without rounding entering it.
    #[test]
    fn the_steps_divide_evenly_against_both_grounds() {
        for step in ALPHA_STEPS {
            for ground in [Ground::RED, Ground::BLACK] {
                for channel in ground {
                    let product = u32::from(channel) * u32::from(255 - step);
                    assert_eq!(
                        product % 255,
                        0,
                        "step {step} over a channel of {channel} leaves a remainder"
                    );
                }
            }
        }
    }

    #[test]
    fn a_region_covers_exactly_its_cell() {
        let texture = atlas();

        // The two ends of the strip, by the accessors a scene uses rather than
        // by index arithmetic repeated here.
        let red = ground_region(Ground::RED, &texture);
        let opaque = step_region(255, &texture);
        assert_ne!(red, opaque, "two cells cannot be one region");

        assert_eq!(
            ground_region(Ground::BLACK, &texture),
            super::region(1, &texture)
        );
        assert_eq!(step_region(0, &texture), super::region(2, &texture));
        assert_eq!(step_region(255, &texture), super::region(5, &texture));
    }

    /// The property `ground_beside` exists to provide: one column of the
    /// combined image is the red ground, uniform from top to bottom.
    #[test]
    fn the_appended_ground_column_is_uniform_all_the_way_down() {
        // A stand-in for a glyph texture: anything with a height past the
        // strip's own single row, so the repeat has work to do.
        let glyphs = narvo_render2d::Pixels::from_rgba8(
            4,
            20,
            std::iter::repeat_n([9_u8, 8, 7, 255], 4 * 20)
                .flatten()
                .collect(),
        )
        .expect("the stand-in buffer matches its dimensions");

        let (combined, ground) = super::ground_beside(&glyphs);
        assert_eq!(
            combined.height(),
            20,
            "the combined image keeps the glyph height"
        );

        for y in 0..combined.height() {
            for x in 0..4 {
                assert_eq!(
                    combined.pixel(x, y).expect("inside the combined image"),
                    [9, 8, 7, 255],
                    "the glyph half was disturbed at ({x}, {y})"
                );
            }
            for x in 4..4 + CELL_TEXELS {
                assert_eq!(
                    combined.pixel(x, y).expect("inside the combined image"),
                    Ground::RED,
                    "the ground column is not uniform at ({x}, {y})"
                );
            }
        }

        // And the region the function hands back is that column, whole.
        assert_eq!(
            ground,
            narvo_render2d::TextureRegion::from_texels(4, 0, CELL_TEXELS, 20, &combined)
        );
    }

    #[test]
    fn an_unknown_step_says_which_ones_exist() {
        let texture = atlas();
        let error =
            std::panic::catch_unwind(|| step_region(128, &texture)).expect_err("128 is not a step");
        let message = error
            .downcast_ref::<String>()
            .expect("the panic carries a formatted message");
        assert!(
            message.contains("128 is not one of this fixture's steps"),
            "{message}"
        );
        assert!(message.contains("[0, 85, 170, 255]"), "{message}");
    }
}
