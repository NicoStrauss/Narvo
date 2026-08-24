//! What comes back out of a PNG, as a property rather than as cases.
//!
//! The module's own tests pin named pixels — an opaque one, a half-transparent
//! one, a dirty-transparent one. This asks the wider question over generated
//! content: **for any RGBA8 image, encoding it and decoding it again gives that
//! image premultiplied, exactly.**
//!
//! # What the property can and cannot catch
//!
//! It catches a premultiply applied to the wrong channel, applied twice, skipped
//! for some pixel, or rounded differently in some corner of the range — the
//! failure modes a handful of hand-picked pixels can walk past.
//!
//! It cannot catch a defect the encoder and the decoder share, because both are
//! `png`. That limit is the same one M4.8's end-to-end test names, and it is why
//! the *expectation* below is computed here from the source bytes rather than
//! read back from anything: the arithmetic under test is this project's, and
//! this file states it independently.

use proptest::prelude::*;

/// Straight-alpha RGBA8 as PNG bytes.
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

/// This file's own statement of the arithmetic, independent of the crate's.
///
/// Deliberately **not** `narvo_assets::premultiply`. If both sides called that
/// function, the property would say "the loader applies whatever premultiply is"
/// rather than "the loader premultiplies", and a wrong formula would move both
/// sides together — the failure shape M4.8 found in a neighbouring test and
/// reported.
fn expected(colour: u8, alpha: u8) -> u8 {
    let exact = f64::from(colour) * f64::from(alpha) / 255.0;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the product of two bytes over 255 is in 0..=255"
    )]
    let byte = exact.round() as u8;
    byte
}

proptest! {
    // 256 cases over images of up to 8 × 8: enough pixels for a per-pixel
    // defect to appear, small enough that the codec is not what is being timed.
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn any_image_comes_back_as_its_own_premultiplication(
        width in 1_u32..=8,
        height in 1_u32..=8,
        seed in any::<u64>(),
    ) {
        // Content from a written-out generator rather than a proptest vector,
        // so the case size does not grow with the image and shrinking stays
        // meaningful: the interesting axis here is the *values*, and every byte
        // of the range is reachable from some seed.
        let mut state = seed | 1;
        let mut straight = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height * 4 {
            // xorshift64, written out: this test needs a reproducible byte
            // stream, not a good generator.
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the low byte is exactly what is wanted"
            )]
            let byte = state as u8;
            straight.push(byte);
        }

        let bytes = encode(width, height, &straight);
        let (out_width, out_height, decoded) =
            narvo_assets::decode(std::path::Path::new("generated.png"), &bytes)
                .expect("what this test encoded, the loader decodes");

        prop_assert_eq!(out_width, width);
        prop_assert_eq!(out_height, height);
        prop_assert_eq!(decoded.len(), straight.len());

        for (index, (source, loaded)) in straight.chunks_exact(4).zip(decoded.chunks_exact(4)).enumerate() {
            let alpha = source[3];
            prop_assert_eq!(
                loaded[3], alpha,
                "pixel {} lost its alpha", index
            );
            for channel in 0..3 {
                prop_assert_eq!(
                    loaded[channel],
                    expected(source[channel], alpha),
                    "pixel {} channel {} came back wrong from {:?}",
                    index,
                    channel,
                    source
                );
            }
        }
    }
}
