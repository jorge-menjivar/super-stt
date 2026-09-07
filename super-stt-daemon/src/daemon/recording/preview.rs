// SPDX-License-Identifier: GPL-3.0-only

use super::RecordingSession;
use crate::daemon::types::{PreviewFrame, SuperSTTDaemon};
use crate::output::typer::Typer;
use anyhow::Result;
use log::{debug, info, warn};
use std::sync::Arc;
use std::time::Duration;
use super_stt_shared::models::protocol::PreviewSource;
use tokio::time::Instant;

/// The shortest preview window: enough context for a batch model to transcribe
/// a fragment well, and what every window was before it followed the pass
/// cadence.
const MIN_PREVIEW_WINDOW: Duration = Duration::from_secs(5);

/// The longest preview window. A model slower than its own interval widens the
/// window to keep the overlap (see [`preview_window`]), and each wider window
/// takes longer still; this is where that stops.
const MAX_PREVIEW_WINDOW: Duration = Duration::from_secs(15);

/// Audio shared between consecutive windows. The typer stitches previews by
/// finding the tail of what it has in the next window
/// (`find_tail_match_in_text`), which needs the two windows to have heard some
/// of the same speech. Without overlap the match can only succeed by
/// coincidence, the typer falls back to keeping the longer text, and the typed
/// preview stalls after the first window.
const PREVIEW_OVERLAP: Duration = Duration::from_secs(3);

