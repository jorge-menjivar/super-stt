// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{
    get_active_backend, get_current_device, get_current_model, get_gpu_info, list_available_models,
    list_backends, set_allow_online_models,
};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use log::info;
use log::warn;

impl AppModel {
    /// Handle model management messages
    pub(in crate::core::app) fn handle_model_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::LoadInitialData => self.handle_model_load_commands(),

            Message::AvailableModelsLoaded(_)
            | Message::CurrentModelLoaded { .. }
            | Message::ModelChanged { .. }
            | Message::ModelError(_) => self.handle_model_results(message),

            _ => Task::none(),
        }
    }

    /// Handle the `LoadInitialData` startup load: models + device info.
    fn handle_model_load_commands(&mut self) -> Task<cosmic::Action<Message>> {
        info!("LoadInitialData: Loading models and device info at startup");
        // One-time startup load: models + device info
        Task::batch([
            Task::perform(list_available_models(), |result| match result {
                Ok(models) => cosmic::Action::App(Message::AvailableModelsLoaded(models)),
                Err(e) => cosmic::Action::App(Message::ModelError(e.to_string())),
            }),
            self.fetch_current_model(),
            Task::perform(get_current_device(), |result| match result {
                Ok((device, available_devices)) => {
                    info!(
                        "Initial device load successful: device={device}, available_devices={available_devices:?}"
                    );
                    cosmic::Action::App(Message::DeviceInfoLoaded(device, available_devices))
                }
                Err(e) => {
                    warn!("Initial device load failed: {e}");
                    cosmic::Action::App(Message::DeviceError(e.to_string()))
                }
            }),
            // Online backends are gated by the install-time choice to
            // add an online-capable backend, not a runtime toggle, so
            // ensure the daemon permits them. Fire-and-forget.
            Task::perform(set_allow_online_models(true), |_| cosmic::Action::None),
            Task::perform(list_backends(), |result| match result {
                Ok(backends) => cosmic::Action::App(Message::BackendsLoaded(backends)),
                Err(e) => cosmic::Action::App(Message::BackendsError(e.to_string())),
            }),
            Task::perform(get_active_backend(), |result| {
                cosmic::Action::App(Message::ActiveBackendLoaded(result.unwrap_or(None)))
            }),
            Task::perform(get_gpu_info(), |result| {
                cosmic::Action::App(Message::GpuInfoLoaded(result.unwrap_or_default()))
            }),
        ])
    }

    /// Handle model result messages: loads, changes, and errors.
    fn handle_model_results(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::AvailableModelsLoaded(models) => {
                self.available_models = models;
                Task::none()
            }

            Message::CurrentModelLoaded {
                model,
                provider,
                source,
                epoch,
            } => {
                // Discard a stale snapshot: a live `model_switched` advanced the
                // epoch after this fetch was issued, so it is the fresher truth
                // and this point-in-time read must not overwrite it.
                if epoch != self.current_model_epoch {
                    return Task::none();
                }
                self.current_model.clone_from(&model);
                self.current_provider = provider;
                self.current_source.clone_from(&source);
                self.model_operation_state = ModelOperationState::Ready;
                // Fetch the per-model language block now that a model is loaded.
                // Wire point 1: model loaded (CurrentModelLoaded).
                self.load_model_language(source, model)
            }

            Message::ModelChanged {
                model,
                provider,
                source,
            } => {
                // Authoritative result of a user-initiated switch — bump the
                // epoch so any in-flight reconnect snapshot is discarded rather
                // than reverting this.
                self.current_model_epoch = self.current_model_epoch.wrapping_add(1);
                self.current_model.clone_from(&model);
                self.current_provider = provider;
                self.current_source.clone_from(&source);
                self.model_operation_state = ModelOperationState::Ready;
                // Fetch the per-model language block now that a model is loaded.
                // Wire point 1: model loaded (ModelChanged).
                self.load_model_language(source, model)
            }

            Message::ModelError(err) => {
                warn!("Model operation failed: {err}");
                let home = std::env::var("HOME").unwrap_or_default();
                let sanitized = sanitize_home(&err, &home);
                self.model_operation_state = ModelOperationState::Error { message: sanitized };
                // A failed switch leaves the daemon idle (no model) — the
                // backend stays selected, but no model is loaded.
                self.clear_loaded_model();
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Snapshot the daemon's current model, tagging the result with the
    /// `current_model_epoch` captured now. The `CurrentModelLoaded` handler
    /// applies it only if the epoch is still current — so a slow query that
    /// resolves after a live `model_switched` is discarded instead of reverting
    /// the model identity. Used at initial load and on every event-stream
    /// (re)subscribe to resync robustly against reconnect/restart ordering.
    pub(in crate::core::app) fn fetch_current_model(&self) -> Task<cosmic::Action<Message>> {
        let epoch = self.current_model_epoch;
        Task::perform(get_current_model(), move |result| match result {
            Ok((model, provider, source)) => cosmic::Action::App(Message::CurrentModelLoaded {
                model,
                provider,
                source,
                epoch,
            }),
            Err(e) => cosmic::Action::App(Message::ModelError(e.to_string())),
        })
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
