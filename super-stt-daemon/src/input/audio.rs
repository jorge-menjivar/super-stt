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
}