/// How much recent capture a preview pass transcribes: everything since the
/// previous pass began plus [`PREVIEW_OVERLAP`], clamped to
/// [`MIN_PREVIEW_WINDOW`]..=[`MAX_PREVIEW_WINDOW`] and rounded down to whole
/// seconds.
///
/// Sized from the measured gap rather than the model's declared interval
/// because the gap is what actually separates two windows: a model whose pass
/// takes longer than its interval spaces its windows by the pass, and a window
/// sized to the interval would leave them adjacent with no shared audio — which
/// is exactly what a fixed 5-second window did for whisper-large's 5-second
/// interval.
fn preview_window(since_last_pass: Duration) -> Duration {
    let secs = (since_last_pass + PREVIEW_OVERLAP)
        .clamp(MIN_PREVIEW_WINDOW, MAX_PREVIEW_WINDOW)
        .as_secs();
    Duration::from_secs(secs)
}

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
        info!("🎛️ Recording mode: {stop_mode}");

        // Claim `busy` FIRST (setup_recording_session does the atomic
        // check-and-set), and only then install the external stop channel.
        // Installing `manual_stop_tx` before the claim let a losing racer
        // overwrite it — and its cleanup then null it — leaving the *winning*
        // recording with no stop channel (unstoppable until the 1-minute
        // timeout) (audit 2 Tier 3 #6). A losing racer now returns here via `?`
        // without ever touching `manual_stop_tx`.
        let mut recorder = self.setup_recording_session(write_mode).await?;

        // Create a broadcast channel so this recording can be stopped externally.
        let (stop_tx, stop_rx) = tokio::sync::broadcast::channel(1);
        *self.manual_stop_tx.write().await = Some(stop_tx);

        // The preview cadence and whether previews are forced at all both
        // come from the loaded model. No model is unreachable here — the
        // caller refuses to record without one — so the fallback only has to
        // be harmless.
        let (model_processing_interval, force_preview_support) = {
            let guard = self.model.read().await;
            guard
                .as_ref()
                .map_or((std::time::Duration::from_secs(2), false), |loaded| {
                    (
                        loaded.definition.processing_interval,
                        loaded.definition.force_preview_support,
                    )
                })
        };

        let actually_typed = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

        // Get a reference to the recorder's internal audio buffer for direct preview access
        let preview_buffer = recorder.get_audio_buffer_ref();
        // Same deal for the speech latch — must be cloned out before the
        // recorder moves into its task below.
        let speech_state = recorder.recording_state_ref();

        // Detect the actual device sample rate for correct buffer calculations.
        // This opens the cpal default input device, so run it on a blocking
        // thread rather than the async worker (audit 2 Tier 1 #3). Falls back to
        // 16kHz if detection (or the task) fails.
        let device_sample_rate = tokio::task::spawn_blocking(
            crate::audio::recorder::DaemonAudioRecorder::detect_default_input_sample_rate,
        )
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(16000);

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
            force_preview_support,
            actually_typed,
            preview_buffer,
            speech_state,
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
        request_language: Option<&str>,
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

        if !session.force_preview_support {
            info!("Preview support is not forced for the active model; waiting for capture to end");
        }

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

            // A model without forced previews still needs this loop for the
            // completion poll and the runaway guard above; only the
            // transcription work is skipped.
            if !session.force_preview_support {
                continue;
            }

            // Throttle the actual preview transcription to the model's interval.
            let since_last_pass = last_preview.elapsed();
            if since_last_pass < session.model_processing_interval {
                continue;
            }
            last_preview = Instant::now();

            // Run the preview-transcription work if a client is streaming
            // preview frames (a preview slot is claimed via `stream_realtime`)
            // OR preview-typing is on. Skip only when neither consumer wants
            // incremental results — the two are decoupled (a client can stream
            // preview without on-screen typing, and vice versa).
            let streaming = self.preview_text.read().await.is_some();
            let typing = self
                .preview_typing_enabled
                .load(std::sync::atomic::Ordering::Relaxed);
            if !streaming && !typing {
                debug!("No preview client and preview-typing off; skipping preview transcription");
                continue;
            }

            let audio_data =
                Self::read_preview_audio_from_buffer(session, preview_window(since_last_pass));
            debug!("Got {} audio samples for preview", audio_data.len());
            if audio_data.is_empty() {
                debug!("No audio data available for preview yet");
            } else {
                // Returns true when resampling failed; nothing else to do this
                // tick either way — the next attempt is a full interval later.
                let _ = self
                    .resample_and_emit_preview(
                        session,
                        audio_data,
                        typer,
                        write_mode,
                        request_language,
                    )
                    .await;
            }
        }
    }

    /// Extract the most recent `window` of audio from the shared ring-buffer,
    /// discarding silence. Returns an empty vec when there is nothing to
    /// transcribe yet.
    fn read_preview_audio_from_buffer(session: &RecordingSession, window: Duration) -> Vec<f32> {
        debug!(
            "Reading the last {}s from the capture buffer",
            window.as_secs()
        );
        let buffer_guard = session.preview_buffer.lock();

        let total_samples = buffer_guard.len();

        if total_samples == 0 {
            return Vec::new();
        }

        // The window is whole seconds at the device's own rate; resampling to
        // the model's rate happens after the read.
        let window_samples =
            usize::try_from(u64::from(session.device_sample_rate) * window.as_secs())
                .unwrap_or(usize::MAX);
        let samples_for_preview = total_samples.min(window_samples);
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
        request_language: Option<&str>,
    ) -> bool {
        // Resample to 16kHz if needed (same as final recording does)
        let resampled_audio = if session.device_sample_rate == 16000 {
            debug!("No resampling needed, device already at 16kHz");
            audio_data
        } else {
            let device_rate = session.device_sample_rate;
            debug!("Resampling from {device_rate}Hz to 16kHz for preview");
            // Resampling is synchronous CPU work over a preview window of
            // capture each tick (and the whole recording on the final drain); run it on a
            // blocking thread rather than parking the request's async worker
            // (audit 2 Tier 3 #2).
            let input_len = audio_data.len();
            match tokio::task::spawn_blocking(move || {
                super_stt_shared::utils::audio::resample(
                    &audio_data,
                    device_rate,
                    16000,
                    super_stt_shared::audio_utils::ResampleQuality::Fast,
                )
            })
            .await
            {
                Ok(Ok(resampled)) => {
                    debug!(
                        "Resampled {input_len} samples to {} samples",
                        resampled.len()
                    );
                    resampled
                }
                Ok(Err(e)) => {
                    warn!("Failed to resample preview audio: {e}");
                    return true; // Signal caller to `continue` to next iteration
                }
                Err(e) => {
                    warn!("Preview resample task panicked: {e}");
                    return true;
                }
            }
        };

        // Transcribe resampled audio data using current model
        debug!(
            "Starting preview transcription with {} samples",
            resampled_audio.len()
        );
        if let Ok(text) = self
            .transcribe_audio_chunk(&resampled_audio, request_language)
            .await
        {
            self.emit_preview(&text, PreviewSource::Window, session, typer, write_mode)
                .await;
        }

        false // Normal completion — do not skip the timeout check
    }

    /// Publish one preview transcript everywhere a preview goes: `/events`
    /// subscribers holding `global_transcriptions`, the waiting `/transcribe`
    /// SSE client, and — when write-mode and preview-typing are both on — the
    /// focused window.
    ///
    /// Shared by both producers of incremental text: the sliding-window loop
    /// that simulates streaming for batch models, and the live session a
    /// realtime model streams through. Each names itself in `source`, which
    /// travels with the text: a window replaces the previous preview and a
    /// stream extends it, and a client cannot tell which from the text alone.
    /// Empty text is not a preview.
    pub(super) async fn emit_preview(
        &self,
        text: &str,
        source: PreviewSource,
        session: &RecordingSession,
        typer: &mut Typer,
        write_mode: bool,
    ) {
        if text.trim().is_empty() {
            return;
        }
        let processed = crate::output::preview::preprocess_text(text, true);

        info!(
            "Preview ({source}): '{}'",
            processed.chars().take(30).collect::<String>()
        );

        // Live preview to widgets holding `global_transcriptions`.
        self.events
            .publish_partial_stt(processed.clone(), 1.0, source);

        // Stream to the waiting client (the id is only used to gate slot
        // claim/clear in the HTTP handler).
        if let Some((_, ref tx)) = *self.preview_text.read().await {
            let _ = tx.send(PreviewFrame {
                text: processed,
                source,
            });
        }

        // Type on screen if in write mode. The typing is now async, and the
        // `actually_typed` guard is a `!Send` `std::Mutex` guard that cannot
        // be held across an `.await`. Take the mirror string out in a scope
        // that drops the guard before typing, then write it back — safe
        // because `update_preview` and `clear_preview` both run under
        // `&mut Typer` and never overlap (audit Tier 3 #35). Skip on a
        // poisoned lock, as before.
        // Type on screen only when write-mode AND preview-typing are both
        // active. The loop may be running purely to stream preview frames to
        // a client (`stream_realtime`) with preview-typing off, in which case
        // it must not type.
        let type_on_screen = write_mode
            && self
                .preview_typing_enabled
                .load(std::sync::atomic::Ordering::Relaxed);
        let taken = if type_on_screen {
            session
                .actually_typed
                .lock()
                .ok()
                .map(|mut g| std::mem::take(&mut *g))
        } else {
            None
        };
        if let Some(mut actually_typed) = taken {
            typer.update_preview(text, &mut actually_typed).await;
            if let Ok(mut g) = session.actually_typed.lock() {
                *g = actually_typed;
            }
        }
    }

    /// Erase preview text typed during Phase 2. Split out of
    /// `collect_and_clear_preview` so the recorder-failure paths clear the field
    /// too — otherwise a failed capture leaves half-typed preview text behind
    /// for the failure notice to append to.
    async fn clear_preview_text(
        &self,
        actually_typed: &Arc<std::sync::Mutex<String>>,
        typer: &mut Typer,
        write_mode: bool,
    ) {
        if !write_mode {
            return;
        }
        if !self
            .preview_typing_enabled
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            debug!("Preview typing was disabled, no preview to clear");
            return;
        }
        // Take the mirror out in a scope that drops the `!Send` guard before the
        // async backspacing (audit Tier 3 #35).
        let taken = if let Ok(mut g) = actually_typed.lock() {
            Some(std::mem::take(&mut *g))
        } else {
            warn!("Failed to acquire actually_typed lock for clearing preview");
            None
        };
        if let Some(mut taken) = taken {
            info!(
                "Clearing preview text: '{}'",
                taken.chars().take(50).collect::<String>()
            );
            typer.clear_preview(&mut taken).await;
            info!("Preview cleared, actually_typed is now: '{taken}'");
            if let Ok(mut g) = actually_typed.lock() {
                *g = taken;
            }
        }
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
                self.clear_preview_text(&session.actually_typed, typer, write_mode)
                    .await;
                return Err(e);
            }
            Err(e) => {
                *self.manual_stop_tx.write().await = None;
                self.clear_preview_text(&session.actually_typed, typer, write_mode)
                    .await;
                return Err(anyhow::anyhow!("Recorder task failed: {e}"));
            }
        };

        // Audio capture is done — clear stop channel.
        // busy stays true until finalize_recording_session so the daemon
        // rejects new recordings while transcription is in progress.
        *self.manual_stop_tx.write().await = None;

        // Clear preview after recording is done (only if preview typing was enabled)
        self.clear_preview_text(&session.actually_typed, typer, write_mode)
            .await;

        Ok(full_audio_data)
    }
}

