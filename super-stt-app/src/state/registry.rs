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
