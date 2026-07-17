// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::transcribe::Transcribe;
use log::{error, info, warn};
use super_stt_shared::models::protocol::{DaemonResponse, DaemonStatusEvent, ErrorCode};

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

        // No model is loaded → nothing to reload. Record the preference so the
        // next model load picks it up, and return. This makes the GPU toggle
        // usable in the active-backend card before a model has been selected.
        if self.model.read().await.is_none() {
            return self.update_device_preference_only(&device).await;
        }

        // Get context for the device switch. The model can be unloaded
        // concurrently between the `is_none()` check above and this read — treat
        // "gone" as "nothing to reload" and just record the preference.
        let Some((current_preferred, model_to_reload, provider, source, is_online)) =
            self.get_device_switch_context(&device).await
        else {
            return self.update_device_preference_only(&device).await;
        };

        // Online models don't use local GPU — just update the preference; no
        // reload is needed since the model runs on a remote service.
        if is_online {
            info!("Current model is online, updating device preference only");
            return self.update_device_preference_only(&device).await;
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
            result = self.load_model_with_target_device(&model_to_reload, &provider, &source, &device) => {
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
                    &provider,
                    &source,
                    &current_preferred,
                )
                .await
            }
            Err(e) => {
                self.handle_device_switch_failure(
                    e,
                    &device,
                    &model_to_reload,
                    &provider,
                    &source,
                    &current_preferred,
                )
                .await
            }
        }
    }

    /// Record a new device preference without reloading anything — used for
    /// the idle case (no model loaded) and for the online-model case (no
    /// local device anyway). Updates the runtime locks + persisted config,
    /// then returns a 200-shaped response carrying the new device.
    async fn update_device_preference_only(&self, device: &str) -> DaemonResponse {
        *self.preferred_device.write().await = device.to_string();
        *self.actual_device.write().await = device.to_string();
        {
            let mut config = self.config.write().await;
            config.update_preferred_device(device.to_string());
        }
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after device preference update: {e}");
        }
        info!("Device preference updated to {device} (no model loaded — nothing to reload)");
        DaemonResponse::success()
            .with_device(device.to_string())
            .with_message(format!(
                "Device preference set to {device}. The next model load will use it."
            ))
    }

    /// Validate device switch request and return early response if validation fails
    async fn validate_device_switch_request(&self, device: &str) -> Option<DaemonResponse> {
        // Validate device parameter. Emit the documented `400 invalid_device`
        // code so clients can distinguish a bad request from a server failure
        // (an uncoded error maps to 500) — audit 2 Tier 2 #7.
        if device != "cpu" && device != "cuda" {
            warn!("Invalid device specified: {device}");
            return Some(DaemonResponse::error_with_code(
                ErrorCode::InvalidDevice,
                &format!("Invalid device '{device}'. Must be 'cpu' or 'cuda'"),
            ));
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

        // Prevent device switching during active recording.
        if let Some(resp) = self.guard_model_mutation("switch devices").await {
            warn!("Device switch rejected - recording in progress");
            return Some(resp);
        }

        None
    }

    /// Get context needed for device switch
    async fn get_device_switch_context(
        &self,
        _device: &str,
    ) -> Option<(
        String,
        String,
        super_stt_shared::models::provider::Provider,
        String,
        bool,
    )> {
        // Read the model that needs to be reloaded. It was present when the
        // caller checked, but the lock is released in between, so a concurrent
        // unload (a reload or a backend uninstall) can leave it `None` — return
        // that instead of panicking. Online-ness is read from the loaded model
        // (which implements `ModelInfo`) — the `provider` string no longer
        // encodes it.
        let (model_to_reload, provider, source, is_online) = {
            let guard = self.model.read().await;
            guard.as_ref().map(|loaded| {
                (
                    loaded.definition.name.clone(),
                    loaded.definition.provider.clone(),
                    loaded.definition.source.clone(),
                    loaded.definition.is_online(),
                )
            })
        }?;
        let current_preferred = self.preferred_device.read().await.clone();
        Some((
            current_preferred,
            model_to_reload,
            provider,
            source,
            is_online,
        ))
    }

    /// Prepare for device switch by broadcasting status and unloading current model
    async fn prepare_device_switch(&self, from_device: &str, to_device: &str, model: &str) {
        // Broadcast device switching status to settings subscribers
        self.events
            .publish_daemon_status(DaemonStatusEvent::SwitchingDevice {
                from_device: from_device.to_string(),
                target_device: to_device.to_string(),
                model: model.to_string(),
            });

        // Unload current model (free memory). Route through the shared graceful
        // path so the backend is `shutdown()` outside the write lock rather than
        // dropped under it — a subprocess `Drop` can block for seconds freeing
        // GPU memory, which would stall every reader (Tier 3 #2).
        self.unload_current_model().await;
    }

    /// Handle successful device switch
    async fn handle_device_switch_success(
        &self,
        model_instance: Box<dyn Transcribe>,
        device: &str,
        model_to_reload: &str,
        provider: &super_stt_shared::models::provider::Provider,
        source: &str,
        previous_device: &str,
    ) -> DaemonResponse {
        // Store the reloaded model. The backend serving it can be uninstalled
        // concurrently with the switch, so `resolve_definition` may now return
        // `None` — fail the request gracefully (leaving the daemon idle) rather
        // than panicking on the capture thread.
        let Some(definition) = self
            .resolve_definition(model_to_reload, provider, source)
            .await
        else {
            error!(
                "Backend serving {model_to_reload} ({source}) disappeared during the device \
                 switch; cannot finalize the reloaded model — leaving the daemon idle"
            );
            return DaemonResponse::error(&format!(
                "Model {model_to_reload} is no longer available (its backend may have been \
                 uninstalled during the device switch)"
            ));
        };
        let actual_device = self.finalize_loaded_model(definition, model_instance).await;

        // Update the preferred device after successful reload (the actual
        // device was already recorded by `finalize_loaded_model`).
        {
            let mut w = self.preferred_device.write().await;
            *w = device.to_string();
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
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: true,
            model_name: Some(model_to_reload.to_string()),
            actual_device: Some(actual_device.clone()),
            preferred_device: Some(device.to_string()),
        });

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
        provider: &super_stt_shared::models::provider::Provider,
        source: &str,
        previous_device: &str,
    ) -> DaemonResponse {
        error!("Failed to reload model on new device: {error}");

        // Broadcast error status
        self.events
            .publish_daemon_status(DaemonStatusEvent::DeviceSwitchError {
                error: error.to_string(),
                failed_device: device.to_string(),
                model: model_to_reload.to_string(),
            });

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
                let Some(definition) = self
                    .resolve_definition(model_to_reload, provider, source)
                    .await
                else {
                    error!(
                        "Backend serving {model_to_reload} ({source}) disappeared during \
                         device-switch recovery; cannot finalize — leaving the daemon idle"
                    );
                    return DaemonResponse::error(&format!(
                        "Device switch failed: {error}. Recovery could not finalize because \
                         model {model_to_reload} is no longer available."
                    ));
                };
                // Install the recovered model and record its actual device.
                let recovery_actual_device =
                    self.finalize_loaded_model(definition, model_instance).await;
                {
                    let mut w = self.preferred_device.write().await;
                    *w = previous_device.to_string();
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
                self.events.publish_daemon_status(DaemonStatusEvent::Ready {
                    model_loaded: true,
                    model_name: Some(model_to_reload.to_string()),
                    actual_device: Some(recovery_actual_device.clone()),
                    preferred_device: Some(previous_device.to_string()),
                });

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

        // The daemon offers both device preferences; whether CUDA is actually
        // usable is decided by the GPU-resident backend at load time (it falls
        // back to CPU if not).
        let available_devices = vec!["cpu".to_string(), "cuda".to_string()];

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

    /// Read-only GPU inventory for `GET /gpu_info`. Hardware detection runs on a
    /// blocking thread (NVML / sysfs / `system_profiler`) so it never stalls the
    /// async runtime. Best-effort: an empty list when no GPU is found.
    pub async fn handle_get_gpu_info() -> DaemonResponse {
        let gpus = tokio::task::spawn_blocking(|| {
            gpu_probe::detect()
                .into_iter()
                .map(gpu_to_wire)
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        DaemonResponse::success().with_gpu_info(gpus)
    }
}

/// Map a [`gpu_probe::GpuInfo`] to the wire payload, normalizing the vendor to
/// its `snake_case` tag (`nvidia` / `amd` / `intel` / `apple` / `unknown`).
fn gpu_to_wire(gpu: gpu_probe::GpuInfo) -> super_stt_shared::models::protocol::GpuInfo {
    let vendor = match gpu.vendor {
        gpu_probe::Vendor::Nvidia => "nvidia",
        gpu_probe::Vendor::Amd => "amd",
        gpu_probe::Vendor::Intel => "intel",
        gpu_probe::Vendor::Apple => "apple",
        _ => "unknown",
    }
    .to_string();
    super_stt_shared::models::protocol::GpuInfo {
        name: gpu.name,
        vendor,
        total_bytes: gpu.total_bytes,
        free_bytes: gpu.free_bytes,
        used_bytes: gpu.used_bytes,
    }
}
