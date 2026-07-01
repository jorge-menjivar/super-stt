// SPDX-License-Identifier: GPL-3.0-only

//! Deserialization types for the daemon's installed-backend catalog
//! (`GET /backends`).
//!
//! A backend is an installed model provider discovered on disk. Each
//! one declares the models it serves, the secrets it needs (stored in
//! the system keyring), and the options it accepts (stored in the
//! daemon config). The settings UI renders one section per backend.

/// A single installed backend and everything the settings UI needs to
/// render its section.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct BackendInfo {
    /// Repo id the backend was installed from, e.g.
    /// `github.com/super-stt/openai`. Used as the daemon's keyring account
    /// key and option key.
    pub source: String,
    /// Human-readable backend name, e.g. `OpenAI`.
    pub name: String,
    /// Hosts the backend is permitted to reach (`[network].allowed_hosts` from
    /// its `backend.toml`). Empty for subprocess/local backends. Feeds the
    /// "Online model" badge so the user sees where a cloud backend's audio goes.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Models this backend serves.
    pub models: Vec<BackendModel>,
    /// Sensitive values (API keys, etc.) stored in the system keyring.
    pub secrets: Vec<BackendSecret>,
    /// Non-sensitive options stored in the daemon config.
    pub options: Vec<BackendOption>,
}

/// One model served by a backend.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct BackendModel {
    pub name: String,
    /// Provider string, e.g. `local_whisper` / `openai`. Parsed into a
    /// [`super_stt_shared::models::provider::Provider`] for selection.
    pub provider: String,
    /// Devices the model can be loaded onto. Non-empty `snake_case` values
    /// from `["cpu", "cuda", "metal", "none"]`. The settings UI surfaces
    /// these as the device choice in the active-backend card; `"none"`
    /// (the only-entry sentinel for online models) means no device picker
    /// is shown.
    #[serde(default)]
    pub supported_devices: Vec<String>,
    /// Conservative GPU memory estimate (weights + KV cache + overhead) in
    /// bytes; `0` when unknown or not GPU-resident. Drives the "may not fit"
    /// warning when a CUDA load is staged against the detected GPU memory.
    #[serde(default)]
    pub estimated_vram_bytes: u64,
    /// Whether this model supports multiple transcription languages (as
    /// opposed to a mono-lingual model baked for a single language).
    #[serde(default)]
    pub multilingual: bool,
    /// BCP-47 tags the model can transcribe, e.g. `["en", "es", "fr"]`.
    /// Empty for mono-lingual models. Deserialized from the daemon for
    /// forward use; the picker currently reads languages from the live
    /// resolution block, so this is not read yet.
    #[serde(default)]
    #[allow(dead_code)]
    pub supported_languages: Vec<String>,
    /// The model's built-in default language (BCP-47 tag). Deserialized from
    /// the daemon for forward use; not read yet.
    #[serde(default)]
    #[allow(dead_code)]
    pub primary_language: String,
}

/// A sensitive value the backend requires, stored in the system keyring.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct BackendSecret {
    /// `snake_case` identifier (the keyring account suffix; the backend reads
    /// it as `x-stt-secret-<name>`).
    pub name: String,
    /// Human-readable label for the UI. Falls back to `name` when absent.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// A non-sensitive option the backend accepts, stored in the daemon config.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct BackendOption {
    /// `snake_case` identifier (the daemon config key; the backend reads it as
    /// `x-stt-option-<name>`).
    pub name: String,
    /// Human-readable label for the UI. Falls back to `name` when absent.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub required: bool,
    /// Current effective value (override or default) reported by the daemon.
    #[serde(default)]
    pub value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::BackendInfo;

    /// `GET /backends` reports each backend's `[network].allowed_hosts`; the
    /// "Online model" badge reads them straight off `BackendInfo`.
    #[test]
    fn parses_allowed_hosts() {
        let json = serde_json::json!({
            "source": "github.com/super-stt/openai",
            "name": "OpenAI",
            "models": [],
            "secrets": [],
            "options": [],
            "allowed_hosts": ["api.openai.com"],
        });
        let info: BackendInfo = serde_json::from_value(json).unwrap();
        assert_eq!(info.allowed_hosts, vec!["api.openai.com".to_string()]);
    }

    /// A backend that declares no hosts (or an older daemon that omits the
    /// field) yields an empty list, not a parse error.
    #[test]
    fn allowed_hosts_defaults_empty_when_absent() {
        let json = serde_json::json!({
            "source": "s",
            "name": "n",
            "models": [],
            "secrets": [],
            "options": [],
        });
        let info: BackendInfo = serde_json::from_value(json).unwrap();
        assert!(info.allowed_hosts.is_empty());
    }
}
