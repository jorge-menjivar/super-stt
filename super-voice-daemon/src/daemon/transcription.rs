// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::dispatch::{DispatchError, dispatch_transcription};
use log::{debug, error, info, warn};
use super_voice_shared::models::protocol::{DaemonResponse, ErrorCode};
use super_voice_shared::utils::audio::validate_audio;

/// Distinguishes a genuine backend failure from "no model is loaded" so
/// [`SuperSTTDaemon::handle_transcribe`] can attach
/// [`ErrorCode::ModelNotLoaded`] only to the latter — the former has no
/// better code than the default uncoded (`500`) error.
enum RunTranscriptionError {
    /// The backend itself failed to produce a transcription.
    Failed(anyhow::Error),
    /// No model is loaded; the request is well-formed and will succeed once
    /// one is (see `docs/protocol/endpoints/v1/transcribe.md`).
    NotLoaded,
}

impl SuperSTTDaemon {
    /// Handle transcribe command
    pub async fn handle_transcribe(
        &self,
        audio_data: Vec<f32>,
        sample_rate: u32,
        client_id: String,
        language: Option<String>,
    ) -> DaemonResponse {
        info!("Processing transcription request from client: {client_id}");

        // Validate audio
        if let Err(e) = validate_audio(&audio_data, sample_rate) {
            warn!("Audio validation failed: {e}");
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidValue,
                &format!("Invalid audio data: {e}"),
            );
        }
        debug!("Audio validation completed");

        // A load that fails is logged; this answer's code says only that no
        // model is loaded, which the inference below reports.
        let _ = self.ensure_model_loaded().await;

        // Process audio
        let processed_audio = match self.audio_processor.process_audio(&audio_data, sample_rate) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to process audio: {e}");
                return DaemonResponse::error(&format!("Failed to process audio: {e}"));
            }
        };

        // Run the model inference (online or blocking path)
        let transcription_result = self
            .run_transcription(processed_audio, language.as_deref())
            .await;

        // Handle the result of the transcription task
        match transcription_result {
            Ok(Ok((transcription, _duration))) => {
                DaemonResponse::success().with_transcription(transcription)
            }
            // No model loaded: coded so this pre-captured path carries the
            // same `error_code` as the daemon-mic paths (see
            // docs/protocol/endpoints/v1/transcribe.md). `message` matches the
            // literal identifier every other coded error on this endpoint uses
            // (`recording_in_progress`, `stream_realtime_with_audio_data`, …);
            // `error_code` — not `message` — is the field clients switch on.
            Ok(Err(RunTranscriptionError::NotLoaded)) => {
                DaemonResponse::error_with_code(ErrorCode::ModelNotLoaded, "model_not_loaded")
            }
            Ok(Err(RunTranscriptionError::Failed(e))) => {
                DaemonResponse::error(&format!("Transcription failed: {e}"))
            }
            Err(e) => {
                error!("Transcription task failed: {e}");
                DaemonResponse::error(&format!("Task execution failed: {e}"))
            }
        }
    }

    /// Resolve the effective language for the currently-loaded model.
    /// `request_override` is an optional per-request language (BCP-47 or
    /// `"auto"`) from an external `POST /v1/stt/transcribe`. Per the
    /// transcription-language design it takes precedence over all configured
    /// resolution and is **passed straight through** — the backend validates it
    /// (it is not re-adapted against the model's supported set). When it is
    /// `None`, fall back to the configured resolution (per-model language →
    /// primary language → model default). A returned `None` means "omit
    /// `language`" (the model uses its primary).
    pub(crate) async fn resolve_active_language(
        &self,
        request_override: Option<&str>,
    ) -> Option<String> {
        if let Some(lang) = request_override {
            return Some(lang.to_string());
        }
        let def = {
            let guard = self.model.read().await;
            guard.as_ref().map(|loaded| loaded.definition.clone())?
        };
        let config = self.config.read().await;
        crate::daemon::language::resolve_language(
            def.is_multilingual,
            config.model_language(&def.source, &def.name),
            config.primary_language(),
            &def.supported_languages,
        )
        .wire
    }

    /// Run the model inference, dispatching to the async (online) or blocking
    /// (local) path based on whether the loaded model is an online API model.
    ///
    /// Returns `Ok(Ok((text, duration)))` on success, `Ok(Err(_))` when the
    /// backend failed or no model is loaded (distinguished by
    /// [`RunTranscriptionError`] so the caller can attach the right
    /// [`ErrorCode`]), or `Err(_)` if the blocking task itself panicked.
    async fn run_transcription(
        &self,
        processed_audio: Vec<f32>,
        request_language: Option<&str>,
    ) -> Result<Result<(String, std::time::Duration), RunTranscriptionError>, tokio::task::JoinError>
    {
        let language = self.resolve_active_language(request_language).await;
        let start_time = std::time::Instant::now();

        match dispatch_transcription(&self.model, processed_audio, 16000, language.clone()).await {
            Ok(text) => {
                let duration = start_time.elapsed();
                info!("Transcription completed in {duration:?}");
                // Post-processing is part of producing the final transcript, so
                // it lands inside the reported duration. Best-effort: this
                // returns the raw text unchanged when it is off or fails. It
                // also logs the raw text and the processed one side by side.
                let text = self.post_process_final(text, language).await;
                Ok(Ok((text, start_time.elapsed())))
            }
            // A real backend failure is surfaced, not masked as empty success —
            // "no speech" is an `Ok("")` from the backend, so `Failed` here is a
            // genuine error the caller must report.
            Err(DispatchError::Failed(e)) => {
                warn!("Transcription failed: {e}");
                Ok(Err(RunTranscriptionError::Failed(anyhow::anyhow!(
                    "Transcription failed: {e}"
                ))))
            }
            Err(DispatchError::NotLoaded) => {
                error!("Model not loaded");
                Ok(Err(RunTranscriptionError::NotLoaded))
            }
            Err(DispatchError::Join(e)) => Err(e),
        }
    }
}
