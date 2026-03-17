// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use chrono::Utc;
use log::{error, info, warn};
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use serde_json::Value;
use std::collections::HashMap;
use strum::VariantArray;
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::stt_model::STTModel;
use super_stt_shared::theme::AudioTheme;

impl SuperSTTDaemon {
    /// Handle ping command - test connectivity and connection status
    pub async fn handle_ping(&self, client_id: Option<String>) -> DaemonResponse {
        // Clean up old connections
        self.cleanup_old_connections().await;

        let mut response = DaemonResponse::success().with_message("pong".to_string());

        if let Some(client_id) = client_id {
            let connection_active = self.is_client_connection_active(&client_id).await;
            response = response.with_connection_active(connection_active);
        }

        response
    }

    /// Handle status command - return daemon and model status
    pub async fn handle_status(&self) -> DaemonResponse {
        let model_guard = self.model.read().await;
        let model_type_guard = self.model_type.read().await;

        let (device, model_loaded) = match model_guard.as_ref() {
            Some(model) => {
                let device_str = match model.device() {
                    candle_core::Device::Cpu => "cpu".to_string(),
                    candle_core::Device::Cuda(_) => "cuda".to_string(),
                    candle_core::Device::Metal(_) => "metal".to_string(),
                };
                (device_str, true)
            }
            None => ("unknown".to_string(), false),
        };

        let model = model_type_guard.as_ref();

        let notification_info = self.notification_manager.get_subscriber_info();

        let mut response = DaemonResponse::success()
            .with_device(device)
            .with_model_loaded(model_loaded)
            .with_notification_info(notification_info);

        if let Some(model) = model {
            response = response.with_current_model(*model);
        }

        response
    }

    /// Handle notify command - broadcast events to subscribers
    #[allow(clippy::cast_possible_truncation)]
    pub async fn handle_notify(
        &self,
        event_type: String,
        client_id: String,
        data: Value,
    ) -> DaemonResponse {
        // Emit D-Bus signals for listening events
        if let Some(ref dbus_manager) = self.dbus_manager {
            match event_type.as_str() {
                "listening_started" => {
                    use crate::services::dbus::ListeningEvent;
                    let event = ListeningEvent {
                        client_id: client_id.clone(),
                        timestamp: Utc::now().to_rfc3339(),
                        write_mode: data
                            .get("write_mode")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        timeout_seconds: data
                            .get("timeout_seconds")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        audio_level: data
                            .get("audio_level")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0) as f32,
                    };

                    if let Err(e) = dbus_manager.emit_listening_started(event).await {
                        warn!("Failed to emit D-Bus listening_started signal: {e}");
                    } else {
                        log::debug!(
                            "Emitted D-Bus listening_started signal for client: {client_id}"
                        );
                    }
                }
                "listening_stopped" => {
                    use crate::services::dbus::ListeningStoppedEvent;
                    let event = ListeningStoppedEvent {
                        client_id: client_id.clone(),
                        timestamp: Utc::now().to_rfc3339(),
                        transcription_success: data
                            .get("transcription_success")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        error: data
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    };

                    if let Err(e) = dbus_manager.emit_listening_stopped(event).await {
                        warn!("Failed to emit D-Bus listening_stopped signal: {e}");
                    } else {
                        log::debug!(
                            "Emitted D-Bus listening_stopped signal for client: {client_id}"
                        );
                    }
                }
                "audio_level" => {
                    use crate::services::dbus::AudioLevelEvent;
                    let event = AudioLevelEvent {
                        client_id: client_id.clone(),
                        timestamp: Utc::now().to_rfc3339(),
                        level: data.get("level").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                        is_speech: data
                            .get("is_speech")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    };

                    if let Err(e) = dbus_manager.emit_audio_level(event).await {
                        warn!("Failed to emit D-Bus audio_level signal: {e}");
                    } else {
                        log::debug!("Emitted D-Bus audio_level signal for client: {client_id}");
                    }
                }
                _ => {
                    // For other event types, just log
                    log::debug!(
                        "Received notification event: {event_type} from client: {client_id}"
                    );
                }
            }
        }

