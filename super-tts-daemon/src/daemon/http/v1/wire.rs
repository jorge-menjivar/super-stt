// SPDX-License-Identifier: GPL-3.0-only
//! What each `/v1` endpoint answers with.
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

use crate::daemon::http::wire::Ack;
use serde::{Deserialize, Serialize};
use super_tts_shared::models::backends::BackendInfo;
use super_tts_shared::models::protocol::{
    DaemonResponse, GpuHostInfo, GpuInfo, StageModelReport, StageReport,
};
use super_tts_shared::models::theme::AudioTheme;
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

/// Whether models that synthesize over the network may be used at all.
#[derive(Serialize, ToSchema)]
pub(crate) struct AllowOnlineModelsState {
    #[schema(example = "success")]
    status: &'static str,
    /// `false` keeps every utterance on this machine: a model whose backend
    /// would send text to a third party cannot be loaded while it is off.
    allow_online_models: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for AllowOnlineModelsState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            allow_online_models: resp.allow_online_models.unwrap_or(false),
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

/// The default synthesis language.
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

/// The models a pipeline stage can run: its backend's.
#[derive(Serialize, ToSchema)]
pub(crate) struct ModelList {
    #[schema(example = "success")]
    status: &'static str,
    /// `[name, source]` pairs. The full catalog — voices, devices, languages
    /// and all — is at `GET /backend/list`; this is the flat picker.
    #[schema(example = json!([["kokoro-82m", "github.com/super-tts/kokoro"]]))]
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
    /// Stage 1 first. An utterance passes through these in order.
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

/// One stage's model slot.
#[derive(Serialize, ToSchema)]
pub(crate) struct StageModelEnvelope {
    #[schema(example = "success")]
    status: &'static str,
    model: StageModelReport,
}

impl FromDaemon for StageModelEnvelope {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            // A stage that has never had a model selected still has a slot, and
            // a client reads `model`/`loaded` off it unconditionally — so an
            // empty slot is reported as an empty slot rather than as an absent
            // key it would have to guard against.
            model: resp.stage_model.unwrap_or(StageModelReport {
                stage: super_tts_shared::models::protocol::SYNTHESIS_STAGE,
                model: None,
                loaded: false,
                device: None,
                switch: None,
            }),
        }
    }
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

/// The backend filling a stage, as that stage's mutations report it.
#[derive(Serialize, serde::Deserialize, ToSchema)]
pub(crate) struct ActiveBackend {
    /// The backend's repo id.
    pub(crate) source: String,
    /// Its display name.
    pub(crate) name: String,
    /// Whether one of its models is currently up.
    pub(crate) model_loaded: bool,
}

/// The answer to a stage mutation.
///
/// Reports the backend the stage now holds, or `null` when the stage was
/// emptied. Read the stage back with `GET /pipeline/{stage}` for the fuller
/// shape; this is the acknowledgement, and it names what the mutation left
/// behind so a client need not re-read to render the result of its own click.
#[derive(Serialize, ToSchema)]
pub(crate) struct StageMutation {
    #[schema(example = "success")]
    status: &'static str,
    /// The backend now filling the stage; `null` when the stage was emptied.
    ///
    /// Doubly optional for the same reason `resolved_accel` is: the key absent
    /// means the response does not speak to the stage's backend at all, where
    /// an explicit `null` means the stage is now empty. A client rendering the
    /// result of a Deselect needs to tell those apart.
    #[allow(clippy::option_option)]
    #[serde(skip_serializing_if = "Option::is_none")]
    active_backend: Option<Option<ActiveBackend>>,
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
            message: resp.message,
        }
    }
}

/// How one model's synthesis language resolves.
///
/// The per-model endpoint answers with this under `language`, where the global
/// `/settings/language` endpoints answer with a bare tag. Same field name,
/// different shapes: the per-model answer has to explain *why* a language is in
/// effect, since three settings can decide it.
#[derive(Serialize, serde::Deserialize, ToSchema)]
pub(crate) struct ModelLanguageBlock {
    /// Whether this model can speak more than one language at all. A
    /// monolingual model ignores every setting below.
    multilingual: bool,
    /// Which setting `effective` came from: the per-model override, the global
    /// setting, or the model's own default.
    source: String,
    /// The tag actually used, after resolution. `null` when the model picks the
    /// language itself.
    effective: Option<String>,
    /// The per-model override, or `null` when none is set.
    #[serde(rename = "override")]
    model_override: Option<String>,
    /// The model's own default language.
    primary: String,
}

/// How a model's voice resolves: the stored preference, or the manifest's
/// `default_voice` when none is stored.
#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct ModelVoiceBlock {
    /// The voice an utterance naming none will actually be spoken in. `null`
    /// when the model has neither a stored voice nor a `default_voice`, which
    /// is the state a cloning model starts in — and the state in which speaking
    /// is refused until a voice is set.
    effective: Option<String>,
    /// Which setting `effective` came from: `override` or `default`.
    source: String,
    /// The stored per-model voice, or `null` when none is set.
    #[serde(rename = "override")]
    model_override: Option<String>,
    /// The manifest's `default_voice`, or `null` when it declares none.
    default: Option<String>,
    /// The `voice` id shapes this model accepts — `preset`, `cloned`,
    /// `described`. A client offers a text field for `described`, since a
    /// described voice is free text with no set to enumerate.
    #[schema(example = json!(["preset"]))]
    kinds: Vec<String>,
}

/// One model's voice resolution.
#[derive(Serialize, ToSchema)]
pub(crate) struct ModelVoiceState {
    #[schema(example = "success")]
    status: &'static str,
    voice: ModelVoiceBlock,
}

impl FromDaemon for ModelVoiceState {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            voice: resp
                .voice
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or(ModelVoiceBlock {
                    effective: None,
                    source: "default".to_string(),
                    model_override: None,
                    default: None,
                    kinds: Vec::new(),
                }),
        }
    }
}

/// One voice a model can be pinned to.
#[derive(Serialize, Deserialize, ToSchema)]
pub(crate) struct VoiceChoice {
    /// The `voice` id to send, e.g. `ryan` or `voice:<uuid>`.
    id: String,
    /// Display name for a picker.
    label: String,
    /// `preset` for one the model declares, `cloned` for one from the voice
    /// library.
    kind: String,
}

/// The voices a model can be pinned to.
#[derive(Serialize, ToSchema)]
pub(crate) struct VoiceList {
    #[schema(example = "success")]
    status: &'static str,
    /// The model's presets, then the stored cloned voices when it clones.
    /// Empty for a model whose voices are all described — free text has no
    /// list, and a client shows a text field for it instead.
    available_voices: Vec<VoiceChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for VoiceList {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            available_voices: resp
                .available_voices
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect(),
            message: resp.message,
        }
    }
}

/// The languages a model, or the global setting, can be pinned to.
#[derive(Serialize, ToSchema)]
pub(crate) struct LanguageList {
    #[schema(example = "success")]
    status: &'static str,
    /// BCP-47 tags, plus the reserved `auto`. Empty for a monolingual model,
    /// which has nothing to choose.
    #[schema(example = json!(["auto", "en", "es"]))]
    available_languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl FromDaemon for LanguageList {
    fn from_daemon(resp: DaemonResponse) -> Self {
        Self {
            status: "success",
            available_languages: resp.available_languages.unwrap_or_default(),
            message: resp.message,
        }
    }
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
                }),
        }
    }
}
