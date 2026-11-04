// SPDX-License-Identifier: GPL-3.0-only

mod install;
mod registry;

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{
    clear_stage_backend, get_gpu_info, list_stage_devices, reload_stage_model, set_model_device,
    set_stage_backend, set_stage_model, unload_stage_model,
};
use crate::state::{ContextPage, DaemonStatus, ModelsTab};
use crate::ui::messages::{
    DeviceMessage, DownloadMessage, Message, ModelMessage, ModelsPageMessage,
};
use cosmic::prelude::*;
use log::debug;
use super_tts_shared::daemon::http_client::HttpError;
use super_tts_shared::models::protocol::SYNTHESIS_STAGE;

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
            | ModelsPageMessage::MoveLoadedModel(_)
            | ModelsPageMessage::LoadStagedModel
            | ModelsPageMessage::UnloadActiveModel
            | ModelsPageMessage::ReloadActiveModel => self.handle_models_tab_selection(message),

            ModelsPageMessage::OpenBackendConfig(_)
            | ModelsPageMessage::CloseBackendConfig
            | ModelsPageMessage::SelectBackend(_)
            | ModelsPageMessage::BackendSelectFailed { .. }
            | ModelsPageMessage::DeselectBackend
            | ModelsPageMessage::ActiveBackendLoaded(_)
            | ModelsPageMessage::RefreshGpuInfo
            | ModelsPageMessage::GpuInfoLoaded(_)
            | ModelsPageMessage::ToggleInstalledMenu(_)
            | ModelsPageMessage::CloseInstalledMenu => self.handle_models_backend_config(message),

            ModelsPageMessage::InstallBackend(_)
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
            | ModelsPageMessage::InstalledOnlineFilter(_)
            | ModelsPageMessage::ImportBackendFromDir
            | ModelsPageMessage::ImportBackendFromDirPicked(_)
            | ModelsPageMessage::RegistryCustomRepoInputChanged(_)
            | ModelsPageMessage::ToggleBrowseMenu
            | ModelsPageMessage::CloseBrowseMenu => self.handle_models_registry(message),

            ModelsPageMessage::AddSourceChanged(_)
            | ModelsPageMessage::AddFolderInputChanged(_)
            | ModelsPageMessage::CheckAddSource
            | ModelsPageMessage::AddPreviewLoaded(_)
            | ModelsPageMessage::AddPreviewFailed(_)
            | ModelsPageMessage::AddPreviewSpin
            | ModelsPageMessage::InstallPreviewed => self.handle_add_sheet(message),
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
                // `staged_device` and only then writes anything.
                //
                // The device is cleared rather than guessed: which devices this
                // model can be loaded onto, and which one it would load with,
                // are the daemon's answers now — the manifest's declared list
                // is not narrowed by the accelerators the installed asset
                // actually shipped with, and seeding from it is what used to
                // stage a GPU an install could not use. The fetch below fills
                // it in; until it does the Load button stays disabled.
                let Some(source) = self.models_page.active_backend.clone() else {
                    log::warn!("StageActiveModel ignored — no backend fills the stage");
                    return Task::none();
                };
                self.models_page.staged_model = Some(model.clone());
                self.models_page.staged_device = None;
                // Wire point 2: model staged. Both per-model answers are read
                // here so the card can show the language control and the device
                // picker before Load, rather than after it.
                Task::batch([
                    self.load_model_language(&source, model.clone()),
                    self.load_model_voice(&source, model.clone()),
                    self.load_model_device(&source, model),
                ])
            }

            ModelsPageMessage::StageActiveDevice(device) => {
                self.models_page.staged_device = Some(device);
                Task::none()
            }

            ModelsPageMessage::MoveLoadedModel(device) => self.handle_move_loaded_model(device),

            ModelsPageMessage::LoadStagedModel => self.handle_load_staged_model(),

            ModelsPageMessage::UnloadActiveModel => self.handle_unload_active_model(),

            ModelsPageMessage::ReloadActiveModel => {
                if !self.is_model_ready() {
                    log::warn!("Model operation already in progress — ignoring Reload click");
                    return Task::none();
                }
                let (model, source) = (self.current_model.clone(), self.current_source.clone());
                if model.is_empty() {
                    log::warn!("ReloadActiveModel ignored — no model is loaded");
                    return Task::none();
                }
                // Same model, same device, brought back up. Nothing is
                // re-downloaded, so this is a short operation — but it is still
                // a switch as far as the card is concerned, and leaving the
                // controls live would let a second click race the first.
                self.set_model_loading(model.clone(), "Reloading model...".to_string());
                Task::perform(
                    reload_stage_model(SYNTHESIS_STAGE),
                    move |result| match result {
                        // Reported as a switch to the same model rather than
                        // waited on: that arm returns the card to Ready and
                        // re-reads the language and the resolved accelerator,
                        // which is exactly what a reload can have changed.
                        // Waiting for the `ready` broadcast instead would leave
                        // the card spinning if it were missed.
                        Ok(()) => cosmic::Action::App(Message::Model(ModelMessage::ModelChanged {
                            model: model.clone(),
                            source: source.clone(),
                        })),
                        Err(e) => cosmic::Action::App(Message::Model(ModelMessage::ModelError(
                            e.to_string(),
                        ))),
                    },
                )
            }

            _ => Task::none(),
        }
    }

    /// Free the loaded model's device memory, keeping both the backend and the
    /// model it was pointed at.
    ///
    /// The daemon remembers the selection through an unload — that is what
    /// separates this from Deselect — so the card remembers it too: what was
    /// running becomes what is staged, and loading it again onto another device
    /// is one click rather than a re-pick from the dropdown. The local change is
    /// optimistic; the daemon's `ready` event with `model_loaded: false` is the
    /// source of truth and lands a moment later.
    fn handle_unload_active_model(&mut self) -> Task<cosmic::Action<Message>> {
        let unloaded = (!self.current_model.is_empty())
            .then(|| (self.current_source.clone(), self.current_model.clone()));
        self.clear_loaded_model();
        self.model_operation_state = ModelOperationState::Ready;
        // Re-read the device rather than reuse what was staged for the load:
        // the preference may have changed since, and the picker must open on
        // what the daemon would actually load with today.
        self.models_page.staged_device = None;
        self.models_page.staged_model = unloaded.as_ref().map(|(_, model)| model.clone());
        let restage = if let Some((source, model)) = unloaded {
            self.load_model_device(&source, model)
        } else {
            Task::none()
        };
        Task::batch([
            restage,
            Task::perform(unload_stage_model(SYNTHESIS_STAGE), |result| match result {
                Ok(()) => cosmic::Action::None,
                Err(e) => {
                    cosmic::Action::App(Message::Model(ModelMessage::ModelError(e.to_string())))
                }
            }),
        ])
    }

    /// Move the loaded model onto `device`: the daemon writes the preference
    /// and reloads the running model there, answering once it is back up.
    ///
    /// The same shape as Reload, because to the card it is one: the model goes
    /// down and comes back, so the controls are disabled for the length of it
    /// and success is reported as a switch to the same model, which re-reads
    /// the resolved accelerator, the language and the device answer. Nothing is
    /// written optimistically — the label names the accelerator a load
    /// confirmed, and the picker the preference the daemon holds.
    fn handle_move_loaded_model(&mut self, device: String) -> Task<cosmic::Action<Message>> {
        let (model, source) = (self.current_model.clone(), self.current_source.clone());
        if model.is_empty() {
            log::warn!("MoveLoadedModel ignored — no model is loaded");
            return Task::none();
        }
        // Re-picking the device it already prefers is not a change. The daemon
        // would skip the reload too, but only after a round trip that greys
        // the card out for nothing.
        if self.model_device_preference(&source, &model) == Some(device.as_str()) {
            return Task::none();
        }
        if !self.is_model_ready() {
            log::warn!("Model operation already in progress — ignoring device change");
            return Task::none();
        }
        self.set_model_loading(model.clone(), format!("Moving to {device}..."));
        Task::perform(
            set_model_device(SYNTHESIS_STAGE, model.clone(), device),
            move |result| match result {
                Ok(()) => cosmic::Action::App(Message::Model(ModelMessage::ModelChanged {
                    model: model.clone(),
                    source: source.clone(),
                })),
                Err(e) => {
                    cosmic::Action::App(Message::Device(DeviceMessage::DeviceError(e.to_string())))
                }
            },
        )
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
            self.model_device_preference(&source, &model),
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

        // No optimistic device write. The device the card shows is the
        // accelerator a load actually resolved to, and the staged value is a
        // `cpu`/`gpu` preference — writing one into the other would have the
        // "Active:" line claim a GPU before any load confirmed one, and there
        // would be nothing to roll it back to if the switch then failed.
        self.set_model_loading(model.clone(), "Initiating model switch...".to_string());

        let model_label = model.clone();
        let source_label = source.clone();
        Task::batch([
            Task::perform(
                async move {
                    // The device is written against the *model*, before the
                    // model is loaded. For a model no stage is running that is
                    // only a note for its next load, which is precisely what
                    // lets the card set a device without the daemon loading the
                    // model twice.
                    if let Some(dev) = device_to_set {
                        set_model_device(SYNTHESIS_STAGE, model.clone(), dev).await?;
                    }
                    set_stage_model(SYNTHESIS_STAGE, model, Some(source)).await
                },
                move |result| match result {
                    Ok(()) => cosmic::Action::App(Message::Model(ModelMessage::ModelChanged {
                        model: model_label.clone(),
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
                // Fill the stage WITHOUT loading a model — the card moves to
                // the top fixed header, any model from a different backend is
                // unloaded. Pointing the stage at a model is the separate act
                // the Load button performs.
                if self.models_page.active_backend.as_deref() == Some(source.as_str()) {
                    return Task::none();
                }
                // Capture the prior selection so a failed activation rolls the
                // card back instead of showing a backend the daemon rejected
                // (audit Tier 3 #37).
                let prev_active = self.models_page.active_backend.take();
                self.models_page.active_backend = Some(source.clone());
                self.clear_loaded_model();
                self.clear_staged_model();
                self.model_operation_state = ModelOperationState::Ready;
                // Activation comes from the Models page's "Select a backend" sheet;
                // dismiss it now that a choice was made.
                self.core.window.show_context = false;
                Task::perform(
                    async move {
                        set_stage_backend(SYNTHESIS_STAGE, source).await?;
                        // Sequenced behind the select, not batched beside it:
                        // this list is the *new* backend's, and a concurrent
                        // read could just as easily answer for the old one.
                        // A failed read is not a failed select — the per-model
                        // list is the accurate one anyway, and this only keeps
                        // the device control populated until it arrives.
                        let devices = list_stage_devices(SYNTHESIS_STAGE)
                            .await
                            .unwrap_or_default();
                        Ok::<Vec<String>, HttpError>(devices)
                    },
                    move |result| match result {
                        Ok(devices) => cosmic::Action::App(Message::Device(
                            DeviceMessage::StageDevicesLoaded(devices),
                        )),
                        Err(e) => cosmic::Action::App(Message::ModelsPage(
                            ModelsPageMessage::BackendSelectFailed {
                                prev_active: prev_active.clone(),
                                message: e.to_string(),
                            },
                        )),
                    },
                )
            }

            ModelsPageMessage::BackendSelectFailed {
                prev_active,
                message,
            } => {
                self.models_page.active_backend = prev_active;
                self.set_model_error(&message);
                Task::none()
            }

            ModelsPageMessage::DeselectBackend => {
                // Optimistically empty the stage — backend, model and all; the
                // daemon goes idle. (Rejected only mid-utterance — an edge case
                // that self-heals on the next refresh.) Unlike Unload, this
                // forgets the model too, so the staged pick and its device
                // answer go with it rather than lingering as an offer to load
                // something from a backend that no longer fills the stage.
                self.models_page.active_backend = None;
                self.clear_loaded_model();
                self.clear_staged_model();
                self.stage_devices.clear();
                self.model_operation_state = ModelOperationState::Ready;
                self.models_page.configure_backend = None;
                // Close the configuration sheet if it was open for this backend.
                self.core.window.show_context = false;
                Task::perform(clear_stage_backend(SYNTHESIS_STAGE), |_| {
                    cosmic::Action::None
                })
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

/// What a Load click resolves to.
#[derive(Debug, PartialEq, Eq)]
enum StagedLoad {
    /// The staged `(source, model)` is no longer in the installed-backend
    /// catalog — the click is stale and must not reach the daemon.
    NotInCatalog,
    /// Point the stage at the staged model, first writing the model's device
    /// when `Some`.
    Switch { device_to_set: Option<String> },
}

/// Resolve a Load click against the installed-backend catalog and the device
/// preference the daemon already holds for the staged model.
///
/// The catalog check is not merely an optimization. A backend can be
/// uninstalled — or the catalog refreshed — between staging a model and
/// clicking Load, and a miss reads here as "not online", which would send a
/// device write for a model that no longer exists and surface as two errors
/// where the click could not have succeeded at all. Nothing reaches the daemon
/// for a pair it cannot serve.
///
/// For online models (the `none` sentinel in `supported_devices`) there is no
/// device to write. Otherwise the staged device goes out only when it differs
/// from what the daemon already records for *this model* — a plain string
/// comparison now, because both sides are the same `cpu`/`gpu` preference from
/// the same endpoint. There is no accelerator to collapse onto an axis: the
/// resolved `cuda`/`rocm` name lives on a separate field and is never what a
/// staged pick is compared against.
///
/// `stored_device` is `None` until that model's device answer arrives. Writing
/// on an unknown is the safe direction — a redundant write to a model no stage
/// is running is a note, not a reload.
fn staged_load_device(
    backends: &[crate::daemon::backends::BackendInfo],
    source: &str,
    model: &str,
    staged_device: Option<&str>,
    stored_device: Option<&str>,
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
            .filter(|d| *d != "none" && Some(*d) != stored_device)
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
            name: "Test".to_string(),
            description: String::new(),
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
            }],
            secrets: Vec::new(),
            options: Vec::new(),
        }
    }

    /// The regression: a Load click for a pair that is no longer installed must
    /// not reach the daemon at all. Treating the catalog miss as merely "not
    /// online" sends a device write for a model that no longer exists, and the
    /// user gets two failures out of one click that could never have worked.
    #[test]
    fn a_stale_pair_sends_nothing() {
        let installed = vec![backend(
            "github.com/super-tts/kokoro",
            "kokoro-tiny",
            &["gpu"],
        )];

        // Backend uninstalled between staging and the click.
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-tts/gone",
                "kokoro-tiny",
                Some("gpu"),
                Some("cpu")
            ),
            StagedLoad::NotInCatalog,
        );
        // Backend still installed, but no longer serving that model.
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-tts/kokoro",
                "kokoro-large",
                Some("gpu"),
                Some("cpu")
            ),
            StagedLoad::NotInCatalog,
        );
        // Nothing installed at all.
        assert_eq!(
            staged_load_device(
                &[],
                "github.com/super-tts/kokoro",
                "kokoro-tiny",
                Some("gpu"),
                Some("cpu")
            ),
            StagedLoad::NotInCatalog,
        );
    }

    /// A staged local model on a device other than the one the daemon has
    /// recorded still writes it — the guard must not swallow the case it sits
    /// in front of.
    #[test]
    fn a_local_model_on_a_new_device_sets_it() {
        let installed = vec![backend(
            "github.com/super-tts/kokoro",
            "kokoro-tiny",
            &["cpu", "gpu"],
        )];
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-tts/kokoro",
                "kokoro-tiny",
                Some("gpu"),
                Some("cpu")
            ),
            StagedLoad::Switch {
                device_to_set: Some("gpu".to_string())
            },
        );
    }

    /// No device write when it would be a no-op: the daemon already records
    /// that preference for this model, or nothing was staged. The comparison is
    /// against *this model's* stored preference, not against whatever the
    /// daemon happens to be running — that was the global setting this
    /// replaced, and under it a second model's load rewrote the first's device.
    #[test]
    fn an_unchanged_device_is_not_resent() {
        let installed = vec![backend(
            "github.com/super-tts/kokoro",
            "kokoro-tiny",
            &["cpu", "gpu"],
        )];
        for staged in [Some("cpu"), None] {
            assert_eq!(
                staged_load_device(
                    &installed,
                    "github.com/super-tts/kokoro",
                    "kokoro-tiny",
                    staged,
                    Some("cpu")
                ),
                StagedLoad::Switch {
                    device_to_set: None
                },
                "staged={staged:?} must not resend the stored preference"
            );
        }
    }

    /// A `gpu` preference the daemon holds for one model is not the same fact
    /// as a `gpu` model *running*: the staged value is compared against that
    /// model's own record, so staging `gpu` for a model recorded as `cpu`
    /// writes even while another model is loaded on the GPU.
    #[test]
    fn the_comparison_is_against_this_models_own_record() {
        let installed = vec![backend(
            "github.com/super-tts/kokoro",
            "kokoro-large",
            &["cpu", "gpu"],
        )];
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-tts/kokoro",
                "kokoro-large",
                Some("gpu"),
                Some("cpu")
            ),
            StagedLoad::Switch {
                device_to_set: Some("gpu".to_string())
            },
        );
    }

    /// Before that model's device answer arrives there is nothing to compare
    /// against, and the staged value is written rather than assumed redundant.
    /// A write to a model no stage is running is a note for its next load, so
    /// the cost of being wrong here is one request — where skipping it would
    /// load the model onto a device the user did not choose.
    #[test]
    fn an_unknown_stored_preference_still_writes() {
        let installed = vec![backend(
            "github.com/super-tts/kokoro",
            "kokoro-tiny",
            &["cpu", "gpu"],
        )];
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/super-tts/kokoro",
                "kokoro-tiny",
                Some("cpu"),
                None
            ),
            StagedLoad::Switch {
                device_to_set: Some("cpu".to_string())
            },
        );
    }

    /// Online models carry the `none` sentinel and have no device to set;
    /// a stale `none` staged against a local model is likewise never sent.
    #[test]
    fn an_online_model_sets_no_device() {
        let online = vec![backend(
            "github.com/super-tts/openai",
            "kokoro-1",
            &["none"],
        )];
        assert_eq!(
            staged_load_device(
                &online,
                "github.com/super-tts/openai",
                "kokoro-1",
                Some("none"),
                Some("none")
            ),
            StagedLoad::Switch {
                device_to_set: None
            },
        );
        // Even with a real device staged, an online model takes no device.
        assert_eq!(
            staged_load_device(
                &online,
                "github.com/super-tts/openai",
                "kokoro-1",
                Some("gpu"),
                Some("none")
            ),
            StagedLoad::Switch {
                device_to_set: None
            },
        );
        let local = vec![backend(
            "github.com/super-tts/kokoro",
            "kokoro-tiny",
            &["cpu"],
        )];
        assert_eq!(
            staged_load_device(
                &local,
                "github.com/super-tts/kokoro",
                "kokoro-tiny",
                Some("none"),
                Some("cpu")
            ),
            StagedLoad::Switch {
                device_to_set: None
            },
        );
    }
}
