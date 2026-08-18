// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::stt_models::transcribe::Transcribe;
use log::{error, info, warn};
use super_stt_shared::models::protocol::{DaemonResponse, DaemonStatusEvent, ErrorCode};

impl SuperSTTDaemon {
    /// Handle set device command - switch between CPU and GPU
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

        // Validate and normalize (`cuda`/`metal` → `gpu`) in one step, so
        // everything downstream stores/threads `cpu`/`gpu` rather than the raw
        // input the client sent.
        let device = match self.validate_device_switch_request(&device).await {
            Ok(device) => device,
            Err(early_return) => return early_return,
        };

        // No model is loaded → nothing to reload. Record the preference so the
        // next model load picks it up, and return. This makes the GPU toggle
        // usable in the active-backend card before a model has been selected.
        if self.model.read().await.is_none() {
            return self.update_device_preference_only(&device).await;
        }

        // Get context for the device switch. The model can be unloaded
        // concurrently between the `is_none()` check above and this read — treat
        // "gone" as "nothing to reload" and just record the preference.
        let Some((current_preferred, model_to_reload, source, is_online)) =
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
            result = self.load_model_with_target_device(&model_to_reload, &source, &device) => {
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
        let resolved_accel = self.resolved_accel(device).await;
        let available_devices = self.probe_available_devices().await;
        DaemonResponse::success()
            .with_device(device.to_string())
            .with_resolved_accel(resolved_accel)
            .with_available_devices(available_devices)
            .with_message(format!(
                "Device preference set to {device}. The next model load will use it."
            ))
    }

    /// Validate and normalize a device switch request. `Ok` carries the
    /// normalized `cpu`/`gpu` preference to thread through the rest of the
    /// switch; `Err` is an early response the caller returns as-is, whether
    /// that is a rejection or an already-satisfied no-op.
    async fn validate_device_switch_request(&self, device: &str) -> Result<String, DaemonResponse> {
        // Validate and normalize (`cuda`/`metal` → `gpu`). Emit the documented
        // `400 invalid_device` code so clients can distinguish a bad request
        // from a server failure (an uncoded error maps to 500) — audit 2 Tier 2 #7.
        let Some(device) = parse_device_preference(device) else {
            warn!("Invalid device specified: {device}");
            return Err(DaemonResponse::error_with_code(
                ErrorCode::InvalidDevice,
                &format!("Invalid device '{device}'. Must be 'cpu' or 'gpu'"),
            ));
        };

        // Check current preferred and actual devices
        let current_preferred = self.preferred_device.read().await.clone();
        let current_actual = self.actual_device.read().await.clone();

        if current_preferred == device && current_actual == device {
            info!(
                "Device switch skipped - already using device: {device} (preferred: {current_preferred}, actual: {current_actual})"
            );
            let resolved_accel = self.resolved_accel(&device).await;
            let available_devices = self.probe_available_devices().await;
            return Err(DaemonResponse::success()
                .with_device(device.clone())
                .with_resolved_accel(resolved_accel)
                .with_available_devices(available_devices)
                .with_message(format!("Already using device: {device}")));
        } else if current_preferred == device && current_actual != device {
            info!(
                "Device preference is set to {device} but actual device is {current_actual} - forcing model reload"
            );
        }

        // Prevent device switching during active recording.
        if let Some(resp) = self.guard_model_mutation("switch devices").await {
            warn!("Device switch rejected - recording in progress");
            return Err(resp);
        }

        Ok(device)
    }