#[cfg(test)]
mod tests {
    use super::super::RecordingSession;
    use super::{MAX_PREVIEW_WINDOW, MIN_PREVIEW_WINDOW, PREVIEW_OVERLAP, preview_window};
    use crate::daemon::types::SuperSTTDaemon;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::time::Duration;

    /// A session over `buffer` captured at `rate`, with everything else inert.
    fn session_over(buffer: Vec<f32>, rate: usize) -> RecordingSession {
        RecordingSession {
            recorder_handle: tokio::spawn(async { anyhow::Ok(Vec::<f32>::new()) }),
            model_processing_interval: Duration::from_secs(2),
            force_preview_support: true,
            actually_typed: Arc::new(std::sync::Mutex::new(String::new())),
            preview_buffer: Arc::new(parking_lot::Mutex::new(VecDeque::from(buffer))),
            speech_state: Arc::new(parking_lot::Mutex::new(
                crate::audio::state::RecordingState::default(),
            )),
            device_sample_rate: u32::try_from(rate).expect("a test rate fits"),
            start_time: tokio::time::Instant::now(),
        }
    }

    /// Samples that ramp from 0 towards 1, so a slice's first value says where
    /// in the capture it came from.
    // reason: a test ramp; the index never exceeds f32's exact range here.
    #[allow(clippy::cast_precision_loss)]
    fn ramp(len: usize) -> Vec<f32> {
        (0..len).map(|i| i as f32 / len as f32).collect()
    }

