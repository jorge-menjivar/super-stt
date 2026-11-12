// SPDX-License-Identifier: GPL-3.0-only
//! The framework a model runs on.
//!
//! A framework is a property of a model, not of the daemon: a small voice runs
//! fine on the CPU while the large one beside it needs the GPU. So the choice
//! is stored per `(source, model)` ([`crate::config::ModelSettings::device`]),
//! and a model nobody chose one for runs on the best framework its installed
//! build runs. There is no global default.
//!
//! Which handler a request lands in decides what setting a device *means*: for
//! the model that is currently loaded it is a reload onto the new device, for
//! any other model it is a note its next load picks up. Both answer with the
//! same `{ device, resolved_accel, available_devices }` body so a client never
//! has to know which case it hit.

use crate::tts::daemon::types::{SuperTTSDaemon, normalize_device};
use crate::tts::tts_models::ModelDefinition;
use crate::tts::tts_models::backends;
use log::{info, warn};
pub(crate) use super_engine_daemon::devices::{
    backend_available_devices, backend_frameworks, model_available_devices, parse_device_preference,
};
use super_engine_daemon::devices::{
    build_installed, device_rejection, device_switch_message, effective_framework,
    switch_is_satisfied,
};
use super_engine_daemon::stage::{DeviceSwitch, ModelId, StageExt};
use super_voice_registry_types::manifest::Device;
use super_voice_shared::tts::models::protocol::{
    Command, DaemonResponse, ErrorCode, StageModelDevice,
};

/// The model a per-model device command resolved to: its definition, and the
/// install directory whose record says which accelerators the installed build
/// actually has.
struct DeviceTarget {
    definition: ModelDefinition,
    backend_dir: std::path::PathBuf,
}

/// What narrows a model's declared devices to the frameworks it can run on
/// here, and says which of them the build on disk runs. See
/// [`model_available_devices`].
pub(crate) struct InstallContext {
    /// Every framework the backend's builds run on here, installed or not.
    frameworks: Vec<String>,
    installed_accel: Vec<String>,
}

impl InstallContext {
    /// The frameworks `model` can be offered on this host.
    fn offer(&self, model: &ModelDefinition) -> Vec<String> {
        model_available_devices(&model.supported_devices, &self.frameworks)
    }

    /// Whether the build on disk runs `framework`.
    fn installed(&self, framework: &str) -> bool {
        build_installed(framework, &self.installed_accel)
    }

    /// The framework `model` loads on: `chosen`, its own, when it can run on
    /// it here and that build is installed, else the best one the build on
    /// disk runs. `None` when nothing is offered.
    pub(crate) fn effective(
        &self,
        model: &ModelDefinition,
        chosen: Option<&str>,
    ) -> Option<String> {
        effective_framework(chosen, &self.offer(model), &self.installed_accel)
    }
}

impl SuperTTSDaemon {
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
    /// of its own answers with the framework it will actually load on, not
    /// with a null a client would have to know to resolve itself — and would
    /// resolve differently from the load path the first time the two
    /// disagreed.
    pub(crate) async fn handle_get_model_device(&self, model: String) -> DaemonResponse {
        let target = match self.resolve_device_target(&model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        let device = self.effective_device(&target).await.unwrap_or_default();
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

        // Validate and normalize (`cuda`/`metal` → `gpu`) in one step, so
        // everything downstream stores and threads `cpu`/`gpu` rather than the
        // raw input the client sent. Emit the documented `400 invalid_device`
        // so a bad request is distinguishable from a server failure.
        let Some(device) = parse_device_preference(&device) else {
            warn!("Invalid device specified: {device}");
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidDevice,
                &format!(
                    "Invalid device '{device}'. Must be a framework: cpu, cuda, rocm, metal or vulkan"
                ),
            );
        };

        let target = match self.resolve_device_target(&model).await {
            Ok(target) => target,
            Err(early_return) => return early_return,
        };
        if let Some(rejection) = model_rejects_device(&target.definition, &device) {
            return rejection;
        }
        // A framework the backend has a build for, but not the one on disk:
        // that build is installed first, through the registry. Refused here
        // rather than stored, so a model is never left pointed at a build it
        // does not have.
        let install = self.install_context(&target.backend_dir).await;
        let offered = install.offer(&target.definition);
        if !offered.contains(&device) {
            return DaemonResponse::error_with_code(
                ErrorCode::InvalidDevice,
                &format!(
                    "{} cannot run on {device} here; it can run on {}.",
                    target.definition.name,
                    offered.join(", ")
                ),
            );
        }
        if !install.installed(&device) {
            return DaemonResponse::error_with_code(
                ErrorCode::BuildNotInstalled,
                &format!(
                    "The {device} build of {} is not installed. Install it with \
                     POST /v1/registry/backend/install, then set the device again.",
                    target.definition.source
                ),
            );
        }

        // Where the model is running now, if it is the loaded one. Only then
        // does the change mean a reload.
        let running_on = self
            .running_device(&target.definition.source, &target.definition.name)
            .await;
        let current = self.effective_device(&target).await.unwrap_or_default();
        let name = target.definition.name.clone();

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
            // have been on this framework with no choice of its own, and the
            // user just made it its own, which is what keeps it there when a
            // better build is installed.
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

        self.switch_device(&target, &device, &current).await
    }

