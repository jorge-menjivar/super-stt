// SPDX-License-Identifier: GPL-3.0-only

//! Internal event bus for the widget HTTP/SSE protocol.
//!
//! `EventBus` owns one `tokio::sync::broadcast::Sender` per topic that the
//! daemon publishes to widget subscribers (recording state, audio frames,
//! frequency bands, transcription text). The HTTP `GET /events` handler
//! subscribes to whichever topics the client requested and forwards each
//! event as an SSE frame.
//!
//! `tokio::sync::broadcast` is multi-subscriber by construction: every
//! `subscribe()` call returns an independent `Receiver` reading into the
//! same ring buffer at its own position. A slow subscriber gets
//! `RecvError::Lagged(n)` and skips ahead — the producer (the audio
//! capture pipeline) never blocks, and other subscribers are unaffected.
//!
//! The wire shape of each topic is set by the structs below and matches
//! `docs/protocol/widget.md` §"Topics" exactly. Audio and frequency-band
//! payloads carry their `f32` slice base64-encoded into a `_b64` field
//! so the JSON envelope is self-contained.

use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::Serialize;
use tokio::sync::broadcast;

/// Ring-buffer depth for each topic. These bound the *replay window* —
/// how far behind a slow subscriber can fall before the broadcast
/// channel starts dropping its oldest entries. Memory is `capacity ×
/// sizeof::<Event>` per channel total, **not** multiplied by subscriber
/// count.
const AUDIO_BUF_CAPACITY: usize = 256;
const STATE_BUF_CAPACITY: usize = 32;

/// Set of topics the daemon emits over `GET /events`. The `as_str` mapping
/// is the wire name used in the `event:` line of each SSE frame.
///
/// `DaemonStatusChanged` and `DownloadProgress` are **settings-only**
/// — `WIDGET_TOPICS` in `http_server.rs` doesn't include them, so a
/// widget-scope token requesting either gets `403 scope_denied`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Topic {
    RecordingStarted,
    RecordingStopped,
    RecordingState,
    AudioSamples,
    FrequencyBands,
    PartialStt,
    FinalStt,
    DaemonStatusChanged,
    DownloadProgress,
}

impl Topic {
    /// Wire name (matches the SSE `event:` line and the `?topics=` query value).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RecordingStarted => "recording_started",
            Self::RecordingStopped => "recording_stopped",
            Self::RecordingState => "recording_state",
            Self::AudioSamples => "audio_samples",
            Self::FrequencyBands => "frequency_bands",
            Self::PartialStt => "partial_stt",
            Self::FinalStt => "final_stt",
            Self::DaemonStatusChanged => "daemon_status_changed",
            Self::DownloadProgress => "download_progress",
        }
    }

    /// Parse a wire-name back into a `Topic`. Returns `None` for unknown
    /// strings; callers translate that to `400 invalid_topic`. Named
    /// `from_wire` rather than `from_str` to avoid confusion with the
    /// `std::str::FromStr` trait method.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "recording_started" => Some(Self::RecordingStarted),
            "recording_stopped" => Some(Self::RecordingStopped),
            "recording_state" => Some(Self::RecordingState),
            "audio_samples" => Some(Self::AudioSamples),
            "frequency_bands" => Some(Self::FrequencyBands),
            "partial_stt" => Some(Self::PartialStt),
            "final_stt" => Some(Self::FinalStt),
            "daemon_status_changed" => Some(Self::DaemonStatusChanged),
            "download_progress" => Some(Self::DownloadProgress),
            _ => None,
        }
    }
}

