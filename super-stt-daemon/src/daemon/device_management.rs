// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/model/{model}/device` — the device a model runs on.
//!
//! Contract: `docs/protocol/endpoints/v1/pipeline.md` (the device verb).
//!
//! A device is a property of a model, not of the daemon: a small model runs
//! fine on the CPU while the large one beside it needs the GPU, and a
//! post-processor sharing the pipeline with either has its own answer again.
//! So the preference is stored per `(source, model)` and addressed through the
//! stage that runs the model, which is also what decides what setting it
//! means: for the model loaded in its stage it is a reload onto the new
//! device, for any other it is a note for the next load.

use crate::daemon::types::{SuperSTTDaemon, normalize_device};
use crate::stt_models::ModelDefinition;
use crate::stt_models::backends;
use log::{error, info, warn};
pub(crate) use super_engine_daemon::devices::{
    backend_available_devices, host_available_devices, model_available_devices,
    parse_device_preference,
};
use super_engine_daemon::devices::{device_rejection, device_switch_message, switch_is_satisfied};
use super_stt_registry_types::manifest::Device;
use super_stt_shared::models::protocol::{Command, DaemonResponse, DaemonStatusEvent, ErrorCode};

/// The pipeline stage a device command addresses. Each has its own selected
/// backend, its own loaded slot and its own reload path; everything between
/// — resolving the model, validating the device, shaping the answer — is
/// shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    /// Stage 1: audio to text.
    Transcription,
    /// Stage 2: the transcript rewriter.
    PostProcessor,
}

impl PipelineStage {
    /// The stage's 1-based position, as `/pipeline/{stage}` spells it — and
    /// as the `stage` field of the events and download reports it emits.
    #[must_use]
    pub fn position(self) -> u32 {
        match self {
            Self::Transcription => super_stt_shared::models::protocol::TRANSCRIPTION_STAGE,
            Self::PostProcessor => super_stt_shared::models::protocol::POST_PROCESSOR_STAGE,
        }
    }

    /// Whether the stage runs post-processors (else transcription models).
    fn is_post_processor(self) -> bool {
        self == Self::PostProcessor
    }
}

/// The model a device command resolved to: its definition, and the install
/// directory whose record says which accelerators the installed build has.
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

impl SuperSTTDaemon {
    /// Route the per-model device commands to their handlers, each naming the
    /// stage its wire command addresses. Keeps the destructuring out of the
    /// giant `handle_command` match.
    ///
    /// # Panics
    /// Panics if `cmd` is not one of the per-model device variants; the caller
    /// (`handle_command`) only ever passes those.
    pub async fn handle_model_device(&self, cmd: Command) -> DaemonResponse {
        use PipelineStage::{PostProcessor, Transcription};
        match cmd {
            Command::SetModelDevice { model, device } => {
                self.handle_set_model_device(Transcription, model, device)
                    .await
            }
            Command::GetModelDevice { model } => {
                self.handle_get_model_device(Transcription, model).await
            }
            Command::SetPostProcessorDevice { model, device } => {
                self.handle_set_model_device(PostProcessor, model, device)
                    .await
            }
            Command::GetPostProcessorDevice { model } => {
                self.handle_get_model_device(PostProcessor, model).await
            }
            Command::ListModelDevices { model } => {
                self.handle_list_model_devices(Transcription, model).await
            }
            Command::ListActiveBackendDevices => {
                self.handle_list_stage_devices(Transcription).await
            }
            Command::ListPostProcessorDevices { model } => {
                self.handle_list_model_devices(PostProcessor, model).await
            }
            Command::ListPostProcessorBackendDevices => {
                self.handle_list_stage_devices(PostProcessor).await
            }
            _ => unreachable!("handle_model_device received a non-device command"),
        }
    }

