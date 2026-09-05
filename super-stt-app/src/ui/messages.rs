// SPDX-License-Identifier: GPL-3.0-only

//! Message types for the Super STT application.
//!
//! The top-level [`Message`] groups its variants into per-area sub-enums (one
//! per `handle_*_messages` handler). Dispatch (`core/app/update.rs`) is then an
//! exhaustive `match` that hands each sub-enum to its handler, and each handler
//! `match`es its sub-enum exhaustively — so a newly added variant is a compile
//! error at both ends instead of silently falling through to `Task::none()`.

use super_stt_shared::models::notification_method::NotificationMethod;
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
    /// Filling a pipeline stage, for every stage.
    Stage(StageMessage),
    Device(DeviceMessage),
    Download(DownloadMessage),
    PreviewTyping(PreviewTypingMessage),
    PostProcessor(PostProcessorMessage),
    RecordingStopMode(RecordingStopModeMessage),
    WriteMethod(WriteMethodMessage),
    NotificationMethod(NotificationMethodMessage),
    Backend(BackendMessage),
    Language(LanguageMessage),
    Recording(RecordingMessage),
    Update(UpdateMessage),

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

/// Model identity: startup load, and the current/loaded model.
#[derive(Debug, Clone)]
pub enum ModelMessage {
    LoadInitialData, // Load device info at startup
    CurrentModelLoaded {
        model: String,
        source: String,
        /// `current_model_epoch` captured when this snapshot was requested.
        /// The handler applies the snapshot only if the epoch is unchanged —
        /// otherwise a live `model_switched` superseded it and wins.
        epoch: u64,
    },
    ModelChanged {
        model: String,
        source: String,
    },
    /// A `fetch_current_model` snapshot query failed. Carries the
    /// `current_model_epoch` captured when the fetch was issued: the handler
    /// clears the loaded model only if the epoch is unchanged. If a live
    /// `model_switched` advanced the epoch since, the failure is stale and is
    /// logged-and-dropped rather than clobbering the fresher state — the same
    /// guard `CurrentModelLoaded` applies to its success path (audit 2 Tier 1 #8).
    CurrentModelFetchFailed {
        epoch: u64,
        error: String,
    },
}

