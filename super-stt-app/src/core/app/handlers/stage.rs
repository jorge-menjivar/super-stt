// SPDX-License-Identifier: GPL-3.0-only

//! Filling a pipeline stage: select a backend, stage a model and a device,
//! load, unload.
//!
//! One implementation for every stage. It was two — the transcription card's
//! and the post-processor's — and the copies drifted apart in ways that were
//! only ever bugs on the newer one: no re-entrancy guard on Load, no
//! stale-catalog check, no device rollback, a Load button that never checked a
//! device was staged, and a device answer matched by model name while ignoring
//! which backend it was staged against.
//!
//! What a stage genuinely decides for itself is small, and each of those points
//! appears exactly once below, marked `STAGE-SPECIFIC`:
//!
//! * where its selected backend is held,
//! * where the model it has up is held, and
//! * what to do once the daemon has taken a load.
//!
//! The fourth — which of a backend's models a stage may run — is
//! `ui::views::models::models_for_stage`, where the cards that render the list
//! can reach it.
//!
//! Everything else is shared.

use cosmic::prelude::*;

use crate::core::app::AppModel;
use crate::daemon::backends::BackendInfo;
use crate::daemon::client::{
    clear_stage_backend, get_model_device, list_model_devices, list_stage_devices,
    set_model_device, set_stage_backend, set_stage_model, unload_stage_model,
};
use crate::state::device_offers::{STT_STAGE, staged_device};
use crate::ui::messages::{Message, ModelMessage, PostProcessorMessage, StageMessage};

