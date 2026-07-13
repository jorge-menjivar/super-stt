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

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub include_incompatible: bool,
    pub online: Option<bool>,
    pub search: String,
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
