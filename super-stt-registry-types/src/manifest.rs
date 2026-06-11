// SPDX-License-Identifier: GPL-3.0-only
//! The canonical `backend.toml` manifest. This is the single source of truth
//! for the manifest contract (`docs/protocol/backend/config.md`): the daemon
//! parses it for discovery, the registry indexer parses it for release
//! validation, and the published JSON Schema is generated from these types.
//!
//! Parsing is deliberately lenient where the runtime allows it (unknown
//! fields ignored, `[assets]` optional); consumer-specific policy lives in
//! each consumer's `validate` step.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::provider::Provider;

/// A backend's `backend.toml`: identity, packaging, network policy,
/// secrets/options, and the models it provides.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Manifest {
    /// Backend identity and packaging.
    pub backend: BackendMeta,
    /// Outbound network the backend is permitted to reach.
    #[serde(default)]
    pub network: Network,
    /// Optional feature flags that unlock transport extensions.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Binary artifacts a release publishes. Optional for locally installed
    /// backends; required (per `kind`) for registry publication.
    #[serde(default)]
    pub assets: Assets,
    /// Encrypted credentials the backend needs at runtime (e.g. API keys).
    #[serde(default)]
    pub secrets: Vec<Secret>,
    /// Non-secret configuration the user can set through the settings UI.
    #[serde(default)]
    pub options: Vec<Opt>,
    /// One entry per model the backend provides.
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

/// `[backend]` — identity and packaging.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BackendMeta {
    /// Canonical repository id, e.g. `github.com/<owner>/<repo>`. Becomes the
    /// `source` of every model this backend provides and must be unique
    /// across installed backends. For a monorepo, namespace it under the repo
    /// (e.g. `github.com/<owner>/<repo>/openai`).
    pub source: String,
    /// Human-readable display name.
    pub name: String,
    /// Backend version (semver). Must match the release tag's version when
    /// published through the registry.
    pub version: String,
    /// Selects the transport.
    pub kind: Kind,
    /// Path, relative to the backend directory, to the executable
    /// (`subprocess`) or the `.wasm` component (`wasm`). May be a nested
    /// relative path such as `bin/launcher` for multi-file bundles.
    /// Must not escape the backend directory: no absolute paths, no `..`
    /// components, no backslashes, no embedded NUL.
    pub entrypoint: String,
    /// The backend-protocol contract version implemented.
    pub contract: Contract,
    /// SPDX license id. Optional for local installs, required for registry
    /// publication — the indexer rejects manifests without an allowlisted
    /// license.
    #[serde(default)]
    pub license: Option<String>,
    /// One-line summary shown in the registry/Browse listing.
    #[serde(default)]
    pub description: Option<String>,
}

/// Transport a backend uses: a wasm32 component or a native executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A `wasm32` component loaded in the daemon's WASM host.
    Wasm,
    /// A native executable run in the daemon's subprocess sandbox.
    Subprocess,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wasm => write!(f, "wasm"),
            Self::Subprocess => write!(f, "subprocess"),
        }
    }
}

/// Backend-protocol contract version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Contract {
    /// The v1 contract (`docs/protocol/backend/contract.md`).
    #[serde(rename = "v1")]
    V1,
}

impl fmt::Display for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1 => write!(f, "v1"),
        }
    }
}

/// `[network]` — outbound network policy.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Network {
    /// Host or `host:port` egress allowlist. Empty or absent means no
    /// network. Honored for `wasm` backends; must be empty for `subprocess`
    /// backends (the transport provides no network).
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

/// `[capabilities]` — transport extensions beyond the base `/v1` contract.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Capabilities {
    /// Opt into the realtime WebSocket import/export. wasm-only — a
    /// `subprocess` backend declaring this is rejected at discovery.
    /// Required for any model with `realtime = true`. Default `false`.
    #[serde(default)]
    pub websocket: bool,
}

