// SPDX-License-Identifier: GPL-3.0-only

use super::RecordingSession;
use crate::daemon::types::SuperSTTDaemon;
use crate::output::typer::Typer;
use anyhow::Result;
use log::{debug, info, warn};
use std::sync::Arc;
use tokio::time::Instant;

impl SuperSTTDaemon {
    /// Phase 1: create the stop broadcast channel, set up the recorder, and
    /// spawn the recording task. Returns a [`RecordingSession`] that carries
    /// all the state the subsequent phases need.
    pub(super) async fn spawn_recorder(
        &self,
        write_mode: bool,
        stop_mode: super_stt_shared::models::recording_stop_mode::RecordingStopMode,
    ) -> Result<RecordingSession> {
        let silence_detection_disabled = !stop_mode.silence_detection_enabled();

        // Create a broadcast channel so any recording can be stopped externally.
        let (stop_tx, stop_rx) = tokio::sync::broadcast::channel(1);
        *self.manual_stop_tx.write().await = Some(stop_tx);
        info!("🎛️ Recording mode: {stop_mode}");

        // Set up recording state and create recorder
        let mut recorder = match self.setup_recording_session(write_mode).await {
            Ok(recorder) => recorder,
            Err(e) => {
                *self.manual_stop_tx.write().await = None;
                return Err(e);
            }
        };

        // Get model processing interval from current model
        let model_processing_interval = {
            let guard = self.model.read().await;
            guard
                .as_ref()
                .map_or(std::time::Duration::from_secs(2), |loaded| {
                    loaded.definition.processing_interval
                })
        };

        let actually_typed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

        // Get a reference to the recorder's internal audio buffer for direct preview access
        let preview_buffer = recorder.get_audio_buffer_ref();

        // Detect the actual device sample rate for correct buffer calculations
        let device_sample_rate = recorder.detect_default_input_sample_rate().unwrap_or(16000); // fallback to 16kHz if detection fails

        // Start the recorder in its own thread
        let recorder_handle = tokio::spawn({
            let events = Arc::clone(&self.events);
            async move {
                recorder
                    .record_until_silence_with_streaming(
                        events,
                        None,
                        silence_detection_disabled,
                        Some(stop_rx),
                    )
                    .await
            }
        });

        let start_time = Instant::now();

        Ok(RecordingSession {
            recorder_handle,
            model_processing_interval,
            actually_typed,
            preview_buffer,
            device_sample_rate,
            start_time,
        })
    }

