//! The test fixtures this workspace shares, in one place (D16, ADR-0016).
//!
//! Every crate here is a **caller**. Before M3.22 the quadrant texture had
//! seven definitions in seven files and the four-quadrant atlas had **ten in
//! nine files** — seven at 16 x 16, three at 20 x 20 — and M3.21 showed what
//! that costs: padding two of them left five others describing a shape that had
//! stopped being M3.9's.
//!
//! # What lives here and what does not
//!
//! Fixture *data* — the texel grids and the geometry that indexes them. Not the
//! rules that check them: [`narvo_render2d::check_region_padding`] stays where
//! M3.21 put it, because it is the rule a later atlas packer (D3/M4) has to
//! satisfy and not a test helper. This crate calls it; it does not restate it.
//!
//! # Two return shapes, and why
//!
//! Every fixture is offered twice: once returning `Vec<u8>` and once returning
//! [`Pixels`]. That is not convenience, it is a measured constraint.
//!
//! This crate is a `dev-dependency` of `narvo-render2d` and also depends on it
//! — a cycle cargo permits for dev-dependencies. A cycle has one consequence,
//! measured in M3.22 rather than assumed: `narvo-render2d`'s **own**
//! `#[cfg(test)]` modules link a second compilation of that crate, so a `Pixels`
//! built here is a different type from theirs. The compiler's words:
//!
//! ```text
//! error[E0308]: mismatched types
//!   expected `offscreen::Pixels`, found `narvo_render2d::offscreen::Pixels`
//! ```
//!
//! `Vec<u8>` is `std`, so it crosses that seam unchanged. In-crate unit tests of
//! `narvo-render2d` therefore call the `_rgba` form and wrap it themselves;
//! everybody else — integration tests in any crate, and every test in
//! `narvo-app`, whose test build links the same `narvo-render2d` this crate
//! does — uses the [`Pixels`] form. **The pixel truth is the `_rgba` function in
//! both cases**, which is what makes "one definition" true rather than nearly
//! true.

pub mod blend;
pub mod glyph_atlas;
pub mod sha256;
pub mod srgb;
pub mod text;

use narvo_render2d::{Pixels, REGION_PADDING_TEXELS, TextureRegion, check_region_padding};

/// The four-quadrant texture's colours, in image row order from the top left.
///
/// Asymmetric on purpose. A single-colour texture satisfies every assertion
/// about placement while being upside down, mirrored or transposed. The
/// bottom-right value is a mid-tone rather than a fourth primary for a second
/// reason, carried here from `offscreen.rs`: 0 and 255 survive a wrong texture
/// format unharmed, while 128 would move by roughly sixty counts. **That
/// quadrant is what catches a colour-space mistake; the other three catch
/// orientation.**
pub const QUADRANT_TOP_LEFT: [u8; 4] = [255, 0, 0, 255];
/// The top-right quadrant.
pub const QUADRANT_TOP_RIGHT: [u8; 4] = [0, 255, 0, 255];
/// The bottom-left quadrant.
pub const QUADRANT_BOTTOM_LEFT: [u8; 4] = [0, 0, 255, 255];
/// The bottom-right quadrant. Deliberately not a primary, so a channel swap
/// shows.
pub const QUADRANT_BOTTOM_RIGHT: [u8; 4] = [128, 64, 32, 255];

/// A `size` x `size` texture split into four differently coloured quadrants —
/// the fixture of M1 and M3.5, as raw RGBA8 rows from the top left.
///
/// # Panics
///
/// If `size * size * 4` overflows `usize`.
#[must_use]
pub fn quadrant_rgba(size: u32) -> Vec<u8> {
    let half = size / 2;
    let mut rgba = Vec::with_capacity(size as usize * size as usize * 4);
    for y in 0..size {
        for x in 0..size {
            rgba.extend_from_slice(&match (x < half, y < half) {
                (true, true) => QUADRANT_TOP_LEFT,
                (false, true) => QUADRANT_TOP_RIGHT,
                (true, false) => QUADRANT_BOTTOM_LEFT,
                (false, false) => QUADRANT_BOTTOM_RIGHT,
            });
        }
    }
    rgba
}

