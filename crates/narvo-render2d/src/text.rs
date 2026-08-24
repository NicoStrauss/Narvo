//! Turning a string into a sequence of sprites, through the M3.34 region table.
//!
//! One capability and no more: a **single line**, left to right, positioned by
//! advance and bearing against a baseline. No shaping, no kerning beyond the
//! advance the font supplies, no line breaking, no colour.
//!
//! Two of those limits come from different places and the difference matters.
//! **D10 fixes the character set, the two sizes, that advance is carried, and
//! that shaping, kerning and hinting are out** — verbatim, `ProjektPlan.md`
//! §11: "Scope: ASCII 32–126, 16/32 px (13 px gemessen), Advance ja,
//! Shaping/Kerning/Hinting nein". **Single-line and monochrome are the M3.35
//! task's limits**, not D10's, and they are the ones a later task can widen
//! without reopening a decision.
//!
//! # Where it lives, and why it moved here in M6.6b
//!
//! Until M6.6b this module and [`glyph_atlas`](crate::glyph_atlas) lived in
//! `narvo-testkit`, and the argument for that placement was written out in this
//! header. It ended with the condition under which it would stop holding:
//!
//! > "When text becomes a shipped capability rather than a golden-image subject,
//! > moving it is the right change, and it is a decision with an ADR rather than
//! > a side effect of this task."
//!
//! **That condition occurred.** M6.6b (v1.06) decided the native text path over
//! egui for the debug overlay, which makes text a shipped capability. The move is
//! ADR-0038, and the rejected placement recorded here — "*Considered and
//! rejected: `narvo-render2d`*" — is the one that was taken, for the reason its
//! own counter-argument named: the region table had to come too.
//!
//! What the old header got right and is worth keeping: the table and the layout
//! belong together, because **layout consumes the region table**. They moved
//! together, and neither can be reached from the other across a crate boundary —
//! `narvo-testkit` depends on this crate, so this crate cannot depend on it
//! back except for dev (ADR-0016).
//!
//! What it produces is [`SpriteInstance`], this crate's own type — a placement
//! of five scalars, a texture region, and the sampler wish. **ADR-0015 is
//! untouched**: the renderer still takes a
//! buffer of scalars, no ECS type appears anywhere near it, and this module is
//! one more producer of that buffer rather than a new way into the renderer.
//!
//! Worth stating precisely, because the neighbouring manifest had to be
//! corrected for the same slip: putting `ab_glyph` here is not "a production
//! graph now contains `ab_glyph` where none did". One already did — `winit`
//! reaches it on Linux for Wayland decorations. What changed is that this crate
//! now declares it.
//!
//! # Positions are integers, and that is a decision
//!
//! **Every bearing in the M3.34 atlas is a whole number of pixels** — checked,
//! not assumed: `the_atlas_bearings_are_whole_pixels` covers all 95 at both
//! sizes. Ninety-four of those come from the font; the space has no outline, so
//! `rasterize` writes its bearings as literal zeros and that one is the
//! generator agreeing with itself rather than a measurement. **The advance is not**: DejaVu Sans Mono gives 8.275167 px at 16 px
//! and 16.550335 px at 32 px. So the only thing that can put a glyph off the
//! pixel grid is the accumulated pen.
//!
//! This module therefore does two things at once, and they are not the same
//! thing:
//!
//! - **The pen carries its fraction**, in `f32`, adding each glyph's own
//!   advance. Not *exactly* — `f32` addition is correctly rounded rather than
//!   exact, and seventy-nine additions of this font's advance land **two ulps**
//!   from the number one multiplication gives: `0x4423_6f3d` against
//!   `0x4423_6f3f`, with a representable value between them. (An earlier
//!   version of this sentence said one ulp. It was written to repair a claim of
//!   exactness and put a checkable number in its place, and got the number
//!   wrong.) What accumulating avoids is the error that would actually show:
//!   rounding the pen itself compounds, and eighty characters of a 0.275 px
//!   truncation is twenty-two pixels of drift — a visibly short line.
//! - **Each glyph is placed at the nearest whole pixel**, by rounding the pen at
//!   the moment it is used. `f32::round` is round-half-away-from-zero and is
//!   exactly specified, so two platforms agree.
//!
//! **What happens at a fractional pen position is therefore: it is allowed, it
//! is carried rather than discarded, and it is snapped at the point of use.**
//! The
//! alternative — refusing a fractional pen — would make this font unusable at
//! its own advance, which is not a defensible thing for a layout to do.
//!
//! Snapping is what makes 1:1 `Nearest` an exact copy of the atlas: a quad whose
//! edges land on pixel boundaries samples each texel centre once. That property
//! is what the golden scene's prediction rests on, and it would be lost the
//! moment a glyph landed on a half pixel.
//!
//! # Characters with nothing to draw
//!
//! - **The space** has an advance and no region ([`Glyph::region`] is `None`,
//!   M3.34). It moves the pen and produces no sprite. It is the only character
//!   in the set that behaves this way, which M3.34 asserts rather than assumes.
//! - **Anything outside ASCII 32..=126** — a tab, a newline, `é`, a control
//!   character — is **skipped entirely and advances the pen by nothing**. The
//!   table has no entry for it, so there is no advance to apply, and inventing
//!   one would put a number in the layout that no font supplied.
//!
//!   That is a deliberate limit rather than a general answer. A shipped text
//!   stack substitutes `.notdef` and advances by its width; this one has a
//!   ninety-five-glyph table and a single-line scope, and pretending otherwise
//!   would be the wider capability D10 did not decide. Named here so the next
//!   task inherits a gap rather than a surprise.
//!
//! # What did **not** move, and why
//!
//! `model_image` and `over` stayed in `narvo-testkit`. They are the CPU model a
//! golden scene is predicted against, and a renderer that owned the model its
//! own golden tests are graded by would be checking nothing.
//!
//! ADR-0038 gave a second reason, and **M7.0 removed it**: they depend on
//! `srgb`, which was then in `narvo-testkit`, so moving them would have needed
//! a *normal* dependency from this crate back to testkit — the one direction
//! ADR-0016's cycle does not permit. `srgb` is [`crate::srgb`] now, so that
//! obstacle is gone and the first reason is the whole of what holds them there.
//! It is enough on its own.

