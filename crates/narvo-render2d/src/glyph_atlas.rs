//! A padded glyph atlas, generated rather than committed.
//!
//! D10 (`ProjektPlan.md` §11) decided `ab_glyph` and decided the architecture
//! with it: **the atlas is generated in the test, and a committed SHA-256
//! anchors it.** Probe M3.33 is what makes that legal — it measured both
//! candidates producing byte-identical output across Windows and WSL at three
//! sizes and two optimisation levels, `ab_glyph` down to the raw `f32`
//! coverage. Generation being reproducible is what turns the generator into the
//! specification.
//!
//! Scope is D10's and no wider: ASCII 32..=126, 16 px and 32 px, advance and
//! bearing. **No shaping, no kerning, no hinting**, no layout and no rendering.
//!
//! # The border width, derived rather than assumed
//!
//! D17 fixed the uv interpolation qualifier at `center` and left an obligation
//! for D10, in its own words: *"`center` verschärft die Randarithmetik um die
//! halbe Pixel-Extrapolation (bei 4 px/Texel 0,625 statt 0,5 Texel — ein Texel
//! trägt beides; **Rechnung des Chats, als Rechnung markiert**)"*. The marker is
//! part of the quotation and is kept: D17's 0.625 is a calculation, not a
//! measurement, and so is everything below.
//!
//! **The scale is a premise, not a repo fact.** That figure is for 4 px per
//! texel; this module works the case out for **1:1**, one texel per pixel,
//! because that is what the M3.34 task states text runs at. Nothing in the
//! repository fixes it — no layout exists yet, and D10 fixes the rasterisation
//! sizes (16 px, 32 px) rather than the scale a glyph is later drawn at. If
//! M3.35 draws text at some other scale, the case below is the one to redo, and
//! the sizes where it stops holding are named at the end.
//!
//! ## The setup
//!
//! Texel coordinates, a region's content occupying `[0, w]` with texel centres
//! at `0.5, 1.5, … w-0.5`. A `Linear` fetch at coordinate `t` reads the two
//! texels `n = floor(t - 0.5)` and `n + 1`, weighting the second by
//! `f = (t - 0.5) - n` and the first by `1 - f`. **The index is what a border
//! has to cover**, and it is derived here rather than a distance, because a
//! distance has to be converted into an index before it says how many texels to
//! write.
//!
//! How far outside the content a sample can sit is the `center` extrapolation:
//! a pixel is shaded from its centre, and at a partly covered edge that centre
//! can lie up to **half a pixel** outside the quad. That bound is per axis and
//! is the bound for an **axis-aligned** edge; a glyph quad is axis-aligned, and
//! D17 carries rotation separately as an open front ("Rotation (beide
//! Qualifizierer ungemessen)"). In texels the extrapolation is
//! `0.5 px × (texels per px)` — `0.125` at 4 px/texel, and **`0.5` at 1:1**.
//!
//! ## The two edges, worked separately
//!
//! They are not symmetric, and doing only one would miss it.
//!
//! **Left.** The extreme sample is `t = -0.5`. Then `t - 0.5 = -1.0`, so
//! `n = -1` and `f = 0`: the fetch returns `1.0 × texel[-1] + 0 × texel[0]`.
//! Index `-1` is the first border texel and it carries the **whole** weight, so
//! it has to be right — and it is, being a copy of `texel[0]`.
//!
//! **Right.** The extreme sample is `t = w + 0.5`. Then `t - 0.5 = w`, so
//! `n = w` and `f = 0`: the fetch returns `1.0 × texel[w] + 0 × texel[w+1]`.
//! Index `w` is the first border texel on that side, a copy of `texel[w-1]`.
//! **Index `w+1` lies outside a one-texel border and is still read**, at weight
//! exactly zero — so what it holds cannot reach the result. One texel suffices
//! on this edge because of the weight, not because the index is in range.
//!
//! So [`GLYPH_PADDING_TEXELS`] is **1**, and both extremes return the edge
//! value, which is what padding is for.
//!
//! ## Where it stops holding
//!
//! At 1:1 the extrapolation is exactly `0.5` texel and both `floor`s land
//! exactly on their boundary, so this is the last scale a single texel carries.
//! Writing `k` for texels per pixel, the extreme left sample is `t = -0.5k` and
//! `n = floor(-0.5k - 0.5)`, so the border a left edge needs is `-n`:
//!
//! | `k` (texels per px) | `n` | border needed |
//! | --- | --- | --- |
//! | `k ≤ 1` (magnification, and 1:1) | `-1` | **1** |
//! | `1 < k ≤ 3` | `-2` | 2 |
//! | `3 < k ≤ 5` | `-3` | 3 |
//!
//! **It is not "minification needs two"** — it needs `ceil((k+1)/2)`, and two is
//! only the first step. Minification is out of scope here and D17 carries it as
//! an open front of its own ("Minification … dieselbe Front, an der `Linear`
//! Mipmaps braucht"), where mipmaps change the question rather than the border.
//! Magnification is safe in the other direction: more pixels per texel shrinks
//! the extrapolation towards zero and never past `n = -1`.
//!
//! ## What is calculated and what is measured
//!
//! Everything above is **calculation**, marked as D17 marks its own. What is
//! *measured* is a different and narrower claim: that the atlas this module
//! generates satisfies
//! [`check_region_padding`](crate::check_region_padding) — that its
//! border texels really are copies of the content they sit against — and that is
//! checked for all ninety-four outlined glyphs at both sizes rather than argued.