/// [`quadrant_rgba`] as a [`Pixels`].
///
/// Not reachable from `narvo-render2d`'s own `#[cfg(test)]` modules — see this
/// module's documentation. Those call [`quadrant_rgba`].
///
/// # Panics
///
/// If `size` is zero or above `OffscreenTarget::MAX_DIMENSION`.
#[must_use]
pub fn quadrant_texture(size: u32) -> Pixels {
    Pixels::from_rgba8(size, size, quadrant_rgba(size))
        .expect("the generated buffer matches its dimensions")
}

/// The atlas's eight colours: the upper half then the lower half of each
/// quadrant.
///
/// The half-and-half split gives each quadrant an interior edge, so a region
/// drawn upside down is visible **inside a single sprite** rather than only by
/// comparing two.
pub const TOP_LEFT_UPPER: [u8; 4] = [255, 0, 0, 255];
/// The top-left quadrant's lower half.
pub const TOP_LEFT_LOWER: [u8; 4] = [128, 0, 0, 255];
/// The top-right quadrant's upper half.
pub const TOP_RIGHT_UPPER: [u8; 4] = [0, 255, 0, 255];
/// The top-right quadrant's lower half.
pub const TOP_RIGHT_LOWER: [u8; 4] = [0, 128, 0, 255];
/// The bottom-left quadrant's upper half.
pub const BOTTOM_LEFT_UPPER: [u8; 4] = [0, 0, 255, 255];
/// The bottom-left quadrant's lower half.
pub const BOTTOM_LEFT_LOWER: [u8; 4] = [0, 0, 128, 255];
/// The bottom-right quadrant's upper half. Sampled by no sprite in any scene
/// that uses this atlas, which is what makes it a detector.
pub const BOTTOM_RIGHT_UPPER: [u8; 4] = [255, 255, 0, 255];
/// The bottom-right quadrant's lower half.
pub const BOTTOM_RIGHT_LOWER: [u8; 4] = [128, 128, 0, 255];

/// How an atlas of four 8 x 8 content regions is laid out: with a border of
/// duplicated edge texels around each region, or without one.
///
/// One type for both, because they are one generator with one number changed.
/// Before M3.22 they were separate hand-written fixtures, which is how five of
/// them ended up carrying M3.21's note that they were the shape M3.9 and M3.11
/// *used* — a sentence that only stayed true because M3.21 corrected its tense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasLayout {
    /// Texels of duplicated border around each content region.
    pub border: u32,
}

impl AtlasLayout {
    /// One content region's edge, in texels. Square, so one number.
    pub const REGION_TEXELS: u32 = 8;

    /// No border: the 16 x 16 shape M3.9, M3.11 and M3.12 **used**, before D13.
    ///
    /// No blessed scene's fixture is this shape any more — all three atlas
    /// scenes are [`PADDED`](Self::PADDED). Its six callers are the margin and
    /// motion measurement files, and for them it is still the right choice: a
    /// fixture that is a **measurement baseline** rather than a re-derivation of
    /// a blessed scene needs to stay put. `linear_motion.rs` compares one
    /// against a padded one and needs both.
    pub const UNPADDED: Self = Self { border: 0 };

    /// A one-texel border, as D13 decided and M3.17 measured: 20 x 20.
    pub const PADDED: Self = Self {
        border: REGION_PADDING_TEXELS,
    };

    /// One cell: a content region plus its border on both sides.
    #[must_use]
    pub const fn cell(self) -> u32 {
        Self::REGION_TEXELS + 2 * self.border
    }

    /// The atlas's edge, in texels. Two cells across, two down.
    #[must_use]
    pub const fn edge(self) -> u32 {
        2 * self.cell()
    }

    /// Where content texel `(x, y)` of the cell at `(cell_x, cell_y)` sits in
    /// the atlas.
    ///
    /// The one place the border enters a derivation written in content
    /// coordinates.
    #[must_use]
    pub const fn content_texel(self, cell_x: u32, cell_y: u32, x: u32, y: u32) -> (u32, u32) {
        (
            cell_x * self.cell() + self.border + x,
            cell_y * self.cell() + self.border + y,
        )
    }