use crate::glyph_atlas::{GlyphAtlas, GlyphRegion};
use crate::{Pixels, SpriteFilter, SpriteInstance, SpritePlacement};

/// One glyph, placed on the pixel grid.
///
/// Coordinates are **pixels with y running down**, the framebuffer's own
/// direction (ADR-0004: "row 0 is the top"), and they name the top-left corner
/// of the glyph's *content* — the same rectangle [`GlyphRegion`] describes in
/// the atlas, so a blit is a rectangle copy with no arithmetic in between.
///
/// `i32` rather than `u32`: a glyph can legitimately sit above or left of the
/// canvas, and a layout that could not express that would have to clamp, which
/// is a rendering decision taken in the wrong place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedGlyph {
    /// The character this came from.
    pub ch: char,
    /// Leftmost content pixel.
    pub left: i32,
    /// Topmost content pixel.
    pub top: i32,
    /// Where the content is in the atlas.
    pub region: GlyphRegion,
}

/// Lays `text` out along one line and returns the glyphs that draw.
///
/// `pen_x` is where the first glyph's origin sits and `baseline_y` is the
/// baseline, both in pixels with y running down. A glyph's content top is
/// `baseline_y + bearing_y`, and `bearing_y` is negative above the baseline —
/// `ab_glyph`'s convention, which M3.34 kept rather than flipping.
///
/// Characters that draw nothing produce no entry: the space, and anything
/// outside the table. See the module documentation for what each of those does
/// to the pen.
#[must_use]
pub fn layout_line(
    text: &str,
    atlas: &GlyphAtlas,
    pen_x: f32,
    baseline_y: f32,
) -> Vec<PlacedGlyph> {
    let mut placed = Vec::new();
    let mut pen = pen_x;

    for ch in text.chars() {
        let Some(glyph) = atlas.glyph(ch) else {
            // Outside the table: no entry, and no advance, because the table
            // holds none to apply.
            continue;
        };

        if let Some(region) = glyph.region {
            placed.push(PlacedGlyph {
                ch,
                // Rounded at the point of use; `pen` itself keeps its fraction.
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "a pixel position of a laid-out line is far inside i32"
                )]
                left: (pen + glyph.bearing_x).round() as i32,
                #[expect(clippy::cast_possible_truncation, reason = "as above")]
                top: (baseline_y + glyph.bearing_y).round() as i32,
                region,
            });
        }

        pen += glyph.advance;
    }

    placed
}

/// Where the pen ends up after laying `text` out from zero.
///
/// The accumulation with its fraction kept — this is the number a caller needs
/// to place a second line's worth of text after the first, and rounding it would
/// be the drift the module documentation refuses. Not an *exact* sum: `f32`
/// addition rounds, and the module doc gives the size of it.
#[must_use]
pub fn advance_of(text: &str, atlas: &GlyphAtlas) -> f32 {
    text.chars()
        .filter_map(|ch| atlas.glyph(ch))
        .map(|glyph| glyph.advance)
        .sum()
}

