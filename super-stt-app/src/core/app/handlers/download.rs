// SPDX-License-Identifier: GPL-3.0-only

use crate::core::app::{AppModel, ModelOperationState};
use crate::daemon::client::{cancel_download, get_download_status};
use crate::state::device_offers::STT_STAGE;
use crate::ui::messages::{DownloadMessage, Message};
use cosmic::prelude::*;
use log::info;
use log::warn;

impl AppModel {
    /// Handle download progress messages
    pub(in crate::core::app) fn handle_download_messages(
        &mut self,
        message: DownloadMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            DownloadMessage::DownloadProgressUpdate(_)
            | DownloadMessage::CancelDownload(_)
            | DownloadMessage::CheckDownloadStatus
            | DownloadMessage::NoDownloadInProgress(_) => self.handle_download_progress(message),

            DownloadMessage::DownloadCompleted(_)
            | DownloadMessage::DownloadCancelled { .. }
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

            DownloadMessage::CancelDownload(stage) => {
                // The stage of the card the Cancel button sits on, taken from
                // the progress it is showing.
                Task::perform(cancel_download(stage), move |result| match result {
                    Ok(()) => {
                        cosmic::Action::App(Message::Download(DownloadMessage::DownloadCancelled {
                            model: String::new(),
                            stage,
                        }))
                    }
                    Err(e) => {
                        cosmic::Action::App(Message::Download(DownloadMessage::DownloadError {
                            model: String::new(),
                            error: e.to_string(),
                            stage,
                        }))
                    }
                })
            }

            DownloadMessage::CheckDownloadStatus => {
                // Poll each stage still owed an outcome, and only those. A
                // Ready stage needs no poll, and an Error state (e.g. a switch
                // that just failed) must keep its banner rather than be
                // cleared here. Every stage asks about itself: polling stage 1
                // for a stage-2 download would find nothing and collapse the
                // post-processor's progress card.
                Task::batch(
                    self.model_operations
                        .pending_stages()
                        .into_iter()
                        .map(|stage| {
                            Task::perform(get_download_status(stage), move |result| match result {
                                Ok(Some(progress)) => {
                                    // Download is actually happening
                                    cosmic::Action::App(Message::Download(
                                        DownloadMessage::DownloadProgressUpdate(progress),
                                    ))
                                }
                                // No download in progress (loaded from cache, or
                                // the switch failed): fall through.
                                Ok(None) | Err(_) => cosmic::Action::App(Message::Download(
                                    DownloadMessage::NoDownloadInProgress(stage),
                                )),
                            })
                        }),
                )
            }

            DownloadMessage::NoDownloadInProgress(stage) => {
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
                    self.model_operations.get(stage),
                    Some(ModelOperationState::Downloading { .. })
                ) {
                    self.model_operations.set_ready(stage);
                }
                Task::none()
            }

            _ => Task::none(),
        }
    }

    /// Drop the locally-tracked model for a stage whose operation just ended
    /// badly. Only stage 1's identity is mirrored in the app (`current_model`
    /// and its source); stage 2's lives in the post-processor block, which the
    /// daemon re-reports, so there is nothing to clear for it here. Clearing
    /// unconditionally is what used to blank the transcription card when a
    /// post-processor download failed.
    fn forget_stage_model(&mut self, stage: u32) {
        if stage == STT_STAGE {
            self.clear_loaded_model();
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

            DownloadMessage::DownloadCancelled { model, stage } => {
                info!("Model {model} download on stage {stage} was cancelled");
                // The daemon already unloaded the previous model before
                // the failed instantiate, so that stage is now idle (with the
                // backend still selected). Reflect that locally
                // — no further set_model: an empty `previous_*` would
                // resolve to "no installed backend serves ''" and surface
                // as a UI error.
                self.model_operations.set_ready(stage);
                self.forget_stage_model(stage);
                Task::none()
            }

            DownloadMessage::DownloadError {
                model,
                error,
                stage,
            } => {
                warn!("Download error for model {model} on stage {stage}: {error}");
                // Surface on that stage's card banner instead of hijacking the
                // Recording page's transcription box (Tier 3 #11).
                self.model_operations.set(
                    stage,
                    ModelOperationState::Error {
                        message: format!("Download failed: {error}"),
                    },
                );
                self.forget_stage_model(stage);
                Task::none()
            }

            _ => Task::none(),
        }
    }
}
