// SPDX-License-Identifier: GPL-3.0-only

mod install;
mod registry;

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{
    clear_active_backend, get_gpu_info, get_model_device, set_active_backend, set_model,
    set_model_device, unload_active_model,
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
            | ModelsPageMessage::StagedDeviceLoaded { .. }
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
                self.models_page.staged_device.clone_from(&device);
                let Some(src) = source else {
                    return Task::none();
                };
                Task::batch([
                    // Fetch the per-model language block at selection time so
                    // the active-backend card can show the language control
                    // before Load. Wire point 2: model staged (StageActiveModel).
                    self.load_model_language(src, model.clone()),
                    // And the model's own device, so a model the user once put
                    // on the GPU is staged there again rather than on the
                    // first device its manifest lists. Nothing to ask for an
                    // online model, which stages no device at all.
                    if device.is_some() {
                        Self::load_staged_device(model)
                    } else {
                        Task::none()
                    },
                ])
            }

            ModelsPageMessage::StageActiveDevice(device) => {
                self.models_page.staged_device = Some(device);
                Task::none()
            }

            ModelsPageMessage::StagedDeviceLoaded { model, device } => {
                // The answer is for the model staged when it was asked; a pick
                // made since wins, and so does a pick the dropdown cannot show.
                if self.models_page.staged_model.as_deref() == Some(model.as_str())
                    && let Some(source) = self.models_page.active_backend.as_deref()
                    && let Some(backend) = self.backends.iter().find(|b| b.source == source)
                    && crate::ui::views::models::offered_devices(backend, &model)
                        .contains(&device.device)
                {
                    self.models_page.staged_device = Some(device.device);
                }
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
                    // The device is the model's own, set before the load so
                    // the load picks it up. For a model that is not loaded the
                    // daemon only records it; for the one already loaded it is
                    // the reload, and `set_model` then has nothing left to do.
                    if let Some(dev) = device_to_set {
                        set_model_device(STT_STAGE, model.clone(), dev).await?;
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

/// Transcription is stage 1 of the pipeline — the stage a staged model's
/// device is addressed through.
const STT_STAGE: u32 = 1;

impl AppModel {
    /// Ask the daemon which device `model` has, for [`ModelsPageMessage::StagedDeviceLoaded`].
    fn load_staged_device(model: String) -> Task<cosmic::Action<Message>> {
        Task::perform(
            get_model_device(STT_STAGE, model.clone()),
            move |result| match result {
                Ok(device) => cosmic::Action::App(Message::ModelsPage(
                    ModelsPageMessage::StagedDeviceLoaded {
                        model: model.clone(),
                        device,
                    },
                )),
                // The local default is already staged; a failed read only
                // means the model's own choice is not reflected.
                Err(e) => {
                    log::warn!("Could not read the device for {model}: {e}");
                    cosmic::Action::None
                }
            },
        )
    }
}

/// The device a freshly staged model starts on: the first device this install
/// can actually offer for it, or `None` when it can offer none.
///
/// The narrowing is what makes `None` reachable, and `None` is what the Load
/// button reads: a model declaring `gpu` on a backend whose installed asset is
/// CPU-only can run on no device here, and staging one from the manifest would
/// leave Load enabled next to the advisory saying so — sending a device the
/// user was just told is unusable. Seeding from the offered list also keeps the
/// staged device inside the set the dropdown renders, so the picker never
/// shows an unselected value. The model's own recorded device, when it has
/// one, replaces this once the daemon answers.
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
/// A backend can be uninstalled — or the catalog refreshed — between staging a
/// model and clicking Load, and a click for a pair that is gone must not reach
/// the daemon at all: the daemon would refuse both calls, but with two errors
/// for one click. For online models (the `none` sentinel in
/// `supported_devices`) there is no device to set; otherwise the staged device
/// is sent as the model's own. The daemon compares it with where the model
/// already is, so resending an unchanged device costs nothing.
fn staged_load_device(
    backends: &[crate::daemon::backends::BackendInfo],
    source: &str,
    model: &str,
    staged_device: Option<&str>,
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
            .filter(|d| *d != "none")
            .map(ToString::to_string)
    };
    StagedLoad::Switch { device_to_set }
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
    /// must not reach the daemon at all.
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
            ),
            StagedLoad::NotInCatalog,
        );
    }

    /// A staged local model sends its device — the model's own, whatever the
    /// daemon happens to be running now — and none when none is staged.
    #[test]
    fn a_local_model_sends_its_staged_device() {
        let installed = vec![backend(
            "github.com/super-stt/whisper",
            "whisper-tiny",
            &["cpu", "gpu"],
        )];
        for staged in ["gpu", "cpu"] {
            assert_eq!(
                staged_load_device(
                    &installed,
                    "github.com/super-stt/whisper",
                    "whisper-tiny",
                    Some(staged),
                ),
                StagedLoad::Switch {
                    device_to_set: Some(staged.to_string())
                },
            );
        }
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-stt/whisper",
                "whisper-tiny",
                None
            ),
            StagedLoad::Switch {
                device_to_set: None
            },
        );
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
            ),
            StagedLoad::Switch {
                device_to_set: None
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
