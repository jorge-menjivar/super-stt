// SPDX-License-Identifier: GPL-3.0-only
use crate::models::provider::Provider;
use crate::models::recording_stop_mode::RecordingStopMode;
use crate::models::write_method::WriteMethod;

#[derive(Debug)]
pub enum Command {
    Transcribe {
        audio_data: Vec<f32>,
        sample_rate: u32,
        client_id: String,
    },
    Ping {
        client_id: Option<String>,
    },
    Status,
    StartRealTimeTranscription {
        client_id: String,
        sample_rate: Option<u32>,
        language: Option<String>,
    },
    RealTimeAudioChunk {
        client_id: String,
        audio_data: Vec<f32>,
        sample_rate: u32,
    },
    Record {
        write_mode: bool,
        stop_mode: Option<RecordingStopMode>,
        wait: bool,
        preview: Option<bool>,
    },
    SetAudioTheme {
        theme: String,
    },
    GetAudioTheme,
    TestAudioTheme,
    SetModel {
        model: String,
        provider: Provider,
        /// Backend repo id that serves the model (e.g.
        /// `github.com/super-stt/openai`). Empty selects the first backend
        /// serving `(model, provider)`.
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
    SetRecordingStopMode {
        mode: RecordingStopMode,
    },
    GetRecordingStopMode,
    SetWriteMethod {
        method: WriteMethod,
    },
    GetWriteMethod,
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
    /// Select the active backend (the provider in use). Does not load a model.
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