    /// Move the loaded model to `device`, as every stage does it: see
    /// [`StageExt::switch_device`]. `fallback` is the framework its setting
    /// loads it on, where a failed switch puts it back.
    async fn switch_device(
        &self,
        target: &DeviceTarget,
        device: &str,
        fallback: &str,
    ) -> DaemonResponse {
        let name = &target.definition.name;
        let model = ModelId::new(name.clone(), target.definition.source.clone());
        let switched = self
            .stage()
            .switch_device(model, device.to_string(), fallback.to_string())
            .await;
        match switched {
            // Unloaded since the check: the choice is its next load's.
            DeviceSwitch::Recorded => {
                self.model_device_response(
                    target,
                    format!("Device for {name} set to {device}. Its next load will use it."),
                )
                .await
            }
            DeviceSwitch::Switched { device: actual } => {
                self.model_device_response(target, device_switch_message(device, &actual))
                    .await
            }
            DeviceSwitch::Recovered {
                error,
                device: actual,
            } => DaemonResponse::error(&format!(
                "Failed to switch to device '{device}': {error:#}. Reverted to previous device '{actual}'."
            )),
            DeviceSwitch::Failed { error, recovery } => DaemonResponse::error(&format!(
                "Device switch failed: {error:#}. Recovery also failed: {recovery:#}. Stage 1 is now idle."
            )),
            DeviceSwitch::Superseded => super::model_management::superseded("device switch"),
            DeviceSwitch::Cancelled => {
                warn!("Device switch cancelled due to shutdown");
                DaemonResponse::error("Device switch cancelled due to shutdown")
            }
        }
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

    /// The framework the target loads on: its own when it can run on it here
    /// and that build is installed; else the best one the build on disk runs.
    /// A client staging a load from this answer never asks for a build that is
    /// not installed. `None` when nothing is offered here.
    async fn effective_device(&self, target: &DeviceTarget) -> Option<String> {
        let chosen = self
            .config
            .read()
            .await
            .model_device(&target.definition.source, &target.definition.name)
            .map(ToString::to_string);
        self.install_context(&target.backend_dir)
            .await
            .effective(&target.definition, chosen.as_deref())
    }

    /// Record the target's device and persist it, for the paths that store
    /// without reloading (an unloaded model, or one already on the device).
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
    pub(in crate::tts::daemon) async fn running_device(
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
    pub(in crate::tts::daemon) async fn model_device_view(
        &self,
        source: &str,
        model: &str,
        running: Option<String>,
    ) -> StageModelDevice {
        let found = {
            let backends = self.backends.read().await;
            backends::find_model(&backends, model, source)
                .map(|(backend, def)| (backend.dir.clone(), def.clone()))
        };
        let online = found.as_ref().is_some_and(|(_, def)| def.is_online());
        if online {
            // The manifest's own sentinel: remote compute has no local device,
            // and nothing resolves locally whether it is loaded or not.
            return StageModelDevice {
                preference: Device::None.to_string(),
                resolved_accel: None,
            };
        }
        let chosen = self
            .config
            .read()
            .await
            .model_device(source, model)
            .map(ToString::to_string);
        let preference = match found {
            Some((dir, definition)) => self
                .install_context(&dir)
                .await
                .effective(&definition, chosen.as_deref()),
            None => chosen,
        }
        .unwrap_or_default();
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
    pub(crate) async fn install_context(&self, backend_dir: &std::path::Path) -> InstallContext {
        let host = probe_host().await;
        let installed_accel = crate::registry::installed::read(backend_dir)
            .map(|r| r.selected.accel)
            .unwrap_or_default();
        // The installed manifest names every build the release publishes, so
        // the frameworks need no network. One that does not parse offers what
        // is on disk.
        let builds = super_voice_registry_types::manifest::Manifest::load_installed(backend_dir)
            .map(|m| m.assets.subprocess)
            .unwrap_or_default();
        InstallContext {
            frameworks: backend_frameworks(&host, &builds, &installed_accel),
            installed_accel,
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

/// The host, probed off the async runtime. A probe that fails reads as a
/// machine with nothing but the CPU, rather than failing the request.
async fn probe_host() -> crate::registry::host_detect::Host {
    tokio::task::spawn_blocking(crate::registry::host_detect::detect)
        .await
        .unwrap_or_else(|_| crate::registry::host_detect::Host {
            target_triple: String::new(),
            cuda: None,
            rocm: None,
            vulkan: None,
            metal: None,
        })
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
            provider: None,
            product: super_voice_registry_types::manifest::VoiceModel::new(
                super_voice_registry_types::manifest::ModelRole::Synthesis,
            ),
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
