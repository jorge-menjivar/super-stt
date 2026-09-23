// SPDX-License-Identifier: GPL-3.0-only
//! What Super STT adds to the backend contract `super-engine-spec` describes:
//! its contract generations, and the fields its manifests and index carry on
//! top of the shared ones.

use std::fmt;

use serde::{Deserialize, Serialize};
use super_engine_spec::manifest::{ContractField, FieldRule, Manifest, ManifestError, ModelEntry};
pub use super_engine_spec::product::{Generation, Product};
use super_engine_spec::product::{SchemaNames, generation_from_str};

/// Super STT, as a [`Product`] of the backend contract.
#[derive(Debug, Clone, Copy)]
pub enum Stt {}

impl Stt {
    /// What Super STT sends as its `User-Agent` to forges and download hosts,
    /// version-stamped so their logs and rate limiters can tell which release
    /// made a request.
    pub const USER_AGENT: &str = concat!("super-stt/", env!("CARGO_PKG_VERSION"));
}

/// Backend-protocol contract version: the one thing a manifest declares about
/// what it implements.
///
/// A contract generation names a set of manifest fields and backend routes.
/// Each generation is additive over the one before — v2 is v1 plus the
/// `[[models]].role` field and the `POST /v1/process` route — so a backend
/// declares the *lowest* generation whose fields it uses, and a daemon
/// supports every generation up to the one it was built with. Closed on
/// purpose; see [`Generation`].
///
/// Variant order is generation order; the derived `Ord` relies on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Contract {
    /// The v1 contract (`docs/protocol/backend/contract.md`): transcription
    /// only.
    V1,
    /// v1 plus `[[models]].role` and `POST /v1/process` — a backend may serve
    /// transcript post-processors.
    V2,
}

impl Generation for Contract {
    const ALL: &'static [Self] = &[Self::V1, Self::V2];
    const LATEST: Self = Self::V2;

    /// A new row is a forecast until its release ships, and nothing can check
    /// it: the version that introduces a generation is by definition not yet
    /// tagged when the row is written. Renumbering that release means
    /// renumbering here.
    fn min_client(self) -> &'static str {
        match self {
            // Not 0.1.0: `[backend].contract` — and the backend manifest
            // itself — first shipped in 0.2.0 (#212). A 0.1.x daemon has no
            // notion of an installable backend to gate.
            Self::V1 => "0.2.0",
            Self::V2 => "0.2.4-beta.1",
        }
    }
}

impl fmt::Display for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1 => write!(f, "v1"),
            Self::V2 => write!(f, "v2"),
        }
    }
}

impl std::str::FromStr for Contract {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        generation_from_str(s)
    }
}

/// Routed through `FromStr` so one table — `ALL` plus `Display` — is the only
/// place a generation is spelled, and so the error names what this build does
/// know.
impl<'de> Deserialize<'de> for Contract {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The `[capabilities]` keys Super STT adds beside `websocket`.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SttCapabilities {
    /// Opt into being handed the user's dictation context: an `x-stt-prompt`
    /// header and an `x-stt-vocabulary` header, on every `/v1` request.
    /// Default `false`, which means neither header is sent.
    ///
    /// Unlike `websocket` this is not transport-restricted, and the deviation
    /// is deliberate: both headers ride the ordinary request every backend
    /// already receives, so a `subprocess` backend can use them exactly as a
    /// `wasm` one does. The flag exists to keep the headers off backends that
    /// would not know what to do with them, not to gate a transport.
    ///
    /// Declared per backend rather than per model, because it describes what
    /// the code reading the request does, and that code is the backend's.
    #[serde(default)]
    pub context: bool,
}

/// The `[[models]]` keys Super STT adds beside the shared ones.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SttModel {
    /// Force live previews onto a model that has none of its own: while the
    /// microphone is open, the daemon re-transcribes a sliding window of the
    /// capture every `processing_interval_ms` and shows the result. Each pass
    /// is a full transcription the final pass repeats — for an online model,
    /// a billed request per pass whose output is discarded — so it is off
    /// unless the manifest turns it on. Default `false`. Not consulted for a
    /// `realtime` model, which streams its own previews.
    ///
    /// A v2 field: declaring it under `contract = "v1"` is a parse error.
    #[serde(default)]
    pub force_preview_support: bool,
    /// What the model is for: transcribing audio, or post-processing a
    /// transcript. Default [`ModelRole::Transcription`], so every manifest
    /// written before the field existed keeps its models transcribing.
    ///
    /// A v2 field: declaring it under `contract = "v1"` is a parse error.
    #[serde(default)]
    pub role: ModelRole,
}

