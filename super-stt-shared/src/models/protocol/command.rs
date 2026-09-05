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
    /// Stage 1's model slot: what is selected there, whether it is up, the
    /// device it runs on, and the load still in flight.
    ///
    /// Reports the *selection*, which survives an unload — not the loaded
    /// instance, which is what `loaded` is for.
    GetModel,
    /// The transcription models stage 1's backend serves.
    ListModels,
    /// The installed backends that can fill stage 1: those serving at least one
    /// transcription model.
    ///
    /// The role filter lives in the daemon rather than in each client, because
    /// the daemon already applies it when it accepts or refuses a stage's
    /// backend — a client filtering on its own can offer one the daemon then
    /// refuses.
    ListTranscriptionBackends,
    /// The stage-2 twin: backends serving at least one post-processor.
    ListPostProcessorBackends,
    /// The post-processors stage 2's backend serves.
    ///
    /// A separate variant rather than a `stage` parameter because that is how
    /// every other stage verb is spelled here — the HTTP layer's `Stage`
    /// resolver maps a position to the command that implements it.
    ListPostProcessorModels,
    /// Set the device a transcription (stage 1) model runs on: `cpu` or
    /// `gpu`. `model` is resolved against the selected transcription backend.
    /// Reloads the model when it is the loaded one; otherwise only records
    /// the choice for its next load.
    SetModelDevice {
        model: String,
        device: String,
    },
    /// Read a transcription model's device: the recorded preference, what it
    /// resolved to, and the devices this install can offer it.
    GetModelDevice {
        model: String,
    },
    /// The stage-2 twins of `SetModelDevice`/`GetModelDevice`, resolving
    /// `model` against the selected post-processor backend.
    SetPostProcessorDevice {
        model: String,
        device: String,
    },
    GetPostProcessorDevice {
        model: String,
    },
    /// The devices this install can offer a transcription model on this
    /// host — the `available_devices` half of `GetModelDevice`, on its own.
    ListModelDevices {
        model: String,
    },
    /// The devices the selected transcription backend can be run on here:
    /// the union of `ListModelDevices` over the models it serves for that
    /// stage.
    ListActiveBackendDevices,
    /// The stage-2 twins, against the selected post-processor backend.
    ListPostProcessorDevices {
        model: String,
    },
    ListPostProcessorBackendDevices,
    GetConfig,
    /// Abandon the transcription stage's in-flight download.
    CancelDownload,
    /// The stage-2 twin. Each stage cancels only its own: a post-processor
    /// downloads its weights like any other model, and one stage abandoning
    /// the other's load would be a surprise, not a courtesy.
    CancelPostProcessorDownload,
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
    SetPostProcessor {
        model: String,
        source: String,
    },
    /// Stage 2's model slot — the twin of `GetModel`, answering the identical
    /// shape. A separate variant rather than a `stage` parameter because that
    /// is how every other stage verb is spelled here.
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
    /// Re-instantiate the loaded post-processor in place so a changed secret
    /// or option takes effect — the stage-2 twin of [`Self::ReloadActiveModel`].
    ReloadPostProcessor,
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
    /// The languages a specific `(source, model)` can be pinned to.
    ///
    /// The set `SetModelLanguage` accepts, not merely what the manifest
    /// declares: `auto` is choosable and monolingual models accept nothing, so
    /// a list built from `supported_languages` alone would offer a value the
    /// setter refuses and omit one it takes.
    ListModelLanguages {
        source: String,
        model: String,
    },
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
