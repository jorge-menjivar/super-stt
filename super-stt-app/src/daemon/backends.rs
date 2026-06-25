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
    /// Models this backend serves.
    pub models: Vec<BackendModel>,
    /// Sensitive values (API keys, etc.) stored in the system keyring.
    pub secrets: Vec<BackendSecret>,
    /// Non-sensitive options stored in the daemon config.
    pub options: Vec<BackendOption>,
}

/// One model served by a backend.
// reason: catalog fields are deserialized for future direct consumption; the
// picker currently reads supported languages from the live resolution block.
#[allow(dead_code)]
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
    /// Empty for mono-lingual models.
    #[serde(default)]
    pub supported_languages: Vec<String>,
    /// The model's built-in default language (BCP-47 tag). Shown in the
    /// language picker when no global or per-model override is set.
    #[serde(default)]
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
