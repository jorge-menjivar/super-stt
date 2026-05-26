// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::transcribe::Transcribe;
use chrono::Utc;
use log::{error, info, warn};
use super_stt_shared::models::protocol::DaemonResponse;
use super_stt_shared::models::provider::Provider;

impl SuperSTTDaemon {
    /// Handle set device command - switch between CPU and CUDA
    pub async fn handle_set_device(&self, device: String) -> DaemonResponse {
        self.handle_set_device_impl(device).await
    }

    /// Internal implementation split from the public API for readability
    async fn handle_set_device_impl(&self, device: String) -> DaemonResponse {
        info!("Device switch requested: {device}");

        // Check if shutdown is in progress before starting device switch
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        if let Ok(()) = shutdown_rx.try_recv() {
            warn!("Device switch rejected - shutdown in progress");
            return DaemonResponse::error("Device switch rejected due to shutdown in progress");
        }

        // Perform all validation checks
        if let Some(early_return) = self.validate_device_switch_request(&device).await {
            return early_return;
        }

        // Get context for the device switch
        let (current_preferred, model_to_reload, provider, source) =
            self.get_device_switch_context(&device).await;

        // Online models don't use local GPU — just update the preference
        if matches!(provider, Provider::Online(_)) {
            info!("Current model uses online provider, updating device preference only");
            *self.preferred_device.write().await = device.clone();
            {
                let mut config = self.config.write().await;
                config.update_preferred_device(device.clone());
            }
            let _ = self.persist_config().await;
            return DaemonResponse::success()
                .with_device(device)
                .with_message("Device preference updated (online model unaffected)".to_string());
        }

        info!(
            "Starting device switch from {current_preferred} to {device} (will reload model: {model_to_reload})"
        );

        // Update actual_device immediately to match preferred_device during switch
        // This prevents get_device from returning the old device during the switch
        {
            let mut w = self.actual_device.write().await;
            w.clone_from(&device);
        }

        // Broadcast device switching status and unload current model
        self.prepare_device_switch(&current_preferred, &device, &model_to_reload)
            .await;

        // Try to reload model with the requested device, but cancel if shutdown occurs
        let load_result = tokio::select! {
            result = self.load_model_with_target_device(&model_to_reload, provider, source, &device) => {
                result
            }
            _ = shutdown_rx.recv() => {
                warn!("Device switch cancelled due to shutdown");
                return DaemonResponse::error("Device switch cancelled due to shutdown");
            }
        };

        match load_result {
            Ok(model_instance) => {
                self.handle_device_switch_success(
                    model_instance,
                    &device,
                    &model_to_reload,
                    provider,
                    source,
                    &current_preferred,
                )
                .await
            }
            Err(e) => {
                self.handle_device_switch_failure(
                    e,
                    &device,
                    &model_to_reload,
                    provider,
                    source,
                    &current_preferred,
                )
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
        let model_loaded = self.model.read().await.is_some();

        if !model_loaded {
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
    ) -> (
        String,
        String,
        super_stt_shared::models::provider::Provider,
        super_stt_shared::models::registry::SourceKind,
    ) {
        // Get the model that needs to be reloaded (validated to exist already)
        let (model_to_reload, provider, source) = {
            let guard = self.model.read().await;
            guard
                .as_ref()
                .map(|loaded| {
                    (
                        loaded.definition.name.to_string(),
                        loaded.definition.provider,
                        loaded.definition.source.kind(),
                    )
                })
                .expect("Model existence already validated")
        };
        let current_preferred = self.preferred_device.read().await.clone();
        (current_preferred, model_to_reload, provider, source)
    }

    /// Prepare for device switch by broadcasting status and unloading current model
    async fn prepare_device_switch(&self, from_device: &str, to_device: &str, model: &str) {
        // Broadcast device switching status to settings subscribers
        self.events
            .publish_daemon_status_changed(serde_json::json!({
                "status": "switching_device",
                "from_device": from_device,
                "to_device": to_device,
                "model": model,
                "timestamp": Utc::now().to_rfc3339(),
            }));

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
        model_instance: Box<dyn Transcribe>,
        device: &str,
        model_to_reload: &str,
        provider: super_stt_shared::models::provider::Provider,
        source: super_stt_shared::models::registry::SourceKind,
        previous_device: &str,
    ) -> DaemonResponse {
        let actual_device = {
            let actual_device_str = match model_instance.device() {
                candle_core::Device::Cpu => "cpu",
                candle_core::Device::Cuda(_) => "cuda",
                candle_core::Device::Metal(_) => "metal",
            };
            actual_device_str.to_string()
        };

        // Store the reloaded model
        let definition = self
            .resolve_definition(model_to_reload, provider, source)
            .await
            .expect("device-switched model resolved before reload");
        *self.model.write().await = Some(crate::daemon::types::LoadedModel {
            definition,
            instance: model_instance,
        });

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
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after device switch: {e}");
        }

        let success_message = if actual_device != device && device == "cuda" {
            "Device switch requested to CUDA, but fell back to CPU due to CUDA unavailability"
                .to_string()
        } else {
            format!("Successfully switched to {actual_device} device")
        };

        info!("Device switch completed: {previous_device} -> {device} (actual: {actual_device})");

        // Broadcast ready status with new device
        self.events
            .publish_daemon_status_changed(serde_json::json!({
                "status": "ready",
                "model_loaded": true,
                "preferred_device": device,
                "actual_device": actual_device,
                "model_name": model_to_reload,
                "timestamp": Utc::now().to_rfc3339(),
            }));

        DaemonResponse::success()
            .with_device(actual_device)
            .with_message(success_message)
    }

    /// Handle failed device switch with recovery attempt
    async fn handle_device_switch_failure(
        &self,
        error: anyhow::Error,
        device: &str,
        model_to_reload: &str,
        provider: super_stt_shared::models::provider::Provider,
        source: super_stt_shared::models::registry::SourceKind,
        previous_device: &str,
    ) -> DaemonResponse {
        error!("Failed to reload model on new device: {error}");

        // Broadcast error status
        self.events
            .publish_daemon_status_changed(serde_json::json!({
                "status": "device_switch_error",
                "error": error.to_string(),
                "failed_device": device,
                "model": model_to_reload,
                "timestamp": Utc::now().to_rfc3339(),
            }));

        // Check if shutdown is in progress before attempting recovery
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        if let Ok(()) = shutdown_rx.try_recv() {
            warn!("Shutdown in progress, skipping device switch recovery");
            return DaemonResponse::error(&format!(
                "Device switch failed: {error}. Recovery skipped due to shutdown."
            ));
        }

        // Try to recover by reverting to previous device
        warn!("Attempting to recover by reverting to previous device: {previous_device}");

        match self
            .load_model_with_target_device(model_to_reload, provider, source, previous_device)
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

                let definition = self
                    .resolve_definition(model_to_reload, provider, source)
                    .await
                    .expect("recovery model resolved before reload");
                *self.model.write().await = Some(crate::daemon::types::LoadedModel {
                    definition,
                    instance: model_instance,
                });
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
                if let Err(e) = self.persist_config().await {
                    warn!("Failed to persist config after device recovery: {e}");
                }

                warn!(
                    "Recovery successful - reverted to previous device: {previous_device} (actual: {recovery_actual_device})"
                );

                // Broadcast ready status after successful recovery
                self.events
                    .publish_daemon_status_changed(serde_json::json!({
                        "status": "ready",
                        "model_loaded": true,
                        "preferred_device": previous_device,
                        "actual_device": recovery_actual_device,
                        "model_name": model_to_reload,
                        "timestamp": Utc::now().to_rfc3339(),
                    }));

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

        let mut response = DaemonResponse::success()
            .with_device(actual_device)
            .with_available_devices(available_devices)
            .with_message(message);

        // Include GPU memory info when CUDA is available
        match Self::get_gpu_memory_info() {
            Ok((free, total)) => {
                response = response
                    .with_gpu_free_memory(free)
                    .with_gpu_total_memory(total);
            }
            Err(e) => {
                info!("GPU memory query unavailable: {e}");
            }
        }

        response
    }

    #[cfg(feature = "cuda")]
    fn get_gpu_memory_info() -> Result<(u64, u64), anyhow::Error> {
        use candle_core::cuda_backend::cudarc::driver::{result, safe::CudaContext};
        let _ctx =
            CudaContext::new(0).map_err(|e| anyhow::anyhow!("CUDA context init failed: {e}"))?;
        let (free, total) =
            result::mem_get_info().map_err(|e| anyhow::anyhow!("CUDA mem_get_info: {e}"))?;
        Ok((free as u64, total as u64))
    }

    #[cfg(not(feature = "cuda"))]
    fn get_gpu_memory_info() -> Result<(u64, u64), anyhow::Error> {
        Err(anyhow::anyhow!("CUDA not available"))
    }
}
