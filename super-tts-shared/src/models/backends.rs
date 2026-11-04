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

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A single installed backend and everything the settings UI needs to render
/// its section.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BackendInfo {
    /// Repo id the backend was installed from, e.g.
    /// `github.com/super-tts/openai`. Used as the daemon's keyring account
    /// key and option key.
    pub source: String,
    /// Human-readable backend name, e.g. `OpenAI`.
    pub name: String,
    /// The installed `backend.toml`'s `[backend].description`, which the
    /// manifest parser requires to be non-empty.
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

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
    /// [`IndexModel::provider`]: super_tts_registry_types::index::IndexModel::provider
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
    /// Whether this model supports multiple speech languages (as
    /// opposed to a mono-lingual model baked for a single language).
    #[serde(default)]
    pub multilingual: bool,
    /// BCP-47 tags the model can speak, e.g. `["en", "es", "fr"]`.
    /// Empty for mono-lingual models.
    #[serde(default)]
    pub supported_languages: Vec<String>,
    /// The model's built-in default language (BCP-47 tag).
    #[serde(default)]
    pub primary_language: String,
    /// Whether the model is driven over the realtime WebSocket path rather than
    /// batch `POST /v1/synthesize`.
    #[serde(default)]
    pub realtime: bool,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A sensitive value the backend requires, stored in the system keyring.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BackendSecret {
    /// `snake_case` identifier (the keyring account suffix; the backend reads
    /// it as `x-tts-secret-<name>`).
    pub name: String,
    /// Human-readable label for the UI. Falls back to `name` when absent.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
/// A non-sensitive option the backend accepts, stored in the daemon config.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BackendOption {
    /// `snake_case` identifier (the daemon config key; the backend reads it as
    /// `x-tts-option-<name>`).
    pub name: String,
    /// Human-readable label for the UI. Falls back to `name` when absent.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub description: String,
    /// The option's input type (`string` / `integer` / `float` / `bool`);
    /// absent when the backend declared none, which means `string`.
    ///
    /// Drives the input a client renders, and what the daemon accepts as a
    /// value: a write of something that is not of this type is refused with
    /// `400 invalid_value`.
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    /// The values this option accepts, when it accepts a closed set. Empty
    /// means any value of `type`, so a client renders a free-text field;
    /// a non-empty list is a dropdown, and the daemon refuses a write of
    /// anything outside it.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Inclusive bounds for a numeric option, when it declares them. The
    /// daemon refuses a write outside them.
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// The increment a numeric option moves in. Present with `min` and `max`
    /// on an option a client should render as a slider; the grid is the
    /// control's, and the daemon accepts any value within the bounds.
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub required: bool,
    /// Current effective value (override or default) reported by the daemon.
    #[serde(default)]
    pub value: Option<String>,
}

impl BackendOption {
    /// Whether a client should render this option as a slider: a numeric
    /// option bounded at both ends and moving in a declared increment.
    ///
    /// The same shape-implies-control rule as `choices`, which renders a
    /// dropdown. An option missing any of the three has something left to type
    /// and gets a field instead.
    #[must_use]
    pub fn is_slider(&self) -> bool {
        matches!(self.r#type.as_deref(), Some("integer" | "float"))
            && self.min.is_some()
            && self.max.is_some()
            && self.step.is_some()
    }

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

    /// Whether this option offers a closed set of values, so a client shows a
    /// dropdown rather than a text field.
    ///
    /// A `bool` never does: it is a switch, whose two values the type already
    /// names. A manifest declaring both is refused at publication, and this
    /// keeps an older one that slipped through rendering as the switch.
    #[must_use]
    pub fn has_choices(&self) -> bool {
        !self.choices.is_empty() && !self.is_bool()
    }

    /// Where this option's effective value sits in [`Self::choices`].
    ///
    /// `None` when nothing is set, and also when the stored value is not one
    /// of the offered ones — a backend that dropped a choice its user had
    /// picked. The dropdown then shows no selection rather than the wrong one,
    /// and the user picks again.
    #[must_use]
    pub fn choice_index(&self) -> Option<usize> {
        let current = self.value.as_deref().or(self.default.as_deref())?;
        self.choices.iter().position(|c| c == current)
    }
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
            name: "kokoro-1".into(),
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
            "name": "kokoro-1",
            "provider": "local_kokoro",
            "supported_devices": ["cpu"],
        });
        let m: BackendModel = serde_json::from_value(json).expect("parses with `provider` present");
        assert_eq!(m.name, "kokoro-1");
        assert_eq!(m.provider, "", "the shim must not adopt an incoming value");

        let without = serde_json::json!({ "name": "kokoro-1" });
        let m: BackendModel =
            serde_json::from_value(without).expect("parses with `provider` absent");
        assert_eq!(m.provider, "");
    }

    /// `GET /backends` reports each backend's `[network].allowed_hosts`; the
    /// "Online model" badge reads them straight off `BackendInfo`.
    #[test]
    fn parses_allowed_hosts() {
        let json = serde_json::json!({
            "source": "github.com/super-tts/openai",
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
            "source": "github.com/super-tts/openai",
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
            choices: Vec::new(),
            min: None,
            max: None,
            step: None,
            required: false,
            value: value.map(Into::into),
        }
    }

    fn with_choices(
        r#type: Option<&str>,
        default: Option<&str>,
        value: Option<&str>,
        choices: &[&str],
    ) -> BackendOption {
        let mut opt = option(r#type, default, value);
        opt.choices = choices.iter().map(|c| (*c).to_string()).collect();
        opt
    }

    /// A closed set is what a client renders a dropdown from, so it has to be
    /// distinguishable from the open-ended case by the payload alone.
    #[test]
    fn only_an_option_offering_values_gets_a_dropdown() {
        assert!(with_choices(Some("string"), None, None, &["a", "b"]).has_choices());
        assert!(!option(Some("string"), None, None).has_choices());
        assert!(
            !with_choices(Some("bool"), None, None, &["a", "b"]).has_choices(),
            "a bool is a switch, whatever else it declares"
        );
    }

    /// The dropdown shows the effective value, and shows nothing at all when
    /// that value is not on the list — a backend that dropped a choice its
    /// user had picked. Selecting the wrong row would be worse than selecting
    /// none.
    #[test]
    fn the_dropdown_selects_the_effective_value_or_nothing() {
        let offered = ["casual", "formal"];
        assert_eq!(
            with_choices(Some("string"), Some("casual"), Some("formal"), &offered).choice_index(),
            Some(1),
            "the override wins over the default"
        );
        assert_eq!(
            with_choices(Some("string"), Some("casual"), None, &offered).choice_index(),
            Some(0),
            "and the default stands in for it"
        );
        assert_eq!(
            with_choices(Some("string"), None, Some("formalish"), &offered).choice_index(),
            None,
            "a value no longer offered selects nothing"
        );
        assert_eq!(
            with_choices(Some("string"), None, None, &offered).choice_index(),
            None
        );
    }
}