/// `[assets]` — binary artifacts a release publishes, so the registry indexer
/// and the daemon's installer can find them without guessing.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Assets {
    /// Filename of the wasm component on the GitHub release. Required for
    /// registry publication when `kind = "wasm"`.
    #[serde(default)]
    pub wasm: Option<String>,
    /// One entry per built subprocess variant. Required (non-empty) for
    /// registry publication when `kind = "subprocess"`.
    #[serde(default)]
    pub subprocess: Vec<SubprocessAsset>,
}

/// One `[[assets.subprocess]]` build variant.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SubprocessAsset {
    /// Filename on the GitHub release (`.tar.gz`). The archive must contain
    /// `bin/<entrypoint>`.
    pub file: String,
    /// Rust target triple, e.g. `x86_64-unknown-linux-gnu`. Tier-1/2 only;
    /// the indexer rejects unknown triples.
    pub target: String,
    /// Acceleration backend the build targets.
    pub accel: Accel,
    /// CUDA major version this build targets. Required when
    /// `accel = "cuda"`, forbidden otherwise.
    #[serde(default)]
    pub cuda_major: Option<u32>,
    /// Compute capability (e.g. `75`, `86`, `90`, `120`). Omit to match any
    /// compute capability — use for multi-architecture framework builds
    /// (e.g. a `PyTorch` wheel). An exact-SM asset is preferred over a
    /// wildcard when both match. Forbidden when `accel != "cuda"`.
    #[serde(default)]
    pub cuda_sm: Option<u32>,
    /// Whether this build links cuDNN. Allowed only when `accel = "cuda"`.
    /// Default `false`.
    #[serde(default)]
    pub cudnn: bool,
}

/// Acceleration backend of a subprocess build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Accel {
    Cpu,
    Cuda,
    Metal,
    Rocm,
    Vulkan,
}

impl fmt::Display for Accel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Cuda => write!(f, "cuda"),
            Self::Metal => write!(f, "metal"),
            Self::Rocm => write!(f, "rocm"),
            Self::Vulkan => write!(f, "vulkan"),
        }
    }
}

/// One `[[secrets]]` declaration — an encrypted credential the backend reads
/// as an `x-stt-secret-<name>` request header.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Secret {
    /// `snake_case` identifier the backend reads the value by. Unique within
    /// the table.
    pub name: String,
    /// Human-readable label shown in the settings UI. Falls back to `name`
    /// when absent.
    #[serde(default)]
    pub label: Option<String>,
    /// Help text shown beside the input in the settings UI.
    pub description: String,
    /// Whether a value must be set before the backend can load. Default
    /// `false`.
    #[serde(default)]
    pub required: bool,
}

/// One `[[options]]` declaration — non-secret configuration the backend reads
/// as an `x-stt-option-<name>` request header.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Opt {
    /// `snake_case` identifier the backend reads the value by. Unique within
    /// the table.
    pub name: String,
    /// Human-readable label shown in the settings UI. Falls back to `name`
    /// when absent.
    #[serde(default)]
    pub label: Option<String>,
    /// Help text shown beside the input in the settings UI.
    pub description: String,
    /// Drives the input the UI renders. Default `string`.
    #[serde(default)]
    pub r#type: Option<OptionType>,
    /// Value used when the user sets none. Should match `type`.
    #[serde(default)]
    pub default: Option<OptionDefault>,
    /// Whether a value must be set before the backend can load. Default
    /// `false`.
    #[serde(default)]
    pub required: bool,
}

/// The input type of an option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum OptionType {
    String,
    Integer,
    Bool,
}

impl fmt::Display for OptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String => write!(f, "string"),
            Self::Integer => write!(f, "integer"),
            Self::Bool => write!(f, "bool"),
        }
    }
}

/// An option's default value; matches the option's declared `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum OptionDefault {
    String(String),
    Integer(i64),
    Bool(bool),
}

