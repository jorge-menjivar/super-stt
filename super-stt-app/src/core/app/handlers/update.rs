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
use crate::ui::messages::{BetaOptinOutcome, Message, UpdateMessage};
use cosmic::Application as _; // `on_nav_select`, reused by the header badge
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
                // A daemon that has not checked yet has no candidate to
                // report, so the page and the header badge read empty until
                // its deferred first check runs. Ask once.
                if self.update.wants_first_check() {
                    return self.handle_update_messages(UpdateMessage::CheckNow);
                }
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
            UpdateMessage::BetaOptinToggled(enabled) => self.start_beta_optin(enabled),
            UpdateMessage::BetaOptinApplied { enabled, outcome } => {
                self.finish_beta_optin(enabled, &outcome);
                Task::none()
            }
            UpdateMessage::OpenUpdatesPage => self.open_updates_page(),
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
    /// Press the beta-updates switch: move it, lock it, then do the work.
    ///
    /// The write is a local socket call but the re-check chained behind it is
    /// a network round-trip, and rendering the switch straight off
    /// `status.beta_optin_effective` left it sitting unmoved under the user's
    /// finger for that whole trip. `checking` rides along so "Check now"
    /// reads as busy for the same window — it is the same check.
    fn start_beta_optin(&mut self, enabled: bool) -> Task<cosmic::Action<Message>> {
        // A second press while the first is still settling would resolve in
        // arrival order rather than press order.
        if self.update.beta_pending {
            return Task::none();
        }
        self.update.beta_optin = Some(enabled);
        self.update.beta_pending = true;
        self.update.checking = true;
        self.update.action_error = None;
        let value = if enabled { "enabled" } else { "disabled" }.to_string();
        Task::perform(
            async move {
                match client::set_update_beta_optin(value).await {
                    Err(e) => BetaOptinOutcome::WriteFailed(e.to_string()),
                    // Channel changed: re-resolve the candidate right away.
                    Ok(()) => match client::check_update_now().await {
                        Ok(s) => BetaOptinOutcome::Applied(s),
                        Err(e) => BetaOptinOutcome::CheckFailed(e.to_string()),
                    },
                }
            },
            move |outcome| {
                cosmic::Action::App(Message::Update(UpdateMessage::BetaOptinApplied {
                    enabled,
                    outcome,
                }))
            },
        )
    }

    /// Release the lock the switch took, and settle it on the outcome.
    fn finish_beta_optin(&mut self, enabled: bool, outcome: &BetaOptinOutcome) {
        // Unlock before applying the status: `apply_status_loaded` re-syncs
        // the switch from the daemon, and it defers to a pending write.
        self.update.beta_pending = false;
        match outcome {
            BetaOptinOutcome::Applied(status) => {
                self.update.apply_status_loaded(status.clone());
            }
            // Nothing changed daemon-side, so the switch must go back.
            BetaOptinOutcome::WriteFailed(e) => {
                log::warn!("Beta opt-in save failed: {e}");
                self.update.beta_optin = Some(!enabled);
                self.update.checking = false;
                self.update.apply_setting_error(e);
            }
            // The setting *did* change; only the candidate version is now
            // unknown. Reverting the switch would misreport the daemon.
            BetaOptinOutcome::CheckFailed(e) => {
                log::warn!("Beta opt-in re-check failed: {e}");
                self.update.apply_status_error(e);
            }
        }
    }

    /// Navigate to the Updates page from the header bar's badge.
    ///
    /// Through `on_nav_select` rather than a bare `activate` so the badge
    /// lands on the page in exactly the state the sidebar entry leaves it in
    /// — status refetched, any open context sheet closed.
    fn open_updates_page(&mut self) -> Task<cosmic::Action<Message>> {
        let entity = self.nav.iter().find(|&id| {
            matches!(
                self.nav.data::<crate::state::Page>(id),
                Some(crate::state::Page::Updates)
            )
        });
        entity.map_or_else(Task::none, |id| self.on_nav_select(id))
    }

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