use crate::{Pixels, TextureRegion};
use serde::{Deserialize, Serialize};

/// First code point in the set, inclusive.
pub const GLYPH_FIRST: u8 = 32;

/// Last code point in the set, inclusive.
pub const GLYPH_LAST: u8 = 126;

/// How many glyphs the set holds.
///
/// Ninety-five. A constant rather than an arithmetic expression at the call
/// site, because the anchor test counts against it *before* it compares a hash:
/// a hash over the wrong number of glyphs is a wrong answer that looks like a
/// wrong answer to a different question.
pub const GLYPH_COUNT: usize = (GLYPH_LAST - GLYPH_FIRST + 1) as usize;

/// The border of duplicated edge texels around every glyph's content.
///
/// One. Derived in this module's own documentation from D17's arithmetic at
/// 1:1 rather than inherited: the influence of a sample reaches 1.0 texel past
/// the content edge at that scale, but the furthest texel *index* it reads is
/// one, and that is what a border has to cover.
///
/// It agrees numerically with this crate's own
/// [`REGION_PADDING_TEXELS`](crate::REGION_PADDING_TEXELS), and it is
/// a separate constant rather than a re-export because the two are derived from
/// different arguments: that one from M3.17's measurement on sprite atlases,
/// this one from D17's arithmetic at text's own scale. Two derivations that
/// agree are evidence; one constant used twice would hide that they were ever
/// asked separately.
pub const GLYPH_PADDING_TEXELS: u32 = 1;

/// Width of every generated atlas, in texels.
///
/// Fixed rather than fitted, so that the packing is a function of the glyph
/// sizes alone and the anchor cannot move because a search found a different
/// arrangement. 256 leaves room for the widest 32 px glyph plus its border many
/// times over; the height follows from the shelves.
const ATLAS_WIDTH: u32 = 256;

/// Where one glyph's **content** sits in the atlas, excluding its border.
///
/// Excluding, deliberately: this is the rectangle a sprite would sample, and
/// the border exists to be read *into* by a filter rather than to be drawn. It
/// is also the rectangle
/// [`check_region_padding`](crate::check_region_padding) takes, so
/// the same numbers are checked and used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlyphRegion {
    /// Leftmost content texel.
    pub left: u32,
    /// Topmost content texel.
    pub top: u32,
    /// Content width in texels, never zero.
    pub width: u32,
    /// Content height in texels, never zero.
    pub height: u32,
}

impl GlyphRegion {
    /// The renderer's own region type for this content rectangle.
    ///
    /// Takes the atlas so the conversion is against the texture the region
    /// actually lives in; `TextureRegion::from_texels` needs its dimensions.
    #[must_use]
    pub fn to_texture_region(self, atlas: &Pixels) -> TextureRegion {
        TextureRegion::from_texels(self.left, self.top, self.width, self.height, atlas)
    }
}

