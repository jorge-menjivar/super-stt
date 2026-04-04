// SPDX-License-Identifier: GPL-3.0-only

//! Message types for the Super STT application.

use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use super_stt_shared::models::write_method::WriteMethod;
use super_stt_shared::stt_model::STTModel;

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
    DaemonConfigReceived(serde_json::Value),
    DaemonError(String),
    TranscriptionReceived(String),
    AudioFeedbackToggled(bool),
    AudioThemeSelected(AudioTheme),
    SetAudioTheme(AudioTheme),
    AudioThemesLoaded(Vec<AudioTheme>),
    RefreshDaemonStatus,
    UdpDataReceived(Vec<u8>),
    RetryConnection,
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
    ModelSelected(STTModel),
    ModelsLoaded {
        current: STTModel,
        available: Vec<STTModel>,
    },
    AvailableModelsLoaded(Vec<STTModel>),
    CurrentModelLoaded(STTModel),
    ModelChanged(STTModel),
    ModelError(String),

    // Device management messages
    DeviceSelected(String),                // "cpu" or "cuda"
    DeviceLoaded(String),                  // Current device from daemon
    DeviceInfoLoaded(String, Vec<String>), // Current device and available devices from daemon
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

    // Model override path messages
    ModelOverridePathInput(String),
    ModelOverridePathSet(Option<String>),
    ModelOverridePathEdit(bool),
    ModelOverridePathError(String),

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
