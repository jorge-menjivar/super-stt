// SPDX-License-Identifier: GPL-3.0-only
use crate::models::recording_stop_mode::RecordingStopMode;
use crate::models::write_method::WriteMethod;

#[derive(Debug)]
pub enum Command {
    Transcribe {
        audio_data: Vec<f32>,
        sample_rate: u32,
        client_id: String,
        /// Optional per-request language override (BCP-47 or `"auto"`). `None`
        /// falls back to the active model's configured language.
        language: Option<String>,
    },
    Ping {
        client_id: Option<String>,
    },
    Status,
    Record {
        write_mode: bool,
        stop_mode: Option<RecordingStopMode>,
        wait: bool,
        preview: Option<bool>,
        /// Optional per-request language override (BCP-47 or `"auto"`). `None`
        /// falls back to the active model's configured language.
        language: Option<String>,
    },
    SetAudioTheme {
        theme: String,
    },
    GetAudioTheme,
    TestAudioTheme,
    SetModel {
        model: String,
        /// Backend repo id that serves the model (e.g.
        /// `github.com/super-stt/openai`). Empty selects the first backend
        /// serving `model`.
        source: String,
    },
    GetModel,
    ListModels,
    SetDevice {
        device: String, // "cpu" or "cuda"
    },
    GetDevice,
    GetConfig,
    CancelDownload,
    GetDownloadStatus,
    ListAudioThemes,
    SetPreviewTyping {
        enabled: bool,
    },
    GetPreviewTyping,
    /// Run final transcripts through `model`. An empty `source` resolves to
    /// the selected post-processor backend, the way `SetModel` resolves against
    /// the active backend. The model must be one its backend declares with
    /// `role = "post_processor"`.
    ///
    /// `device` is the stage's own `cpu`/`gpu` preference: the post-processor
    /// runs beside the transcription model, on hardware chosen for it, not
    /// inherited from stage 1. `None` keeps whatever the stage already has.
    SetPostProcessor {
        model: String,
        source: String,
        device: Option<String>,
    },
    GetPostProcessor,
    /// Stop running the post-processor, keeping the selection.
    ClearPostProcessor,
    /// Select the backend that provides the post-processor. The post-processing
    /// twin of `SetActiveBackend`: it records *which* backend and validates
    /// that it serves one, without loading anything.
    SetPostProcessorBackend {
        source: String,
    },
    /// Deselect the post-processor backend, forgetting the model with it.
    ClearPostProcessorBackend,
    /// Report the whole transcription pipeline: every stage in order, with the
    /// backend and model filling it. Stage 1 transcribes; later stages
    /// post-process what it produced.
    GetPipeline,
    SetRecordingStopMode {
        mode: RecordingStopMode,
    },
    GetRecordingStopMode,
    SetWriteMethod {
        method: WriteMethod,
    },
    GetWriteMethod,
    /// Type a fixed string with the configured write method so a settings UI
    /// can show whether keyboard simulation reaches the focused window.
    /// Contract: `docs/protocol/endpoints/v1/write_method/test.md`.
    TestWriteMethod,
    /// The raw wire string, unparsed. `handle_set_notification_method` parses
    /// it (mirrors `SetAudioTheme`) so an unrecognized value can be rejected
    /// with a classified `error_code` (400), not just a bare error string.
    SetNotificationMethod {
        method: String,
    },
    GetNotificationMethod,
    SetUpdateCheckEnabled {
        enabled: bool,
    },
    GetUpdateCheckEnabled,
    /// The raw wire string, unparsed; `handle_set_update_beta_optin` parses it
    /// (mirrors `SetNotificationMethod`).
    SetUpdateBetaOptin {
        value: String,
    },
    GetUpdateBetaOptin,
    SetVolume {
        volume: u8,
    },
    GetVolume,
    SetPrimaryLanguage {
        language: String,
    },
    GetPrimaryLanguage,
    ClearPrimaryLanguage,
    /// Set the per-model language override for a specific `(source, model)`.
    SetModelLanguage {
        source: String,
        model: String,
        language: String,
    },
    /// Read the resolved language block for a specific `(source, model)`.
    GetModelLanguage {
        source: String,
        model: String,
    },
    /// Clear the per-model language override for a specific `(source, model)`.
    ClearModelLanguage {
        source: String,
        model: String,
    },
    SetAllowOnlineModels {
        enabled: bool,
    },
    GetAllowOnlineModels,
    SetCustomModelsDir {
        path: Option<String>,
    },
    GetCustomModelsDir,
    /// List installed backends with their models, secrets, and options.
    ListBackends,
    /// Re-instantiate the active model in place to apply changed secrets/options.
    ReloadActiveModel,
    /// Unload the currently loaded model. The active backend stays selected
    /// so the user can pick another model from it; to fully idle out, clear
    /// the active backend instead.
    UnloadActiveModel,
    /// Set or clear (empty value) one backend's option override.
    SetBackendOption {
        source: String,
        name: String,
        value: String,
    },
    /// Select the active backend. Does not load a model.
    SetActiveBackend {
        source: String,
    },
    /// Get the active backend.
    GetActiveBackend,
    /// Clear the active backend → unload any model, daemon idle.
    ClearActiveBackend,
    /// Read-only GPU inventory + memory. See `docs/protocol/endpoints/v1/gpu_info.md`.
    GetGpuInfo,
}
