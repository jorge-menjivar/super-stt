// SPDX-License-Identifier: GPL-3.0-only

//! Super STT's event bus for the `/events` SSE stream.
//!
//! The bus, the core topics every daemon publishes (`frequency_bands`,
//! `daemon_status_changed`, `download_progress`, `registry_install`) and the
//! stream itself are `super_engine_daemon::events`, shared with Super TTS.
//! What is Super STT's is below: the recording and transcription topics,
//! their payloads, and the methods that publish them. [`event_topics!`]
//! generates the [`Topic`] enum, the [`EventBus`] and the [`AnyReceiver`]
//! from these rows plus the core ones.
//!
//! The wire shape of each topic is set by the structs below and matches
//! the topic tables in `docs/protocol/endpoints/v1/events.md` exactly.
//!
//! [`event_topics!`]: super_engine_daemon::event_topics

use serde::Serialize;
use super_engine_daemon::events::STATE_BUF_CAPACITY;
use super_stt_shared::models::protocol::PreviewSource;

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
pub struct SttEvent {
    pub text: String,
    pub confidence: f32,
    /// What a `partial_stt` text spans — set on every partial, absent on a
    /// final, which is neither kind of preview.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PreviewSource>,
}

// ---------- Topic table ------------------------------------------------------

super_engine_daemon::event_topics! {
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
    PartialStt {
        wire: "partial_stt", scope: "global_transcriptions",
        field: partial_stt, payload: SttEvent, capacity: STATE_BUF_CAPACITY,
    },
    FinalStt {
        wire: "final_stt", scope: "global_transcriptions",
        field: final_stt, payload: SttEvent, capacity: STATE_BUF_CAPACITY,
    },
}

// ---------- Publish API --------------------------------------------------------

impl EventBus {
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

    pub fn publish_partial_stt(&self, text: String, confidence: f32, source: PreviewSource) {
        let _ = self.partial_stt.send(SttEvent {
            text,
            confidence,
            source: Some(source),
        });
    }

    pub fn publish_final_stt(&self, text: String, confidence: f32) {
        let _ = self.final_stt.send(SttEvent {
            text,
            confidence,
            source: None,
        });
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn publish_with_no_subscribers_is_silent() {
        let bus = EventBus::new();
        // No subscriber for partial_stt — call must not panic / propagate.
        bus.publish_partial_stt("hello".into(), 0.9, PreviewSource::Window);
    }

    /// `source` is what tells a client whether a preview replaces the last one
    /// or extends it, so a partial carries it on the wire — and a final does
    /// not, since a final is neither.
    #[test]
    fn a_partial_carries_its_source_and_a_final_omits_it() {
        let partial = serde_json::to_value(SttEvent {
            text: "hi".into(),
            confidence: 1.0,
            source: Some(PreviewSource::Window),
        })
        .expect("serializes");
        assert_eq!(partial["source"], "window");

        let final_ = serde_json::to_value(SttEvent {
            text: "hi".into(),
            confidence: 1.0,
            source: None,
        })
        .expect("serializes");
        assert!(final_.get("source").is_none(), "got {final_}");
    }

    #[test]
    fn topic_round_trips_through_str() {
        for &t in Topic::ALL {
            assert_eq!(Topic::from_wire(t.as_str()), Some(t));
        }
        assert_eq!(Topic::from_wire("not_a_topic"), None);
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
        for &topic in Topic::ALL {
            assert_eq!(
                required_scope_for_topic(topic.as_str()),
                Some(topic.required_scope()),
                "shared mapping disagrees with Topic::required_scope for {}",
                topic.as_str()
            );
        }
    }
}
