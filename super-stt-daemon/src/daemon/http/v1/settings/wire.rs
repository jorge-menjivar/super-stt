// SPDX-License-Identifier: GPL-3.0-only
//! What each `settings` endpoint answers with.
//!
//! One type per shape, built from the [`DaemonResponse`] the command bus
//! returned. The point is that the type an endpoint *publishes* is the type its
//! handler *builds*: [`FromDaemon`] is the only way a narrow body comes into
//! existence here, so a schema cannot claim a field the handler never fills.
//!
//! Field sets are not guesses — each is the set of `with_*` calls its command
//! handler makes. A response carrying its value in `message` rather than in a
//! field of its own is marked as such, because a client has to parse it back
//! out.

use super::super::super::wire::Ack;
use serde::Serialize;
use super_stt_shared::models::backends::BackendInfo;
use super_stt_shared::models::protocol::{DaemonResponse, GpuHostInfo, GpuInfo, StageReport};
use super_stt_shared::models::theme::AudioTheme;
use utoipa::ToSchema;

/// Build a narrow response body from the command bus's wide one.
///
/// Implemented rather than derived so each type states which fields it takes
/// and what it does when one is missing — a command that stops setting a field
/// should surface as a documented default, not a panic.
pub(crate) trait FromDaemon {
    fn from_daemon(resp: DaemonResponse) -> Self;
}

impl FromDaemon for Ack {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            message: resp.message,
        }
    }
}

/// The selected audio cue theme.
#[derive(Serialize, ToSchema)]
pub(crate) struct AudioThemeState {
    #[schema(example = "success")]
    status: &'static str,
    /// The selected theme's token.
    #[schema(example = "classic")]
    audio_theme: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for AudioThemeState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            audio_theme: resp.audio_theme.unwrap_or_default(),
            message: resp.message,
        }
    }
}

/// Every audio cue theme the daemon ships.
#[derive(Serialize, ToSchema)]
pub(crate) struct AudioThemeList {
    #[schema(example = "success")]
    status: &'static str,
    available_audio_themes: Vec<AudioTheme>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for AudioThemeList {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            available_audio_themes: resp.available_audio_themes.unwrap_or_default(),
            message: resp.message,
        }
    }
}

/// What ends a recording.
#[derive(Serialize, ToSchema)]
pub(crate) struct RecordingStopModeState {
    #[schema(example = "success")]
    status: &'static str,
    recording_stop_mode: String,
}

impl FromDaemon for RecordingStopModeState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            recording_stop_mode: resp.recording_stop_mode.unwrap_or_default(),
        }
    }
}

/// How transcripts reach the focused window.
#[derive(Serialize, ToSchema)]
pub(crate) struct WriteMethodState {
    #[schema(example = "success")]
    status: &'static str,
    write_method: String,
}

impl FromDaemon for WriteMethodState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            write_method: resp.write_method.unwrap_or_default(),
        }
    }
}

/// The outcome of writing sample text with the configured method.
#[derive(Serialize, ToSchema)]
pub(crate) struct WriteMethodTest {
    #[schema(example = "success")]
    status: &'static str,
    /// The configured preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    write_method: Option<String>,
    /// What that preference resolved to for this session, which can differ when
    /// the compositor will not permit the preferred mechanism.
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_write_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for WriteMethodTest {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            write_method: resp.write_method,
            resolved_write_method: resp.resolved_write_method,
            message: resp.message,
        }
    }
}

/// How failures are announced.
#[derive(Serialize, ToSchema)]
pub(crate) struct NotificationMethodState {
    #[schema(example = "success")]
    status: &'static str,
    notification_method: String,
}

impl FromDaemon for NotificationMethodState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            notification_method: resp.notification_method.unwrap_or_default(),
        }
    }
}

/// Whether live preview typing is on.
#[derive(Serialize, ToSchema)]
pub(crate) struct PreviewTypingState {
    #[schema(example = "success")]
    status: &'static str,
    preview_typing_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for PreviewTypingState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            preview_typing_enabled: resp.preview_typing_enabled.unwrap_or(false),
            message: resp.message,
        }
    }
}

/// The models directory override.
#[derive(Serialize, ToSchema)]
pub(crate) struct CustomModelsDirState {
    #[schema(example = "success")]
    status: &'static str,
    /// The configured directory, or `null` when no override is set. Always
    /// present — `null` is the answer, not an absent key.
    custom_models_dir: Option<String>,
}

impl FromDaemon for CustomModelsDirState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            // Doubly optional on the bus: the outer layer is "this command did
            // not set it", the inner is the documented nullable value.
            custom_models_dir: resp.custom_models_dir.flatten(),
        }
    }
}

