//! The CPU model a golden text scene is predicted against.
//!
//! # What is here and what moved away in M6.6b
//!
//! **Layout moved to [`narvo_render2d::text`] and is re-exported below**, so
//! every caller in this workspace keeps the path it already used. The move is
//! ADR-0038: M6.6b decided the native text path over egui for the debug
//! overlay, which made text a shipped capability — the exact condition the old
//! header of this module named for reopening its own placement.
//!
//! **[`model_image`] and [`over`] stayed**, and that is the line this crate
//! already draws for itself: "Fixture *data* … **Not the rules that check
//! them**". A model of what a render should produce is a rule that checks —
//! and a renderer owning the model its own golden tests are graded by would be
//! checking nothing.
//!
//! ADR-0038's second reason for keeping them here was that they depend on
//! [`crate::srgb`], which would have dragged a third module across. **M7.0
//! moved `srgb` on its own** (it is a rule fixed by a published standard, not a
//! model of a render), so that reason no longer applies and the first one is
//! carrying them alone.
//!
//! The re-export is deliberate rather than transitional. Two names for one
//! thing is the cost; what it buys is that the move touched no test file, so
//! every existing test — including the three blessed references that draw
//! through this path — is an **unmoved** reference to the moved code.

pub use narvo_render2d::text::{PlacedGlyph, advance_of, layout_line, sprites_for};

use narvo_render2d::Pixels;

/// The image `placed` should produce, computed on the CPU.
///
/// **This is the prediction the golden scene rests on**, and it is a model of
/// the render rather than a copy of its output: it starts from an opaque black
/// canvas and writes each glyph's atlas texels into it, which is what a
/// `Nearest` draw at 1:1 on whole pixels does and nothing more.
///
/// Three things about the render path it rests on. The first is read from the
/// pipeline source, the second from the formats, and the third is an
/// observation rather than a guarantee — the distinction is the point:
///
/// - **The pipeline blends, premultiplied** (ADR-0023; `blend: Some(BLEND)` in
///   `quad.rs`). Until M4.7 it did not, and a glyph's quad *replaced* what was
///   under it across its whole rectangle — the zero-coverage texels writing
///   `[0, 0, 0, 0]`, alpha and all, over the opaque clear. Those were the
///   "boxes" the M3.35 report describes as by design. They are gone: a
///   premultiplied `[0, 0, 0, 0]` source contributes nothing and leaves the
///   target exactly as it was.
///
///   **What that changed in the image, measured rather than reasoned:** of
///   15 360 pixels, 2 591 moved and **every one of them moved in alpha alone**
///   — no pixel differs in R, G or B on any of the three rasterisers. The
///   colour picture is what it was; what is gone is the holes it carried.
/// - **A composite over a black ground is still exact.** `dst * (1 - a)` is zero
///   in every channel of the clear, so the result is the source term put back
///   through the encoder — which [`crate::srgb`] pins as the identity — and the
///   alpha is `a + 255 * (1 - a/255) = 255`. Where two glyph rectangles overlap
///   the general arithmetic applies and the transfer function is doing real
///   work; in this scene they do not overlap on ink, which is why the render
///   came out byte-identical in colour to the pre-blend reference.
/// - **The sRGB round trip returns the byte it started from.** Atlas and target
///   are both `Rgba8UnormSrgb` — that much is in the source (`quad.rs`'s upload
///   and `offscreen.rs`'s `TARGET_FORMAT`) — so a sampled texel is decoded to
///   linear and re-encoded on write, and alpha is never encoded either way.
///   **That the round trip is byte-exact is an observation, not something the
///   formats guarantee**: the WebGPU specification fixes the transfer function
///   but a decode-then-encode identity is a property of the implementation's
///   precision. It holds on every adapter this repository has run on, and the
///   blessed `textured_quad_quadrants_64x64` has rested on it since M1.
/// - **The pass is multisampled.** `SAMPLE_COUNT` is 4 and the quad is drawn
///   into a multisample attachment that resolves into the target. A copy
///   survives that only because the quad's edges land on pixel boundaries, so
///   every sample of a pixel is either wholly inside the quad or wholly
///   outside and the resolve returns one texel's value. Off the grid it would
///   not, which is the other half of why the layout snaps.
///
/// Glyphs are written in the order given, so two that overlap composite in that
/// order — the same order the renderer draws them in, there being no depth
/// buffer. Since M4.7 order is composition rather than replacement, which makes
/// the order matter more than it did rather than less.
///
/// [`over`] is the arithmetic; this function is the geometry.
#[must_use]
pub fn model_image(placed: &[PlacedGlyph], texture: &Pixels, width: u32, height: u32) -> Vec<u8> {
    let mut canvas = vec![0_u8; (width * height * 4) as usize];
    // Opaque black. Not `ClearColor::BLACK` - this scene draws through
    // `render_sprites`, whose pass hardcodes `LoadOp::Clear(wgpu::Color::BLACK)`
    // in `quad.rs`'s `encode_runs`, and 0.0 encodes to 0 on an sRGB target
    // exactly.
    for texel in canvas.chunks_exact_mut(4) {
        texel.copy_from_slice(&[0, 0, 0, 255]);
    }

    for glyph in placed {
        for dy in 0..glyph.region.height {
            for dx in 0..glyph.region.width {
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "glyph extents are small and non-negative"
                )]
                let (x, y) = (glyph.left + dx as i32, glyph.top + dy as i32);
                #[expect(
                    clippy::cast_possible_wrap,
                    reason = "canvas dimensions are small and non-negative"
                )]
                let inside = x >= 0 && y >= 0 && x < width as i32 && y < height as i32;
                if !inside {
                    continue;
                }

                let texel = texture
                    .pixel(glyph.region.left + dx, glyph.region.top + dy)
                    .expect("the region lies inside the texture");

                #[expect(
                    clippy::cast_sign_loss,
                    reason = "guarded as non-negative directly above"
                )]
                let offset = ((y as u32 * width + x as u32) * 4) as usize;
                let under: [u8; 4] = canvas[offset..offset + 4]
                    .try_into()
                    .expect("four bytes make a texel");
                canvas[offset..offset + 4].copy_from_slice(&over(texel, under));
            }
        }
    }

    canvas
}

