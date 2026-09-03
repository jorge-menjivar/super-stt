// SPDX-License-Identifier: GPL-3.0-only
//! Wire types for `/registry/backends` and friends. All fields `snake_case`.

use serde::{Deserialize, Serialize};

pub mod events;

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryBackend {
    pub id: String,
    /// The backend's reverse-DNS identifier, or `None` when the registry entry
    /// predates it. Names the install directory.
    #[serde(default)]
    pub backend_id: Option<String>,
    pub source: String,
    pub version: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub license: String,
    pub kind: String,
    /// The contract generation the backend declares, as published. Carried
    /// as a string so a client lists an entry whose generation it does not
    /// know; `compatibility` says whether this daemon can drive it.
    pub contract: String,
    /// The Super STT release that first understood `contract`, as stamped by
    /// the indexer. `None` for an index that predates the stamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_client: Option<String>,
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

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compatibility {
    pub compatible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_asset: Option<SelectedAsset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether the block is "this Super STT is too old" rather than "this
    /// machine cannot run it".
    ///
    /// The two are hidden differently. A host that lacks the right GPU will
    /// never run the asset, so Browse tucks it behind "Show incompatible"; a
    /// Super STT one version behind is a thing the user can fix in a minute,
    /// and hiding it hides the only notice they would get. `false` on an older
    /// daemon that does not send the field, which lists as it always did.
    #[serde(default)]
    pub needs_client_update: bool,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedAsset {
    pub target: String,
    /// Acceleration backends the selected build carries. A single entry is
    /// both read and written as a bare string, a list of two or more as an
    /// array — a client that declares this field as a plain `String` still
    /// parses the catalog for every asset that carries one runtime.
    #[serde(
        deserialize_with = "super_stt_registry_types::index::one_or_many_string",
        serialize_with = "super_stt_registry_types::index::one_or_many_string_ser"
    )]
    pub accel: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_sm: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cudnn: bool,
}