    /// `GET /pipeline/{stage}/model/{model}/device` — the device `model`
    /// prefers, what it resolved to, and what this install can offer it.
    pub(crate) async fn handle_get_model_device(
        &self,
        stage: PipelineStage,
        model: String,
    ) -> DaemonResponse {
        let target = match self.resolve_device_target(stage, &model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        let device = self.effective_device(&target).await;
        let message = format!("Device for {model}: {device}");
        self.model_device_response(stage, &target, message).await
    }

    /// `GET /pipeline/{stage}/model/{model}/device/list` — the devices this
    /// install can offer `model` on this host, on their own.
    pub(crate) async fn handle_list_model_devices(
        &self,
        stage: PipelineStage,
        model: String,
    ) -> DaemonResponse {
        let target = match self.resolve_device_target(stage, &model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        let install = self.install_context(&target.backend_dir).await;
        let devices = install.offer(&target.definition);
        DaemonResponse::success()
            .with_available_devices(devices)
            .with_message(format!("Devices available to {model} listed"))
    }

    /// `GET /pipeline/{stage}/device/list` — the devices the backend selected
    /// for `stage` can be run on here: the union over the models it serves
    /// for that stage of what this install can offer each.
    ///
    /// Scoped to the stage's role, because that is what "this backend" means
    /// from a stage: a backend serving both a transcription model and a
    /// post-processor answers stage 1 for the former and stage 2 for the
    /// latter.
    pub(crate) async fn handle_list_stage_devices(&self, stage: PipelineStage) -> DaemonResponse {
        let position = stage.position();
        let Some(source) = self.stage_source(stage).await else {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                &format!(
                    "No backend is selected for stage {position}, so there is no backend \
                     to list devices for. Select one with POST /pipeline/{position}."
                ),
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
                &format!("Backend {source} (stage {position}) is no longer installed."),
            );
        };
        let install = self.install_context(&backend_dir).await;
        let devices = backend_available_devices(
            models
                .iter()
                .filter(|m| m.product.role.is_post_processor() == stage.is_post_processor())
                .map(|m| install.offer(m)),
        );
        DaemonResponse::success()
            .with_available_devices(devices)
            .with_message(format!(
                "Devices available to {source} (stage {position}) listed"
            ))
    }

    /// `POST /pipeline/{stage}/model/{model}/device` — run `model` on
    /// `device`. Reloads it when it is the model loaded in `stage`; otherwise
    /// only records the choice, which its next load picks up.
    pub(crate) async fn handle_set_model_device(
        &self,
        stage: PipelineStage,
        model: String,
        device: String,
    ) -> DaemonResponse {
        info!(
            "Device change requested for {model} (stage {}): {device}",
            stage.position()
        );

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

        let target = match self.resolve_device_target(stage, &model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        if let Some(rejection) = model_rejects_device(&target.definition, &device) {
            return rejection;
        }

        // Where the model is running now, if it is the one loaded in its
        // stage. Only then does the change mean a reload.
        let running_on = self.running_device(stage, &target).await;
        let current = self.effective_device(&target).await;
        let name = &target.definition.name;

        let Some(actual) = running_on else {
            self.store_model_device(&target, &device).await;
            info!("Device for {name} set to {device} (not loaded — nothing to reload)");
            return self
                .model_device_response(
                    stage,
                    &target,
                    format!("Device for {name} set to {device}. Its next load will use it."),
                )
                .await;
        };

        if switch_is_satisfied(&current, &actual, &device) {
            // Nothing to reload — but record the choice anyway: the model
            // may have been on this device only through the global default,
            // and the user just made it its own.
            self.store_model_device(&target, &device).await;
            info!("Device change skipped - {name} already on {device} (actual: {actual})");
            return self
                .model_device_response(stage, &target, format!("Already using device: {device}"))
                .await;
        }
        if current == device {
            info!("Device for {name} is set to {device} but it is on {actual} - forcing a reload");
        }

        // Loading and unloading a backend instance during a recording is the
        // same hazard as switching models mid-recording.
        if let Some(resp) = self.guard_model_mutation("switch devices").await {
            warn!("Device change rejected - recording in progress");
            return resp;
        }

        match stage {
            PipelineStage::Transcription => {
                self.switch_transcription_device(target, &device, &current, shutdown_rx)
                    .await
            }
            PipelineStage::PostProcessor => {
                self.reload_post_processor_device(target, &device, &current)
                    .await
            }
        }
    }

    /// Resolve `model` against the backend selected for `stage`.
    ///
    /// The path names a stage, not a backend, so the model is looked up in
    /// the backend filling that stage — the same resolution `POST
    /// /pipeline/{stage}/model` performs for an omitted `source`. A role
    /// mismatch is refused here, before anything is stored: a post-processor
    /// asked about through stage 1 is a wrong model of the pipeline, not a
    /// model with a device.
    // `DaemonResponse` is the protocol's response type and is returned by value
    // throughout the daemon; boxing it in this one helper's `Err` would buy
    // nothing and read inconsistently against every sibling handler.
    #[allow(clippy::result_large_err)]
    async fn resolve_device_target(
        &self,
        stage: PipelineStage,
        model: &str,
    ) -> Result<DeviceTarget, DaemonResponse> {
        let position = stage.position();
        let Some(source) = self.stage_source(stage).await else {
            return Err(DaemonResponse::error_with_code(
                ErrorCode::InvalidBackend,
                &format!(
                    "No backend is selected for stage {position}, so there is nothing to \
                     resolve the model against. Select one with POST /pipeline/{position}."
                ),
            ));
        };
        let found = {
            let backends = self.backends.read().await;
            backends::find_model(&backends, model, &source)
                .map(|(backend, definition)| (backend.dir.clone(), definition.clone()))
        };
        let Some((backend_dir, definition)) = found else {
            return Err(DaemonResponse::error_with_code(
                ErrorCode::InvalidModel,
                &format!("Backend {source} (stage {position}) serves no model {model}."),
            ));
        };
        if definition.product.role.is_post_processor() != stage.is_post_processor() {
            let (is, other) = if stage.is_post_processor() {
                ("a transcription model", 1)
            } else {
                ("a post-processing model", 2)
            };
            return Err(DaemonResponse::error_with_code(
                ErrorCode::InvalidModel,
                &format!(
                    "Model {model} is {is}, not a stage {position} model. Address it \
                     through /pipeline/{other}/model/{model}/device instead."
                ),
            ));
        }
        Ok(DeviceTarget {
            definition,
            backend_dir,
        })
    }

    /// Repo id of the backend selected for `stage`, or `None` when the stage
    /// is empty.
    async fn stage_source(&self, stage: PipelineStage) -> Option<String> {
        match stage {
            PipelineStage::Transcription => self.active_backend_source().await,
            PipelineStage::PostProcessor => {
                let source = self.config.read().await.post_processor.source.clone();
                (!source.is_empty()).then_some(source)
            }
        }
    }

    /// The device the target loads on: its own, else the global default.
    async fn effective_device(&self, target: &DeviceTarget) -> String {
        self.config
            .read()
            .await
            .effective_device(&target.definition.source, &target.definition.name)
    }

    /// Record the target's device and persist it. A persist failure is logged,
    /// not returned: the in-memory config already holds the choice, so the
    /// daemon behaves as asked until it restarts.
    async fn store_model_device(&self, target: &DeviceTarget, device: &str) {
        self.write_model_device(target, Some(device.to_string()))
            .await;
    }

    /// [`store_model_device`](Self::store_model_device) over the full range of
    /// the setting, `None` included — what a rolled-back switch needs, since a
    /// model that had no device of its own must be left with none rather than
    /// pinned to the global default it was following.
    async fn write_model_device(&self, target: &DeviceTarget, device: Option<String>) {
        self.config.write().await.update_model_device(
            &target.definition.source,
            &target.definition.name,
            device,
        );
        if let Err(e) = self.persist_config().await {
            warn!("Failed to persist config after device change: {e}");
        }
    }

    /// The accelerator the target is running on right now, or `None` when it
    /// is not the model loaded in its stage. Read from the instance rather
    /// than any preference, so a `gpu` choice that fell back to the CPU
    /// reports `cpu`.
    async fn running_device(&self, stage: PipelineStage, target: &DeviceTarget) -> Option<String> {
        let slot = match stage {
            PipelineStage::Transcription => &self.model,
            PipelineStage::PostProcessor => &self.post_processor,
        };
        let guard = slot.read().await;
        let loaded = guard.as_ref()?;
        (loaded.definition.name == target.definition.name
            && loaded.definition.source == target.definition.source)
            .then(|| normalize_device(&loaded.instance.device()))
    }

    /// The `{ device, resolved_accel, available_devices }` body both verbs
    /// answer with, so the shape cannot drift between them.
    async fn model_device_response(
        &self,
        stage: PipelineStage,
        target: &DeviceTarget,
        message: String,
    ) -> DaemonResponse {
        let online = target.definition.is_online();
        let device = if online {
            // The manifest's own sentinel: no local device, ever.
            Device::None.to_string()
        } else {
            self.effective_device(target).await
        };
        let resolved_accel = if online {
            // Remote compute: nothing resolves locally, loaded or not.
            None
        } else {
            match self.running_device(stage, target).await {
                Some(actual) => Some(actual),
                // Not loaded: `cpu` needs no resolution, `gpu` has none yet —
                // a client is never told a device resolved before a load
                // confirmed it.
                None => (device == "cpu").then(|| device.clone()),
            }
        };
        let available_devices = self
            .install_context(&target.backend_dir)
            .await
            .offer(&target.definition);
        DaemonResponse::success()
            .with_device(device)
            .with_resolved_accel(resolved_accel)
            .with_available_devices(available_devices)
            .with_message(message)
    }

    /// What this host and one backend's installed asset can offer, read once
    /// per request: the host is probed fresh, off the async runtime, so an AMD
    /// host is never offered a GPU it cannot use, and the install record says
    /// which accelerators the build actually has.
    async fn install_context(&self, backend_dir: &std::path::Path) -> InstallContext {
        let host = tokio::task::spawn_blocking(crate::registry::host_detect::detect)
            .await
            .unwrap_or_else(|_| crate::registry::host_detect::Host {
                target_triple: String::new(),
                cuda: None,
                rocm: None,
                vulkan: None,
                metal: None,
            });
        InstallContext {
            host_devices: host_available_devices(&host),
            installed_accel: crate::registry::installed::read(backend_dir)
                .map(|r| r.selected.accel)
                .unwrap_or_default(),
        }
    }

    /// Reload the stage-1 model onto `device`: unload, load, and on failure
    /// recover onto `previous`. The preference is stored only once the load
    /// succeeded, so a failed switch leaves the model's setting as it was.
    async fn switch_transcription_device(
        &self,
        target: DeviceTarget,
        device: &str,
        previous: &str,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) -> DaemonResponse {
        let name = target.definition.name.clone();
        let source = target.definition.source.clone();
        info!("Starting device switch for {name} from {previous} to {device}");

        self.events
            .publish_daemon_status(DaemonStatusEvent::SwitchingDevice {
                from_device: previous.to_string(),
                target_device: device.to_string(),
                model: name.clone(),
                stage: PipelineStage::Transcription.position(),
            });
        // Route through the shared graceful path so the backend is
        // `shutdown()` outside the write lock rather than dropped under it — a
        // subprocess `Drop` can block for seconds freeing GPU memory, which
        // would stall every reader (Tier 3 #2).
        self.unload_current_model().await;

        // Reload on the requested device, unless a shutdown arrives first.
        let load_result = tokio::select! {
            result = self.load_model_with_target_device(&name, &source, device) => result,
            _ = shutdown_rx.recv() => {
                warn!("Device switch cancelled due to shutdown");
                return DaemonResponse::error("Device switch cancelled due to shutdown");
            }
        };

        match load_result {
            Ok((instance, definition)) => {
                let actual_device = self.finalize_loaded_model(definition, instance).await;
                self.store_model_device(&target, device).await;
                info!(
                    "Device switch completed for {name}: {previous} -> {device} (actual: {actual_device})"
                );
                self.events.publish_daemon_status(DaemonStatusEvent::Ready {
                    model_loaded: true,
                    model_name: Some(name),
                    actual_device: Some(actual_device.clone()),
                    preferred_device: Some(device.to_string()),
                    stage: PipelineStage::Transcription.position(),
                });
                self.model_device_response(
                    PipelineStage::Transcription,
                    &target,
                    device_switch_message(device, &actual_device),
                )
                .await
            }
            Err(e) => {
                self.recover_transcription_device(&target, e, device, previous)
                    .await
            }
        }
    }

    /// A failed switch: report it, then try to put the model back on
    /// `previous`. Nothing was stored, so the setting needs no reverting.
    async fn recover_transcription_device(
        &self,
        target: &DeviceTarget,
        error: anyhow::Error,
        device: &str,
        previous: &str,
    ) -> DaemonResponse {
        let name = &target.definition.name;
        let source = &target.definition.source;
        error!("Failed to reload {name} on {device}: {error}");
        self.events
            .publish_daemon_status(DaemonStatusEvent::DeviceSwitchError {
                error: error.to_string(),
                failed_device: device.to_string(),
                model: name.clone(),
                stage: PipelineStage::Transcription.position(),
            });

        let mut shutdown_rx = self.shutdown_tx.subscribe();
        if let Ok(()) = shutdown_rx.try_recv() {
            warn!("Shutdown in progress, skipping device switch recovery");
            return DaemonResponse::error(&format!(
                "Device switch failed: {error}. Recovery skipped due to shutdown."
            ));
        }

        warn!("Attempting to recover by reverting {name} to previous device: {previous}");
        match self
            .load_model_with_target_device(name, source, previous)
            .await
        {
            Ok((instance, definition)) => {
                let actual_device = self.finalize_loaded_model(definition, instance).await;
                warn!(
                    "Recovery successful - {name} reverted to {previous} (actual: {actual_device})"
                );
                self.events.publish_daemon_status(DaemonStatusEvent::Ready {
                    model_loaded: true,
                    model_name: Some(name.clone()),
                    actual_device: Some(actual_device.clone()),
                    preferred_device: Some(previous.to_string()),
                    stage: PipelineStage::Transcription.position(),
                });
                DaemonResponse::error(&format!(
                    "Failed to switch to device '{device}': {error}. Reverted to previous device '{actual_device}'."
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

    /// Reload the stage-2 model onto `device`: unload, load, and on failure
    /// recover onto `previous` — the same shape as
    /// [`switch_transcription_device`](Self::switch_transcription_device),
    /// because a stage should not answer for its own device differently from
    /// its neighbour.
    ///
    /// A failed reload is reported as a *failure*. The best-effort policy the
    /// other post-processor writes follow — save the setting, report the load
    /// in the message — is right when the setting is the request and the load
    /// is a consequence; here the reload *is* the request, and answering
    /// `success` left every client showing a device the model never moved to.
    ///
    /// Stage 2 loads on whatever device its config names, so unlike stage 1 the
    /// choice has to be stored before the load rather than after it — and put
    /// back if the load fails.
    async fn reload_post_processor_device(
        &self,
        target: DeviceTarget,
        device: &str,
        previous: &str,
    ) -> DaemonResponse {
        let name = target.definition.name.clone();
        // The setting the model had of its own, which `previous` is not: that
        // is the *effective* device, and may be the global default the model
        // was merely following.
        let prior = self
            .config
            .read()
            .await
            .model_device(&target.definition.source, &target.definition.name)
            .map(str::to_string);
        self.store_model_device(&target, device).await;
        let response = match self.load_configured_post_processor().await {
            Ok(()) => {
                info!("Post-processor {name} reloaded on {device}");
                self.model_device_response(
                    PipelineStage::PostProcessor,
                    &target,
                    format!("Device for {name} set to {device}"),
                )
                .await
            }
            Err(e) => {
                self.recover_post_processor_device(&target, &e, device, previous, prior)
                    .await
            }
        };
        self.publish_settings_changed("post_processor");
        response
    }

    /// A failed stage-2 switch: put the setting back where it was and load the
    /// model on it again, so a device the model cannot run on costs the user
    /// neither the preference nor the running post-processor. The stage-2 twin
    /// of [`recover_transcription_device`](Self::recover_transcription_device).
    async fn recover_post_processor_device(
        &self,
        target: &DeviceTarget,
        error: &anyhow::Error,
        device: &str,
        previous: &str,
        prior: Option<String>,
    ) -> DaemonResponse {
        let name = &target.definition.name;
        error!("Failed to reload post-processor {name} on {device}: {error}");
        warn!("Attempting to recover by reverting {name} to previous device: {previous}");
        self.write_model_device(target, prior).await;
        match self.load_configured_post_processor().await {
            Ok(()) => {
                warn!("Recovery successful - post-processor {name} reverted to {previous}");
                DaemonResponse::error(&format!(
                    "Failed to switch to device '{device}': {error}. Reverted to previous device '{previous}'."
                ))
            }
            Err(recovery_e) => {
                error!("Post-processor recovery failed: {recovery_e}");
                DaemonResponse::error(&format!(
                    "Device switch failed: {error}. Recovery also failed: {recovery_e}. Stage 2 is now idle."
                ))
            }
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

#[cfg(test)]
mod tests {
    use super::*;

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
            product: super_stt_registry_types::manifest::SttModel {
                force_preview_support: true,
                role: super_stt_registry_types::manifest::ModelRole::Transcription,
            },
            provider: None,
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
