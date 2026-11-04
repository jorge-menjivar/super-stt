// SPDX-License-Identifier: GPL-3.0-only
//! Voice-library behavior: what is stored, what comes back, and what is
//! refused.

use super::{CLIP_SAMPLE_RATE, MAX_CLIP_SECONDS, VoiceError, VoiceLibrary};

/// A mono 16-bit WAV of `seconds` at `rate`, carrying a ramp so a resampled
/// clip is still recognizable.
fn wav(rate: u32, seconds: f32) -> Vec<u8> {
    let frames = crate::num_cast::f32_to_usize(seconds * crate::num_cast::u32_to_f32(rate));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
        for i in 0..frames {
            let phase = crate::num_cast::usize_to_f32(i) / 50.0;
            w.write_sample(crate::num_cast::f32_to_i16(phase.sin() * 8000.0))
                .unwrap();
        }
        w.finalize().unwrap();
    }
    cursor.into_inner()
}

fn library() -> (tempfile::TempDir, VoiceLibrary) {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib = VoiceLibrary::new(dir.path().join("voices"));
    (dir, lib)
}

#[test]
fn stores_a_clip_in_the_canonical_shape() {
    let (_guard, lib) = library();
    // 44.1 kHz in, 24 kHz out: the library resamples rather than recording the
    // rate it was handed.
    let voice = lib
        .create(&wav(44_100, 2.0), "  My voice  ", Some("  hello there  "))
        .expect("a plain WAV is accepted");

    assert_eq!(voice.sample_rate, CLIP_SAMPLE_RATE);
    assert_eq!(voice.channels, 1);
    assert_eq!(voice.label, "My voice", "label is trimmed");
    assert_eq!(voice.transcript.as_deref(), Some("hello there"));
    assert!(
        (voice.duration_seconds - 2.0).abs() < 0.05,
        "two seconds survives the resample: {}",
        voice.duration_seconds
    );
    assert!(
        voice.voice_id().starts_with("voice:"),
        "the wire id carries the prefix: {}",
        voice.voice_id()
    );
    assert!(uuid::Uuid::parse_str(&voice.id).is_ok(), "the id is a uuid");
}

/// A voice with no transcript is legal — only in-context cloning needs one,
/// and the model that does declares it.
#[test]
fn a_transcript_is_optional() {
    let (_guard, lib) = library();
    let voice = lib
        .create(&wav(24_000, 0.5), "No words", None)
        .expect("audio alone is enough");
    assert!(voice.transcript.is_none());
    assert_eq!(lib.get(&voice.id).expect("round-trips").id, voice.id);
}

#[test]
fn lists_newest_first_and_deletes() {
    let (_guard, lib) = library();
    assert!(
        lib.list().expect("an untouched library lists").is_empty(),
        "a daemon that never cloned a voice has no library to read"
    );

    let first = lib.create(&wav(24_000, 0.2), "First", None).unwrap();
    // `created_at` has sub-second resolution, but two creations in the same
    // microsecond would make the order arbitrary; the sleep keeps the
    // assertion about ordering rather than about the clock.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = lib.create(&wav(24_000, 0.2), "Second", None).unwrap();

    let listed = lib.list().expect("lists");
    assert_eq!(
        listed.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
        vec![second.id.as_str(), first.id.as_str()],
        "newest first"
    );

    lib.delete(&first.id).expect("deletes");
    assert!(matches!(lib.get(&first.id), Err(VoiceError::NotFound)));
    assert_eq!(lib.list().unwrap().len(), 1, "the other voice is untouched");
    assert!(
        matches!(lib.delete(&first.id), Err(VoiceError::NotFound)),
        "deleting twice is a miss, not a silent success"
    );
}

/// The library keeps the whole recording and each model takes the prefix it
/// declares, so switching models never asks the user to record again.
#[test]
fn trims_the_clip_to_the_model_s_budget() {
    let (_guard, lib) = library();
    let voice = lib.create(&wav(24_000, 4.0), "Long", None).unwrap();

    let whole = lib.clip_pcm(&voice.id, None).expect("untrimmed");
    assert!((whole.seconds - 4.0).abs() < 0.05, "{}", whole.seconds);

    let trimmed = lib.clip_pcm(&voice.id, Some(1.5)).expect("trimmed");
    assert!((trimmed.seconds - 1.5).abs() < 0.05, "{}", trimmed.seconds);
    assert_eq!(
        trimmed.pcm.len(),
        crate::num_cast::f32_to_usize(1.5 * crate::num_cast::u32_to_f32(CLIP_SAMPLE_RATE)) * 2,
        "two bytes per mono s16 sample"
    );
    assert_eq!(
        &trimmed.pcm[..],
        &whole.pcm[..trimmed.pcm.len()],
        "the trim is a prefix of the recording"
    );

    // A budget the recording does not reach leaves it whole rather than
    // padding to it.
    let generous = lib
        .clip_pcm(&voice.id, Some(30.0))
        .expect("generous budget");
    assert_eq!(generous.pcm.len(), whole.pcm.len());
}

