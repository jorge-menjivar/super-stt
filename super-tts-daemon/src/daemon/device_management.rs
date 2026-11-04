// SPDX-License-Identifier: GPL-3.0-only
//! The device a model runs on, and the global default behind it.
//!
//! A device is a property of a model, not of the daemon: a small voice runs
//! fine on the CPU while the large one beside it needs the GPU, and a single
//! global switch forced a user who wanted the GPU for one of them to accept it
//! for both. So the preference is stored per `(source, model)`
//! ([`crate::config::ModelSettings::device`]), and the global
//! `device.preferred_device` survives as the fallback a model with no choice
//! of its own inherits — which is also what keeps a config written before
//! per-model devices loading its models exactly where it always did.
//!
//! Which handler a request lands in decides what setting a device *means*: for
//! the model that is currently loaded it is a reload onto the new device, for
//! any other model it is a note its next load picks up. Both answer with the
//! same `{ device, resolved_accel, available_devices }` body so a client never
//! has to know which case it hit.

use crate::daemon::types::{SuperTTSDaemon, normalize_device};
use crate::tts_models::ModelDefinition;
use crate::tts_models::backends;
use crate::tts_models::synthesize::Synthesize;
use log::{error, info, warn};
pub(crate) use super_engine_daemon::devices::{
    backend_available_devices, host_available_devices, model_available_devices,
    parse_device_preference,
};
use super_engine_daemon::devices::{
    device_rejection, device_switch_message, preference_axis, switch_is_satisfied,
};
use super_tts_registry_types::manifest::Device;
use super_tts_shared::models::protocol::{
    Command, DaemonResponse, DaemonStatusEvent, ErrorCode, StageModelDevice,
};

/// Which stored preference a device switch is changing, and therefore what a
/// completed — or recovered — switch writes back.
///
/// The reload itself is identical either way (unload, load on the new device,
/// fall back to the old one if that fails), so the two callers share every
/// step of it and differ only here. Threading the scope through rather than
/// duplicating the reload is what keeps a fix to the recovery path from
/// landing in one of them and not the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceScope {
    /// The global default, set by `set_device`: mirrored into the
    /// `preferred_device` runtime lock as well as the config.
    Global,
    /// One model's own preference, set by `set_model_device` and stored
    /// against its `(source, model)`.
    Model,
}

/// The model a per-model device command resolved to: its definition, and the
/// install directory whose record says which accelerators the installed build
/// actually has.
struct DeviceTarget {
    definition: ModelDefinition,
    backend_dir: std::path::PathBuf,
}

/// The two facts that narrow a model's declared devices to what it can run
/// on here. See [`model_available_devices`].
struct InstallContext {
    host_devices: Vec<String>,
    installed_accel: Vec<String>,
}

impl InstallContext {
    /// The devices this install can offer `model` on this host.
    fn offer(&self, model: &ModelDefinition) -> Vec<String> {
        model_available_devices(
            &self.host_devices,
            &model.supported_devices,
            &self.installed_accel,
        )
    }
}

impl SuperTTSDaemon {
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

        // Only here, where the model is really reloaded: every return above
        // leaves it running, so none of them has cause to interrupt it.
        self.stop_speech_for("switch devices").await;
        info!(
            "Starting device switch from {current_preferred} to {device} (will reload model: {model_to_reload})"
        );

