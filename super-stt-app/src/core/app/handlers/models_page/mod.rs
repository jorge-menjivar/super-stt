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
                // `staged_device` and only then calls the daemon. Default the
                // device to the model's first supported entry (so a single-
                // device model needs no extra click; multi-device models
                // surface the dropdown).
                let source = self.models_page.active_backend.clone();
                let device = source.as_ref().and_then(|src| {
                    self.backends
                        .iter()
                        .find(|b| &b.source == src)?
                        .models
                        .iter()
                        .find(|m| m.name == model)?
                        .supported_devices
                        .first()
                        .cloned()
                });
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
        let Some(provider) = self.backend_model_provider(&source, &model) else {
            log::warn!(
                "LoadStagedModel: backend/model not in catalog: source={source}, model={model}"
            );
            return Task::none();
        };
        if !self.is_model_ready() {
            log::warn!("Model operation already in progress — ignoring Load click");
            return Task::none();
        }

        // For online models the staged device is `"none"` (the sentinel)
        // and no `set_device` call is needed. Online-ness is derived from the
        // model's `supported_devices` (the `none` sentinel), not the provider.
        let online = self
            .backends
            .iter()
            .find(|b| b.source == source)
            .and_then(|b| b.models.iter().find(|m| m.name == model))
            .is_some_and(|m| m.supported_devices.iter().any(|d| d == "none"));
        let device_to_set = if online {
            None
        } else {
            self.models_page
                .staged_device
                .clone()
                .filter(|d| d != "none" && *d != self.current_device)
        };

        self.set_model_loading(model.clone(), "Initiating model switch...".to_string());
        if let Some(dev) = &device_to_set {
            self.current_device.clone_from(dev);
        }

        let model_label = model.clone();
        let source_label = source.clone();
        let provider_label = provider.clone();
        Task::batch([
            Task::perform(
                async move {
                    if let Some(dev) = device_to_set {
                        set_device(dev).await?;
                    }
                    set_model(model, provider, source).await.map(|_| ())
                },
                move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::Model(ModelMessage::ModelChanged {
                        model: model_label.clone(),
                        provider: provider_label.clone(),
                        source: source_label.clone(),
                    })),
                    Err(e) => {
                        cosmic::Action::App(Message::Model(ModelMessage::ModelError(e.to_string())))
                    }
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
                self.models_page.active_backend = Some(source.clone());
                self.clear_loaded_model();
                self.model_operation_state = ModelOperationState::Ready;
                // Activation comes from the Models page's "Load a backend" sheet;
                // dismiss it now that a choice was made.
                self.core.window.show_context = false;
                Task::perform(set_active_backend(source), |result| match result {
                    Ok(()) => cosmic::Action::None,
                    Err(e) => {
                        cosmic::Action::App(Message::Model(ModelMessage::ModelError(e.to_string())))
                    }
                })
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
