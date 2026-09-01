// SPDX-License-Identifier: GPL-3.0-only

mod install;
mod registry;

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{
    clear_active_backend, get_gpu_info, set_active_backend, set_device, set_model,
    unload_active_model,
};
use crate::state::{ContextPage, DaemonStatus, ModelsTab};
use crate::ui::messages::{DownloadMessage, Message, ModelMessage, ModelsPageMessage};
use cosmic::prelude::*;
use log::debug;

impl AppModel {
    /// Models-page UI: tab switch, per-backend dropdown / GPU / select, the
    /// configuration sub-view, and the (UI-only) download actions.
    pub(in crate::core::app) fn handle_models_page_messages(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::ModelsTabActivated(_)
            | ModelsPageMessage::StageActiveModel(_)
            | ModelsPageMessage::StageActiveDevice(_)
            | ModelsPageMessage::LoadStagedModel
            | ModelsPageMessage::UnloadActiveModel => self.handle_models_tab_selection(message),

            ModelsPageMessage::OpenBackendConfig(_)
            | ModelsPageMessage::CloseBackendConfig
            | ModelsPageMessage::SelectBackend(_)
            | ModelsPageMessage::BackendSelectFailed { .. }
            | ModelsPageMessage::StagedModelLoadFailed { .. }
            | ModelsPageMessage::DeselectBackend
            | ModelsPageMessage::ActiveBackendLoaded(_)
            | ModelsPageMessage::RefreshGpuInfo
            | ModelsPageMessage::GpuInfoLoaded(_)
            | ModelsPageMessage::ToggleInstalledMenu(_)
            | ModelsPageMessage::CloseInstalledMenu => self.handle_models_backend_config(message),

            ModelsPageMessage::InstallBackend(_)
            | ModelsPageMessage::InstallBackendFromRepoUrl(_)
            | ModelsPageMessage::InstallAccepted { .. }
            | ModelsPageMessage::InstallFailedToStart { .. }
            | ModelsPageMessage::UpdateBackend(_) => self.handle_models_install_lifecycle(message),

            ModelsPageMessage::UninstallBackend(_) | ModelsPageMessage::UninstallFailed { .. } => {
                self.handle_models_uninstall(message)
            }

            ModelsPageMessage::InstallProgress { .. }
            | ModelsPageMessage::InstallCompleted { .. }
            | ModelsPageMessage::InstallFailed { .. } => {
                self.handle_models_install_progress(message)
            }

            ModelsPageMessage::RefreshRegistry
            | ModelsPageMessage::RegistryListLoaded(_)
            | ModelsPageMessage::RegistryListFailed(_)
            | ModelsPageMessage::RegistrySearchChanged(_)
            | ModelsPageMessage::RegistryIncludeIncompatible(_)
            | ModelsPageMessage::RegistryOnlineFilter(_)
            | ModelsPageMessage::RegistryRoleFilter(_)
            | ModelsPageMessage::InstalledOnlineFilter(_)
            | ModelsPageMessage::InstalledRoleFilter(_)
            | ModelsPageMessage::ImportBackendFromDir
            | ModelsPageMessage::ImportBackendFromDirPicked(_)
            | ModelsPageMessage::RegistryCustomRepoInputChanged(_) => {
                self.handle_models_registry(message)
            }
        }
    }

    fn handle_models_tab_selection(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::ModelsTabActivated(entity) => {
                self.models_page.models_tabs.activate(entity);
                // Trigger initial registry fetch when the Download tab is opened
                // for the first time (backends empty and no prior refresh attempt).
                let switched_to_download = self
                    .models_page
                    .models_tabs
                    .data::<ModelsTab>(entity)
                    .is_some_and(|t| *t == ModelsTab::Download);
                if switched_to_download
                    && self.registry.backends.is_empty()
                    && self.registry.last_refresh.is_none()
                {
                    return crate::core::app::handlers::tasks::fetch_registry_catalog(false);
                }
                Task::none()
            }

            ModelsPageMessage::StageActiveModel(model) => {
                // Stage the pick. The Load button reads `staged_model` /
                // `staged_device` and only then calls the daemon.
                let source = self.models_page.active_backend.clone();
                let device = staged_default_device(&self.backends, source.as_deref(), &model);
                self.models_page.staged_model = Some(model.clone());
                self.models_page.staged_device = device;
                // Fetch the per-model language block at selection time so the
                // active-backend card can show the language control before Load.
                // Wire point 2: model staged (StageActiveModel).
                if let Some(src) = source {
                    self.load_model_language(src, model)
                } else {
                    Task::none()
                }
            }

            ModelsPageMessage::StageActiveDevice(device) => {
                self.models_page.staged_device = Some(device);
                Task::none()
            }

            ModelsPageMessage::LoadStagedModel => self.handle_load_staged_model(),

            ModelsPageMessage::UnloadActiveModel => {
                // Drop the loaded model but keep the backend selected.
                // Optimistic clear so the UI returns to the staged-pickers
                // state immediately; the daemon's `ready` event with
                // `model_loaded: false` is the source of truth.
                self.clear_loaded_model();
                self.models_page.staged_model = None;
                self.models_page.staged_device = None;
                self.model_operation_state = ModelOperationState::Ready;
                Task::perform(unload_active_model(), |result| match result {
                    Ok(_) => cosmic::Action::None,
                    Err(e) => {
                        cosmic::Action::App(Message::Model(ModelMessage::ModelError(e.to_string())))
                    }
                })
            }

            _ => Task::none(),
        }
    }

    fn handle_load_staged_model(&mut self) -> Task<cosmic::Action<Message>> {
        let Some(source) = self.models_page.active_backend.clone() else {
            log::warn!("LoadStagedModel ignored — no active backend");
            return Task::none();
        };
        let Some(model) = self.models_page.staged_model.clone() else {
            log::warn!("LoadStagedModel ignored — no staged model");
            return Task::none();
        };
        if !self.is_model_ready() {
            log::warn!("Model operation already in progress — ignoring Load click");
            return Task::none();
        }

        let device_to_set = match staged_load_device(
            &self.backends,
            &source,
            &model,
            self.models_page.staged_device.as_deref(),
            &self.current_device,
        ) {
            StagedLoad::NotInCatalog => {
                log::warn!(
                    "LoadStagedModel ignored — backend/model not in catalog: \
                     source={source}, model={model}"
                );
                return Task::none();
            }
            StagedLoad::Switch { device_to_set } => device_to_set,
        };

        // Capture the pre-switch device so a failed switch rolls it back rather
        // than leaving the UI on a device the daemon never adopted (audit
        // Tier 3 #37).
        let prev_device = self.current_device.clone();
        self.set_model_loading(model.clone(), "Initiating model switch...".to_string());
        if let Some(dev) = &device_to_set {
            self.current_device.clone_from(dev);
        }

        let model_label = model.clone();
        let source_label = source.clone();
        Task::batch([
            Task::perform(
                async move {
                    if let Some(dev) = device_to_set {
                        set_device(dev).await?;
                    }
                    set_model(model, source).await.map(|_| ())
                },
                move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::Model(ModelMessage::ModelChanged {
                        model: model_label.clone(),
                        source: source_label.clone(),
                    })),
                    Err(e) => cosmic::Action::App(Message::ModelsPage(
                        ModelsPageMessage::StagedModelLoadFailed {
                            prev_device: prev_device.clone(),
                            message: e.to_string(),
                        },
                    )),
                },
            ),
            Task::perform(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                },
                |()| cosmic::Action::App(Message::Download(DownloadMessage::CheckDownloadStatus)),
            ),
        ])
    }

    fn handle_models_backend_config(
        &mut self,
        message: ModelsPageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelsPageMessage::OpenBackendConfig(source) => {
                // Open the per-backend configuration as a right-side sheet over
                // the current list (active card or Installed tab), instead of a
                // full-page takeover. Also closes the card's overflow menu.
                self.models_page.configure_backend = Some(source);
                self.context_page = ContextPage::ConfigureBackend;
                self.core.window.show_context = true;
                self.models_page.installed_menu_open = None;
                // Start the sheet without a stale save-error banner.
                self.action_error = None;
                Task::none()
            }

            ModelsPageMessage::CloseBackendConfig => {
                self.models_page.configure_backend = None;
                self.core.window.show_context = false;
                self.action_error = None;
                Task::none()
            }

            ModelsPageMessage::ActiveBackendLoaded(source) => {
                self.models_page.active_backend = source;
                Task::none()
            }

            ModelsPageMessage::RefreshGpuInfo => {
                // Periodic poll — only query when connected so the disconnected
                // state doesn't spam failing requests.
                if self.daemon_status == DaemonStatus::Connected {
                    Task::perform(get_gpu_info(), |result| {
                        cosmic::Action::App(Message::ModelsPage(ModelsPageMessage::GpuInfoLoaded(
                            result.unwrap_or_default(),
                        )))
                    })
                } else {
                    Task::none()
                }
            }

            ModelsPageMessage::GpuInfoLoaded(gpus) => {
                debug!("GpuInfoLoaded: storing {} GPU(s) in app state", gpus.len());
                self.gpu_info = gpus;
                Task::none()
            }

            ModelsPageMessage::SelectBackend(source) => {
                // Select the backend WITHOUT loading a model — the card moves
                // to the top fixed header, any model from a different backend
                // is unloaded. (`set_model` is the way to also load a model.)
                if self.models_page.active_backend.as_deref() == Some(source.as_str()) {
                    return Task::none();
                }
                // Capture the prior selection so a failed activation rolls the
                // card back instead of showing a backend the daemon rejected
                // (audit Tier 3 #37).
                let prev_active = self.models_page.active_backend.take();
                self.models_page.active_backend = Some(source.clone());
                self.clear_loaded_model();
                self.model_operation_state = ModelOperationState::Ready;
                // Activation comes from the Models page's "Select a backend" sheet;
                // dismiss it now that a choice was made.
                self.core.window.show_context = false;
                Task::perform(set_active_backend(source), move |result| match result {
                    Ok(()) => cosmic::Action::None,
                    Err(e) => cosmic::Action::App(Message::ModelsPage(
                        ModelsPageMessage::BackendSelectFailed {
                            prev_active: prev_active.clone(),
                            message: e.to_string(),
                        },
                    )),
                })
            }

            ModelsPageMessage::BackendSelectFailed {
                prev_active,
                message,
            } => {
                self.models_page.active_backend = prev_active;
                self.set_model_error(&message);
                Task::none()
            }

            ModelsPageMessage::StagedModelLoadFailed {
                prev_device,
                message,
            } => {
                self.current_device = prev_device;
                self.set_model_error(&message);
                Task::none()
            }

            ModelsPageMessage::DeselectBackend => {
                // Optimistically clear the active backend + loaded model; the
                // daemon goes idle. (Rejected only mid-recording — an edge case
                // that self-heals on the next refresh.)
                self.models_page.active_backend = None;
                self.clear_loaded_model();
                self.model_operation_state = ModelOperationState::Ready;
                self.models_page.configure_backend = None;
                // Close the configuration sheet if it was open for this backend.
                self.core.window.show_context = false;
                Task::perform(clear_active_backend(), |_| cosmic::Action::None)
            }

            ModelsPageMessage::ToggleInstalledMenu(source) => {
                // Toggle this card's overflow menu; opening one closes any other.
                if self.models_page.installed_menu_open.as_deref() == Some(source.as_str()) {
                    self.models_page.installed_menu_open = None;
                } else {
                    self.models_page.installed_menu_open = Some(source);
                }
                Task::none()
            }

            ModelsPageMessage::CloseInstalledMenu => {
                self.models_page.installed_menu_open = None;
                Task::none()
            }

            _ => Task::none(),
        }
    }
}

