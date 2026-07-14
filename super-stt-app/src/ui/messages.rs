// SPDX-License-Identifier: GPL-3.0-only

//! Message types for the Super STT application.
//!
//! The top-level [`Message`] groups its variants into per-area sub-enums (one
//! per `handle_*_messages` handler). Dispatch (`core/app/update.rs`) is then an
//! exhaustive `match` that hands each sub-enum to its handler, and each handler
//! `match`es its sub-enum exhaustively — so a newly added variant is a compile
//! error at both ends instead of silently falling through to `Task::none()`.

use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::write_method::WriteMethod;

use cosmic::widget::segmented_button;

use crate::daemon::backends::BackendInfo;
use crate::state::{AudioTheme, ContextPage};

/// Messages emitted by the application and its widgets.
#[derive(Debug, Clone)]
pub enum Message {
    Shell(ShellMessage),
    Daemon(DaemonMessage),
    Model(ModelMessage),
    ModelsPage(ModelsPageMessage),
    Device(DeviceMessage),
    Download(DownloadMessage),
    PreviewTyping(PreviewTypingMessage),
    RecordingStopMode(RecordingStopModeMessage),
    WriteMethod(WriteMethodMessage),
    Backend(BackendMessage),
    Language(LanguageMessage),
    Recording(RecordingMessage),

    /// A scoped settings/backend save failed. Stored in `AppModel::action_error`
    /// and rendered as an inline banner on the page named by `scope`, instead of
    /// hijacking the UI (Tier 1 #13) or being dropped to the log (Tier 1 #15).
    /// Handled inline in `dispatch` (no dedicated handler).
    SettingActionFailed {
        scope: crate::state::ErrorScope,
        message: String,
    },
}

/// Template / shell-chrome messages.
#[derive(Debug, Clone)]
pub enum ShellMessage {
    OpenRepositoryUrl,
    ToggleContextPage(ContextPage),
    LaunchUrl(String),
}

/// Daemon connection, connection-time settings loads, and the SSE event stream.
#[derive(Debug, Clone)]
pub enum DaemonMessage {
    DaemonConnectionResult(super_stt_shared::daemon::http_client::HttpResult<()>),
    DaemonConnected,
    /// The `/events` SSE stream finished (re)subscribing. Distinct from
    /// `DaemonConnected` (which the REST ping loop also fires): it marks the
    /// point at which live events start flowing, so the app re-fetches the
    /// current model to capture any state that changed before the stream was
    /// subscribed (e.g. a model that finished loading during a daemon restart
    /// whose one-shot broadcast would otherwise have been missed).
    EventStreamConnected,
    // Settings loaded from daemon at connection time (replaces the
    // legacy bulk fetch_daemon_config). Each is fetched with its own
    // GET endpoint.
    CurrentAudioThemeLoaded(AudioTheme),
    VolumeLoaded(u8),
    CustomModelsDirLoaded(Option<String>),
    // Carries the typed error so `classify_daemon_error` can decide
    // blocked-vs-retry on the variant, not the wording.
    DaemonError(super_stt_shared::daemon::http_client::HttpError),
    RefreshDaemonStatus,
    RetryConnection,
    /// User denied (or had been denying via deny cache) settings-scope
    /// consent. Halts the auto-retry loop and surfaces a Retry
    /// affordance to the user.
    WidgetBlocked(String),
    /// User pressed the "Retry authorization" button in the Connection
    /// page. Drops the cached settings token and triggers a fresh
    /// consent flow.
    RetryAuthorization,
    PingTimeout,
    DaemonEventsReceived(Vec<super_stt_shared::models::protocol::NotificationEvent>),
}

/// Model identity: startup load, catalog, and the current/loaded model.
#[derive(Debug, Clone)]
pub enum ModelMessage {
    LoadInitialData, // Load models + device info at startup
    AvailableModelsLoaded(Vec<(String, Provider, String)>),
    CurrentModelLoaded {
        model: String,
        provider: Provider,
        source: String,
        /// `current_model_epoch` captured when this snapshot was requested.
        /// The handler applies the snapshot only if the epoch is unchanged —
        /// otherwise a live `model_switched` superseded it and wins.
        epoch: u64,
    },
    ModelChanged {
        model: String,
        provider: Provider,
        source: String,
    },
    ModelError(String),
}

