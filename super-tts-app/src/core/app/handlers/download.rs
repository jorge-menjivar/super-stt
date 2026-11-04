// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{cancel_download, get_download_status};
use crate::ui::messages::{DownloadMessage, Message};
use cosmic::prelude::*;
use log::info;
use log::warn;
use super_tts_shared::models::protocol::SYNTHESIS_STAGE;

impl AppModel {
    /// Handle download progress messages
    pub(in crate::core::app) fn handle_download_messages(
        &mut self,
        message: DownloadMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DownloadMessage::DownloadProgressUpdate(_)
            | DownloadMessage::CancelDownload
            | DownloadMessage::CheckDownloadStatus
            | DownloadMessage::NoDownloadInProgress => self.handle_download_progress(message),

            DownloadMessage::DownloadCompleted(_)
            | DownloadMessage::DownloadCancelled(_)
            | DownloadMessage::DownloadError { .. } => self.handle_download_completion(message),
        }
    }

    /// Handle active-download messages: progress updates, cancellation requests,
    /// status polling, and the no-download-in-progress sentinel.
    fn handle_download_progress(
        &mut self,
        message: DownloadMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DownloadMessage::DownloadProgressUpdate(progress) => {
                // We have an actual download in progress
                self.apply_download_progress(&progress);
                Task::none()
            }

            DownloadMessage::CancelDownload => {
                // Addressed to the stage, not the daemon: stages provision
                // independently, and a Cancel under one stage's progress bar
                // must not abandon another stage's download.
                Task::perform(cancel_download(SYNTHESIS_STAGE), |result| match result {
                    Ok(()) => cosmic::Action::App(Message::Download(
                        DownloadMessage::DownloadCancelled(String::new()),
                    )),
                    Err(e) => {
                        cosmic::Action::App(Message::Download(DownloadMessage::DownloadError {
                            model: String::new(),
                            error: e.to_string(),
                        }))
                    }
                })
            }

            DownloadMessage::CheckDownloadStatus => {
                // Only poll while a switch is actually pending. A Ready state
                // needs no poll, and an Error state (e.g. a switch that just
                // failed) must keep its banner rather than be cleared here.
                if matches!(
                    self.model_operation_state,
                    ModelOperationState::Loading { .. } | ModelOperationState::Provisioning { .. }
                ) {
                    Task::perform(
                        get_download_status(SYNTHESIS_STAGE),
                        |result| match result {
                            Ok(Some(progress)) => {
                                // Download is actually happening
                                cosmic::Action::App(Message::Download(
                                    DownloadMessage::DownloadProgressUpdate(progress),
                                ))
                            }
                            // No download in progress (loaded from cache, or the
                            // switch failed): fall through to NoDownloadInProgress.
                            Ok(None) | Err(_) => cosmic::Action::App(Message::Download(
                                DownloadMessage::NoDownloadInProgress,
                            )),
                        },
                    )
                } else {
                    Task::none()
                }
            }

            DownloadMessage::NoDownloadInProgress => {
                // Only clear a `Provisioning` state. A `Loading` state means
                // the `POST /pipeline/{stage}/model` call is still in flight
                // (the subprocess might still be spawning, or the WASM
                // component might still be initialising) — its `ModelChanged` /
                // `ModelError` will land later and is the only message
                // authorised to flip Loading off. Flipping it here used to
                // re-enable the Load button mid-load, so a second click
                // fired another model switch, which then collided with the
                // first one in `systemd-run` ("Unit already loaded").
                //
                // The Error branch is preserved untouched — that's its own
                // contract (see `ModelError` handler).
                if matches!(
                    self.model_operation_state,
                    ModelOperationState::Provisioning { .. }
                ) {
                    self.model_operation_state = ModelOperationState::Ready;
                }
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Handle terminal download outcomes: completed, cancelled, and error.
    fn handle_download_completion(
        &mut self,
        message: DownloadMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DownloadMessage::DownloadCompleted(model_name) => {
                info!("Model {model_name} finished downloading");
                // Model information will be updated via daemon events (model_switched, ready)
                Task::none()
            }

            DownloadMessage::DownloadCancelled(model_name) => {
                info!("Model {model_name} download was cancelled");
                // The daemon already unloaded the previous model before
                // the failed instantiate, so it's now idle (with the
                // stage's backend still selected). Reflect that locally —
                // no follow-up model switch: an empty `previous_*` would
                // resolve to "no installed backend serves ''" and surface
                // as a UI error.
                self.model_operation_state = ModelOperationState::Ready;
                self.clear_loaded_model();
                Task::none()
            }

            DownloadMessage::DownloadError { model, error } => {
                warn!("Download error for model {model}: {error}");
                // Surface on the Models card banner instead of hijacking the
                // Speech page's test panel (Tier 3 #11).
                self.model_operation_state = ModelOperationState::Error {
                    message: format!("Download failed: {error}"),
                };
                self.clear_loaded_model();
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
