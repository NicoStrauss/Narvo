//! Writes every synthesised sound under `target/` as a WAV a human can play.
//!
//! # Why a test rather than a script, and why the writer lives here
//!
//! M4.8's precedent, which M5.5a reused for the click demo: making the generator
//! a test buys something a script would not, because it checks that what it
//! wrote is well formed rather than only that it was written.
//!
//! **The WAV writer is deliberately in this file and not in the crate.**
//! ADR-0028 keeps kira's decoders off, and M5.6c's scope says no audio file
//! format becomes an engine capability. A container writer that only tests use
//! is not one: nothing in `src/` can read or write a file, the playback path
//! takes samples directly, and deleting this file would cost the repository no
//! capability at all.
//!
//! Nothing here is committed. ADR-0024's rule — *"No PNG is committed, here or
//! anywhere"* — is about binaries in the repository, and audio is the same
//! class.
//!
//! After any test run this exists:
//!
//! ```text
//! target/m56c-audio/click.wav
//! target/m56c-audio/music_base.wav
//! target/m56c-audio/music_layer.wav
//! target/m56c-audio/README.md
//! ```

use std::path::{Path, PathBuf};

use narvo_audio::sounds::{self, Samples};

/// Where the artefacts go, beside the other generated demos under `target/`.
fn export_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("m56c-audio")
}

/// A 16-bit mono PCM WAV.
///
/// The header is 44 bytes and every field of it is written out rather than
/// copied from a template, because a wrong `byte_rate` produces a file that
/// plays at the wrong speed instead of one that fails to open.
fn wav(samples: &Samples) -> Vec<u8> {
    const BITS: u16 = 16;
    const CHANNELS: u16 = 1;

    let block_align = CHANNELS * BITS / 8;
    let byte_rate = samples.sample_rate * u32::from(block_align);
    let data_len = samples.mono.len() as u32 * u32::from(block_align);

    let mut bytes = Vec::with_capacity(44 + data_len as usize);

    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVE");

    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes()); // PCM chunk size
    bytes.extend_from_slice(&1_u16.to_le_bytes()); // format: PCM
    bytes.extend_from_slice(&CHANNELS.to_le_bytes());
    bytes.extend_from_slice(&samples.sample_rate.to_le_bytes());
    bytes.extend_from_slice(&byte_rate.to_le_bytes());
    bytes.extend_from_slice(&block_align.to_le_bytes());
    bytes.extend_from_slice(&BITS.to_le_bytes());

    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for sample in &samples.mono {
        // Clamped before scaling, so a sample outside the nominal range wraps
        // to the loudest representable value rather than round-tripping through
        // an overflow into the opposite sign.
        let clamped = sample.clamp(-1.0, 1.0);
        let value = (clamped * f32::from(i16::MAX)) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    bytes
}

#[test]
fn every_synthesised_sound_is_written_under_target_as_a_playable_wav() {
    let root = export_root();
    std::fs::create_dir_all(&root).expect("the export directory");

    for (name, make) in sounds::ALL {
        let samples = make();
        let bytes = wav(&samples);

        // The header says what the samples are, or the file is not the sound.
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(bytes.len(), 44 + samples.mono.len() * 2, "{name}");
        let declared = u32::from_le_bytes(bytes[40..44].try_into().expect("four bytes"));
        assert_eq!(declared as usize, samples.mono.len() * 2, "{name}");
        let rate = u32::from_le_bytes(bytes[24..28].try_into().expect("four bytes"));
        assert_eq!(rate, samples.sample_rate, "{name}");

        let path = root.join(format!("{name}.wav"));
        std::fs::write(&path, &bytes)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));

        let written = std::fs::read(&path).expect("what was just written reads back");
        assert_eq!(written, bytes, "{name} did not survive the round trip");
    }

    write_readme(&root);

    println!("audio written: {}", root.display());
}

/// A note beside the files saying what they are and how they were made.
fn write_readme(root: &Path) {
    let text = "# The M5.6c sounds\n\
         \n\
         Written by `every_synthesised_sound_is_written_under_target_as_a_playable_wav`\n\
         in `crates/narvo-audio/tests/wav_export.rs`. Nothing here is under\n\
         version control: ADR-0024 keeps binaries out of the repository, so these\n\
         are generated and this directory is rebuilt by any test run.\n\
         \n\
         These are the exact samples the engine plays. Nothing loads them —\n\
         `narvo-audio` synthesises the same values at startup and hands them to\n\
         kira directly, because ADR-0028 keeps the decoders off. They exist so a\n\
         human can hear what a cue is asking for without opening a window.\n\
         \n\
         - `click.wav` — one `buy`, 60 ms.\n\
         - `music_base.wav` — the bed, two seconds, loops seamlessly.\n\
         - `music_layer.wav` — the layer that arrives at three, a fifth above\n\
           the bed, same length so the two stay in step.\n\
         \n\
         In the running demo the layer fades in over 60 ticks. That fade is\n\
         kira's tween on the channel volume and is *not* in this file: played on\n\
         its own, `music_layer.wav` starts at full volume.\n";

    let path = root.join("README.md");
    std::fs::write(&path, text)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

/// The scaling has to be right at both ends, and a silent block has to come out
/// silent rather than as a DC offset.
#[test]
fn the_encoder_maps_the_extremes_and_silence_the_way_it_claims() {
    let samples = Samples {
        sample_rate: 8_000,
        mono: vec![0.0, 1.0, -1.0, 2.0, -2.0],
        // Irrelevant to the encoder, which writes samples and not playback
        // behaviour — a WAV has nowhere to say "repeat this".
        loops: false,
    };
    let bytes = wav(&samples);

    let value = |index: usize| -> i16 {
        i16::from_le_bytes(
            bytes[44 + index * 2..44 + index * 2 + 2]
                .try_into()
                .expect("two bytes"),
        )
    };

    assert_eq!(value(0), 0, "silence must be zero");
    assert_eq!(value(1), i16::MAX);
    assert_eq!(value(2), -i16::MAX);
    assert_eq!(value(3), i16::MAX, "clamped rather than wrapped");
    assert_eq!(value(4), -i16::MAX, "clamped rather than wrapped");
}
