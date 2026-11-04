// SPDX-License-Identifier: GPL-3.0-only
use crate::models::theme::AudioTheme;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DaemonResponse {
    pub status: String,
    /// Stable, machine-readable error identifier — the field clients switch on.
    /// Present on classified errors; drives the HTTP status. See
    /// [`ErrorCode`](super::ErrorCode) and `docs/protocol/transport.md`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<super::ErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Id of the utterance a `speak` request started, so a client can cancel it
    /// or correlate playback events with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utterance_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_loaded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_models: Option<Vec<(String, String)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_devices: Option<Vec<String>>,
    /// The accelerator `"gpu"` resolved to, on `GET`/`POST /active_device`.
    /// `"cpu"` when the preference itself is `"cpu"` (nothing to resolve);
    /// `"cuda"`/`"rocm"`/`"metal"`/`"vulkan"` once a local model has loaded
    /// onto a `"gpu"` preference; JSON `null` when the preference is `"gpu"`
    /// but nothing has loaded yet. See `docs/protocol/endpoints/v1/active_device.md`.
    ///
    /// Doubly `Option`, the same way `custom_models_dir` below is: the outer
    /// layer is "irrelevant to this response" (omitted, as for every other
    /// field here); the inner layer is the documented nullable value itself,
    /// so an unresolved `"gpu"` preference serializes as an explicit `null`
    /// rather than an absent key on `/active_device`, while every other
    /// command's response omits the key entirely.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_accel: Option<Option<String>>,

    // Notification system fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribed_to: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_subscribers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events: Option<Vec<NotificationEvent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscriber_info: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_info: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,

    // Audio theme fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_audio_themes: Option<Vec<AudioTheme>>,

    // Download progress fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_progress: Option<DownloadProgress>,

    // Daemon configuration fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_config: Option<Value>,

    // Installed-backends catalog (GET /backends): array of backend objects
    // with their models, secrets, and options. See docs/protocol/endpoints/v1/backends.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backends: Option<Value>,

    // Per-backend secret list (GET /backends/{source}/secrets/list): array of
    // `{name, label, required, configured}` objects.
    // See docs/protocol/endpoints/v1/backends.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets: Option<Value>,

    // Active backend (GET /active_backend): `{ source, name, model_loaded }`,
    // or absent when idle. See docs/protocol/endpoints/v1/active_backend.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_backend: Option<Value>,

    // GPU inventory (GET /gpu_info): array of `GpuInfo` objects.
    // See docs/protocol/endpoints/v1/gpu_info.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_info: Option<Vec<GpuInfo>>,

    // Host-wide GPU toolchain/driver versions (GET /gpu_info), independent of
    // any one GPU. See docs/protocol/endpoints/v1/gpu_info.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<GpuHostInfo>,

    // Connection status fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_active: Option<bool>,

    /// Whether the daemon is currently speaking — synthesizing or playing out
    /// an utterance. Surfaced on `GET /v1/status` so a client can decide
    /// whether `POST /v1/speak` will interrupt something. Absent on responses
    /// where it's not meaningful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy: Option<bool>,

    // Notification method
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_method: Option<String>,

    // Self-update settings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_check_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_beta_optin: Option<String>,

    // Online models
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_online_models: Option<bool>,

    // Custom models directory (None = no override, daemon uses default cache)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_models_dir: Option<Option<String>>,

    // Synthesis language: for GET /settings/language a string|null; for
    // GET /pipeline/{stage}/model/{model}/language the resolution block. See
    // docs/protocol/endpoints/v1/settings/language.md and
    // docs/protocol/endpoints/v1/pipeline/language.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<Value>,

    /// The BCP-47 tags a language setting will accept, plus the reserved
    /// `auto`. Answers both `/settings/language/list` (what the global setting
    /// takes) and `/pipeline/{stage}/model/{model}/language/list` (what one
    /// model takes, which may be narrower).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_languages: Option<Vec<String>>,

    /// The voice resolution block for
    /// `GET /pipeline/{stage}/model/{model}/voice`. See
    /// docs/protocol/endpoints/v1/pipeline/voice.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<Value>,

    /// The voices one model can be pinned to, for
    /// `/pipeline/{stage}/model/{model}/voice/list` — its manifest presets and,
    /// when it clones, the stored cloned voices as `voice:<uuid>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_voices: Option<Vec<Value>>,

    /// Every pipeline stage, in order — the body of `GET /pipeline`.
    ///
    /// A typed report rather than a `Value`: the stage shape is published in
    /// the `OpenAPI` document, and a schema cannot be generated from arbitrary
    /// JSON. See [`StageReport`](super::StageReport).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pipeline: Option<Vec<super::StageReport>>,

    /// One stage's model slot — the body of `GET /pipeline/{stage}/model`.
    ///
    /// Separate from `pipeline` because it has a different lifetime: a stage is
    /// a durable selection and its model has a runtime. Reporting the model
    /// inside the stage is what once let a client read "backend chosen" as
    /// "model running".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_model: Option<super::StageModelReport>,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DownloadProgress {
    pub model_name: String,
    pub current_file: String,
    pub file_index: usize,
    pub total_files: usize,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub percentage: f32,
    /// Phase of the provisioning run: `"verifying"` (checking the files
    /// already on disk — every load starts here, and a fully cached one never
    /// leaves it), `"downloading"` (bytes coming off the network),
    /// `"loading_model"` (files present, weights loading), then one of the
    /// terminal `"completed"` / `"cancelled"` / `"error"`.
    pub status: String,
    pub started_at: String,
    pub eta_seconds: Option<u64>,
    /// Failure detail, present only when `status == "error"`. Lets a client
    /// surface why a model switch failed without a second request. Omitted
    /// from the wire on every non-error tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The backend's own account of the load while `status` is
    /// `"loading_model"`: its phase, its step, and how far through the step it
    /// is. Absent until the backend reports any of it, and from backends that
    /// never do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<super::LoadProgress>,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NotificationEvent {
    #[serde(rename = "type")]
    pub event_type_field: String,
    pub event_type: String,
    pub client_id: String,
    pub timestamp: String,
    pub data: Value,
}