impl fmt::Display for OptionDefault {
    /// The string form injected via `x-stt-option-*` headers and shown in the
    /// settings catalog: strings pass through unquoted; integers and bools
    /// use their plain display form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(s) => write!(f, "{s}"),
            Self::Integer(i) => write!(f, "{i}"),
            Self::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// One `[[models]]` entry. Each model is identified on the wire by
/// `(name, provider, source)`, where `source` is `[backend].source`.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelEntry {
    /// Wire model name.
    pub name: String,
    /// Engine that serves the model.
    pub provider: Provider,
    /// Whether the model accepts more than one language. Default `true`.
    /// When `false`, `supported_languages` must be exactly
    /// `[primary_language]`.
    #[serde(default = "default_true")]
    pub multilingual: bool,
    /// Default language code (e.g. `en`); used when `language` is omitted.
    /// Must appear in `supported_languages`.
    pub primary_language: String,
    /// Language codes the model accepts; must include `primary_language`.
    pub supported_languages: Vec<String>,
    /// Devices the model can be loaded onto. The sentinel `none` (remote /
    /// online model with no local compute) must be the only entry when
    /// present. Non-empty.
    pub supported_devices: Vec<Device>,
    /// Conservative GPU memory estimate in bytes. Default `0`; use `0` for
    /// cloud models.
    #[serde(default)]
    pub estimated_vram_bytes: u64,
    /// Suggested minimum interval between streaming passes, in milliseconds.
    #[serde(default)]
    pub processing_interval_ms: Option<u64>,
    /// When `true`, the model is driven over the consumer WebSocket endpoint
    /// rather than batch `POST /v1/transcribe`. Requires
    /// `[capabilities] websocket = true`. Default `false`.
    #[serde(default)]
    pub realtime: bool,
    /// Files the model needs, provisioned into `dest` before
    /// `POST /v1/load`. Cloud models declare none.
    #[serde(default)]
    pub files: Vec<FilesSpec>,
}

/// A device a model can be loaded onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Device {
    Cpu,
    Cuda,
    Metal,
    /// Sentinel for remote/online models with no local compute; must be the
    /// only entry when present.
    None,
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Cuda => write!(f, "cuda"),
            Self::Metal => write!(f, "metal"),
            Self::None => write!(f, "none"),
        }
    }
}

impl std::str::FromStr for Device {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cpu" => Ok(Self::Cpu),
            "cuda" => Ok(Self::Cuda),
            "metal" => Ok(Self::Metal),
            "none" => Ok(Self::None),
            _ => Err(format!("Unknown device: {s}")),
        }
    }
}

/// One `[[models.files]]` download group.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FilesSpec {
    /// Where the files come from. Default `huggingface`.
    #[serde(default)]
    pub source: FileSource,
    /// Hugging Face repo id, e.g. `openai/whisper-tiny`. Required when
    /// `source = "huggingface"`.
    #[serde(default)]
    pub repo: String,
    /// Hugging Face revision. Default `main`.
    #[serde(default = "default_revision")]
    pub revision: String,
    /// Filenames to fetch from the repo. Required when
    /// `source = "huggingface"`.
    #[serde(default)]
    pub files: Vec<String>,
    /// Direct download URL for a single file. Required when
    /// `source = "url"`.
    #[serde(default)]
    pub url: Option<String>,
    /// Directory, relative to the backend dir, to place the files in
    /// (e.g. `models/whisper-tiny`).
    pub dest: String,
    /// Expected SHA-256 of the downloaded file, for integrity verification.
    #[serde(default)]
    pub sha256: Option<String>,
}

/// Source of a `[[models.files]]` download group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FileSource {
    /// Fetch `files` from the Hugging Face repo `repo` at `revision`.
    #[default]
    Huggingface,
    /// Fetch a single file from `url`.
    Url,
}

fn default_true() -> bool {
    true
}

fn default_revision() -> String {
    "main".to_string()
}

/// Errors from reading/parsing a `backend.toml`.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// An I/O error reading the file.
    #[error("read {path}")]
    Io {
        /// Path that failed to read.
        path: String,
        /// Underlying I/O error.
        #[source]
        err: std::io::Error,
    },
    /// A parse error, annotated with the file path.
    #[error("parse {path}")]
    Parse {
        /// Path of the file that failed to parse.
        path: String,
        #[source]
        err: Box<ManifestError>,
    },
    /// A TOML parse error.
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    /// The `entrypoint` field is not a safe relative path.
    #[error("backend.toml entrypoint {0:?} is not a safe relative path")]
    UnsafeEntrypoint(String),
}

