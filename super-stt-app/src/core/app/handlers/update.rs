// SPDX-License-Identifier: GPL-3.0-only
//! Self-update: status load/check, the two settings toggles, and the apply
//! flow (installer download + spawn + JSON progress stream).
//!
//! This module is intentionally thin `Task` routing — every state
//! TRANSITION lives on `UpdateState`/`UpdateRun` in `state/update.rs` (pure,
//! unit-tested there). This module owns only the side effects: which async
//! `Task` to kick off or continue with, given the outcome of a pure state
//! mutation.

use crate::core::app::AppModel;
use crate::daemon::client;
use crate::state::update::RunOutcome;
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
                self.update.apply_status_loaded(status);
                Task::none()
            }
            UpdateMessage::StatusError(e) => {
                log::warn!("Update status error: {e}");
                self.update.apply_status_error(&e);
                Task::none()
            }
            UpdateMessage::SettingError(e) => {
                log::warn!("Update setting save failed: {e}");
                self.update.apply_setting_error(&e);
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
                        Ok(s) => UpdateMessage::StatusLoaded(s),
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
                        Err(e) => UpdateMessage::SettingError(e.to_string()),
                    }))
                })
            }
            UpdateMessage::BetaOptinToggled(enabled) => {
                let value = if enabled { "enabled" } else { "disabled" }.to_string();
                Task::perform(client::set_update_beta_optin(value), |r| {
                    cosmic::Action::App(Message::Update(match r {
                        // Channel changed: re-resolve the candidate right away.
                        Ok(()) => UpdateMessage::CheckNow,
                        Err(e) => UpdateMessage::SettingError(e.to_string()),
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
                let cancellable = self
                    .update
                    .run
                    .as_ref()
                    .is_some_and(crate::state::update::UpdateRun::cancellable);
                if cancellable {
                    if let Some(handle) = self.update.run_abort.take() {
                        handle.abort(); // kill_on_drop reaps the child installer
                    }
                    self.update.run = None;
                }
                Task::none()
            }
            UpdateMessage::DismissRun => {
                self.update.dismiss_run();
                Task::none()
            }
            UpdateMessage::RunEvent(ev) => match self.update.apply_run_event(ev) {
                RunOutcome::Continue => Task::none(),
                // Daemon was restarted by the installer; refresh status.
                RunOutcome::RefetchStatus => {
                    self.handle_update_messages(UpdateMessage::AvailableEventReceived)
                }
            },
            UpdateMessage::RestartApp => {
                // Only exit once the relaunch has actually been spawned — if
                // it fails, the user is left with no running app at all, so
                // surface the failure and let them retry (or start it
                // manually) instead.
                match std::process::Command::new("/usr/local/bin/super-stt-app").spawn() {
                    Ok(_) => std::process::exit(0),
                    Err(e) => {
                        log::warn!("failed to relaunch after update: {e}");
                        if let Some(run) = self.update.run.as_mut() {
                            run.error =
                                Some("Couldn't relaunch — start Super STT manually".to_string());
                        }
                        Task::none()
                    }
                }
            }
        }
    }

    /// Kick off the apply flow: download the installer asset, spawn it, and
    /// stream its JSON progress. No-op if a run is already active or the
    /// daemon hasn't reported an installable candidate — see
    /// `UpdateState::can_start_update`.
    fn start_update(&mut self) -> Task<cosmic::Action<Message>> {
        if !self.update.can_start_update() {
            return Task::none();
        }
        // can_start_update() just confirmed both are present.
        let status = self
            .update
            .status
            .as_ref()
            .expect("can_start_update confirmed a status is present");
        let asset = status
            .installer_asset
            .clone()
            .expect("can_start_update confirmed an installer asset is present");
        let tag = status
            .latest_version
            .clone()
            .expect("can_start_update confirmed a latest_version is present");

        self.update.begin_run();
        let (task, handle) = cosmic::task::stream(
            crate::core::app::updater::run_update_stream(asset, tag)
                .map(|ev| cosmic::Action::App(Message::Update(UpdateMessage::RunEvent(ev)))),
        )
        .abortable();
        self.update.run_abort = Some(handle);
        task
    }
}
