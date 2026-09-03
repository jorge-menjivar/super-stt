// SPDX-License-Identifier: GPL-3.0-only

mod install;
mod registry;

use crate::core::app::AppModel;
use crate::daemon::client::{
    clear_active_backend, get_gpu_info, get_model_device, list_model_devices, list_stage_devices,
    set_active_backend, set_model, set_model_device, unload_active_model,
};
use crate::state::device_offers::{STT_STAGE, staged_device};
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
            | ModelsPageMessage::StagedDevicesLoaded { .. }
            | ModelsPageMessage::BackendDevicesLoaded { .. }
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
                //
                // The device is left unset until the daemon answers what this
                // model can run on here: an empty answer is what disables Load
                // and shows the advisory, so staging a device before the
                // answer would be guessing at exactly the case that must not
                // guess.
                let source = self.models_page.active_backend.clone();
                self.models_page.staged_model = Some(model.clone());
                self.models_page.staged_device = None;
                let Some(src) = source else {
                    return Task::none();
                };
                Task::batch([
                    // Fetch the per-model language block at selection time so
                    // the active-backend card can show the language control
                    // before Load. Wire point 2: model staged (StageActiveModel).
                    self.load_model_language(src.clone(), model.clone()),
                    // And the devices it can run on, plus the device it
                    // already has — so a model the user once put on the GPU is
                    // staged there again rather than on the first device the
                    // list happens to start with.
                    Self::load_staged_devices(src, model),
                ])
            }

            ModelsPageMessage::StageActiveDevice(device) => {
                self.models_page.staged_device = Some(device);
                Task::none()
            }

            ModelsPageMessage::StagedDevicesLoaded {
                source,
                model,
                devices,
                current,
            } => {
                // The answer describes the backend that was selected when it
                // was asked. A switch since then makes it about a backend this
                // card no longer shows, and recording it would evict the new
                // selection's own answers.
                if self.models_page.active_backend.as_deref() != Some(source.as_str()) {
                    return Task::none();
                }
                self.device_offers
                    .record(STT_STAGE, source, Some(model.clone()), devices.clone());
                // Likewise for the model: a pick made since the ask wins.
                if self.models_page.staged_model.as_deref() == Some(model.as_str()) {
                    // The model's own device when the picker can show it,
                    // otherwise the first offered — and nothing at all when
                    // none is offered, which is what keeps Load disabled.
                    self.models_page.staged_device = staged_device(&devices, current);
                }
                Task::none()
            }

            ModelsPageMessage::BackendDevicesLoaded { source, devices } => {
                if self.models_page.active_backend.as_deref() == Some(source.as_str()) {
                    self.device_offers.record(STT_STAGE, source, None, devices);
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
                self.model_operations.set_ready(STT_STAGE);
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
        // Stage 1's own operation only: the post-processor provisions
        // independently, and its download must not swallow this click.
        if !self.is_model_ready(STT_STAGE) {
            log::warn!("A stage-1 model operation is already in progress — ignoring Load click");
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
        self.set_model_loading(
            model.clone(),
            "Initiating model switch...".to_string(),
            STT_STAGE,
        );
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
                self.models_page.active_backend.clone_from(&source);
                // The card's device chips are the daemon's answer for the
                // backend now selected, so every selection asks again.
                source.map_or_else(Task::none, Self::load_backend_devices)
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
                self.model_operations.set_ready(STT_STAGE);
                // Activation comes from the Models page's "Select a backend" sheet;
                // dismiss it now that a choice was made.
                self.core.window.show_context = false;
                let selected = source.clone();
                Task::perform(set_active_backend(source), move |result| match result {
                    // Re-announce the selection the daemon just took, which is
                    // what asks it for the backend's devices.
                    Ok(()) => cosmic::Action::App(Message::ModelsPage(
                        ModelsPageMessage::ActiveBackendLoaded(Some(selected.clone())),
                    )),
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
                self.model_operations.set_ready(STT_STAGE);
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

impl AppModel {
    /// Ask the daemon what `model` can run on here and which device it has,
    /// for [`ModelsPageMessage::StagedDevicesLoaded`].
    ///
    /// One task for both reads so the picker and the device it starts on land
    /// together: staging the model's own device means nothing until the list
    /// that has to contain it is known.
    fn load_staged_devices(source: String, model: String) -> Task<cosmic::Action<Message>> {
        let asked = model.clone();
        Task::perform(
            async move {
                let devices = list_model_devices(STT_STAGE, model.clone()).await?;
                // The recorded device is the nicety, not the requirement: a
                // model that has never been loaded has none, and a failed read
                // only costs the restaging.
                let current = get_model_device(STT_STAGE, model)
                    .await
                    .ok()
                    .map(|d| d.device);
                Ok((devices, current))
            },
            move |result: super_stt_shared::daemon::http_client::HttpResult<_>| match result {
                Ok((devices, current)) => cosmic::Action::App(Message::ModelsPage(
                    ModelsPageMessage::StagedDevicesLoaded {
                        source: source.clone(),
                        model: asked.clone(),
                        devices,
                        current,
                    },
                )),
                // Nothing is staged onto a device the daemon never offered, so
                // a failed read leaves the picker hidden and Load disabled
                // rather than guessing at a device.
                Err(e) => {
                    log::warn!("Could not read the devices for {asked}: {e}");
                    cosmic::Action::None
                }
            },
        )
    }

    /// Ask the daemon which devices the active backend can run transcription
    /// models on, for [`ModelsPageMessage::BackendDevicesLoaded`].
    fn load_backend_devices(source: String) -> Task<cosmic::Action<Message>> {
        Task::perform(list_stage_devices(STT_STAGE), move |result| match result {
            Ok(devices) => cosmic::Action::App(Message::ModelsPage(
                ModelsPageMessage::BackendDevicesLoaded {
                    source: source.clone(),
                    devices,
                },
            )),
            // The chips fall back to the catalog's own reading until an
            // answer lands, so a failed read costs the narrowing, not the row.
            Err(e) => {
                log::warn!("Could not read the devices for {source}: {e}");
                cosmic::Action::None
            }
        })
    }
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
    use super::{StagedLoad, staged_load_device};
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
}
