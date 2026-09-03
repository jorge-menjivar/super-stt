// SPDX-License-Identifier: GPL-3.0-only
//! Typed model of the `daemon_status_changed` SSE event (scope `daemon_status`).
//!
//! Historically this was a hand-built `serde_json::json!` on the daemon side and
//! a hand-matched `.get("status")` / `.get("<field>")` read on the app side with
//! no shared schema — so a field rename or a client typo was a silent runtime
//! degradation with no compile/test signal (the keys had already drifted:
//! `to_device` vs `target_device`). This enum is the single typed source of
//! truth for the discriminated union — the daemon constructs a variant and
//! serializes it, the app deserializes it and matches (audit 2 Tier 2 #9).
//!
//! Every event also carries a `timestamp` (RFC 3339); it is injected by the
//! publish path (mirroring [`crate::models::protocol::DownloadProgress`]) and
//! ignored on deserialize, so it is not modeled as a field here.

use serde::{Deserialize, Serialize};

use super::pipeline::default_stage;

/// A `daemon_status_changed` event. The wire `status` field is the discriminant;
/// variant names map to it via `rename_all = "snake_case"`.
///
/// The variants describing a stage running a model carry `stage` — the
/// `/pipeline/{stage}` position whose model the event is about. A pipeline has
/// more than one stage running models, and the events are the only way a
/// client learns about a load it did not start (the daemon's own startup load,
/// or another client's), so each says whose it is. A payload without the field
/// is stage 1's: transcription is the only stage that emitted these before it.
/// The daemon-wide facts — a settings change, an available update, the active
/// backend — carry no stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DaemonStatusEvent {
    /// A model load has begun (name only, no device context).
    LoadingModel {
        new_model: String,
        #[serde(default = "default_stage")]
        stage: u32,
    },

    /// A model is loading as part of a device switch.
    LoadingModelForDevice {
        model: String,
        target_device: String,
        #[serde(default = "default_stage")]
        stage: u32,
    },

    /// A model finished loading and is now the active model.
    ModelSwitched {
        model_name: String,
        source: String,
        actual_device: String,
        #[serde(default = "default_stage")]
        stage: u32,
    },

    /// The daemon settled into a ready state. `model_loaded` says whether a model
    /// is loaded; the optional fields are present only on the emitters that have
    /// them (a device switch carries device context; an unload carries none).
    Ready {
        model_loaded: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actual_device: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preferred_device: Option<String>,
        #[serde(default = "default_stage")]
        stage: u32,
    },

    /// A device switch has begun. `target_device` is the destination — normalized
    /// from the former `to_device` so it matches [`Self::LoadingModelForDevice`].
    SwitchingDevice {
        from_device: String,
        target_device: String,
        model: String,
        #[serde(default = "default_stage")]
        stage: u32,
    },

    /// A device switch failed; the daemon is recovering to the previous device.
    DeviceSwitchError {
        error: String,
        failed_device: String,
        model: String,
        #[serde(default = "default_stage")]
        stage: u32,
    },

    /// The active backend changed. `source` is the backend's repo id, or `null`
    /// when the active backend was cleared.
    ActiveBackendChanged {
        #[serde(default)]
        source: Option<String>,
    },

    /// A settings value changed (the app refetches the affected block).
    SettingsChanged { setting: String },

    /// A self-update check found a newer release. Clients refetch
    /// `GET /v1/update` for the full status including the installer asset.
    UpdateAvailable { latest_version: String },
}

#[cfg(test)]
mod tests {
    use super::DaemonStatusEvent;
    use crate::models::protocol::{POST_PROCESSOR_STAGE, TRANSCRIPTION_STAGE};

