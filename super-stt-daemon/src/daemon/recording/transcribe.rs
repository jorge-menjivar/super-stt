// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::output::preview::Typer;
use crate::stt_models::dispatch::{DispatchError, dispatch_transcription};
use anyhow::{Context, Result};
use log::{debug, error, info, warn};

impl SuperSTTDaemon {
    /// Transcribe a chunk of audio data for preview
    pub(super) async fn transcribe_audio_chunk(&self, audio_data: &[f32]) -> Result<String> {
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

        let language = self.resolve_active_language().await;

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

    /// Transcribe audio with spinner if needed
    pub(super) async fn transcribe_with_spinner(
        &self,
        _typer: &mut Typer,
        audio_data: &[f32],
        _write_mode: bool,
    ) -> Result<String> {
        // If we'll type the result, show a simple spinner by typing characters and backspacing
        // This indicates work while transcription runs.
        let mut spinner_handle: Option<tokio::task::JoinHandle<()>> = None;
        let spinner_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Track how many temporary spinner characters are visible (0-3)
        let _visible_temp_chars = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Disable loader for now since it interferes with keyboard
        // TODO: Implement proper loader that doesn't conflict with final typing

        // Process audio
        let processed_audio = self
            .audio_processor
            .process_audio(audio_data, 16000)
            .context("Failed to process audio")?;

        // Transcribe the audio.
        let language = self.resolve_active_language().await;
        let start_time = std::time::Instant::now();

        // The final pass surfaces backend errors to the caller — unlike the
        // best-effort preview path, a failure here must not look like silence.
        let transcription_result =
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
                Err(DispatchError::Join(e)) => {
                    Err(anyhow::anyhow!("Transcription task failed: {e}"))
                }
            };

        // Stop spinner if it was started
        if let Some(handle) = spinner_handle.take() {
            spinner_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            // Wait for the spinner task to exit and clean up
            if let Err(e) = handle.await {
                warn!("Spinner task panicked: {e}");
            }
        }

        transcription_result
    }
}