// ---------- Event payload types ----------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RecordingStartedEvent {
    pub client_id: String,
    pub timestamp: String,
    pub write_mode: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingStoppedEvent {
    pub client_id: String,
    pub timestamp: String,
    pub transcription_success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RecordingStateEvent {
    pub is_recording: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AudioSamplesEvent {
    pub sample_rate: f32,
    pub channels: u16,
    pub samples_b64: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FrequencyBandsEvent {
    pub bands_b64: String,
    pub sample_rate: f32,
    pub total_energy: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct SttEvent {
    pub text: String,
    pub confidence: f32,
}

/// `daemon_status_changed` carries a heterogeneous payload: the `status`
/// discriminator selects between `loading_model`, `ready`,
/// `model_switched`, `switching_device`, `device_switch_error`, etc.
/// Each variant has its own keys (`model_loaded`, `actual_device`,
/// `target_device`, …). Storing this as `serde_json::Value` keeps the
/// shape identical to the legacy notification-manager broadcast so the
/// settings app's consumer doesn't have to change.
pub type DaemonStatusChangedEvent = serde_json::Value;

/// `download_progress` mirrors `DownloadProgress` plus a `timestamp`.
/// Same rationale as above — we hand the consumer the legacy JSON
/// shape and let it deserialize into
/// `super_stt_shared::models::protocol::DownloadProgress`.
pub type DownloadProgressEvent = serde_json::Value;

// ---------- The bus ----------------------------------------------------------

/// One `broadcast::Sender` per topic. The bus is held on `SuperSTTDaemon`
/// behind `Arc`; clones share the underlying senders.
#[derive(Clone)]
pub struct EventBus {
    recording_started: broadcast::Sender<RecordingStartedEvent>,
    recording_stopped: broadcast::Sender<RecordingStoppedEvent>,
    recording_state: broadcast::Sender<RecordingStateEvent>,
    audio_samples: broadcast::Sender<AudioSamplesEvent>,
    frequency_bands: broadcast::Sender<FrequencyBandsEvent>,
    partial_stt: broadcast::Sender<SttEvent>,
    final_stt: broadcast::Sender<SttEvent>,
    daemon_status_changed: broadcast::Sender<DaemonStatusChangedEvent>,
    download_progress: broadcast::Sender<DownloadProgressEvent>,
}

impl Default for EventBus {
    fn default() -> Self {
        let (recording_started, _) = broadcast::channel(STATE_BUF_CAPACITY);
        let (recording_stopped, _) = broadcast::channel(STATE_BUF_CAPACITY);
        let (recording_state, _) = broadcast::channel(STATE_BUF_CAPACITY);
        let (audio_samples, _) = broadcast::channel(AUDIO_BUF_CAPACITY);
        let (frequency_bands, _) = broadcast::channel(AUDIO_BUF_CAPACITY);
        let (partial_stt, _) = broadcast::channel(STATE_BUF_CAPACITY);
        let (final_stt, _) = broadcast::channel(STATE_BUF_CAPACITY);
        let (daemon_status_changed, _) = broadcast::channel(STATE_BUF_CAPACITY);
        let (download_progress, _) = broadcast::channel(STATE_BUF_CAPACITY);
        Self {
            recording_started,
            recording_stopped,
            recording_state,
            audio_samples,
            frequency_bands,
            partial_stt,
            final_stt,
            daemon_status_changed,
            download_progress,
        }
    }
}

impl EventBus {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ---------- Publish API ------------------------------------------------
    //
    // All `publish_*` calls are synchronous and best-effort. `broadcast::send`
    // returns `Err(SendError(_))` only when no subscribers exist; we drop it
    // because that's the steady state when no widget is connected.

    pub fn publish_recording_started(&self, evt: RecordingStartedEvent) {
        let _ = self.recording_started.send(evt);
    }

    pub fn publish_recording_stopped(&self, evt: RecordingStoppedEvent) {
        let _ = self.recording_stopped.send(evt);
    }

    pub fn publish_recording_state(&self, is_recording: bool) {
        let _ = self
            .recording_state
            .send(RecordingStateEvent { is_recording });
    }

    pub fn publish_audio_samples(&self, samples: &[f32], sample_rate: f32, channels: u16) {
        let _ = self.audio_samples.send(AudioSamplesEvent {
            sample_rate,
            channels,
            samples_b64: encode_f32_b64(samples),
        });
    }

    pub fn publish_frequency_bands(&self, bands: &[f32], sample_rate: f32, total_energy: f32) {
        let _ = self.frequency_bands.send(FrequencyBandsEvent {
            bands_b64: encode_f32_b64(bands),
            sample_rate,
            total_energy,
        });
    }

    pub fn publish_partial_stt(&self, text: String, confidence: f32) {
        let _ = self.partial_stt.send(SttEvent { text, confidence });
    }

    pub fn publish_final_stt(&self, text: String, confidence: f32) {
        let _ = self.final_stt.send(SttEvent { text, confidence });
    }

    /// Publish a `daemon_status_changed` event. Payload is whatever the
    /// legacy callers built — `{ status: "ready", model_loaded: true,
    /// ... }`, `{ status: "loading_model", ... }`, etc. Settings-scope
    /// only: the SSE router refuses widget tokens for this topic.
    pub fn publish_daemon_status_changed(&self, data: serde_json::Value) {
        let _ = self.daemon_status_changed.send(data);
    }

    /// Publish a `download_progress` event. Payload is the JSON shape
    /// the legacy `notification_manager.broadcast_event("download_progress",...)`
    /// used — the keys of `DownloadProgress` plus a `timestamp`.
    /// Settings-scope only: see [`publish_daemon_status_changed`].
    pub fn publish_download_progress(&self, data: serde_json::Value) {
        let _ = self.download_progress.send(data);
    }

    // ---------- Subscribe API ----------------------------------------------

    /// Subscribe to a topic. Returns a typed `broadcast::Receiver` whose
    /// item type is the event payload struct (also `Serialize`, so the
    /// `/events` handler can pass it directly to `serde_json::to_value`).
    ///
    /// Each call returns an independent receiver — multiple widgets can
    /// subscribe to the same topic concurrently.
    #[must_use]
    pub fn subscribe(&self, topic: Topic) -> AnyReceiver {
        match topic {
            Topic::RecordingStarted => {
                AnyReceiver::RecordingStarted(self.recording_started.subscribe())
            }
            Topic::RecordingStopped => {
                AnyReceiver::RecordingStopped(self.recording_stopped.subscribe())
            }
            Topic::RecordingState => AnyReceiver::RecordingState(self.recording_state.subscribe()),
            Topic::AudioSamples => AnyReceiver::AudioSamples(self.audio_samples.subscribe()),
            Topic::FrequencyBands => AnyReceiver::FrequencyBands(self.frequency_bands.subscribe()),
            Topic::PartialStt => AnyReceiver::PartialStt(self.partial_stt.subscribe()),
            Topic::FinalStt => AnyReceiver::FinalStt(self.final_stt.subscribe()),
            Topic::DaemonStatusChanged => {
                AnyReceiver::DaemonStatusChanged(self.daemon_status_changed.subscribe())
            }
            Topic::DownloadProgress => {
                AnyReceiver::DownloadProgress(self.download_progress.subscribe())
            }
        }
    }
}

/// Heterogeneous receiver wrapper so the `/events` handler can hold a
/// `Vec<AnyReceiver>` keyed by `Topic` and `select` across them in one
/// loop. Each variant carries its typed `broadcast::Receiver`.
pub enum AnyReceiver {
    RecordingStarted(broadcast::Receiver<RecordingStartedEvent>),
    RecordingStopped(broadcast::Receiver<RecordingStoppedEvent>),
    RecordingState(broadcast::Receiver<RecordingStateEvent>),
    AudioSamples(broadcast::Receiver<AudioSamplesEvent>),
    FrequencyBands(broadcast::Receiver<FrequencyBandsEvent>),
    PartialStt(broadcast::Receiver<SttEvent>),
    FinalStt(broadcast::Receiver<SttEvent>),
    DaemonStatusChanged(broadcast::Receiver<DaemonStatusChangedEvent>),
    DownloadProgress(broadcast::Receiver<DownloadProgressEvent>),
}

impl AnyReceiver {
    /// Receive the next event for this topic. Returns the wire topic
    /// name and a `serde_json::Value` payload, ready for the SSE
    /// formatter. Bubbles `RecvError` so the handler can decide whether
    /// to log+continue (lag) or close (closed).
    ///
    /// # Errors
    /// Returns `RecvError::Lagged(n)` when the receiver fell behind the
    /// channel capacity (the SSE handler logs and resyncs). Returns
    /// `RecvError::Closed` when all senders have been dropped.
    pub async fn recv_json(
        &mut self,
    ) -> Result<(&'static str, serde_json::Value), broadcast::error::RecvError> {
        macro_rules! recv_arm {
            ($rx:ident, $topic:ident) => {{
                let evt = $rx.recv().await?;
                Ok((
                    Topic::$topic.as_str(),
                    serde_json::to_value(evt).unwrap_or_default(),
                ))
            }};
        }
        match self {
            Self::RecordingStarted(rx) => recv_arm!(rx, RecordingStarted),
            Self::RecordingStopped(rx) => recv_arm!(rx, RecordingStopped),
            Self::RecordingState(rx) => recv_arm!(rx, RecordingState),
            Self::AudioSamples(rx) => recv_arm!(rx, AudioSamples),
            Self::FrequencyBands(rx) => recv_arm!(rx, FrequencyBands),
            Self::PartialStt(rx) => recv_arm!(rx, PartialStt),
            Self::FinalStt(rx) => recv_arm!(rx, FinalStt),
            Self::DaemonStatusChanged(rx) => {
                let value = rx.recv().await?;
                Ok((Topic::DaemonStatusChanged.as_str(), value))
            }
            Self::DownloadProgress(rx) => {
                let value = rx.recv().await?;
                Ok((Topic::DownloadProgress.as_str(), value))
            }
        }
    }
}

// ---------- Helpers ----------------------------------------------------------

/// Encode an `f32` slice as little-endian bytes, then base64. Matches the
/// shape decoders expect on the widget side.
fn encode_f32_b64(samples: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for &s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    B64.encode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f32_slice_from_b64(b64: &str) -> Vec<f32> {
        let bytes = B64.decode(b64).expect("valid base64");
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[tokio::test]
    async fn single_subscriber_round_trip() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe(Topic::RecordingState);
        bus.publish_recording_state(true);
        let (topic, payload) = rx.recv_json().await.expect("should receive");
        assert_eq!(topic, "recording_state");
        assert_eq!(payload["is_recording"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn fan_out_to_three_subscribers() {
        let bus = EventBus::new();
        let mut rx_a = bus.subscribe(Topic::FrequencyBands);
        let mut rx_b = bus.subscribe(Topic::FrequencyBands);
        let mut rx_c = bus.subscribe(Topic::FrequencyBands);

        bus.publish_frequency_bands(&[1.0, 2.0, 3.0], 16_000.0, 4.5);

        for rx in [&mut rx_a, &mut rx_b, &mut rx_c] {
            let (topic, payload) = rx.recv_json().await.expect("should receive");
            assert_eq!(topic, "frequency_bands");
            let bands = f32_slice_from_b64(payload["bands_b64"].as_str().unwrap());
            assert_eq!(bands, vec![1.0, 2.0, 3.0]);
            let total = payload["total_energy"].as_f64().unwrap();
            assert!((total - 4.5).abs() < 1e-6);
        }
    }

    #[tokio::test]
    async fn slow_subscriber_lags_without_blocking_others() {
        // After overflow there's nothing publishing, so we drain
        // non-blockingly (`try_recv`) — calling `recv().await` on an
        // empty channel with no senders dropped would block forever.
        let bus = EventBus::new();
        let mut fast_rx = match bus.subscribe(Topic::RecordingState) {
            AnyReceiver::RecordingState(rx) => rx,
            _ => unreachable!("subscribe(RecordingState) returns the matching variant"),
        };
        let mut slow_rx = match bus.subscribe(Topic::RecordingState) {
            AnyReceiver::RecordingState(rx) => rx,
            _ => unreachable!(),
        };

        // Push enough state changes to overflow the STATE_BUF_CAPACITY-sized ring.
        for i in 0..(STATE_BUF_CAPACITY * 2) {
            bus.publish_recording_state(i % 2 == 0);
        }

        // Fast receiver: drain non-blockingly until empty. Tolerate a
        // single `Lagged` (overflow recovery) but expect to ultimately
        // receive several values.
        let mut fast_received = 0;
        let mut fast_lagged = false;
        loop {
            match fast_rx.try_recv() {
                Ok(_) => fast_received += 1,
                Err(broadcast::error::TryRecvError::Lagged(_)) => fast_lagged = true,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(e) => panic!("fast receiver closed: {e:?}"),
            }
        }
        // Capacity-many values land; lag is acceptable but not required.
        assert!(
            fast_received >= STATE_BUF_CAPACITY,
            "fast receiver got {fast_received}; expected at least capacity ({STATE_BUF_CAPACITY})"
        );
        let _ = fast_lagged;

        // Slow receiver: never read until after overflow → first read
        // must report Lagged.
        let first = slow_rx.try_recv();
        assert!(
            matches!(first, Err(broadcast::error::TryRecvError::Lagged(_))),
            "expected Lagged on first try_recv after overflow, got {first:?}"
        );
        // After acknowledging the lag, subsequent reads succeed against
        // the still-buffered tail.
        let mut slow_after_lag = 0;
        loop {
            match slow_rx.try_recv() {
                Ok(_) => slow_after_lag += 1,
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Lagged(_)) => {} // shouldn't repeat, but tolerate
                Err(e) => panic!("slow receiver closed: {e:?}"),
            }
        }
        assert!(
            slow_after_lag > 0,
            "slow receiver should resync after Lagged"
        );
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_silent() {
        let bus = EventBus::new();
        // No subscriber for partial_stt — call must not panic / propagate.
        bus.publish_partial_stt("hello".into(), 0.9);
    }

    #[test]
    fn topic_round_trips_through_str() {
        for t in [
            Topic::RecordingStarted,
            Topic::RecordingStopped,
            Topic::RecordingState,
            Topic::AudioSamples,
            Topic::FrequencyBands,
            Topic::PartialStt,
            Topic::FinalStt,
            Topic::DaemonStatusChanged,
            Topic::DownloadProgress,
        ] {
            assert_eq!(Topic::from_wire(t.as_str()), Some(t));
        }
        assert_eq!(Topic::from_wire("not_a_topic"), None);
    }

    #[tokio::test]
    async fn settings_only_topics_publish_and_receive() {
        let bus = EventBus::new();
        let mut status_rx = bus.subscribe(Topic::DaemonStatusChanged);
        let mut prog_rx = bus.subscribe(Topic::DownloadProgress);

        bus.publish_daemon_status_changed(serde_json::json!({
            "status": "ready",
            "model_loaded": true,
        }));
        bus.publish_download_progress(serde_json::json!({
            "model_name": "whisper-tiny",
            "percentage": 42.5,
        }));

        let (topic, payload) = status_rx.recv_json().await.expect("daemon status");
        assert_eq!(topic, "daemon_status_changed");
        assert_eq!(payload["status"], serde_json::json!("ready"));
        assert_eq!(payload["model_loaded"], serde_json::json!(true));

        let (topic, payload) = prog_rx.recv_json().await.expect("download progress");
        assert_eq!(topic, "download_progress");
        assert_eq!(payload["model_name"], serde_json::json!("whisper-tiny"));
    }

    #[test]
    fn b64_round_trip_preserves_f32_slice() {
        let original = vec![0.0_f32, -1.5, 2.5, f32::INFINITY, f32::NEG_INFINITY];
        let encoded = encode_f32_b64(&original);
        let decoded = f32_slice_from_b64(&encoded);
        assert_eq!(decoded.len(), original.len());
        for (a, b) in decoded.iter().zip(original.iter()) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}
