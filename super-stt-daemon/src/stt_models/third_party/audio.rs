// SPDX-License-Identifier: GPL-3.0-only

//! Shared audio utilities for third-party API clients.

use anyhow::{Context, Result};

/// Encode f32 audio samples as a WAV file in memory (16-bit PCM, mono).
///
/// # Errors
///
/// Returns an error if the WAV writer fails to create or finalize.
pub fn encode_wav_in_memory(audio_data: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer =
        hound::WavWriter::new(&mut cursor, spec).context("Failed to create WAV writer")?;
    for &sample in audio_data {
        let clamped = sample.clamp(-1.0, 1.0);
        // Truncation is intentional: converting f32 audio [-1.0, 1.0] to 16-bit PCM
        #[allow(clippy::cast_possible_truncation)]
        let int_sample = (clamped * f32::from(i16::MAX)) as i16;
        writer.write_sample(int_sample)?;
    }
    writer.finalize()?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_wav_produces_valid_output() {
        let samples = vec![0.0_f32, 0.5, -0.5, 1.0, -1.0];
        let wav = encode_wav_in_memory(&samples, 16000).expect("WAV encoding should succeed");
        // WAV header is 44 bytes, plus 5 samples * 2 bytes each = 54 bytes total
        assert_eq!(wav.len(), 44 + 5 * 2);
        // Check RIFF header
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }

    #[test]
    fn encode_wav_empty_audio() {
        let wav = encode_wav_in_memory(&[], 16000).expect("WAV encoding should succeed");
        assert_eq!(wav.len(), 44); // header only
    }

    #[test]
    fn encode_wav_clamps_out_of_range_samples() {
        // Samples beyond [-1.0, 1.0] should be clamped, not cause errors
        let samples = vec![-2.0_f32, 2.0, 100.0, -100.0];
        let wav = encode_wav_in_memory(&samples, 16000).expect("should succeed with out-of-range");
        assert_eq!(wav.len(), 44 + 4 * 2);
    }

    #[test]
    fn encode_wav_different_sample_rates() {
        let samples = vec![0.0_f32; 100];
        for rate in [8000, 16000, 44100, 48000] {
            let wav = encode_wav_in_memory(&samples, rate)
                .unwrap_or_else(|_| panic!("should encode at {rate}Hz"));
            assert_eq!(wav.len(), 44 + 100 * 2);
        }
    }

    #[test]
    fn encode_wav_is_valid_wav_format() {
        let samples = vec![0.5_f32; 16000]; // 1 second of audio
        let wav = encode_wav_in_memory(&samples, 16000).expect("should succeed");

        // Verify WAV format markers
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");

        // Verify data chunk exists
        // "data" marker is at byte 36 in standard WAV
        assert_eq!(&wav[36..40], b"data");
    }
}