    /// The atlas as raw RGBA8 rows from the top left.
    ///
    /// The colour is chosen from the *content* coordinate; the clamp is what
    /// duplicates a border texel from the content texel nearest it.
    #[must_use]
    pub fn rgba(self) -> Vec<u8> {
        let edge = self.edge();
        let cell = self.cell();
        let quarter = Self::REGION_TEXELS / 2;
        let last = cell - self.border - 1;
        let mut rgba = Vec::with_capacity(edge as usize * edge as usize * 4);

        for y in 0..edge {
            for x in 0..edge {
                let (cell_x, cell_y) = (x / cell, y / cell);
                let inside_y = (y % cell).clamp(self.border, last) - self.border;
                let upper = inside_y < quarter;
                rgba.extend_from_slice(&match (cell_x == 0, cell_y == 0, upper) {
                    (true, true, true) => TOP_LEFT_UPPER,
                    (true, true, false) => TOP_LEFT_LOWER,
                    (false, true, true) => TOP_RIGHT_UPPER,
                    (false, true, false) => TOP_RIGHT_LOWER,
                    (true, false, true) => BOTTOM_LEFT_UPPER,
                    (true, false, false) => BOTTOM_LEFT_LOWER,
                    (false, false, true) => BOTTOM_RIGHT_UPPER,
                    (false, false, false) => BOTTOM_RIGHT_LOWER,
                });
            }
        }
        rgba
    }

    /// [`rgba`](Self::rgba) as a [`Pixels`].
    ///
    /// # Panics
    ///
    /// Never for either constant; the buffer matches the dimensions by
    /// construction.
    #[must_use]
    pub fn atlas(self) -> Pixels {
        Pixels::from_rgba8(self.edge(), self.edge(), self.rgba())
            .expect("the generated buffer matches its dimensions")
    }

    /// The content region of the cell at `(cell_x, cell_y)`.
    ///
    /// Points at the **content**, never at the border: padding surrounds a
    /// region's coordinates rather than moving them.
    #[must_use]
    pub fn region(self, cell_x: u32, cell_y: u32, texture: &Pixels) -> TextureRegion {
        let (left, top) = self.content_texel(cell_x, cell_y, 0, 0);
        TextureRegion::from_texels(left, top, Self::REGION_TEXELS, Self::REGION_TEXELS, texture)
    }

    /// Asserts that every one of the atlas's four regions carries the border
    /// this layout claims.
    ///
    /// Calls [`narvo_render2d::check_region_padding`] rather than restating
    /// it. All four cells, including the one no scene draws — an unused
    /// region's border is precisely the one nobody would notice.
    ///
    /// **What it can and cannot catch.** This atlas's colour does not vary with
    /// x inside a cell, so every left- and right-border texel equals its source
    /// whatever the generator does; what this pins is the top and bottom border
    /// rows. The checker's own coverage of all eight sides is in
    /// `narvo-render2d`'s unit tests, on a fixture that varies per texel.
    ///
    /// # Panics
    ///
    /// If any border texel is not a copy of the content texel nearest to it,
    /// naming the cell, the texel, the edge and both colours.
    pub fn assert_every_region_is_padded(self, texture: &Pixels) {
        for cell_y in 0..2 {
            for cell_x in 0..2 {
                let (left, top) = self.content_texel(cell_x, cell_y, 0, 0);
                check_region_padding(
                    left,
                    top,
                    Self::REGION_TEXELS,
                    Self::REGION_TEXELS,
                    self.border,
                    texture,
                )
                .unwrap_or_else(|defect| {
                    panic!("the cell at ({cell_x}, {cell_y}) is not padded: {defect}")
                });
            }
        }
    }

    /// The same atlas with one border texel replaced, for a guard to catch.
    ///
    /// The committed form of M3.21's red demonstration, shared so that the
    /// three scenes using this atlas do not each carry their own copy of it.
    /// The replaced texel is `(4, 0)` — the top border of the top-left cell,
    /// which must copy `(4, 1)`.
    ///
    /// # Panics
    ///
    /// If the layout has no border to break.
    #[must_use]
    pub fn atlas_with_a_broken_border(self) -> Pixels {
        assert!(self.border > 0, "an unpadded atlas has no border to break");
        let mut rgba = self.rgba();
        // Row 0, column 4. Written out rather than as `row * edge + column`,
        // because row 0 makes the multiplication a constant zero and clippy is
        // right to say so.
        let at = 4 * 4;
        rgba[at..at + 4].copy_from_slice(&[0, 0, 0, 255]);
        Pixels::from_rgba8(self.edge(), self.edge(), rgba)
            .expect("the generated buffer matches its dimensions")
    }
}

