// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{
    get_gpu_info, get_stage_view, list_stage_devices, list_stage_models, set_allow_online_models,
};
use crate::ui::messages::{DeviceMessage, Message, ModelMessage, ModelsPageMessage};
use cosmic::prelude::*;
use log::info;
use log::warn;
use super_tts_shared::models::protocol::SYNTHESIS_STAGE;

impl AppModel {
    /// Handle model management messages
    pub(in crate::core::app) fn handle_model_messages(
        &mut self,
        message: ModelMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            ModelMessage::LoadInitialData => self.handle_model_load_commands(),

            ModelMessage::AvailableModelsLoaded(_)
            | ModelMessage::StageViewLoaded { .. }
            | ModelMessage::ModelChanged { .. }
            | ModelMessage::ModelError(_)
            | ModelMessage::StageViewFetchFailed { .. } => self.handle_model_results(message),
        }
    }

    /// Handle the `LoadInitialData` startup load: the stage's view, the models
    /// it can run, and the devices its backend offers.
    fn handle_model_load_commands(&mut self) -> Task<cosmic::Action<Message>> {
        info!("LoadInitialData: Loading the synthesis stage and its catalogs at startup");
        Task::batch([
            Task::perform(list_stage_models(SYNTHESIS_STAGE), |result| match result {
                Ok(models) => {
                    cosmic::Action::App(Message::Model(ModelMessage::AvailableModelsLoaded(models)))
                }
                Err(e) => {
                    cosmic::Action::App(Message::Model(ModelMessage::ModelError(e.to_string())))
                }
            }),
            // Reads the backend, the model and the running accelerator in one
            // go, and is what fills the Models card's active-backend selection
            // — the old separate `/active_backend` read is folded into it.
            self.fetch_stage_view(),
            // The stage-wide device list: the union over the models this
            // stage's backend serves, and the only device answer available
            // before the user picks one. A failure here is not fatal — the
            // per-model list is the accurate one anyway, and this is only what
            // keeps the picker populated until it arrives.
            Task::perform(list_stage_devices(SYNTHESIS_STAGE), |result| match result {
                Ok(devices) => {
                    info!("Initial stage device list: {devices:?}");
                    cosmic::Action::App(Message::Device(DeviceMessage::StageDevicesLoaded(devices)))
                }
                Err(e) => {
                    warn!("Initial stage device list failed: {e}");
                    cosmic::Action::App(Message::Device(DeviceMessage::StageDevicesLoaded(
                        Vec::new(),
                    )))
                }
            }),
            // Online backends are gated by the install-time choice to
            // add an online-capable backend, not a runtime toggle, so
            // ensure the daemon permits them. Fire-and-forget.
            Task::perform(set_allow_online_models(true), |_| cosmic::Action::None),
            crate::core::app::handlers::tasks::reload_backends(),
            Task::perform(get_gpu_info(), |result| {
                cosmic::Action::App(Message::ModelsPage(ModelsPageMessage::GpuInfoLoaded(
                    result.unwrap_or_default(),
                )))
            }),
        ])
    }

    /// Handle model result messages: loads, changes, and errors.
    fn handle_model_results(&mut self, message: ModelMessage) -> Task<cosmic::Action<Message>> {
        match message {
            ModelMessage::AvailableModelsLoaded(models) => {
                self.available_models = models;
                Task::none()
            }

            ModelMessage::StageViewLoaded { view, epoch } => {
                // Discard a stale snapshot: a live `model_switched` advanced the
                // epoch after this fetch was issued, so it is the fresher truth
                // and this point-in-time read must not overwrite it.
                if epoch != self.current_model_epoch {
                    return Task::none();
                }
                self.apply_stage_view(&view)
            }

            ModelMessage::ModelChanged { model, source } => {
                // Authoritative result of a user-initiated switch — bump the
                // epoch so any in-flight reconnect snapshot is discarded rather
                // than reverting this.
                self.current_model_epoch = self.current_model_epoch.wrapping_add(1);
                self.current_model.clone_from(&model);
                self.current_source.clone_from(&source);
                self.model_operation_state = ModelOperationState::Ready;
                // The staging row is gone from the card now that the summary
                // has replaced it; leaving the pick behind would have Unload
                // re-stage a stale device the user never chose for this load.
                self.models_page.staged_model = None;
                self.models_page.staged_device = None;
                // Fetch the per-model language block now that a model is loaded.
                // Wire point 1: model loaded (ModelChanged).
                // The accelerator comes back separately, and only now: a `gpu`
                // preference can fall back to the CPU, so what the card names
                // has to be read after the load rather than assumed from what
                // was staged before it.
                //
                // The device answer is read too, for the loaded row's picker: a
                // move to another device changes the preference it selects, and
                // a load from the stage view never staged one to begin with.
                let mut tasks = vec![
                    self.load_model_language(&source, model.clone()),
                    self.load_model_voice(&source, model.clone()),
                    self.load_model_device(&source, model),
                    self.load_running_device(),
                ];
                // The Voices page states what the loaded model can do with a
                // cloned voice, so a switch that lands while it is open has to
                // move it. Only while it is open — nothing else reads this,
                // and every other entry to the page refetches anyway.
                if matches!(
                    self.nav.data::<crate::state::Page>(self.nav.active()),
                    Some(crate::state::Page::Voices)
                ) {
                    tasks.push(
                        self.dispatch(Message::Voices(crate::ui::messages::VoicesMessage::Refresh)),
                    );
                }
                Task::batch(tasks)
            }

            ModelMessage::ModelError(err) => {
                self.set_model_error(&err);
                Task::none()
            }

            ModelMessage::StageViewFetchFailed { epoch, error } => {
                // A stale snapshot fetch that failed must not clobber live state:
                // if a `model_switched` advanced the epoch after this fetch was
                // issued, the fresher truth already arrived — log and keep it,
                // never `clear_loaded_model()` on a superseded fetch error
                // (audit 2 Tier 1 #8).
                if epoch != self.current_model_epoch {
                    info!(
                        "Ignoring stale stage-view failure (epoch {epoch} != {}); \
                         keeping live model state: {error}",
                        self.current_model_epoch
                    );
                    return Task::none();
                }
                self.set_model_error(&error);
                Task::none()
            }

            // Startup load is driven by the sibling `handle_model_load_commands`
            // arm; nothing to do here.
            ModelMessage::LoadInitialData => Task::none(),
        }
    }

    /// Surface a failed model operation on the Models card and drop the loaded
    /// model, sanitizing any `$HOME` path out of the message. Shared by the
    /// `ModelError` sink and the optimistic-rollback handlers (audit Tier 3 #37).
    pub(in crate::core::app) fn set_model_error(&mut self, err: &str) {
        warn!("Model operation failed: {err}");
        let home = std::env::var("HOME").unwrap_or_default();
        let sanitized = sanitize_home(err, &home);
        self.model_operation_state = ModelOperationState::Error { message: sanitized };
        // A failed switch leaves the daemon idle (no model) — the backend stays
        // selected, but no model is loaded.
        self.clear_loaded_model();
    }

    /// Snapshot the whole synthesis stage, tagging the result with the
    /// `current_model_epoch` captured now. The `StageViewLoaded` handler applies
    /// it only if the epoch is still current — so a slow query that resolves
    /// after a live `model_switched` is discarded instead of reverting the model
    /// identity. Used at initial load and on every event-stream (re)subscribe to
    /// resync robustly against reconnect/restart ordering.
    ///
    /// The stage view rather than the model slot alone, because the slot no
    /// longer carries a `source`: the backend is the stage's property, and both
    /// halves are needed to name a model on the wire. Reading them as one also
    /// resyncs the card's selected backend, which used to drift whenever another
    /// client changed it while this app was disconnected.
    pub(in crate::core::app) fn fetch_stage_view(&self) -> Task<cosmic::Action<Message>> {
        let epoch = self.current_model_epoch;
        Task::perform(
            get_stage_view(SYNTHESIS_STAGE),
            move |result| match result {
                Ok(view) => cosmic::Action::App(Message::Model(ModelMessage::StageViewLoaded {
                    view,
                    epoch,
                })),
                Err(e) => cosmic::Action::App(Message::Model(ModelMessage::StageViewFetchFailed {
                    epoch,
                    error: e.to_string(),
                })),
            },
        )
    }

    /// Fold a stage snapshot into local state: the selected backend, the model
    /// and whether it is up, and the accelerator it is actually on.
    ///
    /// The `loaded` flag is what separates the two shapes the card can take, and
    /// ignoring it is a real misreport: the stage remembers its model through an
    /// unload, so a selection alone would have the card announce "Active:
    /// kokoro-82m" for a model that is not in memory and cannot speak. A
    /// remembered-but-unloaded selection is staged instead, which is exactly the
    /// state the Load button exists for.
    fn apply_stage_view(
        &mut self,
        view: &crate::daemon::client::StageState,
    ) -> Task<cosmic::Action<Message>> {
        self.model_operation_state = ModelOperationState::Ready;
        // The accelerator, not the preference: `running_device` already filters
        // out an unloaded model and the online sentinel, so an empty string here
        // means "there is no local device worth naming".
        self.current_device = view.running_device().unwrap_or_default().to_string();

        // The card's backend is the models page's state, so it is told rather
        // than written to from here — one owner per field, even when one read
        // answered for both.
        let mut tasks =
            vec![
                self.dispatch(Message::ModelsPage(ModelsPageMessage::ActiveBackendLoaded(
                    view.source.clone(),
                ))),
            ];

        match (view.selection(), view.loaded) {
            (Some((model, source)), true) => {
                self.current_model.clone_from(&model);
                self.current_source.clone_from(&source);
                self.models_page.staged_model = None;
                self.models_page.staged_device = None;
                // Wire point 1: a model is up, so its language block can fill
                // the card's language control, and its device answer the
                // loaded row's picker.
                tasks.push(self.load_model_language(&source, model.clone()));
                tasks.push(self.load_model_voice(&source, model.clone()));
                tasks.push(self.load_model_device(&source, model));
            }
            (Some((model, source)), false) => {
                // Selected but not running — the daemon kept the choice through
                // an unload or a failed load. Offer it back rather than making
                // the user find it in the dropdown again.
                self.clear_loaded_model();
                self.models_page.staged_model = Some(model.clone());
                // Cleared so the device answer now in flight is what seeds the
                // picker; a leftover from a previous staging would block it.
                self.models_page.staged_device = None;
                tasks.push(self.load_model_language(&source, model.clone()));
                tasks.push(self.load_model_voice(&source, model.clone()));
                tasks.push(self.load_model_device(&source, model));
            }
            (None, _) => {
                self.clear_loaded_model();
                self.models_page.staged_model = None;
                self.models_page.staged_device = None;
            }
        }
        Task::batch(tasks)
    }
}

/// Collapse the user's home directory back to `$HOME` in an error message and
/// cap it at 200 chars for display. Skips the substitution when `home` is empty
/// — an empty `from` makes `str::replace` insert the replacement at every char
/// boundary, shredding the message (Tier 1 #16).
fn sanitize_home(err: &str, home: &str) -> String {
    if home.is_empty() {
        err.chars().take(200).collect()
    } else {
        err.replace(home, "$HOME").chars().take(200).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_home;

    #[test]
    fn empty_home_leaves_message_intact() {
        // The regression: an empty HOME must NOT insert "$HOME" everywhere.
        assert_eq!(
            sanitize_home("load failed at /a/b", ""),
            "load failed at /a/b"
        );
    }

    #[test]
    fn set_home_is_folded_back() {
        assert_eq!(
            sanitize_home("no file /home/jo/models/x", "/home/jo"),
            "no file $HOME/models/x"
        );
    }

    #[test]
    fn output_is_capped_at_200_chars() {
        let long = "e".repeat(500);
        assert_eq!(sanitize_home(&long, "").chars().count(), 200);
    }
}
