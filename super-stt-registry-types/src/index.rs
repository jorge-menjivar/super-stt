// SPDX-License-Identifier: GPL-3.0-only
//! Canonical `index.json` schema — the registry catalog the indexer publishes
//! and the daemon consumes. Previously this shape was declared three times
//! (indexer producer, daemon consumer, and the shared `/registry/backends`
//! leaf types), kept in sync by comment only and prone to drift. It now lives
//! here once; the producer and consumer both use these types, and the
//! `/registry/backends` response reuses the `IndexModel`/`IndexSecret`/
//! `IndexOption`/`IndexStale` leaves.
//!
//! Daemon-only policy (the `min_client` soft-floor check and the unsafe-path
//! backend filter) stays in the daemon as free functions over [`Index`], the
//! same extension pattern `validate_runtime` uses for `Manifest`.

use crate::manifest::{Device, Manifest, ModelEntry};
use serde::{Deserialize, Serialize};

/// `index.json` schema version the indexer emits.
pub const SCHEMA_VERSION: u32 = 1;

/// Soft floor: the minimum Super STT client (daemon) version expected to
/// understand this index. Older clients still use the registry but are warned
/// to update. Compared with standard semver precedence on the consumer side.
pub const MIN_CLIENT: &str = "0.1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub schema_version: u32,
    pub generated_at: String,
    pub min_client: String,
    pub backends: Vec<IndexBackend>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexBackend {
    pub id: String,
    pub source: String,
    pub version: String,
    pub tag: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    // Lenient on read (a slightly-off index must not fail the whole catalog),
    // always written by the producer. Resolves the old producer-required vs
    // consumer-defaulted drift.
    #[serde(default)]
    pub license: String,
    pub kind: String,
    pub contract: String,
    pub entrypoint: String,
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    pub online: bool,
    pub supports_gpu: bool,
    pub supports_cpu: bool,
    pub models: Vec<IndexModel>,
    pub secrets: Vec<IndexSecret>,
    pub options: Vec<IndexOption>,
    pub assets: IndexAssets,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_stale: Option<IndexStale>,
    /// Pinned `backend.toml` release asset. When present, the daemon installs
    /// these exact bytes (verified against `sha256`) instead of synthesizing a
    /// manifest from the loosely-typed fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<IndexAsset>,
}

/// `<host>/<owner>/<repo>` → `<repo>`. The install-dir name the daemon derives
/// from a backend's `source` (custom-repo and local-dir installs). The registry
/// indexer uses the maintainer-declared `id` instead, so this is only shared by
/// the two daemon-side synthesis paths.
#[must_use]
pub fn id_from_source(source: &str) -> String {
    source.rsplit('/').next().unwrap_or(source).to_string()
}

/// Index-level capability flags derived from a manifest's declared models.
struct ModelSupport {
    online: bool,
    supports_gpu: bool,
    supports_cpu: bool,
}

/// Classify a manifest's models by their [`Device`]s — the `none` sentinel
/// marks online/remote models, `cuda`/`metal` mark GPU, `cpu` marks CPU. Devices
/// are typed and validated by `Manifest::parse`, so there is no string matching
/// to get wrong; the provider string is irrelevant here.
fn model_support(models: &[ModelEntry]) -> ModelSupport {
    let any_device =
        |pred: fn(&Device) -> bool| models.iter().any(|m| m.supported_devices.iter().any(pred));
    ModelSupport {
        online: models.iter().any(ModelEntry::is_online),
        supports_gpu: any_device(|d| matches!(d, Device::Cuda | Device::Metal)),
        supports_cpu: any_device(|d| matches!(d, Device::Cpu)),
    }
}

