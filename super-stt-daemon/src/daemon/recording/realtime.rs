// SPDX-License-Identifier: GPL-3.0-only
//! Phase 2 for a realtime model: one live session for the whole take.
//!
//! A batch model has no incremental output, so [`run_preview_loop`] simulates
//! streaming by re-transcribing a sliding window every `processing_interval`. A
//! realtime model *does* emit incremental transcripts, so simulating them is
//! both wasteful and worse: each window re-opened a fresh upstream session
//! (`transcribe_via_realtime` buffers the whole recording, then opens a socket)
//! and every `preview` frame the backend produced was thrown away.
//!
//! This module holds one session open for the take instead, feeding captured
//! audio in as it arrives and forwarding the backend's own `preview` frames to
//! [`SuperSTTDaemon::emit_preview`] — the same place the sliding-window loop
//! publishes, so `/events`, the `/transcribe` SSE stream, and preview typing all
//! work unchanged. The session's `done` frame is the final transcript, so the
//! separate Phase 4 decode is skipped entirely.
//!
//! **What this does not fix.** The daemon's `ws-stream::subscribe` still traps,
//! so a guest cannot wait on the consumer and its upstream at once and runs
//! half-duplex: it forwards all audio first, then drains transcripts. Previews
//! therefore still arrive in a burst after the take ends. This makes the path
//! honest and cheap — one session per recording carrying the backend's real
//! output — not yet responsive.
#![cfg(feature = "wasm-backends")]

use std::time::Duration;

use log::{debug, info, warn};
use serde_json::Value;

use super::RecordingSession;
use crate::daemon::types::SuperSTTDaemon;
use crate::output::typer::Typer;
use crate::stt_models::wasm::ws_host::{
    CONSUMER_INCOMING_CAPACITY, ConsumerStreamTransport, WsFrame,
};

/// How often to hand newly captured audio to the session. Matches the preview
/// loop's completion poll, so the end of capture is still noticed within ~100ms.
const STREAM_POLL: Duration = Duration::from_millis(100);

/// Sample rate declared in the `start` frame. The consumer contract is PCM16
/// mono at the declared rate; 16 kHz is what the rest of the daemon resamples
/// to, and a backend needing something else converts on its own side.
const TARGET_RATE: u32 = 16000;

/// Same runaway guard the sliding-window loop applies.
const MAX_TAKE: Duration = Duration::from_mins(1);

impl SuperSTTDaemon {
    /// Stream the take through the active model's realtime session.
    ///
    /// Returns the final transcript, or `None` when this take is not one a
    /// realtime session can serve — the active model is not realtime, or the
    /// session failed. `None` means the caller falls back to its normal Phase 4
    /// decode, so a failure here costs efficiency, never the transcription.
    pub(super) async fn run_realtime_stream_loop(
        &self,
        session: &RecordingSession,
        typer: &mut Typer,
        write_mode: bool,
        request_language: Option<&str>,
    ) -> Option<String> {
        // Held for the session's lifetime, exactly as the consumer WebSocket
        // path holds it: the model must not be swapped mid-stream.
        let guard = self.model.read().await;
        let loaded = guard.as_ref()?;
        if !loaded.definition.realtime {
            return None;
        }

        // incoming: us -> guest (bounded, so capture applies backpressure
        // instead of growing memory); outgoing: guest -> us (unbounded — the
        // guest produces bounded output and must never stall).
        let (incoming_tx, incoming_rx) =
            tokio::sync::mpsc::channel::<WsFrame>(CONSUMER_INCOMING_CAPACITY);
        let (outgoing_tx, mut outgoing_rx) = tokio::sync::mpsc::unbounded_channel::<WsFrame>();
        let transport = ConsumerStreamTransport {
            incoming: incoming_rx,
            outgoing: outgoing_tx,
        };

        // The guest's session and our pump run concurrently on this task: the
        // pump feeds audio and drains frames, the session drives the backend.
        let session_fut = loaded.instance.realtime_session(transport);
        let pump = self.pump_take(
            session,
            typer,
            write_mode,
            request_language,
            &incoming_tx,
            &mut outgoing_rx,
        );
        let (session_result, transcript) = tokio::join!(session_fut, pump);

        if let Err(e) = session_result {
            warn!("realtime streaming session ended with error: {e:#}");
            return None;
        }
        transcript
    }

