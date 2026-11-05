// SPDX-License-Identifier: GPL-3.0-only

use crate::tts::daemon::types::SuperTTSDaemon;
use super_voice_shared::tts::models::protocol::{Command, DaemonRequest, DaemonResponse};

impl SuperTTSDaemon {
    /// Route a text-to-speech command to its handler.
    ///
    /// Only the feature's own commands are answered here: its pipeline stage,
    /// speech and voices. What the daemon has once — the audio theme, volume,
    /// notifications, updates, the global language, the backend catalog — is
    /// served by the daemon's shared routes, and a command for it is refused.
    pub async fn handle_command(&self, request: DaemonRequest) -> DaemonResponse {
        let command = match Command::try_from(request) {
            Ok(cmd) => cmd,
            Err(e) => return DaemonResponse::error(&e),
        };

        match command {
            Command::Speak { text } => self.handle_speak(text).await,
            Command::StopSpeaking => self.handle_stop_speaking().await,
            Command::Status => self.handle_status().await,
            Command::SetModel { model, source } => self.handle_set_model(model, source).await,
            Command::GetModel => self.handle_get_model().await,
            Command::ListModels => self.handle_list_models().await,
            cmd @ (Command::SetModelDevice { .. }
            | Command::GetModelDevice { .. }
            | Command::ListModelDevices { .. }
            | Command::ListActiveBackendDevices) => self.handle_model_device(cmd).await,
            Command::GetConfig => self.handle_get_config().await,
            Command::CancelDownload => self.handle_cancel_download(),
            Command::GetDownloadStatus => self.handle_get_download_status(),
            cmd @ (Command::SetModelLanguage { .. }
            | Command::GetModelLanguage { .. }
            | Command::ClearModelLanguage { .. }
            | Command::ListModelLanguages { .. }) => self.handle_model_language(cmd).await,
            cmd @ (Command::SetModelVoice { .. }
            | Command::GetModelVoice { .. }
            | Command::ClearModelVoice { .. }
            | Command::ListModelVoices { .. }) => self.handle_model_voice(cmd).await,
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
            Command::ClearActiveBackend => self.handle_clear_active_backend().await,
            Command::GetPipeline => self.handle_get_pipeline().await,
        }
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
