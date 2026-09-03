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
use super_stt_registry_types::manifest::Device;
use super_stt_shared::models::protocol::{Command, DaemonResponse, DaemonStatusEvent, ErrorCode};

/// The pipeline stage a device command addresses. Each has its own selected
/// backend, its own loaded slot and its own reload path; everything between
/// — resolving the model, validating the device, shaping the answer — is
/// shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PipelineStage {
    /// Stage 1: audio to text.
    Transcription,
    /// Stage 2: the transcript rewriter.
    PostProcessor,
}

impl PipelineStage {
    /// The stage's 1-based position, as `/pipeline/{stage}` spells it.
    fn position(self) -> u32 {
        match self {
            Self::Transcription => 1,
            Self::PostProcessor => 2,
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
                .filter(|m| m.is_post_processor() == stage.is_post_processor())
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
                self.reload_post_processor_device(target, &device).await
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
        if definition.is_post_processor() != stage.is_post_processor() {
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
        self.config.write().await.update_model_device(
            &target.definition.source,
            &target.definition.name,
            Some(device.to_string()),
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

    /// Reload the stage-2 model onto `device`. The setting is stored first
    /// and a load failure is reported, not failed — the same best-effort
    /// policy every other post-processor write follows — and the failed
    /// reload leaves the previous instance in place, since
    /// `load_configured_post_processor` replaces it only once the new one
    /// came up.
    async fn reload_post_processor_device(
        &self,
        target: DeviceTarget,
        device: &str,
    ) -> DaemonResponse {
        let name = target.definition.name.clone();
        self.store_model_device(&target, device).await;
        let note = match self.load_configured_post_processor().await {
            Ok(()) => {
                info!("Post-processor {name} reloaded on {device}");
                String::new()
            }
            Err(e) => {
                warn!("Post-processor {name} device set to {device} but not reloaded: {e}");
                format!(" (not reloaded: {e})")
            }
        };
        self.publish_settings_changed("post_processor");
        self.model_device_response(
            PipelineStage::PostProcessor,
            &target,
            format!("Device for {name} set to {device}{note}"),
        )
        .await
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

/// Refuse a device the model cannot run on at all.
///
/// Only the manifest is consulted: an online model has no local device, and
/// a model declaring only `cpu` cannot be sent to the GPU. Whether *this
/// host* has the accelerator is deliberately not a rejection — a `gpu`
/// choice on a host without one falls back to the CPU at load time, reported
/// through `resolved_accel`, the same as it always has.
fn model_rejects_device(definition: &ModelDefinition, device: &str) -> Option<DaemonResponse> {
    let name = &definition.name;
    if definition.is_online() {
        return Some(DaemonResponse::error_with_code(
            ErrorCode::InvalidDevice,
            &format!("Model {name} runs on a remote service and has no local device to set."),
        ));
    }
    let declared: Vec<String> = definition
        .supported_devices
        .iter()
        .map(ToString::to_string)
        .collect();
    if declared.iter().any(|d| d == device) {
        return None;
    }
    Some(DaemonResponse::error_with_code(
        ErrorCode::InvalidDevice,
        &format!(
            "Model {name} does not run on {device}; it supports {}.",
            declared.join(", ")
        ),
    ))
}

/// The devices this install can offer a model on this host.
///
/// `declared` is what the *model* can do, `installed_accel` what the
/// *installed build* can do, and `host_devices` what the machine has; only
/// the intersection is offerable. A CUDA-only backend on a host with no
/// NVIDIA GPU installs its CPU asset, and offering a GPU there is the defect
/// this closes. An empty `installed_accel` means the daemon has no record —
/// a local-directory import, an install predating the record, or a WASM
/// component, whose record names a transport rather than an accelerator —
/// and the manifest is then the only available answer. Online models
/// (`none`) offer nothing: there is no local compute.
pub(crate) fn model_available_devices(
    host_devices: &[String],
    declared: &[Device],
    installed_accel: &[String],
) -> Vec<String> {
    if declared.contains(&Device::None) {
        return Vec::new();
    }
    let installed_accel: Vec<&String> = installed_accel.iter().filter(|a| *a != "wasm").collect();
    let accelerated =
        installed_accel.is_empty() || installed_accel.iter().any(|a| a.as_str() != "cpu");
    let mut offered: Vec<String> = declared
        .iter()
        .map(ToString::to_string)
        .filter(|d| host_devices.contains(d))
        .filter(|d| d == "cpu" || accelerated)
        .collect();
    offered.dedup();
    offered
}

/// The devices a backend can be run on here: the union of what its models
/// are offered, in the `cpu`, `gpu` order every device list uses.
pub(crate) fn backend_available_devices(
    per_model: impl IntoIterator<Item = Vec<String>>,
) -> Vec<String> {
    let mut devices: Vec<String> = per_model.into_iter().flatten().collect();
    devices.sort();
    devices.dedup();
    devices
}

/// Collapse a device label onto the `cpu`/`gpu` axis the preference is
/// expressed in.
///
/// Everything a client sets is a preference; everything the daemon records as
/// *actual* is the accelerator that preference resolved to. The two are only
/// comparable here — `remote` and anything unrecognized stay themselves, since
/// neither is a local accelerator that a `gpu` preference could have produced.
fn preference_axis(device: &str) -> &str {
    match device {
        "cuda" | "rocm" | "metal" | "vulkan" => "gpu",
        other => other,
    }
}

/// Whether a requested device switch is already in effect, and so has nothing
/// to do.
///
/// The actual device is compared on the preference axis, because it is the
/// accelerator the preference resolved to: a `gpu` request against a daemon
/// already running on `cuda` is asking for what it already has, and reloading
/// the model to grant it costs tens of seconds and a full VRAM churn for no
/// change. A `gpu` preference that fell back to `cpu` still differs, so it is
/// retried — which is the point of tracking preferred and actual separately.
fn switch_is_satisfied(current_preferred: &str, current_actual: &str, requested: &str) -> bool {
    current_preferred == requested && preference_axis(current_actual) == requested
}

/// The message a completed device switch reports.
///
/// Only a GPU request that genuinely landed on the CPU is a fallback; one that
/// landed on an accelerator did exactly what was asked, whatever that
/// accelerator is called.
fn device_switch_message(requested: &str, actual_device: &str) -> String {
    if requested == "gpu" && preference_axis(actual_device) == "cpu" {
        "Device switch requested to GPU, but fell back to CPU: no usable accelerator".to_string()
    } else {
        format!("Successfully switched to {actual_device} device")
    }
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

    /// A `gpu` preference and the accelerator it resolved to are the same
    /// choice spelled on two axes. Comparing them raw makes the early return
    /// unreachable on every GPU host, so a model switch that stages `gpu`
    /// against a daemon already on CUDA unloads the running model and reloads
    /// it on the same GPU — tens of seconds and a full VRAM churn — before the
    /// model switch it was asked for even begins.
    #[test]
    fn a_switch_to_the_accelerator_already_in_use_has_nothing_to_do() {
        for actual in ["cuda", "rocm", "metal", "vulkan", "gpu"] {
            assert!(
                switch_is_satisfied("gpu", actual, "gpu"),
                "gpu preference already resolved to {actual}"
            );
        }
        assert!(switch_is_satisfied("cpu", "cpu", "cpu"));
    }

    /// The deliberate exception the mapping must preserve: a `gpu` preference
    /// that fell back to the CPU is *not* satisfied, so asking for it again
    /// forces the retry.
    #[test]
    fn a_gpu_preference_that_fell_back_to_the_cpu_is_retried() {
        assert!(!switch_is_satisfied("gpu", "cpu", "gpu"));
        assert!(!switch_is_satisfied("cpu", "cpu", "gpu"));
        assert!(!switch_is_satisfied("gpu", "cuda", "cpu"));
    }

    /// A GPU switch that landed on an accelerator succeeded; reporting a
    /// fallback to CPU on every working GPU host tells the user their machine
    /// failed when it did exactly what they asked.
    #[test]
    fn a_successful_gpu_switch_does_not_report_a_fallback() {
        for actual in ["cuda", "rocm", "metal", "vulkan"] {
            assert_eq!(
                device_switch_message("gpu", actual),
                format!("Successfully switched to {actual} device"),
                "resolved to {actual}"
            );
        }
        assert_eq!(
            device_switch_message("cpu", "cpu"),
            "Successfully switched to cpu device"
        );
    }

    /// A GPU switch that really did land on the CPU still says so.
    #[test]
    fn a_gpu_switch_that_fell_back_says_so() {
        assert_eq!(
            device_switch_message("gpu", "cpu"),
            "Device switch requested to GPU, but fell back to CPU: no usable accelerator"
        );
    }

    /// The mapping itself: every resolved accelerator collapses onto `gpu`,
    /// and nothing else moves.
    #[test]
    fn every_accelerator_collapses_onto_the_gpu_preference() {
        for accel in ["cuda", "rocm", "metal", "vulkan"] {
            assert_eq!(preference_axis(accel), "gpu", "{accel}");
        }
        assert_eq!(preference_axis("gpu"), "gpu");
        assert_eq!(preference_axis("cpu"), "cpu");
        assert_eq!(preference_axis("remote"), "remote");
        assert_eq!(preference_axis("unknown"), "unknown");
    }

    fn both() -> Vec<String> {
        vec!["cpu".to_string(), "gpu".to_string()]
    }

    /// The narrowing that makes a per-model list worth asking for: a model
    /// declaring `gpu` on a backend whose installed asset is CPU-only can run
    /// on no GPU here, whatever the host has.
    #[test]
    fn a_cpu_only_install_offers_no_gpu() {
        assert_eq!(
            model_available_devices(&both(), &[Device::Cpu, Device::Gpu], &["cpu".to_string()]),
            vec!["cpu".to_string()]
        );
        assert_eq!(
            model_available_devices(&both(), &[Device::Gpu], &["cpu".to_string()]),
            Vec::<String>::new(),
            "a GPU-only model on a CPU-only install runs nowhere"
        );
    }

    /// A GPU install on a GPU host offers what the model declares, and the
    /// host list still caps it: no GPU on the host, no GPU offered, even with
    /// a GPU asset installed.
    #[test]
    fn the_host_caps_what_the_install_can_offer() {
        let cuda = vec!["cuda".to_string()];
        assert_eq!(
            model_available_devices(&both(), &[Device::Cpu, Device::Gpu], &cuda),
            both()
        );
        assert_eq!(
            model_available_devices(&["cpu".to_string()], &[Device::Cpu, Device::Gpu], &cuda),
            vec!["cpu".to_string()]
        );
        assert_eq!(
            model_available_devices(&both(), &[Device::Cpu], &cuda),
            vec!["cpu".to_string()],
            "a CPU-only model is not offered the GPU"
        );
    }

    /// No record, or a WASM record naming a transport rather than an
    /// accelerator, leaves the manifest as the only answer.
    #[test]
    fn without_an_accelerator_record_the_manifest_answers() {
        assert_eq!(
            model_available_devices(&both(), &[Device::Cpu, Device::Gpu], &[]),
            both()
        );
        assert_eq!(
            model_available_devices(&both(), &[Device::Cpu, Device::Gpu], &["wasm".to_string()]),
            both()
        );
    }

    /// An online model has no local device to offer.
    #[test]
    fn an_online_model_offers_nothing() {
        assert_eq!(
            model_available_devices(&both(), &[Device::None], &[]),
            Vec::<String>::new()
        );
    }

    /// A backend's list is the union of its models' lists: a CPU-only model
    /// beside a GPU-capable one gives both, an online model contributes
    /// nothing, and the order is always `cpu` then `gpu`.
    #[test]
    fn a_backends_devices_are_the_union_of_its_models() {
        assert_eq!(
            backend_available_devices([
                vec!["gpu".to_string()],
                vec!["cpu".to_string()],
                Vec::new(),
                both(),
            ]),
            both()
        );
        assert_eq!(
            backend_available_devices([Vec::new(), Vec::new()]),
            Vec::<String>::new(),
            "a backend of online models runs on nothing local"
        );
        assert_eq!(backend_available_devices([]), Vec::<String>::new());
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
            role: super_stt_registry_types::manifest::ModelRole::Transcription,
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
