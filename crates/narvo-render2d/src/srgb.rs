//! IEC 61966-2-1's transfer function, both directions.
//!
//! # What a consumer needs this for
//!
//! Every texture and render target this crate uses is an `…UnormSrgb` format
//! (`offscreen.rs`'s `TARGET_FORMAT`, `quad.rs`'s `bind_texture`), which wgpu
//! documents as "Srgb-color [0, 255] converted to/from linear-color float
//! [0, 1] in shader" (`wgpu-types-30.0.0/src/texture/format.rs:186`). So the
//! bytes a caller stores and the bytes it reads back are **encoded**, and
//! everything the pipeline does between them happens in **linear light**:
//!
//! - [`SpriteTint`](crate::SpriteTint) multiplies the sampled value, and the
//!   sample is already linear when the multiply happens. A white texel under a
//!   tint of `0.5` therefore reads back as **188**, not 128 — sixty counts
//!   apart, and measured rather than argued in
//!   `the_half_tint_lands_where_a_linear_multiply_puts_it`.
//! - ADR-0023's `OVER` blends premultiplied colour, and that blend is linear
//!   too: the hardware decodes the target's stored bytes, blends, and re-encodes
//!   on write.
//!
//! A consumer that wants to predict a stored byte — a HUD checking its own
//! colour, a test checking a render, a tool comparing two frames — has to apply
//! the same transfer, and getting the threshold or the exponent slightly wrong
//! produces an answer that is right in the middle of the range and wrong at both
//! ends. That is why the function is offered rather than left to be looked up.
//!
//! # Why it lives here
//!
//! It moved out of `narvo-testkit` in M7.0. That crate is `publish = false` and
//! a dev-dependency everywhere it is used (ADR-0016), so **no production
//! consumer could reach it** — and two independent external consumers were
//! measured writing their own copy rather than doing without.
//!
//! The boundary that decides the destination is the one `narvo-testkit`'s own
//! header already draws: "Fixture *data* … **Not the rules that check them**."
//! A transfer function fixed by a published standard is not fixture data; it is
//! the rule the `…UnormSrgb` formats obey, and this crate is where those formats
//! are pinned. The same sentence sent `check_region_padding` here in M3.21 and
//! the glyph atlas here in M6.6b (ADR-0038).
//!
//! **It does not make the renderer grade its own homework**, which is the
//! objection ADR-0038 raised against moving `model_image` and `over`. Those are
//! a model of what a render should produce. This is not: no production code in
//! this crate performs an sRGB conversion, in Rust or in WGSL — the transfer on
//! the render path is the *hardware's*, done by the format. So a prediction
//! built from these functions and the frame it is compared against stay on
//! opposite sides of the GPU boundary, which is what keeps the golden scenes
//! evidence.
//!
//! # Not behind `gpu`, and the only module here that is not
//!
//! Every other module in this crate sits behind the `gpu` feature, because
//! everything in them touches a graphics API and CLAUDE.md requires headless
//! builds to be provably free of one. These two functions touch nothing: they
//! are `f64` arithmetic over a published constant, they need no device, and a
//! headless consumer predicting a byte has the same right to them as a windowed
//! one. Gating them would be a gate with nothing behind it.
//!
//! # Three more copies exist, deliberately
//!
//! `camera_motion.rs`, `camera_pan_steps.rs`, `blend_proof.rs` and
//! `camera_scene.rs` each write their own, and M4.7's census reported that
//! rather than folding it. They are independent statements inside comparisons
//! whose two sides must not move together: a golden reference outlives a
//! dependency's rounding, and a test that predicted with the same code the
//! renderer used would be checking nothing. This module does not fold them, and
//! M7.0 did not either.
//!
//! # The one property everything here rests on
//!
//! [`decode`] followed by [`encode`] returns the byte it started from, for all
//! 256 of them. That is what makes an unblended copy exact, and what makes a
//! composite over a black ground exact — `dst * (1 - a)` is zero there, so the
//! result is the source term put straight back through the encoder.
//! `the_round_trip_is_the_identity_on_every_byte` checks it here rather than
//! leaving it to be discovered as a one-count mismatch in a 15 360 pixel image.

/// A stored byte as linear light, `0.0..=1.0`.
#[must_use]
pub fn decode(byte: u8) -> f64 {
    let c = f64::from(byte) / 255.0;
    if c <= 0.040_449_936 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear light as a stored byte, rounded to nearest.
///
/// # Panics
///
/// Never: the encoded value is clamped into `0.0..=1.0` before scaling.
#[must_use]
pub fn encode(linear: f64) -> u8 {
    let clamped = linear.clamp(0.0, 1.0);
    let encoded = if clamped <= 0.003_130_8 {
        clamped * 12.92
    } else {
        clamped.powf(1.0 / 2.4).mul_add(1.055, -0.055)
    };

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "encoded is in 0.0..=1.0, so the product is in 0..=255"
    )]
    let byte = (encoded * 255.0).round() as u8;
    byte
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    #[test]
    fn the_round_trip_is_the_identity_on_every_byte() {
        for byte in 0..=u8::MAX {
            assert_eq!(
                encode(decode(byte)),
                byte,
                "the sRGB round trip moved {byte}"
            );
        }
    }

    #[test]
    fn the_ends_are_exact_and_the_toe_is_where_it_is_documented() {
        assert!((decode(0) - 0.0).abs() < f64::EPSILON);
        assert!((decode(255) - 1.0).abs() < f64::EPSILON);
        assert_eq!(encode(0.0), 0);
        assert_eq!(encode(1.0), 255);
    }

    /// The quarter-coverage levels `camera_pan_steps.rs` committed as
    /// `[0, 137, 188, 225, 255]`, reproduced here.
    ///
    /// Not a copy of that table for its own sake: it is the one place where a
    /// second, independently written copy of this function has already been
    /// measured against a GPU, so reproducing it is evidence that this copy is
    /// the same function rather than a plausible one.
    #[test]
    fn the_quarter_levels_are_the_ones_already_measured_against_a_gpu() {
        let levels: Vec<u8> = (0..=4)
            .map(|covered| encode(f64::from(covered) / 4.0))
            .collect();
        assert_eq!(levels, vec![0, 137, 188, 225, 255]);
    }

    /// Out-of-range input is clamped rather than producing a wrapped byte.
    #[test]
    fn light_beyond_the_ends_clamps() {
        assert_eq!(encode(-0.5), 0);
        assert_eq!(encode(1.5), 255);
    }

    /// The half tint's 188, computed here rather than only rendered.
    ///
    /// M7.0's reason for the move is that a consumer must be able to work this
    /// out without a GPU, and the number is the one the tint documentation and
    /// `tint.rs` both name. It is a **second** statement of the same claim, in a
    /// place a consumer reads before it renders anything: encoded 255 halved in
    /// linear light is 188, and halved in encoded space would be 128.
    #[test]
    fn a_white_texel_halved_in_linear_light_is_the_188_the_tint_promises() {
        assert_eq!(encode(decode(255) * 0.5), 188);
        // The reading this separates from, so the test states the difference
        // rather than only the answer.
        assert_ne!(encode(decode(255) * 0.5), 128);
    }
}