/// Models-page UI: tabs, active-backend card, GPU readout, backend
/// select/config, and the registry / download-tab install lifecycle.
#[derive(Debug, Clone)]
pub enum ModelsPageMessage {
    /// Activate a Models-page tab (Installed / Download) in the tab bar.
    ModelsTabActivated(segmented_button::Entity),
    /// User picked a model in the active-backend card's model dropdown.
    /// Stages it for the Load button — does *not* call the daemon. Resets
    /// the staged device to the model's first supported device.
    StageActiveModel(String),
    /// User picked a device in the active-backend card's device dropdown.
    /// Stages it for the Load button — does *not* call the daemon.
    StageActiveDevice(String),
    /// User clicked the Load button. Fires `set_device(staged_device)` then
    /// `set_model(staged_model)` for the active backend. No-op when nothing
    /// is staged or a load is already in progress.
    LoadStagedModel,
    /// User clicked the Unload button. `DELETE /active_model` drops the
    /// model but keeps the active backend selected.
    UnloadActiveModel,
    /// Open the per-backend configuration sub-view for `source`.
    OpenBackendConfig(String),
    /// Leave the configuration sub-view and return to the backend list.
    CloseBackendConfig,
    /// Select a backend as active without loading a model — the card moves to
    /// the top, any model from a different backend is unloaded.
    SelectBackend(String),
    /// Deselect the active backend (unload its model → daemon idle).
    DeselectBackend,
    /// Active backend `source` loaded from the daemon (None = idle).
    ActiveBackendLoaded(Option<String>),
    /// Periodic tick that re-fetches GPU inventory + memory so the header
    /// readout stays live. No-op when disconnected; on success it emits
    /// [`ModelsPageMessage::GpuInfoLoaded`].
    RefreshGpuInfo,
    /// GPU inventory + memory loaded from the daemon (empty when none detected).
    GpuInfoLoaded(Vec<super_stt_shared::models::protocol::GpuInfo>),
    // Registry / Download-tab messages
    /// User clicked Install on a Download-tab card.
    InstallBackend(String),
    /// User clicked Install on the Custom-repo input.
    InstallBackendFromRepoUrl(String),
    /// Daemon accepted the install request.
    InstallAccepted { source: String, install_id: String },
    /// Install POST failed (couldn't start).
    InstallFailedToStart { source: String, error: String },
    /// SSE: registry.install.progress
    InstallProgress {
        install_id: String,
        source: String,
        phase: super_stt_shared::registry::events::InstallPhase,
        bytes_done: Option<u64>,
        bytes_total: Option<u64>,
    },
    /// SSE: registry.install.completed
    InstallCompleted { source: String },
    /// SSE: registry.install.failed
    InstallFailed {
        install_id: String,
        source: String,
        phase: super_stt_shared::registry::events::InstallPhase,
        error: super_stt_shared::registry::events::InstallError,
    },
    /// User clicked Update on an Installed-tab card.
    UpdateBackend(String),
    /// User clicked Uninstall.
    UninstallBackend(String),
    /// An uninstall request failed; carries the backend `source` and a
    /// human-readable error to surface on the installed card.
    UninstallFailed { source: String, error: String },
    /// User clicked Retry on the Download-tab empty state, or any other refresh trigger.
    RefreshRegistry,
    /// Initial fetch of /registry/backends succeeded.
    RegistryListLoaded(super_stt_shared::registry::RegistryListResponse),
    /// Initial fetch of /registry/backends failed.
    RegistryListFailed(String),
    /// User typed in the search box.
    RegistrySearchChanged(String),
    /// User toggled "show incompatible".
    RegistryIncludeIncompatible(bool),
    /// User chose an online filter.
    RegistryOnlineFilter(Option<bool>),
    /// Toggle the per-row overflow ("⋯") menu on an installed-backend card,
    /// keyed by backend `source`. Opening one closes any other.
    ToggleInstalledMenu(String),
    /// Dismiss any open installed-backend overflow menu (click-outside).
    CloseInstalledMenu,
    /// User clicked "+ Import from dir" on the Download tab. Opens an async
    /// folder picker; if the user picks one, the path comes back as
    /// [`ModelsPageMessage::ImportBackendFromDirPicked`].
    ImportBackendFromDir,
    /// Async folder picker resolved. `None` means the user cancelled — no-op.
    ImportBackendFromDirPicked(Option<String>),
    /// User typed in the Custom-repo URL field in the Download tab.
    RegistryCustomRepoInputChanged(String),
}

/// Device inventory + device-switch errors.
#[derive(Debug, Clone)]
pub enum DeviceMessage {
    DeviceInfoLoaded(String, Vec<String>), // Current device, available devices
    DeviceError(String),                   // Device switching error
}

/// Model-download progress lifecycle.
#[derive(Debug, Clone)]
pub enum DownloadMessage {
    DownloadProgressUpdate(super_stt_shared::models::protocol::DownloadProgress),
    CancelDownload,
    DownloadCompleted(String), // model name
    DownloadCancelled(String), // model name
    DownloadError { model: String, error: String },
    CheckDownloadStatus,
    NoDownloadInProgress,
}

/// Preview-typing setting.
#[derive(Debug, Clone)]
pub enum PreviewTypingMessage {
    Toggled(bool),       // User toggled the setting
    SettingLoaded(bool), // Setting loaded from daemon
    Error(String),       // Error setting or getting preview typing
}

