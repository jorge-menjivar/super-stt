// SPDX-License-Identifier: GPL-3.0-only

//! Internal event bus for the widget HTTP/SSE protocol.
//!
//! `EventBus` owns one `tokio::sync::broadcast::Sender` per topic that the
//! daemon publishes to `/events` subscribers (recording state, frequency
//! bands, transcription text, daemon status). The HTTP `GET /events` handler
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
//! the topic tables in `docs/protocol/endpoints/v1/events.md` exactly.
//! Frequency-band payloads carry their `f32` slice base64-encoded into a
//! `bands_b64` field so the JSON envelope is self-contained.
//!
//! The per-topic surface — the [`Topic`] enum and its wire-name/scope
//! mappings, the [`EventBus`] senders, and the [`AnyReceiver`] wrapper — is
//! generated from the single [`event_topics!`] table below, so adding a topic
//! is a one-line change that can't drift out of sync across those sites.

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

// ---------- Event payload types ----------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct RecordingStartedEvent {
    pub client_id: String,
    pub timestamp: String,
    pub write_mode: bool,
}

/// `recording_stopped` — mic capture ended (before transcription).
#[derive(Clone, Debug, Serialize)]
pub struct RecordingStoppedEvent {
    pub client_id: String,
    pub timestamp: String,
}

/// `transcribing_started` — model decode of the captured audio began.
#[derive(Clone, Debug, Serialize)]
pub struct TranscribingStartedEvent {
    pub client_id: String,
    pub timestamp: String,
}

/// `transcribing_stopped` — decode + typing finished; carries the outcome.
#[derive(Clone, Debug, Serialize)]
pub struct TranscribingStoppedEvent {
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

/// `registry_install` carries a serialized `RegistryEvent` payload
/// (install.progress / install.completed / install.failed / refresh.completed /
/// refresh.failed). Stored as a raw `Value` for the same reason as
/// `DaemonStatusChangedEvent` — avoids a cyclic dep between events.rs and
/// the registry types. Settings-scope only.
pub type RegistryInstallEvent = serde_json::Value;

// ---------- Topic table ------------------------------------------------------

/// Generate the per-topic surface from a single table: the [`Topic`] enum and
/// its `as_str` / `from_wire` / `required_scope` mappings, the [`EventBus`]
/// senders + `Default` + `subscribe`, and the [`AnyReceiver`] wrapper. Adding a
/// topic is one row here; the mappings can't drift out of sync.
macro_rules! event_topics {
    (
        $(
            $(#[$vmeta:meta])*
            $Variant:ident {
                wire: $wire:literal,
                scope: $scope:literal,
                field: $field:ident,
                payload: $Payload:ty,
                capacity: $cap:expr,
            }
        ),+ $(,)?
    ) => {
        /// Set of topics the daemon emits over `GET /events`. The `as_str`
        /// mapping is the wire name used in the `event:` line of each SSE
        /// frame; each topic's [`Topic::required_scope`] gates who may
        /// subscribe to it.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Topic {
            $( $(#[$vmeta])* $Variant, )+
        }

        impl Topic {
            /// Wire name (matches the SSE `event:` line and the `?topics=` query value).
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$Variant => $wire, )+ }
            }

            /// Parse a wire-name back into a `Topic`. Returns `None` for unknown
            /// strings; callers translate that to `400 invalid_topic`. Named
            /// `from_wire` rather than `from_str` to avoid confusion with the
            /// `std::str::FromStr` trait method.
            #[must_use]
            pub fn from_wire(s: &str) -> Option<Self> {
                match s {
                    $( $wire => Some(Self::$Variant), )+
                    _ => None,
                }
            }

            /// The scope a token must hold to subscribe to this topic on
            /// `GET /events`. Single source of truth for the topic→scope gate.
            #[must_use]
            pub const fn required_scope(self) -> &'static str {
                match self { $( Self::$Variant => $scope, )+ }
            }
        }

        /// One `broadcast::Sender` per topic. The bus is held on `SuperSTTDaemon`
        /// behind `Arc`; clones share the underlying senders.
        #[derive(Clone)]
        pub struct EventBus {
            $( $field: broadcast::Sender<$Payload>, )+
        }

