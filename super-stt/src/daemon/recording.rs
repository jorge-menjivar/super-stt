// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::services::dbus::ListeningEvent;
use crate::{audio::recorder::DaemonAudioRecorder, output::preview::Typer};
use anyhow::{Context, Result};
use chrono::Utc;
use log::{debug, error, info, warn};
use std::sync::Arc;
use super_stt_shared::models::protocol::DaemonResponse;
use tokio::time::Instant;

// Removed PreviewContext - no longer needed with simplified architecture

impl SuperSTTDaemon {
    /// Handle record command - direct recording in daemon (legacy method)
    pub async fn handle_record(&self, typer: &mut Typer, write_mode: bool) -> DaemonResponse {
        self.handle_record_internal(typer, write_mode).await
    }

    /// Internal record handling implementation
    pub async fn handle_record_internal(
        &self,
        typer: &mut Typer,
        write_mode: bool,
    ) -> DaemonResponse {
        // Check if already recording - prevent multiple simultaneous recordings
        {
            let is_recording_guard = self.is_recording.read().await;
            if *is_recording_guard {
                warn!("Recording request rejected - already recording");
                return DaemonResponse::error(
                    "Recording already in progress. Please wait for current recording to complete.",
                );
            }
        }

        // Wait for recording to complete and return the transcription
        match self.record_and_transcribe(typer, write_mode).await {
            Ok(transcription) => {
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
            Err(e) => {
                error!("🎤 Recording failed: {e}");
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
    #[allow(clippy::too_many_lines)]
    pub async fn record_and_transcribe(
        &self,
        typer: &mut Typer,
        write_mode: bool,
    ) -> Result<String> {
        info!("Starting direct audio recording in daemon with simplified architecture");

        // Set up recording state and create recorder
        let mut recorder = self.setup_recording_session(write_mode).await?;

        // Get model processing interval from current model type
        let model_processing_interval = {
            let model_type_guard = self.model_type.read().await;
            if let Some(model_type) = model_type_guard.as_ref() {
                model_type.get_processing_interval()
            } else {
                // Default interval if no model loaded
                std::time::Duration::from_millis(2000)
            }
        };

        let actually_typed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

        // Get a reference to the recorder's internal audio buffer for direct preview access
        let preview_buffer = recorder.get_audio_buffer_ref();

        // Detect the actual device sample rate for correct buffer calculations
        let device_sample_rate = recorder.detect_default_input_sample_rate().unwrap_or(16000); // fallback to 16kHz if detection fails

        // Start the recorder in its own thread
        let recorder_handle = tokio::spawn({
            let udp_streamer = Arc::clone(&self.udp_streamer);
            async move {
                recorder
                    .record_until_silence_with_streaming(udp_streamer, None)
                    .await
            }
        });

        let start_time = Instant::now();

        // Main transcription loop - process audio chunks while recording
        loop {
            debug!("Starting transcription loop");
            debug!(
                "Model processing interval: {:?}",
                model_processing_interval.as_millis()
            );
            // Sleep until model processing interval has been reached
            tokio::time::sleep(model_processing_interval).await;

            // Check if recorder is still active
            if recorder_handle.is_finished() {
                break;
            }

            // Get last 10 seconds of audio data directly from buffer for preview
            debug!("About to get 10 secs from buffer");
            let audio_data = {
                let buffer_guard = match preview_buffer.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => {
                        debug!("Buffer lock poisoned, recovering");
                        poisoned.into_inner()
                    }
                };

                let total_samples = buffer_guard.len();

                if total_samples == 0 {
                    Vec::new()
                } else {
                    // For preview, get the most recent audio (last 3-5 seconds is usually enough)
                    // Using 5 seconds at the actual device sample rate
                    let samples_for_preview =
                        std::cmp::min(total_samples, device_sample_rate as usize * 5);
                    let start_idx = total_samples - samples_for_preview;

                    let samples: Vec<f32> = buffer_guard.range(start_idx..).copied().collect();
                    debug!(
                        "Extracted {} samples for preview (from idx {} to {})",
                        samples.len(),
                        start_idx,
                        total_samples
                    );

                    // Basic audio validation - check if we have reasonable audio levels
                    let max_amplitude = samples.iter().map(|&x| x.abs()).fold(0.0, f32::max);

                    if max_amplitude < 0.001 {
                        debug!("Audio appears to be mostly silence, skipping transcription");
                        Vec::new()
                    } else {
                        samples
                    }
                }
            };

            debug!("Got {} audio samples for preview", audio_data.len());

            // Check if preview typing is enabled before doing any processing
            let preview_enabled = self
                .preview_typing_enabled
                .load(std::sync::atomic::Ordering::Relaxed);

            if !audio_data.is_empty() && write_mode && preview_enabled {
                // Resample to 16kHz if needed (same as final recording does)
                let resampled_audio = if device_sample_rate == 16000 {
                    debug!("No resampling needed, device already at 16kHz");
                    audio_data
                } else {
                    debug!("Resampling from {device_sample_rate}Hz to 16kHz for preview");
                    match super_stt_shared::utils::audio::resample(
                        &audio_data,
                        device_sample_rate,
                        16000,
                        super_stt_shared::audio_utils::ResampleQuality::Fast,
                    ) {
                        Ok(resampled) => {
                            debug!(
                                "Resampled {} samples to {} samples",
                                audio_data.len(),
                                resampled.len()
                            );
                            resampled
                        }
                        Err(e) => {
                            warn!("Failed to resample preview audio: {e}");
                            continue; // Skip this preview iteration
                        }
                    }
                };

                // Transcribe resampled audio data using current model
                debug!(
                    "Starting preview transcription with {} samples",
                    resampled_audio.len()
                );
                if let Ok(text) = self.transcribe_audio_chunk(&resampled_audio).await
                    && !text.trim().is_empty()
                {
                    info!(
                        "Updating preview with text: '{}'",
                        text.chars().take(30).collect::<String>()
                    );
                    if let Ok(mut actually_typed_guard) = actually_typed.lock() {
                        typer.update_preview(&text, &mut actually_typed_guard);
                    }
                }
            } else if !preview_enabled {
                debug!("Preview typing is disabled, skipping audio processing and transcription");
            } else if audio_data.is_empty() {
                debug!("No audio data available for preview yet");
            }

            // Prevent infinite loops with a reasonable timeout
            if start_time.elapsed() > std::time::Duration::from_secs(60) {
                warn!("Recording timeout reached, stopping preview loop");
                break;
            }
        }

        info!("Step 1 complete: Preview has finished");

        // Wait for recorder to finish and get full audio data
        let full_audio_data = recorder_handle.await??;

        // Clear preview after recording is done (only if preview typing was enabled)
        if write_mode {
            let preview_enabled = self
                .preview_typing_enabled
                .load(std::sync::atomic::Ordering::Relaxed);
            if preview_enabled {
                if let Ok(mut actually_typed_guard) = actually_typed.lock() {
                    info!(
                        "Clearing preview text: '{}'",
                        actually_typed_guard.chars().take(50).collect::<String>()
                    );
                    typer.clear_preview(&mut actually_typed_guard);
                    info!("Preview cleared, actually_typed is now: '{actually_typed_guard}'");
                } else {
                    warn!("Failed to acquire actually_typed lock for clearing preview");
                }
            } else {
                debug!("Preview typing was disabled, no preview to clear");
            }
        }
        info!("Step 2 complete: Preview has been cleared");

        // STEP 3: Loader start + STEP 4: GPU final transcription + STEP 5: Loader end
        info!("Step 3-5: Starting loader, running GPU final transcription, stopping loader");
        let transcription_result = self
            .transcribe_with_spinner(typer, &full_audio_data, write_mode)
            .await?;
        info!("Step 3-5 complete: Final GPU transcription finished");

        // STEP 6: Type final transcript
        if write_mode {
            typer.process_final_text(&transcription_result);
        }
        info!("Step 6 complete: Final transcription typed successfully");

        // Finalize recording session
        self.finalize_recording_session(
            &transcription_result,
            &std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .await;

        info!(
            "🎯 Perfect sequence completed: GPU preview finish → clear → loader → GPU final → type final"
        );

        Ok(transcription_result)
    }

    /// Transcribe a chunk of audio data for preview
    async fn transcribe_audio_chunk(&self, audio_data: &[f32]) -> Result<String> {
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

        // Clone the model Arc for the blocking task
        let model_clone = Arc::clone(&self.model);

        // Run transcription in a blocking task to avoid blocking the async runtime
        let result = tokio::task::spawn_blocking(move || {
            // Get exclusive write access to the model
            let mut model_guard = model_clone.blocking_write();

            if let Some(model) = model_guard.as_mut() {
                match model.transcribe_audio(&processed_audio, 16000) {
                    Ok(text) => Ok(text) as Result<String>,
                    Err(e) => {
                        // For preview transcription errors, return empty string instead of failing
                        warn!("Preview transcription failed, continuing: {e}");
                        Ok(String::new()) as Result<String>
                    }
                }
            } else {
                warn!("Model not loaded for preview transcription");
                Ok(String::new()) as Result<String>
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("Preview transcription task failed: {e}"))??;

        Ok(result)
    }

    /// Set up recording state and create audio recorder
    async fn setup_recording_session(&self, write_mode: bool) -> Result<DaemonAudioRecorder> {
        // Double-check recording state and set atomically
        {
            let mut is_recording_guard = self.is_recording.write().await;
            if *is_recording_guard {
                error!("Recording already in progress - rejecting duplicate request");
                return Err(anyhow::anyhow!("Recording already in progress"));
            }
            // Set recording state to true atomically
            *is_recording_guard = true;
        }

        // Emit UDP recording state change
        self.broadcast_recording_state_change(true).await;

        // Emit D-Bus listening started event
        self.emit_listening_started_dbus(write_mode).await;

        // Create audio recorder with current theme
        let current_theme = self.get_audio_theme();
        let mut recorder = DaemonAudioRecorder::new_with_theme(current_theme)
            .context("Failed to create audio recorder")?;

        // Initialize the recorder for threaded operation
        recorder.prepare_for_threaded_recording();

        Ok(recorder)
    }

    /// Emit D-Bus listening started event
    async fn emit_listening_started_dbus(&self, write_mode: bool) {
        if let Some(ref dbus_manager) = self.dbus_manager {
            let event = ListeningEvent {
                client_id: "daemon_recorder".to_string(),
                timestamp: Utc::now().to_rfc3339(),
                write_mode,
                timeout_seconds: 0,
                audio_level: 0.0,
            };

            if let Err(e) = dbus_manager.emit_listening_started(event).await {
                warn!("Failed to emit D-Bus listening_started signal: {e}");
            }
        }
    }

    /// Record audio and clean up preview session (legacy - kept for reference)
    #[allow(dead_code)]
    async fn record_audio_and_cleanup_preview(
        &self,
        mut recorder: DaemonAudioRecorder,
        _legacy_param: (),
        _write_mode: bool,
    ) -> Result<Vec<f32>> {
        // Legacy method - replaced with simplified architecture
        recorder
            .record_until_silence_with_streaming(Arc::clone(&self.udp_streamer), None)
            .await
    }

    /// Transcribe audio with spinner if needed
    async fn transcribe_with_spinner(
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

        // Transcribe the audio
        let transcription_result = {
            // Clone the model Arc for the blocking task
            let model_clone = Arc::clone(&self.model);

            // Run transcription in a blocking task to avoid blocking the async runtime
            tokio::task::spawn_blocking(move || {
                let start_time = std::time::Instant::now();

                // Get exclusive write access to the model
                let mut model_guard = model_clone.blocking_write();

                if let Some(model) = model_guard.as_mut() {
                    match model.transcribe_audio(&processed_audio, 16000) {
                        Ok(text) => {
                            let duration = start_time.elapsed();
                            info!("Transcription completed in {duration:?}: '{text}'");
                            Ok(text)
                        }
                        Err(e) => {
                            // For transcription errors (like Voxtral mel generation issues),
                            // return empty string instead of failing the entire request
                            warn!("Transcription failed, returning empty result: {e}");
                            Ok(String::new())
                        }
                    }
                } else {
                    error!("Model not loaded");
                    Err(anyhow::anyhow!("Model not loaded"))
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("Transcription task failed: {e}"))?
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

    /// Finalize recording session and emit events
    async fn finalize_recording_session(
        &self,
        _transcription_result: &str,
        _preview_typed_count: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        // Reset recording state
        {
            let mut is_recording_guard = self.is_recording.write().await;
            *is_recording_guard = false;
        }
        self.broadcast_recording_state_change(false).await;

        // Emit D-Bus listening stopped event
        if let Some(ref dbus_manager) = self.dbus_manager {
            let event = crate::services::dbus::ListeningStoppedEvent {
                client_id: "daemon_recorder".to_string(),
                timestamp: Utc::now().to_rfc3339(),
                transcription_success: true, // We only call this on success
                error: String::new(),
            };

            if let Err(e) = dbus_manager.emit_listening_stopped(event).await {
                warn!("Failed to emit D-Bus listening_stopped signal: {e}");
            }
        }
    }
}
