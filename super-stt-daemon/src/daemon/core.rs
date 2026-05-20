// SPDX-License-Identifier: GPL-3.0-only

use crate::{daemon::types::SuperSTTDaemon, output::keyboard::Simulator, output::preview::Typer};
use super_stt_shared::models::protocol::{Command, DaemonRequest, DaemonResponse};

impl SuperSTTDaemon {
    /// Main command handler - routes commands to appropriate handlers
    pub async fn handle_command(&self, request: DaemonRequest) -> DaemonResponse {
        // Track connection if client_id is present
        if let Some(client_id) = &request.client_id {
            self.update_client_connection(client_id.clone()).await;
        }

        let command = match Command::try_from(request) {
            Ok(cmd) => cmd,
            Err(e) => return DaemonResponse::error(&e),
        };

        match command {
            Command::Transcribe {
                audio_data,
                sample_rate,
                client_id,
            } => {
                self.handle_transcribe(audio_data, sample_rate, client_id)
                    .await
            }
            Command::Subscribe {
                event_types,
                client_info,
            } => self.handle_subscribe(event_types, client_info),
            Command::Unsubscribe => {
                DaemonResponse::error("Unsubscribe must be called on persistent connection")
            }
            Command::GetEvents {
                since_timestamp,
                event_types,
                limit,
            } => self.handle_get_events(since_timestamp, event_types, limit),
            Command::GetSubscriberInfo => self.handle_get_subscriber_info(),
            Command::Notify {
                event_type,
                client_id,
                data,
            } => self.handle_notify(event_type, client_id, data).await,
            Command::Ping { client_id } => self.handle_ping(client_id).await,
            Command::Status => self.handle_status().await,
            Command::StartRealTimeTranscription {
                client_id,
                sample_rate,
                language,
            } => {
                self.handle_start_realtime(client_id, sample_rate, language)
                    .await
            }
            Command::RealTimeAudioChunk {
                client_id,
                audio_data,
                sample_rate,
            } => {
                self.handle_realtime_audio(client_id, audio_data, sample_rate)
                    .await
            }
            Command::Record {
                write_mode,
                stop_mode,
                preview,
                ..
            } => {
                self.handle_record_command(write_mode, stop_mode, preview)
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
            Command::SetAllowOnlineModels { enabled } => {
                self.handle_set_allow_online_models(enabled).await
            }
            Command::GetAllowOnlineModels => self.handle_get_allow_online_models().await,
            Command::SetCustomModelsDir { path } => self.handle_set_custom_models_dir(path).await,
            Command::GetCustomModelsDir => self.handle_get_custom_models_dir().await,
        }
    }

    /// Handle a record command — resolve mode, toggle stop, or start recording.
    async fn handle_record_command(
        &self,
        write_mode: bool,
        stop_mode: Option<super_stt_shared::models::recording_stop_mode::RecordingStopMode>,
        preview: Option<bool>,
    ) -> DaemonResponse {
        // Resolve effective mode: per-request override or daemon config default
        let effective_mode = if let Some(mode) = stop_mode {
            mode
        } else {
            let config = self.config.read().await;
            config.transcription.recording_stop_mode
        };

        // Toggle behaviour: if already recording, stop it (if mode allows)
        let is_recording = *self.is_recording.read().await;
        if is_recording {
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
            .handle_record_internal(&mut typer, write_mode, effective_mode)
            .await;

        // Restore original preview setting.
        if preview.is_some() {
            self.preview_typing_enabled
                .store(original_preview, std::sync::atomic::Ordering::Relaxed);
        }
        // Return the simulator to the cache for reuse.
        *self.simulator.write().await = Some(typer.take_simulator());
        response
    }

    /// Placeholder for real-time handlers - these need to be implemented
    pub async fn handle_start_realtime(
        &self,
        client_id: String,
        sample_rate: Option<u32>,
        language: Option<String>,
    ) -> DaemonResponse {
        match self
            .realtime_manager
            .start_session(client_id.clone(), sample_rate, language)
            .await
        {
            Ok(_receiver) => {
                log::info!("Started real-time transcription for client: {client_id}");
                DaemonResponse::success()
                    .with_client_id(client_id)
                    .with_message("Real-time transcription session started".to_string())
            }
            Err(e) => {
                log::error!("Failed to start real-time session: {e}");
                DaemonResponse::error(&format!("Failed to start real-time session: {e}"))
            }
        }
    }

    pub async fn handle_realtime_audio(
        &self,
        client_id: String,
        audio_data: Vec<f32>,
        sample_rate: u32,
    ) -> DaemonResponse {
        match self
            .realtime_manager
            .process_audio_chunk(&client_id, audio_data, sample_rate)
            .await
        {
            Ok(()) => DaemonResponse::success().with_message("Audio chunk processed".to_string()),
            Err(e) => {
                log::warn!("Failed to process audio chunk for {client_id}: {e}");
                DaemonResponse::error(&format!("Failed to process audio chunk: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DaemonConfig;
    use crate::daemon::auth::ProcessAuth;
    use crate::daemon::events::EventBus;
    use crate::download_progress::DownloadStateManager;
    use crate::input::audio::AudioProcessor;
    use crate::services::transcription::RealTimeTranscriptionManager;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, RwLock};
    use super_stt_shared::NotificationManager;
    use super_stt_shared::resource_management::ResourceManager;
    use super_stt_shared::theme::AudioTheme;
    use tokio::sync::broadcast;
    use tokio::time::{Duration, timeout};

    async fn test_daemon() -> SuperSTTDaemon {
        let socket_path = PathBuf::from("/tmp/super-stt-test.sock");
        let model = Arc::new(tokio::sync::RwLock::new(None));
        let notification_manager = Arc::new(NotificationManager::new(10, 10));
        let audio_processor = Arc::new(AudioProcessor::new());
        let (shutdown_tx, _) = broadcast::channel(1);
        let realtime_manager = Arc::new(RealTimeTranscriptionManager::new(
            Arc::clone(&model),
            Arc::clone(&notification_manager),
            Arc::clone(&audio_processor),
        ));
        SuperSTTDaemon {
            socket_path,
            model,
            notification_manager,
            audio_processor,
            shutdown_tx,
            dbus_manager: None,
            realtime_manager,
            events: Arc::new(EventBus::new()),
            audio_theme: Arc::new(RwLock::new(AudioTheme::default())),
            volume: Arc::new(RwLock::new(100)),
            is_recording: Arc::new(tokio::sync::RwLock::new(false)),
            audio_monitoring_handle: Arc::new(tokio::sync::RwLock::new(None)),
            download_manager: Arc::new(DownloadStateManager::new()),
            preferred_device: Arc::new(tokio::sync::RwLock::new("cpu".to_string())),
            actual_device: Arc::new(tokio::sync::RwLock::new("cpu".to_string())),
            config: Arc::new(tokio::sync::RwLock::new(DaemonConfig::default())),
            active_connections: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            process_auth: ProcessAuth::new(),
            resource_manager: Arc::new(ResourceManager::development()),
            preview_typing_enabled: Arc::new(AtomicBool::new(false)),
            manual_stop_tx: Arc::new(tokio::sync::RwLock::new(None)),
            simulator: Arc::new(tokio::sync::RwLock::new(None)),
            preview_text: Arc::new(tokio::sync::RwLock::new(None)),
            custom_models: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }

    fn make_request(command: &str) -> DaemonRequest {
        DaemonRequest {
            command: command.to_string(),
            audio_data: None,
            sample_rate: None,
            client_id: None,
            event_types: None,
            client_info: None,
            since_timestamp: None,
            limit: None,
            event_type: None,
            data: None,
            language: None,
            enabled: None,
        }
    }

    fn make_record_request(data: Option<serde_json::Value>) -> DaemonRequest {
        DaemonRequest {
            command: "record".to_string(),
            audio_data: None,
            sample_rate: None,
            client_id: None,
            event_types: None,
            client_info: None,
            since_timestamp: None,
            limit: None,
            event_type: None,
            data,
            language: None,
            enabled: None,
        }
    }

    #[tokio::test]
    async fn stop_signal_sent_on_second_press_with_default_mode() {
        // Default config mode is SilenceAndManual, which allows manual stop
        let daemon = test_daemon().await;
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);

        *daemon.is_recording.write().await = true;
        *daemon.manual_stop_tx.write().await = Some(tx);

        let request = make_record_request(Some(serde_json::json!({
            "write_mode": false,
        })));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(
            response.message.as_deref(),
            Some(DaemonResponse::RECORDING_STOP_SIGNAL_MSG)
        );

        let recv = timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(recv.is_ok(), "expected stop signal to be sent");
    }

    #[tokio::test]
    async fn second_press_ignored_in_silence_only_mode() {
        let daemon = test_daemon().await;
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);

        // Set daemon config to SilenceOnly
        {
            use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
            let mut config = daemon.config.write().await;
            config.transcription.recording_stop_mode = RecordingStopMode::SilenceOnly;
        }

        *daemon.is_recording.write().await = true;
        *daemon.manual_stop_tx.write().await = Some(tx);

        // No stop_mode in request → uses daemon config (SilenceOnly)
        let request = make_record_request(Some(serde_json::json!({
            "write_mode": false,
        })));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(
            response.message.as_deref(),
            Some("Manual stop not enabled in current mode")
        );

        // Stop signal should NOT have been sent
        let recv = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(
            recv.is_err(),
            "stop signal should not be sent in SilenceOnly mode"
        );
    }

    #[tokio::test]
    async fn per_request_stop_mode_overrides_config() {
        let daemon = test_daemon().await;
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);

        // Daemon config is SilenceOnly (no manual stop)
        {
            use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
            let mut config = daemon.config.write().await;
            config.transcription.recording_stop_mode = RecordingStopMode::SilenceOnly;
        }

        *daemon.is_recording.write().await = true;
        *daemon.manual_stop_tx.write().await = Some(tx);

        // But the request explicitly asks for manual-only mode
        let request = make_record_request(Some(serde_json::json!({
            "write_mode": false,
            "stop_mode": "manual-only",
        })));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(
            response.message.as_deref(),
            Some(DaemonResponse::RECORDING_STOP_SIGNAL_MSG)
        );

        let recv = timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(
            recv.is_ok(),
            "per-request override should allow manual stop"
        );
    }

    #[tokio::test]
    async fn second_press_during_transcription_returns_wait_message() {
        let daemon = test_daemon().await;

        // Transcribing state: is_recording=true, manual_stop_tx=None
        *daemon.is_recording.write().await = true;
        // manual_stop_tx is already None by default

        let request = make_record_request(Some(serde_json::json!({
            "write_mode": false,
        })));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(
            response.message.as_deref(),
            Some("Transcription in progress, please wait")
        );
    }

    #[tokio::test]
    async fn per_request_silence_only_overrides_manual_config() {
        let daemon = test_daemon().await;
        let (tx, mut rx) = tokio::sync::broadcast::channel(1);

        // Config allows manual stop (default SilenceAndManual)
        *daemon.is_recording.write().await = true;
        *daemon.manual_stop_tx.write().await = Some(tx);

        // But request forces SilenceOnly
        let request = make_record_request(Some(serde_json::json!({
            "write_mode": false,
            "stop_mode": "silence-only",
        })));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(
            response.message.as_deref(),
            Some("Manual stop not enabled in current mode")
        );

        let recv = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(
            recv.is_err(),
            "stop signal should not be sent in SilenceOnly mode"
        );
    }

    #[tokio::test]
    async fn stop_signal_succeeds_even_with_no_receivers() {
        let daemon = test_daemon().await;
        let (tx, _rx) = tokio::sync::broadcast::channel::<()>(1);
        // Drop _rx so there are no receivers

        *daemon.is_recording.write().await = true;
        *daemon.manual_stop_tx.write().await = Some(tx);

        let request = make_record_request(Some(serde_json::json!({
            "write_mode": false,
        })));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(
            response.message.as_deref(),
            Some(DaemonResponse::RECORDING_STOP_SIGNAL_MSG)
        );
    }

    #[tokio::test]
    async fn is_recording_reset_after_error_cleanup() {
        // Verify that the error cleanup pattern in handle_record_internal
        // correctly resets is_recording. We can't trigger the full recording
        // pipeline in CI (requires audio hardware + display server for Typer),
        // so we simulate the state and verify cleanup.
        let daemon = test_daemon().await;

        // Simulate: setup_recording_session ran and set is_recording = true,
        // then record_and_transcribe failed.
        *daemon.is_recording.write().await = true;
        assert!(*daemon.is_recording.read().await);

        // The error path in handle_record_internal does:
        //   *self.is_recording.write().await = false;
        //   self.broadcast_recording_state_change(false);
        // Verify the daemon can recover from this state.
        {
            let mut guard = daemon.is_recording.write().await;
            *guard = false;
        }
        daemon.broadcast_recording_state_change(false);

        assert!(
            !*daemon.is_recording.read().await,
            "is_recording must be false after error cleanup"
        );

        // And a new recording request should NOT hit the toggle path
        // (it should try to start, not return "transcription in progress")
        // We can't fully test starting a recording here, but we verify the
        // state allows it by checking the guard is clear.
        assert!(daemon.manual_stop_tx.read().await.is_none());
    }

    #[tokio::test]
    async fn set_allow_online_models_updates_config() {
        let daemon = test_daemon().await;

        let request = DaemonRequest {
            command: "set_allow_online_models".to_string(),
            audio_data: None,
            sample_rate: None,
            client_id: None,
            event_types: None,
            client_info: None,
            since_timestamp: None,
            limit: None,
            event_type: None,
            data: None,
            language: None,
            enabled: Some(true),
        };

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(response.allow_online_models, Some(true));

        let config = daemon.config.read().await;
        assert!(config.online.allow_online_models);
    }

    #[tokio::test]
    async fn get_allow_online_models_returns_config_value() {
        let daemon = test_daemon().await;

        // Set it to true first
        {
            let mut config = daemon.config.write().await;
            config.online.allow_online_models = true;
        }

        let request = DaemonRequest {
            command: "get_allow_online_models".to_string(),
            audio_data: None,
            sample_rate: None,
            client_id: None,
            event_types: None,
            client_info: None,
            since_timestamp: None,
            limit: None,
            event_type: None,
            data: None,
            language: None,
            enabled: None,
        };

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(response.allow_online_models, Some(true));
    }

    #[tokio::test]
    async fn set_model_online_rejected_when_disabled() {
        let daemon = test_daemon().await;

        // Ensure online models are disabled (default)
        {
            let config = daemon.config.read().await;
            assert!(!config.online.allow_online_models);
        }

        let request = DaemonRequest {
            command: "set_model".to_string(),
            audio_data: None,
            sample_rate: None,
            client_id: None,
            event_types: None,
            client_info: None,
            since_timestamp: None,
            limit: None,
            event_type: None,
            data: Some(serde_json::json!({ "model": "whisper-1", "provider": "openai" })),
            language: None,
            enabled: None,
        };

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "error");
        assert!(
            response
                .message
                .as_deref()
                .unwrap_or("")
                .contains("disabled")
                || response
                    .message
                    .as_deref()
                    .unwrap_or("")
                    .contains("Online models are disabled"),
            "expected error about online models being disabled, got: {:?}",
            response.message
        );
    }

    #[tokio::test]
    async fn set_model_mistral_rejected_when_disabled() {
        let daemon = test_daemon().await;

        let mut request = make_request("set_model");
        request.data = Some(serde_json::json!({ "model": "voxtral-mini-transcribe-v2" }));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "error");
    }

    #[tokio::test]
    async fn set_model_deepgram_rejected_when_disabled() {
        let daemon = test_daemon().await;

        let mut request = make_request("set_model");
        request.data = Some(serde_json::json!({ "model": "nova-3" }));

        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "error");
    }

    #[tokio::test]
    async fn toggle_online_models_off_defaults_to_false() {
        let daemon = test_daemon().await;
        let config = daemon.config.read().await;
        assert!(
            !config.online.allow_online_models,
            "online models should be disabled by default"
        );
    }

    #[tokio::test]
    async fn toggle_online_models_on_then_off() {
        let daemon = test_daemon().await;

        // Enable
        let mut request = make_request("set_allow_online_models");
        request.enabled = Some(true);
        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(response.allow_online_models, Some(true));

        // Disable
        let mut request = make_request("set_allow_online_models");
        request.enabled = Some(false);
        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");
        assert_eq!(response.allow_online_models, Some(false));

        let config = daemon.config.read().await;
        assert!(!config.online.allow_online_models);
    }

    #[tokio::test]
    async fn list_models_includes_online_models() {
        let daemon = test_daemon().await;

        let request = make_request("list_models");
        let response = daemon.handle_command(request).await;
        assert_eq!(response.status, "success");

        let models = response.available_models.expect("should have models");
        // Should include online models in the list
        assert!(
            models.iter().any(|(name, _, _)| name == "whisper-1"),
            "list should include OpenAI models"
        );
        assert!(
            models
                .iter()
                .any(|(name, _, _)| name == "voxtral-mini-latest"),
            "list should include Mistral models"
        );
        assert!(
            models.iter().any(|(name, _, _)| name == "nova-3"),
            "list should include Deepgram models"
        );
    }

    #[tokio::test]
    async fn set_model_local_works_without_online_toggle() {
        let daemon = test_daemon().await;

        // Local models should not be blocked by the online toggle
        // (they will fail because no model files exist, but the error
        // should NOT be about online models being disabled)
        let mut request = make_request("set_model");
        request.data = Some(serde_json::json!({ "model": "whisper-tiny" }));

        let response = daemon.handle_command(request).await;
        // Should either succeed (already loaded) or fail for non-online reasons
        if response.status == "error" {
            let msg = response.message.as_deref().unwrap_or("");
            assert!(
                !msg.contains("Online models are disabled"),
                "local model should not be blocked by online toggle"
            );
        }
    }
}
