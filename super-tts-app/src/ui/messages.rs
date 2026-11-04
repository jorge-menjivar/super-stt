// SPDX-License-Identifier: GPL-3.0-only

//! Message types for the Super TTS application.
//!
//! The top-level [`Message`] groups its variants into per-area sub-enums (one
//! per `handle_*_messages` handler). Dispatch (`core/app/update.rs`) is then an
//! exhaustive `match` that hands each sub-enum to its handler, and each handler
//! `match`es its sub-enum exhaustively — so a newly added variant is a compile
//! error at both ends instead of silently falling through to `Task::none()`.

use super_tts_shared::models::notification_method::NotificationMethod;

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
    NotificationMethod(NotificationMethodMessage),
    Backend(BackendMessage),
    Language(LanguageMessage),
    Voice(VoiceMessage),
    Speech(SpeechMessage),
    Update(UpdateMessage),
    Voices(VoicesMessage),

    /// A scoped settings/backend save failed. Stored in `AppModel::action_error`
    /// and rendered as an inline banner on the page named by `scope`, instead of
    /// hijacking the UI (Tier 1 #13) or being dropped to the log (Tier 1 #15).
    /// Handled inline in `dispatch` (no dedicated handler).
    SettingActionFailed {
        scope: crate::state::ErrorScope,
        message: String,
    },
}

/// The Voices page: the cloned-voice library, and the recording or import that
/// adds to it.
#[derive(Debug, Clone)]
pub enum VoicesMessage {
    /// Fetch the library and the loaded model's cloning capability.
    Refresh,
    VoicesLoaded {
        voices: Vec<super_tts_shared::models::voices::VoiceInfo>,
        model: Option<super_tts_shared::models::voices::VoiceModelSupport>,
    },
    LoadFailed(String),