/// One glyph's place in the atlas and what layout needs to position it.
///
/// # Empty glyphs
///
/// `region` is `None` for a glyph with no outline. In ASCII 32..=126 that is
/// **the space, and only the space** — the count is asserted rather than
/// assumed, in `only_the_space_has_no_outline`. Such a glyph occupies no atlas
/// texels at all, which is the honest representation: a zero-by-zero rectangle
/// would be a region that
/// [`check_region_padding`](crate::check_region_padding) rejects as
/// [`EmptyRegion`](crate::PaddingDefect::EmptyRegion), and inventing
/// a one-texel transparent block would put a lie in the table.
///
/// It still carries an `advance`, which is the whole of what a space does.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Glyph {
    /// The character this entry is for.
    pub ch: char,
    /// How far the pen moves after drawing it, in pixels.
    pub advance: f32,
    /// Offset from the pen position to the left edge of the content, in pixels.
    pub bearing_x: f32,
    /// Offset from the baseline to the top edge of the content, in pixels.
    ///
    /// Negative above the baseline, which is `ab_glyph`'s own convention and is
    /// kept rather than flipped: ADR-0004 governs image row order, not this,
    /// and a silent sign change between the rasteriser and the table would be
    /// invisible until layout drew everything upside down.
    pub bearing_y: f32,
    /// Where the content sits, or `None` for a glyph with no outline.
    pub region: Option<GlyphRegion>,
}

/// A generated atlas and its table.
#[derive(Debug)]
pub struct GlyphAtlas {
    pixels: Pixels,
    glyphs: Vec<Glyph>,
    size_px: f32,
}

impl GlyphAtlas {
    /// The atlas texture.
    #[must_use]
    pub fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    /// Every glyph, in ascending code-point order.
    #[must_use]
    pub fn glyphs(&self) -> &[Glyph] {
        &self.glyphs
    }

    /// The pixel size this atlas was rasterised at.
    #[must_use]
    pub fn size_px(&self) -> f32 {
        self.size_px
    }

    /// The entry for `ch`, if the set holds it.
    #[must_use]
    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        self.glyphs.iter().find(|glyph| glyph.ch == ch)
    }

    /// The bytes the SHA-256 anchor is taken over.
    ///
    /// **Pixels and table together**, in one fixed order, because either alone
    /// would miss half of what generation produces: a changed advance or bearing
    /// moves no pixel and would leave a pixels-only hash untouched, and a
    /// changed coverage quantisation moves no region and would leave a
    /// table-only hash untouched.
    ///
    /// Floats go in as `to_bits`, not as text. A formatted float would put the
    /// standard library's formatting in the anchor's blast radius, which is
    /// precisely the kind of dependence ADR-0008 warns a committed value
    /// against.
    #[must_use]
    pub fn anchor_bytes(&self) -> Vec<u8> {
        let mut blob = Vec::new();

        blob.extend_from_slice(&self.pixels.width().to_le_bytes());
        blob.extend_from_slice(&self.pixels.height().to_le_bytes());
        blob.extend_from_slice(self.pixels.rgba());

        for glyph in &self.glyphs {
            blob.extend_from_slice(&(glyph.ch as u32).to_le_bytes());
            blob.extend_from_slice(&glyph.advance.to_bits().to_le_bytes());
            blob.extend_from_slice(&glyph.bearing_x.to_bits().to_le_bytes());
            blob.extend_from_slice(&glyph.bearing_y.to_bits().to_le_bytes());

            match glyph.region {
                Some(region) => {
                    blob.push(1);
                    blob.extend_from_slice(&region.left.to_le_bytes());
                    blob.extend_from_slice(&region.top.to_le_bytes());
                    blob.extend_from_slice(&region.width.to_le_bytes());
                    blob.extend_from_slice(&region.height.to_le_bytes());
                }
                // A tag rather than four zeros: four zeros is also what a region
                // at the origin with no extent would write, and the two must not
                // hash alike.
                None => blob.push(0),
            }
        }

        blob
    }
}

/// The font this crate rasterises, embedded at compile time.
///
/// `include_bytes!` rather than a path read at runtime: a test that depends on
/// the working directory fails differently on a developer's machine and in CI,
/// and the file is 335 KiB of committed test asset either way. Its licence and
/// attribution sit beside it in `assets/DejaVuSansMono-LICENSE.txt`, which the
/// Bitstream Vera licence requires to travel with the font.
pub const FONT: &[u8] = include_bytes!("../assets/DejaVuSansMono.ttf");