impl AppModel {
    /// Handle a stage message — the same handler for every stage.
    pub(in crate::core::app) fn handle_stage_messages(
        &mut self,
        message: StageMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            StageMessage::SelectBackend { stage, source } => {
                self.select_stage_backend(stage, source)
            }
            StageMessage::DeselectBackend { stage } => self.deselect_stage_backend(stage),
            StageMessage::BackendSelected {
                stage,
                source,
                model,
            } => {
                self.set_selected_backend(stage, source.clone());
                let Some(source) = source else {
                    return Task::none();
                };
                Task::batch([
                    // The card's device chips are the daemon's answer for the
                    // backend now selected, so every selection asks again.
                    Self::load_backend_devices(stage, source.clone()),
                    // The model the daemon remembers is the one the card offers
                    // to load, running or not: a stage keeps its selection
                    // through an unload, and reading it back is what stops the
                    // card coming up empty after one.
                    model.map_or_else(Task::none, |model| {
                        self.stage_selection_if_unstaged(stage, &model, &source)
                    }),
                ])
            }
            StageMessage::BackendSelectFailed {
                stage,
                prev,
                message,
            } => {
                self.set_selected_backend(stage, prev);
                self.set_stage_error(stage, &message);
                Task::none()
            }
            StageMessage::StageModel { stage, model } => self.stage_model(stage, model),
            StageMessage::StageDevice { stage, device } => self.choose_device(stage, device),
            StageMessage::DeviceChangeFailed {
                stage,
                prev_device,
                message,
            } => {
                // The model is still up on the device it had, so put the picker
                // back on that rather than leaving it showing one the daemon
                // never adopted.
                self.staged_picks.set_device(stage, prev_device.clone());
                if stage == STT_STAGE {
                    self.device_state = crate::core::app::DeviceState::Ready;
                    if let Some(device) = prev_device {
                        self.current_device = device;
                    }
                }
                self.report_stage_error(stage, &message);
                Task::none()
            }
            StageMessage::StagedDevicesLoaded {
                stage,
                source,
                model,
                devices,
                current,
            } => {
                // The answer describes the backend selected when it was asked.
                // A switch since then makes it about a backend this card no
                // longer shows, and recording it would evict the new
                // selection's own answers.
                if self.selected_backend(stage).as_deref() != Some(source.as_str()) {
                    return Task::none();
                }
                self.device_offers.record(
                    stage,
                    source.clone(),
                    Some(model.clone()),
                    devices.clone(),
                );
                // Likewise for the pick: one made since the ask wins. Matched
                // on the backend as well as the name, or a same-named model on
                // another backend would claim this answer.
                if self.staged_picks.model(stage, &source) == Some(model.as_str()) {
                    // The model's own device when the picker can show it,
                    // otherwise the first offered — and nothing at all when
                    // none is offered, which is what keeps Load disabled.
                    self.staged_picks
                        .set_device(stage, staged_device(&devices, current));
                }
                Task::none()
            }
            StageMessage::BackendDevicesLoaded {
                stage,
                source,
                devices,
            } => {
                if self.selected_backend(stage).as_deref() == Some(source.as_str()) {
                    self.device_offers.record(stage, source, None, devices);
                }
                Task::none()
            }
            StageMessage::Load { stage } => self.load_staged(stage),
            StageMessage::LoadFailed {
                stage,
                prev_device,
                message,
            } => {
                // Roll the device back rather than leaving the card on one the
                // daemon never adopted.
                if stage == STT_STAGE {
                    self.current_device = prev_device;
                }
                self.set_stage_error(stage, &message);
                Task::none()
            }
            StageMessage::Unload { stage } => self.unload_stage(stage),
        }
    }

    // ---- STAGE-SPECIFIC (1/4): where a stage holds its selected backend ----

    /// The backend filling `stage`, as the app currently knows it.
    ///
    /// Stage 1 owns its selection locally so the card can move optimistically;
    /// later stages read it back from the daemon's own stage block.
    #[must_use]
    pub fn selected_backend(&self, stage: u32) -> Option<String> {
        if stage == STT_STAGE {
            self.models_page.active_backend.clone()
        } else {
            self.post_processor.source.clone()
        }
    }

    /// Record `source` as `stage`'s selection locally.
    ///
    /// A no-op for stages the daemon is authoritative for: their block is
    /// replaced wholesale when it is re-read, and writing here would only
    /// create a second truth to disagree with.
    fn set_selected_backend(&mut self, stage: u32, source: Option<String>) {
        if stage == STT_STAGE {
            self.models_page.active_backend = source;
        }
    }

    // ---- STAGE-SPECIFIC (2/4): where a stage holds the model it has up ----

    /// The model `stage` is currently running, as the app knows it.
    ///
    /// Stage 1 keeps its identity locally, kept live by the daemon's events;
    /// later stages read it back from their own stage block. Both answer `None`
    /// when nothing is up — which, since a stage remembers its selection
    /// through an unload, is not the same as having no model selected.
    #[must_use]
    fn loaded_model(&self, stage: u32) -> Option<&str> {
        if stage == STT_STAGE {
            (!self.current_model.is_empty()).then_some(self.current_model.as_str())
        } else if self.post_processor.loaded {
            self.post_processor.model.as_deref()
        } else {
            None
        }
    }

    // ---- STAGE-SPECIFIC (3/4): what a taken load means for a stage ----

    /// The message that follows the daemon accepting a load.
    ///
    /// Stage 1 announces the new identity, which bumps the epoch that protects
    /// it from a stale snapshot. Later stages have no identity of their own to
    /// announce and simply re-read their block.
    fn load_committed(stage: u32, model: &str, source: &str) -> cosmic::Action<Message> {
        if stage == STT_STAGE {
            cosmic::Action::App(Message::Model(ModelMessage::ModelChanged {
                model: model.to_string(),
                source: source.to_string(),
            }))
        } else {
            cosmic::Action::App(Message::PostProcessor(
                PostProcessorMessage::ReloadRequested,
            ))
        }
    }

    // ---- shared from here down ----

    /// Surface a failed stage operation on that stage's card.
    ///
    /// Both stages render `operation_status(app, stage)`, so both report a
    /// failure the same way, with the same `$HOME` sanitizing. Stage 1 also
    /// drops its locally-held identity: a failed switch leaves it with no model
    /// loaded.
    pub(in crate::core::app) fn set_stage_error(&mut self, stage: u32, err: &str) {
        self.report_stage_error(stage, err);
        if stage == STT_STAGE {
            self.clear_loaded_model();
        }
    }

    /// Surface a failed stage operation without touching what the stage is
    /// running.
    ///
    /// The half of [`Self::set_stage_error`] that a failed *device* change
    /// wants: the daemon puts the model back on the device it had, so it is
    /// still up, and dropping stage 1's identity here would blank a card whose
    /// model never stopped running.
    fn report_stage_error(&mut self, stage: u32, err: &str) {
        log::warn!("Stage {stage} operation failed: {err}");
        let home = std::env::var("HOME").unwrap_or_default();
        let sanitized = super::model::sanitize_home(err, &home);
        self.model_operations.set(
            stage,
            crate::core::app::ModelOperationState::Error { message: sanitized },
        );
    }

    /// Select the backend filling `stage`, without loading a model.
    ///
    /// Optimistic with rollback: the card moves at once and
    /// [`StageMessage::BackendSelectFailed`] puts it back, so a refused
    /// selection never leaves the card showing a backend the daemon rejected.
    fn select_stage_backend(
        &mut self,
        stage: u32,
        source: String,
    ) -> Task<cosmic::Action<Message>> {
        // Re-selecting what is already selected is not a change; round-tripping
        // it would unload the running model for nothing.
        if self.selected_backend(stage).as_deref() == Some(source.as_str()) {
            self.core.window.show_context = false;
            return Task::none();
        }
        let prev = self.selected_backend(stage);
        self.set_selected_backend(stage, Some(source.clone()));
        self.staged_picks.clear(stage);
        if stage == STT_STAGE {
            self.clear_loaded_model();
        }
        self.model_operations.set_ready(stage);
        // The choice came from a picker sheet; dismiss it now it has been made.
        self.core.window.show_context = false;

        let selected = source.clone();
        Task::perform(
            set_stage_backend(stage, source),
            move |result| match result {
                // Re-announce the selection the daemon just took, which is what
                // asks it for the backend's devices.
                Ok(()) => cosmic::Action::App(Message::Stage(StageMessage::BackendSelected {
                    stage,
                    source: Some(selected.clone()),
                    // A backend the user just picked has no model yet —
                    // selecting one deliberately drops the previous stage's.
                    model: None,
                })),
                Err(e) => cosmic::Action::App(Message::Stage(StageMessage::BackendSelectFailed {
                    stage,
                    prev: prev.clone(),
                    message: e.to_string(),
                })),
            },
        )
    }

    /// Empty `stage`, forgetting its model with it.
    fn deselect_stage_backend(&mut self, stage: u32) -> Task<cosmic::Action<Message>> {
        self.set_selected_backend(stage, None);
        self.staged_picks.clear(stage);
        if stage == STT_STAGE {
            self.clear_loaded_model();
            self.models_page.configure_backend = None;
        }
        self.model_operations.set_ready(stage);
        self.core.window.show_context = false;
        Task::perform(clear_stage_backend(stage), move |result| match result {
            Ok(()) => cosmic::Action::None,
            // Reported rather than swallowed: a deselect the daemon refused
            // (mid-recording, say) otherwise leaves the card empty and the
            // stage still filled, with nothing saying so.
            Err(e) => cosmic::Action::App(Message::Stage(StageMessage::BackendSelectFailed {
                stage,
                prev: None,
                message: e.to_string(),
            })),
        })
    }

    /// Stage `model` in `stage`, then ask the daemon what it can run on and
    /// what language it resolves to.
    ///
    /// The device is left unset until that answer lands: an empty answer is
    /// what disables Load and shows the advisory, so staging a device before
    /// the answer would be guessing at exactly the case that must not guess.
    fn stage_model(&mut self, stage: u32, model: String) -> Task<cosmic::Action<Message>> {
        let Some(source) = self.selected_backend(stage) else {
            return Task::none();
        };
        self.staged_picks
            .stage_model(stage, source.clone(), model.clone());
        self.clear_action_error(crate::state::ErrorScope::PostProcessing);
        Task::batch([
            Self::load_staged_devices(stage, source.clone(), model.clone()),
            // So the card can show the language control before Load.
            self.load_model_language(stage, &source, model),
        ])
    }

    /// Stage the model the daemon reports for `stage`, when the user has no
    /// pick of their own.
    ///
    /// The daemon's selection is what the card's dropdown shows and what Load
    /// commits, so it is a pick like any other and needs its device and
    /// language answers fetched. Leaving it unstaged is what used to hide the
    /// device picker after an unload.
    pub(in crate::core::app) fn stage_selection_if_unstaged(
        &mut self,
        stage: u32,
        model: &str,
        source: &str,
    ) -> Task<cosmic::Action<Message>> {
        // A pick of the user's own outranks the daemon's memory, or every
        // reload would throw their choice away.
        if model.is_empty() || source.is_empty() || self.staged_picks.get(stage).is_some() {
            return Task::none();
        }
        self.staged_picks
            .stage_model(stage, source.to_string(), model.to_string());
        Task::batch([
            Self::load_staged_devices(stage, source.to_string(), model.to_string()),
            self.load_model_language(stage, source, model.to_string()),
        ])
    }

    /// Choose the device `stage`'s model runs on.
    ///
    /// Local while nothing is up: there is no model to move, and the Load
    /// button commits the choice with the pick it belongs to. Once the model is
    /// running the choice goes straight to the daemon, which reloads it onto
    /// the new device in place — something it has always done, and that the
    /// card used to make the user do by hand as unload, re-pick, load.
    fn choose_device(&mut self, stage: u32, device: String) -> Task<cosmic::Action<Message>> {
        // Re-picking the same device is not a change; sending it would reload a
        // running model for nothing.
        let previous = self.staged_picks.device(stage).map(ToString::to_string);
        if previous.as_deref() == Some(device.as_str()) {
            return Task::none();
        }
        self.staged_picks.set_device(stage, Some(device.clone()));

        // Only the model actually running can be moved in place. A device
        // chosen for a different pick is staged like any other and waits for
        // Load — sending it would move the wrong model.
        let picked = self.staged_picks.get(stage).map(|pick| pick.model.clone());
        let Some(running) = self.loaded_model(stage).map(ToString::to_string) else {
            return Task::none();
        };
        if picked.as_deref() != Some(running.as_str()) {
            return Task::none();
        }
        // One operation per stage, as everywhere else here.
        if !self.is_model_ready(stage) {
            log::warn!(
                "A stage-{stage} model operation is already in progress — ignoring device change"
            );
            return Task::none();
        }

        let source = self.selected_backend(stage).unwrap_or_default();
        let prev_device = if stage == STT_STAGE {
            let prev = self.current_device.clone();
            self.set_device_switching(device.clone(), format!("Switching to {device}..."));
            self.current_device.clone_from(&device);
            (!prev.is_empty()).then_some(prev)
        } else {
            // Stage 2 publishes no device events of its own, so its card needs
            // a progress line put up here or the reload passes unremarked.
            self.set_model_loading(running.clone(), format!("Loading on {device}..."), stage);
            previous
        };

        let (model, source_label) = (running.clone(), source.clone());
        Task::perform(
            set_model_device(stage, running, device),
            move |result| match result {
                Ok(()) => Self::load_committed(stage, &model, &source_label),
                Err(e) => cosmic::Action::App(Message::Stage(StageMessage::DeviceChangeFailed {
                    stage,
                    prev_device: prev_device.clone(),
                    message: e.to_string(),
                })),
            },
        )
    }

    /// Commit `stage`'s staged pick: set the model's device, then load it.
    fn load_staged(&mut self, stage: u32) -> Task<cosmic::Action<Message>> {
        let Some(pick) = self.staged_picks.get(stage) else {
            log::warn!("Load ignored — nothing staged for stage {stage}");
            return Task::none();
        };
        let (model, source) = (pick.model.clone(), pick.source.clone());
        let staged_device = pick.device.clone();

        // This stage's own operation only: the stages provision independently,
        // so one stage's download must not swallow the other's click.
        if !self.is_model_ready(stage) {
            log::warn!("A stage-{stage} model operation is already in progress — ignoring Load");
            return Task::none();
        }

        // A backend can be uninstalled between staging and the click; the
        // daemon would refuse both calls, but with two errors for one click.
        let StagedLoad::Load { device_to_set } =
            staged_load_device(&self.backends, &source, &model, staged_device.as_deref())
        else {
            log::warn!(
                "Load ignored — backend/model not in catalog: source={source}, model={model}"
            );
            return Task::none();
        };

        // Captured so a failed switch rolls the device back rather than leaving
        // the card on one the daemon never adopted.
        let prev_device = self.current_device.clone();
        self.set_model_loading(
            model.clone(),
            "Initiating model switch...".to_string(),
            stage,
        );
        if stage == STT_STAGE
            && let Some(dev) = &device_to_set
        {
            self.current_device.clone_from(dev);
        }

        let (model_label, source_label) = (model.clone(), source.clone());
        Task::batch([
            Task::perform(
                async move {
                    // The device is the model's own, set before the load so the
                    // load picks it up. For a model that is not loaded the
                    // daemon only records it.
                    if let Some(dev) = device_to_set {
                        set_model_device(stage, model.clone(), dev).await?;
                    }
                    set_stage_model(stage, model, Some(source)).await
                },
                move |result| match result {
                    Ok(()) => Self::load_committed(stage, &model_label, &source_label),
                    Err(e) => cosmic::Action::App(Message::Stage(StageMessage::LoadFailed {
                        stage,
                        prev_device: prev_device.clone(),
                        message: e.to_string(),
                    })),
                },
            ),
            // A load that is really a download reports through
            // `download_progress`; this kick asks for the first tick rather
            // than waiting for the daemon to volunteer it.
            Task::perform(
                async {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                },
                |()| {
                    cosmic::Action::App(Message::Download(
                        crate::ui::messages::DownloadMessage::CheckDownloadStatus,
                    ))
                },
            ),
        ])
    }

    /// Stop running `stage`'s model, keeping its backend selected.
    ///
    /// Optimistic: the card returns to its pickers at once. The staged pick is
    /// left alone, so the model that was running is still the one offered —
    /// which is also what gives its device picker something to key on.
    fn unload_stage(&mut self, stage: u32) -> Task<cosmic::Action<Message>> {
        if stage == STT_STAGE {
            self.clear_loaded_model();
        }
        self.model_operations.set_ready(stage);
        Task::perform(unload_stage_model(stage), move |result| match result {
            Ok(()) => {
                if stage == STT_STAGE {
                    cosmic::Action::None
                } else {
                    // Later stages have no identity event of their own, so the
                    // re-read is what tells the card it stopped.
                    cosmic::Action::App(Message::PostProcessor(
                        PostProcessorMessage::ReloadRequested,
                    ))
                }
            }
            Err(e) => cosmic::Action::App(Message::Stage(StageMessage::LoadFailed {
                stage,
                prev_device: String::new(),
                message: e.to_string(),
            })),
        })
    }

    /// Ask the daemon what `model` can run on in `stage` and which device it
    /// already has.
    ///
    /// One task for both reads so the picker and the device it starts on land
    /// together: staging the model's own device means nothing until the list
    /// that has to contain it is known.
    fn load_staged_devices(
        stage: u32,
        source: String,
        model: String,
    ) -> Task<cosmic::Action<Message>> {
        let asked = model.clone();
        Task::perform(
            async move {
                let devices = list_model_devices(stage, model.clone()).await?;
                // The recorded device is the nicety, not the requirement: a
                // model that has never been loaded has none, and a failed read
                // only costs the restaging.
                let current = get_model_device(stage, model).await.ok().map(|d| d.device);
                Ok((devices, current))
            },
            move |result: super_stt_shared::daemon::http_client::HttpResult<_>| match result {
                Ok((devices, current)) => {
                    cosmic::Action::App(Message::Stage(StageMessage::StagedDevicesLoaded {
                        stage,
                        source: source.clone(),
                        model: asked.clone(),
                        devices,
                        current,
                    }))
                }
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

    /// Ask the daemon which devices `stage`'s backend can run its models on.
    pub(in crate::core::app) fn load_backend_devices(
        stage: u32,
        source: String,
    ) -> Task<cosmic::Action<Message>> {
        Task::perform(list_stage_devices(stage), move |result| match result {
            Ok(devices) => {
                cosmic::Action::App(Message::Stage(StageMessage::BackendDevicesLoaded {
                    stage,
                    source: source.clone(),
                    devices,
                }))
            }
            // The chips fall back to the catalog's own reading until an answer
            // lands, so a failed read costs the narrowing, not the row.
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
    /// The staged pair is no longer in the installed-backend catalog — the
    /// click is stale and must not reach the daemon.
    NotInCatalog,
    /// Load the staged model, first setting the device when `Some`.
    Load { device_to_set: Option<String> },
}

/// Resolve a Load click against the installed-backend catalog.
///
/// For online models (the `none` sentinel in `supported_devices`) there is no
/// device to set; otherwise the staged device is sent as the model's own. The
/// daemon compares it with where the model already is, so resending an
/// unchanged device costs nothing.
fn staged_load_device(
    backends: &[BackendInfo],
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
    StagedLoad::Load {
        device_to_set: if online {
            None
        } else {
            staged_device
                .filter(|d| *d != "none")
                .map(ToString::to_string)
        },
    }
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

    /// A Load click for a pair that is no longer installed must not reach the
    /// daemon at all.
    #[test]
    fn a_stale_pair_sends_nothing() {
        let installed = vec![backend("github.com/x/whisper", "whisper-tiny", &["cuda"])];

        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/x/gone",
                "whisper-tiny",
                Some("cuda")
            ),
            StagedLoad::NotInCatalog,
        );
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/x/whisper",
                "whisper-large",
                Some("cuda"),
            ),
            StagedLoad::NotInCatalog,
        );
        assert_eq!(
            staged_load_device(&[], "github.com/x/whisper", "whisper-tiny", Some("cuda")),
            StagedLoad::NotInCatalog,
        );
    }

    /// An online model has no device to set, so the `none` sentinel is never
    /// sent as one.
    #[test]
    fn an_online_model_sets_no_device() {
        let installed = vec![backend(
            "github.com/x/openai",
            "gpt-4o-transcribe",
            &["none"],
        )];
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/x/openai",
                "gpt-4o-transcribe",
                Some("none"),
            ),
            StagedLoad::Load {
                device_to_set: None
            },
        );
    }

    /// A local model sends the staged device as its own.
    #[test]
    fn a_local_model_sets_the_staged_device() {
        let installed = vec![backend(
            "github.com/x/whisper",
            "whisper-tiny",
            &["cpu", "gpu"],
        )];
        assert_eq!(
            staged_load_device(
                &installed,
                "github.com/x/whisper",
                "whisper-tiny",
                Some("gpu")
            ),
            StagedLoad::Load {
                device_to_set: Some("gpu".to_string())
            },
        );
    }
}
