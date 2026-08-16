// SPDX-License-Identifier: GPL-3.0-only
//! Wire types for `/registry/backends` and friends. All fields `snake_case`.

use serde::{Deserialize, Serialize};

pub mod events;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryListResponse {
    pub schema_version: u32,
    pub generated_at: String,
    pub backends: Vec<RegistryBackend>,
}

// A flat mirror of the `/registry/backends` JSON. The lint wants related flags
// grouped into a sub-struct, which here would reshape the wire payload to suit
// an internal API guideline.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryBackend {
    pub id: String,
    pub source: String,
    pub version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub license: String,
    pub kind: String,
    pub contract: String,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    pub online: bool,
    pub supports_gpu: bool,
    pub supports_cpu: bool,
    pub models: Vec<RegistryModel>,
    pub secrets: Vec<RegistrySecret>,
    pub options: Vec<RegistryOption>,
    pub compatibility: Compatibility,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    /// Whether `version` is newer than `installed_version`, decided by the
    /// daemon.
    ///
    /// The comparison is semver, and it belongs here rather than in each client
    /// for the same reason `installed_version` does: the daemon is what reads
    /// the installed manifest and owns the index, so it is the one place that
    /// can answer without a client re-deriving it. A client that wants to
    /// present the versions still has both.
    ///
    /// `false` when nothing is installed, when the installed version is at or
    /// ahead of the index's, or when either version does not parse.
    #[serde(default)]
    pub update_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_stale: Option<IndexStale>,
}

// The `/registry/backends` model/secret/option leaves are field-identical to
// the `index.json` leaves, so they share one canonical definition rather than
// drifting. Re-exported under the historical `Registry*` names.
pub use super_stt_registry_types::index::{
    IndexModel as RegistryModel, IndexOption as RegistryOption, IndexSecret as RegistrySecret,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compatibility {
    pub compatible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_asset: Option<SelectedAsset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedAsset {
    pub target: String,
    pub accel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_sm: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cudnn: bool,
}

pub use super_stt_registry_types::index::IndexStale;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InstallRequest {
    BySource { source: String },
    ByRepoUrl { repo_url: String },
    ByLocalPath { local_path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallAccepted {
    pub install_id: String,
    pub source: String,
    pub version: String,
    pub selected_asset: SelectedAsset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    pub from_version: String,
    pub to_version: String,
    pub noop: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResponse {
    pub schema_version: u32,
    pub generated_at: String,
    pub backend_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallResponse {
    pub uninstalled: bool,
    pub was_active: bool,
}

pub use super_stt_registry_types::{is_safe_component, is_safe_relative_path};