impl DaemonResponse {
    #[must_use]
    pub fn success() -> Self {
        Self {
            status: "success".to_string(),
            ..Default::default()
        }
    }

    /// Build an error response, sanitizing `message` before it crosses the Unix
    /// socket boundary. Full details stay in the daemon logs; clients receive
    /// only a trimmed first-line prefix so internal paths/secrets aren't leaked
    /// (opt out for local debugging via `SUPER_TTS_DEBUG_ERRORS=1`).
    #[must_use]
    pub fn error(message: &str) -> Self {
        fn sanitize_error_message(message: &str) -> String {
            // Opt-in detailed errors for local debugging
            let debug = std::env::var("SUPER_TTS_DEBUG_ERRORS").is_ok_and(|v| v == "1")
                || cfg!(debug_assertions);
            if debug {
                return message.to_string();
            }

            // Keep only the first line and trim internal details after a colon
            let first_line = message.lines().next().unwrap_or(message).trim();
            if let Some((prefix, _)) = first_line.split_once(':') {
                prefix.trim().to_string()
            } else {
                first_line.to_string()
            }
        }
        Self {
            status: "error".to_string(),
            message: Some(sanitize_error_message(message)),
            ..Default::default()
        }
    }

    /// Build a classified error response: a machine-readable [`ErrorCode`] plus
    /// a sanitized human-readable `message`. The HTTP layer derives the status
    /// from the code (see [`ErrorCode::http_status`](super::ErrorCode)).
    #[must_use]
    pub fn error_with_code(code: super::ErrorCode, message: &str) -> Self {
        Self {
            error_code: Some(code),
            ..Self::error(message)
        }
    }

    /// Attach (or override) the machine-readable error code.
    #[must_use]
    pub fn with_error_code(mut self, code: super::ErrorCode) -> Self {
        self.error_code = Some(code);
        self
    }

    #[must_use]
    pub fn with_utterance_id(mut self, id: String) -> Self {
        self.utterance_id = Some(id);
        self
    }

    #[must_use]
    pub fn with_device(mut self, device: String) -> Self {
        self.device = Some(device);
        self
    }

    #[must_use]
    pub fn with_model_loaded(mut self, loaded: bool) -> Self {
        self.model_loaded = Some(loaded);
        self
    }

    #[must_use]
    pub fn with_current_model(mut self, model: impl Into<String>) -> Self {
        self.current_model = Some(model.into());
        self
    }

    #[must_use]
    pub fn with_current_source(mut self, source: String) -> Self {
        self.current_source = Some(source);
        self
    }

    #[must_use]
    pub fn with_message(mut self, message: String) -> Self {
        self.message = Some(message);
        self
    }

    #[must_use]
    pub fn with_client_id(mut self, client_id: String) -> Self {
        self.client_id = Some(client_id);
        self
    }

    #[must_use]
    pub fn with_subscribed_to(mut self, events: Vec<String>) -> Self {
        self.subscribed_to = Some(events);
        self
    }

    #[must_use]
    pub fn with_total_subscribers(mut self, count: u32) -> Self {
        self.total_subscribers = Some(count);
        self
    }

    #[must_use]
    pub fn with_events(mut self, events: Vec<NotificationEvent>) -> Self {
        self.events = Some(events);
        self
    }