        impl Default for EventBus {
            fn default() -> Self {
                $( let ($field, _) = broadcast::channel($cap); )+
                Self { $( $field, )+ }
            }
        }

        impl EventBus {
            /// Subscribe to a topic. Returns a typed `broadcast::Receiver` whose
            /// item type is the event payload struct (also `Serialize`, so the
            /// `/events` handler can pass it directly to `serde_json::to_value`).
            ///
            /// Each call returns an independent receiver — multiple widgets can
            /// subscribe to the same topic concurrently.
            #[must_use]
            pub fn subscribe(&self, topic: Topic) -> AnyReceiver {
                match topic {
                    $( Topic::$Variant => AnyReceiver::$Variant(self.$field.subscribe()), )+
                }
            }
        }

        /// Heterogeneous receiver wrapper so the `/events` handler can hold a
        /// `Vec<AnyReceiver>` keyed by `Topic` and `select` across them in one
        /// loop. Each variant carries its typed `broadcast::Receiver`.
        pub enum AnyReceiver {
            $( $Variant(broadcast::Receiver<$Payload>), )+
        }
    };
}

event_topics! {
    RecordingStarted {
        wire: "recording_started", scope: "recording_events",
        field: recording_started, payload: RecordingStartedEvent, capacity: STATE_BUF_CAPACITY,
    },
    RecordingStopped {
        wire: "recording_stopped", scope: "recording_events",
        field: recording_stopped, payload: RecordingStoppedEvent, capacity: STATE_BUF_CAPACITY,
    },
    RecordingState {
        wire: "recording_state", scope: "recording_events",
        field: recording_state, payload: RecordingStateEvent, capacity: STATE_BUF_CAPACITY,
    },
    TranscribingStarted {
        wire: "transcribing_started", scope: "recording_events",
        field: transcribing_started, payload: TranscribingStartedEvent, capacity: STATE_BUF_CAPACITY,
    },
    TranscribingStopped {
        wire: "transcribing_stopped", scope: "recording_events",
        field: transcribing_stopped, payload: TranscribingStoppedEvent, capacity: STATE_BUF_CAPACITY,
    },
    FrequencyBands {
        wire: "frequency_bands", scope: "audio_visualization",
        field: frequency_bands, payload: FrequencyBandsEvent, capacity: AUDIO_BUF_CAPACITY,
    },
    PartialStt {
        wire: "partial_stt", scope: "global_transcriptions",
        field: partial_stt, payload: SttEvent, capacity: STATE_BUF_CAPACITY,
    },
    FinalStt {
        wire: "final_stt", scope: "global_transcriptions",
        field: final_stt, payload: SttEvent, capacity: STATE_BUF_CAPACITY,
    },
    DaemonStatusChanged {
        wire: "daemon_status_changed", scope: "daemon_status",
        field: daemon_status_changed, payload: DaemonStatusChangedEvent, capacity: STATE_BUF_CAPACITY,
    },
    DownloadProgress {
        wire: "download_progress", scope: "daemon_status",
        field: download_progress, payload: DownloadProgressEvent, capacity: STATE_BUF_CAPACITY,
    },
    /// Registry install / refresh progress events. Requires the `daemon_status`
    /// scope (same as `DaemonStatusChanged`).
    RegistryInstall {
        wire: "registry_install", scope: "daemon_status",
        field: registry_install, payload: RegistryInstallEvent, capacity: STATE_BUF_CAPACITY,
    },
}

