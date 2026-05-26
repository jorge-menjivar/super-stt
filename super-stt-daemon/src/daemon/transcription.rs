// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::types::SuperSTTDaemon;
use chrono::Utc;
use log::{debug, error, info, warn};
use std::sync::Arc;
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::utils::audio::validate_audio;

impl SuperSTTDaemon {
    /// Handle transcribe command
    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
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

        // Calculate audio level for visualization
        let audio_level = if audio_data.is_empty() {
            0.0
        } else {
            let rms: f32 =
                (audio_data.iter().map(|&x| x * x).sum::<f32>() / audio_data.len() as f32).sqrt();
            let is_speech = rms > 0.02; // Use same threshold as client

            // Emit D-Bus audio level signal
            if let Some(ref dbus_manager) = self.dbus_manager {
                let audio_level_event = crate::services::dbus::AudioLevelEvent {
                    client_id: client_id.clone(),
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
        };

        debug!("Audio level calculated: {audio_level:.3}");

        // Emit D-Bus transcription started signal
        if let Some(ref dbus_manager) = self.dbus_manager {
            let event = crate::services::dbus::TranscriptionStartedEvent {
                client_id: client_id.clone(),
                timestamp: Utc::now().to_rfc3339(),
                audio_length_ms: (audio_data.len() as f64 / f64::from(sample_rate)) * 1000.0,
                sample_rate,
            };

            if let Err(e) = dbus_manager.emit_transcription_started(event).await {
                warn!("Failed to emit D-Bus transcription_started signal: {e}");
            } else {
                debug!("Emitted D-Bus transcription_started signal for client: {client_id}");
            }
        }

        // Process audio
        let processed_audio = match self.audio_processor.process_audio(&audio_data, sample_rate) {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to process audio: {e}");
                return DaemonResponse::error(&format!("Failed to process audio: {e}"));
            }
        };

        // Clone the model Arc for the transcription task
        let model_clone = Arc::clone(&self.model);

        // Check if the model is online to choose async vs blocking path
        let is_online = {
            let guard = model_clone.read().await;
            guard
                .as_ref()
                .is_some_and(|loaded| loaded.instance.is_online())
        };

        let transcription_result = if is_online {
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
        };

        // Handle the result of the blocking task
        match transcription_result {
            Ok(Ok((transcription, duration))) => {
                // Emit D-Bus transcription completed signal
                if let Some(ref dbus_manager) = self.dbus_manager {
                    let event = crate::services::dbus::TranscriptionCompletedEvent {
                        client_id: client_id.clone(),
                        timestamp: Utc::now().to_rfc3339(),
                        transcription: transcription.clone(),
                        duration_ms: u64::try_from(duration.as_millis()).unwrap_or(u64::MAX),
                    };

                    if let Err(e) = dbus_manager.emit_transcription_completed(event).await {
                        warn!("Failed to emit D-Bus transcription_completed signal: {e}");
                    } else {
                        debug!(
                            "Emitted D-Bus transcription_completed signal for client: {client_id}"
                        );
                    }
                }

                DaemonResponse::success().with_transcription(transcription)
            }
            Ok(Err(e)) => DaemonResponse::error(&format!("Transcription failed: {e}")),
            Err(e) => {
                error!("Transcription task failed: {e}");
                DaemonResponse::error(&format!("Task execution failed: {e}"))
            }
        }
    }
}
