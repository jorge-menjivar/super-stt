// SPDX-License-Identifier: GPL-3.0-only

use crate::{daemon::types::SuperSTTDaemon, output::keyboard::Simulator, output::typer::Typer};
use super_stt_shared::models::protocol::{Command, DaemonRequest, DaemonResponse};

impl SuperSTTDaemon {
    /// Main command handler - routes commands to appropriate handlers
    // A flat table with one arm per wire command: its length is the
    // protocol's, and the exhaustive match is what makes a new command a
    // compile error rather than a silent gap. Splitting it would only hide
    // the table.
    #[allow(clippy::too_many_lines)]
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
            Command::SetModel { model, source } => self.handle_set_model(model, source).await,
            Command::GetModel => {
                self.handle_get_stage_model(
                    crate::daemon::device_management::PipelineStage::Transcription,
                )
                .await
            }
            Command::ListModels => self.handle_list_stage_models(false).await,
            Command::ListPostProcessorModels => self.handle_list_stage_models(true).await,
            cmd @ (Command::SetModelDevice { .. }
            | Command::GetModelDevice { .. }
            | Command::SetPostProcessorDevice { .. }
            | Command::GetPostProcessorDevice { .. }
            | Command::ListModelDevices { .. }
            | Command::ListActiveBackendDevices
            | Command::ListPostProcessorDevices { .. }
            | Command::ListPostProcessorBackendDevices) => self.handle_model_device(cmd).await,
            Command::GetConfig => self.handle_get_config().await,
            Command::CancelDownload => self.handle_cancel_download(
                crate::daemon::device_management::PipelineStage::Transcription,
            ),
            Command::CancelPostProcessorDownload => self.handle_cancel_download(
                crate::daemon::device_management::PipelineStage::PostProcessor,
            ),
            // Routeless: every client reads a stage's in-flight load from
            // `GET /pipeline/{stage}`, which asks per stage. Kept as the
            // transcription stage's for the internal callers that predate it.
            Command::GetDownloadStatus => self.handle_get_download_status(
                crate::daemon::device_management::PipelineStage::Transcription,
            ),
            Command::ListAudioThemes => self.handle_list_audio_themes(),
            Command::SetPreviewTyping { enabled } => self.handle_set_preview_typing(enabled).await,
            Command::GetPreviewTyping => self.handle_get_preview_typing(),
            Command::SetPostProcessor { model, source } => {
                self.handle_set_post_processor(model, source).await
            }
            Command::GetPostProcessor => {
                self.handle_get_stage_model(
                    crate::daemon::device_management::PipelineStage::PostProcessor,
                )
                .await
            }
            Command::ClearPostProcessor => self.handle_clear_post_processor().await,
            Command::SetPostProcessorBackend { source } => {
                self.handle_set_post_processor_backend(source).await
            }
            Command::ClearPostProcessorBackend => self.handle_clear_post_processor_backend().await,
            Command::ReloadPostProcessor => self.handle_reload_post_processor().await,
            Command::GetPipeline => self.handle_get_pipeline().await,
            Command::SetRecordingStopMode { mode } => {
                self.handle_set_recording_stop_mode(mode).await
            }
            Command::GetRecordingStopMode => self.handle_get_recording_stop_mode().await,
            Command::SetWriteMethod { method } => self.handle_set_write_method(method).await,
            Command::GetWriteMethod => self.handle_get_write_method().await,
            Command::TestWriteMethod => self.handle_test_write_method().await,
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
            cmd @ (Command::SetModelLanguage { .. }
            | Command::GetModelLanguage { .. }
            | Command::ClearModelLanguage { .. }) => self.handle_model_language(cmd).await,
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