/// Whether the periodic update check runs.
#[derive(Serialize, ToSchema)]
pub(crate) struct UpdateCheckEnabledState {
    #[schema(example = "success")]
    status: &'static str,
    update_check_enabled: bool,
}

impl FromDaemon for UpdateCheckEnabledState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            update_check_enabled: resp.update_check_enabled.unwrap_or(false),
        }
    }
}

/// Which release channel updates come from.
#[derive(Serialize, ToSchema)]
pub(crate) struct UpdateBetaOptinState {
    #[schema(example = "success")]
    status: &'static str,
    update_beta_optin: String,
}

impl FromDaemon for UpdateBetaOptinState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            update_beta_optin: resp.update_beta_optin.unwrap_or_default(),
        }
    }
}

/// The default transcription language.
#[derive(Serialize, ToSchema)]
pub(crate) struct LanguageState {
    #[schema(example = "success")]
    status: &'static str,
    /// A BCP-47 tag, `auto`, or `null` when nothing is configured. Always
    /// present — `null` is the answer, not an absent key.
    #[schema(example = "es")]
    language: Option<String>,
}

impl FromDaemon for LanguageState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            language: resp
                .language
                .as_ref()
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        }
    }
}

/// The models the backend filling stage 1 can transcribe with.
#[derive(Serialize, ToSchema)]
pub(crate) struct ModelList {
    #[schema(example = "success")]
    status: &'static str,
    /// `[name, source]` pairs. Post-processor models are excluded — they are not
    /// switchable transcription models, and offering one would fail every
    /// recording. The full catalog, roles included, is at `GET /backends`.
    #[schema(example = json!([["whisper-tiny", "github.com/super-stt/whisper"]]))]
    available_models: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for ModelList {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            available_models: resp.available_models.unwrap_or_default(),
            message: resp.message,
        }
    }
}

/// Every installed backend, with its models, options and secrets.
#[derive(Serialize, ToSchema)]
pub(crate) struct BackendCatalog {
    #[schema(example = "success")]
    status: &'static str,
    backends: Vec<BackendInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for BackendCatalog {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            // The command builds a typed catalog and flattens it to `Value` at
            // the last step; this reads it straight back, so the published
            // schema is `BackendInfo` rather than "some JSON".
            backends: resp
                .backends
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            message: resp.message,
        }
    }
}

/// The host's GPUs and its GPU toolchain versions.
#[derive(Serialize, ToSchema)]
pub(crate) struct GpuInventory {
    #[schema(example = "success")]
    status: &'static str,
    /// One entry per detected GPU; empty on a host with none.
    gpu_info: Vec<GpuInfo>,
    /// Driver and runtime versions, independent of any one GPU.
    host: GpuHostInfo,
}

impl FromDaemon for GpuInventory {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            gpu_info: resp.gpu_info.unwrap_or_default(),
            host: resp.host.unwrap_or_default(),
        }
    }
}

/// The whole pipeline, in order.
#[derive(Serialize, ToSchema)]
pub(crate) struct PipelineReport {
    #[schema(example = "success")]
    status: &'static str,
    /// Stage 1 first. A transcript passes through these in order.
    pipeline: Vec<StageReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for PipelineReport {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            pipeline: resp.pipeline.unwrap_or_default(),
            message: resp.message,
        }
    }
}

/// One stage of the pipeline.
#[derive(Serialize, ToSchema)]
pub(crate) struct StageEnvelope {
    #[schema(example = "success")]
    pub(crate) status: &'static str,
    pub(crate) stage: StageReport,
}

/// The devices a model or a stage can run on.
#[derive(Serialize, ToSchema)]
pub(crate) struct DeviceList {
    #[schema(example = "success")]
    status: &'static str,
    /// Accelerator tokens, e.g. `cpu`, `cuda`, `vulkan`.
    #[schema(example = json!(["cpu", "cuda"]))]
    available_devices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for DeviceList {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            available_devices: resp.available_devices.unwrap_or_default(),
            message: resp.message,
        }
    }
}

