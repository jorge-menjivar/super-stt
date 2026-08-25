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

/// The option name that carries a backend's configurable endpoint.
///
/// It is the one option whose *value* changes what the daemon permits: the host
/// it names is authorized for egress with the SSRF guard relaxed. That is sound
/// only while the value is the user's, so consumers treat a manifest-supplied
/// one as no value — the indexer refuses such a release, the catalog synthesis
/// in [`IndexBackend::from_manifest`](crate::index::IndexBackend::from_manifest)
/// drops it, and the daemon drops it at discovery. Named here so those checks
/// cannot drift apart over a string literal.
pub const BASE_URL_OPTION: &str = "base_url";

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
    /// Globally unique reverse-DNS identifier, e.g. `app.super-stt.voxtral`.
    /// Names the directory this backend installs into. Optional on disk so a
    /// backend installed before the field existed keeps loading; required for
    /// registry listing, which the indexer enforces.
    #[serde(default)]
    pub id: Option<String>,
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
    /// License of the backend: a current SPDX identifier that is OSI-approved
    /// or FSF Free/Libre (e.g. `Apache-2.0`, `MIT`, `GPL-3.0-only`), or the
    /// literal `other` for a license outside that set. Optional for local
    /// installs; required for registry publication, where the indexer rejects
    /// a manifest that omits the field or declares an unrecognized value.
    #[serde(default)]
    pub license: Option<String>,
    /// One-line, human-readable summary shown in the registry/Browse listing.
    /// Required for every backend.
    pub description: String,
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
///
/// The variant's `.tar.gz` is named by `file`, or — when it would exceed the
/// 2 GiB GitHub release-asset limit — by `parts`, whose byte-for-byte
/// concatenation in order is the archive. Exactly one of the two is set
/// (enforced by [`Manifest::parse`]).
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SubprocessAsset {
    /// Single-file archive: the filename on the GitHub release (`.tar.gz`).
    /// Mutually exclusive with `parts`. The archive must contain
    /// `bin/<entrypoint>`.
    #[serde(default)]
    pub file: Option<String>,
    /// Multi-part archive: ordered release filenames whose byte-for-byte
    /// concatenation is the `.tar.gz`. Mutually exclusive with `file`; use when
    /// the archive exceeds the 2 GiB release-asset limit. The indexer pins each
    /// part independently.
    #[serde(default)]
    pub parts: Vec<String>,
    /// Rust target triple, e.g. `x86_64-unknown-linux-gnu`. Tier-1/2 only;
    /// the indexer rejects unknown triples.
    pub target: String,
    /// Acceleration backends this build carries. A single string is accepted
    /// and read as a one-element list, which is what every published manifest
    /// uses; an array declares a binary carrying several runtimes, and the
    /// daemon tells it at load time which one to use. Must be non-empty.
    #[serde(deserialize_with = "one_or_many_accel")]
    pub accel: Vec<Accel>,
    /// CUDA major version this build targets. Required when `accel` contains
    /// `cuda`, forbidden otherwise.
    #[serde(default)]
    pub cuda_major: Option<u32>,
    /// Compute capability (e.g. `75`, `86`, `90`, `120`). Omit to match any
    /// compute capability — use for multi-architecture framework builds
    /// (e.g. a `PyTorch` wheel). An exact-SM asset is preferred over a
    /// wildcard when both match. Forbidden when `accel` lacks `cuda`.
    #[serde(default)]
    pub cuda_sm: Option<u32>,
    /// Whether this build links cuDNN. Allowed only when `accel` contains
    /// `cuda`. Default `false`.
    #[serde(default)]
    pub cudnn: bool,
    /// AMD architecture targets this build carries, in `--offload-arch`
    /// spelling. Required when `accel` contains `rocm`, forbidden otherwise.
    ///
    /// There is no wildcard, deliberately breaking symmetry with `cuda_sm`:
    /// PTX gives CUDA a JIT path that makes "any compute capability" a true
    /// claim, while HIP code objects are architecture-specific AMDGCN ISA with
    /// no equivalent. A wildcard would install a binary that fails at model
    /// load instead of falling back to CPU. Fat builds list every target they
    /// were compiled for.
    #[serde(default)]
    pub gfx: Vec<crate::arch::GfxSpec>,
    /// Minimum Vulkan API version this build requires. Allowed only when
    /// `accel` contains `vulkan`. There is no architecture field: SPIR-V is
    /// portable and driver-compiled.
    #[serde(default)]
    pub vulkan_api: Option<crate::arch::VulkanApi>,
}

