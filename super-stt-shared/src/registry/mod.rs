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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_stale: Option<IndexStale>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryModel {
    pub name: String,
    pub provider: String,
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySecret {
    pub name: String,
    pub label: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryOption {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStale {
    pub latest_attempted: String,
    pub tag: String,
    pub error: String,
    pub since: String,
}

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

/// A backend `id` or `entrypoint` must be a single, relative, non-traversing
/// path component before it is used in a `Path::join`. Reject empty, `.`,
/// `..`, anything containing a path separator, and embedded NUL — these are
/// the inputs that would let a `join` escape the backends directory or select
/// an absolute host path.
#[must_use]
pub fn is_safe_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::is_safe_component;

    #[test]
    fn accepts_plain_components() {
        assert!(is_safe_component("openai.wasm"));
        assert!(is_safe_component("super-stt-backend-whisper"));
        assert!(is_safe_component("mistral"));
    }

    #[test]
    fn rejects_traversal_and_separators() {
        assert!(!is_safe_component(""));
        assert!(!is_safe_component("."));
        assert!(!is_safe_component(".."));
        assert!(!is_safe_component("../evil"));
        assert!(!is_safe_component("a/b"));
        assert!(!is_safe_component("/usr/bin/python3"));
        assert!(!is_safe_component("a\\b"));
        assert!(!is_safe_component("a\0b"));
    }
}