    /// Feed captured audio into the session and forward what comes back.
    /// Returns the transcript carried by the session's `done` frame.
    async fn pump_take(
        &self,
        session: &RecordingSession,
        typer: &mut Typer,
        write_mode: bool,
        request_language: Option<&str>,
        incoming_tx: &tokio::sync::mpsc::Sender<WsFrame>,
        outgoing_rx: &mut tokio::sync::mpsc::UnboundedReceiver<WsFrame>,
    ) -> Option<String> {
        let mut start = serde_json::json!({ "type": "start", "sample_rate": TARGET_RATE });
        // `auto` is the reserved "detect it" value, which the contract spells as
        // an absent field.
        if let Some(language) = request_language.filter(|l| *l != "auto") {
            start["language"] = Value::String(language.to_string());
        }
        if incoming_tx
            .send(WsFrame::Text(start.to_string()))
            .await
            .is_err()
        {
            warn!("realtime session ended before the start frame was accepted");
            return None;
        }

        let mut transcript = None;
        let mut sent_samples = 0usize;
        loop {
            // Anything the guest has already emitted, without blocking the feed.
            while let Ok(frame) = outgoing_rx.try_recv() {
                self.handle_stream_frame(&frame, session, typer, write_mode, &mut transcript)
                    .await;
            }

            if session.recorder_handle.is_finished() {
                break;
            }
            tokio::time::sleep(STREAM_POLL).await;

            // Same runaway guard as the sliding-window loop: signal the
            // recorder's stop channel so capture ends and the audio so far is
            // returned, rather than streaming forever.
            if session.start_time.elapsed() > MAX_TAKE {
                warn!("Recording timeout reached, signalling recorder to stop");
                if let Some(tx) = self.manual_stop_tx.read().await.as_ref() {
                    let _ = tx.send(());
                }
                break;
            }

            if !self
                .feed_new_audio(session, &mut sent_samples, incoming_tx)
                .await
            {
                break; // the session is gone; stop feeding it
            }
        }

        // Whatever landed between the last poll and the recorder stopping.
        self.feed_new_audio(session, &mut sent_samples, incoming_tx)
            .await;
        let _ = incoming_tx
            .send(WsFrame::Text(r#"{"type":"stop"}"#.to_string()))
            .await;

        // Drain to completion: the guest drops its sender when the session ends,
        // which is what closes this loop.
        while let Some(frame) = outgoing_rx.recv().await {
            self.handle_stream_frame(&frame, session, typer, write_mode, &mut transcript)
                .await;
        }
        transcript
    }

    /// Hand the session every sample captured since the last call. Returns
    /// `false` once the session is no longer accepting audio.
    async fn feed_new_audio(
        &self,
        session: &RecordingSession,
        sent_samples: &mut usize,
        incoming_tx: &tokio::sync::mpsc::Sender<WsFrame>,
    ) -> bool {
        // The take's buffer is cleared at the start and drained at the end, and
        // nothing evicts from the front, so a running index into it is stable.
        let new_samples: Vec<f32> = {
            let buffer = session.preview_buffer.lock();
            let total = buffer.len();
            if total <= *sent_samples {
                return true;
            }
            let taken = buffer.range(*sent_samples..).copied().collect();
            *sent_samples = total;
            taken
        };

        let device_rate = session.device_sample_rate;
        let samples = if device_rate == TARGET_RATE {
            new_samples
        } else {
            // Resampling is CPU work; a poll's worth of audio is ~100ms rather
            // than the sliding window's 5s, but keep it off the async worker for
            // the same reason (audit 2 Tier 3 #2).
            match tokio::task::spawn_blocking(move || {
                super_stt_shared::utils::audio::resample(
                    &new_samples,
                    device_rate,
                    TARGET_RATE,
                    super_stt_shared::audio_utils::ResampleQuality::Fast,
                )
            })
            .await
            {
                Ok(Ok(resampled)) => resampled,
                Ok(Err(e)) => {
                    warn!("Failed to resample streaming audio: {e}");
                    return true;
                }
                Err(e) => {
                    warn!("Streaming resample task panicked: {e}");
                    return true;
                }
            }
        };
        if samples.is_empty() {
            return true;
        }

        debug!(
            "Streaming {} samples to the realtime session",
            samples.len()
        );
        incoming_tx
            .send(WsFrame::Binary(to_pcm16(&samples)))
            .await
            .is_ok()
    }

    /// Route one frame from the guest: previews go where every preview goes,
    /// `done` is the transcript, `error` is logged and ends with no transcript
    /// so the caller falls back.
    async fn handle_stream_frame(
        &self,
        frame: &WsFrame,
        session: &RecordingSession,
        typer: &mut Typer,
        write_mode: bool,
        transcript: &mut Option<String>,
    ) {
        match classify_frame(frame) {
            Some(StreamEvent::Preview(text)) => {
                self.emit_preview(&text, session, typer, write_mode).await;
            }
            Some(StreamEvent::Done(text)) => {
                info!(
                    "Realtime session finished: '{}'",
                    text.chars().take(30).collect::<String>()
                );
                *transcript = Some(text);
            }
            Some(StreamEvent::Error(message)) => {
                warn!("Realtime session reported an error: {message}");
            }
            None => debug!("Ignoring unrecognized frame from the realtime session"),
        }
    }
}

/// What one consumer-protocol frame means. Split from the effects so the
/// parsing — the part with edge cases — is unit-testable without a daemon, a
/// recorder, or a microphone.
#[derive(Debug, PartialEq, Eq)]
enum StreamEvent {
    Preview(String),
    Done(String),
    Error(String),
}

/// Classify a frame from the guest, or `None` for anything the protocol does
/// not define in this direction (binary, non-JSON, unknown or missing `type`).
fn classify_frame(frame: &WsFrame) -> Option<StreamEvent> {
    // The consumer protocol is JSON text guest -> daemon; binary only goes the
    // other way.
    let WsFrame::Text(text) = frame else {
        return None;
    };
    let event: Value = serde_json::from_str(text).ok()?;
    let field = |name: &str| event.get(name).and_then(Value::as_str).map(str::to_string);
    match event.get("type").and_then(Value::as_str)? {
        // A preview with no text says nothing; there is no preview to publish.
        "preview" => field("text").map(StreamEvent::Preview),
        // A `done` without a transcription is still the end of the take: an
        // empty transcript, not a missing one.
        "done" => Some(StreamEvent::Done(
            field("transcription").unwrap_or_default(),
        )),
        "error" => Some(StreamEvent::Error(
            field("message").unwrap_or_else(|| "unknown error".to_string()),
        )),
        _ => None,
    }
}

/// f32 samples as PCM16 little-endian mono — the consumer contract's audio
/// frame.
#[allow(clippy::cast_possible_truncation)] // intentional f32 -> i16 PCM clamp
fn to_pcm16(samples: &[f32]) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        pcm.extend_from_slice(&value.to_le_bytes());
    }
    pcm
}