#[test]
fn renames_without_touching_the_audio() {
    let (_guard, lib) = library();
    let voice = lib
        .create(&wav(24_000, 0.5), "Before", Some("said this"))
        .unwrap();
    let before = lib.clip_pcm(&voice.id, None).unwrap();

    let renamed = lib.rename(&voice.id, "After").expect("renames");
    assert_eq!(renamed.label, "After");
    assert_eq!(
        renamed.transcript.as_deref(),
        Some("said this"),
        "the transcript describes the clip and survives a rename"
    );
    assert_eq!(renamed.created_at, voice.created_at);
    assert_eq!(lib.clip_pcm(&voice.id, None).unwrap().pcm, before.pcm);
}

#[test]
fn refuses_input_it_cannot_use() {
    let (_guard, lib) = library();
    let good = wav(24_000, 0.2);

    assert!(
        matches!(
            lib.create(&good, "   ", None),
            Err(VoiceError::InvalidValue(_))
        ),
        "a voice needs a name"
    );
    assert!(
        matches!(
            lib.create(&good, &"x".repeat(super::MAX_LABEL_CHARS + 1), None),
            Err(VoiceError::InvalidValue(_))
        ),
        "labels are bounded"
    );
    assert!(
        matches!(
            lib.create(
                &good,
                "ok",
                Some(&"x".repeat(super::MAX_TRANSCRIPT_CHARS + 1))
            ),
            Err(VoiceError::InvalidValue(_))
        ),
        "transcripts are bounded"
    );
    assert!(
        matches!(
            lib.create(b"not audio", "ok", None),
            Err(VoiceError::Audio(_))
        ),
        "the bytes have to be a WAV"
    );
    assert!(
        matches!(
            lib.create(&wav(24_000, MAX_CLIP_SECONDS + 5.0), "ok", None),
            Err(VoiceError::Audio(_))
        ),
        "a clip past the archive bound is refused"
    );
    assert!(
        lib.list().expect("lists").is_empty(),
        "nothing refused left a directory behind"
    );
}

/// Ids come from a client, so a non-uuid must never reach a path join. The
/// answer is `NotFound` rather than a distinct code: a probe learns nothing
/// from it.
#[test]
fn a_bogus_id_cannot_escape_the_library_directory() {
    let (_guard, lib) = library();
    for id in [
        "../../etc/passwd",
        "..",
        "voice:../secrets",
        "",
        "not-a-uuid",
        "0000",
    ] {
        assert!(
            matches!(lib.get(id), Err(VoiceError::NotFound)),
            "get({id:?}) must be a miss"
        );
        assert!(
            matches!(lib.delete(id), Err(VoiceError::NotFound)),
            "delete({id:?}) must be a miss"
        );
        assert!(
            matches!(lib.clip_wav(id), Err(VoiceError::NotFound)),
            "clip_wav({id:?}) must be a miss"
        );
    }
}

/// The `voice:` prefix is how the id travels on `/speak`, so the library
/// accepts it either way rather than making every caller strip it.
#[test]
fn accepts_an_id_with_or_without_its_prefix() {
    let (_guard, lib) = library();
    let voice = lib.create(&wav(24_000, 0.2), "Either", None).unwrap();
    assert_eq!(lib.get(&voice.voice_id()).expect("prefixed").id, voice.id);
    assert_eq!(lib.get(&voice.id).expect("bare").id, voice.id);
}

/// Reference clips are recordings of a person; they are not world-readable.
#[test]
fn stores_clips_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let (_guard, lib) = library();
    let voice = lib.create(&wav(24_000, 0.2), "Private", None).unwrap();
    let clip = lib.dir().join(&voice.id).join("clip.wav");
    let mode = std::fs::metadata(&clip).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "clip.wav is owner-only, got {mode:o}");
}