pub use super_stt_registry_types::index::IndexStale;

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InstallRequest {
    BySource { source: String },
    ByRepoUrl { repo_url: String },
    ByLocalPath { local_path: String },
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallAccepted {
    pub install_id: String,
    pub source: String,
    pub version: String,
    pub selected_asset: SelectedAsset,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRequest {
    pub source: String,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    pub from_version: String,
    pub to_version: String,
    pub noop: bool,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefreshResponse {
    pub schema_version: u32,
    pub generated_at: String,
    pub backend_count: usize,
}

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallResponse {
    pub uninstalled: bool,
    /// The backend was filling stage 1, which was emptied before the files
    /// went.
    pub was_active: bool,
    /// The backend was filling stage 2 — selected as the post-processor
    /// backend, loaded or not — which was emptied before the files went.
    /// Absent from an older daemon's answer, which reads as `false`.
    #[serde(default)]
    pub was_post_processor: bool,
}

pub use super_stt_registry_types::{is_safe_component, is_safe_relative_path};

#[cfg(test)]
mod tests {
    use super::RegistryBackend;

    /// The minimal `/registry/backends` entry every test below starts from,
    /// extending it with `serde_json::Value` indexing for the field each test
    /// cares about. One fixture rather than three keeps them from drifting
    /// apart on the fields that are merely required to parse at all.
    fn minimal_backend_json() -> serde_json::Value {
        serde_json::json!({
            "id": "openai",
            "source": "github.com/super-stt/openai",
            "version": "0.1.1",
            "name": "OpenAI",
            "license": "Apache-2.0",
            "kind": "wasm",
            "contract": "v1",
            "online": true,
            "supports_gpu": false,
            "supports_cpu": false,
            "models": [],
            "secrets": [],
            "options": [],
            "compatibility": { "compatible": true },
        })
    }

    /// A daemon that predates `update_available` still deserializes here. The
    /// field moved the update decision from the client to the daemon; a client
    /// that hard-required it would fail to list anything at all against a
    /// daemon that has not rolled over, which is worse than not knowing about
    /// an update.
    #[test]
    fn a_registry_entry_without_update_available_still_parses() {
        let mut v = minimal_backend_json();
        v["installed_version"] = serde_json::json!("0.1.0");
        let b: RegistryBackend = serde_json::from_value(v).expect("older payload must parse");
        assert!(
            !b.update_available,
            "an absent flag reads as no update, never as one"
        );
    }

    /// Unknown keys are ignored, so a newer daemon adding a field does not
    /// break a client built against this shape. The compatibility runs both
    /// ways or it is not compatibility.
    #[test]
    fn a_registry_entry_with_an_unknown_field_still_parses() {
        let mut v = minimal_backend_json();
        v["update_available"] = serde_json::json!(true);
        v["a_field_from_a_later_daemon"] = serde_json::json!(42);
        let b: RegistryBackend = serde_json::from_value(v).expect("newer payload must parse");
        assert!(b.update_available);
    }

    /// A daemon that predates `backend_id` still deserializes here, and a
    /// response carrying one round-trips.
    #[test]
    fn backend_id_is_optional_on_the_wire() {
        let without: RegistryBackend =
            serde_json::from_value(minimal_backend_json()).expect("parses without backend_id");
        assert!(without.backend_id.is_none());

        let mut v = minimal_backend_json();
        v["backend_id"] = serde_json::json!("app.super-stt.voxtral");
        let with: RegistryBackend = serde_json::from_value(v).expect("parses with backend_id");
        assert_eq!(with.backend_id.as_deref(), Some("app.super-stt.voxtral"));
    }

    fn selected(accel: &[&str]) -> super::SelectedAsset {
        super::SelectedAsset {
            target: "x86_64-unknown-linux-gnu".into(),
            accel: accel.iter().map(|a| (*a).to_string()).collect(),
            cuda_major: Some(12),
            cuda_sm: Some(86),
            cudnn: false,
        }
    }

    /// An app built before the list form declares `accel` as a required
    /// `String` and fails to parse the *whole* `/registry/backends` response
    /// when it turns into an array. Every asset carrying one runtime — which
    /// is every asset a backend can publish — therefore keeps the bare-string
    /// shape on the wire.
    #[test]
    fn a_single_accel_selection_still_serializes_as_a_bare_string() {
        let json = serde_json::to_string(&selected(&["cuda"])).expect("serializes");
        assert!(
            json.contains(r#""accel":"cuda""#),
            "accel is no longer a bare string; an older app cannot parse the catalog: {json}"
        );

        #[derive(serde::Deserialize)]
        struct DeployedSelectedAsset {
            accel: String,
        }
        let deployed: DeployedSelectedAsset =
            serde_json::from_str(&json).expect("an older app must still parse this");
        assert_eq!(deployed.accel, "cuda");
    }

    /// A build carrying two runtimes has no bare-string spelling, so it is
    /// written as the array it is, and read back unchanged.
    #[test]
    fn a_multi_accel_selection_serializes_as_an_array_and_round_trips() {
        let json = serde_json::to_string(&selected(&["cuda", "rocm"])).expect("serializes");
        assert!(
            json.contains(r#""accel":["cuda","rocm"]"#),
            "a multi-runtime build must keep its list: {json}"
        );
        let back: super::SelectedAsset = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(back.accel, vec!["cuda".to_string(), "rocm".to_string()]);
    }

    /// Read leniency is unchanged: a payload written either way parses.
    #[test]
    fn a_selection_parses_from_either_shape() {
        let scalar: super::SelectedAsset =
            serde_json::from_str(r#"{"target":"t","accel":"cuda"}"#).expect("scalar parses");
        assert_eq!(scalar.accel, vec!["cuda".to_string()]);
        let list: super::SelectedAsset =
            serde_json::from_str(r#"{"target":"t","accel":["cuda"]}"#).expect("list parses");
        assert_eq!(list.accel, vec!["cuda".to_string()]);
    }
}