        self.reload_onto_device(
            DeviceScope::Global,
            &model_to_reload,
            &source,
            &device,
            &current_preferred,
            shutdown_rx,
        )
        .await
    }

    /// Move a loaded model onto `device`: announce, unload, load, and on
    /// failure put it back on `previous`. The preference is recorded once the
    /// load succeeded (or reverted once recovery has), which `scope` decides
    /// the destination of.
    ///
    /// Shared by the global setter and the per-model one so there is exactly
    /// one reload dance in the daemon. A second copy would drift on precisely
    /// the paths nobody exercises by hand — the shutdown race, the recovery
    /// load, the "backend uninstalled mid-switch" hole — and those are the ones
    /// that leave a user with no model loaded at all.
    async fn reload_onto_device(
        &self,
        scope: DeviceScope,
        model: &str,
        source: &str,
        device: &str,
        previous: &str,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> DaemonResponse {
        // Broadcast device switching status and unload current model
        self.prepare_device_switch(previous, device, model).await;

        // Try to reload model with the requested device, but cancel if shutdown occurs
        let load_result = tokio::select! {
            result = self.load_model_with_target_device(model, source, device) => {
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
                    scope,
                    model_instance,
                    device,
                    model,
                    source,
                    previous,
                )
                .await
            }
            Err(e) => {
                self.handle_device_switch_failure(scope, e, device, model, source, previous)
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
    // `DaemonResponse` is the protocol's response type and is returned by value
    // throughout the daemon; boxing it in this one helper's `Err` would buy
    // nothing and read inconsistently against every sibling handler.
    #[allow(clippy::result_large_err)]
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

        if switch_is_satisfied(&current_preferred, &current_actual, &device) {
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
        } else if current_preferred == device {
            info!(
                "Device preference is set to {device} but actual device is {current_actual} - forcing model reload"
            );
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
        // Stage `actual_device` at the target for the duration of the switch,
        // so `get_device` does not keep reporting the device the model just
        // left while its replacement is still loading. `finalize_loaded_model`
        // overwrites this with the accelerator the load really landed on, and
        // the recovery path with the one it reverted to, so the optimistic
        // value never outlives the switch.
        {
            let mut w = self.actual_device.write().await;
            *w = to_device.to_string();
        }

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
        scope: DeviceScope,
        model_instance: Box<dyn Synthesize>,
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

        // Record the preference only now: a switch that failed to load must
        // leave the stored device as it was, or a daemon restart would put the
        // model back on a device it has already proven it cannot run on.
        self.record_device_preference(scope, source, model_to_reload, device)
            .await;

        let success_message = device_switch_message(device, &actual_device);

        info!("Device switch completed: {previous_device} -> {device} (actual: {actual_device})");

        // Broadcast ready status with new device
        self.events.publish_daemon_status(DaemonStatusEvent::Ready {
            model_loaded: true,
            model_name: Some(model_to_reload.to_string()),
            actual_device: Some(actual_device.clone()),
            preferred_device: Some(device.to_string()),
        });

        self.device_switch_response(scope, source, model_to_reload, device, success_message)
            .await
    }

    /// Handle failed device switch with recovery attempt
    async fn handle_device_switch_failure(
        &self,
        scope: DeviceScope,
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
                self.restore_device_preference(scope, model_to_reload, previous_device)
                    .await;

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

    /// Record `device` as the preference a completed switch established, and
    /// persist it. A persist failure is logged, not returned: the in-memory
    /// config already holds the choice, so the daemon behaves as asked until
    /// it restarts, and failing the request would claim the switch did not
    /// happen when the model has already moved.
    ///
    /// The global scope also mirrors into the `preferred_device` runtime lock,
    /// which `get_device` reads; the per-model scope has no runtime mirror
    /// because every reader of a model's device goes through the config.
    async fn record_device_preference(
        &self,
        scope: DeviceScope,
        source: &str,
        model: &str,
        device: &str,
    ) {
        match scope {
            DeviceScope::Global => {
                *self.preferred_device.write().await = device.to_string();
                self.config
                    .write()
                    .await
                    .update_preferred_device(device.to_string());
            }
            DeviceScope::Model => {
                self.config.write().await.update_model_device(
                    source,
                    model,
                    Some(device.to_string()),
                );
            }
        }
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after device change: {e}");
        }
    }

    /// Put the preference back after a failed switch recovered the model onto
    /// `previous`.
    ///
    /// Only the global default is written. A per-model preference is
    /// deliberately left alone: nothing was stored for it (the store happens
    /// only after a successful load), and `previous` is the model's
    /// *effective* device, which may be the global default it was merely
    /// inheriting — writing that back would silently pin the model to a device
    /// it never chose, and a later change to the default would then skip it.
    async fn restore_device_preference(&self, scope: DeviceScope, model: &str, previous: &str) {
        if scope == DeviceScope::Model {
            log::debug!(
                "Device switch for {model} failed; its stored preference was never changed, \
                 so there is nothing to revert"
            );
            return;
        }
        *self.preferred_device.write().await = previous.to_string();
        self.config
            .write()
            .await
            .update_preferred_device(previous.to_string());
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after device recovery: {e}");
        }
    }

    /// The success body a completed switch answers with, in the shape its
    /// scope owns: the global setter reports the host's devices, the per-model
    /// setter the narrower list that model can actually be offered here. Both
    /// carry the same three keys, so a client reads one shape either way.
    ///
    /// A per-model switch whose backend vanished mid-flight falls back to the
    /// host-wide shape rather than failing: the model did load, and answering
    /// with a slightly wider device list beats reporting a failure that did
    /// not happen.
    async fn device_switch_response(
        &self,
        scope: DeviceScope,
        source: &str,
        model: &str,
        device: &str,
        message: String,
    ) -> DaemonResponse {
        if scope == DeviceScope::Model
            && let Some(target) = self.device_target(model, source).await
        {
            return self.model_device_response(&target, message).await;
        }
        let resolved_accel = self.resolved_accel(device).await;
        let available_devices = self.probe_available_devices().await;
        DaemonResponse::success()
            .with_device(device.to_string())
            .with_resolved_accel(resolved_accel)
            .with_available_devices(available_devices)
            .with_message(message)
    }

    /// Probe the host's device availability fresh, off the async runtime.
    ///
    /// Shared by every device response path — the global default's, and the
    /// per-model narrowing that starts from it — so `available_devices` never
    /// depends on which one produced the response. Probed rather than cached
    /// because a driver can be installed under a running daemon, and a stale
    /// "no GPU here" would outlive the machine it was true of.
    async fn probe_available_devices(&self) -> Vec<String> {
        let host = tokio::task::spawn_blocking(crate::registry::host_detect::detect)
            .await
            .unwrap_or_else(|_| crate::registry::host_detect::Host {
                target_triple: String::new(),
                cuda: None,
                rocm: None,
                vulkan: None,
                metal: None,
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

        let message = device_status_message(&preferred_device, &actual_device);

        DaemonResponse::success()
            .with_device(preferred_device)
            .with_resolved_accel(resolved_accel)
            .with_available_devices(available_devices)
            .with_message(message)
    }

    /// Route the per-model device commands to their handlers. Keeps the
    /// destructuring out of the giant `handle_command` match.
    ///
    /// # Panics
    /// Panics if `cmd` is not one of the per-model device variants; the caller
    /// (`handle_command`) only ever passes those.
    pub async fn handle_model_device(&self, cmd: Command) -> DaemonResponse {
        match cmd {
            Command::SetModelDevice { model, device } => {
                self.handle_set_model_device(model, device).await
            }
            Command::GetModelDevice { model } => self.handle_get_model_device(model).await,
            Command::ListModelDevices { model } => self.handle_list_model_devices(model).await,
            Command::ListActiveBackendDevices => self.handle_list_active_backend_devices().await,
            _ => unreachable!("handle_model_device received a non-device command"),
        }
    }

    /// Read one model's device: the preference in effect for it, what that
    /// resolved to, and what this install can offer it.
    ///
    /// "In effect" rather than "stored", deliberately: a model with no device
    /// of its own answers with the global default it will actually load on,
    /// not with a null a client would have to know to resolve itself — and
    /// would resolve differently from the load path the first time the two
    /// disagreed.
    pub(crate) async fn handle_get_model_device(&self, model: String) -> DaemonResponse {
        let target = match self.resolve_device_target(&model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        let device = self.effective_device(&target).await;
        let message = format!("Device for {model}: {device}");
        self.model_device_response(&target, message).await
    }

    /// The devices this install can offer `model` on this host, on their own.
    ///
    /// The `available_devices` half of
    /// [`handle_get_model_device`](Self::handle_get_model_device) without the
    /// preference beside it, so a client painting a device picker for a model
    /// it is not asking about — a row in a model list, say — does not also
    /// pull that model's stored choice and its resolution.
    pub(crate) async fn handle_list_model_devices(&self, model: String) -> DaemonResponse {
        let target = match self.resolve_device_target(&model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        let install = self.install_context(&target.backend_dir).await;
        let devices = install.offer(&target.definition);
        DaemonResponse::success()
            .with_available_devices(devices)
            .with_message(format!("Devices available to {model} listed"))
    }

    /// The devices the active backend can be run on here: the union over the
    /// models it serves of what this install can offer each.
    ///
    /// The same answer as [`handle_list_model_devices`](Self::handle_list_model_devices)
    /// without naming a model, for a client that has selected a backend but no
    /// model yet — and which would otherwise have to fan out one request per
    /// model and union the results itself.
    pub(crate) async fn handle_list_active_backend_devices(&self) -> DaemonResponse {
        let Some(source) = self.active_backend_source().await else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                "No backend is selected, so there is no backend to list devices for. \
                 Select one first.",
            );
        };
        let found = {
            let backends = self.backends.read().await;
            backends
                .iter()
                .find(|b| b.source == source)
                .map(|b| (b.dir.clone(), b.models.clone()))
        };
        let Some((backend_dir, models)) = found else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                &format!("Backend {source} is no longer installed."),
            );
        };
        let install = self.install_context(&backend_dir).await;
        let devices = backend_available_devices(models.iter().map(|m| install.offer(m)));
        DaemonResponse::success()
            .with_available_devices(devices)
            .with_message(format!("Devices available to {source} listed"))
    }

    /// Run `model` on `device`. Reloads it when it is the loaded model;
    /// otherwise only records the choice, which its next load picks up — which
    /// is what lets a user stage a device for a model before ever loading it,
    /// instead of loading it once on the wrong device to move it.
    pub(crate) async fn handle_set_model_device(
        &self,
        model: String,
        device: String,
    ) -> DaemonResponse {
        info!("Device change requested for {model}: {device}");

        // A reload started now would race the exit; refuse before touching
        // anything.
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        if let Ok(()) = shutdown_rx.try_recv() {
            warn!("Device change rejected - shutdown in progress");
            return DaemonResponse::error("Device change rejected due to shutdown in progress");
        }

        // Validate and normalize (`cuda`/`metal` → `gpu`) in one step, so
        // everything downstream stores and threads `cpu`/`gpu` rather than the
        // raw input the client sent. Emit the documented `400 invalid_device`
        // so a bad request is distinguishable from a server failure.
        let Some(device) = parse_device_preference(&device) else {
            warn!("Invalid device specified: {device}");
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidDevice,
                &format!("Invalid device '{device}'. Must be 'cpu' or 'gpu'"),
            );
        };

        let target = match self.resolve_device_target(&model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        if let Some(rejection) = model_rejects_device(&target.definition, &device) {
            return rejection;
        }

        // Where the model is running now, if it is the loaded one. Only then
        // does the change mean a reload.
        let running_on = self
            .running_device(&target.definition.source, &target.definition.name)
            .await;
        let current = self.effective_device(&target).await;
        let name = target.definition.name.clone();
        let source = target.definition.source.clone();

        let Some(actual) = running_on else {
            self.store_model_device(&target, &device).await;
            info!("Device for {name} set to {device} (not loaded — nothing to reload)");
            return self
                .model_device_response(
                    &target,
                    format!("Device for {name} set to {device}. Its next load will use it."),
                )
                .await;
        };

        if switch_is_satisfied(&current, &actual, &device) {
            // Nothing to reload — but record the choice anyway: the model may
            // have been on this device only through the global default, and
            // the user just made it its own, which is what stops a later
            // change to that default from moving it.
            self.store_model_device(&target, &device).await;
            info!("Device change skipped - {name} already on {device} (actual: {actual})");
            return self
                .model_device_response(&target, format!("Already using device: {device}"))
                .await;
        }
        if current == device {
            info!("Device for {name} is set to {device} but it is on {actual} - forcing a reload");
        }

        // Loading and unloading a backend instance mid-utterance is the same
        // as switching models mid-utterance: the utterance stops.
        self.stop_speech_for("switch devices").await;

        info!("Starting device switch for {name} from {current} to {device}");
        self.reload_onto_device(
            DeviceScope::Model,
            &name,
            &source,
            &device,
            &current,
            shutdown_rx,
        )
        .await
    }

    /// Resolve `model` against the active backend.
    ///
    /// A device command names a model, not a `(source, model)` pair, so the
    /// model is looked up in the selected backend — the same resolution a
    /// model switch performs for an omitted `source`, and for the same reason:
    /// two installed backends may serve the same model name, and picking
    /// whichever the directory scan reached first would set a device on a
    /// model the user never meant.
    // `DaemonResponse` is the protocol's response type and is returned by value
    // throughout the daemon; boxing it in this one helper's `Err` would buy
    // nothing and read inconsistently against every sibling handler.
    #[allow(clippy::result_large_err)]
    async fn resolve_device_target(&self, model: &str) -> Result<DeviceTarget, DaemonResponse> {
        let Some(source) = self.active_backend_source().await else {
            return Err(DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                "No backend is selected, so there is nothing to resolve the model against. \
                 Select a backend first.",
            ));
        };
        self.device_target(model, &source).await.ok_or_else(|| {
            DaemonResponse::error_with_code(
                ErrorCode::InvalidModel,
                &format!("Backend {source} serves no model {model}."),
            )
        })
    }

    /// The `(definition, install dir)` pair a device answer needs for one
    /// known `(model, source)`, or `None` when no discovered backend serves
    /// it — which a backend uninstalled concurrently with the request makes
    /// reachable at any point, not just before the work starts.
    async fn device_target(&self, model: &str, source: &str) -> Option<DeviceTarget> {
        let backends = self.backends.read().await;
        backends::find_model(&backends, model, source).map(|(backend, definition)| DeviceTarget {
            definition: definition.clone(),
            backend_dir: backend.dir.clone(),
        })
    }

    /// The device the target loads on: its own, else the global default.
    async fn effective_device(&self, target: &DeviceTarget) -> String {
        self.config
            .read()
            .await
            .effective_device(&target.definition.source, &target.definition.name)
    }

    /// Record the target's device and persist it, for the paths that store
    /// without reloading (an unloaded model, or one already on the device).
    async fn store_model_device(&self, target: &DeviceTarget, device: &str) {
        self.record_device_preference(
            DeviceScope::Model,
            &target.definition.source,
            &target.definition.name,
            device,
        )
        .await;
    }

    /// The accelerator `(source, model)` is running on right now, or `None`
    /// when it is not the model that is loaded. Read from the instance rather
    /// than from any preference, so a `gpu` choice that fell back to the CPU
    /// reports `cpu` — the whole point of tracking the two separately.
    ///
    /// Doubles as the "is *this* model loaded?" test, which is why the two are
    /// deliberately one call: a caller that asked them separately could be told
    /// a model is loaded and then be handed the device of a different one,
    /// which is precisely the window a model switch leaves open.
    ///
    /// Takes the pair rather than a resolved [`DeviceTarget`] because the
    /// pipeline report asks about a *selection*, which stays meaningful after
    /// the backend serving it has been uninstalled and can no longer be
    /// resolved to a definition.
    pub(in crate::daemon) async fn running_device(
        &self,
        source: &str,
        model: &str,
    ) -> Option<String> {
        let guard = self.model.read().await;
        let loaded = guard.as_ref()?;
        (loaded.definition.name == model && loaded.definition.source == source)
            .then(|| normalize_device(&loaded.instance.device()))
    }

    /// The device block for one `(source, model)`: the preference in effect and
    /// what it has actually resolved to. `running` is that model's live
    /// accelerator, from [`running_device`](Self::running_device), or `None`
    /// when it is not the loaded one.
    ///
    /// Shared by the per-model device verbs and by stage 1's model slot on
    /// `GET /pipeline/{stage}/model`, which answer the same question in two
    /// shapes. Two derivations of "what device is this model on" would drift on
    /// the cases that are easy to get wrong — the online sentinel, and refusing
    /// to resolve a `gpu` preference no load has confirmed — and a client
    /// polling both endpoints would see them disagree.
    ///
    /// Deliberately without `available_devices`: that list costs a fresh host
    /// probe, and the pipeline's model slot is polled far more often than a
    /// device picker is painted.
    pub(in crate::daemon) async fn model_device_view(
        &self,
        source: &str,
        model: &str,
        running: Option<String>,
    ) -> StageModelDevice {
        let online = {
            let backends = self.backends.read().await;
            backends::find_model(&backends, model, source).is_some_and(|(_, def)| def.is_online())
        };
        if online {
            // The manifest's own sentinel: remote compute has no local device,
            // and nothing resolves locally whether it is loaded or not.
            return StageModelDevice {
                preference: Device::None.to_string(),
                resolved_accel: None,
            };
        }
        let preference = self.config.read().await.effective_device(source, model);
        let resolved_accel = match running {
            Some(actual) => Some(actual),
            // Not loaded: `cpu` needs no resolution, `gpu` has none yet — a
            // client is never told a device resolved before a load confirmed it.
            None => (preference == "cpu").then(|| preference.clone()),
        };
        StageModelDevice {
            preference,
            resolved_accel,
        }
    }

    /// The `{ device, resolved_accel, available_devices }` body every
    /// per-model device verb answers with, so the shape cannot drift between
    /// them.
    async fn model_device_response(
        &self,
        target: &DeviceTarget,
        message: String,
    ) -> DaemonResponse {
        let source = &target.definition.source;
        let name = &target.definition.name;
        let running = self.running_device(source, name).await;
        let view = self.model_device_view(source, name, running).await;
        let available_devices = self
            .install_context(&target.backend_dir)
            .await
            .offer(&target.definition);
        DaemonResponse::success()
            .with_device(view.preference)
            .with_resolved_accel(view.resolved_accel)
            .with_available_devices(available_devices)
            .with_message(message)
    }

    /// What this host and one backend's installed asset can offer, read once
    /// per request: the host is probed fresh, off the async runtime, so an AMD
    /// host is never offered a GPU it cannot use, and the install record says
    /// which accelerators the build actually has.
    async fn install_context(&self, backend_dir: &std::path::Path) -> InstallContext {
        InstallContext {
            host_devices: self.probe_available_devices().await,
            installed_accel: crate::registry::installed::read(backend_dir)
                .map(|r| r.selected.accel)
                .unwrap_or_default(),
        }
    }

    /// Read-only GPU inventory for `GET /gpu_info`. Hardware detection runs on a
    /// blocking thread (NVML / sysfs / `system_profiler`) so it never stalls the
    /// async runtime. Best-effort: an empty list when no GPU is found.
    pub async fn handle_get_gpu_info() -> DaemonResponse {
        let (gpus, host) = tokio::task::spawn_blocking(super_engine_daemon::devices::gpu_info)
            .await
            .unwrap_or_default();
        DaemonResponse::success()
            .with_gpu_info(gpus)
            .with_gpu_host_info(host)
    }
}