impl IndexBackend {
    /// Assemble the manifest-derived fields of an index entry from a validated
    /// [`Manifest`]. The caller supplies the source-specific pins — the resolved
    /// `id`, `version`, `tag`, hashed `assets`, and optional pinned `manifest`
    /// asset — because those differ per install path (the registry uses the
    /// maintainer-declared id + release version; custom-repo/local-dir use
    /// [`id_from_source`] + the manifest version).
    ///
    /// Everything else — `online`/`supports_gpu`/`supports_cpu`, and the
    /// secret/option label (falls back to the item `name`) and option `type`
    /// (defaults to `"string"`) — is derived here so every install path renders
    /// a backend identically. This is the single mapping that the indexer, the
    /// custom-repo resolver, and the local-dir resolver all share; previously
    /// the local-dir path silently dropped secrets and options entirely.
    #[must_use]
    pub fn from_manifest(
        id: String,
        m: Manifest,
        version: String,
        tag: String,
        assets: IndexAssets,
        manifest: Option<IndexAsset>,
    ) -> Self {
        let ModelSupport {
            online,
            supports_gpu,
            supports_cpu,
        } = model_support(&m.models);
        Self {
            id,
            source: m.backend.source,
            version,
            tag,
            name: m.backend.name,
            description: Some(m.backend.description),
            license: m.backend.license.unwrap_or_default(),
            kind: m.backend.kind.to_string(),
            contract: m.backend.contract.to_string(),
            entrypoint: m.backend.entrypoint,
            allowed_hosts: m.network.allowed_hosts,
            online,
            supports_gpu,
            supports_cpu,
            models: m
                .models
                .into_iter()
                .map(|md| IndexModel {
                    name: md.name,
                    provider: md.provider.to_string(),
                    supported_devices: md
                        .supported_devices
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                })
                .collect(),
            secrets: m
                .secrets
                .into_iter()
                .map(|s| IndexSecret {
                    label: s.label.unwrap_or_else(|| s.name.clone()),
                    name: s.name,
                    required: s.required,
                })
                .collect(),
            options: m
                .options
                .into_iter()
                .map(|o| IndexOption {
                    label: o.label.unwrap_or_else(|| o.name.clone()),
                    name: o.name,
                    r#type: o
                        .r#type
                        .map_or_else(|| "string".to_string(), |t| t.to_string()),
                    // Untagged serialize yields the plain JSON value
                    // (string/number/bool). `.ok()` drops the theoretically
                    // impossible failure rather than panicking in a library.
                    default: o.default.and_then(|d| serde_json::to_value(d).ok()),
                })
                .collect(),
            assets,
            index_stale: None,
            manifest,
        }
    }
}

