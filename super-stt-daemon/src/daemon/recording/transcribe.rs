// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::dispatch::{DispatchError, dispatch_transcription};
use anyhow::{Context, Result};
use log::{debug, error, info, warn};

impl SuperSTTDaemon {
    /// Transcribe a chunk of audio data for preview
    pub(super) async fn transcribe_audio_chunk(
        &self,
        audio_data: &[f32],
        request_language: Option<&str>,
    ) -> Result<String> {
        debug!(
            "Processing {} samples for preview transcription",
            audio_data.len()
        );

        // Basic validation of audio data
        if audio_data.is_empty() {
            debug!("Audio data is empty, skipping transcription");
            return Ok(String::new());
        }

        // Check audio length - need at least 1 second of audio for decent transcription
        if audio_data.len() < 16000 {
            debug!(
                "Audio data too short ({} samples), skipping transcription",
                audio_data.len()
            );
            return Ok(String::new());
        }

        // Process audio
        let processed_audio = self
            .audio_processor
            .process_audio(audio_data, 16000)
            .context("Failed to process audio chunk")?;

        debug!(
            "Audio processing complete, processed {} samples",
            processed_audio.len()
        );

        let language = self.resolve_active_language(request_language).await;

        // Preview is best-effort: a backend failure or missing model yields an
        // empty string rather than surfacing an error to the recording flow.
        match dispatch_transcription(&self.model, processed_audio, 16000, language).await {
            Ok(text) => Ok(text),
            Err(DispatchError::Failed(e)) => {
                warn!("Preview transcription failed, continuing: {e}");
                Ok(String::new())
            }
            Err(DispatchError::NotLoaded) => {
                warn!("Model not loaded for preview transcription");
                Ok(String::new())
            }
            Err(DispatchError::Join(e)) => {
                Err(anyhow::anyhow!("Preview transcription task failed: {e}"))
            }
        }
    }

    /// Run the final transcription pass over the full recording. Unlike the
    /// best-effort preview path, a backend failure here surfaces to the caller
    /// as an `Err` (it must not look like silence).
    pub(super) async fn transcribe_final(
        &self,
        audio_data: &[f32],
        request_language: Option<&str>,
    ) -> Result<String> {
        let processed_audio = self
            .audio_processor
            .process_audio(audio_data, 16000)
            .context("Failed to process audio")?;

        let language = self.resolve_active_language(request_language).await;
        let start_time = std::time::Instant::now();

        match dispatch_transcription(&self.model, processed_audio, 16000, language).await {
            Ok(text) => {
                info!(
                    "Transcription completed in {:?}: '{text}'",
                    start_time.elapsed()
                );
                Ok(text)
            }
            Err(DispatchError::Failed(e)) => {
                warn!("Transcription failed: {e}");
                Err(e)
            }
            Err(DispatchError::NotLoaded) => {
                error!("Model not loaded");
                Err(anyhow::anyhow!("Model not loaded"))
            }
            Err(DispatchError::Join(e)) => Err(anyhow::anyhow!("Transcription task failed: {e}")),
        }
    }
}
