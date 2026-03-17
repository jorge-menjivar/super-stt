// SPDX-License-Identifier: GPL-3.0-only

use crate::{daemon::types::SuperSTTDaemon, output::preview::Typer};
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
            } => {
                // Resolve effective mode: per-request override or daemon config default
                let effective_mode = match stop_mode {
                    Some(mode) => mode,
                    None => {
                        let config = self.config.read().await;
                        config.transcription.recording_stop_mode
                    }
                };

                // Toggle behaviour: if already recording, stop it (if mode allows)
                let is_recording = *self.is_recording.read().await;
                if is_recording {
                    if !effective_mode.manual_stop_enabled() {
                        log::info!("Second press ignored: recording in SilenceOnly mode");
                        return DaemonResponse::success()
                            .with_message("Manual stop not enabled in current mode".to_string());
                    }
                    let guard = self.manual_stop_tx.read().await;
                    if let Some(tx) = guard.as_ref() {
                        let _ = tx.send(());
                        log::info!("🛑 Stop triggered via shortcut while recording");
                    } else {
                        log::warn!(
                            "Stop requested but no stop channel found (recording not ready or already finishing)"
                        );
                    }
                    return DaemonResponse::success()
                        .with_message(DaemonResponse::RECORDING_STOP_SIGNAL_MSG.to_string());
                }
                let mut typer = Typer::default();
                self.handle_record_internal(&mut typer, write_mode, effective_mode)
                    .await
            }
            Command::SetAudioTheme { theme } => self.handle_set_audio_theme(theme),
            Command::GetAudioTheme => self.handle_get_audio_theme(),
            Command::TestAudioTheme => self.handle_test_audio_theme().await,
            Command::SetModel { model } => self.handle_set_model(model).await,
            Command::GetModel => self.handle_get_model().await,
            Command::ListModels => self.handle_list_models(),
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
        }
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
    use crate::audio::streamer::UdpAudioStreamer;
    use crate::config::DaemonConfig;
    use crate::daemon::auth::ProcessAuth;
    use crate::download_progress::DownloadStateManager;
    use crate::input::audio::AudioProcessor;
    use crate::services::transcription::RealTimeTranscriptionManager;
    use super_stt_shared::NotificationManager;
    use super_stt_shared::resource_management::ResourceManager;
    use super_stt_shared::theme::AudioTheme;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, RwLock};
    use tokio::sync::broadcast;
    use tokio::time::{timeout, Duration};

    async fn test_daemon() -> SuperSTTDaemon {
        let socket_path = PathBuf::from("/tmp/super-stt-test.sock");
        let model = Arc::new(tokio::sync::RwLock::new(None));
        let model_type = Arc::new(tokio::sync::RwLock::new(None));
        let notification_manager = Arc::new(NotificationManager::new(10, 10));
        let audio_processor = Arc::new(AudioProcessor::new());
        let (shutdown_tx, _) = broadcast::channel(1);
        let realtime_manager = Arc::new(RealTimeTranscriptionManager::new(
            Arc::clone(&model),
            Arc::clone(&model_type),
            Arc::clone(&notification_manager),
            Arc::clone(&audio_processor),
        ));
        let udp_streamer = Arc::new(
            UdpAudioStreamer::new("127.0.0.1:0")
                .await
                .expect("udp streamer should bind"),
        );

        SuperSTTDaemon {
            socket_path,
            model,
            model_type,
            notification_manager,
            audio_processor,
            shutdown_tx,
            dbus_manager: None,
            realtime_manager,
            udp_streamer,
            audio_theme: Arc::new(RwLock::new(AudioTheme::default())),
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
        assert!(recv.is_err(), "stop signal should not be sent in SilenceOnly mode");
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
        assert!(recv.is_ok(), "per-request override should allow manual stop");
    }
}