/// `source` composited over `target`, premultiplied, in linear light.
///
/// The CPU statement of the blend state `quad.rs` sets — written independently
/// rather than derived from it, so that a wrong blend state and a wrong model
/// cannot agree with each other.
///
/// ```text
/// rgb: E( D(src) + D(dst) * (1 - a) )
/// a:      a_src  +   a_dst * (1 - a)
/// ```
///
/// Alpha is not sRGB-encoded, so its line is plain arithmetic. Colour is, so its
/// line goes through [`crate::srgb`] in both directions — and where `dst` is
/// black the second term vanishes and the whole thing collapses to the round
/// trip, which is the identity.
#[must_use]
pub fn over(source: [u8; 4], target: [u8; 4]) -> [u8; 4] {
    let coverage = f64::from(source[3]) / 255.0;
    let remainder = 1.0 - coverage;

    let mut out = [0_u8; 4];
    for channel in 0..3 {
        out[channel] = crate::srgb::encode(
            crate::srgb::decode(target[channel])
                .mul_add(remainder, crate::srgb::decode(source[channel])),
        );
    }

    let alpha = f64::from(source[3]) + f64::from(target[3]) * remainder;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "two bytes weighted to one sum to at most 255"
    )]
    let rounded = alpha.round().clamp(0.0, 255.0) as u8;
    out[3] = rounded;
    out
}

#[cfg(test)]
mod tests {
    use super::{layout_line, model_image, over};
    use crate::glyph_atlas::rasterize;