/// A model's device preference, what it resolved to, and what this host can
/// offer it.
#[derive(Serialize, ToSchema)]
pub(crate) struct ModelDevice {
    #[schema(example = "success")]
    status: &'static str,
    /// The preference itself: `cpu`, `gpu`, or a specific accelerator. `none`
    /// for a model that runs remotely and therefore has no local device.
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<String>,
    /// What a `gpu` preference resolved to once a model loaded — `cuda`,
    /// `rocm`, `metal`, `vulkan`. `null` while the preference is `gpu` but
    /// nothing has loaded yet; equal to the preference when it is `cpu`.
    ///
    /// Doubly optional because the wire distinguishes three states and a client
    /// reads them differently: the key absent means this response does not speak
    /// to the device at all, an explicit `null` means the preference is `gpu`
    /// and nothing has resolved it yet, and a value is the accelerator in use.
    /// Collapsing the first two would report "unresolved" where the daemon said
    /// nothing.
    #[allow(clippy::option_option)]
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_accel: Option<Option<String>>,
    /// What this host can actually offer this model — the intersection of the
    /// machine's accelerators and the builds the model ships. Empty for a model
    /// that runs remotely.
    #[schema(example = json!(["cpu", "cuda"]))]
    available_devices: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for ModelDevice {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            device: resp.device,
            resolved_accel: resp.resolved_accel,
            available_devices: resp.available_devices.unwrap_or_default(),
            message: resp.message,
        }
    }
}

/// The backend filling stage 1, as that stage's mutations report it.
#[derive(Serialize, serde::Deserialize, ToSchema)]
pub(crate) struct ActiveBackend {
    /// The backend's repo id.
    pub(crate) source: String,
    /// Its display name.
    pub(crate) name: String,
    /// Whether one of its models is currently up.
    pub(crate) model_loaded: bool,
}

/// Stage 2's state, as that stage's mutations report it.
#[derive(Serialize, serde::Deserialize, ToSchema)]
pub(crate) struct PostProcessorState {
    /// The user's on/off choice, which is separate from whether the model came
    /// up: a stage can be enabled with a failed load, and transcripts then pass
    /// through untouched.
    pub(crate) enabled: bool,
    /// The selected model, or `null` when none is picked.
    pub(crate) model: Option<String>,
    /// The selected backend, or `null` when the stage is empty.
    pub(crate) source: Option<String>,
    /// Whether that model is loaded and ready.
    pub(crate) loaded: bool,
}

/// The answer to a stage mutation.
///
/// The two stages answer with different keys — stage 1 with `active_backend`,
/// stage 2 with `post_processor` — because each grew its own endpoint before
/// the pipeline addressed stages by position. Both are documented rather than
/// reconciled: changing either is a breaking wire change. Read the stage back
/// with `GET /pipeline/{stage}` for the one shape both share.
#[derive(Serialize, ToSchema)]
pub(crate) struct StageMutation {
    #[schema(example = "success")]
    status: &'static str,
    /// Stage 1 only. `null` when the stage was emptied.
    ///
    /// Doubly optional for the same reason `resolved_accel` is: absent means
    /// this was a stage 2 mutation, which reports `post_processor` instead;
    /// `null` means stage 1 itself is now empty.
    #[allow(clippy::option_option)]
    #[serde(skip_serializing_if = "Option::is_none")]
    active_backend: Option<Option<ActiveBackend>>,
    /// Stage 2 only.
    #[serde(skip_serializing_if = "Option::is_none")]
    post_processor: Option<PostProcessorState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for StageMutation {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            active_backend: resp
                .active_backend
                .map(|v| serde_json::from_value(v).unwrap_or(None)),
            post_processor: resp
                .post_processor
                .and_then(|v| serde_json::from_value(v).ok()),
            message: resp.message,
        }
    }
}

/// How one model's transcription language resolves.
///
/// The per-model endpoints answer with this under `language`, where the global
/// `/language` endpoints answer with a bare tag. Same field name, different
/// shapes: the per-model answer has to explain *why* a language is in effect,
/// since three settings can decide it.
#[derive(Serialize, serde::Deserialize, ToSchema)]
pub(crate) struct ModelLanguageBlock {
    /// Whether this model can transcribe more than one language at all. A
    /// monolingual model ignores every setting below.
    multilingual: bool,
    /// Which setting `effective` came from: the per-model override, the global
    /// setting, or the model's own default.
    source: String,
    /// The tag actually used, after resolution. `null` when the model detects
    /// the language itself.
    effective: Option<String>,
    /// The per-model override, or `null` when none is set.
    #[serde(rename = "override")]
    model_override: Option<String>,
    /// The model's own default language.
    primary: String,
    /// Every tag this model accepts.
    supported: Vec<String>,
}

/// The per-model language resolution.
#[derive(Serialize, ToSchema)]
pub(crate) struct ModelLanguageState {
    #[schema(example = "success")]
    status: &'static str,
    language: ModelLanguageBlock,
}

impl FromDaemon for ModelLanguageState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            language: resp
                .language
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or(ModelLanguageBlock {
                    multilingual: false,
                    source: "default".to_string(),
                    effective: None,
                    model_override: None,
                    primary: String::new(),
                    supported: Vec::new(),
                }),
        }
    }
}
