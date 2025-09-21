// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::{STTModelInstance, SuperSTTDaemon};
use chrono::Utc;
use log::{error, info, warn};
use super_stt_shared::models::protocol::DaemonResponse;

impl SuperSTTDaemon {
    /// Handle set device command - switch between CPU and CUDA
    pub async fn handle_set_device(&self, device: String) -> DaemonResponse {
        self.handle_set_device_impl(device).await
    }

    /// Internal implementation split from the public API for readability
    async fn handle_set_device_impl(&self, device: String) -> DaemonResponse {
        info!("Device switch requested: {device}");

        // Perform all validation checks
        if let Some(early_return) = self.validate_device_switch_request(&device).await {
            return early_return;
        }

        // Get context for the device switch
        let (current_preferred, model_to_reload) = self.get_device_switch_context(&device).await;

        info!(
            "Starting device switch from {current_preferred} to {device} (will reload model: {model_to_reload})"
        );

        // Update actual_device immediately to match preferred_device during switch
        // This prevents get_device from returning the old device during the switch
        {
            let mut w = self.actual_device.write().await;
            *w = device.to_string();
        }

        // Broadcast device switching status and unload current model
        self.prepare_device_switch(&current_preferred, &device, &model_to_reload)
            .await;

        // Try to reload model with the requested device
        match self
            .load_model_with_target_device(&model_to_reload, &device)
            .await
        {
            Ok(model_instance) => {
                self.handle_device_switch_success(
                    model_instance,
                    &device,
                    &model_to_reload,
                    &current_preferred,
                )
                .await
            }
            Err(e) => {
                self.handle_device_switch_failure(e, &device, &model_to_reload, &current_preferred)
                    .await
            }
        }
    }

    /// Validate device switch request and return early response if validation fails
    async fn validate_device_switch_request(&self, device: &str) -> Option<DaemonResponse> {
        // Validate device parameter
        if device != "cpu" && device != "cuda" {
            warn!("Invalid device specified: {device}");
            return Some(DaemonResponse::error(&format!(
                "Invalid device '{device}'. Must be 'cpu' or 'cuda'"
            )));
        }

        // Check current preferred and actual devices
        let current_preferred = self.preferred_device.read().await.clone();
        let current_actual = self.actual_device.read().await.clone();

        if current_preferred == device && current_actual == device {
            info!(
                "Device switch skipped - already using device: {device} (preferred: {current_preferred}, actual: {current_actual})"
            );
            return Some(
                DaemonResponse::success()
                    .with_device(current_actual.clone())
                    .with_message(format!("Already using device: {device}")),
            );
        } else if current_preferred == device && current_actual != device {
            info!(
                "Device preference is set to {device} but actual device is {current_actual} - forcing model reload"
            );
        }

        // Security check: prevent device switching during active recording
        {
            let is_recording_guard = self.is_recording.read().await;
            if *is_recording_guard {
                warn!("Device switch rejected - recording in progress");
                return Some(DaemonResponse::error(
                    "Cannot switch devices during active recording. Please wait for recording to complete.",
                ));
            }
        }

        // Security check: prevent device switching during real-time transcription
        let active_sessions = self.realtime_manager.get_active_sessions().await;
        if !active_sessions.is_empty() {
            warn!(
                "Device switch rejected - {} real-time transcription sessions active",
                active_sessions.len()
            );
            return Some(DaemonResponse::error(&format!(
                "Cannot switch devices during active real-time transcription sessions. {} active sessions: {}. Please stop all sessions first.",
                active_sessions.len(),
                active_sessions.join(", ")
            )));
        }

        // Check if a model is currently loaded - we'll need to reload it
        let current_model_type = {
            let model_type_guard = self.model_type.read().await;
            *model_type_guard
        };

        if current_model_type.is_none() {
            warn!("Device switch rejected - no model loaded");
            return Some(DaemonResponse::error(
                "Cannot switch devices when no model is loaded. Load a model first.",
            ));
        }

        None
    }

    /// Get context needed for device switch
    async fn get_device_switch_context(
        &self,
        _device: &str,
    ) -> (String, super_stt_shared::stt_model::STTModel) {
        // Get the model that needs to be reloaded (validated to exist already)
        let current_model_type = {
            let model_type_guard = self.model_type.read().await;
            *model_type_guard
        };

        let model_to_reload = current_model_type.expect("Model existence already validated");
        let current_preferred = self.preferred_device.read().await.clone();
        (current_preferred, model_to_reload)
    }

    /// Prepare for device switch by broadcasting status and unloading current model
    async fn prepare_device_switch(
        &self,
        from_device: &str,
        to_device: &str,
        model: &super_stt_shared::stt_model::STTModel,
    ) {
        // Broadcast device switching status
        if let Err(e) = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "switching_device",
                    "from_device": from_device,
                    "to_device": to_device,
                    "model": model.to_string(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast device switching status: {e}");
        }

        // Unload current model (free memory)
        {
            let mut model_guard = self.model.write().await;
            *model_guard = None;
            info!("Current model unloaded for device switch");
        }
    }

