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

    /// The reason, as one string. `{:#}` so an `anyhow` chain reports its causes
    /// and not just the outermost context.
    pub(super) fn detail(&self) -> String {
        format!("{:#}", self.error)
    }
}

/// Who authored a dispatch failure's message.
///
/// `Failed` is the backend's own answer — it was asked and it refused. The other
/// two are the daemon never getting as far as asking: an empty model slot, or an
/// inference task that did not come back.
///
/// Separated from [`SuperSTTDaemon::transcribe_final`] because this is the
/// decision the user-facing notice reads, and that function needs a whole daemon
/// to call. A decision worth showing the user is worth being able to test.
fn origin_of(error: &DispatchError) -> Origin {
    match error {
        DispatchError::Failed(_) => Origin::Backend,
        DispatchError::NotLoaded | DispatchError::Join(_) => Origin::Daemon,
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
            // One place decides the origin, so the notice cannot disagree with
            // the variant; the arms below only shape the message and log it.
            Err(e) => {
                let origin = origin_of(&e);
                let error = match e {
                    DispatchError::Failed(err) => {
                        warn!("Transcription failed: {err}");
                        err
                    }
                    DispatchError::NotLoaded => {
                        error!("Model not loaded");
                        anyhow::anyhow!("Model not loaded")
                    }
                    DispatchError::Join(err) => {
                        anyhow::anyhow!("Transcription task failed: {err}")
                    }
                };
                Err(FinalFailure { origin, error })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FinalFailure, origin_of};
    use crate::output::notice::Origin;
    use crate::stt_models::dispatch::DispatchError;

    /// The backend was asked and refused, so its message is its own and the
    /// notice says so.
    #[test]
    fn a_backend_refusal_is_the_backends_own() {
        let e = DispatchError::Failed(anyhow::anyhow!("write_failed"));
        assert_eq!(origin_of(&e), Origin::Backend);
    }

    /// Nothing was asked: there was no model to ask. Labelling this as the
    /// backend's would blame a backend for the daemon's own state.
    #[test]
    fn an_empty_model_slot_is_the_daemons() {
        assert_eq!(origin_of(&DispatchError::NotLoaded), Origin::Daemon);
    }

    /// The inference task died, so whatever the backend would have said never
    /// arrived. The message here is the daemon's own description of the crash.
    #[tokio::test]
    async fn a_dead_inference_task_is_the_daemons() {
        let handle = tokio::spawn(std::future::pending::<()>());
        handle.abort();
        let join_error = handle.await.expect_err("an aborted task fails to join");
        assert_eq!(origin_of(&DispatchError::Join(join_error)), Origin::Daemon);
    }

    /// The reason shown to the user expands the whole `anyhow` chain. With the
    /// plain `{}` form the user would get "Failed to process audio" and no clue
    /// what about it failed.
    #[test]
    fn the_reason_expands_the_cause_chain() {
        let e = FinalFailure::daemon(
            anyhow::anyhow!("rate mismatch").context("Failed to process audio"),
        );
        assert_eq!(e.detail(), "Failed to process audio: rate mismatch");
        assert_eq!(
            e.to_string(),
            "Failed to process audio",
            "the plain form is what the existing error text keeps using"
        );
    }
}
