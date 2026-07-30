// SPDX-License-Identifier: GPL-3.0-only

use crate::{daemon::types::SuperSTTDaemon, output::keyboard::Simulator, output::typer::Typer};
use super_stt_shared::models::protocol::{Command, DaemonRequest, DaemonResponse};

impl SuperSTTDaemon {
    /// Main command handler - routes commands to appropriate handlers
    pub async fn handle_command(&self, request: DaemonRequest) -> DaemonResponse {
        let command = match Command::try_from(request) {
            Ok(cmd) => cmd,
            Err(e) => return DaemonResponse::error(&e),
        };

        match command {
            Command::Transcribe {
                audio_data,
                sample_rate,
                client_id,
                language,
            } => {
                self.handle_transcribe(audio_data, sample_rate, client_id, language)
                    .await
            }
            Command::Ping { client_id } => self.handle_ping(client_id),
            Command::Status => self.handle_status().await,
            Command::Record {
                write_mode,
                stop_mode,
                preview,
                language,
                ..
            } => {
                self.handle_record_command(write_mode, stop_mode, preview, language)
                    .await
            }
            Command::SetAudioTheme { theme } => self.handle_set_audio_theme(theme),
            Command::GetAudioTheme => self.handle_get_audio_theme(),
            Command::TestAudioTheme => self.handle_test_audio_theme().await,
            Command::SetModel {
                model,
                provider,
                source,
            } => self.handle_set_model(model, provider, source).await,
            Command::GetModel => self.handle_get_model().await,
            Command::ListModels => self.handle_list_models().await,
            Command::SetDevice { device } => self.handle_set_device(device).await,
            Command::GetDevice => self.handle_get_device().await,
            Command::GetConfig => self.handle_get_config().await,
            Command::CancelDownload => self.handle_cancel_download(),
            Command::GetDownloadStatus => self.handle_get_download_status(),
            Command::ListAudioThemes => self.handle_list_audio_themes(),
            Command::SetPreviewTyping { enabled } => self.handle_set_preview_typing(enabled).await,
            Command::GetPreviewTyping => self.handle_get_preview_typing(),
            Command::SetRecordingStopMode { mode } => {
                self.handle_set_recording_stop_mode(mode).await
            }
            Command::GetRecordingStopMode => self.handle_get_recording_stop_mode().await,
            Command::SetWriteMethod { method } => self.handle_set_write_method(method).await,
            Command::GetWriteMethod => self.handle_get_write_method().await,
            Command::SetVolume { volume } => self.handle_set_volume(volume),
            Command::GetVolume => self.handle_get_volume(),
            Command::SetPrimaryLanguage { language } => {
                self.handle_set_primary_language(language).await
            }
            Command::GetPrimaryLanguage => self.handle_get_primary_language().await,
            Command::ClearPrimaryLanguage => self.handle_clear_primary_language().await,
            cmd @ (Command::SetModelLanguage { .. }
            | Command::GetModelLanguage { .. }
            | Command::ClearModelLanguage { .. }) => self.handle_model_language(cmd).await,
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
        }
    }

    /// Handle a record command — resolve mode, toggle stop, or start recording.
    async fn handle_record_command(
        &self,
        write_mode: bool,
        stop_mode: Option<super_stt_shared::models::recording_stop_mode::RecordingStopMode>,
        preview: Option<bool>,
        language: Option<String>,
    ) -> DaemonResponse {
        // Resolve effective mode: per-request override or daemon config default
        let effective_mode = if let Some(mode) = stop_mode {
            mode
        } else {
            let config = self.config.read().await;
            config.transcription.recording_stop_mode
        };

        // Toggle behaviour: if already busy, stop it (if mode allows)
        let busy = *self.busy.read().await;
        if busy {
            let guard = self.manual_stop_tx.read().await;
            if guard.is_none() {
                log::info!("Transcription in progress, please wait");
                return DaemonResponse::success()
                    .with_message("Transcription in progress, please wait".to_string());
            }
            if !effective_mode.manual_stop_enabled() {
                log::info!("Second press ignored: recording in SilenceOnly mode");
                return DaemonResponse::success()
                    .with_message("Manual stop not enabled in current mode".to_string());
            }
            if let Some(tx) = guard.as_ref() {
                let _ = tx.send(());
                log::info!("🛑 Stop triggered via shortcut while recording");
            }
            return DaemonResponse::success()
                .with_message(DaemonResponse::RECORDING_STOP_SIGNAL_MSG.to_string());
        }
        // Take the cached simulator, or create a new one.
        let simulator = {
            let mut guard = self.simulator.write().await;
            guard.take()
        };
        let simulator = if let Some(s) = simulator {
            s
        } else {
            let write_method = {
                let config = self.config.read().await;
                config.transcription.write_method
            };
            match Simulator::new(write_method).await {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Failed to create keyboard simulator: {e}");
                    return DaemonResponse::error(&format!("Keyboard simulator failed: {e}"));
                }
            }
        };
        // Temporarily override preview setting for this recording, restore after.
        let original_preview = self
            .preview_typing_enabled
            .load(std::sync::atomic::Ordering::Relaxed);
        if let Some(override_val) = preview {
            self.preview_typing_enabled
                .store(override_val, std::sync::atomic::Ordering::Relaxed);
        }

        let mut typer = Typer::new(simulator);
        let response = self
            .handle_record_internal(&mut typer, write_mode, effective_mode, language.as_deref())
            .await;

        // Restore original preview setting.
        if preview.is_some() {
            self.preview_typing_enabled
                .store(original_preview, std::sync::atomic::Ordering::Relaxed);
        }
        // Return the simulator to the cache for reuse, unless this backend
        // goes stale while idle (see `Simulator::is_cacheable`) — in which
        // case it is dropped here and the next recording builds a fresh one.
        let simulator = typer.take_simulator();
        if simulator.is_cacheable() {
            *self.simulator.write().await = Some(simulator);
        }
        response
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
