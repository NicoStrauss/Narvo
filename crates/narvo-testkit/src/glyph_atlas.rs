//! The M3.34 glyph atlas, re-exported from where it now lives.
//!
//! **The rasteriser moved to [`narvo_render2d::glyph_atlas`] in M6.6b**
//! (ADR-0038), together with the layout in [`crate::text`] and the font itself.
//! It had to: layout consumes the region table, so the two cannot sit on
//! opposite sides of a crate boundary, and `narvo-render2d` cannot take a
//! normal dependency on this crate (ADR-0016).
//!
//! This module is the re-export that keeps `narvo_testkit::glyph_atlas::…`
//! resolving for every caller that already wrote it — which is what makes the
//! three blessed references drawn through this path an unmoved reference to
//! moved code.

pub use narvo_render2d::glyph_atlas::{
    FONT, GLYPH_COUNT, GLYPH_FIRST, GLYPH_LAST, GLYPH_PADDING_TEXELS, Glyph, GlyphAtlas,
    GlyphRegion, rasterize,
};
