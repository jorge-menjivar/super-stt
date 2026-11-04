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

/// A `daemon_status_changed` event. The wire `status` field is the discriminant;
/// variant names map to it via `rename_all = "snake_case"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DaemonStatusEvent {
    /// A model load has begun (name only, no device context).
    LoadingModel { new_model: String },

    /// A model is loading as part of a device switch.
    LoadingModelForDevice {
        model: String,
        target_device: String,
    },

    /// A model finished loading and is now the active model.
    ModelSwitched {
        model_name: String,
        source: String,
        actual_device: String,
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
    },

    /// A device switch has begun. `target_device` is the destination — normalized
    /// from the former `to_device` so it matches [`Self::LoadingModelForDevice`].
    SwitchingDevice {
        from_device: String,
        target_device: String,
        model: String,
    },

    /// A device switch failed; the daemon is recovering to the previous device.
    DeviceSwitchError {
        error: String,
        failed_device: String,
        model: String,
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

    /// A cloned voice is being handed to the loaded model, which has to derive
    /// from the reference clip before it can speak in it.
    ///
    /// This is why speech in a voice can be slow to start the first time: the
    /// derivation is real GPU work on a clip the model has not seen, and it
    /// happens whether it was triggered by saving the clip or by the first
    /// request to speak in it. A client that shows it can say "preparing"
    /// instead of looking hung.
    ///
    /// `voice` is the `voice:<uuid>` wire id and nothing more — the label, the
    /// transcript and the recording stay behind the `voices` scope, which is
    /// granted separately from the `daemon_status` scope that carries this.
    PreparingVoice { voice: String, model: String },

    /// The model is ready to speak in that voice, or could not be made ready.
    ///
    /// Always follows a [`Self::PreparingVoice`] for the same id. `error` is
    /// present only on failure, mirroring `download_progress`: absent means it
    /// worked.
    VoicePrepared {
        voice: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::DaemonStatusEvent;

    /// The preparation pair is what a client watches to explain a pause before
    /// speech, so its discriminants and its one optional field have to be
    /// exactly what the docs promise.
    #[test]
    fn voice_preparation_wire_shape() {
        let started = serde_json::to_value(DaemonStatusEvent::PreparingVoice {
            voice: "voice:2f8a2d0e".into(),
            model: "qwen3-tts-0.6b-base".into(),
        })
        .expect("serializes");
        assert_eq!(started["status"], "preparing_voice");
        assert_eq!(started["voice"], "voice:2f8a2d0e");
        assert_eq!(started["model"], "qwen3-tts-0.6b-base");

        // Success omits `error` rather than sending a null: a client testing
        // for its presence is testing for failure.
        let ok = serde_json::to_value(DaemonStatusEvent::VoicePrepared {
            voice: "voice:2f8a2d0e".into(),
            model: "qwen3-tts-0.6b-base".into(),
            error: None,
        })
        .expect("serializes");
        assert_eq!(ok["status"], "voice_prepared");
        assert!(ok.get("error").is_none(), "{ok}");

        let failed = serde_json::to_value(DaemonStatusEvent::VoicePrepared {
            voice: "voice:2f8a2d0e".into(),
            model: "qwen3-tts-0.6b-base".into(),
            error: Some("the encoder was not loaded".into()),
        })
        .expect("serializes");
        assert_eq!(failed["error"], "the encoder was not loaded");
    }

    /// The wire discriminant is `status`, and variant/field names match the
    /// hand-built JSON the daemon used to emit.
    #[test]
    fn model_switched_wire_shape() {
        let json = serde_json::to_value(DaemonStatusEvent::ModelSwitched {
            model_name: "kokoro-tiny".into(),
            source: "github.com/super-tts/kokoro".into(),
            actual_device: "cpu".into(),
        })
        .unwrap();
        assert_eq!(json["status"], "model_switched");
        assert_eq!(json["model_name"], "kokoro-tiny");
        assert_eq!(json["actual_device"], "cpu");
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
            "model": "kokoro-tiny",
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
        })
        .unwrap();
        assert_eq!(json["status"], "ready");
        assert_eq!(json["model_loaded"], false);
        assert!(json.get("actual_device").is_none());
    }
}
