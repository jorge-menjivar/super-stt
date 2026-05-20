// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use log::warn;

use super_stt_shared::audio_utils::{
    ResampleQuality, apply_pre_emphasis, normalize_audio, resample,
};

pub struct AudioProcessor;

impl Default for AudioProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioProcessor {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Process raw audio data for Whisper model input
    ///
    /// # Errors
    ///
    /// Returns an error if the audio data is invalid.
    pub fn process_audio(&self, audio_data: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
        // Ensure audio data is in the correct range (-1 to 1)
        let mut processed = audio_data.to_vec();
        normalize_audio(&mut processed);

        // Resample to 16kHz if needed (Whisper expects 16kHz)
        if sample_rate != 16000 {
            warn!("Audio sample rate is {sample_rate}Hz, resampling to 16kHz");
            processed = resample(&processed, sample_rate, 16000, ResampleQuality::Fast)?;
        }

        // Apply pre-emphasis filter (common in speech processing)
        apply_pre_emphasis(&mut processed);

        // Ensure minimum length for processing
        if processed.len() < 1600 {
            // 0.1 seconds at 16kHz
            warn!("Audio too short ({} samples), padding", processed.len());
            processed.resize(1600, 0.0);
        }

        Ok(processed)
    }

    /// Convert audio data to WAV format for debugging/testing
    ///
    /// # Errors
    ///
    /// Returns an error if the audio data cannot be converted to WAV format.
    #[allow(clippy::cast_possible_truncation)]
    pub fn audio_to_wav(&self, audio: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
        use std::io::{Cursor, Write};

        let mut cursor = Cursor::new(Vec::new());

        // WAV header
        cursor.write_all(b"RIFF")?;
        cursor.write_all(&(36 + audio.len() * 2).to_le_bytes())?; // File size - 8
        cursor.write_all(b"WAVE")?;

        // Format chunk
        cursor.write_all(b"fmt ")?;
        cursor.write_all(&16u32.to_le_bytes())?; // Format chunk size
        cursor.write_all(&1u16.to_le_bytes())?; // PCM format
        cursor.write_all(&1u16.to_le_bytes())?; // Mono
        cursor.write_all(&sample_rate.to_le_bytes())?;
        cursor.write_all(&(sample_rate * 2).to_le_bytes())?; // Byte rate
        cursor.write_all(&2u16.to_le_bytes())?; // Block align
        cursor.write_all(&16u16.to_le_bytes())?; // Bits per sample

        // Data chunk
        cursor.write_all(b"data")?;
        cursor.write_all(&(audio.len() * 2).to_le_bytes())?; // Data chunk size

        // Convert f32 samples to i16
        for &sample in audio {
            let sample_i16 = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            cursor.write_all(&sample_i16.to_le_bytes())?;
        }

        Ok(cursor.into_inner())
    }
}