/// Rasterises ASCII 32..=126 at `size_px` into a padded atlas.
///
/// # Packing
///
/// Shelves, in ascending code-point order: each glyph is placed to the right of
/// the last on the current shelf, and a glyph that does not fit starts a new one
/// whose height is its own. Deterministic by construction — the order is the
/// code points, and no measurement of the result feeds back into the placement.
///
/// # Panics
///
/// If the embedded font cannot be parsed, or if a glyph is wider than the
/// atlas. Both are programming errors in this crate rather than conditions a
/// caller can act on: the font is committed beside the code and the width is a
/// constant above.
#[must_use]
pub fn rasterize(size_px: f32) -> GlyphAtlas {
    use ab_glyph::{Font as _, ScaleFont as _};

    let font = ab_glyph::FontRef::try_from_slice(FONT).expect("the committed font parses");
    let scaled = font.as_scaled(size_px);

    // First pass: rasterise every glyph and note how big it is. Nothing is
    // placed yet, because the atlas height is not known until the shelves are.
    struct Raster {
        ch: char,
        advance: f32,
        bearing_x: f32,
        bearing_y: f32,
        width: u32,
        height: u32,
        coverage: Vec<u8>,
    }

    let mut rasters = Vec::with_capacity(GLYPH_COUNT);

    for code in GLYPH_FIRST..=GLYPH_LAST {
        let ch = char::from(code);
        let glyph_id = font.glyph_id(ch);
        let advance = scaled.h_advance(glyph_id);
        let outlined = font.outline_glyph(scaled.scaled_glyph(ch));

        let (bearing_x, bearing_y, width, height, coverage) = match outlined {
            Some(outlined) => {
                let bounds = outlined.px_bounds();
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "px_bounds of a rasterised glyph is non-negative in extent \
                              and far inside u32 at these sizes"
                )]
                let width = bounds.width().ceil() as u32;
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "as above"
                )]
                let height = bounds.height().ceil() as u32;

                let mut coverage = vec![0_u8; (width * height) as usize];
                outlined.draw(|x, y, c| {
                    if x < width && y < height {
                        // The same quantisation probe M3.33 measured, so its
                        // evidence is evidence about this code and not about a
                        // near relative of it.
                        // `as u8` saturates rather than wrapping, which matters
                        // here: `ab_glyph`'s callback documents "a value of
                        // `1.0` or greater means fully covered", so `c` is not
                        // bounded above by one and the product can exceed 255.
                        // Saturation turns that into 255, which is the right
                        // answer for "fully covered".
                        #[expect(
                            clippy::cast_sign_loss,
                            clippy::cast_possible_truncation,
                            reason = "coverage is non-negative and `as u8` saturates, so an \
                                      over-one coverage becomes 255 rather than wrapping"
                        )]
                        let value = (c * 255.0 + 0.5) as u8;
                        coverage[(y * width + x) as usize] = value;
                    }
                });

                (bounds.min.x, bounds.min.y, width, height, coverage)
            }
            None => (0.0, 0.0, 0, 0, Vec::new()),
        };

        rasters.push(Raster {
            ch,
            advance,
            bearing_x,
            bearing_y,
            width,
            height,
            coverage,
        });
    }

    // Second pass: shelve them, border included in the footprint.
    let border = GLYPH_PADDING_TEXELS;
    let mut placements: Vec<Option<GlyphRegion>> = Vec::with_capacity(GLYPH_COUNT);
    let (mut pen_x, mut shelf_top, mut shelf_height) = (0_u32, 0_u32, 0_u32);

    for raster in &rasters {
        if raster.width == 0 || raster.height == 0 {
            placements.push(None);
            continue;
        }

        let footprint_w = raster.width + 2 * border;
        let footprint_h = raster.height + 2 * border;
        assert!(
            footprint_w <= ATLAS_WIDTH,
            "glyph {:?} is {footprint_w} texels wide with its border, wider than the atlas",
            raster.ch
        );

        if pen_x + footprint_w > ATLAS_WIDTH {
            shelf_top += shelf_height;
            shelf_height = 0;
            pen_x = 0;
        }

        placements.push(Some(GlyphRegion {
            left: pen_x + border,
            top: shelf_top + border,
            width: raster.width,
            height: raster.height,
        }));

        pen_x += footprint_w;
        shelf_height = shelf_height.max(footprint_h);
    }

    let height = shelf_top + shelf_height;
    let mut rgba = vec![0_u8; (ATLAS_WIDTH * height * 4) as usize];

    // Third pass: write the coverage, then the border around it.
    for (raster, placement) in rasters.iter().zip(&placements) {
        let Some(region) = placement else { continue };

        for y in 0..raster.height {
            for x in 0..raster.width {
                let value = raster.coverage[(y * raster.width + x) as usize];
                write_texel(&mut rgba, height, region.left + x, region.top + y, value);
            }
        }

        // The border, as duplicated edge texels. Corners included, which is what
        // `check_region_padding` checks separately as `RegionEdge`'s four corner
        // cases - an edge-only loop is the mistake it exists to catch.
        for step in 1..=border {
            for x in 0..raster.width {
                let top = raster.coverage[x as usize];
                let bottom = raster.coverage[((raster.height - 1) * raster.width + x) as usize];
                write_texel(&mut rgba, height, region.left + x, region.top - step, top);
                write_texel(
                    &mut rgba,
                    height,
                    region.left + x,
                    region.top + raster.height - 1 + step,
                    bottom,
                );
            }
            for y in 0..raster.height {
                let left = raster.coverage[(y * raster.width) as usize];
                let right = raster.coverage[(y * raster.width + raster.width - 1) as usize];
                write_texel(&mut rgba, height, region.left - step, region.top + y, left);
                write_texel(
                    &mut rgba,
                    height,
                    region.left + raster.width - 1 + step,
                    region.top + y,
                    right,
                );
            }
        }

        for down in 1..=border {
            for across in 1..=border {
                let last_x = raster.width - 1;
                let last_y = raster.height - 1;
                let corners = [
                    (region.left - across, region.top - down, 0, 0),
                    (region.left + last_x + across, region.top - down, last_x, 0),
                    (region.left - across, region.top + last_y + down, 0, last_y),
                    (
                        region.left + last_x + across,
                        region.top + last_y + down,
                        last_x,
                        last_y,
                    ),
                ];
                for (x, y, source_x, source_y) in corners {
                    let value = raster.coverage[(source_y * raster.width + source_x) as usize];
                    write_texel(&mut rgba, height, x, y, value);
                }
            }
        }
    }

    let glyphs = rasters
        .iter()
        .zip(placements)
        .map(|(raster, region)| Glyph {
            ch: raster.ch,
            advance: raster.advance,
            bearing_x: raster.bearing_x,
            bearing_y: raster.bearing_y,
            region,
        })
        .collect();

    GlyphAtlas {
        pixels: Pixels::from_rgba8(ATLAS_WIDTH, height, rgba).expect("the buffer matches its size"),
        glyphs,
        size_px,
    }
}

