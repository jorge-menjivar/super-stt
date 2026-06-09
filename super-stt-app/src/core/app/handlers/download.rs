// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{cancel_download, get_download_status};
use crate::ui::messages::Message;
use cosmic::prelude::*;
use log::info;
use log::warn;
use super_stt_shared::models::provider::Provider;

impl AppModel {
    /// Handle download progress messages
    pub(in crate::core::app) fn handle_download_messages(
        &mut self,
        message: Message,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DownloadProgressUpdate(_)
            | Message::CancelDownload
            | Message::CheckDownloadStatus
            | Message::NoDownloadInProgress => self.handle_download_progress(message),

            Message::DownloadCompleted(_)
            | Message::DownloadCancelled(_)
            | Message::DownloadError { .. } => self.handle_download_completion(message),

            _ => Task::none(),
        }
    }

    /// Handle active-download messages: progress updates, cancellation requests,
    /// status polling, and the no-download-in-progress sentinel.
    fn handle_download_progress(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DownloadProgressUpdate(progress) => {
                // We have an actual download in progress
                self.apply_download_progress(&progress);
                Task::none()
            }

            Message::CancelDownload => Task::perform(cancel_download(), |result| match result {
                Ok(_) => cosmic::Action::App(Message::DownloadCancelled(String::new())),
                Err(e) => cosmic::Action::App(Message::DownloadError {
                    model: String::new(),
                    error: e,
                }),
            }),

            Message::CheckDownloadStatus => {
                // Only poll while a switch is actually pending. A Ready state
                // needs no poll, and an Error state (e.g. a switch that just
                // failed) must keep its banner rather than be cleared here.
                if matches!(
                    self.model_operation_state,
                    ModelOperationState::Loading { .. } | ModelOperationState::Downloading { .. }
                ) {
                    Task::perform(get_download_status(), |result| match result {
                        Ok(Some(progress)) => {
                            // Download is actually happening
                            cosmic::Action::App(Message::DownloadProgressUpdate(progress))
                        }
                        // No download in progress (loaded from cache, or the
                        // switch failed): fall through to NoDownloadInProgress.
                        Ok(None) | Err(_) => cosmic::Action::App(Message::NoDownloadInProgress),
                    })
                } else {
                    Task::none()
                }
            }

            Message::NoDownloadInProgress => {
                // Only clear a `Downloading` state. A `Loading` state means
                // the daemon's `set_model` HTTP call is still in flight (the
                // subprocess might still be spawning, or the WASM component
                // might still be initialising) — its `ModelChanged` /
                // `ModelError` will land later and is the only message
                // authorised to flip Loading off. Flipping it here used to
                // re-enable the Load button mid-load, so a second click
                // fired another `set_model`, which then collided with the
                // first one in `systemd-run` ("Unit already loaded").
                //
                // The Error branch is preserved untouched — that's its own
                // contract (see `ModelError` handler).
                if matches!(
                    self.model_operation_state,
                    ModelOperationState::Downloading { .. }
                ) {
                    self.model_operation_state = ModelOperationState::Ready;
                }
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle terminal download outcomes: completed, cancelled, and error.
    fn handle_download_completion(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::DownloadCompleted(model_name) => {
                info!("Model {model_name} finished downloading");
                // Model information will be updated via daemon events (model_switched, ready)
                Task::none()
            }

            Message::DownloadCancelled(model_name) => {
                info!("Model {model_name} download was cancelled");
                // The daemon already unloaded the previous model before
                // the failed instantiate, so it's now idle (with the
                // active backend still selected). Reflect that locally
                // — no further set_model: an empty `previous_*` would
                // resolve to "no installed backend serves  via
                // local_whisper" and surface as a UI error.
                self.model_operation_state = ModelOperationState::Ready;
                self.current_model.clear();
                self.current_provider = Provider::default();
                self.current_source.clear();
                Task::none()
            }

            Message::DownloadError { model, error } => {
                warn!("Download error for model {model}: {error}");
                self.model_operation_state = ModelOperationState::Ready;
                self.transcription_text = format!("Download Error: {error}");
                self.current_model.clear();
                self.current_provider = Provider::default();
                self.current_source.clear();
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
