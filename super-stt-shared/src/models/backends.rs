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
    /// The installed `backend.toml`'s `[backend].description`, empty when the
    /// manifest omits it.
    ///
    /// A registry entry carries one too, but only for backends the registry
    /// lists: a sideloaded or imported-from-dir backend has no entry, and its
    /// description would otherwise be invisible everywhere in the UI. `default`
    /// so a payload written before the field existed still deserializes.
    #[serde(default)]
    pub description: String,
    /// The installed backend's `[backend].version`, read from the `backend.toml`
    /// on disk — what is installed, not what is published.
    ///
    /// For a backend the registry does not list (imported from a directory, or
    /// installed from an arbitrary repo) this is the only version there is; for
    /// the rest it is still the authoritative one, since the registry reports
    /// what a release offers rather than what this machine has. `default` so a
    /// payload written before the field existed still deserializes.
    #[serde(default)]
    pub version: String,
    /// `"wasm"` or `"subprocess"` — the backend's transport.
    #[serde(default)]
    pub kind: String,
    /// Hosts the backend is permitted to reach (`[network].allowed_hosts` from
    /// its `backend.toml`). Empty for subprocess/local backends. Feeds the
    /// "Online model" badge so the user sees where a cloud backend's audio goes.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Acceleration backends of the asset variant actually installed on this
    /// host, e.g. `["cuda"]` or `["cpu"]`.
    ///
    /// Empty for a backend imported from a local directory, where the binary's
    /// accel is not knowable, for one installed before the daemon recorded it,
    /// and for a `wasm` backend — its `installed.json` records `"wasm"` for its
    /// own purposes, but that is a transport, not an accelerator, so it is
    /// filtered before it reaches this field. Clients read an empty list as "no
    /// information" and fall back to each model's `supported_devices`.
    ///
    /// A client offering a device picker intersects: a `cpu` asset offers the
    /// CPU alone, an accelerated one offers both, since a GPU build still runs
    /// on the CPU.
    #[serde(default)]
    pub installed_accel: Vec<String>,
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
    /// What the model is for: `"transcription"` (the default) or
    /// `"post_processor"`. A settings UI filters its transcription-model picker
    /// and its post-processor picker on this.
    ///
    /// `default` rather than required, so a catalog from a daemon that predates
    /// the field still parses — reading every model as transcribing, which is
    /// what it was.
    #[serde(default = "default_role")]
    pub role: String,
}

/// The role a catalog entry without the key is read as.
fn default_role() -> String {
    "transcription".to_string()
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

impl BackendOption {
    /// Whether the backend declared this option a boolean, so a client can
    /// offer a switch rather than a free-text field.
    ///
    /// An option that declares no type is a string (the manifest default), so
    /// only an explicit `bool` qualifies.
    #[must_use]
    pub fn is_bool(&self) -> bool {
        self.r#type
            .as_deref()
            .is_some_and(|t| t.eq_ignore_ascii_case("bool"))
    }

    /// This option's effective value read as a boolean.
    ///
    /// `value` already carries the override-or-default the daemon resolved;
    /// `default` is the fallback for a payload that omits it. Anything that
    /// isn't a recognized true spelling is false, so a malformed stored value
    /// reads as off rather than failing the row.
    #[must_use]
    pub fn bool_value(&self) -> bool {
        let raw = self.value.as_deref().or(self.default.as_deref());
        raw.is_some_and(parse_bool)
    }

    /// The declared default read as a boolean; `None` when none was declared.
    ///
    /// Lets a client tell "the user chose this" from "this is just the
    /// default" — a toggle back to the default clears the override instead of
    /// storing a value identical to it.
    #[must_use]
    pub fn bool_default(&self) -> Option<bool> {
        self.default.as_deref().map(parse_bool)
    }
}

/// The true spellings a manifest or a stored override may use. TOML writes
/// `true`, but a config edited by hand (or a backend documenting `on`) should
/// not silently read as off.
fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::{BackendInfo, BackendModel, BackendOption};

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
            role: "transcription".into(),
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

    /// A catalog from a daemon that predates `role` still parses, and its
    /// models read as transcription models — which is what they were. A
    /// required field here would black out the whole settings UI against any
    /// daemon not yet upgraded.
    #[test]
    fn a_model_without_a_role_reads_as_transcription() {
        let json = serde_json::json!({ "name": "whisper-1" });
        let m: BackendModel = serde_json::from_value(json).expect("older payload must parse");
        assert_eq!(m.role, "transcription");

        let json = serde_json::json!({ "name": "cleanup", "role": "post_processor" });
        let m: BackendModel = serde_json::from_value(json).expect("parses");
        assert_eq!(m.role, "post_processor");
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

    /// A daemon that predates `version` still deserializes here, reporting an
    /// empty one rather than failing the whole catalog. The field was added to
    /// `GET /backends`; a client that hard-required it would black out the
    /// settings UI against any daemon not yet upgraded.
    #[test]
    fn a_backends_payload_without_a_version_still_parses() {
        let json = serde_json::json!({
            "source": "github.com/super-stt/openai",
            "name": "OpenAI",
            "kind": "wasm",
            "allowed_hosts": [],
            "models": [],
            "secrets": [],
            "options": [],
        });
        let b: BackendInfo = serde_json::from_value(json).expect("older payload must parse");
        assert_eq!(
            b.version, "",
            "a missing version reads as unknown, not an error"
        );
    }

    fn option(r#type: Option<&str>, default: Option<&str>, value: Option<&str>) -> BackendOption {
        BackendOption {
            name: "flag".into(),
            label: None,
            description: String::new(),
            r#type: r#type.map(Into::into),
            default: default.map(Into::into),
            required: false,
            value: value.map(Into::into),
        }
    }

    /// Only an explicit `bool` gets a switch: an untyped option is a string by
    /// manifest default, and rendering it as a toggle would silently rewrite
    /// whatever the user had stored in it.
    #[test]
    fn only_an_explicitly_bool_option_is_a_bool() {
        assert!(option(Some("bool"), None, None).is_bool());
        assert!(option(Some("BOOL"), None, None).is_bool());
        assert!(!option(Some("string"), None, None).is_bool());
        assert!(!option(None, None, None).is_bool());
    }

    /// The stored value wins over the default, and a payload carrying neither
    /// reads as off rather than panicking the row.
    #[test]
    fn a_bool_option_reads_its_value_then_its_default() {
        assert!(option(Some("bool"), Some("false"), Some("true")).bool_value());
        assert!(!option(Some("bool"), Some("true"), Some("false")).bool_value());
        assert!(option(Some("bool"), Some("true"), None).bool_value());
        assert!(!option(Some("bool"), None, None).bool_value());
    }

    /// A hand-edited config may spell truth several ways; anything else is off.
    #[test]
    fn the_spoken_spellings_of_true_all_read_as_on() {
        for raw in ["true", "TRUE", " 1 ", "yes", "on"] {
            assert!(
                option(Some("bool"), None, Some(raw)).bool_value(),
                "{raw:?} should read as on"
            );
        }
        for raw in ["false", "0", "no", "off", "", "maybe"] {
            assert!(
                !option(Some("bool"), None, Some(raw)).bool_value(),
                "{raw:?} should read as off"
            );
        }
    }

    /// An option declaring no default has none to compare against, so a client
    /// cannot decide a toggle is "back to default" and must store the value.
    #[test]
    fn an_option_without_a_default_reports_no_bool_default() {
        assert_eq!(
            option(Some("bool"), Some("true"), None).bool_default(),
            Some(true)
        );
        assert_eq!(
            option(Some("bool"), None, Some("true")).bool_default(),
            None
        );
    }
}