impl Manifest {
    /// Parse a `backend.toml` from its text.
    ///
    /// The entrypoint is joined onto the backend dir to spawn/load the
    /// backend; an absolute or traversing value would escape it. The guard
    /// lives in the single canonical parser so every consumer inherits it.
    ///
    /// # Errors
    /// Returns a [`ManifestError`] on TOML errors or an unsafe entrypoint.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        let m: Self = toml::from_str(text)?;
        if !crate::is_safe_relative_path(&m.backend.entrypoint) {
            return Err(ManifestError::UnsafeEntrypoint(m.backend.entrypoint));
        }
        Ok(m)
    }

    /// Read and parse `<dir>/backend.toml`.
    ///
    /// # Errors
    /// Returns a [`ManifestError`] if the file is missing, unreadable, or
    /// fails [`Manifest::parse`].
    pub fn load(dir: &Path) -> Result<Self, ManifestError> {
        let path = dir.join("backend.toml");
        let text = std::fs::read_to_string(&path).map_err(|err| ManifestError::Io {
            path: path.display().to_string(),
            err,
        })?;
        Self::parse(&text).map_err(|err| ManifestError::Parse {
            path: path.display().to_string(),
            err: Box::new(err),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every in-repo backend manifest must parse with the canonical types.
    #[test]
    fn parses_all_in_repo_manifests() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let mut count = 0;
        for entry in std::fs::read_dir(root.join("backends")).unwrap().flatten() {
            let dir = entry.path();
            if !dir.join("backend.toml").exists() {
                continue;
            }
            let m = Manifest::load(&dir)
                .unwrap_or_else(|e| panic!("{} must parse: {e}", dir.display()));
            assert!(!m.backend.source.is_empty());
            count += 1;
        }
        // Tripwire: bump when adding/removing in-repo backends.
        assert!(count >= 5, "expected the 5 in-repo backends, found {count}");
    }

    #[test]
    fn parses_wasm_manifest_with_secrets_and_options() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"

            [assets]
            wasm = "y.wasm"

            [[secrets]]
            name = "y_api_key"
            description = "Key."

            [[options]]
            name = "base_url"
            description = "Override."
            type = "string"
            default = "https://api.y.com"

            [[options]]
            name = "timeout"
            description = "Seconds."
            type = "integer"
            default = 30
            "#,
        )
        .unwrap();
        assert_eq!(m.backend.kind, Kind::Wasm);
        assert_eq!(m.backend.contract, Contract::V1);
        assert!(m.secrets[0].label.is_none());
        assert!(!m.secrets[0].required);
        assert_eq!(m.options[0].r#type, Some(OptionType::String));
        assert_eq!(
            m.options[0].default,
            Some(OptionDefault::String("https://api.y.com".into()))
        );
        assert_eq!(m.options[1].default, Some(OptionDefault::Integer(30)));
        assert_eq!(m.options[1].default.as_ref().unwrap().to_string(), "30");
    }

    #[test]
    fn rejects_secret_without_description() {
        let err = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"

            [[secrets]]
            name = "y_api_key"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("description"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_kind_at_parse() {
        let err = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "container"
            entrypoint = "y.wasm"
            contract = "v1"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown variant"), "got: {err}");
    }

    #[test]
    fn rejects_unknown_provider_at_parse() {
        let err = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"

            [[models]]
            name = "m"
            provider = "anthropic"
            primary_language = "en"
            supported_languages = ["en"]
            supported_devices = ["none"]
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("anthropic"), "got: {err}");
    }

    #[test]
    fn rejects_unsafe_entrypoint() {
        // "a/b" is a *valid* relative path ("bin/launcher" style) — only
        // absolute and traversing values are rejected.
        for bad in ["../escape", "/usr/bin/python3", ".."] {
            let text = format!(
                r#"
                [backend]
                source = "github.com/x/y"
                name = "Y"
                version = "1.0.0"
                kind = "subprocess"
                entrypoint = "{bad}"
                contract = "v1"
                "#
            );
            let err = Manifest::parse(&text).unwrap_err();
            assert!(
                matches!(err, ManifestError::UnsafeEntrypoint(_)),
                "entrypoint {bad:?} should be rejected, got {err}"
            );
        }
    }

    #[test]
    fn files_spec_defaults_and_url_fields() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"

            [[models]]
            name = "m"
            provider = "local_whisper"
            primary_language = "en"
            supported_languages = ["en"]
            supported_devices = ["cpu"]

            [[models.files]]
            repo = "openai/whisper-tiny"
            files = ["model.safetensors"]
            dest = "models/m"

            [[models.files]]
            source = "url"
            url = "https://example.com/extra.bin"
            sha256 = "ab"
            dest = "models/m"
            "#,
        )
        .unwrap();
        let f = &m.models[0].files[0];
        assert_eq!(f.source, FileSource::Huggingface);
        assert_eq!(f.revision, "main");
        let g = &m.models[0].files[1];
        assert_eq!(g.source, FileSource::Url);
        assert_eq!(g.url.as_deref(), Some("https://example.com/extra.bin"));
    }

    #[test]
    fn load_errors_carry_the_file_path() {
        let dir = std::env::temp_dir().join("sstt-manifest-err-test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("backend.toml"), "not [ valid toml").unwrap();
        let err = Manifest::load(&dir).unwrap_err();
        let chain = format!(
            "{err}: {}",
            std::error::Error::source(&err)
                .map(ToString::to_string)
                .unwrap_or_default()
        );
        assert!(chain.contains("backend.toml"), "got: {chain}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Untagged `OptionDefault` must bind TOML primitives by their actual type —
    /// these pins guard against serde/toml upgrades changing untagged behavior.
    #[test]
    fn option_default_binds_by_toml_type() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"

            [[options]]
            name = "a"
            description = "A."
            default = true

            [[options]]
            name = "b"
            description = "B."
            default = "30"
            "#,
        )
        .unwrap();
        assert_eq!(m.options[0].default, Some(OptionDefault::Bool(true)));
        assert_eq!(
            m.options[1].default,
            Some(OptionDefault::String("30".into()))
        );
    }

    /// Unknown fields and tables are ignored — older daemons must tolerate
    /// manifests written for newer contract revisions.
    #[test]
    fn unknown_fields_and_tables_are_ignored() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
            future_field = "ignored"

            [future_table]
            x = 1
            "#,
        )
        .unwrap();
        assert_eq!(m.backend.name, "Y");
    }

    #[test]
    fn cuda_sm_is_optional_wildcard() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"

            [[assets.subprocess]]
            file = "y-cuda13.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 13
            "#,
        )
        .unwrap();
        let a = &m.assets.subprocess[0];
        assert_eq!(a.accel, Accel::Cuda);
        assert_eq!(a.cuda_major, Some(13));
        assert_eq!(a.cuda_sm, None);
        assert!(!a.cudnn);
    }

    #[test]
    fn device_from_str_round_trips_canonical_forms() {
        for device in [Device::Cpu, Device::Cuda, Device::Metal, Device::None] {
            let s = device.to_string();
            let parsed: Device = s.parse().unwrap();
            assert_eq!(device, parsed, "round-trip failed for {s}");
        }
    }

    #[test]
    fn device_from_str_rejects_non_canonical_strings() {
        // `rocm` is an `Accel` build axis, never a model `Device`; non-snake_case
        // and unknown strings must error so callers don't accept stale forms.
        for bad in ["rocm", "Cpu", "CUDA", "gpu", "metal_gpu", ""] {
            assert!(
                bad.parse::<Device>().is_err(),
                "{bad:?} should fail to parse as a Device"
            );
        }
    }
}
