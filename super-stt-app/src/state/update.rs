// SPDX-License-Identifier: GPL-3.0-only
//! UI state for the self-update flow (the Updates page + header badge).
//!
//! Every state TRANSITION lives here as a pure method on [`UpdateState`] /
//! [`UpdateRun`] so it's testable without constructing an `AppModel` (which
//! can't be built outside the running app — private fields, no `Default`,
//! no test harness). `core/app/handlers/update.rs` calls these methods to
//! mutate state, then separately decides which `Task` (if any) to return —
//! it owns the side effects, this module owns the logic.

use crate::core::app::updater::{InstallerEvent, UpdateRunEvent};
use super_stt_shared::models::self_update::SelfUpdateStatus;

/// Self-update state: last known daemon status, a settings-toggle error
/// banner, and the in-flight apply run (if any).
// No Debug derive: the abort handle isn't Debug.
#[derive(Default)]
pub struct UpdateState {
    /// Last known daemon-reported status. None until first load.
    pub status: Option<SelfUpdateStatus>,
    /// True once `GET /v1/update` (or `POST /v1/update/check`) returned 404
    /// (daemon predates the feature).
    pub unsupported: bool,
    pub checking: bool,
    pub auto_check_enabled: Option<bool>,
    /// Beta opt-in as the toggle should currently render it. Owned locally
    /// rather than read straight off `status` so the switch can move the
    /// instant it is pressed: `status.beta_optin_effective` only changes when
    /// a whole new status arrives, and the re-check that produces one is a
    /// network round-trip. Re-synced from every fresh status, so the daemon
    /// still has the last word. `None` until the first status loads.
    pub beta_optin: Option<bool>,
    /// True while a beta-opt-in write and its follow-up re-check are in
    /// flight. Locks the toggle so a second press cannot race the first —
    /// the two would resolve in arrival order, not press order, and the
    /// switch would settle on whichever finished last.
    pub beta_pending: bool,
    /// In-flight or finished update run; None when idle.
    pub run: Option<UpdateRun>,
    /// Aborts the run's task stream. Only honored before the escalate
    /// phase — see [`UpdateRun::cancellable`].
    pub run_abort: Option<cosmic::iced::task::Handle>,
    /// Error from a settings toggle, shown in the page banner.
    pub action_error: Option<String>,
}

impl UpdateState {
    /// Applied on every fresh status (connect-time load, page refresh,
    /// manual/auto check via `CheckNow`, or the post-install
    /// `AvailableEventReceived` refetch): `None` means the daemon answered
    /// 404 (predates `/v1/update`), so it marks `unsupported` instead of
    /// storing a status; `Some` clears `unsupported` and stores it. Either
    /// way, `checking` ends and a stale `Failed` run is superseded (see
    /// [`Self::clear_run_on_status_refresh`]).
    pub fn apply_status_loaded(&mut self, status: Option<SelfUpdateStatus>) {
        self.checking = false;
        match status {
            Some(s) => {
                // The daemon has the last word on the beta channel — but not
                // while our own write is still settling, or a status fetched
                // before it would drag the switch back under the user's
                // finger and then flip it again a moment later.
                if !self.beta_pending {
                    self.beta_optin = Some(s.beta_optin_effective);
                }
                self.unsupported = false;
                self.status = Some(s);
            }
            None => self.unsupported = true,
        }
        self.clear_run_on_status_refresh();
    }

    /// A `GET`/`POST /v1/update*` call itself failed (network/daemon error,
    /// not a 404). Shown in the page banner; does not touch `status`,
    /// `unsupported`, or `run`.
    pub fn apply_status_error(&mut self, msg: &str) {
        self.checking = false;
        self.action_error = Some(format!("Couldn't fetch update status: {msg}"));
    }

    /// A settings-toggle write (`AutoCheckToggled`/`BetaOptinToggled`)
    /// failed. Distinct banner text from [`Self::apply_status_error`] so it
    /// names the right verb.
    pub fn apply_setting_error(&mut self, msg: &str) {
        self.action_error = Some(format!("Couldn't update setting: {msg}"));
    }

