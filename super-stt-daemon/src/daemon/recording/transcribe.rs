// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::output::notice::Origin;
use crate::stt_models::dispatch::{DispatchError, dispatch_transcription};
use anyhow::{Context, Result};
use log::{debug, error, info, warn};

/// A final-transcription failure, tagged with who authored the message.
///
/// The error alone cannot answer that: a backend refusing the audio and the
/// daemon failing to resample it arrive at the same `Err`, and the notice the
/// user sees says which it was.
pub(super) struct FinalFailure {
    pub(super) origin: Origin,
    pub(super) error: anyhow::Error,
}

impl FinalFailure {
    /// The daemon's own doing: audio processing, an empty model slot, a task
    /// that did not come back.
    fn daemon(error: anyhow::Error) -> Self {
        Self {
            origin: Origin::Daemon,
            error,
        }
    }

    /// The backend answered and said no.
    fn backend(error: anyhow::Error) -> Self {
        Self {
            origin: Origin::Backend,
            error,
        }
    }

    /// The reason, as one string. `{:#}` so an `anyhow` chain reports its causes
    /// and not just the outermost context.
    pub(super) fn detail(&self) -> String {
        format!("{:#}", self.error)
    }
}

impl std::fmt::Display for FinalFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The caller's existing error text is the plain form; `detail()` is the
        // one that expands the chain.
        write!(f, "{}", self.error)
    }
}

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
    ) -> std::result::Result<String, FinalFailure> {
        let processed_audio = self
            .audio_processor
            .process_audio(audio_data, 16000)
            .context("Failed to process audio")
            .map_err(FinalFailure::daemon)?;

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
                Err(FinalFailure::backend(e))
            }
            Err(DispatchError::NotLoaded) => {
                error!("Model not loaded");
                Err(FinalFailure::daemon(anyhow::anyhow!("Model not loaded")))
            }
            Err(DispatchError::Join(e)) => Err(FinalFailure::daemon(anyhow::anyhow!(
                "Transcription task failed: {e}"
            ))),
        }
    }
}