// ---------- The bus: construction + publish API ------------------------------

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

    /// Publish a `transcribing_started` event (paired with `publish_transcribing_stopped`).
    pub fn publish_transcribing_started(&self, evt: TranscribingStartedEvent) {
        let _ = self.transcribing_started.send(evt);
    }

    /// Publish a `transcribing_stopped` event (paired with `publish_transcribing_started`).
    pub fn publish_transcribing_stopped(&self, evt: TranscribingStoppedEvent) {
        let _ = self.transcribing_stopped.send(evt);
    }

    pub fn publish_recording_state(&self, is_recording: bool) {
        let _ = self
            .recording_state
            .send(RecordingStateEvent { is_recording });
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
    /// ... }`, `{ status: "loading_model", ... }`, etc. Subscribers need
    /// the `daemon_status` scope (gated by [`Topic::required_scope`]).
    pub fn publish_daemon_status_changed(&self, data: serde_json::Value) {
        let _ = self.daemon_status_changed.send(data);
    }

    /// Publish a typed [`DaemonStatusEvent`], injecting the `timestamp` every
    /// event carries (mirroring [`Self::publish_download_progress`]). Serializing
    /// the enum is the single construction path, so producers no longer hand-build
    /// `json!` maps whose keys can silently drift (audit 2 Tier 2 #9).
    pub fn publish_daemon_status(
        &self,
        event: super_stt_shared::models::protocol::DaemonStatusEvent,
    ) {
        let mut payload = serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "timestamp".into(),
                serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
            );
        }
        self.publish_daemon_status_changed(payload);
    }

    /// Publish a `download_progress` event. Payload is the JSON shape
    /// the legacy `notification_manager.broadcast_event("download_progress",...)`
    /// used — the keys of `DownloadProgress` plus a `timestamp`. Requires
    /// the `daemon_status` scope: see [`publish_daemon_status_changed`].
    pub fn publish_download_progress(&self, data: serde_json::Value) {
        let _ = self.download_progress.send(data);
    }

    /// Publish a `registry_install` event.  Payload is a serialized
    /// `super_stt_shared::registry::events::RegistryEvent`. Requires the
    /// `daemon_status` scope.
    pub fn publish_registry_install(&self, data: serde_json::Value) {
        let _ = self.registry_install.send(data);
    }
}

impl AnyReceiver {
    /// Receive the next event for this topic as `(wire_name, json_data)`, where
    /// `json_data` is the serialized SSE `data:` payload ready for the frame
    /// formatter. Bubbles `RecvError` so the handler can log+continue (lag) or
    /// close (closed).
    ///
    /// The typed topics are serialized straight to their JSON string, skipping the
    /// throwaway `serde_json::Value` the old path built (audit 2 Tier 3 #4) —
    /// notably `frequency_bands`, emitted at the audio-callback rate, where every
    /// visualization subscriber otherwise paid a discarded `Value` allocation plus
    /// a second full serialization per frame. The 3 heterogeneous topics already
    /// carry a `Value`; serializing it here is the same work the old
    /// `format_sse_frame` did.
    ///
    /// Note: a typed `f32` now serializes in its own shortest round-trip form
    /// (e.g. `0.95`) rather than the f64-widened form the `to_value` hop produced
    /// (`0.949999988079071`); both parse back to the same value.
    ///
    /// # Errors
    /// Returns `RecvError::Lagged(n)` when the receiver fell behind the channel
    /// capacity (the SSE handler logs and resyncs). Returns `RecvError::Closed`
    /// when all senders have been dropped.
    pub async fn recv_json_str(
        &mut self,
    ) -> Result<(&'static str, String), broadcast::error::RecvError> {
        macro_rules! recv_arm {
            ($rx:ident, $topic:ident) => {{
                let evt = $rx.recv().await?;
                Ok((
                    Topic::$topic.as_str(),
                    serde_json::to_string(&evt).unwrap_or_default(),
                ))
            }};
        }
        match self {
            Self::RecordingStarted(rx) => recv_arm!(rx, RecordingStarted),
            Self::RecordingStopped(rx) => recv_arm!(rx, RecordingStopped),
            Self::RecordingState(rx) => recv_arm!(rx, RecordingState),
            Self::TranscribingStarted(rx) => recv_arm!(rx, TranscribingStarted),
            Self::TranscribingStopped(rx) => recv_arm!(rx, TranscribingStopped),
            Self::FrequencyBands(rx) => recv_arm!(rx, FrequencyBands),
            Self::PartialStt(rx) => recv_arm!(rx, PartialStt),
            Self::FinalStt(rx) => recv_arm!(rx, FinalStt),
            Self::DaemonStatusChanged(rx) => recv_arm!(rx, DaemonStatusChanged),
            Self::DownloadProgress(rx) => recv_arm!(rx, DownloadProgress),
            Self::RegistryInstall(rx) => recv_arm!(rx, RegistryInstall),
        }
    }

