//! SHA-256, written out. The engine's content-anchor digest.
//!
//! # Two consumers, one implementation
//!
//! It arrived in `narvo-app` with M4.3's scene anchor (ADR-0019) and moved here
//! in M4.4, when the asset contract's atlas anchor (ADR-0020) needed the same
//! function — and could not reach it, because `narvo-app` is a binary with no
//! library target. `narvo-core` is where both can see it, and it is the crate
//! that already holds what has no dependencies of its own.
//!
//! **A third copy exists and is deliberately untouched.** `narvo-testkit` has
//! its own, written for the M3.34 glyph atlas anchor. Folding it in here would
//! be right and is not this task's: the glyph atlas's committed anchor and its
//! blessing are out of M4.4's scope, and the M4.4 report carries the finding
//! along with the observation that the fold is one import.
//!
//! # Why this is here rather than a dependency
//!
//! The same reasoning ADR-0008 gives for FNV-1a and ADR-0010 for PCG, with one
//! part of it explicitly weakened and said so:
//!
//! - **The stability argument is weaker here, and that is admitted.** FNV-1a and
//!   the generator were written out because a crate could change what they
//!   produce. SHA-256 cannot: it is frozen by FIPS 180-4, and a release that
//!   changed its output would be broken rather than different. So "one
//!   dependency bump away" does not apply, and this module does not pretend it
//!   does.
//! - **What does apply is the cost.** `sha2` brings eight crates new to this
//!   workspace — `sha2`, `cpufeatures`, `digest`, `block-buffer`,
//!   `crypto-common`, `generic-array`, `typenum` — and 1.8 s of clean build, for
//!   about eighty lines of arithmetic used to hash a two-kilobyte file once per
//!   run. That is the same trade ADR-0008 refused for six lines of FNV.
//! - **And it is checkable to the same standard.** ADR-0008 draws the line by
//!   *direction*: a value this repository produces and checks against itself is
//!   forbidden, a value the specification produces and this repository is
//!   checked against is required. The vectors below are the second kind, from
//!   FIPS 180-4's own appendix, and they exercise the three places an
//!   implementation breaks — one-block padding, the 56-byte case that forces a
//!   second padding block, and a multi-block message.
//!
//! **This is not a security boundary.** The anchor answers "is this the same
//! file", not "did somebody tamper with it". A collision would have to be
//! constructed on purpose by somebody who could equally edit the recording
//! beside it. If a security use ever appears, take the dependency — that is this
//! module's revision condition, and it is a decision to make then rather than a
//! reason to carry eight crates now.

/// The first thirty-two bits of the fractional parts of the cube roots of the
/// first sixty-four primes (FIPS 180-4, §4.2.2).
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

/// The first thirty-two bits of the fractional parts of the square roots of the
/// first eight primes (FIPS 180-4, §5.3.3).
const INITIAL: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// One block, in bytes.
const BLOCK: usize = 64;