/// What a model is for.
///
/// A backend serves both roles from the same manifest and the same install: a
/// role only decides which `/v1` route the daemon drives the model over —
/// `POST /v1/transcribe` for [`Transcription`](Self::Transcription),
/// `POST /v1/process` for [`PostProcessor`](Self::PostProcessor) — and which
/// of the daemon's two model slots it may be selected into. Everything else
/// (files, devices, secrets, options, discovery, install) is identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Transcribes audio. The default, and what every model was before the
    /// field existed.
    #[default]
    Transcription,
    /// Rewrites a finished transcript — filler removal, punctuation,
    /// formatting. Driven over `POST /v1/process`.
    PostProcessor,
}

impl fmt::Display for ModelRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transcription => write!(f, "transcription"),
            Self::PostProcessor => write!(f, "post_processor"),
        }
    }
}

impl std::str::FromStr for ModelRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "transcription" => Ok(Self::Transcription),
            "post_processor" => Ok(Self::PostProcessor),
            _ => Err(format!("Unknown model role: {s}")),
        }
    }
}

/// Routed through `FromStr` so the spelling is validated identically wherever
/// a role is deserialized — TOML manifests and JSON index entries alike.
impl<'de> Deserialize<'de> for ModelRole {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl ModelRole {
    /// Whether this is the post-processing role.
    #[must_use]
    pub fn is_post_processor(self) -> bool {
        matches!(self, Self::PostProcessor)
    }
}

/// The per-model fields Super STT's index carries beside `name` and
/// `supported_devices`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttIndexModel {
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
/// cannot drift from [`ModelRole::default`].
fn default_role() -> String {
    ModelRole::default().to_string()
}

impl Product for Stt {
    type Contract = Contract;
    type Capabilities = SttCapabilities;
    type Model = SttModel;
    type IndexModel = SttIndexModel;

    const CONTRACT_FIELDS: &'static [ContractField<Contract>] = &[
        ContractField {
            since: Contract::V2,
            rule: FieldRule::Added,
            table: "models",
            key: "role",
        },
        ContractField {
            since: Contract::V2,
            rule: FieldRule::Added,
            table: "models",
            key: "force_preview_support",
        },
        // `id` names the install directory and is what the registry matches an
        // entry against, so a published backend has always needed one — the
        // indexer refuses a release without it. It stayed optional in the type
        // only so backends installed before the field existed keep loading, and
        // v2 is new enough that no such backend can declare it. Requiring it
        // here moves the failure from a rejected release to the author's
        // editor.
        ContractField {
            since: Contract::V2,
            rule: FieldRule::RequiredFrom,
            table: "backend",
            key: "id",
        },
    ];

    const SCHEMA: SchemaNames = SchemaNames {
        backend_id: "https://jorge-menjivar.github.io/super-stt/backend.schema.json",
        backend_title: "Super STT backend manifest (backend.toml)",
        registry_id: "https://jorge-menjivar.github.io/super-stt/registry.schema.json",
        registry_title: "Super STT backend registry",
    };

    /// Refuses a model file that names a host (`accel` and the selectors that
    /// narrow it). The spec reads those so one destination can have a variant
    /// per GPU, but choosing between them happens at download, and Super STT's
    /// daemon does not choose yet: it would fetch every variant into the same
    /// file. Refusing says so at install instead.
    fn validate(manifest: &Manifest<Self>) -> Result<(), ManifestError> {
        for model in &manifest.models {
            if let Some(file) = model.files.iter().find(|f| f.is_conditional()) {
                return Err(ManifestError::Product(
                    format!(
                        "model `{}`: file `{}` names a host (`accel`), and Super STT does \
                         not pick between file variants yet",
                        model.name, file.destination
                    )
                    .into(),
                ));
            }
        }
        Ok(())
    }