#[cfg(test)]
mod tests {
    use super::{StreamEvent, WsFrame, classify_frame, to_pcm16};

    fn text(frame: &str) -> WsFrame {
        WsFrame::Text(frame.to_string())
    }

    /// The three frames the consumer protocol defines guest -> daemon.
    #[test]
    fn classify_frame_reads_the_protocol_frames() {
        assert_eq!(
            classify_frame(&text(r#"{"type":"preview","text":"hello wor"}"#)),
            Some(StreamEvent::Preview("hello wor".to_string()))
        );
        assert_eq!(
            classify_frame(&text(r#"{"type":"done","transcription":"hello world"}"#)),
            Some(StreamEvent::Done("hello world".to_string()))
        );
        assert_eq!(
            classify_frame(&text(r#"{"type":"error","message":"upstream refused"}"#)),
            Some(StreamEvent::Error("upstream refused".to_string()))
        );
    }

    /// An escaped transcript must come back as the characters it denotes, not
    /// the source text — the reason this parses JSON rather than scanning it.
    #[test]
    fn classify_frame_decodes_escapes() {
        assert_eq!(
            classify_frame(&text(
                r#"{"type":"done","transcription":"line\none \"quoted\""}"#
            )),
            Some(StreamEvent::Done("line\none \"quoted\"".to_string()))
        );
        // Non-ASCII survives too; a transcript is arbitrary UTF-8.
        assert_eq!(
            classify_frame(&text(r#"{"type":"preview","text":"café — naïve"}"#)),
            Some(StreamEvent::Preview("café — naïve".to_string()))
        );
    }

    /// A `done` carrying nothing still ends the take with an empty transcript;
    /// a `preview` carrying nothing has no preview to publish.
    #[test]
    fn classify_frame_handles_missing_payloads() {
        assert_eq!(
            classify_frame(&text(r#"{"type":"done"}"#)),
            Some(StreamEvent::Done(String::new()))
        );
        assert_eq!(
            classify_frame(&text(r#"{"type":"error"}"#)),
            Some(StreamEvent::Error("unknown error".to_string()))
        );
        assert_eq!(classify_frame(&text(r#"{"type":"preview"}"#)), None);
    }

    /// Anything the protocol does not define is ignored rather than guessed at:
    /// a frame the daemon misreads as `done` would truncate a recording.
    #[test]
    fn classify_frame_ignores_everything_else() {
        assert_eq!(classify_frame(&text(r#"{"type":"session.created"}"#)), None);
        assert_eq!(classify_frame(&text(r#"{"no_type":"x"}"#)), None);
        assert_eq!(classify_frame(&text("not json at all")), None);
        assert_eq!(classify_frame(&text("")), None);
        // Binary only ever goes daemon -> guest.
        assert_eq!(classify_frame(&WsFrame::Binary(vec![0, 1, 2])), None);
    }

    /// s16le mono, clamped — the audio frame the consumer contract specifies.
    #[test]
    fn to_pcm16_encodes_little_endian_and_clamps() {
        let pcm = to_pcm16(&[0.0, 1.0, -1.0, 2.0, -2.0]);
        assert_eq!(pcm.len(), 10, "2 bytes per sample");
        let sample = |i: usize| i16::from_le_bytes([pcm[i * 2], pcm[i * 2 + 1]]);
        assert_eq!(sample(0), 0);
        assert_eq!(sample(1), i16::MAX);
        assert_eq!(sample(2), -i16::MAX);
        // Out of range clamps rather than wrapping — a wrap would invert the
        // waveform and produce audible garbage upstream.
        assert_eq!(sample(3), i16::MAX);
        assert_eq!(sample(4), -i16::MAX);
    }

    #[test]
    fn to_pcm16_of_nothing_is_nothing() {
        assert!(to_pcm16(&[]).is_empty());
    }
}
