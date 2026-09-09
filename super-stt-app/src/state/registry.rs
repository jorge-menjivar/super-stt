// SPDX-License-Identifier: GPL-3.0-only
//! UI state for the backend registry.

use std::collections::HashMap;

use super_stt_shared::registry::RegistryBackend;
use super_stt_shared::registry::events::{InstallError, InstallPhase};

#[derive(Debug, Clone, Default)]
pub struct RegistryState {
    pub backends: Vec<RegistryBackend>,
    pub generated_at: Option<String>,
    pub filters: Filters,
    pub installs: HashMap<String, InstallStatus>,
    /// Uninstall failures keyed by `source`, surfaced on the installed card.
    /// Cleared when the user retries that backend or it disappears from the
    /// reloaded catalog (i.e. the uninstall ultimately succeeded).
    pub uninstall_errors: HashMap<String, String>,
    /// Install-request failures that never produced a background install
    /// (`InstallFailedToStart`), keyed by `source`/repo-url. Surfaced on the
    /// Browse card so a rejected request isn't silently dropped (Tier 1 #15).
    /// Cleared when the user retries or the install ultimately succeeds.
    pub install_errors: HashMap<String, String>,
    pub last_refresh: Option<RefreshOutcome>,
    /// In-progress URL text for the Custom-repo input in the Download tab.
    pub custom_repo_input: String,
    /// Which install source the Add-a-backend drawer is showing.
    pub add_source: AddSource,
    /// Folder for a local import, before it is previewed or installed.
    ///
    /// A plain string rather than an `Option` because the drawer's field is
    /// typeable: a path can arrive from the picker or from the keyboard, and
    /// empty is the same "nothing yet" either way.
    pub add_folder: String,
    /// What the drawer's preview panel is showing. Every outcome of resolving
    /// a source lands here, successes and failures alike, so a rejected
    /// resolve is visible in the drawer instead of only in the log.
    pub add_preview: PreviewState,
    /// Which frame of the drawer's "reading" spinner to draw. Counts up and is
    /// taken modulo the frame count at draw time.
    ///
    /// Kept in state because the glyph is a still SVG: each frame is its own
    /// drawing, so something has to say which one. Only advances while
    /// [`PreviewState::Checking`] — the app subscribes to its tick on that
    /// condition and drops it again the moment the check ends.
    pub add_spin: usize,
}

impl RegistryState {
    /// What the Add-a-backend drawer would install: the pasted URL or the
    /// folder path, whichever source is selected.
    ///
    /// This is also the key the daemon reports progress under, because the
    /// install request carries exactly this string — so it is what matches a
    /// completion event back to the drawer that started it.
    #[must_use]
    pub fn add_key(&self) -> String {
        match self.add_source {
            AddSource::Repository => self.custom_repo_input.trim().to_string(),
            AddSource::Folder => self.add_folder.trim().to_string(),
        }
    }

    /// Empty the drawer, keeping the chosen source.
    ///
    /// Called once an install it started has finished: the preview described a
    /// backend that is now installed, so leaving it up would offer to install
    /// it again. The source kind survives because someone who just imported a
    /// folder is more likely to import another than to switch to a URL.
    pub fn clear_add_sheet(&mut self) {
        self.custom_repo_input.clear();
        self.add_folder.clear();
        self.add_preview = PreviewState::Empty;
        self.add_spin = 0;
    }
}

/// The install source the Add-a-backend drawer is pointed at. Both routes are
/// manual: neither is the published catalog, which is the Browse tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AddSource {
    /// A git repository URL, resolved through its forge's latest release.
    #[default]
    Repository,
    /// A directory on this machine holding a `backend.toml`.
    Folder,
}

impl AddSource {
    /// The dropdown's options, in the order it lists them.
    pub const ALL: [AddSource; 2] = [AddSource::Repository, AddSource::Folder];
}

/// The drawer's preview panel.
#[derive(Debug, Clone, Default)]
pub enum PreviewState {
    /// Nothing checked yet.
    #[default]
    Empty,
    /// A check is in flight against `source`.
    Checking(String),
    /// A backend was resolved.
    ///
    /// The daemon's `unverified_source` marker is deliberately not kept: every
    /// install this drawer can start is unverified, so a banner on each one is
    /// noise. The preview names the source and what it reaches, which is the
    /// same caution with facts attached.
    Ready(Box<super_stt_shared::registry::RegistryBackend>),
    /// The resolve failed. The daemon's message is the whole content: it names
    /// the repo or path and says what to do, so the drawer shows it verbatim.
    Failed(String),
}