    /// Handle successful device switch
    async fn handle_device_switch_success(
        &self,
        model_instance: STTModelInstance,
        device: &str,
        model_to_reload: &super_stt_shared::stt_model::STTModel,
        previous_device: &str,
    ) -> DaemonResponse {
        let model_name = match &model_instance {
            STTModelInstance::Whisper(_) => "Whisper",
            STTModelInstance::Voxtral(_) => "Voxtral",
        };

        let actual_device = {
            let actual_device_str = match model_instance.device() {
                candle_core::Device::Cpu => "cpu",
                candle_core::Device::Cuda(_) => "cuda",
                candle_core::Device::Metal(_) => "metal",
            };
            actual_device_str.to_string()
        };

        // Store the reloaded model
        *self.model.write().await = Some(model_instance);

        // Update both preferred and actual device after successful reload
        {
            let mut w = self.preferred_device.write().await;
            *w = device.to_string();
        }
        {
            let mut w = self.actual_device.write().await;
            w.clone_from(&actual_device);
        }

        // Update the config with new device preference and save to disk
        {
            let mut config_guard = self.config.write().await;
            config_guard.update_preferred_device(device.to_string());
        }

        // Broadcast config change event
        if let Err(e) = self.broadcast_config_change().await {
            warn!("Failed to broadcast config change after device switch: {e}");
        }

        let success_message = if actual_device != device && device == "cuda" {
            "Device switch requested to CUDA, but fell back to CPU due to CUDA unavailability"
                .to_string()
        } else {
            format!("Successfully switched to {actual_device} device")
        };

        info!("Device switch completed: {previous_device} -> {device} (actual: {actual_device})");

        // Note: We don't broadcast "device_switched" here anymore because the app
        // treats it as completion. Instead, we'll only broadcast "ready" status
        // which properly indicates that the device switch AND model loading are complete.

        // Broadcast ready status with new device
        if let Err(e) = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "ready",
                    "model_loaded": true,
                    "preferred_device": device,
                    "actual_device": actual_device,
                    "model_type": model_name.to_lowercase(),
                    "model_name": model_to_reload.to_string(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await
        {
            warn!("Failed to broadcast ready status with new device: {e}");
        }

        DaemonResponse::success()
            .with_device(actual_device)
            .with_message(success_message)
    }

    /// Handle failed device switch with recovery attempt
    async fn handle_device_switch_failure(
        &self,
        error: anyhow::Error,
        device: &str,
        model_to_reload: &super_stt_shared::stt_model::STTModel,
        previous_device: &str,
    ) -> DaemonResponse {
        error!("Failed to reload model on new device: {error}");

        // Broadcast error status
        let _ = self
            .notification_manager
            .broadcast_event(
                "daemon_status_changed".to_string(),
                "daemon".to_string(),
                serde_json::json!({
                    "status": "device_switch_error",
                    "error": error.to_string(),
                    "failed_device": device,
                    "model": model_to_reload.to_string(),
                    "timestamp": Utc::now().to_rfc3339()
                }),
            )
            .await;

        // Try to recover by reverting to previous device
        warn!("Attempting to recover by reverting to previous device: {previous_device}");

        match self
            .load_model_with_target_device(model_to_reload, previous_device)
            .await
        {
            Ok(model_instance) => {
                // Update both preferred and actual device after successful recovery
                let recovery_actual_device = match model_instance.device() {
                    candle_core::Device::Cpu => "cpu",
                    candle_core::Device::Cuda(_) => "cuda",
                    candle_core::Device::Metal(_) => "metal",
                }
                .to_string();

                // Extract model type before moving the instance
                let model_type_name = match &model_instance {
                    STTModelInstance::Whisper(_) => "whisper",
                    STTModelInstance::Voxtral(_) => "voxtral",
                };

                *self.model.write().await = Some(model_instance);
                {
                    let mut w = self.preferred_device.write().await;
                    *w = previous_device.to_string();
                }
                {
                    let mut w = self.actual_device.write().await;
                    w.clone_from(&recovery_actual_device);
                }

                // Update the config to revert to previous device
                {
                    let mut config_guard = self.config.write().await;
                    config_guard.update_preferred_device(previous_device.to_string());
                }

                // Broadcast config change event for recovery
                if let Err(e) = self.broadcast_config_change().await {
                    warn!("Failed to broadcast config change after device recovery: {e}");
                }

                warn!(
                    "Recovery successful - reverted to previous device: {previous_device} (actual: {recovery_actual_device})"
                );

                // Broadcast ready status after successful recovery
                if let Err(e) = self
                    .notification_manager
                    .broadcast_event(
                        "daemon_status_changed".to_string(),
                        "daemon".to_string(),
                        serde_json::json!({
                            "status": "ready",
                            "model_loaded": true,
                            "preferred_device": previous_device,
                            "actual_device": recovery_actual_device,
                            "model_type": model_type_name,
                            "model_name": model_to_reload.to_string(),
                            "timestamp": Utc::now().to_rfc3339()
                        }),
                    )
                    .await
                {
                    warn!("Failed to broadcast ready status after recovery: {e}");
                }

                DaemonResponse::error(&format!(
                    "Failed to switch to device '{device}': {error}. Reverted to previous device '{recovery_actual_device}'."
                ))
            }
            Err(recovery_e) => {
                error!("Recovery failed: {recovery_e}");
                DaemonResponse::error(&format!(
                    "Device switch failed: {error}. Recovery also failed: {recovery_e}. Daemon is now in no-model state."
                ))
            }
        }
    }

    /// Handle get device command - return current device information
    pub async fn handle_get_device(&self) -> DaemonResponse {
        let preferred_device = self.preferred_device.read().await.clone();
        let actual_device = self.actual_device.read().await.clone();

        info!("Device status requested - preferred: {preferred_device}, actual: {actual_device}");

        // Determine available devices based on build features
        let available_devices = super_stt_shared::device_options!();

        let message = if preferred_device != actual_device && preferred_device == "cuda" {
            format!(
                "Preferred device: CUDA, Actual device: {actual_device} (CUDA unavailable or failed)"
            )
        } else {
            format!("Device: {actual_device} (preferred and actual match)")
        };

        DaemonResponse::success()
            .with_device(actual_device)
            .with_available_devices(available_devices)
            .with_message(message)
    }
}