/// Models-page UI: tabs, active-backend card, GPU readout, backend
/// select/config, and the registry / download-tab install lifecycle.
#[derive(Debug, Clone)]
pub enum ModelsPageMessage {
    /// Activate a Models-page tab (Installed / Download) in the tab bar.
    ModelsTabActivated(segmented_button::Entity),
    /// Open the per-backend configuration sub-view for `source`.
    OpenBackendConfig(String),
    /// Leave the configuration sub-view and return to the backend list.
    CloseBackendConfig,
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
    /// Initial fetch of /registry/backend/list succeeded.
    RegistryListLoaded(super_stt_shared::registry::RegistryListResponse),
    /// Initial fetch of /registry/backend/list failed.
    RegistryListFailed(String),
    /// User typed in the search box.
    RegistrySearchChanged(String),
    /// User toggled "show incompatible".
    RegistryIncludeIncompatible(bool),
    /// User chose an online filter.
    RegistryOnlineFilter(Option<bool>),
    /// User chose which kind of model a Browse entry must serve.
    RegistryRoleFilter(crate::state::registry::RoleFilter),
    /// User chose an online filter for the Installed tab.
    InstalledOnlineFilter(Option<bool>),
    /// User chose which kind of model an installed backend must serve.
    InstalledRoleFilter(crate::state::registry::RoleFilter),
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

/// The stage-1 model's device + device-switch errors.
#[derive(Debug, Clone)]
pub enum DeviceMessage {
    /// The accelerator the loaded model is on, from the startup read of
    /// `GET /pipeline/1`; `None` when nothing is loaded.
    DeviceInfoLoaded(Option<String>),
    DeviceError(String), // Device switching error
}

/// Model-download progress lifecycle.
#[derive(Debug, Clone)]
pub enum DownloadMessage {
    /// A progress snapshot, from a `download_progress` event or a poll. The
    /// stage it belongs to travels inside it.
    DownloadProgressUpdate(super_stt_shared::models::protocol::DownloadProgress),
    /// Abandon the download the named stage has in flight. Every variant below
    /// that settles a stage's card carries its stage for the same reason: the
    /// two stages download independently, so an outcome that did not say whose
    /// it was would settle whichever card happened to be showing.
    CancelDownload(u32),
    DownloadCompleted(String), // model name
    DownloadCancelled {
        model: String,
        stage: u32,
    },
    DownloadError {
        model: String,
        error: String,
        stage: u32,
    },
    /// Ask every stage still owed an outcome for its progress.
    CheckDownloadStatus,
    /// The named stage has no download in flight after all.
    NoDownloadInProgress(u32),
}

/// Preview-typing setting.
#[derive(Debug, Clone)]
pub enum PreviewTypingMessage {
    Toggled(bool),       // User toggled the setting
    SettingLoaded(bool), // Setting loaded from daemon
    Error(String),       // Error setting or getting preview typing
}

/// Filling a pipeline stage: select a backend, stage a model and a device,
/// load, unload.
///
/// Every variant carries the `stage` it is about, so one handler serves them
/// all. The stages were two hand-maintained copies of this flow before, and the
/// copies drifted — the newer one lost the re-entrancy guard, the stale-catalog
/// check, the device rollback, and the check that a device was staged at all.
///
/// `stage` values come from `state::device_offers::{STT_STAGE, PP_STAGE}`,
/// which are the daemon's own pipeline positions.
#[derive(Debug, Clone)]
pub enum StageMessage {
    /// Select the backend filling this stage, without loading a model. Any
    /// model from a different backend is unloaded.
    SelectBackend { stage: u32, source: String },
    /// Empty the stage, forgetting its model with it.
    DeselectBackend { stage: u32 },
    /// The daemon took the selection (`None` = the stage is empty). Re-announced
    /// after a successful select, which is what asks for the backend's devices.
    BackendSelected {
        stage: u32,
        source: Option<String>,
        /// The model the daemon remembers for the stage, running or not.
        ///
        /// `None` for a backend the user has just picked, which has no model
        /// yet. Carried on the startup read because a stage keeps its selection
        /// through an unload, and the card offers that model again rather than
        /// coming up empty.
        model: Option<String>,
    },
    /// A select or deselect was refused: restore `prev` and report it on the
    /// stage's card.
    BackendSelectFailed {
        stage: u32,
        prev: Option<String>,
        message: String,
    },
    /// User picked a model in the card's dropdown. Staged for the Load button —
    /// does *not* load. The device waits on the daemon, which is asked what the
    /// model can run on here and which device it already has.
    StageModel { stage: u32, model: String },
    /// User picked a device in the card's device dropdown.
    ///
    /// Staged locally while the model is not up. Once it is running the choice
    /// is sent straight through: the daemon reloads the model onto the new
    /// device in place, so switching device no longer means unload, re-pick,
    /// load.
    StageDevice { stage: u32, device: String },
    /// A device change the daemon refused or could not apply.
    ///
    /// Distinct from [`StageMessage::LoadFailed`]: the model is still up, on
    /// the device it had, so this rolls the picker back and reports the failure
    /// without dropping the stage's identity the way a failed load does.
    DeviceChangeFailed {
        stage: u32,
        prev_device: Option<String>,
        message: String,
    },
    /// The daemon answered what can fill `stage`: the backends, each already
    /// narrowed to the models this stage can run.
    ///
    /// Re-asked whenever the installed catalog changes, since installing or
    /// uninstalling a backend changes the answer — and never on selection,
    /// because the answer does not depend on what is selected.
    StageBackendsLoaded {
        stage: u32,
        backends: Vec<BackendInfo>,
    },
    /// The daemon answered for the staged `model` of `source`: `devices` is what
    /// it can be loaded onto here, `current` the device it already has. `source`
    /// is the backend selected when the question was asked, so an answer a
    /// switch has overtaken can be dropped.
    StagedDevicesLoaded {
        stage: u32,
        source: String,
        model: String,
        devices: Vec<String>,
        current: Option<String>,
    },
    /// The daemon answered which devices this stage's backend can run its
    /// models on — the union over the models it serves in this stage's role.
    BackendDevicesLoaded {
        stage: u32,
        source: String,
        devices: Vec<String>,
    },
    /// User clicked Load. Sets the staged model's device, then runs it. A no-op
    /// when nothing is staged or this stage already has an operation in flight.
    Load { stage: u32 },
    /// A load or unload failed: restore `prev_device` and report it on the
    /// stage's card.
    LoadFailed {
        stage: u32,
        prev_device: String,
        message: String,
    },
    /// User clicked Unload. Drops the model but keeps the backend selected.
    Unload { stage: u32 },
}

/// Post-processor state, read back from the daemon.
///
/// Selecting, staging and loading go through [`StageMessage`] like every other
/// stage's; what is left here is stage 2's own block arriving from the daemon,
/// which has no stage-1 counterpart because stage 1 announces its identity
/// through `ModelMessage` events instead.
#[derive(Debug, Clone)]
pub enum PostProcessorMessage {
    /// A write succeeded; re-read the daemon's own state rather than assuming
    /// the write's arguments took effect verbatim.
    ReloadRequested,
    /// The daemon's current state, from the initial load or after a change.
    Loaded(crate::daemon::client::StageState),
    /// A get/set failed; the message is shown inline.
    Error(String),
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
    /// Ask the daemon to type the test string. The Input Simulation test field
    /// is focused first so the keystrokes have somewhere to land.
    Test,
    /// Start the countdown before an unfocused test, giving the user time to
    /// switch to the window they actually dictate into.
    TestDelayed,
    /// One second of that countdown elapsed.
    TestTick,
    /// Abandon a running countdown before it types.
    TestCancel,
    /// The daemon typed the test string; carries the backend it resolved to,
    /// which is the only way to see which rung `Auto` picked. `None` when the
    /// daemon named no backend this build understands — the typing still
    /// happened, so that is a pass with an empty readout, not a failure.
    Tested(Option<WriteMethod>),
    /// Contents of the test field — whatever the daemon (or the user) typed.
    TestInput(String),
    Error(String),
}

/// Notification-method setting.
#[derive(Debug, Clone)]
pub enum NotificationMethodMessage {
    Changed(NotificationMethod),
    Loaded(NotificationMethod),
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
    /// A `type = "bool"` option's switch was flipped. A switch has no Save
    /// button to press, so this both records the new value and writes it.
    BackendOptionToggled {
        source: String,
        name: String,
        value: bool,
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
        /// `(stage, source, model)` for a per-model override, `None` for the
        /// global setting. The stage is what resolves the bare model name.
        model: Option<(u32, String, String)>,
    },
    CloseLanguagePicker,
    LanguagePickerQueryChanged(String),
    /// Global Primary Language loaded from the daemon (None = unset).
    PrimaryLanguageLoaded(Option<String>),
    /// The tags the global setting accepts, from `GET /settings/language/list`.
    PrimaryLanguagesLoaded(Vec<String>),
    /// User picked a global language (None = clear → DELETE).
    PrimaryLanguageSelected(Option<String>),
    /// Per-model resolution block (`/pipeline/{stage}/model/{model}/language`)
    /// loaded from the daemon for `(source, model)`.
    /// The languages a model can be pinned to, from
    /// `GET /pipeline/{stage}/model/{model}/language/list`.
    ///
    /// Separate from the resolution block: the block changes with every pick,
    /// the offer cannot change for a given model, so a pick does not re-fetch
    /// a list that has not moved.
    ModelLanguagesLoaded {
        stage: u32,
        source: String,
        model: String,
        languages: Vec<String>,
    },
    ModelLanguageLoaded {
        /// The stage the block was read through.
        stage: u32,
        source: String,
        model: String,
        block: crate::state::LanguageResolution,
    },
    /// User picked a per-model override.
    /// `choice = None` → Follow global (DELETE override);
    /// `choice = Some("auto")` → Auto-detect;
    /// `choice = Some(tag)` → explicit BCP-47 tag.
    ModelLanguageSelected {
        /// The stage the model is addressed through: it resolves the bare
        /// model name against the backend filling it.
        stage: u32,
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
    /// A theme/feedback save failed: restore the optimistically-set theme
    /// fields to the captured pre-save values and raise a scoped banner
    /// (audit Tier 3 #37).
    AudioThemeSaveFailed {
        prev_selected: AudioTheme,
        prev_non_silent: AudioTheme,
        message: String,
    },
    /// A drag tick: updates the local slider value only (no daemon POST).
    VolumeChanged(u8),
    /// The slider was released: commit the current value to the daemon once,
    /// rather than one POST per drag tick (Tier 1 #19).
    VolumeCommit,
    /// A volume commit failed: restore the slider to the last committed value
    /// and raise a scoped banner (audit Tier 3 #37).
    VolumeSaveFailed {
        prev_volume: u8,
        message: String,
    },
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

/// How a beta-opt-in toggle ended. Three cases because each leaves the UI
/// somewhere different: only a failed *write* may snap the toggle back — a
/// failed re-check means the setting did change and only the candidate
/// version is unknown, so reverting the switch would misreport the daemon.
#[derive(Debug, Clone)]
pub enum BetaOptinOutcome {
    /// Saved, and the re-check returned a fresh status (`None` = the daemon
    /// predates `/v1/update`).
    Applied(Option<super_stt_shared::models::self_update::SelfUpdateStatus>),
    /// The setting write itself failed; nothing changed daemon-side.
    WriteFailed(String),
    /// Saved, but the follow-up re-check failed. The toggle stands; only the
    /// banner reports it.
    CheckFailed(String),
}

/// Self-update: status load/check, the two settings toggles, the apply-flow
/// run (installer download + spawn + JSON progress stream), and the
/// `UpdateAvailable` SSE-driven refetch.
#[derive(Debug, Clone)]
pub enum UpdateMessage {
    StatusLoaded(Option<super_stt_shared::models::self_update::SelfUpdateStatus>),
    StatusError(String),
    CheckNow,
    AutoCheckLoaded(bool),
    AutoCheckToggled(bool),
    BetaOptinToggled(bool),
    /// The beta-opt-in write and its follow-up re-check settled. Ends the
    /// lock the toggle took when it was pressed; `enabled` is the value that
    /// was requested, so a failed write knows what to snap back from.
    BetaOptinApplied {
        enabled: bool,
        outcome: BetaOptinOutcome,
    },
    /// Open the Updates page (from the header bar's "Update available"
    /// badge). Routed through the nav model so the page's status refetch
    /// happens exactly as it does when the sidebar entry is clicked.
    OpenUpdatesPage,
    /// A settings-toggle write (`AutoCheckToggled`/`BetaOptinToggled`)
    /// failed. Distinct from `StatusError` (a fetch/check failure) so the
    /// banner names the right verb ("update a setting" vs. "fetch status").
    SettingError(String),
    StartUpdate,
    CancelUpdate,
    RunEvent(crate::core::app::updater::UpdateRunEvent),
    RestartApp,
    AvailableEventReceived,
    /// User dismissed a finished (`Done`/`Failed`) run's panel without
    /// restarting — clears it so the page returns to the idle CTA (or a
    /// future `StartUpdate` is no longer blocked). No-op on an in-flight run;
    /// that goes through `CancelUpdate` instead.
    DismissRun,
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
    PostProcessor => PostProcessorMessage,
    RecordingStopMode => RecordingStopModeMessage,
    WriteMethod => WriteMethodMessage,
    NotificationMethod => NotificationMethodMessage,
    Backend => BackendMessage,
    Language => LanguageMessage,
    Recording => RecordingMessage,
    Update => UpdateMessage,
}
