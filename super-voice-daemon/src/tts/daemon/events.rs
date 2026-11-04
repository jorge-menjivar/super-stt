// SPDX-License-Identifier: GPL-3.0-only

//! Text to speech's events on the one bus: the playback topics' payloads and
//! the methods that publish them, and the synthesis stage's status. The
//! topics themselves are rows of the table in [`crate::daemon::events`].
//!
//! The wire shape of each topic is set by the structs below and matches
//! the topic tables in `docs/protocol/endpoints/v1/events.md` exactly.
//!
//! [`event_topics!`]: super_engine_daemon::event_topics

use serde::Serialize;

// ---------- Event payload types ----------------------------------------------

/// Payload of the `speaking_state` topic.
#[derive(Clone, Debug, Serialize)]
pub struct SpeakingStateEvent {
    /// Whether an utterance is currently being played.
    pub is_speaking: bool,
    /// The utterance being played, absent when speech has stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utterance_id: Option<String>,
}

/// Payload of the `speech_progress` topic.
///
/// Both figures are positions within the utterance derived from the device
/// format, not wall-clock time, so they stay correct while the stream is idle.
#[derive(Clone, Debug, Serialize)]
pub struct SpeechProgressEvent {
    /// The utterance these figures describe.
    pub utterance_id: String,
    /// Milliseconds already handed to the output device.
    pub spoken_ms: u64,
    /// Milliseconds queued and not yet played.
    pub queued_ms: u64,
}

// ---------- Topic table ------------------------------------------------------

// The playback topics are rows of the one table, in `crate::daemon::events`.
pub use crate::daemon::events::{AnyReceiver, EventBus, Topic};

// ---------- Publish API --------------------------------------------------------

impl EventBus {
    // The playback topics' own publishers are beside the bus's fields, in
    // `crate::daemon::events`; this one goes through the core topic.

    /// Publish a typed [`DaemonStatusEvent`], injecting the `timestamp` every
    /// event carries (mirroring [`Self::publish_download_progress`]). Serializing
    /// the enum is the single construction path, so producers no longer hand-build
    /// `json!` maps whose keys can silently drift (audit 2 Tier 2 #9).
    pub fn publish_synthesis_status(
        &self,
        event: super_tts_shared::models::protocol::DaemonStatusEvent,
    ) {
        let mut payload = serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "timestamp".into(),
                serde_json::Value::String(chrono::Utc::now().to_rfc3339()),
            );
            obj.insert("feature".into(), serde_json::Value::String("tts".into()));
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
        let mut rx = bus.subscribe(Topic::SpeakingState);
        bus.publish_speaking_state(true, Some("u-1".to_string()));
        let (topic, payload) = rx.recv_json().await.expect("should receive");
        assert_eq!(topic, "speaking_state");
        assert_eq!(payload["is_speaking"], serde_json::json!(true));
        assert_eq!(payload["utterance_id"], serde_json::json!("u-1"));
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_silent() {
        let bus = EventBus::new();
        // No subscriber for speech_progress — call must not panic / propagate.
        bus.publish_speech_progress("u-1".into(), 120, 480);
    }

    /// A `None` on an optional payload field is omitted from the wire rather
    /// than sent as `null`, so a consumer can use key presence as the test.
    #[tokio::test]
    async fn an_absent_optional_field_is_omitted_from_the_payload() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe(Topic::SpeakingState);
        bus.publish_speaking_state(false, None);
        let (topic, payload) = rx.recv_json().await.expect("should receive");
        assert_eq!(topic, "speaking_state");
        assert_eq!(payload["is_speaking"], serde_json::json!(false));
        assert!(
            payload.get("utterance_id").is_none(),
            "None fields must be omitted"
        );
    }
}

#[cfg(test)]
mod playback_topic_tests {
    use super::{EventBus, Topic};

    #[tokio::test]
    async fn speaking_state_publishes_and_receives() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe(Topic::SpeakingState);
        bus.publish_speaking_state(true, Some("utt_1".into()));
        let (topic, json) = rx.recv_json().await.expect("received");
        assert_eq!(topic, "speaking_state");
        assert_eq!(
            json,
            serde_json::json!({ "is_speaking": true, "utterance_id": "utt_1" })
        );
    }

    /// A stop with no id is legal — the daemon knows it stopped speaking even
    /// when it has already released the utterance — and the field is omitted
    /// rather than sent as null.
    #[tokio::test]
    async fn a_stop_without_an_utterance_omits_the_id() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe(Topic::SpeakingState);
        bus.publish_speaking_state(false, None);
        let (_, json) = rx.recv_json().await.expect("received");
        assert_eq!(json, serde_json::json!({ "is_speaking": false }));
        assert!(
            json.get("utterance_id").is_none(),
            "an absent id is omitted, not sent as null"
        );
    }

    #[tokio::test]
    async fn speech_progress_publishes_and_receives() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe(Topic::SpeechProgress);
        bus.publish_speech_progress("utt_1".into(), 120, 880);
        let (topic, json) = rx.recv_json().await.expect("received");
        assert_eq!(topic, "speech_progress");
        assert_eq!(
            json,
            serde_json::json!({
                "utterance_id": "utt_1",
                "spoken_ms": 120,
                "queued_ms": 880,
            })
        );
    }

    /// Playback events are gated separately from the visualizer feed. An applet
    /// that draws a waveform holds `audio_visualization`; knowing *what* the
    /// daemon is speaking and how far through it is, is a different disclosure
    /// and must cost a second grant.
    #[test]
    fn playback_topics_need_their_own_scope() {
        assert_eq!(Topic::SpeakingState.required_scope(), "playback_events");
        assert_eq!(Topic::SpeechProgress.required_scope(), "playback_events");
        assert_ne!(
            Topic::SpeakingState.required_scope(),
            Topic::FrequencyBands.required_scope(),
            "the visualizer grant must not also carry utterance state"
        );
    }
}
