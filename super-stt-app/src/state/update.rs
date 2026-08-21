// SPDX-License-Identifier: GPL-3.0-only
//! UI state for the self-update flow (the Updates page + header badge).

use super_stt_shared::models::self_update::SelfUpdateStatus;

/// Self-update state: last known daemon status, a settings-toggle error
/// banner, and the in-flight apply run (if any).
// No Debug derive: the abort handle isn't Debug.
#[derive(Default)]
pub struct UpdateState {
    /// Last known daemon-reported status. None until first load.
    pub status: Option<SelfUpdateStatus>,
    /// True once `GET /v1/update` returned 404 (daemon predates the feature).
    pub unsupported: bool,
    pub checking: bool,
    pub auto_check_enabled: Option<bool>,
    /// In-flight or finished update run; None when idle.
    pub run: Option<UpdateRun>,
    /// Aborts the run's task stream. Only honored before the escalate
    /// phase — see [`UpdateRun::cancellable`].
    pub run_abort: Option<cosmic::iced::task::Handle>,
    /// Error from a settings toggle, shown in the page banner.
    pub action_error: Option<String>,
}

pub struct UpdateRun {
    pub phase: RunPhase,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub error: Option<String>,
    pub completed_components: Vec<String>,
}

impl UpdateRun {
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
