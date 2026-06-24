// SPDX-License-Identifier: GPL-3.0-only

//! Message types for the Super STT application.

use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::write_method::WriteMethod;

use cosmic::widget::segmented_button;

use crate::daemon::backends::BackendInfo;
use crate::state::{AudioTheme, ContextPage};

/// Messages emitted by the application and its widgets
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Message {
    OpenRepositoryUrl,
    ToggleContextPage(ContextPage),
    LaunchUrl(String),

    // Super STT specific messages
    StartRecording,
    StopRecording,
    PreviewTextReceived(String),
    ConnectToDaemon,
    DaemonConnectionResult(Result<(), String>),
    DaemonConnected,
    // Settings loaded from daemon at connection time (replaces the
    // legacy bulk fetch_daemon_config). Each is fetched with its own
    // GET endpoint.
    CurrentAudioThemeLoaded(AudioTheme),
    VolumeLoaded(u8),
    CustomModelsDirLoaded(Option<String>),
    DaemonError(String),
    TranscriptionReceived(String),
    AudioFeedbackToggled(bool),
    AudioThemeSelected(AudioTheme),
    SetAudioTheme(AudioTheme),
    AudioThemesLoaded(Vec<AudioTheme>),
    RefreshDaemonStatus,
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
    DaemonEventsReceived(Vec<super_stt_shared::models::protocol::NotificationEvent>), // Received events
    DaemonEventsError(String), // Error receiving or parsing events
    RecordingStateChanged(crate::state::RecordingStatus),
    AudioLevelUpdate {
        level: f32,
        is_speech: bool,
    },

    // Model management messages
    LoadInitialData, // Load models + device info at startup
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
    /// [`Message::GpuInfoLoaded`].
    RefreshGpuInfo,
    /// GPU inventory + memory loaded from the daemon (empty when none detected).
    GpuInfoLoaded(Vec<super_stt_shared::models::protocol::GpuInfo>),
    // Registry / Download-tab messages
    /// User clicked Install on a Download-tab card.
    InstallBackend(String),
    /// User clicked Install on the Custom-repo input.
    InstallBackendFromRepoUrl(String),
    /// Daemon accepted the install request.
    InstallAccepted {
        source: String,
        install_id: String,
        warning: Option<String>,
    },
    /// Install POST failed (couldn't start).
    InstallFailedToStart {
        source: String,
        error: String,
    },
    /// SSE: registry.install.progress
    InstallProgress {
        install_id: String,
        source: String,
        phase: super_stt_shared::registry::events::InstallPhase,
        bytes_done: Option<u64>,
        bytes_total: Option<u64>,
    },
    /// SSE: registry.install.completed
    InstallCompleted {
        install_id: String,
        source: String,
        version: String,
    },
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
    UninstallFailed {
        source: String,
        error: String,
    },
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
    /// [`Message::ImportBackendFromDirPicked`].
    ImportBackendFromDir,
    /// Async folder picker resolved. `None` means the user cancelled — no-op.
    ImportBackendFromDirPicked(Option<String>),
    /// User typed in the Custom-repo URL field in the Download tab.
    RegistryCustomRepoInputChanged(String),
    ModelSelected {
        model: String,
        provider: Provider,
        source: String,
    },
    ModelsLoaded {
        current_model: String,
        current_provider: Provider,
        current_source: String,
        available: Vec<(String, Provider, String)>,
    },
    AvailableModelsLoaded(Vec<(String, Provider, String)>),
    CurrentModelLoaded {
        model: String,
        provider: Provider,
        source: String,
    },
    ModelChanged {
        model: String,
        provider: Provider,
        source: String,
    },
    ModelError(String),

    // Device management messages
    DeviceSelected(String),                // "cpu" or "cuda"
    DeviceLoaded(String),                  // Current device from daemon
    DeviceInfoLoaded(String, Vec<String>), // Current device, available devices
    DeviceError(String),                   // Device switching error

    // Download progress messages
    DownloadProgressUpdate(super_stt_shared::models::protocol::DownloadProgress),
    CancelDownload,
    DownloadCompleted(String), // model name
    DownloadCancelled(String), // model name
    DownloadError {
        model: String,
        error: String,
    },
    CheckDownloadStatus,
    NoDownloadInProgress,

    // Preview typing messages
    PreviewTypingToggled(bool),       // User toggled the setting
    PreviewTypingSettingLoaded(bool), // Setting loaded from daemon
    PreviewTypingError(String),       // Error setting or getting preview typing

    // Recording stop mode messages
    RecordingStopModeChanged(RecordingStopMode),
    RecordingStopModeLoaded(RecordingStopMode),
    RecordingStopModeError(String),

    // Write method messages
    WriteMethodChanged(WriteMethod),
    WriteMethodLoaded(WriteMethod),
    WriteMethodError(String),

    // Volume messages
    VolumeChanged(u8),

    // Custom models directory messages
    CustomModelsDirInput(String),
    CustomModelsDirSet(Option<String>),
    CustomModelsDirEdit(bool),
    CustomModelsDirError(String),

    // Backend catalog + per-backend secret/option configuration.
    // Secrets are managed via the daemon's secrets endpoints.
    // Options go to the daemon config via the client.
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
