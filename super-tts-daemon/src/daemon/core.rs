// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperTTSDaemon;
use super_tts_shared::models::protocol::{Command, DaemonRequest, DaemonResponse};

impl SuperTTSDaemon {
    /// Main command handler - routes commands to appropriate handlers
    pub async fn handle_command(&self, request: DaemonRequest) -> DaemonResponse {
        let command = match Command::try_from(request) {
            Ok(cmd) => cmd,
            Err(e) => return DaemonResponse::error(&e),
        };

        match command {
            Command::Speak { text } => self.handle_speak(text).await,
            Command::StopSpeaking => self.handle_stop_speaking().await,
            Command::Ping { client_id } => self.handle_ping(client_id),
            Command::Status => self.handle_status().await,
            Command::SetAudioTheme { theme } => self.handle_set_audio_theme(theme),
            Command::GetAudioTheme => self.handle_get_audio_theme(),
            Command::TestAudioTheme => self.handle_test_audio_theme().await,
            Command::SetModel { model, source } => self.handle_set_model(model, source).await,
            Command::GetModel => self.handle_get_model().await,
            Command::ListModels => self.handle_list_models().await,
            Command::SetDevice { device } => self.handle_set_device(device).await,
            Command::GetDevice => self.handle_get_device().await,
            cmd @ (Command::SetModelDevice { .. }
            | Command::GetModelDevice { .. }
            | Command::ListModelDevices { .. }
            | Command::ListActiveBackendDevices) => self.handle_model_device(cmd).await,
            Command::GetConfig => self.handle_get_config().await,
            Command::CancelDownload => self.handle_cancel_download(),
            Command::GetDownloadStatus => self.handle_get_download_status(),
            Command::ListAudioThemes => self.handle_list_audio_themes(),
            Command::SetNotificationMethod { method } => {
                self.handle_set_notification_method(method).await
            }
            Command::GetNotificationMethod => self.handle_get_notification_method().await,
            Command::SetUpdateCheckEnabled { enabled } => {
                self.handle_set_update_check_enabled(enabled).await
            }
            Command::GetUpdateCheckEnabled => self.handle_get_update_check_enabled().await,
            Command::SetUpdateBetaOptin { value } => self.handle_set_update_beta_optin(value).await,
            Command::GetUpdateBetaOptin => self.handle_get_update_beta_optin().await,
            Command::SetVolume { volume } => self.handle_set_volume(volume),
            Command::GetVolume => self.handle_get_volume(),
            Command::SetPrimaryLanguage { language } => {
                self.handle_set_primary_language(language).await
            }
            Command::GetPrimaryLanguage => self.handle_get_primary_language().await,
            Command::ClearPrimaryLanguage => self.handle_clear_primary_language().await,
            Command::ListPrimaryLanguages => Self::handle_list_primary_languages(),
            cmd @ (Command::SetModelLanguage { .. }
            | Command::GetModelLanguage { .. }
            | Command::ClearModelLanguage { .. }
            | Command::ListModelLanguages { .. }) => self.handle_model_language(cmd).await,
            cmd @ (Command::SetModelVoice { .. }
            | Command::GetModelVoice { .. }
            | Command::ClearModelVoice { .. }
            | Command::ListModelVoices { .. }) => self.handle_model_voice(cmd).await,
            Command::SetAllowOnlineModels { enabled } => {
                self.handle_set_allow_online_models(enabled).await
            }
            Command::GetAllowOnlineModels => self.handle_get_allow_online_models().await,
            Command::SetCustomModelsDir { path } => self.handle_set_custom_models_dir(path).await,
            Command::GetCustomModelsDir => self.handle_get_custom_models_dir().await,
            Command::ListBackends => self.handle_list_backends().await,
            Command::ReloadActiveModel => self.handle_reload_active_model().await,
            Command::UnloadActiveModel => self.handle_unload_active_model().await,
            Command::SetBackendOption {
                source,
                name,
                value,
            } => self.handle_set_backend_option(source, name, value).await,
            Command::SetActiveBackend { source } => self.handle_set_active_backend(source).await,
            Command::GetActiveBackend => self.handle_get_active_backend().await,
            Command::GetGpuInfo => Self::handle_get_gpu_info().await,
            Command::ClearActiveBackend => self.handle_clear_active_backend().await,
            Command::GetPipeline => self.handle_get_pipeline().await,
        }
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