        // Continue with existing notification system
        match self
            .notification_manager
            .broadcast_event(event_type, client_id, data)
            .await
        {
            Ok(delivered) => DaemonResponse::success()
                .with_message(format!("Event broadcasted to {delivered} subscribers")),
            Err(e) => DaemonResponse::error(&format!("Failed to broadcast event: {e}")),
        }
    }

    /// Handle subscribe command - subscribe to event types
    #[must_use]
    pub fn handle_subscribe(
        &self,
        event_types: Vec<String>,
        client_info: HashMap<String, Value>,
    ) -> DaemonResponse {
        match self
            .notification_manager
            .subscribe(event_types.clone(), client_info)
        {
            Ok((client_id, _receiver)) => {
                info!("Client {client_id} subscribed to events: {event_types:?}");

                // Note: Audio monitoring will only start during recording sessions
                // No continuous monitoring for event subscriptions

                DaemonResponse::success()
                    .with_client_id(client_id)
                    .with_subscribed_to(event_types)
                    .with_total_subscribers(
                        u32::try_from(self.notification_manager.get_total_subscribers())
                            .unwrap_or(u32::MAX),
                    )
            }
            Err(e) => {
                warn!("Subscription failed: {e}");
                DaemonResponse::error(&e.to_string())
            }
        }
    }

    /// Handle get events command - retrieve recent events
    #[must_use]
    pub fn handle_get_events(
        &self,
        since_timestamp: Option<String>,
        event_types: Option<Vec<String>>,
        limit: u32,
    ) -> DaemonResponse {
        match self
            .notification_manager
            .get_recent_events(since_timestamp, event_types, limit)
        {
            Ok(events) => DaemonResponse::success().with_events(events),
            Err(e) => DaemonResponse::error(&e.to_string()),
        }
    }

    /// Handle get subscriber info command
    #[must_use]
    pub fn handle_get_subscriber_info(&self) -> DaemonResponse {
        let info = self.notification_manager.get_subscriber_info();
        DaemonResponse::success().with_notification_info(info)
    }

    /// Handle get config command - return current daemon configuration
    pub async fn handle_get_config(&self) -> DaemonResponse {
        let config = self.config.read().await;

        // Serialize the config to JSON Value for the response
        let config_json = match serde_json::to_value(&*config) {
            Ok(value) => value,
            Err(e) => {
                error!("Failed to serialize daemon config: {e}");
                return DaemonResponse::error(&format!("Failed to serialize config: {e}"));
            }
        };

        DaemonResponse::success()
            .with_daemon_config(config_json)
            .with_message("Daemon configuration retrieved successfully".to_string())
    }

    /// Handle list all available models command
    #[must_use]
    pub fn handle_list_models(&self) -> DaemonResponse {
        let available_models = STTModel::VARIANTS.to_vec();
        info!(
            "Available models requested, returning {} models",
            available_models.len()
        );

        DaemonResponse::success()
            .with_available_models(available_models)
            .with_message("Available models listed successfully".to_string())
    }

    /// Handle list audio themes command - return all available audio themes
    #[must_use]
    pub fn handle_list_audio_themes(&self) -> DaemonResponse {
        let available_themes = AudioTheme::all_themes();
        info!(
            "Available audio themes requested, returning {} themes",
            available_themes.len()
        );

        DaemonResponse::success()
            .with_available_audio_themes(available_themes)
            .with_message("Available audio themes listed successfully".to_string())
    }

    /// Handle set preview typing command - enable or disable preview typing
    #[must_use]
    pub async fn handle_set_preview_typing(&self, enabled: bool) -> DaemonResponse {
        // Update the in-memory setting
        self.preview_typing_enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);

        // Update the config directly (don't clone)
        {
            let mut config_guard = self.config.write().await;
            config_guard.transcription.preview_typing_enabled = enabled;
        }

        // Broadcast config change (this saves the config to disk)
        let broadcast_result = self.broadcast_config_change().await;

        match broadcast_result {
            Ok(()) => {
                info!(
                    "Preview typing {} and saved to config",
                    if enabled { "enabled" } else { "disabled" }
                );
                DaemonResponse::success()
                    .with_preview_typing_enabled(enabled)
                    .with_message(format!(
                        "Preview typing {} and saved",
                        if enabled { "enabled" } else { "disabled" }
                    ))
            }
            Err(e) => {
                warn!("Preview typing setting changed but failed to save to config: {e}");
                DaemonResponse::success()
                    .with_preview_typing_enabled(enabled)
                    .with_message(format!(
                        "Preview typing {} (config save failed: {e})",
                        if enabled { "enabled" } else { "disabled" }
                    ))
            }
        }
    }

    /// Handle get preview typing command - return current preview typing setting
    #[must_use]
    pub fn handle_get_preview_typing(&self) -> DaemonResponse {
        let enabled = self
            .preview_typing_enabled
            .load(std::sync::atomic::Ordering::Relaxed);

        DaemonResponse::success()
            .with_preview_typing_enabled(enabled)
            .with_message("Preview typing setting retrieved successfully".to_string())
    }

    /// Handle set recording stop mode command
    pub async fn handle_set_recording_stop_mode(
        &self,
        mode: RecordingStopMode,
    ) -> DaemonResponse {
        {
            let mut config_guard = self.config.write().await;
            config_guard.transcription.recording_stop_mode = mode;
        }

        let broadcast_result = self.broadcast_config_change().await;

        match broadcast_result {
            Ok(()) => {
                info!("Recording stop mode set to {mode} and saved to config");
                DaemonResponse::success()
                    .with_recording_stop_mode(mode.to_string())
                    .with_message(format!("Recording stop mode set to {mode}"))
            }
            Err(e) => {
                warn!("Recording stop mode changed but failed to save: {e}");
                DaemonResponse::success()
                    .with_recording_stop_mode(mode.to_string())
                    .with_message(format!("Recording stop mode set to {mode} (save failed: {e})"))
            }
        }
    }

    /// Handle get recording stop mode command
    pub async fn handle_get_recording_stop_mode(&self) -> DaemonResponse {
        let config = self.config.read().await;
        let mode = config.transcription.recording_stop_mode;
        DaemonResponse::success()
            .with_recording_stop_mode(mode.to_string())
    }

    /// Handle cancel download command
    #[must_use]
    pub fn handle_cancel_download(&self) -> DaemonResponse {
        match self.download_manager.cancel_current_download() {
            Ok(()) => {
                info!("Download cancellation requested");
                DaemonResponse::success()
                    .with_message("Download cancelled successfully".to_string())
            }
            Err(e) => {
                warn!("Failed to cancel download: {e}");
                DaemonResponse::error(&e)
            }
        }
    }

    /// Handle get download status command
    #[must_use]
    pub fn handle_get_download_status(&self) -> DaemonResponse {
        if let Some(tracker) = self.download_manager.get_current_download() {
            let progress = tracker.get_progress();
            DaemonResponse::success().with_download_progress(progress)
        } else {
            DaemonResponse::success().with_message("No download in progress".to_string())
        }
    }
}