/// Refuse a device the model cannot run on at all, as an `invalid_device`
/// error. See `super_engine_daemon::devices::device_rejection`.
fn model_rejects_device(definition: &ModelDefinition, device: &str) -> Option<DaemonResponse> {
    device_rejection(definition, device)
        .map(|message| DaemonResponse::error_with_code(ErrorCode::InvalidDevice, &message))
}

/// The message `GET /active_device` reports, drawing the same distinction as
/// [`device_switch_message`].
fn device_status_message(preferred_device: &str, actual_device: &str) -> String {
    if preferred_device == "gpu" && preference_axis(actual_device) == "cpu" {
        format!(
            "Preferred device: GPU, Actual device: {actual_device} (no usable accelerator or load failed)"
        )
    } else {
        format!("Device: {actual_device} (preference: {preferred_device})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GET /active_device` reports the same distinction: a working GPU host
    /// is not a failed one, and a remote model is on no local accelerator at
    /// all — neither is "no usable accelerator".
    #[test]
    fn the_device_status_message_only_reports_a_real_fallback() {
        assert_eq!(
            device_status_message("gpu", "cuda"),
            "Device: cuda (preference: gpu)"
        );
        assert_eq!(
            device_status_message("cpu", "cpu"),
            "Device: cpu (preference: cpu)"
        );
        assert_eq!(
            device_status_message("gpu", "remote"),
            "Device: remote (preference: gpu)"
        );
        assert_eq!(
            device_status_message("gpu", "cpu"),
            "Preferred device: GPU, Actual device: cpu (no usable accelerator or load failed)"
        );
    }

    fn definition(devices: Vec<Device>) -> ModelDefinition {
        ModelDefinition {
            name: "m".to_string(),
            source: "github.com/x/y".to_string(),
            is_multilingual: false,
            primary_language: "en".to_string(),
            supported_languages: vec!["en".to_string()],
            estimated_vram_bytes: 0,
            processing_interval: std::time::Duration::from_secs(1),
            supported_devices: devices,
            realtime: false,
            provider: None,
            product: super_tts_registry_types::manifest::TtsModel::default(),
        }
    }

    /// The manifest, and only the manifest, decides what a model may be set
    /// to: the host's accelerators are a load-time fallback, not a rejection.
    #[test]
    fn a_model_is_refused_only_what_its_manifest_rules_out() {
        let local = definition(vec![Device::Cpu, Device::Gpu]);
        assert!(model_rejects_device(&local, "cpu").is_none());
        assert!(model_rejects_device(&local, "gpu").is_none());

        let cpu_only = definition(vec![Device::Cpu]);
        assert!(model_rejects_device(&cpu_only, "cpu").is_none());
        let rejection = model_rejects_device(&cpu_only, "gpu").expect("refused");
        assert_eq!(rejection.error_code, Some(ErrorCode::InvalidDevice));

        let online = definition(vec![Device::None]);
        for device in ["cpu", "gpu"] {
            let rejection = model_rejects_device(&online, device).expect("refused");
            assert_eq!(rejection.error_code, Some(ErrorCode::InvalidDevice));
        }
    }
}