/// The sprites that draw `placed` into a `width` × `height` canvas.
///
/// `texture` is whatever the regions point into — one atlas, or several stacked
/// into one image, which is what the golden scene does because a draw call binds
/// exactly one texture. It is read for its dimensions, to normalise the uv.
///
/// Converts pixels-with-y-down into the renderer's world units, which run with
/// y **up** and put the origin at the centre of the target. The conversion is
/// the whole of what this function does, and it is written once here rather than
/// at each call site because getting it wrong produces an upside-down line that
/// still looks like text.
///
/// A glyph's content occupies pixel rows `top .. top + height`, so its quad's
/// top edge sits at world `height/2 - top` and its centre half a glyph lower.
/// The sprite is `Nearest`: at 1:1 on whole pixels the quad's edges land on
/// pixel boundaries and each texel is sampled once at its centre, which makes
/// the drawn image a copy of the atlas rather than a resampling of it.
#[must_use]
pub fn sprites_for(
    placed: &[PlacedGlyph],
    texture: &Pixels,
    width: u32,
    height: u32,
) -> Vec<SpriteInstance> {
    #[expect(
        clippy::cast_precision_loss,
        reason = "canvas dimensions of a golden scene are small integers"
    )]
    let (half_width, half_height) = (width as f32 / 2.0, height as f32 / 2.0);

    placed
        .iter()
        .map(|glyph| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "glyph extents and positions are small integers"
            )]
            let (w, h) = (glyph.region.width as f32, glyph.region.height as f32);
            #[expect(clippy::cast_precision_loss, reason = "as above")]
            let (left, top) = (glyph.left as f32, glyph.top as f32);

            let placement = SpritePlacement {
                x: left + w / 2.0 - half_width,
                y: half_height - top - h / 2.0,
                rot_cos: SpritePlacement::UNTURNED.0,
                rot_sin: SpritePlacement::UNTURNED.1,
                scale_x: w,
                scale_y: h,
            };

            SpriteInstance::new(placement, glyph.region.to_texture_region(texture))
                .sampled(SpriteFilter::Nearest)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SpritePlacement, advance_of, layout_line, sprites_for};
    use crate::SpriteFilter;
    use crate::glyph_atlas::{GLYPH_COUNT, rasterize};

    /// DejaVu Sans Mono's advance at 16 px, as M3.34's table carries it.
    ///
    /// Written out so the expected positions below are arithmetic a reader can
    /// redo, rather than a number this file reads from the thing it is testing.
    const ADVANCE_16: f32 = 8.275_167;

    #[test]
    fn the_atlas_bearings_are_whole_pixels() {
        // The measurement the module's rounding rule rests on. If a font or a
        // rasteriser ever gave a fractional bearing, snapping the pen alone
        // would stop putting glyphs on whole pixels and the golden scene's
        // copy property would go with it.
        for size in [16.0_f32, 32.0] {
            let atlas = rasterize(size);
            let fractional: Vec<char> = atlas
                .glyphs()
                .iter()
                .filter(|glyph| glyph.bearing_x.fract() != 0.0 || glyph.bearing_y.fract() != 0.0)
                .map(|glyph| glyph.ch)
                .collect();

            assert!(
                fractional.is_empty(),
                "at {size} px these glyphs have a fractional bearing: {fractional:?}"
            );
            assert_eq!(atlas.glyphs().len(), GLYPH_COUNT);
        }
    }

    #[test]
    fn the_advance_is_not_a_whole_pixel_which_is_why_the_pen_is_snapped() {
        // The other half of the same fact, and the reason the rounding exists at
        // all. A whole-pixel advance would make this module's snapping a no-op
        // and the reader could not tell whether it worked.
        let atlas = rasterize(16.0);

        assert_eq!(atlas.glyphs()[0].advance, ADVANCE_16);
        assert!(ADVANCE_16.fract() != 0.0);
    }

    #[test]
    fn a_known_string_lands_on_predicted_pixels() {
        // Hand-computed, then asserted. `A` has bearing (0, -11) at 16 px and
        // the advance is 8.275167, so with the pen starting at 10 and the
        // baseline at 20 the three A's sit at:
        //
        //   n=0: pen 10.000000 -> round 10 -> left 10, top 20 + (-11) = 9
        //   n=1: pen 18.275167 -> round 18 -> left 18
        //   n=2: pen 26.550335 -> round 27 -> left 27   <- rounds *up*
        //
        // The third is the one worth having: 26.550335 rounds away from zero to
        // 27, so a floor-instead-of-round mistake shows up here and nowhere in
        // the first two.
        let atlas = rasterize(16.0);
        let placed = layout_line("AAA", &atlas, 10.0, 20.0);

        assert_eq!(placed.len(), 3);
        assert_eq!(
            placed.iter().map(|glyph| glyph.left).collect::<Vec<_>>(),
            vec![10, 18, 27]
        );
        assert!(placed.iter().all(|glyph| glyph.top == 9));

        let region = atlas
            .glyph('A')
            .expect("A is in the set")
            .region
            .expect("A draws");
        assert!(placed.iter().all(|glyph| glyph.region == region));
    }

    #[test]
    fn the_space_moves_the_pen_and_draws_nothing() {
        // The space is the only character in the set with no region (M3.34), so
        // it is the one case where the pen moves without an entry appearing.
        let atlas = rasterize(16.0);
        let placed = layout_line("A A", &atlas, 0.0, 20.0);

        assert_eq!(placed.len(), 2, "the space must not produce a glyph");
        assert_eq!(placed[0].left, 0);
        // Two advances of 8.275167 is 16.550335, which rounds to 17.
        assert_eq!(placed[1].left, 17);
        assert_eq!(advance_of("A A", &atlas), 3.0 * ADVANCE_16);
    }

    #[test]
    fn characters_outside_the_table_are_skipped_and_move_nothing() {
        // Tab, newline and a non-ASCII letter. The documented behaviour is that
        // they contribute no glyph *and* no advance, so a line containing them
        // lays out exactly as the line without them.
        let atlas = rasterize(16.0);

        let with = layout_line("A\tB\né", &atlas, 0.0, 20.0);
        let without = layout_line("AB", &atlas, 0.0, 20.0);

        assert_eq!(with, without, "skipped characters changed the layout");
        assert_eq!(advance_of("A\tB\né", &atlas), advance_of("AB", &atlas));
    }

    #[test]
    fn the_pen_keeps_its_fraction_rather_than_drifting() {
        // The reason the pen is not rounded between glyphs. Eighty characters of
        // a rounded advance would end up whole pixels short; the exact
        // accumulation ends where the arithmetic says.
        let atlas = rasterize(16.0);
        let line = "A".repeat(80);
        let placed = layout_line(&line, &atlas, 0.0, 20.0);

        // The oracle is a multiplication and the code is a repeated addition,
        // and the two are **not** the same `f32`: 79 additions give 653.73810,
        // one multiply gives 653.73822 — two ulps apart. They round alike
        // because the value sits 0.238 from a half-pixel boundary, not because
        // the arithmetic agrees.
        //
        // Where that stops being true is measurable: **n = 318 is the first
        // length at which the two round to different pixels** (2631 against
        // 2632), and the worst gap over the first four thousand characters is
        // 0.7637 px. At 80 there is room to spare, and a longer line would need
        // a different oracle.
        //
        // Keeping the oracle independent is deliberate. Summing the atlas's own
        // advances instead would make it move with the code, and demonstration
        // (a)'s injected advance error would leave this test green — the trap
        // `the_scene_is_exactly_what_the_model_predicts` documents for itself.
        let expected_last = (79.0 * ADVANCE_16).round() as i32;
        assert_eq!(
            placed[79].left, expected_last,
            "the eightieth A is not where accumulating the advance puts it"
        );
        // What rounding every glyph would have given instead, for contrast: eighty
        // steps of 8 px.
        assert_ne!(placed[79].left, 79 * 8);
    }

    #[test]
    fn sprites_carry_the_placement_the_layout_computed() {
        // The pixels-down to world-up conversion, on one glyph whose numbers are
        // easy to redo by hand: a 9x11 `A` at (10, 9) in a 64x32 canvas.
        //   x = 10 + 9/2 - 32 = -17.5
        //   y = 16 - 9 - 11/2 = 1.5
        let atlas = rasterize(16.0);
        let placed = layout_line("A", &atlas, 10.0, 20.0);
        let sprites = sprites_for(&placed, atlas.pixels(), 64, 32);

        assert_eq!(sprites.len(), 1);
        assert_eq!(sprites[0].placement.x, -17.5);
        assert_eq!(sprites[0].placement.y, 1.5);
        assert_eq!(sprites[0].placement.scale_x, 9.0);
        assert_eq!(sprites[0].placement.scale_y, 11.0);
        assert_eq!(
            (sprites[0].placement.rot_cos, sprites[0].placement.rot_sin),
            SpritePlacement::UNTURNED
        );

        // **The sampler wish, which nothing else checks.** Both this module and
        // the golden scene argue at length that the draw is `Nearest`, and
        // until this line no test read the filter a sprite carries. The scene's
        // own `linear_draws_this_scene_identically_to_nearest` cannot stand in
        // for it: it rebuilds every sprite with `.sampled(Linear)` and discards
        // whatever was there, so if this function ever produced `Linear` that
        // test would compare `Linear` against `Linear`, pass, and quietly stop
        // meaning what its name says. Found by M3.35's audit.
        assert_eq!(sprites[0].filter, SpriteFilter::Nearest);
    }
}
