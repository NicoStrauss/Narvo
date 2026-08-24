//! SHA-256, written out rather than depended on.
//!
//! # Why this is here and not `sha2`
//!
//! ADR-0010 set the precedent for the seeded generator, deciding
//! "**PCG-XSH-RR 64/32, written out in `crates/narvo-ecs/src/rng.rs`**" —
//! CLAUDE.md summarises that as "the algorithm is written out rather than
//! depended on" (`CLAUDE.md:317-318`), and the shorter phrase is the summary
//! rather than the ADR's own words.
//!
//! **The reasoning is adjacent rather than identical**, and the difference is
//! worth stating: ADR-0010 names a concrete instability — a generator crate
//! that changes its defaults across releases would silently move every seeded
//! run. Nothing here says `sha2` would do that; SHA-256 is a fixed
//! specification and a competent implementation of it cannot drift. What
//! carries over is narrower: a value this repository *commits* should not
//! depend on a crate version for its meaning, and the algorithm being local is
//! what keeps the anchor's blast radius to `ab_glyph` and the font.
//!
//! It also keeps the change's count of **new packages in `Cargo.lock` at
//! zero**. `ab_glyph` was already there — `winit` pulls it in on Linux for
//! Wayland decorations — so adding it as a direct dependency added no package,
//! and a `sha2` would have been the first genuinely new one, taken for
//! convenience.
//!
//! # What makes it trustworthy
//!
//! Not that it looks right. [`tests`] checks it against the **published FIPS
//! 180-4 vectors** — the two one-block and multi-block examples from the
//! specification's own appendix, plus the empty input from the NIST CSRC
//! examples. ADR-0008 names this exact kind of constant as the *required* kind:
//! "a test vector of an algorithm … is produced by a published specification
//! and this repository is checked against it … the only thing that can move it
//! is an edit to the algorithm — which is exactly what it is there to catch."
//!
//! Without those vectors a transposed constant would still hash
//! deterministically, still agree with itself on both platforms, and still be
//! the wrong function.

/// The eight initial hash values, FIPS 180-4 §5.3.3.
///
/// The first thirty-two bits of the fractional parts of the square roots of the
/// first eight primes. Written as literals because that is how the
/// specification writes them; `the_initial_values_are_the_published_ones`
/// re-derives all eight from the primes rather than trusting the
/// transcription.
const H0: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// The sixty-four round constants, FIPS 180-4 §4.2.2.
///
/// The first thirty-two bits of the fractional parts of the cube roots of the
/// first sixty-four primes.
const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// The SHA-256 digest of `message`, as thirty-two bytes.
#[must_use]
pub fn sha256(message: &[u8]) -> [u8; 32] {
    let mut state = H0;

    // Padding, FIPS 180-4 §5.1.1: a single 1 bit, then zeros, then the length
    // in bits as a big-endian u64, to a whole number of 64-byte blocks.
    let mut padded = message.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    let bits = (message.len() as u64) * 8;
    padded.extend_from_slice(&bits.to_be_bytes());

    for block in padded.chunks_exact(64) {
        compress(&mut state, block);
    }

    let mut digest = [0_u8; 32];
    for (chunk, word) in digest.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// The digest as lowercase hexadecimal, which is how an anchor is written down.
#[must_use]
pub fn sha256_hex(message: &[u8]) -> String {
    sha256(message)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One 64-byte block, FIPS 180-4 §6.2.2.
fn compress(state: &mut [u32; 8], block: &[u8]) {
    let mut w = [0_u32; 64];

    for (index, chunk) in block.chunks_exact(4).enumerate() {
        w[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    for index in 16..64 {
        let s0 =
            w[index - 15].rotate_right(7) ^ w[index - 15].rotate_right(18) ^ (w[index - 15] >> 3);
        let s1 =
            w[index - 2].rotate_right(17) ^ w[index - 2].rotate_right(19) ^ (w[index - 2] >> 10);
        w[index] = w[index - 16]
            .wrapping_add(s0)
            .wrapping_add(w[index - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;

    for index in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(choose)
            .wrapping_add(K[index])
            .wrapping_add(w[index]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(majority);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }

    for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{H0, K, sha256_hex};

    #[test]
    fn the_published_fips_180_4_vectors_hash_to_their_published_digests() {
        // The specification's own worked examples, plus the empty input from
        // NIST's example set. ADR-0008 names this the *required* kind of
        // committed literal: produced by a published specification, moved only
        // by an edit to the algorithm.
        for (input, expected) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
        ] {
            assert_eq!(
                sha256_hex(input.as_bytes()),
                expected,
                "SHA-256 of {input:?} does not match its published digest"
            );
        }
    }

    #[test]
    fn a_multi_block_message_hashes_to_its_published_digest() {
        // One million 'a', the specification's long-message example. It is the
        // only vector here that exercises the block loop more than twice and
        // the 64-bit length field beyond one byte.
        let input = vec![b'a'; 1_000_000];

        assert_eq!(
            sha256_hex(&input),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn the_initial_values_are_the_published_ones() {
        // Derived rather than transcribed: the first thirty-two bits of the
        // fractional part of the square root of the nth prime. A transposed
        // digit in the table above survives every other test in this file
        // except the vectors, and this says which constant moved.
        for (index, prime) in [2_u32, 3, 5, 7, 11, 13, 17, 19].into_iter().enumerate() {
            let fraction = f64::from(prime).sqrt().fract();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the fraction is in 0..1, so the product is inside u32"
            )]
            let derived = (fraction * 2_f64.powi(32)) as u32;

            assert_eq!(
                H0[index], derived,
                "H0[{index}] is not the fractional square root of {prime}"
            );
        }
    }

    #[test]
    fn the_round_constants_are_the_published_ones() {
        // The same derivation for the cube roots of the first sixty-four
        // primes, which is what FIPS 180-4 §4.2.2 says K is.
        let mut primes = Vec::new();
        let mut candidate = 2_u32;
        while primes.len() < 64 {
            if (2..candidate).all(|divisor| !candidate.is_multiple_of(divisor)) {
                primes.push(candidate);
            }
            candidate += 1;
        }

        for (index, prime) in primes.into_iter().enumerate() {
            let fraction = f64::from(prime).cbrt().fract();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the fraction is in 0..1, so the product is inside u32"
            )]
            let derived = (fraction * 2_f64.powi(32)) as u32;

            assert_eq!(
                K[index], derived,
                "K[{index}] is not the fractional cube root of {prime}"
            );
        }
    }
}