    /// The user picked a reading script, or chose to read their own words
    /// (`None`). Carries the script's id rather than the picker's row number:
    /// the picker is ordered by speech language, so a row number means nothing
    /// once the language changes underneath it.
    ScriptSelected(Option<&'static str>),

    /// Open the microphone and start capturing.
    StartRecording,
    /// One tick of the level meter and elapsed readout while recording.
    RecordingTick,
    /// Stop capturing and keep what was recorded as the pending sample. Also
    /// how a capture that ended by itself — the cap, or a device failure —
    /// is collected, so there is one path out of recording.
    StopRecording,

    /// Open the file picker.
    ImportFile,
    /// A file was chosen and read. `None` when the user cancelled.
    FileImported(Option<(String, Vec<u8>)>),
    /// The chosen file could not be read.
    ImportFailed(String),

    LabelChanged(String),
    TranscriptChanged(String),
    /// Throw the pending sample away without saving it.
    DiscardPending,
    /// Upload the pending sample as a new voice.
    Save,
    Saved(super_tts_shared::models::voices::VoiceInfo),
    SaveFailed(String),

    /// Start editing one voice's label. Carries the id and the label to seed
    /// the field with.
    BeginRename {
        id: String,
        label: String,
    },
    RenameChanged(String),
    CommitRename,
    CancelRename,
    Renamed(super_tts_shared::models::voices::VoiceInfo),
    RenameFailed(String),

    Delete(String),
    Deleted(String),
    DeleteFailed {
        id: String,
        message: String,
    },

    /// Speak a fixed sentence in this voice, so the user can hear it.
    Preview(String),
    /// The daemon accepted the preview — registration is done and the audio is
    /// on its way, so the row stops saying it is preparing one.
    Previewed,
    PreviewFailed(String),
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
    DaemonConnectionResult(super_tts_shared::daemon::http_client::HttpResult<()>),
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
    DaemonError(super_tts_shared::daemon::http_client::HttpError),
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
    DaemonEventsReceived(Vec<super_tts_shared::models::protocol::NotificationEvent>),
}

/// Model identity: startup load, catalog, and the synthesis stage's selection.
#[derive(Debug, Clone)]
pub enum ModelMessage {
    LoadInitialData, // Load the stage view, its models, and its devices at startup
    AvailableModelsLoaded(Vec<(String, String)>),
    /// A point-in-time read of the whole synthesis stage: the backend filling
    /// it, the model it is pointed at, whether that model is up, and the
    /// accelerator it is actually on.
    ///
    /// One message rather than a backend one and a model one, because the two
    /// are read together and a card that applied half of them would draw a
    /// backend name over a model row from before the switch.
    StageViewLoaded {
        view: crate::daemon::client::StageState,
        /// `current_model_epoch` captured when this snapshot was requested.
        /// The handler applies the snapshot only if the epoch is unchanged —
        /// otherwise a live `model_switched` superseded it and wins.
        epoch: u64,
    },
    ModelChanged {
        model: String,
        source: String,
    },
    ModelError(String),
    /// A stage-view snapshot query failed. Carries the `current_model_epoch`
    /// captured when the fetch was issued: the handler clears the loaded model
    /// only if the epoch is unchanged. If a live `model_switched` advanced the
    /// epoch since, the failure is stale and is logged-and-dropped rather than
    /// clobbering the fresher state — the same guard `StageViewLoaded` applies
    /// to its success path (audit 2 Tier 1 #8).
    StageViewFetchFailed {
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
    /// User picked a model in the active-backend card's model dropdown.
    /// Stages it for the Load button — the only daemon calls it makes are the
    /// reads that fetch that model's device and language, since both are
    /// per-model answers the card cannot draw without asking.
    StageActiveModel(String),
    /// User picked a device in the active-backend card's device dropdown.
    /// Stages it for the Load button — does *not* call the daemon.
    StageActiveDevice(String),
    /// User picked a device for the model already loaded. Moves it there: the
    /// daemon reloads the running model onto the new device.
    MoveLoadedModel(String),
    /// User clicked the Load button. Writes the staged device against the
    /// staged *model* when it differs from what the daemon already holds for
    /// it, then points the stage at that model. No-op when nothing is staged
    /// or a load is already in progress.
    LoadStagedModel,
    /// User clicked the Unload button. `DELETE /pipeline/{stage}/model` frees
    /// the device memory but keeps both the backend and the model it was
    /// pointed at, so loading it again — onto another device, say — is one
    /// click rather than a re-pick.
    UnloadActiveModel,
    /// User clicked Reload. Re-instantiates the same model on the same device
    /// (`POST /pipeline/{stage}/model/reload`), downloading nothing.
    ///
    /// Rarely the right button, and deliberately not the only route: writing a
    /// backend option or secret already reloads every stage running that
    /// backend. This is for what the daemon cannot see change — a keyring entry
    /// edited by another tool, a model file replaced underneath it — where the
    /// alternative is Unload, re-pick, Load.
    ReloadActiveModel,
    /// Open the per-backend configuration sub-view for `source`.
    OpenBackendConfig(String),
    /// Leave the configuration sub-view and return to the backend list.
    CloseBackendConfig,
    /// Select a backend as active without loading a model — the card moves to
    /// the top, any model from a different backend is unloaded.
    SelectBackend(String),
    /// A `SelectBackend` activation POST failed: restore the previously-active
    /// backend and surface the model-card error (audit Tier 3 #37).
    BackendSelectFailed {
        prev_active: Option<String>,
        message: String,
    },
    /// Deselect the active backend (unload its model → daemon idle).
    DeselectBackend,
    /// Active backend `source` loaded from the daemon (None = idle).
    ActiveBackendLoaded(Option<String>),
    /// Periodic tick that re-fetches GPU inventory + memory so the header
    /// readout stays live. No-op when disconnected; on success it emits
    /// [`ModelsPageMessage::GpuInfoLoaded`].
    RefreshGpuInfo,
    /// GPU inventory + memory loaded from the daemon (empty when none detected).
    GpuInfoLoaded(Vec<super_tts_shared::models::protocol::GpuInfo>),
    // Registry / Download-tab messages
    /// User clicked Install on a Download-tab card.
    InstallBackend(String),
    /// Daemon accepted the install request.
    InstallAccepted { source: String, install_id: String },
    /// Install POST failed (couldn't start).
    InstallFailedToStart { source: String, error: String },
    /// SSE: registry.install.progress
    InstallProgress {
        install_id: String,
        source: String,
        phase: super_tts_shared::registry::events::InstallPhase,
        bytes_done: Option<u64>,
        bytes_total: Option<u64>,
    },
    /// SSE: registry.install.completed
    InstallCompleted { source: String },
    /// SSE: registry.install.failed
    InstallFailed {
        install_id: String,
        source: String,
        phase: super_tts_shared::registry::events::InstallPhase,
        error: super_tts_shared::registry::events::InstallError,
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
    RegistryListLoaded(super_tts_shared::registry::RegistryListResponse),
    /// Initial fetch of /registry/backends failed.
    RegistryListFailed(String),
    /// User typed in the search box.
    RegistrySearchChanged(String),
    /// User toggled "show incompatible".
    RegistryIncludeIncompatible(bool),
    /// User chose an online filter.
    RegistryOnlineFilter(Option<bool>),
    /// User chose an online filter on the Installed tab.
    InstalledOnlineFilter(Option<bool>),
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
    /// Toggle the Browse toolbar's overflow ("⋯") menu.
    ToggleBrowseMenu,
    /// Dismiss the Browse toolbar's overflow menu (click-outside).
    CloseBrowseMenu,
    /// User picked an install source in the Add-a-backend drawer. Switching
    /// clears the preview: it described the other source.
    AddSourceChanged(crate::state::registry::AddSource),
    /// User typed in the drawer's folder-path field.
    AddFolderInputChanged(String),
    /// User pressed Check in the drawer, or submitted the URL field. Resolves
    /// the source without installing it.
    CheckAddSource,
    /// A preview resolved. Carries the backend the source would install.
    AddPreviewLoaded(Box<super_tts_shared::registry::PreviewResponse>),
    /// A preview failed. Carries the daemon's message, shown verbatim.
    AddPreviewFailed(String),
    /// One frame of the drawer's "reading" spinner. Only fires while a check
    /// is in flight; the subscription that emits it is gated on that.
    AddPreviewSpin,
    /// User pressed Install in the drawer, committing to the previewed backend.
    InstallPreviewed,
}

/// Device inventory + device-switch errors.
///
/// Two loads rather than one because the daemon answers two different
/// questions: what the stage's backend can run on at all, and what one
/// particular model can run on. The narrow answer is the accurate one, but it
/// cannot be asked until a model is picked, so the broad one is what a device
/// control has to draw itself from until then.
#[derive(Debug, Clone)]
pub enum DeviceMessage {
    /// `GET /pipeline/{stage}/device/list` — the union over the models the
    /// stage's backend serves. Re-read whenever that backend changes, since it
    /// is that backend's answer and not the host's.
    StageDevicesLoaded(Vec<String>),
    /// `GET /pipeline/{stage}/model/{model}/device` for one model: its stored
    /// preference, what that resolved to, and the devices this host can offer
    /// it. Tagged with the `(source, model)` it describes so a late answer for
    /// a model the user has already moved on from is dropped rather than
    /// applied to whatever is staged now.
    ModelDeviceLoaded {
        source: String,
        model: String,
        device: crate::daemon::client::v1::pipeline::device::ModelDevice,
    },
    /// `GET /pipeline/{stage}/model/{model}/device/list` — only the offered
    /// devices, for the case where what a model *may* run on has changed while
    /// what it is *set to* has not: a backend update can swap a CPU-only asset
    /// for an accelerated one. Re-reading the whole block there would overwrite
    /// a preference nothing touched.
    ModelDevicesListed {
        source: String,
        model: String,
        devices: Vec<String>,
    },
    /// The accelerator the stage's model actually came up on, read back from
    /// its slot once a load reports success.
    ///
    /// A `gpu` preference is only ever a request — it can fall back to the CPU
    /// — so nothing may claim an accelerator until a load has confirmed one.
    /// The `ready` broadcast carries the same fact, but this read is ordered
    /// against the load that produced it, where the broadcast may arrive before
    /// or after the switch's own reply.
    RunningDeviceLoaded(Option<String>),
    DeviceError(String), // Device switching error
}

/// Model-download progress lifecycle.
#[derive(Debug, Clone)]
pub enum DownloadMessage {
    DownloadProgressUpdate(super_tts_shared::models::protocol::DownloadProgress),
    CancelDownload,
    DownloadCompleted(String), // model name
    DownloadCancelled(String), // model name
    DownloadError { model: String, error: String },
    CheckDownloadStatus,
    NoDownloadInProgress,
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
    /// The backends `GET /pipeline/{stage}/backend/list` says can fill the
    /// synthesis stage — the subset of the catalog above that serves this
    /// stage's role.
    ///
    /// Read separately because `POST /pipeline/{stage}` refuses a backend that
    /// serves nothing the stage can run: a picker built from the general
    /// catalog offers choices the daemon then rejects, and the user only finds
    /// out by making one. The two lists hold the same backends while synthesis
    /// is the only role, which is exactly why the wrong one is easy to reach
    /// for and would go unnoticed until a second stage exists.
    StageBackendsLoaded(Vec<BackendInfo>),
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
    /// A value was picked from an option's dropdown. Unlike the text field,
    /// which stages keystrokes and waits for Save, a dropdown has nothing to
    /// press afterwards, so choosing writes.
    BackendOptionChosen {
        source: String,
        name: String,
        value: String,
    },
    BackendOptionReset {
        source: String,
        name: String,
    },
}

/// The active model's voice: which one it speaks in by default.
///
/// Separate from [`crate::ui::messages::VoicesPageMessage`], which is the
/// cloned-voice library — recording, naming and previewing clips. This is the
/// per-model preference those clips (and the model's own presets) can be
/// chosen into.
#[derive(Debug, Clone)]
pub enum VoiceMessage {
    /// Resolution block for `(source, model)`, from
    /// `GET /pipeline/{stage}/model/{model}/voice`.
    ModelVoiceLoaded {
        source: String,
        model: String,
        block: crate::state::VoiceResolution,
    },
    /// The voices `(source, model)` can be pinned to.
    ///
    /// A separate message from the block for the same reason the language pair
    /// is split: choosing a voice rewrites the block and leaves this list as it
    /// was, so folding the two would blank the picker on every pick.
    ModelVoicesListed {
        source: String,
        model: String,
        voices: Vec<crate::state::VoiceChoice>,
    },
    /// The user picked a voice for the active model, or cleared it back to the
    /// model's own default (`None`).
    ModelVoiceSelected(Option<String>),
    /// A voice request failed; carries what to show.
    VoiceError(String),
}

/// Speech language (global Primary Language + per-model override).
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
    /// The tags `GET /settings/language/list` says the global setting accepts.
    ///
    /// The daemon's own vocabulary rather than a list this app curates: it is
    /// the union of what the installed models can speak, so it grows and
    /// shrinks with the installed backends and a hard-coded copy would offer
    /// tags no model serves — and, worse, quietly stop offering ones a newly
    /// installed backend does.
    PrimaryLanguagesListed(Vec<String>),
    /// User picked a global language (None = clear → DELETE).
    PrimaryLanguageSelected(Option<String>),
    /// Per-model resolution block
    /// (`/pipeline/{stage}/model/{model}/language`) loaded from the daemon for
    /// `(source, model)`.
    ModelLanguageLoaded {
        source: String,
        model: String,
        block: crate::state::LanguageResolution,
    },
    /// The tags `GET /pipeline/{stage}/model/{model}/language/list` says this
    /// model can be pinned to.
    ///
    /// A separate message from the resolution block because they change on
    /// different occasions: pinning a language rewrites the block and leaves
    /// this list exactly as it was, so folding the two would either re-request
    /// what cannot have changed or blank the picker on every pick.
    ModelLanguagesListed {
        source: String,
        model: String,
        languages: Vec<String>,
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

/// Speech / audio / widget (SSE-driven meter + coarse speaking state).
#[derive(Debug, Clone)]
pub enum SpeechMessage {
    /// The text in the Speech page's test field changed.
    TestTextChanged(String),
    /// Speak whatever is in the test field.
    Speak,
    /// The daemon accepted the utterance and returned its id.
    SpeakStarted(String),
    /// The daemon refused it; carries the message for the page's banner.
    SpeakFailed(String),
    /// Stop the utterance now playing.
    StopSpeaking,
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
    /// `speaking_state` event from `/events` — the coarse flag the UI
    /// projects into a `SpeakingStatus`, plus the utterance it describes.
    ///
    /// The id travels with it because the daemon speaks for whoever asked: an
    /// utterance this page did not start still turns the badge on, and the id
    /// is the only way the page can tell the two apart.
    WidgetSpeakingState {
        is_speaking: bool,
        utterance_id: Option<String>,
    },
}

/// How a beta-opt-in toggle ended. Three cases because each leaves the UI
/// somewhere different: only a failed *write* may snap the toggle back — a
/// failed re-check means the setting did change and only the candidate
/// version is unknown, so reverting the switch would misreport the daemon.
#[derive(Debug, Clone)]
pub enum BetaOptinOutcome {
    /// Saved, and the re-check returned a fresh status (`None` = the daemon
    /// predates `/v1/update`).
    Applied(Option<super_tts_shared::models::self_update::SelfUpdateStatus>),
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
    StatusLoaded(Option<super_tts_shared::models::self_update::SelfUpdateStatus>),
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
    NotificationMethod => NotificationMethodMessage,
    Backend => BackendMessage,
    Language => LanguageMessage,
    Speech => SpeechMessage,
    Update => UpdateMessage,
}