    /// A `Done`/`Failed` run has nothing further to preserve *once superseded
    /// by a fresh snapshot that no longer needs it as a stale error*, so a
    /// `Failed` run is cleared here rather than masking newer information
    /// (e.g. a newer version now available).
    ///
    /// A `Done` run's Restart affordance must survive this — the
    /// `Done → AvailableEventReceived → StatusLoaded` sequence fires
    /// immediately after a successful update (the daemon restarted onto the
    /// new version), so clearing `run` here would erase the CTA before the
    /// user ever sees it.
    ///
    /// An in-flight (non-terminal) run is never touched — only
    /// `CancelUpdate`/`DismissRun` end those.
    pub fn clear_run_on_status_refresh(&mut self) {
        if matches!(self.run.as_ref().map(|r| r.phase), Some(RunPhase::Failed)) {
            self.run = None;
            self.run_abort = None;
        }
    }

    /// Applied on `DismissRun`: only a terminal (`Done`/`Failed`) run can be
    /// dismissed this way — an in-flight run must go through `CancelUpdate`
    /// instead, so it still honors [`UpdateRun::cancellable`]'s
    /// escalate-phase safety cutoff.
    pub fn dismiss_run(&mut self) {
        if self
            .run
            .as_ref()
            .is_some_and(|r| matches!(r.phase, RunPhase::Done | RunPhase::Failed))
        {
            self.run = None;
            self.run_abort = None;
        }
    }

    /// Whether `StartUpdate` would actually begin a new run right now: no
    /// run already active (regardless of phase — a terminal one must be
    /// dismissed/superseded first), and the last-known status carries both
    /// an installable asset and a target tag to install.
    #[must_use]
    pub fn can_start_update(&self) -> bool {
        self.run.is_none()
            && self
                .status
                .as_ref()
                .is_some_and(|s| s.installer_asset.is_some() && s.latest_version.is_some())
    }

    /// Begin a new run at `FetchingInstaller`. Callers must check
    /// [`Self::can_start_update`] first — this unconditionally (over)writes
    /// `run`.
    pub fn begin_run(&mut self) {
        self.run = Some(UpdateRun {
            phase: RunPhase::FetchingInstaller,
            bytes_done: 0,
            bytes_total: 0,
            error: None,
            error_code: None,
            completed_components: Vec::new(),
        });
    }

    /// Whether the "Check now" button should be pressable: not already
    /// checking, no run in progress, and the daemon actually supports the
    /// feature (a `CheckNow` against an `unsupported` daemon would just
    /// 404 again).
    #[must_use]
    pub fn can_check_now(&self) -> bool {
        !self.checking && self.run.is_none() && !self.unsupported
    }

    /// Whether to ask the daemon for a check right now.
    ///
    /// A daemon defers its own first check for a minute after start so it
    /// does not compete with the model load, and until then it has no
    /// candidate to report: the Updates page reads empty and the header
    /// badge is absent, however long an update has actually been waiting.
    /// Restart the daemon and the app together — the development loop — and
    /// that is the state the app opens in. Asking closes the gap without
    /// changing when the daemon checks on its own.
    ///
    /// Self-limiting rather than latched: a *completed* check stamps
    /// `checked_at` whether it succeeded or failed, so this turns false as
    /// soon as one lands. A check that fails before completing reports
    /// through `StatusError`, which stores no status and so cannot re-enter
    /// this — it leaves the ask available to the next page open, which is
    /// the retry a user would expect anyway.
    ///
    /// Not asked while a run is in flight (the run owns the page) or while a
    /// check is already going.
    #[must_use]
    pub fn wants_first_check(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.checked_at.is_none())
            && !self.unsupported
            && !self.checking
            && self.run.is_none()
    }

    /// Whether the beta-updates toggle should render as on.
    ///
    /// The locally-owned value when there is one — it is the fresher of the
    /// two the moment the switch is pressed — falling back to the daemon's
    /// last-known status before any toggle or status has landed.
    #[must_use]
    pub fn beta_optin_shown(&self) -> bool {
        self.beta_optin
            .unwrap_or_else(|| self.status.as_ref().is_some_and(|s| s.beta_optin_effective))
    }

    /// Whether the daemon's last-known status reports an installable update.
    #[must_use]
    pub fn update_offered(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.update_available)
    }

    /// Whether the Update section (CTA / in-flight progress / terminal
    /// panel) should render at all: whenever a run exists — independent of
    /// what the latest status says, so a `Done` run's Restart CTA survives
    /// the post-update refetch that reports `update_available: false` — or
    /// the daemon currently offers an update with no run yet started.
    #[must_use]
    pub fn update_section_visible(&self) -> bool {
        self.run.is_some() || self.update_offered()
    }

    /// Whether the header-bar "Update available" badge should render: only
    /// while an update is offered and no apply run is in flight (once a run
    /// starts, the Updates page's phase readout is the source of truth).
    #[must_use]
    pub fn header_badge_visible(&self) -> bool {
        self.run.is_none() && self.update_offered()
    }

    /// Fold one apply-flow event into the in-flight run. A no-op if there's
    /// no run (the stream outlived its cancellation, or a stray event
    /// arrived after `DismissRun`/`CancelUpdate`). Returns whether the
    /// caller should follow up with a status refetch — only for the
    /// post-`Done` `Finished` case, since the daemon restarted onto the new
    /// version and the cached status is now stale.
    #[must_use]
    pub fn apply_run_event(&mut self, ev: UpdateRunEvent) -> RunOutcome {
        let Some(run) = self.run.as_mut() else {
            return RunOutcome::Continue;
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
                    _ => run.phase, // tolerate future/unknown phases
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
                run.error_code = Some(code);
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
                    // Daemon was restarted by the installer; caller refreshes
                    // status.
                    return RunOutcome::RefetchStatus;
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
        RunOutcome::Continue
    }
}