/// Writes one coverage value as a premultiplied-white texel.
///
/// All four channels carry the coverage. That is white with the coverage as its
/// alpha, **already premultiplied** - which is what makes a `Linear` blend of
/// two neighbouring texels correct rather than dark-fringed: interpolating
/// straight-alpha colour towards a transparent-black neighbour drags the colour
/// down as well as the alpha, and premultiplied does not.
fn write_texel(rgba: &mut [u8], height: u32, x: u32, y: u32, value: u8) {
    debug_assert!(x < ATLAS_WIDTH && y < height, "texel outside the atlas");
    let offset = ((y * ATLAS_WIDTH + x) * 4) as usize;
    rgba[offset..offset + 4].copy_from_slice(&[value, value, value, value]);
}

#[cfg(test)]
mod tests {
    use super::{GLYPH_COUNT, GLYPH_FIRST, GLYPH_LAST, GLYPH_PADDING_TEXELS, Glyph, rasterize};
    use crate::check_region_padding;
    use narvo_testkit::sha256::sha256_hex;

    /// The two sizes D10 fixed. Both are anchored; nothing else is generated.
    const SIZES: [f32; 2] = [16.0, 32.0];

    /// The committed anchors, one per (font, size, set).
    ///
    /// **Font** DejaVu Sans Mono, the file in `assets/` whose own SHA-256 is in
    /// `assets/DejaVuSansMono-LICENSE.txt`. **Set** ASCII 32..=126.
    /// **Rasteriser** `ab_glyph` 0.2.32. `Cargo.toml` states `"0.2.32"`, which
    /// is a caret requirement rather than a pin — what holds the version still
    /// is `Cargo.lock`, and CI builds `--locked`. A `cargo update` inside
    /// `0.2.x` is therefore exactly the deliberate re-anchoring this anchor
    /// exists to force.
    ///
    /// # What moves these, and why that is the point
    ///
    /// An `ab_glyph` release that changes a coverage value by one, a font file
    /// swapped for another cut, a change to the packing, the padding or the
    /// quantisation. **All of those should move it**, and an update is meant to
    /// be a deliberate re-anchoring with a human looking at the result rather
    /// than a number quietly following the code.
    ///
    /// # The tension with ADR-0008, named rather than glossed
    ///
    /// ADR-0008 says "No hash literal is ever committed to this repository" and
    /// gives a test for a new literal: *what would have to change for this value
    /// to move? If the answer includes a dependency version … it is the
    /// forbidden kind.* **By that test this is the forbidden kind** — an
    /// `ab_glyph` bump moves it.
    ///
    /// It is committed anyway because D10 (`ProjektPlan.md` §11, decided
    /// 10.08.2026) decided it, on a reading ADR-0008 does not cover: that ADR
    /// bounds the *state hash*, where a dependency-driven move is noise that
    /// says nothing about simulation correctness. Here the move is the signal.
    /// A glyph atlas is an asset whose bytes are the specification, and an
    /// update that silently changed them is exactly what nothing else in the
    /// repository would notice.
    ///
    /// The two-run comparison ADR-0008 prefers cannot substitute: two runs of
    /// the same build agree by construction, and probe M3.33 already measured
    /// that they agree across platforms. What a two-run comparison cannot do is
    /// notice that the bytes changed between commits.
    const ANCHORS: [(f32, &str); 2] = [
        (
            16.0,
            "edca688da89d64cb53edbe7138ff8ef8678b766469908490e167caeeb6613c4e",
        ),
        (
            32.0,
            "b2aa8edebf4e120fe4b114a732a056422e3ecc4a7dc06e781a6b42c92df17fb6",
        ),
    ];

