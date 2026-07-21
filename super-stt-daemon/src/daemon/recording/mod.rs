// SPDX-License-Identifier: GPL-3.0-only

mod preview;
mod transcribe;

use crate::daemon::types::SuperSTTDaemon;
use crate::output::notice;
use crate::services::dbus::ListeningEvent;
use crate::{audio::recorder::DaemonAudioRecorder, output::typer::Typer};
use anyhow::{Context, Result};
use chrono::Utc;
use log::{error, info, warn};
use std::collections::VecDeque;
use std::sync::Arc;
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use tokio::time::Instant;

/// `client_id` reported on every daemon-mic lifecycle event.
const CLIENT_ID: &str = "daemon_recorder";

/// Outputs from the recorder-spawn phase, consumed by the loop and collect phases.
struct RecordingSession {
    pub(super) recorder_handle: tokio::task::JoinHandle<Result<Vec<f32>>>,
    pub(super) model_processing_interval: std::time::Duration,
    pub(super) actually_typed: Arc<std::sync::Mutex<String>>,
    // Shared with the recorder's ring buffer (`get_audio_buffer_ref`), which is
    // a `parking_lot::Mutex` (Tier 3 #5).
    pub(super) preview_buffer: Arc<parking_lot::Mutex<VecDeque<f32>>>,
    pub(super) device_sample_rate: u32,
    pub(super) start_time: Instant,
}

impl SuperSTTDaemon {
    /// Internal record handling implementation
    pub async fn handle_record_internal(
        &self,
        typer: &mut Typer,
        write_mode: bool,
        stop_mode: RecordingStopMode,
        request_language: Option<&str>,
    ) -> DaemonResponse {
        // Check if already busy - prevent multiple simultaneous recordings
        {
            let busy_guard = self.busy.read().await;
            if *busy_guard {
                warn!("Recording request rejected - already recording");
                return DaemonResponse::error_with_code(
                    ErrorCode::RecordingInProgress,
                    "Recording already in progress. Please wait for current recording to complete.",
                );
            }
        }

        // Fail before capture. With no model loaded the cycle can only end in a
        // discarded recording, so starting the mic would cost the user a full
        // take — beeps, speech, and all — to learn nothing. In write mode the
        // reason lands in the field they are actually looking at.
        if self.model.read().await.is_none() {
            warn!("Recording request rejected - no model loaded");
            if write_mode {
                typer.type_notice(notice::NO_MODEL_LOADED).await;
            }
            return DaemonResponse::error_with_code(
                ErrorCode::ModelNotLoaded,
                "No model is loaded. Load a model and try again.",
            );
        }

        // Wait for recording to complete and return the transcription.
        match self
            .record_and_transcribe(typer, write_mode, stop_mode, request_language)
            .await
        {
            // Cycle completed successfully (empty text = no speech, still success).
            Ok(Ok(transcription)) => {
                if transcription.trim().is_empty() {
                    info!("🎤 Recording completed - No speech detected");
                    DaemonResponse::success()
                        .with_message("Recording completed - No speech detected".to_string())
                        .with_transcription(String::new())
                } else {
                    info!("🎤 Recording completed: '{transcription}'");
                    DaemonResponse::success()
                        .with_message("Recording completed successfully".to_string())
                        .with_transcription(transcription)
                }
            }
            // Capture/transcription failed after the cycle started. `finalize`
            // already cleared `busy` and emitted `transcribing_stopped`; surface
            // it as an error so the HTTP layer emits the `error` SSE event
            // instead of a `done` with error text as the transcription.
            Ok(Err(detail)) => {
                warn!("🎤 Recording cycle failed: {detail}");
                DaemonResponse::error(&detail)
            }
            // Setup failed before capture began: no `recording_started`/state(true)
            // went out, so just release the busy guard the setup claimed.
            Err(e) => {
                error!("🎤 Recording setup failed: {e}");
                {
                    let mut guard = self.busy.write().await;
                    *guard = false;
                }
                if write_mode {
                    typer.type_notice(notice::COULD_NOT_START_RECORDING).await;
                }
                DaemonResponse::error(&format!("Recording failed: {e}"))
            }
        }
    }