impl PreviewState {
    /// The backend an Install press would install, if there is one.
    #[must_use]
    pub fn ready(&self) -> Option<&super_stt_shared::registry::RegistryBackend> {
        match self {
            PreviewState::Ready(backend) => Some(backend),
            _ => None,
        }
    }

    /// Whether a check is in flight.
    #[must_use]
    pub fn is_checking(&self) -> bool {
        matches!(self, PreviewState::Checking(_))
    }
}

/// Which kind of model a backend must serve to be listed.
///
/// A named enum rather than the `Option<bool>` [`Filters::online`] uses: "is a
/// post-processor" has no obvious true/false reading at a call site, and the
/// two stages are named things in the UI already.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RoleFilter {
    #[default]
    All,
    Transcription,
    PostProcessing,
}

impl RoleFilter {
    /// Whether a backend serving these model roles passes the filter.
    ///
    /// Takes the roles rather than a `BackendInfo` so it works for an installed
    /// backend and a registry entry alike — the two carry the same `role`
    /// strings in different structs.
    pub fn admits<'a>(self, roles: impl IntoIterator<Item = &'a str>) -> bool {
        let want_post_processor = match self {
            Self::All => return true,
            Self::Transcription => false,
            Self::PostProcessing => true,
        };
        roles
            .into_iter()
            .any(|role| (role == "post_processor") == want_post_processor)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub include_incompatible: bool,
    pub online: Option<bool>,
    pub search: String,
    /// Which kind of model a backend must serve.
    pub role: RoleFilter,
}

/// The Installed tab's filters. A separate value from the Browse tab's
/// [`Filters`] so narrowing one list does not silently narrow the other, and
/// without the fields that only make sense before installing (search over the
/// registry, incompatible entries).
#[derive(Debug, Clone, Default)]
pub struct InstalledFilters {
    pub online: Option<bool>,
    pub role: RoleFilter,
}

#[derive(Debug, Clone)]
pub struct InstallStatus {
    pub install_id: String,
    pub phase: InstallPhase,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub error: Option<InstallError>,
}

#[derive(Debug, Clone)]
pub enum RefreshOutcome {
    Ok,
    Failed(String),
}

impl RegistryState {
    pub fn by_source(&self) -> HashMap<&str, &RegistryBackend> {
        self.backends
            .iter()
            .map(|b| (b.source.as_str(), b))
            .collect()
    }
}

#[cfg(test)]
mod role_filter_tests {
    use super::RoleFilter;

    const STT: &str = "transcription";
    const PP: &str = "post_processor";

    /// "All" is the default and hides nothing, including a backend whose
    /// manifest declares no models at all.
    #[test]
    fn all_admits_everything() {
        assert_eq!(RoleFilter::default(), RoleFilter::All);
        assert!(RoleFilter::All.admits([STT]));
        assert!(RoleFilter::All.admits([PP]));
        assert!(RoleFilter::All.admits([]));
    }

    /// A filter keeps a backend that serves *at least one* model of that kind,
    /// so a dual-role backend appears under both — it genuinely offers both.
    #[test]
    fn a_dual_role_backend_survives_either_filter() {
        assert!(RoleFilter::Transcription.admits([STT, PP]));
        assert!(RoleFilter::PostProcessing.admits([STT, PP]));
    }

    #[test]
    fn a_single_role_backend_is_hidden_by_the_other_filter() {
        assert!(RoleFilter::Transcription.admits([STT]));
        assert!(!RoleFilter::Transcription.admits([PP]));
        assert!(RoleFilter::PostProcessing.admits([PP]));
        assert!(!RoleFilter::PostProcessing.admits([STT]));
    }

    /// An unrecognized role reads as transcription, matching the manifest
    /// default — a model from a newer backend stays visible rather than
    /// disappearing from both filters.
    #[test]
    fn an_unknown_role_reads_as_transcription() {
        assert!(RoleFilter::Transcription.admits(["quantum"]));
        assert!(!RoleFilter::PostProcessing.admits(["quantum"]));
    }

    /// A backend with no models is hidden by either specific filter: it serves
    /// nothing for that stage, which is what the filter asks.
    #[test]
    fn a_backend_with_no_models_matches_no_specific_filter() {
        assert!(!RoleFilter::Transcription.admits([]));
        assert!(!RoleFilter::PostProcessing.admits([]));
    }
}