    fn index_model(model: &ModelEntry<SttModel>) -> SttIndexModel {
        SttIndexModel {
            role: model.product.role.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Contract, Generation, ModelRole};
    use crate::index::IndexModel;
    use crate::manifest::{Manifest, ManifestError};

    /// A wasm manifest declaring `contract`, with `backend` added to
    /// `[backend]`, `model` to its one model, and `tail` after it.
    fn manifest(contract: &str, backend: &str, model: &str, tail: &str) -> String {
        format!(
            r#"
            [backend]
            source = "github.com/x/y"
            name = "Y"
            version = "1.0.0"
            kind = "wasm"
            entrypoint = "y.wasm"
            contract = "{contract}"
            description = "Test backend."
            {backend}

            [[models]]
            name = "m"
            primary_language = "en"
            supported_languages = ["en"]
            supported_devices = ["cpu"]
            {model}

            {tail}
            "#
        )
    }

    /// The versions each generation is reported as needing are the releases
    /// that shipped them. A daemon that predates a generation shows this to the
    /// user as what to update to.
    #[test]
    fn each_generation_names_the_release_that_shipped_it() {
        assert_eq!(Contract::V1.min_client(), "0.2.0");
        assert_eq!(Contract::V2.min_client(), "0.2.4-beta.1");
        assert_eq!(Contract::LATEST, Contract::V2);
    }

    /// `role` and `force_preview_support` are v2 fields: a v1 manifest that
    /// spells either one is refused, naming the field.
    #[test]
    fn v2_model_fields_are_refused_under_v1() {
        for key in ["role = \"post_processor\"", "force_preview_support = true"] {
            match Manifest::parse(&manifest("v1", "", key, "")) {
                Err(ManifestError::FieldRequiresContract {
                    since, declared, ..
                }) => {
                    assert_eq!(since, "v2");
                    assert_eq!(declared, "v1");
                }
                other => panic!("`{key}` under v1 must be refused, got {other:?}"),
            }
        }
    }

    /// v2 requires `[backend].id`, and reads both model fields.
    #[test]
    fn v2_requires_an_id_and_reads_the_model_fields() {
        let fields = "role = \"post_processor\"\nforce_preview_support = true";
        assert!(matches!(
            Manifest::parse(&manifest("v2", "", fields, "")),
            Err(ManifestError::FieldRequiredByContract { .. })
        ));

        let m = Manifest::parse(&manifest("v2", "id = \"com.example.y\"", fields, ""))
            .expect("a v2 manifest with an id parses");
        assert!(m.models[0].product.role.is_post_processor());
        assert!(m.models[0].product.force_preview_support);
    }

    /// A model that leaves `role` out transcribes, in a manifest and in an
    /// index published before the field existed.
    #[test]
    fn a_missing_role_reads_as_transcription() {
        let m = Manifest::parse(&manifest("v1", "", "", "")).expect("parses");
        assert_eq!(m.models[0].product.role, ModelRole::Transcription);

        let model: IndexModel =
            serde_json::from_str(r#"{"name":"m","supported_devices":["cpu"]}"#).expect("parses");
        assert_eq!(model.product.role, "transcription");
    }

    /// A file that names a host is refused until the daemon picks between
    /// variants; one that does not is the plain v1 shape and still parses.
    #[test]
    fn a_file_variant_is_refused() {
        let file = |selector: &str| {
            format!(
                "[[models.files]]\nurl = \"https://h/w.bin\"\ndestination = \"w.bin\"\n{selector}"
            )
        };
        assert!(Manifest::parse(&manifest("v1", "", "", &file(""))).is_ok());
        match Manifest::parse(&manifest("v1", "", "", &file("accel = \"cuda\""))) {
            Err(ManifestError::Product(e)) => assert!(e.to_string().contains("`w.bin`"), "{e}"),
            other => panic!("a file variant must be refused, got {other:?}"),
        }
    }

    /// `[capabilities] context` is Super STT's, read beside the shared
    /// `websocket`.
    #[test]
    fn context_is_read_from_capabilities() {
        let m = Manifest::parse(&manifest("v1", "", "", "[capabilities]\ncontext = true"))
            .expect("parses");
        assert!(m.capabilities.product.context);
        assert!(!m.capabilities.websocket);
    }
}