/// Recording stop-mode setting.
#[derive(Debug, Clone)]
pub enum RecordingStopModeMessage {
    Changed(RecordingStopMode),
    Loaded(RecordingStopMode),
    Error(String),
}

/// Write-method setting.
#[derive(Debug, Clone)]
pub enum WriteMethodMessage {
    Changed(WriteMethod),
    Loaded(WriteMethod),
    Error(String),
}

/// Backend catalog + per-backend secret/option configuration.
/// Secrets are managed via the daemon's secrets endpoints; options go to the
/// daemon config via the client.
#[derive(Debug, Clone)]
pub enum BackendMessage {
    BackendsLoaded(Vec<BackendInfo>),
    /// Re-fetch the backend catalog (e.g. after a secret/option save) so the
    /// UI reflects the new effective values.
    BackendsReload,
    BackendsError(String),
    /// Daemon-sourced configured flags for a backend's secrets, received after
    /// `BackendsLoaded`. Folds `(name, configured)` into `backend_secret_configured`.
    BackendSecretsConfigured {
        source: String,
        items: Vec<(String, bool)>,
    },
    BackendSecretInputChanged {
        source: String,
        name: String,
        value: String,
    },
    BackendSecretSaved {
        source: String,
        name: String,
    },
    /// Daemon confirmed that a backend secret was written successfully.
    /// Triggers input-buffer clearance and a catalog reload.
    BackendSecretStored {
        source: String,
        name: String,
    },
    BackendSecretRemoved {
        source: String,
        name: String,
    },
    BackendOptionInputChanged {
        source: String,
        name: String,
        value: String,
    },
    BackendOptionSaved {
        source: String,
        name: String,
    },
    BackendOptionReset {
        source: String,
        name: String,
    },
}

/// Transcription language (global Primary Language + per-model override).
#[derive(Debug, Clone)]
pub enum LanguageMessage {
    /// Open the language search sheet.
    /// `model = None` → global Primary Language sheet.
    /// `model = Some((source, model))` → per-model sheet for that specific model.
    OpenLanguagePicker {
        model: Option<(String, String)>,
    },
    CloseLanguagePicker,
    LanguagePickerQueryChanged(String),
    /// Global Primary Language loaded from the daemon (None = unset).
    PrimaryLanguageLoaded(Option<String>),
    /// User picked a global language (None = clear → DELETE).
    PrimaryLanguageSelected(Option<String>),
    /// Per-model resolution block (`/backends/{source}/models/{model}/language`)
    /// loaded from the daemon for `(source, model)`.
    ModelLanguageLoaded {
        source: String,
        model: String,
        block: serde_json::Value,
    },
    /// User picked a per-model override.
    /// `choice = None` → Follow global (DELETE override);
    /// `choice = Some("auto")` → Auto-detect;
    /// `choice = Some(tag)` → explicit BCP-47 tag.
    ModelLanguageSelected {
        source: String,
        model: String,
        choice: Option<String>,
    },
    LanguageError(String),
}

/// Recording / audio / widget (SSE-driven meter + coarse recording state).
#[derive(Debug, Clone)]
pub enum RecordingMessage {
    StartRecording,
    StopRecording,
    PreviewTextReceived(String),
    TranscriptionReceived(String),
    AudioFeedbackToggled(bool),
    AudioThemeSelected(AudioTheme),
    AudioThemesLoaded(Vec<AudioTheme>),
    /// A drag tick: updates the local slider value only (no daemon POST).
    VolumeChanged(u8),
    /// The slider was released: commit the current value to the daemon once,
    /// rather than one POST per drag tick (Tier 1 #19).
    VolumeCommit,
    /// `frequency_bands` event from the daemon's `/events` SSE stream
    /// — already converted to (`display_level_percent`, `is_speech`) so
    /// the settings UI can drive its meter unchanged.
    WidgetAudioLevel {
        level: f32,
        is_speech: bool,
    },
    /// `recording_state` event from `/events` — coarse `is_recording`
    /// flag the UI projects into a `RecordingStatus`.
    WidgetRecordingState(bool),
}

macro_rules! message_from {
    ($($variant:ident => $ty:ident),+ $(,)?) => {
        $(
            impl From<$ty> for Message {
                fn from(m: $ty) -> Self {
                    Message::$variant(m)
                }
            }
        )+
    };
}

message_from! {
    Shell => ShellMessage,
    Daemon => DaemonMessage,
    Model => ModelMessage,
    ModelsPage => ModelsPageMessage,
    Device => DeviceMessage,
    Download => DownloadMessage,
    PreviewTyping => PreviewTypingMessage,
    RecordingStopMode => RecordingStopModeMessage,
    WriteMethod => WriteMethodMessage,
    Backend => BackendMessage,
    Language => LanguageMessage,
    Recording => RecordingMessage,
}