/// Accept `accel = "cuda"` as well as `accel = ["cuda", "rocm"]`.
///
/// Every manifest published so far uses the scalar form, and `backend.toml` is
/// a pinned release asset the daemon re-reads on every scan, so the scalar has
/// to keep parsing indefinitely.
fn one_or_many_accel<'de, D>(d: D) -> Result<Vec<Accel>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(Accel),
        Many(Vec<Accel>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(a) => vec![a],
        OneOrMany::Many(v) => v,
    })
}

impl SubprocessAsset {
    /// The release filenames composing this variant's archive: the single
    /// `file`, or the ordered `parts`. Exactly one source is populated once the
    /// manifest has passed [`Manifest::parse`].
    #[must_use]
    pub fn release_files(&self) -> Vec<&str> {
        match &self.file {
            Some(f) => vec![f.as_str()],
            None => self.parts.iter().map(String::as_str).collect(),
        }
    }

    /// Whether the archive is delivered as multiple concatenated parts.
    #[must_use]
    pub fn is_multipart(&self) -> bool {
        self.file.is_none()
    }

    /// A short label for diagnostics (the `file`, else the first part).
    #[must_use]
    pub fn label(&self) -> String {
        self.file
            .clone()
            .or_else(|| self.parts.first().cloned())
            .unwrap_or_else(|| "<unnamed subprocess asset>".into())
    }
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

impl OptionType {
    /// The canonical lowercase string form (e.g. for JSON responses).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Bool => "bool",
        }
    }
}

impl fmt::Display for OptionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
/// `(name, source)`, where `source` is `[backend].source`.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ModelEntry {
    /// Wire model name.
    pub name: String,
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
    /// Files the model needs, each provisioned to its own `destination`
    /// before `POST /v1/load`. Cloud models declare none.
    #[serde(default)]
    pub files: Vec<FileSpec>,
    /// Compatibility shim; not part of model identity, which is
    /// `(name, source)`.
    ///
    /// `provider` used to be the third component of that key, and backends
    /// released against the earlier contract compare it against their own
    /// fixed value on `POST /v1/load` — answering `400 invalid_model` when it
    /// does not match. Dropping the field from the parser would make every
    /// such backend unloadable, so the value is kept solely to be echoed back
    /// on load; the daemon reads no meaning from it (`is_online` comes from
    /// `supported_devices`).
    ///
    /// It also has to stay in the generated schema: every published manifest
    /// declares the key, and a closed `ModelEntry` without it flags all of
    /// them as invalid in an editor bound to the schema.
    ///
    /// Delete the field once no supported backend validates the key.
    #[serde(default)]
    pub provider: Option<String>,
}

impl ModelEntry {
    /// Whether the model is served by a remote API with no local compute —
    /// encoded by the `none` sentinel in `supported_devices` (which validation
    /// requires to be the sole entry when present). This is the single source
    /// of the online/local distinction; the `provider` string is free-form and
    /// carries no such meaning.
    #[must_use]
    pub fn is_online(&self) -> bool {
        self.supported_devices.contains(&Device::None)
    }
}

/// A device a model can be loaded onto.
///
/// Only two local answers exist, because `registry::compat` has already chosen
/// exactly one asset by the time this matters and that asset names its own
/// runtimes: run on the CPU, or run on the accelerator the installed build
/// targets. Which accelerator that is — CUDA, `ROCm`, Metal, Vulkan — is a
/// property of the asset, reported by `Accel`, not a choice made here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Device {
    Cpu,
    Gpu,
    /// Sentinel for remote/online models with no local compute; must be the
    /// only entry when present.
    None,
}

impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::Gpu => write!(f, "gpu"),
            Self::None => write!(f, "none"),
        }
    }
}

impl std::str::FromStr for Device {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cpu" => Ok(Self::Cpu),
            // `cuda` and `metal` are deprecated input spellings. They are
            // accepted because `backend.toml` is a pinned release asset and
            // published `index.json` files carry them, so a manifest written
            // before this vocabulary must keep loading. `Display` never emits
            // them, so nothing new can come to depend on them.
            "gpu" | "cuda" | "metal" => Ok(Self::Gpu),
            "none" => Ok(Self::None),
            _ => Err(format!("Unknown device: {s}")),
        }
    }
}