    /// The wire discriminant is `status`, and variant/field names match the
    /// hand-built JSON the daemon used to emit.
    #[test]
    fn model_switched_wire_shape() {
        let json = serde_json::to_value(DaemonStatusEvent::ModelSwitched {
            model_name: "whisper-tiny".into(),
            source: "github.com/super-stt/whisper".into(),
            actual_device: "cpu".into(),
            stage: TRANSCRIPTION_STAGE,
        })
        .unwrap();
        assert_eq!(json["status"], "model_switched");
        assert_eq!(json["model_name"], "whisper-tiny");
        assert_eq!(json["actual_device"], "cpu");
        assert_eq!(json["stage"], 1);
    }

    /// The wire discriminant is `status`, and the field name is
    /// `latest_version` verbatim — pins the shape against a future stray
    /// `#[serde(rename)]`.
    #[test]
    fn update_available_wire_shape() {
        let json = serde_json::to_value(DaemonStatusEvent::UpdateAvailable {
            latest_version: "v0.2.3-beta.1".into(),
        })
        .unwrap();
        assert_eq!(json["status"], "update_available");
        assert_eq!(json["latest_version"], "v0.2.3-beta.1");
    }

    /// An extra `timestamp` key (injected by the publish path) is ignored on
    /// deserialize, and `switching_device` reads back with the normalized
    /// `target_device` key.
    #[test]
    fn deserialize_ignores_timestamp_and_reads_target_device() {
        let v = serde_json::json!({
            "status": "switching_device",
            "from_device": "cpu",
            "target_device": "cuda",
            "model": "whisper-tiny",
            "timestamp": "2026-07-16T00:00:00+00:00",
        });
        match serde_json::from_value::<DaemonStatusEvent>(v).unwrap() {
            DaemonStatusEvent::SwitchingDevice { target_device, .. } => {
                assert_eq!(target_device, "cuda");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// `ready` from an unload carries only `model_loaded`; the optional device
    /// fields stay absent (not serialized).
    #[test]
    fn ready_omits_absent_optionals() {
        let json = serde_json::to_value(DaemonStatusEvent::Ready {
            model_loaded: false,
            model_name: None,
            actual_device: None,
            preferred_device: None,
            stage: TRANSCRIPTION_STAGE,
        })
        .unwrap();
        assert_eq!(json["status"], "ready");
        assert_eq!(json["model_loaded"], false);
        assert!(json.get("actual_device").is_none());
    }

    /// Stage 2 reports its own lifecycle on the same topic, and the number is
    /// what tells the two apart: a client watching the post-processor must not
    /// read a transcription model's load as its own, or the reverse.
    #[test]
    fn a_post_processors_events_carry_stage_two() {
        let json = serde_json::to_value(DaemonStatusEvent::Ready {
            model_loaded: true,
            model_name: Some("s1-mini".into()),
            actual_device: Some("cpu".into()),
            preferred_device: None,
            stage: POST_PROCESSOR_STAGE,
        })
        .unwrap();
        assert_eq!(json["stage"], 2);

        let json = serde_json::to_value(DaemonStatusEvent::LoadingModel {
            new_model: "s1-mini".into(),
            stage: POST_PROCESSOR_STAGE,
        })
        .unwrap();
        assert_eq!(json["status"], "loading_model");
        assert_eq!(json["stage"], 2);
    }

    /// A daemon older than the field emits no `stage`, and everything it
    /// emitted was stage 1's — so that is what an absent field reads as,
    /// rather than a `0` no stage answers to.
    #[test]
    fn an_absent_stage_reads_as_transcription() {
        let v = serde_json::json!({
            "status": "ready",
            "model_loaded": true,
            "model_name": "whisper-tiny",
        });
        match serde_json::from_value::<DaemonStatusEvent>(v).unwrap() {
            DaemonStatusEvent::Ready { stage, .. } => assert_eq!(stage, TRANSCRIPTION_STAGE),
            other => panic!("wrong variant: {other:?}"),
        }

        let v = serde_json::json!({ "status": "loading_model", "new_model": "whisper-tiny" });
        match serde_json::from_value::<DaemonStatusEvent>(v).unwrap() {
            DaemonStatusEvent::LoadingModel { stage, .. } => {
                assert_eq!(stage, TRANSCRIPTION_STAGE);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