    #[must_use]
    pub fn with_notification_info(mut self, info: Value) -> Self {
        self.notification_info = Some(info);
        self
    }

    #[must_use]
    pub fn with_audio_theme(mut self, theme: String) -> Self {
        self.audio_theme = Some(theme);
        self
    }

    #[must_use]
    pub fn with_available_audio_themes(mut self, themes: Vec<AudioTheme>) -> Self {
        self.available_audio_themes = Some(themes);
        self
    }

    #[must_use]
    pub fn with_available_models(mut self, models: Vec<(String, String)>) -> Self {
        self.available_models = Some(models);
        self
    }

    #[must_use]
    pub fn with_download_progress(mut self, progress: DownloadProgress) -> Self {
        self.download_progress = Some(progress);
        self
    }

    #[must_use]
    pub fn with_available_devices(mut self, devices: Vec<String>) -> Self {
        self.available_devices = Some(devices);
        self
    }

    /// Set `resolved_accel` to the documented value, which may itself be
    /// `None` (unresolved). Wrapping in `Some` here marks the field present
    /// on this response, so an unresolved `"gpu"` preference serializes as
    /// `null` rather than being dropped like an irrelevant field.
    #[must_use]
    pub fn with_resolved_accel(mut self, accel: Option<String>) -> Self {
        self.resolved_accel = Some(accel);
        self
    }

    #[must_use]
    pub fn with_daemon_config(mut self, config: Value) -> Self {
        self.daemon_config = Some(config);
        self
    }

    #[must_use]
    pub fn with_backends(mut self, backends: Value) -> Self {
        self.backends = Some(backends);
        self
    }

    #[must_use]
    pub fn with_active_backend(mut self, active_backend: Value) -> Self {
        self.active_backend = Some(active_backend);
        self
    }

    #[must_use]
    pub fn with_language(mut self, language: Value) -> Self {
        self.language = Some(language);
        self
    }

    #[must_use]
    pub fn with_voice(mut self, voice: Value) -> Self {
        self.voice = Some(voice);
        self
    }

    #[must_use]
    pub fn with_available_voices(mut self, voices: Vec<Value>) -> Self {
        self.available_voices = Some(voices);
        self
    }

    #[must_use]
    pub fn with_gpu_info(mut self, gpu_info: Vec<GpuInfo>) -> Self {
        self.gpu_info = Some(gpu_info);
        self
    }

    #[must_use]
    pub fn with_gpu_host_info(mut self, host: GpuHostInfo) -> Self {
        self.host = Some(host);
        self
    }

    #[must_use]
    pub fn with_connection_active(mut self, active: bool) -> Self {
        self.connection_active = Some(active);
        self
    }

    #[must_use]
    pub fn with_busy(mut self, busy: bool) -> Self {
        self.busy = Some(busy);
        self
    }

    #[must_use]
    pub fn with_notification_method(mut self, method: String) -> Self {
        self.notification_method = Some(method);
        self
    }

    #[must_use]
    pub fn with_update_check_enabled(mut self, enabled: bool) -> Self {
        self.update_check_enabled = Some(enabled);
        self
    }

    #[must_use]
    pub fn with_update_beta_optin(mut self, value: String) -> Self {
        self.update_beta_optin = Some(value);
        self
    }

    #[must_use]
    pub fn with_allow_online_models(mut self, allowed: bool) -> Self {
        self.allow_online_models = Some(allowed);
        self
    }

    #[must_use]
    pub fn with_custom_models_dir(mut self, dir: Option<String>) -> Self {
        self.custom_models_dir = Some(dir);
        self
    }
}

/// What `GET /gpu_info` reports. See `super_engine_protocol::models::gpu`.
pub use super_engine_protocol::models::gpu::{
    CudaHostInfo, GpuHostInfo, GpuInfo, RocmHostInfo, VulkanHostInfo,
};

#[cfg(test)]
mod tests {
    use super::DaemonResponse;

    /// `utterance_id` stays off the wire until a request actually starts an
    /// utterance. Every other response would otherwise gain the field, and a
    /// client that treats its presence as "speech began" — which is the only
    /// reason it exists — would misread every settings reply.
    #[test]
    fn utterance_id_is_omitted_until_set() {
        let json = serde_json::to_string(&DaemonResponse::success()).expect("serialize");
        assert!(
            !json.contains("utterance_id"),
            "absent field must not be serialized: {json}"
        );

        let json =
            serde_json::to_string(&DaemonResponse::success().with_utterance_id("u-1".to_string()))
                .expect("serialize");
        assert!(
            json.contains(r#""utterance_id":"u-1""#),
            "a started utterance must reach the wire verbatim: {json}"
        );
    }
}