/// Routed through `FromStr` so the deprecated spellings are accepted wherever
/// a device is deserialized — TOML manifests and JSON index entries alike.
impl<'de> Deserialize<'de> for Device {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// One `[[models.files]]` entry: a single file to download and where to put it.
///
/// Source-agnostic — a file is just a URL, fetched the same way regardless of
/// host. Hugging Face is reached by writing its plain resolve URL, with no
/// special treatment.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FileSpec {
    /// Full download URL for this file, e.g.
    /// `https://huggingface.co/openai/whisper-tiny/resolve/main/config.json`.
    pub url: String,
    /// Relative file path (including filename) under the backend directory to
    /// write the download to, e.g. `models/whisper-tiny/config.json`.
    /// Validated as a safe relative path so it cannot escape the backend dir.
    pub destination: String,
    /// Expected SHA-256 of the file, hex-encoded, for integrity verification.
    #[serde(default)]
    pub sha256: Option<String>,
}

fn default_true() -> bool {
    true
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
    /// `[backend].id` is present but not a well-formed reverse-DNS id.
    #[error("`[backend].id` is not a valid reverse-DNS id: {0}")]
    InvalidId(String),
    /// A `[[models.files]]` `destination` is not a safe relative path.
    #[error("backend.toml file destination {0:?} is not a safe relative path")]
    UnsafeDestination(String),
    /// A `[[assets.subprocess]]` entry set neither or both of `file`/`parts`.
    #[error(
        "backend.toml subprocess asset for target {0:?} must set exactly one of \
         `file` or `parts`"
    )]
    AssetFileXorParts(String),
    /// A `[[assets.subprocess]]` entry declared an empty `accel` list.
    #[error("asset `{file}` declares an empty `accel` list")]
    AccelEmpty {
        /// The asset's label (its `file`, or its first `parts` entry).
        file: String,
    },
    /// A `[[assets.subprocess]]` entry declared `accel = "rocm"` with no `gfx`.
    #[error("asset `{file}` declares `accel = rocm` but no `gfx` targets")]
    RocmMissingGfx {
        /// The asset's label (its `file`, or its first `parts` entry).
        file: String,
    },
    /// A `[[assets.subprocess]]` entry declared `gfx` without `rocm` in `accel`.
    #[error("asset `{file}` declares `gfx` without `accel = rocm`")]
    GfxRequiresRocm {
        /// The asset's label (its `file`, or its first `parts` entry).
        file: String,
    },
    /// A `[[assets.subprocess]]` entry declared `vulkan_api` without `vulkan`
    /// in `accel`.
    #[error("asset `{file}` declares `vulkan_api` without `accel = vulkan`")]
    VulkanApiRequiresVulkan {
        /// The asset's label (its `file`, or its first `parts` entry).
        file: String,
    },
    /// A `[[assets.subprocess]]` entry declared `accel` containing `cuda` but
    /// no `cuda_major`.
    #[error("asset `{file}` declares `accel` containing `cuda` but no `cuda_major`")]
    CudaMissingMajor {
        /// The asset's label (its `file`, or its first `parts` entry).
        file: String,
    },
    /// A `[[assets.subprocess]]` entry declared `cuda_major`/`cuda_sm` without
    /// `cuda` in `accel`.
    #[error("asset `{file}` declares `cuda_major`/`cuda_sm` without `accel` containing `cuda`")]
    CudaForbiddenFields {
        /// The asset's label (its `file`, or its first `parts` entry).
        file: String,
    },
    /// A `[[assets.subprocess]]` entry declared `cudnn = true` without `cuda`
    /// in `accel`.
    #[error("asset `{file}` declares `cudnn = true` without `accel` containing `cuda`")]
    CudnnRequiresCuda {
        /// The asset's label (its `file`, or its first `parts` entry).
        file: String,
    },
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
        let mut m: Self = toml::from_str(text)?;
        if !crate::is_safe_relative_path(&m.backend.entrypoint) {
            return Err(ManifestError::UnsafeEntrypoint(m.backend.entrypoint));
        }
        // Validated in the canonical parser so the daemon (which joins it onto
        // the backends dir) and the indexer (which pins it against
        // registry.toml) inherit one definition.
        if let Some(id) = &m.backend.id
            && !crate::backend_id::is_valid(id)
        {
            return Err(ManifestError::InvalidId(id.clone()));
        }
        // Each file's `destination` is joined onto the backend dir before the
        // daemon writes the download; reject any value that would escape it.
        // The guard lives in the canonical parser so every consumer inherits it.
        for model in &m.models {
            for file in &model.files {
                if !crate::is_safe_relative_path(&file.destination) {
                    return Err(ManifestError::UnsafeDestination(file.destination.clone()));
                }
            }
        }
        // A subprocess build variant names its archive with exactly one of
        // `file` (single) or `parts` (split across release assets, concatenated
        // in order). The guard lives in the canonical parser so the daemon and
        // the indexer agree on the contract.
        for a in &mut m.assets.subprocess {
            // Normalize an empty `file` to `None` so the XOR check and the
            // downstream `release_files()` / `is_multipart()` all agree that
            // `parts` is the source. Without this, `file = ""` plus valid `parts`
            // passed parse but `release_files()` then returned `[""]` and
            // `is_multipart()` was false (Tier 1 #25).
            if a.file.as_deref().is_some_and(str::is_empty) {
                a.file = None;
            }
            let has_file = a.file.is_some();
            let has_parts = !a.parts.is_empty() && a.parts.iter().all(|p| !p.is_empty());
            if has_file == has_parts {
                return Err(ManifestError::AssetFileXorParts(a.target.clone()));
            }
            if a.accel.is_empty() {
                return Err(ManifestError::AccelEmpty { file: a.label() });
            }
            let has = |k: Accel| a.accel.contains(&k);
            if has(Accel::Cuda) {
                // `cuda_sm` stays optional: omitted means the build matches any
                // compute capability (multi-architecture framework builds).
                if a.cuda_major.is_none() {
                    return Err(ManifestError::CudaMissingMajor { file: a.label() });
                }
            } else {
                if a.cuda_major.is_some() || a.cuda_sm.is_some() {
                    return Err(ManifestError::CudaForbiddenFields { file: a.label() });
                }
                if a.cudnn {
                    return Err(ManifestError::CudnnRequiresCuda { file: a.label() });
                }
            }
            if has(Accel::Rocm) {
                if a.gfx.is_empty() {
                    return Err(ManifestError::RocmMissingGfx { file: a.label() });
                }
            } else if !a.gfx.is_empty() {
                return Err(ManifestError::GfxRequiresRocm { file: a.label() });
            }
            if !has(Accel::Vulkan) && a.vulkan_api.is_some() {
                return Err(ManifestError::VulkanApiRequiresVulkan { file: a.label() });
            }
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

    /// A minimal manifest with no `[backend].id`, for tests that only care
    /// about behavior around the field.
    const VALID: &str = r#"
        [backend]
        source = "github.com/x/y"
        name = "Y"
        version = "1.0.0"
        kind = "wasm"
        entrypoint = "y.wasm"
        contract = "v1"
        description = "Test backend."
        "#;

    #[test]
    fn parses_a_manifest_declaring_a_backend_id() {
        let t = VALID.replace("[backend]", "[backend]\n    id = \"app.super-stt.voxtral\"");
        let m = Manifest::parse(&t).expect("a manifest with a valid id parses");
        assert_eq!(m.backend.id.as_deref(), Some("app.super-stt.voxtral"));
    }

    #[test]
    fn a_manifest_without_an_id_still_parses() {
        let m = Manifest::parse(VALID).expect("id is optional on disk");
        assert!(m.backend.id.is_none());
    }

    #[test]
    fn rejects_a_malformed_backend_id() {
        let t = VALID.replace("[backend]", "[backend]\n    id = \"voxtral\"");
        let err = Manifest::parse(&t).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidId(_)));
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
            description = "Test backend."

            [assets]
            wasm = "y.wasm"

            [[secrets]]
            name = "y_api_key"
            description = "Key."

            [[options]]
            name = "region"
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

    /// `base_url` names the endpoint whose host is authorized for egress with
    /// the SSRF guard relaxed, so a value for it must come from the user. The
    /// format stays lenient about that — the parser keeps whatever the manifest
    /// wrote, and the consumers enforce the rule: the indexer refuses to publish
    /// such a release, and the daemon drops the value and loads the backend
    /// anyway (`super-stt-indexer::manifest::validate`,
    /// `super_stt_daemon::stt_models::backends`).
    #[test]
    fn parse_keeps_a_base_url_default_for_consumers_to_judge() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
            description = "Test backend."

            [[options]]
            name = "base_url"
            description = "Endpoint."
            type = "string"
            default = "https://api.y.com"
            "#,
        )
        .expect("parse stays lenient; policy lives in each consumer");
        assert_eq!(m.options[0].name, "base_url");
        assert_eq!(
            m.options[0].default,
            Some(OptionDefault::String("https://api.y.com".into()))
        );
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
            description = "Test backend."

            [[secrets]]
            name = "y_api_key"
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("description"), "got: {err}");
    }

    #[test]
    fn rejects_backend_without_description() {
        let err = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "v1"
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
            description = "Test backend."
            "#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown variant"), "got: {err}");
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
                description = "Test backend."
                "#
            );
            let err = Manifest::parse(&text).unwrap_err();
            assert!(
                matches!(err, ManifestError::UnsafeEntrypoint(_)),
                "entrypoint {bad:?} should be rejected, got {err}"
            );
        }
    }

    /// A `[[models]]` table may carry keys this crate does not read, and
    /// `provider` is the one published backends actually ship. The parser must
    /// keep ignoring it: a manifest is fetched from a backend's release at
    /// index time, so rejecting an unread key would drop every already-released
    /// backend out of the index rather than fail some local build.
    ///
    /// Concretely, this is the test that fails if `deny_unknown_fields` is ever
    /// added to `ModelEntry`.
    #[test]
    fn a_model_carrying_an_unread_provider_key_still_parses() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[models]]
            name = "m1"
            provider = "local_whisper"
            primary_language = "en"
            supported_languages = ["en"]
            supported_devices = ["cpu"]
            "#,
        )
        .expect("a manifest declaring `provider` must still parse");
        assert_eq!(m.models.len(), 1);
        assert_eq!(m.models[0].name, "m1");
        assert_eq!(m.models[0].supported_devices, vec![Device::Cpu]);
    }

    #[test]
    fn file_spec_parses_inline_and_block_forms() {
        // The inline-table array and the `[[models.files]]` block form are the
        // same TOML structure; exercise both (on separate models — TOML forbids
        // mixing the two for one key).
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[models]]
            name = "m1"
            primary_language = "en"
            supported_languages = ["en"]
            supported_devices = ["cpu"]
            files = [
                { url = "https://example.com/config.json", destination = "models/m1/config.json" },
            ]

            [[models]]
            name = "m2"
            primary_language = "en"
            supported_languages = ["en"]
            supported_devices = ["cpu"]

            [[models.files]]
            url = "https://huggingface.co/openai/whisper-tiny/resolve/main/model.safetensors"
            destination = "models/m2/model.safetensors"
            sha256 = "abc123"
            "#,
        )
        .unwrap();
        let inline = &m.models[0].files[0];
        assert_eq!(inline.url, "https://example.com/config.json");
        assert_eq!(inline.destination, "models/m1/config.json");
        assert!(inline.sha256.is_none());
        let block = &m.models[1].files[0];
        assert_eq!(block.destination, "models/m2/model.safetensors");
        assert_eq!(block.sha256.as_deref(), Some("abc123"));
    }

    #[test]
    fn rejects_unsafe_destination() {
        let manifest = |dest: &str| {
            format!(
                r#"
                [backend]
                source = "github.com/x/y"
                name = "Y"
                version = "1.0.0"
                kind = "subprocess"
                entrypoint = "y"
                contract = "v1"
                description = "Test backend."

                [[models]]
                name = "m"
                primary_language = "en"
                supported_languages = ["en"]
                supported_devices = ["cpu"]
                files = [{{ url = "https://example.com/x", destination = "{dest}" }}]
                "#
            )
        };
        for bad in ["../escape", "/abs/path", "a/../b", "models/"] {
            let err = Manifest::parse(&manifest(bad)).unwrap_err();
            assert!(
                matches!(err, ManifestError::UnsafeDestination(_)),
                "destination {bad:?} should be rejected, got {err}"
            );
        }
        // A nested relative path is accepted.
        Manifest::parse(&manifest("models/m/model.safetensors")).expect("safe nested path");
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
            description = "Test backend."

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
            description = "Test backend."
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
            description = "Test backend."

            [[assets.subprocess]]
            file = "y-cuda13.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 13
            "#,
        )
        .unwrap();
        let a = &m.assets.subprocess[0];
        assert_eq!(a.accel, vec![Accel::Cuda]);
        assert_eq!(a.cuda_major, Some(13));
        assert_eq!(a.cuda_sm, None);
        assert!(!a.cudnn);
    }

    #[test]
    fn parses_multipart_subprocess_asset() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[assets.subprocess]]
            parts = ["y-cuda13.tar.gz.part00", "y-cuda13.tar.gz.part01"]
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 13
            "#,
        )
        .unwrap();
        let a = &m.assets.subprocess[0];
        assert!(a.is_multipart());
        assert_eq!(a.file, None);
        assert_eq!(
            a.release_files(),
            vec!["y-cuda13.tar.gz.part00", "y-cuda13.tar.gz.part01"]
        );
    }

    #[test]
    fn empty_file_string_normalizes_to_parts() {
        // Regression (Tier 1 #25): `file = ""` plus valid `parts` used to pass
        // parse but leave `file = Some("")`, so `release_files()` returned `[""]`
        // and `is_multipart()` was false. Parse must normalize empty -> None.
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[assets.subprocess]]
            file = ""
            parts = ["y.tar.gz.part00", "y.tar.gz.part01"]
            target = "x86_64-unknown-linux-gnu"
            accel = "cpu"
            "#,
        )
        .unwrap();
        let a = &m.assets.subprocess[0];
        assert_eq!(a.file, None);
        assert!(a.is_multipart());
        assert_eq!(
            a.release_files(),
            vec!["y.tar.gz.part00", "y.tar.gz.part01"]
        );
    }

    #[test]
    fn rejects_subprocess_asset_with_both_file_and_parts() {
        let err = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[assets.subprocess]]
            file = "y.tar.gz"
            parts = ["y.tar.gz.part00"]
            target = "x86_64-unknown-linux-gnu"
            accel = "cpu"
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ManifestError::AssetFileXorParts(_)));
    }

    #[test]
    fn rejects_subprocess_asset_with_neither_file_nor_parts() {
        let err = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            entrypoint = "y"
            contract = "v1"
            description = "Test backend."

            [[assets.subprocess]]
            target = "x86_64-unknown-linux-gnu"
            accel = "cpu"
            "#,
        )
        .unwrap_err();
        assert!(matches!(err, ManifestError::AssetFileXorParts(_)));
    }

    #[test]
    fn device_from_str_round_trips_canonical_forms() {
        for device in [Device::Cpu, Device::Gpu, Device::None] {
            let s = device.to_string();
            let parsed: Device = s.parse().unwrap();
            assert_eq!(device, parsed, "round-trip failed for {s}");
        }
    }

    /// `cuda` and `metal` are the spelling every shipped manifest and published
    /// index uses. They are accepted as input and mapped onto the one device that
    /// means "an accelerator"; nothing ever writes them back.
    #[test]
    fn deprecated_device_spellings_parse_as_gpu() {
        assert_eq!("cuda".parse(), Ok(Device::Gpu));
        assert_eq!("metal".parse(), Ok(Device::Gpu));
        assert_eq!("gpu".parse(), Ok(Device::Gpu));
        assert_eq!("cpu".parse(), Ok(Device::Cpu));
        assert_eq!("none".parse(), Ok(Device::None));
        assert!(
            "rocm".parse::<Device>().is_err(),
            "rocm is an accel, not a device"
        );
        assert!("nonsense".parse::<Device>().is_err());
    }

    #[test]
    fn device_never_emits_a_deprecated_spelling() {
        for device in [Device::Cpu, Device::Gpu, Device::None] {
            let text = device.to_string();
            assert!(
                !matches!(text.as_str(), "cuda" | "metal"),
                "Display emitted a deprecated spelling: {text}"
            );
            assert_eq!(text.parse(), Ok(device), "round trip for {text}");
        }
        assert_eq!(Device::Gpu.to_string(), "gpu");
    }

    #[test]
    fn a_manifest_declaring_cuda_yields_gpu() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            contract = "v1"
            entrypoint = "y"
            license = "Apache-2.0"
            description = "Test backend."

            [[assets.subprocess]]
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 12

            [[models]]
            name = "m"
            supported_devices = ["cpu", "cuda"]
            primary_language = "en"
            supported_languages = ["en"]
        "#,
        )
        .expect("shipped manifests must keep parsing");
        assert_eq!(
            m.models[0].supported_devices,
            vec![Device::Cpu, Device::Gpu]
        );
    }

    #[test]
    fn device_from_str_rejects_non_canonical_strings() {
        // `rocm` is an `Accel` build axis, never a model `Device`; non-snake_case
        // and unknown strings must error so callers don't accept stale forms.
        for bad in ["rocm", "Cpu", "CUDA", "metal_gpu", ""] {
            assert!(
                bad.parse::<Device>().is_err(),
                "{bad:?} should fail to parse as a Device"
            );
        }
    }

    /// Build a minimal valid manifest around one `[[assets.subprocess]]` body, so
    /// asset-level validation tests carry only the lines under test.
    fn manifest_with_asset(asset_body: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse(&format!(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            contract = "v1"
            entrypoint = "y"
            license = "Apache-2.0"
            description = "Test backend."

            [[assets.subprocess]]
            {asset_body}

            [[models]]
            name = "m"
            supported_devices = ["cpu"]
            primary_language = "en"
            supported_languages = ["en"]
        "#
        ))
    }

    /// A scalar `accel` is the spelling every shipped manifest uses, and
    /// `backend.toml` is a pinned release asset — rejecting it would break
    /// already-installed backends on users' machines.
    #[test]
    fn a_scalar_accel_parses_as_a_one_element_list() {
        let m = Manifest::parse(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "subprocess"
            contract = "v1"
            entrypoint = "y"
            license = "Apache-2.0"
            description = "Test backend."

            [[assets.subprocess]]
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = "cuda"
            cuda_major = 12

            [[models]]
            name = "m"
            supported_devices = ["cpu"]
            primary_language = "en"
            supported_languages = ["en"]
        "#,
        )
        .expect("a scalar accel must parse");
        assert_eq!(m.assets.subprocess[0].accel, vec![Accel::Cuda]);
    }

    #[test]
    fn a_list_accel_parses() {
        let m = manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = ["cuda", "rocm"]
            cuda_major = 12
            gfx = ["gfx1030"]
        "#,
        )
        .expect("a dual-runtime asset must parse");
        assert_eq!(m.assets.subprocess[0].accel, vec![Accel::Cuda, Accel::Rocm]);
        assert_eq!(
            m.assets.subprocess[0].gfx,
            vec![crate::arch::GfxSpec::new(10, 3, 0)]
        );
    }

    #[test]
    fn an_empty_accel_list_is_rejected() {
        let err = manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = []
        "#,
        )
        .expect_err("an asset must declare at least one accel");
        assert!(format!("{err}").contains("accel"), "{err}");
    }

    #[test]
    fn rocm_requires_gfx_and_forbids_it_elsewhere() {
        let err = manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = ["rocm"]
        "#,
        )
        .expect_err("a rocm asset must list its gfx targets");
        assert!(format!("{err}").contains("gfx"), "{err}");

        let err = manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = ["cpu"]
            gfx = ["gfx1030"]
        "#,
        )
        .expect_err("gfx is meaningless without rocm");
        assert!(format!("{err}").contains("gfx"), "{err}");
    }

    #[test]
    fn vulkan_api_is_allowed_only_with_vulkan() {
        manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = ["vulkan"]
            vulkan_api = "1.2"
        "#,
        )
        .expect("a vulkan asset may declare an api floor");

        let err = manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = ["cpu"]
            vulkan_api = "1.2"
        "#,
        )
        .expect_err("vulkan_api without vulkan is a contradiction");
        assert!(format!("{err}").contains("vulkan"), "{err}");
    }

    #[test]
    fn cuda_fields_are_gated_on_accel_containing_cuda() {
        manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = ["cuda", "rocm"]
            cuda_major = 12
            cuda_sm = 86
            gfx = ["gfx1030"]
        "#,
        )
        .expect("a dual asset may carry both vendors' discriminators");

        let err = manifest_with_asset(
            r#"
            file = "y.tar.gz"
            target = "x86_64-unknown-linux-gnu"
            accel = ["rocm"]
            gfx = ["gfx1030"]
            cuda_sm = 86
        "#,
        )
        .expect_err("cuda_sm without cuda is a contradiction");
        assert!(format!("{err}").contains("cuda"), "{err}");
    }
}