    /// Writes both atlases under `target/` so a person can look at them.
    ///
    /// Not an assertion and not a reference: `tests/golden/` is untouched by
    /// this crate. It exists because the anchors above are only re-anchored
    /// deliberately, and "deliberately" means somebody looked at the atlas
    /// before pasting a new hash in. Same shape as
    /// `sprite_placement.rs`'s preview.
    #[test]
    fn a_preview_of_each_atlas_is_written_for_the_maintainer_to_look_at() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("glyph-atlas");
        std::fs::create_dir_all(&directory).expect("the preview directory");

        for size in SIZES {
            let atlas = rasterize(size);
            let path = directory.join(format!("dejavu-sans-mono-{size}px.png"));
            atlas
                .pixels()
                .save_png(&path)
                .expect("the preview is written");
            println!(
                "{} px: {}x{}, {} glyphs -> {}",
                size,
                atlas.pixels().width(),
                atlas.pixels().height(),
                atlas.glyphs().len(),
                path.display()
            );
        }
    }

    #[test]
    fn the_atlas_is_premultiplied_white_and_m_has_a_solid_core() {
        // Two claims `write_texel`'s doc makes, and neither is visible in the
        // preview: it renders white-on-transparent, so against a white page a
        // filled glyph and a hollow one look alike.
        //
        // **All four channels equal**, not merely R == A. Premultiplied *red* -
        // `[v, 0, 0, v]` - satisfies R == A at every texel while being the
        // opposite of what the doc claims, so the weaker assertion would pass a
        // generator that had gone wrong in exactly the way worth catching.
        //
        // The name says "M has a solid core" rather than "interiors are fully
        // covered", because that is what the second half checks: one glyph, and
        // the existence of full coverage rather than a property of every
        // interior texel. At 16 px only a couple of M's texels reach 255, which
        // is antialiasing doing its job on a thin stem, so a stronger assertion
        // here would be false rather than better.
        for size in SIZES {
            let atlas = rasterize(size);

            for y in 0..atlas.pixels().height() {
                for x in 0..atlas.pixels().width() {
                    let texel = atlas.pixels().pixel(x, y).expect("inside the atlas");
                    assert!(
                        texel[0] == texel[1] && texel[1] == texel[2] && texel[2] == texel[3],
                        "texel ({x}, {y}) of the {size} px atlas is {texel:?}, \
                         which is not premultiplied white"
                    );
                }
            }

            let region = atlas
                .glyph('M')
                .expect("M is in the set")
                .region
                .expect("M has an outline");
            let full = (region.top..region.top + region.height)
                .flat_map(|y| (region.left..region.left + region.width).map(move |x| (x, y)))
                .filter(|(x, y)| atlas.pixels().pixel(*x, *y).expect("inside the atlas")[3] == 255)
                .count();

            assert!(
                full > 0,
                "M at {size} px has no fully covered texel, so it was rasterised hollow"
            );
        }
    }

    #[test]
    fn a_region_converts_to_the_renderer_s_own_region_type() {
        // `GlyphRegion::to_texture_region` is what M3.35 will hand a sprite, and
        // an accessor with no caller is an accessor nobody has checked. The uv
        // bounds have to land inside the atlas and describe the content block
        // rather than the whole texture.
        let atlas = rasterize(16.0);
        let region = atlas
            .glyph('M')
            .expect("M is in the set")
            .region
            .expect("M has an outline");

        let [u_left, v_top, u_right, v_bottom] =
            region.to_texture_region(atlas.pixels()).uv_bounds();

        assert!(
            0.0 < u_left && u_left < u_right && u_right < 1.0,
            "u bounds {u_left}..{u_right} do not sit inside the atlas"
        );
        assert!(
            0.0 < v_top && v_top < v_bottom && v_bottom < 1.0,
            "v bounds {v_top}..{v_bottom} do not sit inside the atlas"
        );
        assert_eq!(
            atlas.size_px(),
            16.0,
            "the atlas does not report the size it was rasterised at"
        );
    }

    #[test]
    fn the_set_is_ninety_five_glyphs() {
        // Against the code points rather than against `GLYPH_COUNT`'s own
        // definition: `GLYPH_COUNT` *is* `GLYPH_LAST - GLYPH_FIRST + 1`, so
        // asserting that pair would restate itself and could not disagree.
        assert_eq!(GLYPH_COUNT, 95);
        assert_eq!(GLYPH_FIRST, b' ');
        assert_eq!(GLYPH_LAST, b'~');
        assert_eq!((GLYPH_FIRST..=GLYPH_LAST).count(), 95);
    }

    #[test]
    fn every_size_that_is_generated_is_also_anchored() {
        // `SIZES` and `ANCHORS` are two lists, and seven tests iterate the first
        // while only the anchor test iterates the second. Without this, adding a
        // size would generate it, padding-check it and overlap-check it while
        // silently anchoring nothing.
        let anchored: Vec<f32> = ANCHORS.iter().map(|(size, _)| *size).collect();

        assert_eq!(
            anchored,
            SIZES.to_vec(),
            "the generated sizes and the anchored sizes have drifted apart"
        );
    }

    #[test]
    fn every_generated_atlas_matches_its_committed_anchor() {
        // **The count is asserted before the hash**, deliberately. A run that
        // rasterised ninety-four glyphs would produce a hash that differs, and
        // "the hash differs" is the least useful thing the test could say about
        // it. §6.10 and probe M3.33 both record the same lesson: count
        // completeness first, compare second.
        for (size, expected) in ANCHORS {
            let atlas = rasterize(size);

            assert_eq!(
                atlas.glyphs().len(),
                GLYPH_COUNT,
                "the atlas at {size} px holds {} glyphs, not {GLYPH_COUNT}",
                atlas.glyphs().len()
            );
            for (offset, glyph) in atlas.glyphs().iter().enumerate() {
                let expected_char = char::from(GLYPH_FIRST + offset as u8);
                assert_eq!(
                    glyph.ch, expected_char,
                    "glyph {offset} of the {size} px atlas is {:?}, not {expected_char:?}",
                    glyph.ch
                );
            }

            let actual = sha256_hex(&atlas.anchor_bytes());
            assert_eq!(
                actual, expected,
                "the {size} px atlas hashes to {actual}, not to its committed anchor \
                 {expected}. If `ab_glyph`, the font or the generator changed on purpose, \
                 re-anchor deliberately: look at the atlas, then put this value in."
            );
        }
    }

    #[test]
    fn every_glyph_region_passes_the_padding_check() {
        // The declared padding is checked rather than claimed - M3.21's rule,
        // and `check_region_padding`'s own doc names glyph atlases as a caller
        // it was written to expect. A glyph with no outline has no region and
        // is skipped; that it is only the space is a separate assertion below.
        for size in SIZES {
            let atlas = rasterize(size);
            let mut checked = 0;

            for glyph in atlas.glyphs() {
                let Some(region) = glyph.region else { continue };

                check_region_padding(
                    region.left,
                    region.top,
                    region.width,
                    region.height,
                    GLYPH_PADDING_TEXELS,
                    atlas.pixels(),
                )
                .unwrap_or_else(|defect| {
                    panic!(
                        "glyph {:?} at {size} px is not padded as declared: {defect}",
                        glyph.ch
                    )
                });
                checked += 1;
            }

            assert_eq!(
                checked,
                GLYPH_COUNT - 1,
                "at {size} px, {checked} glyphs were padding-checked; \
                 ninety-four have outlines and one - the space - does not"
            );
        }
    }

    #[test]
    fn only_the_space_has_no_outline() {
        // Asserted rather than assumed. If a later font or a later `ab_glyph`
        // gave some other ASCII glyph an empty outline, the padding test's count
        // would move and this says which glyph did it.
        for size in SIZES {
            let atlas = rasterize(size);
            let empty: Vec<char> = atlas
                .glyphs()
                .iter()
                .filter(|glyph| glyph.region.is_none())
                .map(|glyph| glyph.ch)
                .collect();

            assert_eq!(empty, vec![' '], "at {size} px");
        }
    }

    #[test]
    fn the_space_still_advances_the_pen() {
        // The whole of what a space does. A table that dropped it would lay text
        // out with no gaps and nothing about the atlas would look wrong.
        for size in SIZES {
            let atlas = rasterize(size);
            let space = atlas.glyph(' ').expect("the set holds the space");

            assert!(space.region.is_none());
            assert!(
                space.advance > 0.0,
                "the space at {size} px advances by {}, which lays out no gap",
                space.advance
            );
        }
    }

    #[test]
    fn the_region_table_survives_a_ron_round_trip() {
        // ADR-0006's format. The table is data before it is anything else, and
        // this is what makes "RON-capable" a checked property rather than a
        // claim about derives.
        let atlas = rasterize(16.0);
        let table: Vec<Glyph> = atlas.glyphs().to_vec();

        let text = ron::ser::to_string(&table).expect("the table serializes");
        let back: Vec<Glyph> = ron::from_str(&text).expect("the table parses back");

        assert_eq!(back.len(), GLYPH_COUNT);
        assert_eq!(back, table, "the table did not survive its round trip");
    }

    #[test]
    fn a_monospace_font_gives_every_glyph_the_same_advance() {
        // Not a property of atlases in general - a property of *this* font, and
        // the cheapest available check that the advances are being read rather
        // than invented. DejaVu Sans Mono is monospaced, so ninety-five glyphs
        // share one advance; a table of zeros or of bounding-box widths would
        // fail this.
        for size in SIZES {
            let atlas = rasterize(size);
            let first = atlas.glyphs()[0].advance;

            assert!(first > 0.0, "the advance at {size} px is {first}");
            for glyph in atlas.glyphs() {
                assert_eq!(
                    glyph.advance, first,
                    "glyph {:?} advances {} where the monospace font says {first}",
                    glyph.ch, glyph.advance
                );
            }
        }
    }

    #[test]
    fn a_bigger_size_gives_a_bigger_glyph() {
        // The size parameter reaches the rasteriser. Without this, both anchors
        // could be of the same atlas and every other test here would pass.
        let small = rasterize(16.0);
        let large = rasterize(32.0);

        let small_m = small
            .glyph('M')
            .expect("M is in the set")
            .region
            .expect("M has an outline");
        let large_m = large
            .glyph('M')
            .expect("M is in the set")
            .region
            .expect("M has an outline");

        assert!(
            large_m.width > small_m.width && large_m.height > small_m.height,
            "M is {}x{} at 16 px and {}x{} at 32 px",
            small_m.width,
            small_m.height,
            large_m.width,
            large_m.height
        );
    }

    #[test]
    fn regions_do_not_overlap_and_stay_inside_the_atlas() {
        // Packing correctness, which no hash can report as such: a hash notices
        // that two glyphs overlap only by being different from a value nobody
        // has yet computed. Borders are included in the footprint, because two
        // regions whose borders overlap would each pass the padding check and
        // still corrupt one another.
        for size in SIZES {
            let atlas = rasterize(size);
            let border = GLYPH_PADDING_TEXELS;
            let (width, height) = (atlas.pixels().width(), atlas.pixels().height());

            let boxes: Vec<(char, u32, u32, u32, u32)> = atlas
                .glyphs()
                .iter()
                .filter_map(|glyph| {
                    glyph.region.map(|region| {
                        (
                            glyph.ch,
                            region.left - border,
                            region.top - border,
                            region.width + 2 * border,
                            region.height + 2 * border,
                        )
                    })
                })
                .collect();

            for (ch, left, top, box_w, box_h) in &boxes {
                assert!(
                    left + box_w <= width && top + box_h <= height,
                    "glyph {ch:?} at {size} px runs outside the {width}x{height} atlas"
                );
            }

            for (index, (ch, left, top, box_w, box_h)) in boxes.iter().enumerate() {
                for (other_ch, other_left, other_top, other_w, other_h) in &boxes[index + 1..] {
                    let apart = left + box_w <= *other_left
                        || other_left + other_w <= *left
                        || top + box_h <= *other_top
                        || other_top + other_h <= *top;
                    assert!(
                        apart,
                        "at {size} px the footprints of {ch:?} and {other_ch:?} overlap"
                    );
                }
            }
        }
    }
}