#[cfg(test)]
mod tests {
    use super::{AtlasLayout, quadrant_rgba, quadrant_texture};

    #[test]
    fn the_two_layouts_are_the_shapes_the_repository_uses() {
        assert_eq!(AtlasLayout::UNPADDED.edge(), 16);
        assert_eq!(AtlasLayout::PADDED.edge(), 20);
        assert_eq!(AtlasLayout::PADDED.cell(), 10);
        // The content origins the three padded scenes point at.
        assert_eq!(AtlasLayout::PADDED.content_texel(0, 0, 0, 0), (1, 1));
        assert_eq!(AtlasLayout::PADDED.content_texel(1, 0, 0, 0), (11, 1));
        assert_eq!(AtlasLayout::PADDED.content_texel(0, 1, 0, 0), (1, 11));
        // And the unpadded ones, which are the same regions without the shift.
        assert_eq!(AtlasLayout::UNPADDED.content_texel(1, 0, 0, 0), (8, 0));
    }

    /// The padded atlas's content is the unpadded atlas, texel for texel.
    ///
    /// This is the whole-texel argument as an assertion rather than a
    /// paragraph: if the border were doing anything but surrounding the
    /// content, the two would disagree somewhere.
    #[test]
    fn padding_surrounds_the_content_without_changing_it() {
        let plain = AtlasLayout::UNPADDED.atlas();
        let padded = AtlasLayout::PADDED.atlas();

        for cell_y in 0..2 {
            for cell_x in 0..2 {
                for y in 0..AtlasLayout::REGION_TEXELS {
                    for x in 0..AtlasLayout::REGION_TEXELS {
                        let (px, py) = AtlasLayout::UNPADDED.content_texel(cell_x, cell_y, x, y);
                        let (qx, qy) = AtlasLayout::PADDED.content_texel(cell_x, cell_y, x, y);
                        assert_eq!(
                            plain.pixel(px, py),
                            padded.pixel(qx, qy),
                            "content texel ({x}, {y}) of cell ({cell_x}, {cell_y})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_padded_atlas_satisfies_the_renderer_s_own_padding_rule() {
        AtlasLayout::PADDED.assert_every_region_is_padded(&AtlasLayout::PADDED.atlas());
    }

    #[test]
    #[should_panic(
        expected = "border texel (4, 0) on the region's top edge must copy content \
                               texel (4, 1): expected [255, 0, 0, 255], found [0, 0, 0, 255]"
    )]
    fn a_broken_border_is_caught_by_that_rule() {
        let broken = AtlasLayout::PADDED.atlas_with_a_broken_border();
        AtlasLayout::PADDED.assert_every_region_is_padded(&broken);
    }

    #[test]
    fn the_quadrant_texture_is_asymmetric_on_both_axes() {
        // Pairwise distinctness first: without it every corner assertion below
        // holds for a single flat colour, which is the fixture's whole point.
        let corners = [
            super::QUADRANT_TOP_LEFT,
            super::QUADRANT_TOP_RIGHT,
            super::QUADRANT_BOTTOM_LEFT,
            super::QUADRANT_BOTTOM_RIGHT,
        ];
        for (i, left) in corners.iter().enumerate() {
            for right in &corners[i + 1..] {
                assert_ne!(left, right, "two quadrants share a colour: {corners:?}");
            }
        }

        let raw = quadrant_rgba(8);
        assert_eq!(raw.len(), 8 * 8 * 4);
        let texture = quadrant_texture(8);
        assert_eq!(texture.pixel(0, 0), Some(super::QUADRANT_TOP_LEFT));
        assert_eq!(texture.pixel(7, 0), Some(super::QUADRANT_TOP_RIGHT));
        assert_eq!(texture.pixel(0, 7), Some(super::QUADRANT_BOTTOM_LEFT));
        assert_eq!(texture.pixel(7, 7), Some(super::QUADRANT_BOTTOM_RIGHT));
    }
}