    /// Phase 2: poll the recorder task and stream live preview transcriptions
    /// until the recorder finishes or the timeout is reached.
    pub(super) async fn run_preview_loop(
        &self,
        session: &RecordingSession,
        typer: &mut Typer,
        write_mode: bool,
    ) {
        // Poll for recorder completion at a fine cadence so the mic actually
        // stopping is noticed within ~100ms — letting `recording_stopped` and
        // the final transcription kick off promptly — independent of the
        // model's much coarser preview interval.
        //
        // Previously this loop slept the full `model_processing_interval`
        // (~2s) *before* checking `recorder_handle.is_finished()`, so the end
        // of recording was detected up to a whole interval late. The stop cue
        // plays during that gap, which made it look like the daemon was
        // blocking on the stop sound. Preview transcription itself is still
        // throttled to once per `model_processing_interval`.
        const COMPLETION_POLL: std::time::Duration = std::time::Duration::from_millis(100);

        let mut last_preview = Instant::now();
        loop {
            // Notice the recorder finishing promptly — before and after the nap.
            if session.recorder_handle.is_finished() {
                break;
            }
            tokio::time::sleep(COMPLETION_POLL).await;
            if session.recorder_handle.is_finished() {
                break;
            }

            // Prevent runaway recordings. Breaking out of the preview loop is
            // not enough: `collect_and_clear_preview` then awaits the recorder
            // task, which — with silence detection disabled and manual stop
            // refused (SilenceOnly mode) — would otherwise never finish, leaving
            // capture unbounded with `busy=true` and a frozen preview. Signal
            // the recorder's stop channel (same one the manual-stop shortcut
            // uses) so it ends cleanly and returns the audio captured so far.
            if session.start_time.elapsed() > std::time::Duration::from_mins(1) {
                warn!("Recording timeout reached, signalling recorder to stop");
                if let Some(tx) = self.manual_stop_tx.read().await.as_ref() {
                    let _ = tx.send(());
                }
                break;
            }

            // Throttle the actual preview transcription to the model's interval.
            if last_preview.elapsed() < session.model_processing_interval {
                continue;
            }
            last_preview = Instant::now();

            // Skip the preview-transcription work entirely when the
            // preview-typing setting is off.
            if !self
                .preview_typing_enabled
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                debug!("Preview typing is disabled, skipping audio processing and transcription");
                continue;
            }

            let audio_data = Self::read_preview_audio_from_buffer(session);
            debug!("Got {} audio samples for preview", audio_data.len());
            if audio_data.is_empty() {
                debug!("No audio data available for preview yet");
            } else {
                // Returns true when resampling failed; nothing else to do this
                // tick either way — the next attempt is a full interval later.
                let _ = self
                    .resample_and_emit_preview(session, audio_data, typer, write_mode)
                    .await;
            }
        }
    }

    /// Extract up to 5 seconds of recent audio from the shared ring-buffer,
    /// discarding silence. Returns an empty vec when there is nothing to
    /// transcribe yet.
    fn read_preview_audio_from_buffer(session: &RecordingSession) -> Vec<f32> {
        // Get last 5 seconds of audio data directly from buffer for preview
        debug!("About to get 10 secs from buffer");
        let buffer_guard = match session.preview_buffer.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                debug!("Buffer lock poisoned, recovering");
                poisoned.into_inner()
            }
        };

        let total_samples = buffer_guard.len();

        if total_samples == 0 {
            return Vec::new();
        }

        // For preview, get the most recent audio (last 3-5 seconds is usually enough)
        // Using 5 seconds at the actual device sample rate
        let samples_for_preview =
            std::cmp::min(total_samples, session.device_sample_rate as usize * 5);
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

    /// Resample `audio_data` to 16 kHz, transcribe it for preview, stream the
    /// result to any waiting client, and optionally type it on screen.
    ///
    /// Returns `true` when the caller should `continue` to the next loop
    /// iteration (i.e. resampling failed and the current tick should be
    /// skipped entirely), `false` otherwise.
    async fn resample_and_emit_preview(
        &self,
        session: &RecordingSession,
        audio_data: Vec<f32>,
        typer: &mut Typer,
        write_mode: bool,
    ) -> bool {
        // Resample to 16kHz if needed (same as final recording does)
        let resampled_audio = if session.device_sample_rate == 16000 {
            debug!("No resampling needed, device already at 16kHz");
            audio_data
        } else {
            debug!(
                "Resampling from {}Hz to 16kHz for preview",
                session.device_sample_rate
            );
            match super_stt_shared::utils::audio::resample(
                &audio_data,
                session.device_sample_rate,
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
                    return true; // Signal caller to `continue` to next iteration
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
            let processed = crate::output::preview::preprocess_text(&text, true);

            info!(
                "Preview: '{}'",
                processed.chars().take(30).collect::<String>()
            );

            // Live preview to widgets holding `global_transcriptions`.
            self.events.publish_partial_stt(processed.clone(), 1.0);

            // Stream to the waiting client (the id is only used to gate slot
            // claim/clear in the HTTP handler).
            if let Some((_, ref tx)) = *self.preview_text.read().await {
                let _ = tx.send(processed);
            }

            // Type on screen if in write mode
            if write_mode && let Ok(mut actually_typed_guard) = session.actually_typed.lock() {
                typer.update_preview(&text, &mut actually_typed_guard);
            }
        }

        false // Normal completion — do not skip the timeout check
    }

    /// Phase 3: await the recorder task to get the full audio data, clear the
    /// stop channel, and erase any preview text that was typed during Phase 2.
    pub(super) async fn collect_and_clear_preview(
        &self,
        session: RecordingSession,
        typer: &mut Typer,
        write_mode: bool,
    ) -> Result<Vec<f32>> {
        // Wait for recorder to finish and get full audio data
        let full_audio_data = match session.recorder_handle.await {
            Ok(Ok(data)) => data,
            Ok(Err(e)) => {
                *self.manual_stop_tx.write().await = None;
                return Err(e);
            }
            Err(e) => {
                *self.manual_stop_tx.write().await = None;
                return Err(anyhow::anyhow!("Recorder task failed: {e}"));
            }
        };

        // Audio capture is done — clear stop channel.
        // busy stays true until finalize_recording_session so the daemon
        // rejects new recordings while transcription is in progress.
        *self.manual_stop_tx.write().await = None;

        // Clear preview after recording is done (only if preview typing was enabled)
        if write_mode {
            let preview_enabled = self
                .preview_typing_enabled
                .load(std::sync::atomic::Ordering::Relaxed);
            if preview_enabled {
                if let Ok(mut actually_typed_guard) = session.actually_typed.lock() {
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

        Ok(full_audio_data)
    }
}