    fn read(session: &RecordingSession, secs: u64) -> Vec<f32> {
        SuperSTTDaemon::read_preview_audio_from_buffer(session, Duration::from_secs(secs))
    }

    /// The window is counted at the device's own rate, and it is the most
    /// recent audio: the slice ends where the capture ends.
    #[tokio::test]
    async fn the_read_takes_the_most_recent_window_at_the_device_rate() {
        let rate = 48_000;
        let total = 20 * rate;
        let capture = ramp(total);
        let session = session_over(capture.clone(), rate);

        let out = read(&session, 8);

        assert_eq!(out.len(), 8 * rate);
        assert_eq!(out[0], capture[total - 8 * rate]);
        assert_eq!(out.last(), capture.last());
    }

    /// Early in a take the buffer is shorter than the window; the read is
    /// everything captured so far, not nothing.
    #[tokio::test]
    async fn a_window_longer_than_the_capture_reads_all_of_it() {
        let rate = 16_000;
        let session = session_over(ramp(3 * rate), rate);
        assert_eq!(read(&session, 5).len(), 3 * rate);
    }

    /// Near-silence is not worth a transcription pass.
    #[tokio::test]
    async fn silence_reads_as_nothing() {
        let session = session_over(vec![0.0005; 5 * 16_000], 16_000);
        assert!(read(&session, 5).is_empty());
    }

    /// Fast models keep the window they always had: the minimum is wider than
    /// their gap plus the overlap.
    #[test]
    fn a_short_gap_gets_the_minimum_window() {
        assert_eq!(preview_window(Duration::from_secs(1)), MIN_PREVIEW_WINDOW);
        assert_eq!(preview_window(Duration::from_secs(2)), MIN_PREVIEW_WINDOW);
    }

    /// The regression: a 5-second gap used to get a 5-second window, so
    /// consecutive windows shared nothing. Now the window covers the gap and
    /// the overlap.
    #[test]
    fn a_gap_as_long_as_the_old_window_still_overlaps() {
        assert_eq!(
            preview_window(Duration::from_secs(5)),
            Duration::from_secs(5) + PREVIEW_OVERLAP
        );
    }

    /// A slow pass widens the next window to keep the overlap, rounded down
    /// to whole seconds.
    #[test]
    fn a_slow_pass_widens_the_window() {
        assert_eq!(
            preview_window(Duration::from_millis(7_500)),
            Duration::from_secs(10)
        );
    }

    /// Widening cannot run away: a model slower than the cap gets the cap.
    #[test]
    fn the_window_is_capped() {
        assert_eq!(preview_window(Duration::from_secs(60)), MAX_PREVIEW_WINDOW);
    }
}