    /// The glyph box is gone, and that is the whole point of M4.7.
    ///
    /// **This test used to assert the opposite**, under the name
    /// `the_model_writes_the_whole_glyph_box_including_its_dark_texels`: with
    /// `blend: None` a glyph's quad replaced everything under it, so the
    /// zero-coverage texels wrote `[0, 0, 0, 0]` over the opaque clear and every
    /// glyph carried a transparent rectangle. The M3.35 report calls those boxes
    /// "by design", and they were — of a design that could not put text over
    /// anything.
    ///
    /// Under premultiplied `OVER` (ADR-0023) a `[0, 0, 0, 0]` source contributes
    /// nothing, so those texels leave the target exactly as they found it. The
    /// old assertion is kept here as the thing that must now be false.
    #[test]
    fn the_model_leaves_no_box_around_a_glyph() {
        let atlas = rasterize(16.0);
        let placed = layout_line("A", &atlas, 2.0, 13.0);
        let image = model_image(&placed, atlas.pixels(), 16, 16);

        let at = |x: u32, y: u32| {
            let offset = ((y * 16 + x) * 4) as usize;
            [
                image[offset],
                image[offset + 1],
                image[offset + 2],
                image[offset + 3],
            ]
        };

        // A's box is 9x11 at (2, 2). Its top-left texel is dark - the apex of
        // the A is in the middle of the row - and it is now indistinguishable
        // from the clear, which is exactly what "no box" means.
        assert_eq!(
            at(2, 2),
            [0, 0, 0, 255],
            "inside the box, dark: the clear must stand where coverage is zero"
        );
        // One pixel to the left is outside the box and keeps the clear too, so
        // the two are no longer telling apart.
        assert_eq!(
            at(1, 2),
            [0, 0, 0, 255],
            "outside the box, the clear stands"
        );

        // **Nothing anywhere is transparent.** The old semantics could be found
        // by looking for a single alpha below 255; the new one is defined by
        // there being none. This is the image-wide form of the assertion, and it
        // is what a `One, Zero` alpha component would break while leaving every
        // colour channel looking right.
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    at(x, y)[3],
                    255,
                    "({x}, {y}) is {:?}, so a box survived",
                    at(x, y)
                );
            }
        }

        // And the ink is still ink: a lit texel composites over the black clear
        // to its own coverage in all three colour channels, which is the
        // round-trip identity doing the work. Taken from the atlas rather than
        // from the model, so the two sides are not the same arithmetic.
        let mut lit = 0_usize;
        for glyph in &placed {
            for dy in 0..glyph.region.height {
                for dx in 0..glyph.region.width {
                    let texel = atlas
                        .pixels()
                        .pixel(glyph.region.left + dx, glyph.region.top + dy)
                        .expect("the region lies inside the atlas");
                    if texel[3] == 0 {
                        continue;
                    }
                    lit += 1;
                    #[expect(
                        clippy::cast_sign_loss,
                        reason = "this glyph is placed at (2, 2) with a 9x11 box"
                    )]
                    let (x, y) = (
                        (glyph.left + dx as i32) as u32,
                        (glyph.top + dy as i32) as u32,
                    );
                    assert_eq!(
                        at(x, y),
                        [texel[0], texel[1], texel[2], 255],
                        "({x}, {y}) does not carry its own coverage over the clear"
                    );
                }
            }
        }

        assert!(
            lit > 0,
            "the glyph has no lit texels, so nothing was checked"
        );
    }

    /// The three fixed points of `over`, none of which needs a transfer
    /// function, checked before anything trusts the general case.
    #[test]
    fn over_has_the_fixed_points_premultiplied_compositing_requires() {
        let ground = [200, 100, 50, 255];

        assert_eq!(
            over([0, 0, 0, 0], ground),
            ground,
            "a fully transparent premultiplied source must change nothing"
        );
        assert_eq!(
            over([12, 34, 56, 255], ground),
            [12, 34, 56, 255],
            "a fully opaque source must replace the target outright"
        );
        assert_eq!(
            over([90, 90, 90, 90], [0, 0, 0, 255]),
            [90, 90, 90, 255],
            "over black the second term vanishes and the round trip is the identity"
        );
    }

    /// Compositing over an opaque target leaves it opaque, for every source
    /// alpha there is.
    #[test]
    fn over_an_opaque_target_stays_opaque() {
        for alpha in 0..=u8::MAX {
            assert_eq!(
                over([alpha, alpha, alpha, alpha], [17, 17, 17, 255])[3],
                255,
                "source alpha {alpha} made an opaque target transparent"
            );
        }
    }

    /// The result is premultiplied whenever both inputs are.
    ///
    /// The invariant `quad.rs`'s account names as the reason one pipeline can
    /// both consume and produce the same representation. Checked on a grid
    /// rather than argued.
    #[test]
    fn over_keeps_the_premultiplied_invariant() {
        for source_alpha in (0..=255).step_by(17) {
            for target_alpha in (0..=255).step_by(17) {
                let source = [source_alpha, source_alpha / 2, 0, source_alpha];
                let target = [target_alpha, 0, target_alpha / 3, target_alpha];
                let out = over(source, target);
                for channel in 0..3 {
                    assert!(
                        out[channel] <= out[3],
                        "{source:?} over {target:?} gave {out:?}, which has more \
                         colour than coverage in channel {channel}"
                    );
                }
            }
        }
    }
}