/// The device a freshly staged model starts on: the first device this install
/// can actually offer for it, or `None` when it can offer none.
///
/// The narrowing is what makes `None` reachable, and `None` is what the Load
/// button reads: a model declaring `gpu` on a backend whose installed asset is
/// CPU-only can run on no device here, and staging one from the manifest would
/// leave Load enabled next to the advisory saying so — sending a `set_device`
/// that persists a GPU preference the user was just told is unusable. Seeding
/// from the offered list also keeps the staged device inside the set the
/// dropdown renders, so the picker never shows an unselected value.
fn staged_default_device(
    backends: &[crate::daemon::backends::BackendInfo],
    source: Option<&str>,
    model: &str,
) -> Option<String> {
    let source = source?;
    let backend = backends.iter().find(|b| b.source == source)?;
    crate::ui::views::models::offered_devices(backend, model)
        .first()
        .cloned()
}

/// What a Load click resolves to.
#[derive(Debug, PartialEq, Eq)]
enum StagedLoad {
    /// The staged `(source, model)` is no longer in the installed-backend
    /// catalog — the click is stale and must not reach the daemon.
    NotInCatalog,
    /// Switch to the staged model, first setting the device when `Some`.
    Switch { device_to_set: Option<String> },
}

/// Resolve a Load click against the installed-backend catalog.
///
/// The catalog check is not merely an optimization. A backend can be
/// uninstalled — or the catalog refreshed — between staging a model and
/// clicking Load. Without it, a miss reads as "not online", the staged device
/// is sent as a real `set_device`, and the daemon unloads the working model,
/// reloads it on the new device, and *persists* that device — before the
/// `set_model` that follows fails with `invalid_model`. The user ends up on a
/// device they never chose, from a click that could not have succeeded.
///
/// For online models (the `none` sentinel in `supported_devices`) there is no
/// device to set; otherwise the staged device is sent only when it actually
/// differs from the current one, compared on the preference axis (see
/// [`preference_axis`]).
fn staged_load_device(
    backends: &[crate::daemon::backends::BackendInfo],
    source: &str,
    model: &str,
    staged_device: Option<&str>,
    current_device: &str,
) -> StagedLoad {
    let Some(online) = backends
        .iter()
        .find(|b| b.source == source)
        .and_then(|b| b.models.iter().find(|m| m.name == model))
        .map(|m| m.supported_devices.iter().any(|d| d == "none"))
    else {
        return StagedLoad::NotInCatalog;
    };
    let device_to_set = if online {
        None
    } else {
        staged_device
            .filter(|d| *d != "none" && preference_axis(d) != preference_axis(current_device))
            .map(ToString::to_string)
    };
    StagedLoad::Switch { device_to_set }
}

