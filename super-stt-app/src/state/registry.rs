// SPDX-License-Identifier: GPL-3.0-only
//! UI state for the backend registry.

use std::collections::{HashMap, HashSet};

use super_stt_shared::registry::RegistryBackend;
use super_stt_shared::registry::events::{InstallError, InstallPhase};

#[derive(Debug, Clone, Default)]
pub struct RegistryState {
    pub backends: Vec<RegistryBackend>,
    pub generated_at: Option<String>,
    pub filters: Filters,
    pub installs: HashMap<String, InstallStatus>,
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

    // Used by the SSE event handler in a later batch (P3 batch D).
    #[allow(dead_code)]
    pub fn in_flight_sources(&self) -> HashSet<&str> {
        self.installs
            .iter()
            .filter(|(_, s)| s.error.is_none() && !matches!(s.phase, InstallPhase::Rescanning))
            .map(|(k, _)| k.as_str())
            .collect()
    }
}
