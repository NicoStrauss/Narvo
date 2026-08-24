//! Placeholder content the runner draws until there is real content to draw.

use narvo_render2d::Pixels;

/// Edge length of the generated pattern in texels.
const SIZE: u32 = 8;

/// Quadrant colours, in the order they appear on screen.
const TOP_LEFT: [u8; 4] = [255, 0, 0, 255];
const TOP_RIGHT: [u8; 4] = [0, 255, 0, 255];
const BOTTOM_LEFT: [u8; 4] = [0, 0, 255, 255];
const BOTTOM_RIGHT: [u8; 4] = [128, 64, 32, 255];

/// A small texture of four differently coloured quadrants.
///
/// The same pattern the renderer's own tests use, and for the same reason: it is
/// asymmetric, so a glance at the window says whether the image is the right way
/// up. Red belongs at the top left, per the orientation convention in ADR-0004.
///
/// # Panics
///
/// Never in practice: the buffer is generated from the dimensions it is then
/// handed alongside, so the length always agrees.
#[must_use]
pub fn demo_texture() -> Pixels {
    let half = SIZE / 2;
    let mut rgba = Vec::with_capacity(SIZE as usize * SIZE as usize * 4);

    for y in 0..SIZE {
        for x in 0..SIZE {
            rgba.extend_from_slice(&match (x < half, y < half) {
                (true, true) => TOP_LEFT,
                (false, true) => TOP_RIGHT,
                (true, false) => BOTTOM_LEFT,
                (false, false) => BOTTOM_RIGHT,
            });
        }
    }

    Pixels::from_rgba8(SIZE, SIZE, rgba).expect("the generated buffer matches its dimensions")
}
