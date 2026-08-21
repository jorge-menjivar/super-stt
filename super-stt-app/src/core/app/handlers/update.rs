// SPDX-License-Identifier: GPL-3.0-only
//! Self-update: status load/check, the two settings toggles, and the apply
//! flow (installer download + spawn + JSON progress stream).

use crate::core::app::AppModel;
use crate::core::app::updater::{InstallerEvent, UpdateRunEvent};
use crate::daemon::client;
use crate::state::update::{RunPhase, UpdateRun, UpdateState};
use crate::ui::messages::{Message, UpdateMessage};
use cosmic::prelude::*;
use futures_util::StreamExt;

/// Applied on every fresh `StatusLoaded` (connect-time load, page refresh,
/// manual/auto check, or the post-install `AvailableEventReceived` refetch).
///
/// A `Done` run's Restart affordance must survive this — the
/// `Done → AvailableEventReceived → StatusLoaded` sequence fires immediately
/// after a successful update (the daemon restarted onto the new version), so
/// clearing `run` here would erase the CTA before the user ever sees it.
/// A `Failed` run has nothing further to preserve, so a fresh snapshot
/// supersedes its stale error instead of masking newer information (e.g. a
/// newer version now available). An in-flight (non-terminal) run is never
/// touched — only `CancelUpdate`/`DismissRun` end those.
fn clear_run_on_status_refresh(update: &mut UpdateState) {
    if matches!(update.run.as_ref().map(|r| r.phase), Some(RunPhase::Failed)) {
        update.run = None;
        update.run_abort = None;
    }
}

/// Applied on `DismissRun`: only a terminal (`Done`/`Failed`) run can be
/// dismissed this way — an in-flight run must go through `CancelUpdate`
/// instead, so it still honors `UpdateRun::cancellable()`'s escalate-phase
/// safety cutoff.
fn dismiss_terminal_run(update: &mut UpdateState) {
    if update
        .run
        .as_ref()
        .is_some_and(|r| matches!(r.phase, RunPhase::Done | RunPhase::Failed))
    {
        update.run = None;
        update.run_abort = None;
    }
}

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
                clear_run_on_status_refresh(&mut self.update);
                Task::none()
            }
            UpdateMessage::StatusError(e) => {
                self.update.checking = false;
                log::warn!("Update status error: {e}");
                self.update.action_error = Some(format!("Couldn't fetch update status: {e}"));
                Task::none()
            }
            UpdateMessage::SettingError(e) => {
                log::warn!("Update setting save failed: {e}");
                self.update.action_error = Some(format!("Couldn't update setting: {e}"));
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
                let cancellable = self.update.run.as_ref().is_some_and(UpdateRun::cancellable);
                if cancellable {
                    if let Some(handle) = self.update.run_abort.take() {
                        handle.abort(); // kill_on_drop reaps the child installer
                    }
                    self.update.run = None;
                }
                Task::none()
            }
            UpdateMessage::DismissRun => {
                dismiss_terminal_run(&mut self.update);
                Task::none()
            }
            UpdateMessage::RunEvent(ev) => self.apply_run_event(ev),
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

#[cfg(test)]
mod tests {
    use super::{clear_run_on_status_refresh, dismiss_terminal_run};
    use crate::state::update::{RunPhase, UpdateRun, UpdateState};

    fn run(phase: RunPhase) -> UpdateRun {
        UpdateRun {
            phase,
            bytes_done: 0,
            bytes_total: 0,
            error: None,
            completed_components: Vec::new(),
        }
    }

    /// (a) A completed app-component update's Restart affordance must
    /// survive the `Done → AvailableEventReceived → StatusLoaded` refetch
    /// that fires right after the daemon restarts onto the new version —
    /// even though that fresh status now reports `update_available: false`.
    #[test]
    fn a_completed_app_update_run_survives_a_status_refresh() {
        let mut update = UpdateState {
            run: Some(UpdateRun {
                completed_components: vec!["daemon".to_string(), "app".to_string()],
                ..run(RunPhase::Done)
            }),
            ..Default::default()
        };

        clear_run_on_status_refresh(&mut update);

        let run = update.run.expect("Done run must survive a status refresh");
        assert_eq!(run.phase, RunPhase::Done);
        assert!(run.completed_components.iter().any(|c| c == "app"));
    }

    /// A `Failed` run has no further affordance to preserve, so a fresh
    /// status refetch supersedes it rather than leaving stale error text
    /// masking newer information (e.g. a newer version now available).
    #[test]
    fn a_failed_run_is_cleared_by_a_status_refresh() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Failed)),
            ..Default::default()
        };

        clear_run_on_status_refresh(&mut update);

        assert!(update.run.is_none());
    }

    /// An in-flight run must never be silently dropped by a status refetch —
    /// only `CancelUpdate` (which respects `UpdateRun::cancellable()`) or a
    /// terminal transition ends it.
    #[test]
    fn an_in_flight_run_is_never_cleared_by_a_status_refresh() {
        for phase in [
            RunPhase::FetchingInstaller,
            RunPhase::Resolve,
            RunPhase::Download,
            RunPhase::Verify,
            RunPhase::Stage,
            RunPhase::WaitingAuth,
            RunPhase::Install,
            RunPhase::PostInstall,
        ] {
            let mut update = UpdateState {
                run: Some(run(phase)),
                ..Default::default()
            };
            clear_run_on_status_refresh(&mut update);
            assert_eq!(update.run.map(|r| r.phase), Some(phase), "phase {phase:?}");
        }
    }

    /// (b) Dismissing a `Failed` run clears it — `start_update`'s only guard
    /// is `self.update.run.is_some()`, so a cleared (`None`) run means a
    /// subsequent `StartUpdate` is no longer blocked.
    #[test]
    fn dismiss_clears_a_failed_run_so_a_future_start_update_is_not_blocked() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Failed)),
            ..Default::default()
        };

        dismiss_terminal_run(&mut update);

        assert!(
            update.run.is_none(),
            "start_update's `run.is_some()` guard must see None after Dismiss"
        );
    }

    /// Dismiss also clears a `Done` run (e.g. after the user chose not to
    /// restart, or a failed relaunch attempt) so it doesn't linger forever.
    #[test]
    fn dismiss_clears_a_done_run() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Done)),
            ..Default::default()
        };

        dismiss_terminal_run(&mut update);

        assert!(update.run.is_none());
    }

    /// Dismiss must not be able to end an in-flight run — that path belongs
    /// to `CancelUpdate`, which additionally enforces the escalate-phase
    /// safety cutoff (`UpdateRun::cancellable()`).
    #[test]
    fn dismiss_does_not_clear_an_in_flight_run() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Install)),
            ..Default::default()
        };

        dismiss_terminal_run(&mut update);

        assert_eq!(update.run.map(|r| r.phase), Some(RunPhase::Install));
    }
}
