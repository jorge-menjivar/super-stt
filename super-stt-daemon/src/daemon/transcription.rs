// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use chrono::Utc;
use log::{debug, error, info, warn};
use std::sync::Arc;
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::utils::audio::validate_audio;

impl SuperSTTDaemon {
    /// Handle transcribe command
    pub async fn handle_transcribe(
        &self,
        audio_data: Vec<f32>,
        sample_rate: u32,
        client_id: String,
    ) -> DaemonResponse {
        info!("Processing transcription request from client: {client_id}");

        // Validate audio
        if let Err(e) = validate_audio(&audio_data, sample_rate) {
            warn!("Audio validation failed: {e}");
            return DaemonResponse::error(&format!("Invalid audio data: {e}"));
        }
        debug!("Audio validation completed");

        // Calculate audio level for visualization and emit D-Bus audio level signal
        let audio_level = self
            .compute_audio_level_and_notify(&audio_data, &client_id)
            .await;
        debug!("Audio level calculated: {audio_level:.3}");

        // Emit D-Bus transcription started signal
        self.emit_transcription_started(&audio_data, sample_rate, &client_id)
            .await;

        // Process audio
        let processed_audio = match self.audio_processor.process_audio(&audio_data, sample_rate) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to process audio: {e}");
                return DaemonResponse::error(&format!("Failed to process audio: {e}"));
            }
        };

        // Run the model inference (online or blocking path)
        let transcription_result = self.run_transcription(processed_audio).await;

        // Handle the result of the transcription task
        match transcription_result {
            Ok(Ok((transcription, duration))) => {
                self.emit_transcription_completed(&client_id, &transcription, duration)
                    .await;
                DaemonResponse::success().with_transcription(transcription)
            }
            Ok(Err(e)) => DaemonResponse::error(&format!("Transcription failed: {e}")),
            Err(e) => {
                error!("Transcription task failed: {e}");
                DaemonResponse::error(&format!("Task execution failed: {e}"))
            }
        }
    }

    /// Calculate RMS audio level and emit a D-Bus audio-level signal when a
    /// D-Bus manager is present. Returns the RMS level (0.0 for empty audio).
    async fn compute_audio_level_and_notify(&self, audio_data: &[f32], client_id: &str) -> f32 {
        if audio_data.is_empty() {
            return 0.0;
        }

        let rms: f32 = (audio_data.iter().map(|&x| x * x).sum::<f32>()
            / crate::num_cast::usize_to_f32(audio_data.len()))
        .sqrt();
        let is_speech = rms > 0.02; // Use same threshold as client

        if let Some(ref dbus_manager) = self.dbus_manager {
            let audio_level_event = crate::services::dbus::AudioLevelEvent {
                client_id: client_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
                level: rms,
                is_speech,
            };

            if let Err(e) = dbus_manager.emit_audio_level(audio_level_event).await {
                warn!("Failed to emit D-Bus audio_level signal: {e}");
            } else {
                debug!(
                    "Emitted D-Bus audio_level signal for client: {client_id}, level: {rms:.3}, speech: {is_speech}"
                );
            }
        }

        rms
    }

    /// Emit a D-Bus `transcription_started` signal when a D-Bus manager is present.
    async fn emit_transcription_started(
        &self,
        audio_data: &[f32],
        sample_rate: u32,
        client_id: &str,
    ) {
        if let Some(ref dbus_manager) = self.dbus_manager {
            let event = crate::services::dbus::TranscriptionStartedEvent {
                client_id: client_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
                audio_length_ms: (crate::num_cast::usize_to_f64(audio_data.len())
                    / f64::from(sample_rate))
                    * 1000.0,
                sample_rate,
            };

            if let Err(e) = dbus_manager.emit_transcription_started(event).await {
                warn!("Failed to emit D-Bus transcription_started signal: {e}");
            } else {
                debug!("Emitted D-Bus transcription_started signal for client: {client_id}");
            }
        }
    }

    /// Run the model inference, dispatching to the async (online) or blocking
    /// (local) path based on whether the loaded model is an online API model.
    ///
    /// Returns `Ok(Ok((text, duration)))` on success, `Ok(Err(_))` when the
    /// model is not loaded, or `Err(_)` if the blocking task itself panicked.
    async fn run_transcription(
        &self,
        processed_audio: Vec<f32>,
    ) -> Result<Result<(String, std::time::Duration), anyhow::Error>, tokio::task::JoinError> {
        let model_clone = Arc::clone(&self.model);

        let is_online = {
            let guard = model_clone.read().await;
            guard
                .as_ref()
                .is_some_and(|loaded| loaded.instance.is_online())
        };

        if is_online {
            // Async path for online models (API call)
            let start_time = std::time::Instant::now();
            let mut model_guard = model_clone.write().await;
            if let Some(loaded) = model_guard.as_mut() {
                match loaded
                    .instance
                    .transcribe_audio(&processed_audio, 16000)
                    .await
                {
                    Ok(text) => {
                        let duration = start_time.elapsed();
                        info!("Online transcription completed in {duration:?}: '{text}'");
                        Ok(Ok((text, duration)))
                    }
                    Err(e) => {
                        warn!("Online transcription failed, returning empty result: {e}");
                        let duration = start_time.elapsed();
                        Ok(Ok((String::new(), duration)))
                    }
                }
            } else {
                error!("Model not loaded");
                Ok(Err(anyhow::anyhow!("Model not loaded")))
            }
        } else {
            // Blocking path for local models
            tokio::task::spawn_blocking(move || {
                let handle = tokio::runtime::Handle::current();
                let start_time = std::time::Instant::now();
                let mut model_guard = model_clone.blocking_write();

                if let Some(loaded) = model_guard.as_mut() {
                    match handle.block_on(loaded.instance.transcribe_audio(&processed_audio, 16000))
                    {
                        Ok(text) => {
                            let duration = start_time.elapsed();
                            info!("Transcription completed in {duration:?}: '{text}'");
                            Ok((text, duration))
                        }
                        Err(e) => {
                            warn!("Transcription failed, returning empty result: {e}");
                            let duration = start_time.elapsed();
                            Ok((String::new(), duration))
                        }
                    }
                } else {
                    error!("Model not loaded");
                    Err(anyhow::anyhow!("Model not loaded"))
                }
            })
            .await
        }
    }

    /// Emit a D-Bus `transcription_completed` signal when a D-Bus manager is present.
    async fn emit_transcription_completed(
        &self,
        client_id: &str,
        transcription: &str,
        duration: std::time::Duration,
    ) {
        if let Some(ref dbus_manager) = self.dbus_manager {
            let event = crate::services::dbus::TranscriptionCompletedEvent {
                client_id: client_id.to_owned(),
                timestamp: Utc::now().to_rfc3339(),
                transcription: transcription.to_owned(),
                duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
            };

            if let Err(e) = dbus_manager.emit_transcription_completed(event).await {
                warn!("Failed to emit D-Bus transcription_completed signal: {e}");
            } else {
                debug!("Emitted D-Bus transcription_completed signal for client: {client_id}");
            }
        }
    }
}
