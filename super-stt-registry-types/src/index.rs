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
    /// The backend's reverse-DNS identifier from its release manifest. Names
    /// the install directory. `None` for an entry that predates the field;
    /// `id` remains the registry key and is unchanged, so a client that does
    /// not read this field installs exactly where it always did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_id: Option<String>,
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
    /// The contract generation the release manifest declares. A loose string
    /// here, so a daemon still lists an entry whose generation it does not
    /// know — and can then say so, rather than failing the whole catalog.
    pub contract: String,
    /// The Super STT release that first understood `contract`, from
    /// [`Contract::min_client`](crate::manifest::Contract::min_client). Stamped
    /// by the indexer so a daemon that predates the generation can still name
    /// the version to update to. `None` on an index published before the
    /// field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_client: Option<String>,
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

/// Accept a bare string where a list is expected.
///
/// The index's leaf types stay loose `String`s on purpose — they must tolerate
/// a carried-forward `index.json` — and that tolerance extends to the shape of
/// this field, not just its values.
///
/// # Errors
/// Returns the deserializer's error if the value is neither a string nor an
/// array of strings.
pub fn one_or_many_string<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

/// Write a one-element list as a bare string, and a longer one as an array.
///
/// The counterpart to [`one_or_many_string`], and the reason the pair exists:
/// `index.json` is a single shared artifact, rebuilt from `main` and served to
/// every installed daemon at once. A client that declares this field as a
/// plain `String` rejects the *whole* document when it turns into an array,
/// which no version floor can soften — the parse fails before the floor is
/// read. Emitting the scalar for the one-element case keeps the published
/// bytes readable by those clients, and an array is written only where there
/// is genuinely more than one value to carry.
///
/// # Errors
/// Returns the serializer's error.
pub fn one_or_many_string_ser<S>(v: &[String], s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match v {
        [only] => s.serialize_str(only),
        many => s.collect_seq(many),
    }
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
/// marks online/remote models, `gpu` marks GPU, `cpu` marks CPU. Devices are
/// typed and validated by `Manifest::parse`, so there is no string matching to
/// get wrong.
fn model_support(models: &[ModelEntry]) -> ModelSupport {
    let any_device =
        |pred: fn(&Device) -> bool| models.iter().any(|m| m.supported_devices.iter().any(pred));
    ModelSupport {
        online: models.iter().any(ModelEntry::is_online),
        supports_gpu: any_device(|d| matches!(d, Device::Gpu)),
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
            backend_id: m.backend.id,
            source: m.backend.source,
            version,
            tag,
            name: m.backend.name,
            description: Some(m.backend.description),
            license: m.backend.license.unwrap_or_default(),
            kind: m.backend.kind.to_string(),
            contract: m.backend.contract.to_string(),
            min_client: Some(m.backend.contract.min_client().to_string()),
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
                    provider: String::new(),
                    supported_devices: md
                        .supported_devices
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    role: md.role.to_string(),
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
                .map(|o| {
                    // A `base_url` value authorizes egress, so only the user may
                    // supply one (see [`BASE_URL_OPTION`]). Dropping it here is
                    // what keeps every install path agreeing with the daemon:
                    // this synthesis is shared by the registry indexer, the
                    // custom-repo resolver, and the local-dir import, and the
                    // daemon drops the same value at discovery. Advertising it
                    // would show the user a default their install then loses.
                    let default = (o.name != crate::manifest::BASE_URL_OPTION)
                        .then_some(o.default)
                        .flatten()
                        // Untagged serialize yields the plain JSON value
                        // (string/number/bool). `.ok()` drops the theoretically
                        // impossible failure rather than panicking in a library.
                        .and_then(|d| serde_json::to_value(d).ok());
                    IndexOption {
                        label: o.label.unwrap_or_else(|| o.name.clone()),
                        name: o.name,
                        r#type: o
                            .r#type
                            .map_or_else(|| "string".to_string(), |t| t.to_string()),
                        default,
                    }
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
    /// Wire-compatibility shim, always empty. Daemons through v0.2.0
    /// deserialize the index with this key **required**, so publishing an
    /// index without it makes every installed one of them fail to parse the
    /// whole document. `min_client` cannot gate that: serde rejects the
    /// missing field before the floor is ever read.
    ///
    /// Nothing reads the value. Delete the field once no supported daemon
    /// requires the key.
    #[serde(default, skip_deserializing)]
    pub provider: String,
    pub supported_devices: Vec<String>,
    /// What the model is for: `"transcription"` (the default) or
    /// `"post_processor"`. Lets Browse show that a backend provides a
    /// post-processor before it is installed. `default` so an index published
    /// before the field existed still parses, reading every model as
    /// transcribing — which is what it was.
    #[serde(default = "default_role")]
    pub role: String,
}

/// The role an index entry without the key is read as. Every model predates
/// the field, so they all transcribe. Spelled via the canonical enum so this
/// cannot drift from [`ModelRole::default`](crate::manifest::ModelRole::default).
fn default_role() -> String {
    crate::manifest::ModelRole::default().to_string()
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
    /// Acceleration backends the build carries. A single entry is both read
    /// and written as a bare string, a list of two or more as an array — see
    /// [`one_or_many_string_ser`] for why the published bytes keep the scalar
    /// shape.
    #[serde(
        deserialize_with = "one_or_many_string",
        serialize_with = "one_or_many_string_ser"
    )]
    pub accel: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_major: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_sm: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cudnn: bool,
    /// AMD architecture targets, in `--offload-arch` spelling. Non-empty when
    /// `accel` contains `rocm`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gfx: Vec<String>,
    /// Minimum Vulkan API version as `major.minor`, when the build declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulkan_api: Option<String>,
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
    use crate::manifest::{ModelEntry, ModelRole};

    /// The published `index.json` must keep carrying `provider` on every model.
    /// Daemons through v0.2.0 declare it as a required `String`, so an index
    /// without it fails to deserialize *in full* on every installed one of
    /// them — the registry stops resolving, and `min_client` cannot soften it
    /// because the missing field is rejected before the floor is read.
    ///
    /// This is the test that fails if the compatibility shim is deleted before
    /// those daemons have rolled over.
    #[test]
    fn the_published_index_still_carries_the_provider_key() {
        let m = IndexModel {
            name: "m1".into(),
            provider: String::new(),
            supported_devices: vec!["cpu".into()],
            role: ModelRole::Transcription.to_string(),
        };
        let v = serde_json::to_value(&m).expect("serializes");
        assert!(
            v.get("provider").is_some(),
            "index.json dropped `provider`; daemons <= v0.2.0 cannot parse this: {v}"
        );
    }

    /// The shim is write-only: an index carrying `provider` parses (it is
    /// ignored), and so does one without it, so this crate can keep reading
    /// indexes published either side of the change.
    #[test]
    fn an_index_model_parses_with_or_without_the_provider_key() {
        let with: IndexModel = serde_json::from_str(
            r#"{"name":"m1","provider":"local_whisper","supported_devices":["cpu"]}"#,
        )
        .expect("an index carrying `provider` must parse");
        assert_eq!(with.name, "m1");

        let without: IndexModel =
            serde_json::from_str(r#"{"name":"m1","supported_devices":["cpu"]}"#)
                .expect("an index without `provider` must parse");
        assert_eq!(without.name, "m1");
    }

    /// An index published before `role` existed still parses, and its models
    /// read as transcription models — which is what they were. A required
    /// field here would stop the registry resolving against any such index.
    #[test]
    fn an_index_model_without_a_role_reads_as_transcription() {
        let without: IndexModel =
            serde_json::from_str(r#"{"name":"m1","supported_devices":["cpu"]}"#)
                .expect("an index without `role` must parse");
        assert_eq!(without.role, "transcription");

        let with: IndexModel = serde_json::from_str(
            r#"{"name":"m1","supported_devices":["cpu"],"role":"post_processor"}"#,
        )
        .expect("an index carrying `role` must parse");
        assert_eq!(with.role, "post_processor");
    }

    /// Build a `ModelEntry` with the given devices; the other
    /// fields are irrelevant to `model_support` (which reads only devices).
    fn model(devices: &[Device]) -> ModelEntry {
        ModelEntry {
            name: "m".into(),
            multilingual: true,
            primary_language: "en".into(),
            supported_languages: vec!["en".into()],
            supported_devices: devices.to_vec(),
            estimated_vram_bytes: 0,
            processing_interval_ms: None,
            realtime: false,
            role: ModelRole::Transcription,
            files: vec![],
            provider: None,
        }
    }

    /// Online-ness comes from the `none` device sentinel alone. `provider` is
    /// a free-form legacy string that survives only to be echoed back on load
    /// (see [`ModelEntry::provider`]), so a cloud-sounding value on a local
    /// model — or none at all on a cloud one — must not move the answer.
    #[test]
    fn classify_marks_online_from_none_device_not_provider() {
        let with_provider = |devices: &[Device], provider: &str| {
            let mut m = model(devices);
            m.provider = Some(provider.to_string());
            m
        };

        assert!(model_support(&[model(&[Device::None])]).online);
        assert!(
            model_support(&[with_provider(&[Device::None], "")]).online,
            "a cloud model with no provider is still online"
        );
        assert!(!model_support(&[model(&[Device::Cpu])]).online);
        assert!(
            !model_support(&[with_provider(&[Device::Cpu], "openai")]).online,
            "a cloud-sounding provider must not make a local model online"
        );
        assert!(!model_support(&[model(&[])]).online);
    }

    #[test]
    fn classify_marks_gpu_for_gpu_device() {
        assert!(model_support(&[model(&[Device::Gpu])]).supports_gpu);
        assert!(!model_support(&[model(&[Device::Cpu])]).supports_gpu);
    }

    #[test]
    fn classify_marks_cpu_only_for_cpu_device() {
        assert!(model_support(&[model(&[Device::Cpu])]).supports_cpu);
        assert!(!model_support(&[model(&[Device::None])]).supports_cpu);
        assert!(!model_support(&[model(&[Device::Gpu])]).supports_cpu);
    }

    #[test]
    fn roundtrips_a_minimal_index() {
        let idx = Index {
            schema_version: SCHEMA_VERSION,
            generated_at: "2026-05-29T18:00:00Z".into(),
            min_client: MIN_CLIENT.into(),
            backends: vec![IndexBackend {
                id: "openai".into(),
                backend_id: None,
                source: "github.com/x/y".into(),
                version: "1.0.0".into(),
                tag: "v1.0.0".into(),
                name: "OpenAI".into(),
                description: None,
                license: "Apache-2.0".into(),
                kind: "wasm".into(),
                contract: "v1".into(),
                min_client: None,
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

    /// `backend_id` carries the manifest's reverse-DNS `[backend].id`,
    /// distinct from the caller-supplied registry `id`. An older daemon that
    /// does not read this field still installs into the directory named by
    /// `id`, so the two must never be conflated.
    #[test]
    fn from_manifest_propagates_the_manifest_id_into_backend_id() {
        let m = crate::manifest::Manifest::parse(
            r#"
            [backend]
            id = "app.super-stt.voxtral"
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
            description = "Test backend."

            [assets]
            wasm = "y.wasm"
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

        assert_eq!(b.id, "y", "id stays the registry key, unaffected");
        assert_eq!(b.backend_id.as_deref(), Some("app.super-stt.voxtral"));
    }

    /// The entry carries both the contract the manifest declares and the
    /// release floor that contract implies, so a daemon that does not know the
    /// generation can still name the version to update to.
    #[test]
    fn from_manifest_stamps_the_contract_and_its_client_floor() {
        for contract in crate::manifest::Contract::ALL {
            let m = crate::manifest::Manifest::parse(&format!(
                r#"
                [backend]
                source = "github.com/x/y"
                name = "Y"
                version = "1.0.0"
                kind = "wasm"
                entrypoint = "y.wasm"
                contract = "{contract}"
                description = "Test backend."

                [assets]
                wasm = "y.wasm"
                "#
            ))
            .unwrap();
            let b = IndexBackend::from_manifest(
                "y".into(),
                m,
                "1.0.0".into(),
                "v1.0.0".into(),
                IndexAssets::default(),
                None,
            );
            assert_eq!(b.contract, contract.to_string());
            assert_eq!(b.min_client.as_deref(), Some(contract.min_client()));
        }
    }

    /// An index published before per-entry `min_client` existed still parses,
    /// with the field absent rather than the whole catalog failing.
    #[test]
    fn an_index_entry_without_a_min_client_still_parses() {
        let json = serde_json::json!({
            "id": "y", "source": "github.com/x/y", "version": "1.0.0", "tag": "v1.0.0",
            "name": "Y", "kind": "wasm", "contract": "v1", "entrypoint": "y.wasm",
            "online": false, "supports_gpu": false, "supports_cpu": true,
            "models": [], "secrets": [], "options": [], "assets": {}
        });
        let b: IndexBackend = serde_json::from_value(json).expect("older entry parses");
        assert_eq!(b.min_client, None);
    }

    /// A manifest that predates `[backend].id` yields a `None` `backend_id`,
    /// not an empty string or a fallback to `source`.
    #[test]
    fn from_manifest_leaves_backend_id_none_when_the_manifest_has_none() {
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

        assert!(b.backend_id.is_none());
    }

    /// Every install path — registry, custom repo, local dir — synthesizes its
    /// catalog entry here, and the daemon drops a manifest-declared `base_url`
    /// value at discovery. Carrying one into the catalog would advertise a
    /// default that disappears the moment the backend is installed, and would
    /// leave the Configure sheet offering to "reset" to a value nothing holds.
    /// Every other option keeps its default.
    #[test]
    fn from_manifest_drops_a_base_url_default_and_keeps_the_others() {
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

            [[options]]
            name = "base_url"
            description = "Override."
            default = "https://api.y.example"

            [[options]]
            name = "region"
            description = "Region."
            default = "us-east-1"
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

        assert_eq!(b.options[0].name, "base_url");
        assert!(
            b.options[0].default.is_none(),
            "a manifest-declared base_url value must not reach the catalog"
        );
        assert_eq!(b.options[1].name, "region");
        assert_eq!(
            b.options[1].default,
            Some(serde_json::Value::String("us-east-1".into()))
        );
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

    /// Indexes published before the list form carry `accel` as a bare string.
    /// The daemon reads whatever is on the registry today, so both must parse.
    #[test]
    fn an_index_asset_parses_a_scalar_or_list_accel() {
        let scalar: IndexSubprocessAsset = serde_json::from_str(
            r#"{"target":"x86_64-unknown-linux-gnu","accel":"cuda","cuda_major":12}"#,
        )
        .expect("a scalar accel must parse");
        assert_eq!(scalar.accel, vec!["cuda".to_string()]);

        let list: IndexSubprocessAsset = serde_json::from_str(
            r#"{"target":"x86_64-unknown-linux-gnu","accel":["cuda","rocm"],
                "cuda_major":12,"gfx":["gfx1030"]}"#,
        )
        .expect("a list accel must parse");
        assert_eq!(list.accel, vec!["cuda".to_string(), "rocm".to_string()]);
        assert_eq!(list.gfx, vec!["gfx1030".to_string()]);
    }

    #[test]
    fn an_index_asset_omits_empty_new_fields() {
        let asset = IndexSubprocessAsset {
            target: "x86_64-unknown-linux-gnu".into(),
            accel: vec!["cpu".into()],
            cuda_major: None,
            cuda_sm: None,
            cudnn: false,
            gfx: Vec::new(),
            vulkan_api: None,
            url: Some("u".into()),
            size: Some(1),
            sha256: Some("s".into()),
            parts: Vec::new(),
        };
        let json = serde_json::to_string(&asset).expect("serializes");
        assert!(
            !json.contains("gfx"),
            "empty gfx must not be emitted: {json}"
        );
        assert!(
            !json.contains("vulkan_api"),
            "absent floor must not be emitted: {json}"
        );
    }

    fn asset(accel: &[&str]) -> IndexSubprocessAsset {
        IndexSubprocessAsset {
            target: "x86_64-unknown-linux-gnu".into(),
            accel: accel.iter().map(|a| (*a).to_string()).collect(),
            cuda_major: None,
            cuda_sm: None,
            cudnn: false,
            gfx: Vec::new(),
            vulkan_api: None,
            url: Some("u".into()),
            size: Some(1),
            sha256: Some("s".into()),
            parts: Vec::new(),
        }
    }

    /// `index.json` is one shared server-side artifact: the cron rebuild
    /// replaces the document every installed daemon reads. Daemons through
    /// v0.2.0 declare `accel` as a required `String` and reject the *whole*
    /// document when it is an array, so a single-accel asset — which is every
    /// asset a backend can publish — must still serialize as a bare string.
    ///
    /// This is the test that fails if the scalar form is dropped before those
    /// daemons have rolled over.
    #[test]
    fn a_single_accel_asset_still_serializes_as_a_bare_string() {
        let json = serde_json::to_string(&asset(&["cuda"])).expect("serializes");
        assert!(
            json.contains(r#""accel":"cuda""#),
            "accel is no longer a bare string; daemons <= v0.2.0 cannot parse this index: {json}"
        );
    }

    /// The scalar form is a compatibility shape, not a lossy one: a build
    /// carrying two runtimes has no bare-string spelling, so it is written as
    /// the array it is.
    #[test]
    fn a_multi_accel_asset_serializes_as_an_array() {
        let json = serde_json::to_string(&asset(&["cuda", "rocm"])).expect("serializes");
        assert!(
            json.contains(r#""accel":["cuda","rocm"]"#),
            "a multi-runtime build must keep its list: {json}"
        );
    }

    /// The published bytes for the single case are parseable by the shape
    /// deployed daemons declare — a plain required `String`, no leniency.
    #[test]
    fn a_single_accel_asset_parses_into_the_deployed_string_shape() {
        #[derive(Deserialize)]
        struct DeployedAsset {
            #[allow(dead_code)]
            target: String,
            accel: String,
        }
        let json = serde_json::to_string(&asset(&["cuda"])).expect("serializes");
        let deployed: DeployedAsset =
            serde_json::from_str(&json).expect("a deployed daemon must still parse this");
        assert_eq!(deployed.accel, "cuda");
    }

    /// Round-trip: whatever shape it was written in, this crate reads it back
    /// as the same list.
    #[test]
    fn an_accel_list_round_trips_through_either_shape() {
        for accel in [vec!["cuda"], vec!["cuda", "rocm"]] {
            let json = serde_json::to_string(&asset(&accel)).expect("serializes");
            let back: IndexSubprocessAsset = serde_json::from_str(&json).expect("round-trips");
            assert_eq!(
                back.accel,
                accel.iter().map(|a| (*a).to_string()).collect::<Vec<_>>()
            );
        }
    }
}