    /// [`recv_json_str`](Self::recv_json_str) parsed back into a
    /// `serde_json::Value` — used by tests that assert on structured fields.
    ///
    /// # Errors
    /// See [`recv_json_str`](Self::recv_json_str).
    #[cfg(test)]
    pub async fn recv_json(
        &mut self,
    ) -> Result<(&'static str, serde_json::Value), broadcast::error::RecvError> {
        let (name, json) = self.recv_json_str().await?;
        Ok((name, serde_json::from_str(&json).unwrap_or_default()))
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
        let AnyReceiver::RecordingState(mut fast_rx) = bus.subscribe(Topic::RecordingState) else {
            unreachable!("subscribe(RecordingState) returns the matching variant")
        };
        let AnyReceiver::RecordingState(mut slow_rx) = bus.subscribe(Topic::RecordingState) else {
            unreachable!("subscribe(RecordingState) returns the matching variant")
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
            Topic::TranscribingStarted,
            Topic::TranscribingStopped,
            Topic::FrequencyBands,
            Topic::PartialStt,
            Topic::FinalStt,
            Topic::DaemonStatusChanged,
            Topic::DownloadProgress,
            Topic::RegistryInstall,
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

    #[tokio::test]
    async fn transcribing_stopped_round_trip() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe(Topic::TranscribingStopped);
        bus.publish_transcribing_stopped(TranscribingStoppedEvent {
            client_id: "test-client".into(),
            timestamp: "2026-06-08T00:00:00Z".into(),
            transcription_success: true,
            error: None,
        });
        let (topic, payload) = rx.recv_json().await.expect("should receive");
        assert_eq!(topic, "transcribing_stopped");
        assert_eq!(payload["client_id"], serde_json::json!("test-client"));
        assert_eq!(payload["transcription_success"], serde_json::json!(true));
        assert!(
            payload.get("error").is_none(),
            "None fields must be omitted"
        );
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

    #[test]
    fn required_scope_maps_every_topic() {
        assert_eq!(Topic::RecordingStarted.required_scope(), "recording_events");
        assert_eq!(Topic::RecordingStopped.required_scope(), "recording_events");
        assert_eq!(Topic::RecordingState.required_scope(), "recording_events");
        assert_eq!(
            Topic::TranscribingStarted.required_scope(),
            "recording_events"
        );
        assert_eq!(
            Topic::TranscribingStopped.required_scope(),
            "recording_events"
        );
        assert_eq!(
            Topic::FrequencyBands.required_scope(),
            "audio_visualization"
        );
        assert_eq!(Topic::PartialStt.required_scope(), "global_transcriptions");
        assert_eq!(Topic::FinalStt.required_scope(), "global_transcriptions");
        assert_eq!(Topic::DaemonStatusChanged.required_scope(), "daemon_status");
        assert_eq!(Topic::DownloadProgress.required_scope(), "daemon_status");
        assert_eq!(Topic::RegistryInstall.required_scope(), "daemon_status");
    }

    #[test]
    fn required_scope_matches_shared_mapping() {
        // The daemon enum and the client-facing helper in `super-stt-shared`
        // must agree on every topic's scope, so a client validating its
        // (scopes, topics) against the shared mapping sees the same gate the
        // daemon enforces here. This pins the two sources of truth together.
        use super_stt_shared::daemon::widget_subscription::required_scope_for_topic;
        for topic in [
            Topic::RecordingStarted,
            Topic::RecordingStopped,
            Topic::RecordingState,
            Topic::TranscribingStarted,
            Topic::TranscribingStopped,
            Topic::FrequencyBands,
            Topic::PartialStt,
            Topic::FinalStt,
            Topic::DaemonStatusChanged,
            Topic::DownloadProgress,
            Topic::RegistryInstall,
        ] {
            assert_eq!(
                required_scope_for_topic(topic.as_str()),
                Some(topic.required_scope()),
                "shared mapping disagrees with Topic::required_scope for {}",
                topic.as_str()
            );
        }
    }
}