/// What the handler should do after [`UpdateState::apply_run_event`] folds
/// in one event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// State was updated; no further app-side action needed.
    Continue,
    /// The run just reached its post-`Done` `Finished` — the daemon
    /// restarted onto the new version, so the caller should re-fetch status.
    RefetchStatus,
}

pub struct UpdateRun {
    pub phase: RunPhase,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub error: Option<String>,
    /// The installer's typed error code for a `Failed` run (its closed set
    /// lives in `super-stt-install`'s `InstallError::code`). `None` for a
    /// failure raised app-side, which has no installer code to report.
    ///
    /// Kept beside `error` because the message is for the user to read and
    /// the code is for the UI to branch on — see
    /// [`Self::was_authorization_declined`].
    pub error_code: Option<String>,
    pub completed_components: Vec<String>,
}

impl UpdateRun {
    /// Whether this failure is the user declining the authorization prompt
    /// rather than something going wrong.
    ///
    /// Worth telling apart: every other failure leaves the user with an
    /// update they still cannot apply, and a way to install it outside the
    /// app is a real answer. Declining the prompt is not a dead end — the
    /// answer is to press Update again and authorize — so handing over a
    /// shell command reads as a non-sequitur to someone who just pressed
    /// Cancel.
    ///
    /// Keyed on the installer's typed code, never on the message's wording:
    /// the codes are a closed contract (`InstallError::code`), the prose is
    /// not, and `pkexec` localizes its own diagnostics.
    #[must_use]
    pub fn was_authorization_declined(&self) -> bool {
        self.error_code.as_deref() == Some("escalation_denied")
    }

