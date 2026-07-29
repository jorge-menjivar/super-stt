// SPDX-License-Identifier: GPL-3.0-only

//! The `GET /backends` installed-backend catalog response.
//!
//! A backend is an installed model provider discovered on disk. Each one
//! declares the models it serves, the secrets it needs (stored in the system
//! keyring), and the options it accepts (stored in the daemon config). The
//! daemon serializes this catalog from its discovered backends; the settings UI
//! deserializes it and renders one section per backend. Keeping the shape here,
//! shared by both sides, is what keeps the wire contract from drifting.
//!
//! `#[serde(default)]` on the non-identity fields lets an older daemon that
//! omits a newer field still deserialize (the value simply defaults).

use serde::{Deserialize, Serialize};

/// A single installed backend and everything the settings UI needs to render
/// its section.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BackendInfo {
    /// Repo id the backend was installed from, e.g.
    /// `github.com/super-stt/openai`. Used as the daemon's keyring account
    /// key and option key.
    pub source: String,
    /// Human-readable backend name, e.g. `OpenAI`.
    pub name: String,
    /// `"wasm"` or `"subprocess"` — the backend's transport.
    #[serde(default)]
    pub kind: String,
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
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BackendModel {
    pub name: String,
    /// Compatibility shim, mirroring [`IndexModel::provider`]. Always an empty
    /// string; a model is identified by `(name, source)`.
    ///
    /// It cannot simply be dropped: clients through v0.2.0 declare it a
    /// required `String` with no `#[serde(default)]`, so a payload without the
    /// key fails to deserialize *in full* on every installed one of them — the
    /// whole `GET /backends` catalog, not just this field. The settings UI then
    /// lists no installed backends at all.
    ///
    /// `skip_deserializing` keeps it write-only: it is emitted for those
    /// clients but never read back, so nothing here can start depending on it.
    ///
    /// Delete the field once no supported client requires the key.
    ///
    /// [`IndexModel::provider`]: super_stt_registry_types::index::IndexModel::provider
    #[serde(default, skip_deserializing)]
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
    /// The model's built-in default language (BCP-47 tag).
    #[serde(default)]
    pub primary_language: String,
    /// Whether the model is driven over the realtime WebSocket path rather than
    /// batch `POST /v1/transcribe`.
    #[serde(default)]
    pub realtime: bool,
}

/// A sensitive value the backend requires, stored in the system keyring.
#[derive(Serialize, Deserialize, Clone, Debug)]
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
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BackendOption {
    /// `snake_case` identifier (the daemon config key; the backend reads it as
    /// `x-stt-option-<name>`).
    pub name: String,
    /// Human-readable label for the UI. Falls back to `name` when absent.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    /// The option's input type (`string` / `integer` / `bool`); absent when the
    /// backend declared none.
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
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
    use super::{BackendInfo, BackendModel};

    /// `GET /backends` must keep carrying `provider` on every model. Clients
    /// through v0.2.0 declare it a required `String`, so a payload without it
    /// fails to deserialize *in full* on every installed one of them: the
    /// settings UI lists no backends, and no secret, option, or model switch
    /// is reachable.
    ///
    /// This is the test that fails if the compatibility shim is deleted before
    /// those clients have rolled over.
    #[test]
    fn the_backends_catalog_still_carries_the_provider_key() {
        let m = BackendModel {
            name: "whisper-1".into(),
            provider: String::new(),
            supported_devices: vec!["cpu".into()],
            estimated_vram_bytes: 0,
            multilingual: false,
            supported_languages: Vec::new(),
            primary_language: String::new(),
            realtime: false,
        };
        let v = serde_json::to_value(&m).expect("serializes");
        assert!(
            v.get("provider").is_some(),
            "GET /backends dropped `provider`; clients <= v0.2.0 cannot parse this: {v}"
        );
    }

    /// The shim is write-only: a payload carrying a `provider` still parses,
    /// and the value is not adopted. Nothing on this side may start reading a
    /// key that is on its way out.
    #[test]
    fn an_incoming_provider_is_tolerated_but_not_read() {
        let json = serde_json::json!({
            "name": "whisper-1",
            "provider": "local_whisper",
            "supported_devices": ["cpu"],
        });
        let m: BackendModel = serde_json::from_value(json).expect("parses with `provider` present");
        assert_eq!(m.name, "whisper-1");
        assert_eq!(m.provider, "", "the shim must not adopt an incoming value");

        let without = serde_json::json!({ "name": "whisper-1" });
        let m: BackendModel =
            serde_json::from_value(without).expect("parses with `provider` absent");
        assert_eq!(m.provider, "");
    }

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