    /// Get context needed for device switch
    async fn get_device_switch_context(
        &self,
        _device: &str,
    ) -> Option<(String, String, String, bool)> {
        // Read the model that needs to be reloaded. It was present when the
        // caller checked, but the lock is released in between, so a concurrent
        // unload (a reload or a backend uninstall) can leave it `None` — return
        // that instead of panicking. Online-ness is read from the loaded model
        // (which implements `ModelInfo`).
        let (model_to_reload, source, is_online) = {
            let guard = self.model.read().await;
            guard.as_ref().map(|loaded| {
                (
                    loaded.definition.name.clone(),
                    loaded.definition.source.clone(),
                    loaded.definition.is_online(),
                )
            })
        }?;
        let current_preferred = self.preferred_device.read().await.clone();
        Some((current_preferred, model_to_reload, source, is_online))
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
        source: &str,
        previous_device: &str,
    ) -> DaemonResponse {
        // Store the reloaded model. The backend serving it can be uninstalled
        // concurrently with the switch, so `resolve_definition` may now return
        // `None` — fail the request gracefully (leaving the daemon idle) rather
        // than panicking on the capture thread.
        let Some(definition) = self.resolve_definition(model_to_reload, source).await else {
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

        let success_message = if actual_device != device && device == "gpu" {
            "Device switch requested to GPU, but fell back to CPU: no usable accelerator"
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

        let resolved_accel = self.resolved_accel(device).await;
        let available_devices = self.probe_available_devices().await;
        DaemonResponse::success()
            .with_device(device.to_string())
            .with_resolved_accel(resolved_accel)
            .with_available_devices(available_devices)
            .with_message(success_message)
    }

    /// Handle failed device switch with recovery attempt
    async fn handle_device_switch_failure(
        &self,
        error: anyhow::Error,
        device: &str,
        model_to_reload: &str,
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
            .load_model_with_target_device(model_to_reload, source, previous_device)
            .await
        {
            Ok(model_instance) => {
                let Some(definition) = self.resolve_definition(model_to_reload, source).await
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

    /// Probe the host's device availability fresh, off the async runtime.
    /// Shared by every `/active_device` response path so `available_devices`
    /// never depends on which one produced the response.
    async fn probe_available_devices(&self) -> Vec<String> {
        let host = tokio::task::spawn_blocking(crate::registry::host_detect::detect)
            .await
            .unwrap_or_else(|_| crate::registry::host_detect::Host {
                target_triple: String::new(),
                cuda: None,
                rocm: None,
                vulkan: None,
            });
        host_available_devices(&host)
    }

    /// The documented `resolved_accel` rule: `"cpu"` needs no resolution — it
    /// is always resolved. A `"gpu"` preference resolves only once a *local*
    /// model has actually loaded onto it (an online model has nothing to
    /// resolve locally either), reported via `self.actual_device`; until then
    /// it is `None`, so a client is never told a device resolved before an
    /// actual load event confirmed it.
    async fn resolved_accel(&self, preferred_device: &str) -> Option<String> {
        if preferred_device == "cpu" {
            return Some("cpu".to_string());
        }
        let local_model_loaded = self
            .model
            .read()
            .await
            .as_ref()
            .is_some_and(|loaded| !loaded.definition.is_online());
        if !local_model_loaded {
            return None;
        }
        Some(self.actual_device.read().await.clone())
    }

    /// Handle get device command - return current device information
    pub async fn handle_get_device(&self) -> DaemonResponse {
        let preferred_device = self.preferred_device.read().await.clone();
        let actual_device = self.actual_device.read().await.clone();

        info!("Device status requested - preferred: {preferred_device}, actual: {actual_device}");

        // Answers for the host, not for any one model — probed fresh rather
        // than assumed, so an AMD host is never offered a GPU it cannot use.
        let available_devices = self.probe_available_devices().await;
        let resolved_accel = self.resolved_accel(&preferred_device).await;

        let message = if preferred_device != actual_device && preferred_device == "gpu" {
            format!(
                "Preferred device: GPU, Actual device: {actual_device} (no usable accelerator or load failed)"
            )
        } else {
            format!("Device: {actual_device} (preferred and actual match)")
        };

        DaemonResponse::success()
            .with_device(preferred_device)
            .with_resolved_accel(resolved_accel)
            .with_available_devices(available_devices)
            .with_message(message)
    }

    /// Read-only GPU inventory for `GET /gpu_info`. Hardware detection runs on a
    /// blocking thread (NVML / sysfs / `system_profiler`) so it never stalls the
    /// async runtime. Best-effort: an empty list when no GPU is found.
    pub async fn handle_get_gpu_info() -> DaemonResponse {
        let (gpus, host) = tokio::task::spawn_blocking(|| {
            let gpus = gpu_probe::detect()
                .into_iter()
                .map(gpu_to_wire)
                .collect::<Vec<_>>();
            (gpus, gpu_host_to_wire())
        })
        .await
        .unwrap_or_default();
        DaemonResponse::success()
            .with_gpu_info(gpus)
            .with_gpu_host_info(host)
    }
}

/// The devices this host can offer.
///
/// Answers for the host, not for any one model: a client narrowing to a
/// specific model intersects this with that model's `supported_devices` and
/// the backend's `installed_accel` from `GET /backends`.
pub(crate) fn host_available_devices(host: &crate::registry::host_detect::Host) -> Vec<String> {
    let mut devices = vec!["cpu".to_string()];
    if host.cuda.is_some() || host.rocm.is_some() || host.vulkan.is_some() {
        devices.push("gpu".to_string());
    }
    devices
}

/// Normalize a requested device preference, or `None` when it is not one.
///
/// `cuda` and `metal` are accepted as deprecated spellings of `gpu` so clients
/// shipped before this vocabulary keep working; `none` is a model property, not
/// a preference a client may set, so it is rejected here even though
/// `Device::from_str` parses it.
pub(crate) fn parse_device_preference(device: &str) -> Option<String> {
    match device.parse::<super_stt_registry_types::manifest::Device>() {
        Ok(super_stt_registry_types::manifest::Device::Cpu) => Some("cpu".to_string()),
        Ok(super_stt_registry_types::manifest::Device::Gpu) => Some("gpu".to_string()),
        _ => None,
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
    let arch_target = arch_label(gpu.arch_target);
    super_stt_shared::models::protocol::GpuInfo {
        name: gpu.name,
        vendor,
        total_bytes: gpu.total_bytes,
        free_bytes: gpu.free_bytes,
        used_bytes: gpu.used_bytes,
        arch_target,
    }
}

/// Render a probed architecture target for the wire.
///
/// `ArchTarget`'s `Display` already emits each vendor's own spelling —
/// `sm_86` for CUDA, `gfx1030` for `--offload-arch` — so this exists only to
/// carry `None` through as `null` and to give that behavior a test, since
/// `gpu_probe::GpuInfo` is `#[non_exhaustive]` and cannot be built here.
fn arch_label(target: Option<gpu_probe::ArchTarget>) -> Option<String> {
    target.map(|t| t.to_string())
}

/// Build the `/gpu_info` host block from `gpu-probe`'s raw toolchain probes.
///
/// Deliberately the *unfiltered* facts, unlike [`host_available_devices`] and
/// the `Host` it reads: that path gates `vulkan` on a GPU actually being
/// present, because a false positive there would make a lavapipe-only host
/// download a GPU asset it should never run. `/gpu_info` is a read-only
/// diagnostics endpoint that mutates nothing and drives no selection, so the
/// safety concern that motivates that gate does not apply here — this reports
/// whichever loader/toolchain is installed, full stop, the same way
/// `host.rocm` already reports a `ROCm` userspace install with no claim about
/// whether a GPU is behind it. A caller wanting "is there a real GPU here"
/// already has that from `gpu_info[].vendor`.
///
/// [`host_available_devices`]: host_available_devices
fn gpu_host_to_wire() -> super_stt_shared::models::protocol::GpuHostInfo {
    use super_stt_shared::models::protocol::{
        CudaHostInfo, GpuHostInfo, RocmHostInfo, VulkanHostInfo,
    };
    GpuHostInfo {
        cuda: gpu_probe::cuda_host().map(|h| CudaHostInfo {
            driver_version: h.driver_version.to_string(),
        }),
        rocm: gpu_probe::rocm_host().map(|h| RocmHostInfo {
            version: h.version.to_string(),
        }),
        vulkan: gpu_probe::vulkan_host().map(|h| VulkanHostInfo {
            api_version: h.api_version.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::host_detect::{Host, VulkanHost};

    fn bare_host() -> Host {
        Host {
            target_triple: "x86_64-unknown-linux-gnu".into(),
            cuda: None,
            rocm: None,
            vulkan: None,
        }
    }

    /// The list used to be a constant `["cpu", "cuda"]`, which offered an AMD
    /// host a device it could never resolve. It answers from the probe now.
    #[test]
    fn a_host_without_an_accelerator_offers_only_the_cpu() {
        assert_eq!(
            host_available_devices(&bare_host()),
            vec!["cpu".to_string()]
        );
    }

    #[test]
    fn any_accelerator_adds_the_gpu() {
        let mut cuda = bare_host();
        cuda.cuda = Some(crate::registry::host_detect::CudaHost {
            compute_capability: 86,
            runtime_major: 13,
            cudnn_present: false,
        });
        assert_eq!(
            host_available_devices(&cuda),
            vec!["cpu".to_string(), "gpu".to_string()]
        );

        let mut vulkan = bare_host();
        vulkan.vulkan = Some(VulkanHost {
            api_version: gpu_probe::VulkanVersion::new(1, 3, 0),
        });
        assert_eq!(
            host_available_devices(&vulkan),
            vec!["cpu".to_string(), "gpu".to_string()]
        );
    }

    #[test]
    fn the_wire_setter_accepts_the_deprecated_spellings_and_rejects_junk() {
        assert_eq!(parse_device_preference("gpu"), Some("gpu".to_string()));
        assert_eq!(parse_device_preference("cuda"), Some("gpu".to_string()));
        assert_eq!(parse_device_preference("metal"), Some("gpu".to_string()));
        assert_eq!(parse_device_preference("cpu"), Some("cpu".to_string()));
        assert_eq!(
            parse_device_preference("rocm"),
            None,
            "an accel is not a device"
        );
        assert_eq!(parse_device_preference("none"), None, "not a preference");
        assert_eq!(parse_device_preference("nonsense"), None);
    }

    #[test]
    fn an_architecture_target_renders_in_the_vendors_own_spelling() {
        assert_eq!(
            arch_label(Some(gpu_probe::ArchTarget::Sm(
                gpu_probe::ComputeCapability::new(8, 6)
            ))),
            Some("sm_86".to_string())
        );
        assert_eq!(
            arch_label(Some(gpu_probe::ArchTarget::Gfx(gpu_probe::GfxTarget::new(
                10, 3, 0
            )))),
            Some("gfx1030".to_string())
        );
    }

    /// A GPU whose driver reports no target — an Apple or Intel part, or an
    /// AMD card on a kernel without KFD — is `null`, never a placeholder
    /// string a client would have to know to ignore.
    #[test]
    fn an_unreported_architecture_is_null() {
        assert_eq!(arch_label(None), None);
    }
}