    /// Cancel is only offered while nothing system-owned has been touched
    /// (spec §1: safe to kill before the escalate phase).
    #[must_use]
    pub fn cancellable(&self) -> bool {
        matches!(
            self.phase,
            RunPhase::FetchingInstaller
                | RunPhase::Resolve
                | RunPhase::Download
                | RunPhase::Verify
                | RunPhase::Stage
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    FetchingInstaller,
    Resolve,
    Download,
    Verify,
    Stage,
    WaitingAuth,
    Install,
    PostInstall,
    Done,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::{RunOutcome, RunPhase, UpdateRun, UpdateState};
    use crate::core::app::updater::{InstallerEvent, UpdateRunEvent};
    use super_stt_shared::models::self_update::{InstallerAsset, SelfUpdateStatus};

    /// All non-terminal (mid-flight) phases — used to sweep invariants that
    /// must hold across every phase before `Done`/`Failed`.
    const IN_FLIGHT_PHASES: [RunPhase; 8] = [
        RunPhase::FetchingInstaller,
        RunPhase::Resolve,
        RunPhase::Download,
        RunPhase::Verify,
        RunPhase::Stage,
        RunPhase::WaitingAuth,
        RunPhase::Install,
        RunPhase::PostInstall,
    ];

    fn run(phase: RunPhase) -> UpdateRun {
        UpdateRun {
            phase,
            bytes_done: 0,
            bytes_total: 0,
            error: None,
            error_code: None,
            completed_components: Vec::new(),
        }
    }

    fn status(update_available: bool) -> SelfUpdateStatus {
        SelfUpdateStatus {
            current_version: "0.2.2-beta.2".to_string(),
            latest_version: Some("v0.2.3".to_string()),
            update_available,
            checked_at: None,
            last_check_error: None,
            beta_optin_effective: false,
            installer_asset: Some(InstallerAsset {
                name: "super-stt-installer".to_string(),
                url: "https://example.invalid/installer".to_string(),
                size: 1024,
                sha256: "0".repeat(64),
            }),
        }
    }

    // ---- failure classification -------------------------------------------

    /// The installer's typed code has to survive into state, or the panel has
    /// nothing but prose to branch on — and `pkexec` localizes its prose.
    #[test]
    fn an_installer_failure_keeps_its_code_beside_its_message() {
        let mut update = UpdateState::default();
        update.begin_run();
        let outcome = update.apply_run_event(UpdateRunEvent::Installer(InstallerEvent::Error {
            code: "escalation_denied".to_string(),
            message: "authorization was denied".to_string(),
        }));
        assert_eq!(outcome, RunOutcome::Continue);

        let run = update.run.as_ref().expect("run");
        assert_eq!(run.phase, RunPhase::Failed);
        assert_eq!(run.error.as_deref(), Some("authorization was denied"));
        assert_eq!(run.error_code.as_deref(), Some("escalation_denied"));
    }

    /// An app-side failure never came from the installer, so it carries no
    /// code — and must not be mistaken for a declined prompt.
    #[test]
    fn an_app_side_failure_carries_no_installer_code() {
        let mut update = UpdateState::default();
        update.begin_run();
        let _ = update.apply_run_event(UpdateRunEvent::Failed("spawn failed".to_string()));

        let run = update.run.as_ref().expect("run");
        assert_eq!(run.phase, RunPhase::Failed);
        assert!(run.error_code.is_none());
        assert!(!run.was_authorization_declined());
    }

    /// Only the declined prompt is a declined prompt. `escalation_unavailable`
    /// in particular must NOT be: no polkit agent to authorize against is
    /// exactly the case where installing outside the app is the only way
    /// forward, so its panel has to keep offering that.
    #[test]
    fn only_escalation_denied_counts_as_a_declined_prompt() {
        let declined = UpdateRun {
            error_code: Some("escalation_denied".to_string()),
            ..run(RunPhase::Failed)
        };
        assert!(declined.was_authorization_declined());

        for code in [
            "escalation_unavailable",
            "install_failed",
            "post_install_failed",
            "checksum_mismatch",
            "download_failed",
            "no_release_found",
            "unsupported_arch",
        ] {
            let other = UpdateRun {
                error_code: Some(code.to_string()),
                ..run(RunPhase::Failed)
            };
            assert!(
                !other.was_authorization_declined(),
                "{code} is a real failure, not a declined prompt"
            );
        }

        assert!(!run(RunPhase::Failed).was_authorization_declined());
    }

    // ---- first-check kick ------------------------------------------------

    /// A daemon that has not checked yet reports no candidate at all, so the
    /// page and the badge read empty until its deferred first check runs.
    /// Asking once closes that window; the latch is what keeps a failing
    /// check from asking forever.
    #[test]
    fn a_never_checked_daemon_is_asked_to_check() {
        let mut update = UpdateState::default();
        update.apply_status_loaded(Some(SelfUpdateStatus {
            checked_at: None,
            ..status(false)
        }));
        assert!(update.wants_first_check());

        // The check that follows sets `checking`, which closes the ask for
        // the round trip; the status it returns closes it for good.
        update.checking = true;
        assert!(!update.wants_first_check());
    }

    #[test]
    fn a_daemon_that_has_already_checked_is_left_alone() {
        let mut update = UpdateState::default();
        update.apply_status_loaded(Some(SelfUpdateStatus {
            checked_at: Some("2026-08-25T00:00:00Z".to_string()),
            ..status(false)
        }));
        assert!(!update.wants_first_check());
    }

    #[test]
    fn no_first_check_before_a_status_or_against_an_unsupported_daemon() {
        assert!(
            !UpdateState::default().wants_first_check(),
            "nothing to conclude before the first status lands"
        );

        let mut update = UpdateState::default();
        update.apply_status_loaded(None);
        assert!(update.unsupported);
        assert!(!update.wants_first_check());
    }

    #[test]
    fn no_first_check_while_busy() {
        let never = SelfUpdateStatus {
            checked_at: None,
            ..status(false)
        };

        let checking = UpdateState {
            status: Some(never.clone()),
            checking: true,
            ..Default::default()
        };
        assert!(!checking.wants_first_check());

        let running = UpdateState {
            status: Some(never),
            run: Some(run(RunPhase::Download)),
            ..Default::default()
        };
        assert!(!running.wants_first_check());
    }

    fn beta_status(beta_optin_effective: bool) -> SelfUpdateStatus {
        SelfUpdateStatus {
            beta_optin_effective,
            ..status(false)
        }
    }

    // ---- beta opt-in toggle ---------------------------------------------

    /// Before anything is toggled the switch reports what the daemon says —
    /// including the pre-status default, where there is nothing to report.
    #[test]
    fn beta_toggle_falls_back_to_the_daemon_until_it_is_touched() {
        assert!(!UpdateState::default().beta_optin_shown());

        let mut update = UpdateState::default();
        update.apply_status_loaded(Some(beta_status(true)));
        assert!(update.beta_optin_shown());
    }

    /// The point of owning the value locally: pressing the switch moves it,
    /// without waiting for the network re-check that follows.
    #[test]
    fn beta_toggle_shows_the_pressed_value_before_a_status_arrives() {
        let mut update = UpdateState::default();
        update.apply_status_loaded(Some(beta_status(false)));

        // What the handler does on press.
        update.beta_optin = Some(true);
        update.beta_pending = true;

        assert!(update.beta_optin_shown());
        assert!(
            !update
                .status
                .as_ref()
                .expect("status stored")
                .beta_optin_effective,
            "the daemon has not been told yet — only the local value moved"
        );
    }

    /// A status fetched before our write landed must not drag the switch back
    /// under the user's finger, only for our own re-check to flip it again.
    #[test]
    fn a_stale_status_does_not_move_a_pending_beta_toggle() {
        let mut update = UpdateState {
            beta_optin: Some(true),
            beta_pending: true,
            ..Default::default()
        };
        update.apply_status_loaded(Some(beta_status(false)));
        assert!(update.beta_optin_shown(), "pending write wins");

        // Once the write settles, the daemon has the last word again.
        update.beta_pending = false;
        update.apply_status_loaded(Some(beta_status(false)));
        assert!(!update.beta_optin_shown());
    }

    /// Another client changing the channel is a real change, and the switch
    /// must follow it — the local value is a cache, not an override.
    #[test]
    fn an_unpending_status_resyncs_the_beta_toggle() {
        let mut update = UpdateState {
            beta_optin: Some(false),
            ..Default::default()
        };
        update.apply_status_loaded(Some(beta_status(true)));
        assert!(update.beta_optin_shown());
    }

    // ---- apply_status_loaded --------------------------------------------

    #[test]
    fn apply_status_loaded_none_marks_unsupported() {
        let mut update = UpdateState::default();
        update.apply_status_loaded(None);
        assert!(update.unsupported);
        assert!(update.status.is_none());
        assert!(!update.checking, "checking must end regardless of outcome");
    }

    #[test]
    fn apply_status_loaded_some_clears_unsupported_and_stores_status() {
        let mut update = UpdateState {
            unsupported: true,
            checking: true,
            ..Default::default()
        };
        update.apply_status_loaded(Some(status(true)));
        assert!(!update.unsupported);
        assert_eq!(
            update.status.as_ref().map(|s| s.update_available),
            Some(true)
        );
        assert!(!update.checking);
    }

    #[test]
    fn apply_status_error_sets_banner_and_ends_checking() {
        let mut update = UpdateState {
            checking: true,
            ..Default::default()
        };
        update.apply_status_error("boom");
        assert!(!update.checking);
        assert_eq!(
            update.action_error.as_deref(),
            Some("Couldn't fetch update status: boom")
        );
    }

    #[test]
    fn apply_setting_error_sets_its_own_banner_text() {
        let mut update = UpdateState::default();
        update.apply_setting_error("boom");
        assert_eq!(
            update.action_error.as_deref(),
            Some("Couldn't update setting: boom")
        );
    }

    // ---- clear_run_on_status_refresh: every RunPhase ----------------------

    /// (a) A completed app-component update's Restart affordance must
    /// survive the `Done → AvailableEventReceived → StatusLoaded` refetch
    /// that fires right after the daemon restarts onto the new version —
    /// even though that fresh status now reports `update_available: false`.
    #[test]
    fn clear_run_on_status_refresh_keeps_a_done_run() {
        let mut update = UpdateState {
            run: Some(UpdateRun {
                completed_components: vec!["daemon".to_string(), "app".to_string()],
                ..run(RunPhase::Done)
            }),
            ..Default::default()
        };

        update.clear_run_on_status_refresh();

        let run = update.run.expect("Done run must survive a status refresh");
        assert_eq!(run.phase, RunPhase::Done);
        assert!(run.completed_components.iter().any(|c| c == "app"));
    }

    /// A `Failed` run has no further affordance to preserve, so a fresh
    /// status refetch supersedes it rather than leaving stale error text
    /// masking newer information (e.g. a newer version now available).
    #[test]
    fn clear_run_on_status_refresh_clears_a_failed_run() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Failed)),
            ..Default::default()
        };

        update.clear_run_on_status_refresh();

        assert!(update.run.is_none());
    }

    /// An in-flight run must never be silently dropped by a status
    /// refetch — only `CancelUpdate` (which respects
    /// `UpdateRun::cancellable()`) or a terminal transition ends it. Swept
    /// across every mid-flight phase, not just one.
    #[test]
    fn clear_run_on_status_refresh_never_clears_an_in_flight_run() {
        for phase in IN_FLIGHT_PHASES {
            let mut update = UpdateState {
                run: Some(run(phase)),
                ..Default::default()
            };
            update.clear_run_on_status_refresh();
            assert_eq!(update.run.map(|r| r.phase), Some(phase), "phase {phase:?}");
        }
    }

    // ---- dismiss_run: every RunPhase --------------------------------------

    /// Dismissing a `Failed` run clears it — `can_start_update`'s guard is
    /// `run.is_none()`, so a cleared run means a subsequent `StartUpdate` is
    /// no longer blocked.
    #[test]
    fn dismiss_run_clears_a_failed_run_so_a_future_start_update_is_not_blocked() {
        let mut update = UpdateState {
            status: Some(status(true)),
            run: Some(run(RunPhase::Failed)),
            ..Default::default()
        };

        update.dismiss_run();

        assert!(
            update.run.is_none(),
            "can_start_update's `run.is_none()` guard must see None after Dismiss"
        );
        assert!(update.can_start_update());
    }

    /// Dismiss also clears a `Done` run (e.g. after the user chose not to
    /// restart, or a failed relaunch attempt) so it doesn't linger forever.
    #[test]
    fn dismiss_run_clears_a_done_run() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Done)),
            ..Default::default()
        };

        update.dismiss_run();

        assert!(update.run.is_none());
    }

    /// Dismiss must not be able to end an in-flight run in any mid-flight
    /// phase — that path belongs to `CancelUpdate`, which additionally
    /// enforces the escalate-phase safety cutoff (`UpdateRun::cancellable`).
    #[test]
    fn dismiss_run_never_clears_an_in_flight_run() {
        for phase in IN_FLIGHT_PHASES {
            let mut update = UpdateState {
                run: Some(run(phase)),
                ..Default::default()
            };
            update.dismiss_run();
            assert_eq!(update.run.map(|r| r.phase), Some(phase), "phase {phase:?}");
        }
    }

    // ---- apply_run_event: installer phase mapping -------------------------

    #[test]
    fn apply_run_event_maps_every_installer_phase() {
        let cases = [
            ("resolve", RunPhase::Resolve),
            ("download", RunPhase::Download),
            ("verify", RunPhase::Verify),
            ("stage", RunPhase::Stage),
            ("escalate", RunPhase::WaitingAuth),
            ("install", RunPhase::Install),
            ("post_install", RunPhase::PostInstall),
        ];
        for (wire, expected) in cases {
            let mut update = UpdateState {
                run: Some(run(RunPhase::FetchingInstaller)),
                ..Default::default()
            };
            let outcome =
                update.apply_run_event(UpdateRunEvent::Installer(InstallerEvent::Phase {
                    phase: wire.to_string(),
                    message: "x".to_string(),
                }));
            assert_eq!(outcome, RunOutcome::Continue);
            assert_eq!(
                update.run.map(|r| r.phase),
                Some(expected),
                "wire phase {wire:?}"
            );
        }
    }

    /// An unknown/future phase string must leave the current phase
    /// unchanged (forward-compat with a newer installer), never panic or
    /// reset to some default.
    #[test]
    fn apply_run_event_leaves_phase_unchanged_on_unknown_phase_string() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Stage)),
            ..Default::default()
        };
        let outcome = update.apply_run_event(UpdateRunEvent::Installer(InstallerEvent::Phase {
            phase: "defragment".to_string(),
            message: "x".to_string(),
        }));
        assert_eq!(outcome, RunOutcome::Continue);
        assert_eq!(update.run.map(|r| r.phase), Some(RunPhase::Stage));
    }

    #[test]
    fn apply_run_event_complete_records_completed_components() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Install)),
            ..Default::default()
        };
        let _ = update.apply_run_event(UpdateRunEvent::Installer(InstallerEvent::Complete {
            installed_version: "v0.2.3".to_string(),
            components: vec!["daemon".to_string(), "app".to_string()],
        }));
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::Done);
        assert_eq!(run.completed_components, vec!["daemon", "app"]);
    }

    #[test]
    fn apply_run_event_installer_error_lands_in_failed_with_message() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Verify)),
            ..Default::default()
        };
        let _ = update.apply_run_event(UpdateRunEvent::Installer(InstallerEvent::Error {
            code: "checksum_mismatch".to_string(),
            message: "boom".to_string(),
        }));
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::Failed);
        assert_eq!(run.error.as_deref(), Some("boom"));
    }

    #[test]
    fn apply_run_event_app_side_failed_lands_in_failed_with_message() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Download)),
            ..Default::default()
        };
        let _ = update.apply_run_event(UpdateRunEvent::Failed(
            "spawn installer-bin: boom".to_string(),
        ));
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::Failed);
        assert_eq!(run.error.as_deref(), Some("spawn installer-bin: boom"));
    }

    #[test]
    fn apply_run_event_finished_after_non_terminal_becomes_failed_with_stderr_tail() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Install)),
            ..Default::default()
        };
        let outcome = update.apply_run_event(UpdateRunEvent::Finished {
            exit_ok: false,
            stderr_tail: "line1\nline2".to_string(),
        });
        assert_eq!(outcome, RunOutcome::Continue);
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::Failed);
        assert_eq!(run.error.as_deref(), Some("installer failed: line1\nline2"));
    }

    #[test]
    fn apply_run_event_finished_exit_ok_after_non_terminal_is_still_failed() {
        // exit_ok but no terminal InstallerEvent ever arrived — the
        // installer exited without ever reporting Complete/Error.
        let mut update = UpdateState {
            run: Some(run(RunPhase::PostInstall)),
            ..Default::default()
        };
        let _ = update.apply_run_event(UpdateRunEvent::Finished {
            exit_ok: true,
            stderr_tail: String::new(),
        });
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::Failed);
        assert_eq!(run.error.as_deref(), Some("installer ended unexpectedly"));
    }

    #[test]
    fn apply_run_event_finished_after_done_keeps_done_and_requests_refetch() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Done)),
            ..Default::default()
        };
        let outcome = update.apply_run_event(UpdateRunEvent::Finished {
            exit_ok: true,
            stderr_tail: String::new(),
        });
        assert_eq!(outcome, RunOutcome::RefetchStatus);
        assert_eq!(update.run.map(|r| r.phase), Some(RunPhase::Done));
    }

    #[test]
    fn apply_run_event_finished_after_failed_stays_failed_unchanged() {
        let mut update = UpdateState {
            run: Some(UpdateRun {
                error: Some("original error".to_string()),
                ..run(RunPhase::Failed)
            }),
            ..Default::default()
        };
        let outcome = update.apply_run_event(UpdateRunEvent::Finished {
            exit_ok: false,
            stderr_tail: "ignored".to_string(),
        });
        assert_eq!(outcome, RunOutcome::Continue);
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::Failed);
        assert_eq!(
            run.error.as_deref(),
            Some("original error"),
            "must not overwrite the original failure with the exit reason"
        );
    }

    #[test]
    fn apply_run_event_is_a_no_op_without_an_active_run() {
        let mut update = UpdateState::default();
        let outcome = update.apply_run_event(UpdateRunEvent::Failed("x".to_string()));
        assert_eq!(outcome, RunOutcome::Continue);
        assert!(update.run.is_none());
    }

    // ---- byte-progress fields ----------------------------------------------

    #[test]
    fn fetch_progress_updates_byte_fields_and_phase() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Resolve)),
            ..Default::default()
        };
        let _ = update.apply_run_event(UpdateRunEvent::FetchProgress {
            bytes_done: 512,
            bytes_total: 2048,
        });
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::FetchingInstaller);
        assert_eq!(run.bytes_done, 512);
        assert_eq!(run.bytes_total, 2048);
    }

    #[test]
    fn installer_progress_updates_byte_fields_without_changing_phase() {
        let mut update = UpdateState {
            run: Some(run(RunPhase::Download)),
            ..Default::default()
        };
        let _ = update.apply_run_event(UpdateRunEvent::Installer(InstallerEvent::Progress {
            phase: "download".to_string(),
            bytes_done: 100,
            bytes_total: 400,
        }));
        let run = update.run.expect("run must still exist");
        assert_eq!(run.phase, RunPhase::Download);
        assert_eq!(run.bytes_done, 100);
        assert_eq!(run.bytes_total, 400);
    }

    // ---- predicates: truth tables ------------------------------------------

    #[test]
    fn can_start_update_truth_table() {
        // No status yet: never startable.
        assert!(!UpdateState::default().can_start_update());

        // Status with no installer asset (unsupported host arch): not startable.
        let mut update = UpdateState {
            status: Some(SelfUpdateStatus {
                installer_asset: None,
                ..status(true)
            }),
            ..Default::default()
        };
        assert!(!update.can_start_update());

        // Status with asset+tag, no run: startable.
        update.status = Some(status(true));
        assert!(update.can_start_update());

        // A run in ANY phase (including terminal) blocks a start.
        for phase in IN_FLIGHT_PHASES
            .into_iter()
            .chain([RunPhase::Done, RunPhase::Failed])
        {
            update.run = Some(run(phase));
            assert!(!update.can_start_update(), "phase {phase:?}");
        }

        // Dismissing a terminal run un-blocks it again (Dismiss/auto-clear
        // provide the reset).
        update.run = Some(run(RunPhase::Failed));
        update.dismiss_run();
        assert!(update.can_start_update());
    }

    /// An asset with no resolved tag can't be started — the view must gate
    /// its idle CTA on `can_start_update()` rather than re-deriving a
    /// weaker `installer_asset.is_some()` condition that misses this case.
    #[test]
    fn can_start_update_false_with_asset_but_no_tag() {
        let update = UpdateState {
            status: Some(SelfUpdateStatus {
                latest_version: None,
                ..status(true)
            }),
            ..Default::default()
        };
        assert!(!update.can_start_update());
    }

    #[test]
    fn can_check_now_truth_table() {
        assert!(UpdateState::default().can_check_now());

        assert!(
            !UpdateState {
                checking: true,
                ..Default::default()
            }
            .can_check_now()
        );

        assert!(
            !UpdateState {
                run: Some(run(RunPhase::Install)),
                ..Default::default()
            }
            .can_check_now()
        );

        // F1: an unsupported daemon must disable "Check now" too, not just
        // surface a repeat error on press.
        assert!(
            !UpdateState {
                unsupported: true,
                ..Default::default()
            }
            .can_check_now()
        );
    }

    #[test]
    fn update_offered_truth_table() {
        assert!(!UpdateState::default().update_offered());
        assert!(
            !UpdateState {
                status: Some(status(false)),
                ..Default::default()
            }
            .update_offered()
        );
        assert!(
            UpdateState {
                status: Some(status(true)),
                ..Default::default()
            }
            .update_offered()
        );
    }

    /// The run UI must render whenever a run exists, independent of
    /// `update_available` — the invariant that keeps the post-update
    /// "Restart Super STT" CTA alive through the refetch that reports no
    /// update.
    #[test]
    fn update_section_visible_truth_table() {
        assert!(!UpdateState::default().update_section_visible());

        assert!(
            UpdateState {
                status: Some(status(true)),
                ..Default::default()
            }
            .update_section_visible()
        );

        // A run makes the section visible even when the freshest status
        // says no update is available.
        assert!(
            UpdateState {
                status: Some(status(false)),
                run: Some(run(RunPhase::Done)),
                ..Default::default()
            }
            .update_section_visible()
        );

        assert!(
            UpdateState {
                status: None,
                run: Some(run(RunPhase::Install)),
                ..Default::default()
            }
            .update_section_visible()
        );
    }

    #[test]
    fn header_badge_visible_hides_once_a_run_starts() {
        let mut update = UpdateState {
            status: Some(status(true)),
            ..Default::default()
        };
        assert!(update.header_badge_visible());
        update.run = Some(run(RunPhase::FetchingInstaller));
        assert!(!update.header_badge_visible());
    }

    #[test]
    fn cancellable_truth_table() {
        for phase in [
            RunPhase::FetchingInstaller,
            RunPhase::Resolve,
            RunPhase::Download,
            RunPhase::Verify,
            RunPhase::Stage,
        ] {
            assert!(run(phase).cancellable(), "phase {phase:?}");
        }
        for phase in [
            RunPhase::WaitingAuth,
            RunPhase::Install,
            RunPhase::PostInstall,
            RunPhase::Done,
            RunPhase::Failed,
        ] {
            assert!(!run(phase).cancellable(), "phase {phase:?}");
        }
    }
}