    /// Record audio directly in daemon and transcribe
    ///
    /// # Errors
    ///
    /// Returns an error if recording setup fails, audio processing fails,
    /// or if model execution encounters a fatal error.
    ///
    /// # Panics
    ///
    /// Panics if internal locks (e.g., audio theme or buffers) are poisoned.
    /// Run one record→transcribe cycle. The outer `Result` is a *setup* failure
    /// (before capture began, busy not yet consumed by a cycle); the inner
    /// `Result` is the cycle outcome — `Ok(text)` on success, `Err(detail)` when
    /// capture or transcription failed after the cycle started. A failure is
    /// never returned as `Ok` success text and is never typed into the user's
    /// window (finalize already reported it via `transcribing_stopped`).
    pub async fn record_and_transcribe(
        &self,
        typer: &mut Typer,
        write_mode: bool,
        stop_mode: RecordingStopMode,
        request_language: Option<&str>,
    ) -> Result<Result<String, String>> {
        info!("Starting direct audio recording in daemon with simplified architecture");

        // Phase 1: spawn the recorder. `busy` is set inside setup.
        let session = self.spawn_recorder(write_mode, stop_mode).await?;
        // Capture is starting — announce it now that the recorder exists.
        self.emit_recording_started(write_mode).await;

        // Phase 2: stream preview transcriptions while capturing.
        self.run_preview_loop(&session, typer, write_mode, request_language)
            .await;

        // Phase 3: await the recorder; the mic releases here.
        let full_audio_data = match self
            .collect_and_clear_preview(session, typer, write_mode)
            .await
        {
            Ok(data) => {
                self.emit_mic_stopped();
                data
            }
            Err(e) => {
                // The recorder failed after capture started. Tell widgets the
                // mic is done, then close the cycle as a failed transcription —
                // surfaced as an inner `Err`, never as success text.
                self.emit_mic_stopped();
                warn!("Recording capture failed: {e}");
                self.finalize_recording_session("", false, Some(e.to_string()))
                    .await;
                if write_mode {
                    typer.type_notice(notice::RECORDING_FAILED).await;
                }
                return Ok(Err(format!("recording error: {e}")));
            }
        };

        // Phase 4: final transcription.
        self.emit_transcribing_started();
        let transcription_result = match self
            .transcribe_final(&full_audio_data, request_language)
            .await
        {
            Ok(text) => text,
            Err(e) => {
                // A transcription failure is not typed into the user's focused
                // window (the old code typed "[STT error: …]"): finalize reports
                // it via `transcribing_stopped`, and it surfaces as an inner
                // `Err` so the HTTP layer emits the contract's `error` event.
                warn!("Final transcription failed: {e}");
                self.finalize_recording_session("", false, Some(e.to_string()))
                    .await;
                if write_mode {
                    typer.type_notice(notice::TRANSCRIPTION_FAILED).await;
                }
                return Ok(Err(format!("STT error: {e}")));
            }
        };

        // Phase 5: type the final transcript.
        if write_mode {
            info!("Writing transcription via {}", typer.write_method_name());
            typer.process_final_text(&transcription_result).await;
        }

        // Phase 6: finalize the cycle.
        self.finalize_recording_session(&transcription_result, true, None)
            .await;

        Ok(Ok(transcription_result))
    }

    /// Set up recording state and create audio recorder
    pub(super) async fn setup_recording_session(
        &self,
        _write_mode: bool,
    ) -> Result<DaemonAudioRecorder> {
        // Double-check busy state and set atomically
        {
            let mut busy_guard = self.busy.write().await;
            if *busy_guard {
                error!("Recording already in progress - rejecting duplicate request");
                return Err(anyhow::anyhow!("Recording already in progress"));
            }
            // Set busy state to true atomically
            *busy_guard = true;
        }

        // Create audio recorder with current theme and volume. Construction runs
        // the cpal cold-start (default host/output device/config) and a
        // `std::thread::sleep` device-verification spin — up to ~1.6s of real
        // blocking on a cold start. Run it on a blocking thread so it doesn't
        // park a runtime worker and stall concurrent SSE/event/status handling
        // exactly when the user starts talking (audit 2 Tier 1 #3).
        let current_theme = self.get_audio_theme();
        let current_volume = self.get_volume_f32();
        let mut recorder = tokio::task::spawn_blocking(move || {
            DaemonAudioRecorder::new_with_theme(current_theme, current_volume)
        })
        .await
        .context("Audio recorder construction task panicked")?
        .context("Failed to create audio recorder")?;

        // Initialize the recorder for threaded operation
        recorder.prepare_for_threaded_recording();

        Ok(recorder)
    }

