// SPDX-License-Identifier: GPL-3.0-only
//! Self-update: status load/check, the two settings toggles, and the apply
//! flow (installer download + spawn + JSON progress stream).

use crate::core::app::AppModel;
use crate::core::app::updater::{InstallerEvent, UpdateRunEvent};
use crate::daemon::client;
use crate::state::update::{RunPhase, UpdateRun};
use crate::ui::messages::{Message, UpdateMessage};
use cosmic::prelude::*;
use futures_util::StreamExt;

impl AppModel {
    pub(in crate::core::app) fn handle_update_messages(
        &mut self,
        message: UpdateMessage,
    ) -> Task<cosmic::Action<Message>> {
        match message {
            UpdateMessage::StatusLoaded(status) => {
                self.update.checking = false;
                match status {
                    Some(s) => {
                        self.update.unsupported = false;
                        self.update.status = Some(s);
                    }
                    None => self.update.unsupported = true,
                }
                Task::none()
            }
            UpdateMessage::StatusError(e) => {
                self.update.checking = false;
                log::warn!("Update status error: {e}");
                self.update.action_error = Some(format!("Couldn't fetch update status: {e}"));
                Task::none()
            }
            UpdateMessage::CheckNow => {
                if self.update.checking {
                    return Task::none();
                }
                self.update.checking = true;
                self.update.action_error = None;
                Task::perform(client::check_update_now(), |r| {
                    cosmic::Action::App(Message::Update(match r {
                        Ok(s) => UpdateMessage::StatusLoaded(Some(s)),
                        Err(e) => UpdateMessage::StatusError(e.to_string()),
                    }))
                })
            }
            UpdateMessage::AutoCheckLoaded(enabled) => {
                self.update.auto_check_enabled = Some(enabled);
                Task::none()
            }
            UpdateMessage::AutoCheckToggled(enabled) => {
                Task::perform(client::set_update_check_enabled(enabled), move |r| {
                    cosmic::Action::App(Message::Update(match r {
                        Ok(()) => UpdateMessage::AutoCheckLoaded(enabled),
                        Err(e) => UpdateMessage::StatusError(e.to_string()),
                    }))
                })
            }
            UpdateMessage::BetaOptinToggled(enabled) => {
                let value = if enabled { "enabled" } else { "disabled" }.to_string();
                Task::perform(client::set_update_beta_optin(value), |r| {
                    cosmic::Action::App(Message::Update(match r {
                        // Channel changed: re-resolve the candidate right away.
                        Ok(()) => UpdateMessage::CheckNow,
                        Err(e) => UpdateMessage::StatusError(e.to_string()),
                    }))
                })
            }
            UpdateMessage::AvailableEventReceived => {
                Task::perform(client::get_update_status(), |r| {
                    cosmic::Action::App(Message::Update(match r {
                        Ok(s) => UpdateMessage::StatusLoaded(s),
                        Err(e) => UpdateMessage::StatusError(e.to_string()),
                    }))
                })
            }
            UpdateMessage::StartUpdate => self.start_update(),
            UpdateMessage::CancelUpdate => {
                let cancellable = self.update.run.as_ref().is_some_and(UpdateRun::cancellable);
                if cancellable {
                    if let Some(handle) = self.update.run_abort.take() {
                        handle.abort(); // kill_on_drop reaps the child installer
                    }
                    self.update.run = None;
                }
                Task::none()
            }
            UpdateMessage::RunEvent(ev) => self.apply_run_event(ev),
            UpdateMessage::RestartApp => {
                let _ = std::process::Command::new("/usr/local/bin/super-stt-app").spawn();
                std::process::exit(0);
            }
        }
    }

    /// Kick off the apply flow: download the installer asset, spawn it, and
    /// stream its JSON progress. No-op if a run is already active or the
    /// daemon hasn't reported an installable candidate.
    fn start_update(&mut self) -> Task<cosmic::Action<Message>> {
        if self.update.run.is_some() {
            return Task::none();
        }
        let Some(status) = &self.update.status else {
            return Task::none();
        };
        let (Some(asset), Some(tag)) = (
            status.installer_asset.clone(),
            status.latest_version.clone(),
        ) else {
            return Task::none();
        };
        self.update.run = Some(UpdateRun {
            phase: RunPhase::FetchingInstaller,
            bytes_done: 0,
            bytes_total: 0,
            error: None,
            completed_components: Vec::new(),
        });
        let (task, handle) = cosmic::task::stream(
            crate::core::app::updater::run_update_stream(asset, tag)
                .map(|ev| cosmic::Action::App(Message::Update(UpdateMessage::RunEvent(ev)))),
        )
        .abortable();
        self.update.run_abort = Some(handle);
        task
    }

    /// Fold one apply-flow event into the in-flight [`UpdateRun`].
    fn apply_run_event(&mut self, ev: UpdateRunEvent) -> Task<cosmic::Action<Message>> {
        let Some(run) = self.update.run.as_mut() else {
            return Task::none();
        };
        match ev {
            UpdateRunEvent::FetchProgress {
                bytes_done,
                bytes_total,
            } => {
                run.phase = RunPhase::FetchingInstaller;
                run.bytes_done = bytes_done;
                run.bytes_total = bytes_total;
            }
            UpdateRunEvent::Installer(InstallerEvent::Phase { phase, message }) => {
                log::debug!("installer phase: {phase} — {message}");
                run.phase = match phase.as_str() {
                    "resolve" => RunPhase::Resolve,
                    "download" => RunPhase::Download,
                    "verify" => RunPhase::Verify,
                    "stage" => RunPhase::Stage,
                    "escalate" => RunPhase::WaitingAuth,
                    "install" => RunPhase::Install,
                    "post_install" => RunPhase::PostInstall,
                    _ => run.phase, // tolerate future phases
                };
            }
            UpdateRunEvent::Installer(InstallerEvent::Progress {
                phase,
                bytes_done,
                bytes_total,
            }) => {
                log::trace!("installer progress ({phase}): {bytes_done}/{bytes_total}");
                run.bytes_done = bytes_done;
                run.bytes_total = bytes_total;
            }
            UpdateRunEvent::Installer(InstallerEvent::Complete {
                installed_version,
                components,
            }) => {
                log::info!("installer completed: {installed_version} ({components:?})");
                run.phase = RunPhase::Done;
                run.completed_components = components;
            }
            UpdateRunEvent::Installer(InstallerEvent::Error { code, message }) => {
                log::warn!("installer reported error {code}: {message}");
                run.phase = RunPhase::Failed;
                run.error = Some(message);
            }
            UpdateRunEvent::Failed(message) => {
                run.phase = RunPhase::Failed;
                run.error = Some(message);
            }
            UpdateRunEvent::Finished {
                exit_ok,
                stderr_tail,
            } => {
                if run.phase == RunPhase::Done {
                    // Daemon was restarted by the installer; refresh status.
                    return self.handle_update_messages(UpdateMessage::AvailableEventReceived);
                }
                if run.phase != RunPhase::Failed {
                    run.phase = RunPhase::Failed;
                    run.error = Some(if exit_ok {
                        "installer ended unexpectedly".to_string()
                    } else {
                        format!("installer failed: {stderr_tail}")
                    });
                }
            }
        }
        Task::none()
    }
}