/// Hashes `bytes` with SHA-256.
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut state = INITIAL;

    let mut blocks = bytes.chunks_exact(BLOCK);
    for block in &mut blocks {
        compress(
            &mut state,
            block.try_into().expect("chunks_exact yields 64"),
        );
    }

    // The tail: what is left, a `1` bit, zeroes, and the length in bits. It
    // needs one block, or two when the remainder leaves no room for the nine
    // bytes of marker and length.
    let rest = blocks.remainder();
    let mut tail = [0_u8; BLOCK * 2];
    tail[..rest.len()].copy_from_slice(rest);
    tail[rest.len()] = 0x80;

    let length = if rest.len() + 9 <= BLOCK {
        BLOCK
    } else {
        BLOCK * 2
    };
    let bits = (bytes.len() as u64).wrapping_mul(8);
    tail[length - 8..length].copy_from_slice(&bits.to_be_bytes());

    for block in tail[..length].chunks_exact(BLOCK) {
        compress(
            &mut state,
            block.try_into().expect("chunks_exact yields 64"),
        );
    }

    let mut digest = [0_u8; 32];
    for (out, word) in digest.chunks_exact_mut(4).zip(state) {
        out.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Renders a digest as lower-case hex, which is the form the recording holds.
#[must_use]
pub fn hex(digest: &[u8; 32]) -> String {
    use std::fmt::Write as _;

    digest
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The block function of FIPS 180-4 §6.2.2, step for step.
fn compress(state: &mut [u32; 8], block: &[u8; BLOCK]) {
    let mut schedule = [0_u32; 64];
    for (word, chunk) in schedule.iter_mut().zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes(chunk.try_into().expect("chunks_exact yields 4"));
    }
    for index in 16..64 {
        let a = schedule[index - 15];
        let b = schedule[index - 2];
        let s0 = a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3);
        let s1 = b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10);
        schedule[index] = schedule[index - 16]
            .wrapping_add(s0)
            .wrapping_add(schedule[index - 7])
            .wrapping_add(s1);
    }

    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;

    for (word, constant) in schedule.into_iter().zip(K) {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choose = (e & f) ^ ((!e) & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(choose)
            .wrapping_add(constant)
            .wrapping_add(word);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(majority);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }

    for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *slot = slot.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{hex, sha256};

    /// The published vectors, which is what makes writing this out defensible.
    ///
    /// ADR-0008's rule is about the *direction* a value comes from: one this
    /// repository produces and checks against itself is forbidden, one the
    /// specification produces and this repository is checked against is
    /// required. These are the second kind, and each is labelled with where it
    /// comes from, because a vector whose provenance nobody can state is a value
    /// this repository is checking against itself with extra steps.
    ///
    /// They are chosen for where implementations break rather than for
    /// tidiness: `"abc"` is one block with room to pad, and the 56-byte string
    /// is the case that leaves no room and forces a second block.
    #[test]
    fn the_published_vectors_hold() {
        for (input, expected, source) in [
            (
                "",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                "FIPS 180-4, the empty message",
            ),
            (
                "abc",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                "FIPS 180-4 appendix B.1",
            ),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
                "FIPS 180-4 appendix B.2",
            ),
        ] {
            assert_eq!(
                hex(&sha256(input.as_bytes())),
                expected,
                "SHA-256 of {input:?} does not match the vector from {source}"
            );
        }
    }

    /// An exact block, where the padding falls entirely into a second one.
    ///
    /// **Not a FIPS vector, and labelled so.** It is a boundary this
    /// implementation has to get right and the standard's appendix does not
    /// cover, so the expected value was taken from an independent implementation
    /// — GNU coreutils `sha256sum` — rather than from this module. That is still
    /// ADR-0008's permitted direction: the value comes from outside this
    /// repository. It is separated from the vectors above so that nobody reads
    /// it as one.
    ///
    /// The first draft of this test asserted a different digest for this input,
    /// remembered rather than sourced. The cross-check is what found it; the
    /// M4.3 report records the episode.
    #[test]
    fn an_exact_block_boundary_matches_an_independent_implementation() {
        assert_eq!(
            hex(&sha256(
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmno"
            )),
            "2ff100b36c386c65a1afc462ad53e25479bec9498ed00aa5a04de584bc25301b"
        );
    }

    /// The multi-block vector: a million `a`s, from the same standard.
    ///
    /// Slow to type and quick to run. It is the one that catches an
    /// implementation that is right for one block and wrong for the message
    /// length that follows many.
    #[test]
    fn the_multi_block_vector_holds() {
        let input = vec![b'a'; 1_000_000];

        assert_eq!(
            hex(&sha256(&input)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// Every length across the padding boundary hashes to something distinct.
    ///
    /// Not a vector — a property. The boundary at 55/56 bytes and the one at
    /// 63/64 are where a length or a block count goes wrong, and a mistake there
    /// usually shows up as two different inputs sharing a digest rather than as
    /// a wrong constant.
    #[test]
    fn lengths_across_the_block_boundaries_stay_distinct() {
        let mut seen = std::collections::BTreeSet::new();

        for length in 0..=130 {
            let input = vec![b'x'; length];
            assert!(
                seen.insert(sha256(&input)),
                "two inputs of different length share a digest, at {length} bytes"
            );
        }
    }

    #[test]
    fn hex_is_lower_case_and_sixty_four_characters() {
        let rendered = hex(&sha256(b"abc"));

        assert_eq!(rendered.len(), 64);
        assert!(rendered.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!rendered.chars().any(|c| c.is_ascii_uppercase()));
    }
}
