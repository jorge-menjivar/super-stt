// SPDX-License-Identifier: GPL-3.0-only

//! Message types for the Super STT application.

use super_stt_shared::models::provider::Provider;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::registry::SourceKind;
use super_stt_shared::models::write_method::WriteMethod;

use crate::state::{AudioTheme, ContextPage};

/// Messages emitted by the application and its widgets
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Message {
    // Original template messages
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
    ModelSearchChanged(String),
    ModelSelected {
        model: String,
        provider: Provider,
        source: SourceKind,
    },
    ModelsLoaded {
        current_model: String,
        current_provider: Provider,
        current_source: SourceKind,
        available: Vec<(String, Provider, SourceKind)>,
    },
    AvailableModelsLoaded(Vec<(String, Provider, SourceKind)>),
    CurrentModelLoaded {
        model: String,
        provider: Provider,
        source: SourceKind,
    },
    ModelChanged {
        model: String,
        provider: Provider,
        source: SourceKind,
    },
    ModelError(String),

    // Device management messages
    DeviceSelected(String), // "cpu" or "cuda"
    DeviceLoaded(String),   // Current device from daemon
    DeviceInfoLoaded(
        String,
        Vec<String>,
        super_stt_shared::daemon::http_client::GpuMemoryInfo,
    ), // Current device, available devices, GPU memory (free, total)
    DeviceError(String),    // Device switching error

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

    // Online models messages
    AllowOnlineModelsToggled(bool),
    AllowOnlineModelsLoaded(bool),
    AllowOnlineModelsError(String),
    OpenAIApiKeyChanged(String),
    OpenAIApiKeySaved,
    OpenAIApiKeyRemoved,
    OpenAIApiKeyError(String),
    OpenAIApiKeyStatusLoaded(bool),
    MistralApiKeyChanged(String),
    MistralApiKeySaved,
    MistralApiKeyRemoved,
    MistralApiKeyError(String),
    MistralApiKeyStatusLoaded(bool),
    DeepgramApiKeyChanged(String),
    DeepgramApiKeySaved,
    DeepgramApiKeyRemoved,
    DeepgramApiKeyError(String),
    DeepgramApiKeyStatusLoaded(bool),
}