    /// Emit "mic capture ended" on the widget bus the instant audio capture
    /// stops (Phase 3), independent of the transcription that follows so
    /// widgets drop the live visualization immediately. `busy` stays true.
    pub(super) fn emit_mic_stopped(&self) {
        self.events
            .publish_recording_stopped(crate::daemon::events::RecordingStoppedEvent {
                client_id: CLIENT_ID.to_string(),
                timestamp: Utc::now().to_rfc3339(),
            });
        self.broadcast_recording_state_change(false);
    }

    /// Emit "model decode has begun" (Phase 4).
    pub(super) fn emit_transcribing_started(&self) {
        self.events
            .publish_transcribing_started(crate::daemon::events::TranscribingStartedEvent {
                client_id: CLIENT_ID.to_string(),
                timestamp: Utc::now().to_rfc3339(),
            });
    }

    /// Emit "capture has begun" (Phase 1): mic event + D-Bus `listening_started`.
    async fn emit_recording_started(&self, write_mode: bool) {
        self.broadcast_recording_state_change(true);
        self.emit_listening_started_dbus(write_mode).await;
    }

    /// Emit D-Bus listening started event AND publish the matching
    /// `recording_started` event on the widget HTTP/SSE bus. The two
    /// transports carry the same fact, so they're issued together to
    /// avoid drift between subscribers.
    async fn emit_listening_started_dbus(&self, write_mode: bool) {
        let timestamp = Utc::now().to_rfc3339();
        let client_id = CLIENT_ID.to_string();

        // Widget bus
        self.events
            .publish_recording_started(crate::daemon::events::RecordingStartedEvent {
                client_id: client_id.clone(),
                timestamp: timestamp.clone(),
                write_mode,
            });

        if let Some(ref dbus_manager) = self.dbus_manager {
            let event = ListeningEvent {
                client_id,
                timestamp,
                write_mode,
                timeout_seconds: 0,
                audio_level: 0.0,
            };

            if let Err(e) = dbus_manager.emit_listening_started(event).await {
                warn!("Failed to emit D-Bus listening_started signal: {e}");
            }
        }
    }

    /// Finalize the cycle: clear the busy guard and emit the transcription
    /// result + the transcribing-stopped lifecycle event. Called exactly
    /// once per cycle that reached the transcription phase.
    pub(super) async fn finalize_recording_session(
        &self,
        final_text: &str,
        transcription_success: bool,
        error: Option<String>,
    ) {
        {
            let mut busy_guard = self.busy.write().await;
            *busy_guard = false;
        }

        let timestamp = Utc::now().to_rfc3339();
        let client_id = CLIENT_ID.to_string();
        let dbus_error = error.clone().unwrap_or_default();

        // Final transcription text — success only; failures are conveyed by
        // transcribing_stopped's `error`. Widgets need `global_transcriptions`.
        if transcription_success {
            self.events.publish_final_stt(final_text.to_string(), 1.0);
        }

        // Transcribing phase ended (widget bus).
        self.events
            .publish_transcribing_stopped(crate::daemon::events::TranscribingStoppedEvent {
                client_id: client_id.clone(),
                timestamp: timestamp.clone(),
                transcription_success,
                error,
            });

        // D-Bus listening stopped, paired with transcribing_stopped.
        if let Some(ref dbus_manager) = self.dbus_manager {
            let event = crate::services::dbus::ListeningStoppedEvent {
                client_id,
                timestamp,
                transcription_success,
                error: dbus_error,
            };
            if let Err(e) = dbus_manager.emit_listening_stopped(event).await {
                warn!("Failed to emit D-Bus listening_stopped signal: {e}");
            }
        }
    }
}
