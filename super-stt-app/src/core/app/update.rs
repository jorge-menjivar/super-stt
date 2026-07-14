// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::AppModel;
use crate::ui::messages::Message;
use cosmic::prelude::*;

impl AppModel {
    /// Pure message dispatcher — routes every [`Message`] variant to the
    /// appropriate handler method and returns the resulting [`Task`].
    pub(in crate::core::app) fn dispatch(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        if let Some(task) = self.dispatch_core(message) {
            return task;
        }
        Task::none()
    }

    /// Routes daemon, model, models-page, device, and download messages.
    fn dispatch_core(&mut self, message: Message) -> Option<Task<cosmic::Action<Message>>> {
        // Scoped action failure: park it in the per-page banner slot.
        if let Message::SettingActionFailed { scope, message } = message {
            self.action_error = Some(crate::state::ActionError { scope, message });
            return Some(Task::none());
        }

        // Daemon-related messages
        if matches!(
            message,
            Message::DaemonConnectionResult(_)
                | Message::DaemonConnected
                | Message::EventStreamConnected
                | Message::CurrentAudioThemeLoaded(_)
                | Message::VolumeLoaded(_)
                | Message::CustomModelsDirLoaded(_)
                | Message::DaemonError(_)
                | Message::RetryConnection
                | Message::WidgetBlocked(_)
                | Message::RetryAuthorization
                | Message::RefreshDaemonStatus
                | Message::PingTimeout
                | Message::DaemonEventsReceived(_)
        ) {
            return Some(self.handle_daemon_messages(message));
        }

        // Model-related messages
        if matches!(
            message,
            Message::LoadInitialData
                | Message::AvailableModelsLoaded(_)
                | Message::CurrentModelLoaded { .. }
                | Message::ModelChanged { .. }
                | Message::ModelError(_)
        ) {
            return Some(self.handle_model_messages(message));
        }

        // Models-page UI: tabs, per-backend dropdown / GPU / select,
        // configuration sub-view, and the (UI-only) download actions.
        if matches!(
            message,
            Message::ModelsTabActivated(_)
                | Message::StageActiveModel(_)
                | Message::StageActiveDevice(_)
                | Message::LoadStagedModel
                | Message::UnloadActiveModel
                | Message::OpenBackendConfig(_)
                | Message::CloseBackendConfig
                | Message::SelectBackend(_)
                | Message::DeselectBackend
                | Message::ActiveBackendLoaded(_)
                | Message::RefreshGpuInfo
                | Message::GpuInfoLoaded(_)
                | Message::InstallBackend(_)
                | Message::InstallBackendFromRepoUrl(_)
                | Message::InstallAccepted { .. }
                | Message::InstallFailedToStart { .. }
                | Message::InstallProgress { .. }
                | Message::InstallCompleted { .. }
                | Message::InstallFailed { .. }
                | Message::UpdateBackend(_)
                | Message::UninstallBackend(_)
                | Message::UninstallFailed { .. }
                | Message::RefreshRegistry
                | Message::RegistryListLoaded(_)
                | Message::RegistryListFailed(_)
                | Message::RegistrySearchChanged(_)
                | Message::RegistryIncludeIncompatible(_)
                | Message::RegistryOnlineFilter(_)
                | Message::ToggleInstalledMenu(_)
                | Message::CloseInstalledMenu
                | Message::ImportBackendFromDir
                | Message::ImportBackendFromDirPicked(_)
                | Message::RegistryCustomRepoInputChanged(_)
        ) {
            return Some(self.handle_models_page_messages(message));
        }

        // Device-related messages
        if matches!(
            message,
            Message::DeviceInfoLoaded(_, _) | Message::DeviceError(_)
        ) {
            return Some(self.handle_device_messages(message));
        }

        // Download-related messages
        if matches!(
            message,
            Message::DownloadProgressUpdate(_)
                | Message::CancelDownload
                | Message::DownloadCompleted(_)
                | Message::DownloadCancelled(_)
                | Message::DownloadError { .. }
                | Message::CheckDownloadStatus
                | Message::NoDownloadInProgress
        ) {
            return Some(self.handle_download_messages(message));
        }

        self.dispatch_settings(message)
    }

    /// Routes settings, backend, shell, and recording/audio messages.
    fn dispatch_settings(&mut self, message: Message) -> Option<Task<cosmic::Action<Message>>> {
        // Preview typing-related messages
        if matches!(
            message,
            Message::PreviewTypingToggled(_)
                | Message::PreviewTypingSettingLoaded(_)
                | Message::PreviewTypingError(_)
        ) {
            return Some(self.handle_preview_typing_messages(message));
        }

        // Recording stop mode messages
        if matches!(
            message,
            Message::RecordingStopModeChanged(_)
                | Message::RecordingStopModeLoaded(_)
                | Message::RecordingStopModeError(_)
        ) {
            return Some(self.handle_recording_stop_mode_messages(message));
        }

        // Write method messages
        if matches!(
            message,
            Message::WriteMethodChanged(_)
                | Message::WriteMethodLoaded(_)
                | Message::WriteMethodError(_)
        ) {
            return Some(self.handle_write_method_messages(message));
        }

        // Backend catalog + per-backend secret/option configuration
        if matches!(
            message,
            Message::BackendsLoaded(_)
                | Message::BackendsReload
                | Message::BackendsError(_)
                | Message::BackendSecretInputChanged { .. }
                | Message::BackendSecretSaved { .. }
                | Message::BackendSecretRemoved { .. }
                | Message::BackendSecretStored { .. }
                | Message::BackendSecretsConfigured { .. }
                | Message::BackendOptionInputChanged { .. }
                | Message::BackendOptionSaved { .. }
                | Message::BackendOptionReset { .. }
        ) {
            return Some(self.handle_backend_messages(message));
        }

        // Transcription language messages
        if matches!(
            message,
            Message::OpenLanguagePicker { .. }
                | Message::CloseLanguagePicker
                | Message::LanguagePickerQueryChanged(_)
                | Message::PrimaryLanguageLoaded(_)
                | Message::PrimaryLanguageSelected(_)
                | Message::ModelLanguageLoaded { .. }
                | Message::ModelLanguageSelected { .. }
                | Message::LanguageError(_)
        ) {
            return Some(self.handle_language_messages(message));
        }

        // Template/shell messages
        if matches!(
            message,
            Message::OpenRepositoryUrl | Message::ToggleContextPage(_) | Message::LaunchUrl(_)
        ) {
            return Some(self.handle_shell_messages(message));
        }

        // Recording/audio/widget messages
        if matches!(
            message,
            Message::StartRecording
                | Message::StopRecording
                | Message::PreviewTextReceived(_)
                | Message::TranscriptionReceived(_)
                | Message::AudioFeedbackToggled(_)
                | Message::AudioThemeSelected(_)
                | Message::AudioThemesLoaded(_)
                | Message::VolumeChanged(_)
                | Message::VolumeCommit
                | Message::WidgetAudioLevel { .. }
                | Message::WidgetRecordingState(_)
        ) {
            return Some(self.handle_recording_messages(message));
        }

        None
    }
}