/// Collapse a device label onto the `cpu`/`gpu` axis the preference is
/// expressed in.
///
/// A staged device is a preference (`set_device` takes `cpu` or `gpu`), while
/// `current_device` is the accelerator that preference resolved to — `cuda` on
/// an NVIDIA host. The two are only comparable here: raw, a staged `gpu` never
/// equals a current `cuda`, so every GPU model switch resends the preference
/// and the daemon unloads and reloads the running model on the same GPU before
/// the model switch begins. A `gpu` preference that fell back to `cpu` still
/// differs on this axis, so retrying the accelerator keeps working.
fn preference_axis(device: &str) -> &str {
    match device {
        "cuda" | "rocm" | "metal" | "vulkan" => "gpu",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{StagedLoad, staged_default_device, staged_load_device};
    use crate::daemon::backends::{BackendInfo, BackendModel};

    fn backend(source: &str, model: &str, devices: &[&str]) -> BackendInfo {
        BackendInfo {
            source: source.to_string(),
            description: String::new(),
            name: "Test".to_string(),
            version: "1.0.0".to_string(),
            kind: "wasm".to_string(),
            allowed_hosts: Vec::new(),
            installed_accel: Vec::new(),
            models: vec![BackendModel {
                name: model.to_string(),
                provider: String::new(),
                supported_devices: devices.iter().map(|d| (*d).to_string()).collect(),
                estimated_vram_bytes: 0,
                multilingual: false,
                supported_languages: Vec::new(),
                primary_language: String::new(),
                realtime: false,
                role: "transcription".into(),
            }],
            secrets: Vec::new(),
            options: Vec::new(),
        }
    }

    /// The regression: a Load click for a pair that is no longer installed
    /// must not reach the daemon at all. Treating the catalog miss as merely
    /// "not online" sends a real `set_device` first, which unloads the working
    /// model and persists a device the user never chose — and only then does
    /// the `set_model` fail with `invalid_model`.
    #[test]
    fn a_stale_pair_sends_nothing() {
        let installed = vec![backend(
            "github.com/super-stt/whisper",
            "whisper-tiny",
            &["cuda"],
        )];

        // Backend uninstalled between staging and the click.
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-stt/gone",
                "whisper-tiny",
                Some("cuda"),
                "cpu"
            ),
            StagedLoad::NotInCatalog,
        );
        // Backend still installed, but no longer serving that model.
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-stt/whisper",
                "whisper-large",
                Some("cuda"),
                "cpu"
            ),
            StagedLoad::NotInCatalog,
        );
        // Nothing installed at all.
        assert_eq!(
            staged_load_device(
                &[],
                "github.com/super-stt/whisper",
                "whisper-tiny",
                Some("cuda"),
                "cpu"
            ),
            StagedLoad::NotInCatalog,
        );
    }

    /// A staged local model on a different device still sets it — the guard
    /// must not swallow the case it sits in front of.
    #[test]
    fn a_local_model_on_a_new_device_sets_it() {
        let installed = vec![backend(
            "github.com/super-stt/whisper",
            "whisper-tiny",
            &["cpu", "cuda"],
        )];
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-stt/whisper",
                "whisper-tiny",
                Some("cuda"),
                "cpu"
            ),
            StagedLoad::Switch {
                device_to_set: Some("cuda".to_string())
            },
        );
    }

    /// No `set_device` when it would be a no-op: the daemon is already on that
    /// device, or none was staged.
    #[test]
    fn an_unchanged_device_is_not_resent() {
        let installed = vec![backend(
            "github.com/super-stt/whisper",
            "whisper-tiny",
            &["cpu", "cuda"],
        )];
        for staged in [Some("cpu"), None] {
            assert_eq!(
                staged_load_device(
                    &installed,
                    "github.com/super-stt/whisper",
                    "whisper-tiny",
                    staged,
                    "cpu"
                ),
                StagedLoad::Switch {
                    device_to_set: None
                },
                "staged={staged:?} must not resend the current device"
            );
        }
    }

    /// Online models carry the `none` sentinel and have no device to set;
    /// a stale `none` staged against a local model is likewise never sent.
    #[test]
    fn an_online_model_sets_no_device() {
        let online = vec![backend(
            "github.com/super-stt/openai",
            "whisper-1",
            &["none"],
        )];
        assert_eq!(
            staged_load_device(
                &online,
                "github.com/super-stt/openai",
                "whisper-1",
                Some("none"),
                "cpu"
            ),
            StagedLoad::Switch {
                device_to_set: None
            },
        );
        // Even with a real device staged, an online model takes no device.
        assert_eq!(
            staged_load_device(
                &online,
                "github.com/super-stt/openai",
                "whisper-1",
                Some("cuda"),
                "cpu"
            ),
            StagedLoad::Switch {
                device_to_set: None
            },
        );
        let local = vec![backend(
            "github.com/super-stt/whisper",
            "whisper-tiny",
            &["cpu"],
        )];
        assert_eq!(
            staged_load_device(
                &local,
                "github.com/super-stt/whisper",
                "whisper-tiny",
                Some("none"),
                "cpu"
            ),
            StagedLoad::Switch {
                device_to_set: None
            },
        );
    }

    /// A staged `gpu` against a daemon already on CUDA is the *same* choice
    /// spelled two ways: `set_device` takes the `cpu`/`gpu` preference, while
    /// the current device is the accelerator that preference resolved to.
    /// Comparing them raw makes every GPU model switch resend the preference,
    /// which unloads the running model and reloads it on the same GPU before
    /// the model switch even starts.
    #[test]
    fn a_staged_gpu_is_not_resent_to_a_daemon_already_on_an_accelerator() {
        let installed = vec![backend(
            "github.com/super-stt/whisper",
            "whisper-tiny",
            &["cpu", "gpu"],
        )];
        for current in ["cuda", "rocm", "metal", "vulkan", "gpu"] {
            assert_eq!(
                staged_load_device(
                    &installed,
                    "github.com/super-stt/whisper",
                    "whisper-tiny",
                    Some("gpu"),
                    current
                ),
                StagedLoad::Switch {
                    device_to_set: None
                },
                "current={current} is already the gpu preference"
            );
        }
    }

    /// The deliberate exception: a `gpu` preference that fell back to the CPU
    /// still differs on the preference axis, so Load retries the accelerator.
    #[test]
    fn a_staged_gpu_is_resent_when_the_daemon_actually_fell_back_to_the_cpu() {
        let installed = vec![backend(
            "github.com/super-stt/whisper",
            "whisper-tiny",
            &["cpu", "gpu"],
        )];
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-stt/whisper",
                "whisper-tiny",
                Some("gpu"),
                "cpu"
            ),
            StagedLoad::Switch {
                device_to_set: Some("gpu".to_string())
            },
        );
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-stt/whisper",
                "whisper-tiny",
                Some("cpu"),
                "cuda"
            ),
            StagedLoad::Switch {
                device_to_set: Some("cpu".to_string())
            },
        );
    }

    fn backend_with_accel(devices: &[&str], installed_accel: &[&str]) -> Vec<BackendInfo> {
        let mut b = backend("github.com/super-stt/voxtral", "voxtral-mini", devices);
        b.installed_accel = installed_accel.iter().map(|a| (*a).to_string()).collect();
        vec![b]
    }

    /// The reported bug, on the staging path: a GPU-only model on an install
    /// that resolved to a CPU asset can run on nothing here. Staging must
    /// leave the device unset, because that is the only thing standing
    /// between the "needs a device this install doesn't have" advisory and a
    /// Load button that would happily persist an unusable GPU preference.
    #[test]
    fn a_gpu_only_model_on_a_cpu_install_stages_no_device() {
        let installed = backend_with_accel(&["gpu"], &["cpu"]);
        assert_eq!(
            staged_default_device(
                &installed,
                Some("github.com/super-stt/voxtral"),
                "voxtral-mini"
            ),
            None,
        );
    }

    /// Same root cause, quieter symptom: a model declaring `["gpu", "cpu"]`
    /// on a CPU-only install can run — on the CPU. Seeding from the manifest
    /// stages `gpu`, which is not in the offered list, so the dropdown renders
    /// with nothing selected while Load sends a device the install cannot use.
    /// The staged device must always be one the picker actually offers.
    #[test]
    fn a_gpu_first_model_on_a_cpu_install_stages_the_cpu() {
        let installed = backend_with_accel(&["gpu", "cpu"], &["cpu"]);
        let staged = staged_default_device(
            &installed,
            Some("github.com/super-stt/voxtral"),
            "voxtral-mini",
        );
        assert_eq!(staged, Some("cpu".to_string()));
        assert!(
            crate::ui::views::models::offered_devices(&installed[0], "voxtral-mini")
                .contains(&staged.expect("staged")),
            "the staged device must be one the picker offers"
        );
    }

    /// With an accelerated asset installed, the model's first device is staged
    /// as before — the narrowing only removes what this install cannot do.
    #[test]
    fn an_accelerated_install_stages_the_models_first_device() {
        let installed = backend_with_accel(&["gpu", "cpu"], &["cuda"]);
        assert_eq!(
            staged_default_device(
                &installed,
                Some("github.com/super-stt/voxtral"),
                "voxtral-mini"
            ),
            Some("gpu".to_string()),
        );
    }

    /// An online model has no local device, and an unknown backend/model has
    /// no answer at all — both stage nothing rather than guessing.
    #[test]
    fn an_online_or_unknown_model_stages_no_device() {
        let online = backend_with_accel(&["none"], &[]);
        assert_eq!(
            staged_default_device(
                &online,
                Some("github.com/super-stt/voxtral"),
                "voxtral-mini"
            ),
            None,
        );
        let installed = backend_with_accel(&["cpu"], &["cpu"]);
        assert_eq!(
            staged_default_device(&installed, None, "voxtral-mini"),
            None,
            "no active backend stages nothing"
        );
        assert_eq!(
            staged_default_device(
                &installed,
                Some("github.com/super-stt/gone"),
                "voxtral-mini"
            ),
            None,
        );
    }
}