/// The browse-only model subset the catalog and host-compatibility filter need
/// before download. The authoritative manifest (languages, files, …) ships as
/// the pinned `manifest` asset and is installed verbatim — it is not re-encoded
/// here. Also the leaf type for `/registry/backends` (`RegistryModel`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexModel {
    pub name: String,
    pub provider: String,
    pub supported_devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSecret {
    pub name: String,
    pub label: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexOption {
    pub name: String,
    pub label: String,
    pub r#type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexAssets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<IndexAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subprocess: Vec<IndexSubprocessAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexAsset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexSubprocessAsset {
    pub target: String,
    pub accel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_sm: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cudnn: bool,
    /// Single-file archive pin. Present for a single-file variant; omitted when
    /// the archive is delivered as `parts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Multi-part archive: ordered part pins whose byte-for-byte concatenation
    /// is the `.tar.gz`. Present for a multi-part variant; omitted for
    /// single-file. Each part is hash-verified independently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<IndexAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStale {
    pub latest_attempted: String,
    pub tag: String,
    pub error: String,
    pub since: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ModelEntry;
    use crate::provider::Provider;

    /// Build a `ModelEntry` with the given provider and devices; the other
    /// fields are irrelevant to `model_support` (which reads only devices).
    fn model(provider: &str, devices: &[Device]) -> ModelEntry {
        ModelEntry {
            name: "m".into(),
            provider: Provider::from(provider),
            multilingual: true,
            primary_language: "en".into(),
            supported_languages: vec!["en".into()],
            supported_devices: devices.to_vec(),
            estimated_vram_bytes: 0,
            processing_interval_ms: None,
            realtime: false,
            files: vec![],
        }
    }

    #[test]
    fn classify_marks_online_from_none_device_not_provider() {
        // Online-ness is derived from the `none` device sentinel, not the
        // (free-form) provider string.
        assert!(model_support(&[model("openai", &[Device::None])]).online);
        assert!(model_support(&[model("mistral", &[Device::None])]).online);
        assert!(model_support(&[model("totally_bogus", &[Device::None])]).online);
        assert!(!model_support(&[model("openai", &[Device::Cpu])]).online);
        assert!(!model_support(&[model("local_whisper", &[Device::Cpu])]).online);
        assert!(!model_support(&[model("openai", &[])]).online);
    }

    #[test]
    fn classify_marks_gpu_for_cuda_or_metal() {
        assert!(model_support(&[model("local_whisper", &[Device::Cuda])]).supports_gpu);
        assert!(model_support(&[model("local_whisper", &[Device::Metal])]).supports_gpu);
        assert!(!model_support(&[model("local_whisper", &[Device::Cpu])]).supports_gpu);
    }

    #[test]
    fn classify_marks_cpu_only_for_cpu_device() {
        assert!(model_support(&[model("local_whisper", &[Device::Cpu])]).supports_cpu);
        assert!(!model_support(&[model("openai", &[Device::None])]).supports_cpu);
        assert!(!model_support(&[model("local_whisper", &[Device::Cuda])]).supports_cpu);
    }

    #[test]
    fn roundtrips_a_minimal_index() {
        let idx = Index {
            schema_version: SCHEMA_VERSION,
            generated_at: "2026-05-29T18:00:00Z".into(),
            min_client: MIN_CLIENT.into(),
            backends: vec![IndexBackend {
                id: "openai".into(),
                source: "github.com/x/y".into(),
                version: "1.0.0".into(),
                tag: "v1.0.0".into(),
                name: "OpenAI".into(),
                description: None,
                license: "Apache-2.0".into(),
                kind: "wasm".into(),
                contract: "v1".into(),
                entrypoint: "openai.wasm".into(),
                allowed_hosts: vec!["api.openai.com".into()],
                online: true,
                supports_gpu: false,
                supports_cpu: false,
                models: vec![],
                secrets: vec![],
                options: vec![],
                assets: IndexAssets {
                    wasm: Some(IndexAsset {
                        url: "https://x".into(),
                        size: 1,
                        sha256: "abc".into(),
                    }),
                    subprocess: vec![],
                },
                index_stale: None,
                manifest: None,
            }],
        };
        let s = serde_json::to_string_pretty(&idx).unwrap();
        let back: Index = serde_json::from_str(&s).unwrap();
        assert_eq!(back.backends.len(), 1);
        assert_eq!(back.backends[0].id, "openai");
    }

    /// The unified synthesis maps secrets and options (the local-dir path used
    /// to drop them) and applies the documented defaults: a label-less
    /// secret/option falls back to its `name`, and a type-less option defaults
    /// to `"string"` — not the empty strings the custom-repo path produced.
    #[test]
    fn from_manifest_maps_secrets_options_with_name_and_type_fallbacks() {
        let m = crate::manifest::Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
            description = "Test backend."

            [assets]
            wasm = "y.wasm"

            [[secrets]]
            name = "y_api_key"
            description = "Key."

            [[options]]
            name = "base_url"
            description = "Override."
            "#,
        )
        .unwrap();

        let b = IndexBackend::from_manifest(
            id_from_source("github.com/x/y"),
            m,
            "1.0.0".into(),
            "v1.0.0".into(),
            IndexAssets::default(),
            None,
        );

        assert_eq!(b.id, "y");
        assert_eq!(b.secrets.len(), 1, "secret must not be dropped");
        assert_eq!(b.secrets[0].name, "y_api_key");
        assert_eq!(b.secrets[0].label, "y_api_key", "label falls back to name");
        assert_eq!(b.options.len(), 1, "option must not be dropped");
        assert_eq!(b.options[0].label, "base_url", "label falls back to name");
        assert_eq!(b.options[0].r#type, "string", "type defaults to string");
    }

    /// A backend with no `license` field still deserializes (lenient read),
    /// resolving the old producer-required vs consumer-defaulted drift.
    #[test]
    fn backend_without_license_deserializes() {
        let json = r#"{
            "id": "b", "source": "github.com/x/y", "version": "1.0.0", "tag": "v1",
            "name": "B", "kind": "wasm", "contract": "v1", "entrypoint": "b.wasm",
            "online": false, "supports_gpu": false, "supports_cpu": true,
            "models": [], "secrets": [], "options": [], "assets": {}
        }"#;
        let b: IndexBackend = serde_json::from_str(json).expect("missing license is tolerated");
        assert_eq!(b.license, "");
    }
}
