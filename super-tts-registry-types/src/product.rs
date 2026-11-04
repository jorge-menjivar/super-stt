// SPDX-License-Identifier: GPL-3.0-only
//! What Super TTS adds to the backend contract `super-engine-spec` describes:
//! its contract generations, the voice and synthesis fields its models
//! declare, and the rules those fields carry.

use std::fmt;

use serde::{Deserialize, Serialize};
use super_engine_protocol::ProductSpec;
use super_engine_spec::manifest::{ContractField, FieldRule, Manifest, ManifestError, ModelEntry};
pub use super_engine_spec::product::{Generation, Product};
use super_engine_spec::product::{SchemaNames, generation_from_str};

/// Super TTS's names: everything the daemon and its clients meet on, from the
/// socket to the scopes a token can carry. See [`ProductSpec`].
///
/// Defined here, beside [`Tts`], rather than in `super-tts-shared`, so the
/// installer can name it without that crate's keyring and HTTP client.
/// `super_tts_shared::product` re-exports it, with the tests that pin what
/// Super TTS shipped.
pub static SUPER_TTS: ProductSpec = ProductSpec {
    display_name: "Super TTS",
    slug: "super-tts",
    short_name: "tts",
    env_prefix: "SUPER_TTS",
    tcp_port: 7301,
    repo: "github.com/jorge-menjivar/super-tts",
    index_url: "https://jorge-menjivar.github.io/super-tts/index.json",
    scopes: &["speak", "voices", "playback_events"],
    topics: &[
        ("speaking_state", "playback_events"),
        ("speech_progress", "playback_events"),
    ],
};

/// Super TTS, as a [`Product`] of the backend contract.
#[derive(Debug, Clone, Copy)]
pub enum Tts {}

impl Tts {
    /// What Super TTS sends as its `User-Agent` to forges and download hosts,
    /// version-stamped so their logs and rate limiters can tell which release
    /// made a request.
    pub const USER_AGENT: &str = concat!("super-tts/", env!("CARGO_PKG_VERSION"));
}

/// Backend-protocol contract version: the one thing a manifest declares about
/// what it implements.
///
/// A contract generation names a set of manifest fields and backend routes.
/// Each generation is additive over the one before, so a backend declares the
/// *lowest* generation whose fields it uses, and a daemon supports every
/// generation up to the one it was built with.
///
/// Closed on purpose; see [`Generation`].
///
/// Variant order is generation order; the derived `Ord` relies on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Contract {
    /// The v1 contract (`docs/protocol/backend/contract.md`).
    V1,
}

impl Generation for Contract {
    const ALL: &'static [Self] = &[Self::V1];
    const LATEST: Self = Self::V1;

    /// A new row is a forecast until its release ships, and nothing can check
    /// it: the version that introduces a generation is by definition not yet
    /// tagged when the row is written. Renumbering that release means
    /// renumbering here.
    fn min_client(self) -> &'static str {
        match self {
            // The manifest and its `[backend].contract` field both date from
            // the first Super TTS release; there is no earlier daemon to gate.
            Self::V1 => "0.1.0",
        }
    }
}

impl fmt::Display for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1 => write!(f, "v1"),
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

/// The `[capabilities]` keys Super TTS adds beside `websocket`.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TtsCapabilities {
    /// Reserved. Opts a backend into an incremental-text path, so it can hold
    /// an upstream provider's prosodic context across deltas instead of
    /// receiving one chunk per request.
    ///
    /// Not specified yet: the daemon implements no such path, and a manifest
    /// declaring it is rejected at parse rather than silently ignored — a
    /// backend author who sets this is expecting behavior that does not exist,
    /// and the loud failure is the honest answer.
    #[serde(default)]
    pub streaming_input: bool,
}

/// The `[[models]]` keys Super TTS adds beside the shared ones: what a model
/// can say, in which voices, and the bounds on a synthesis request.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TtsModel {
    /// Longest `text` the model accepts in one `POST /v1/synthesize`. Absent
    /// means unbounded: the daemon sends whole utterances and never splits for
    /// length. When set, the daemon chunks on sentence boundaries to stay
    /// under it.
    #[serde(default)]
    pub max_input_chars: Option<u32>,
    /// Native output sample rate in Hz, e.g. `24000`.
    ///
    /// Advisory. The authoritative rate is the `x-tts-sample-rate` response
    /// header on each synthesis, which is what the daemon resamples from — a
    /// manifest cannot know what a cloud provider will actually return. This
    /// value exists so the settings UI can show an output rate and the daemon
    /// can pre-size buffers before the first response arrives.
    #[serde(default)]
    pub output_sample_rate: Option<u32>,
    /// Voice used when a synthesis request omits `voice`. Required when
    /// `voices` is non-empty; must name one of them.
    #[serde(default)]
    pub default_voice: Option<String>,
    /// Which kinds of `voice` id this model accepts. Default `["preset"]`.
    /// The daemon refuses an id shape the model did not opt into, so a backend
    /// never sees one it cannot resolve.
    #[serde(default = "default_voice_kinds")]
    pub voice_kinds: Vec<VoiceKind>,
    /// Longest reference audio accepted for a cloned voice, in seconds.
    /// Required when `voice_kinds` contains `cloned`, forbidden otherwise.
    #[serde(default)]
    pub clone_ref_seconds: Option<f32>,
    /// Whether registering a cloned voice requires the reference clip's
    /// transcript as well as its audio. Default `false`.
    ///
    /// In-context cloning conditions on the reference audio *and* the tokens
    /// of what it says, so a backend taking that path cannot build a voice
    /// from audio alone; speaker-embedding cloning needs no transcript and
    /// leaves this unset. The daemon refuses to register a transcript-less
    /// clip against a model that declares it, which turns a synthesis failure
    /// on every later request into one refusal at the point the voice is
    /// added. Only meaningful alongside `cloned` in `voice_kinds`.
    #[serde(default)]
    pub clone_needs_transcript: bool,
    /// Preset voices the model provides. Empty for models whose voices are
    /// entirely cloned or described.
    #[serde(default)]
    pub voices: Vec<VoiceEntry>,
}

impl Default for TtsModel {
    fn default() -> Self {
        Self {
            max_input_chars: None,
            output_sample_rate: None,
            default_voice: None,
            voice_kinds: default_voice_kinds(),
            clone_ref_seconds: None,
            clone_needs_transcript: false,
            voices: Vec::new(),
        }
    }
}

/// A kind of `voice` id a model accepts on `POST /v1/synthesize`.
///
/// The kind is carried by the id's shape, so the daemon can route without
/// asking the backend: a bare id is a preset, `voice:<uuid>` is a cloned voice
/// the daemon holds reference audio for, and `desc:<text>` is a free-text
/// description for models that can synthesize a voice from one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum VoiceKind {
    /// A voice declared in `[[models.voices]]`, named by a bare id.
    Preset,
    /// A user-cloned voice, `voice:<uuid>`. The daemon pushes reference audio
    /// on the request; requires `clone_ref_seconds`.
    Cloned,
    /// A free-text voice description, `desc:<text>`.
    Described,
}

impl fmt::Display for VoiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preset => write!(f, "preset"),
            Self::Cloned => write!(f, "cloned"),
            Self::Described => write!(f, "described"),
        }
    }
}

/// One `[[models.voices]]` entry: a preset voice the backend provides.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VoiceEntry {
    /// The `voice` id sent on `POST /v1/synthesize`. Unique within the model.
    pub id: String,
    /// Display name for the voice picker. Falls back to `id` when absent.
    #[serde(default)]
    pub label: Option<String>,
    /// Primary language of the voice. Must be one of the model's
    /// `supported_languages` when set.
    #[serde(default)]
    pub language: Option<String>,
    /// Free-form tags for filtering in the picker, e.g. `["warm", "female"]`.
    /// Not interpreted by the daemon.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl VoiceEntry {
    /// Display name: `label` when set, otherwise the bare `id`.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

fn default_voice_kinds() -> Vec<VoiceKind> {
    vec![VoiceKind::Preset]
}

/// The per-model fields Super TTS's index carries beside `name` and
/// `supported_devices`: none yet.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TtsIndexModel {}

/// A Super TTS rule a manifest breaks, reported as
/// [`ManifestError::Product`].
#[derive(Debug, thiserror::Error)]
pub enum TtsManifestError {
    /// `[backend].description` is empty or only whitespace.
    #[error("`[backend] description` is empty; say in one line what the backend is")]
    EmptyDescription,
    /// Two `[[models.voices]]` under one model share an `id`.
    #[error("model `{model}` declares voice id `{id}` more than once")]
    DuplicateVoiceId {
        /// The model declaring the duplicate.
        model: String,
        /// The repeated voice id.
        id: String,
    },
    /// A model declared `[[models.voices]]` but no `default_voice`.
    #[error("model `{model}` declares voices but no `default_voice`")]
    MissingDefaultVoice {
        /// The model missing the default.
        model: String,
    },
    /// `default_voice` does not name any declared voice.
    #[error("model `{model}` sets `default_voice = {voice:?}`, which is not a declared voice")]
    UnknownDefaultVoice {
        /// The model with the dangling default.
        model: String,
        /// The value that matched no voice.
        voice: String,
    },
    /// A voice's `language` is not in the model's `supported_languages`.
    #[error(
        "model `{model}` voice `{id}` declares language `{language}`, \
         which is not in `supported_languages`"
    )]
    VoiceLanguageUnsupported {
        /// The model declaring the voice.
        model: String,
        /// The voice id.
        id: String,
        /// The unsupported language tag.
        language: String,
    },
    /// `voice_kinds` was declared empty; a model must accept at least one kind.
    #[error("model `{model}` declares an empty `voice_kinds`")]
    VoiceKindsEmpty {
        /// The model with the empty list.
        model: String,
    },
    /// `voice_kinds` contains `cloned` but `clone_ref_seconds` is absent.
    #[error("model `{model}` accepts cloned voices but declares no `clone_ref_seconds`")]
    CloneMissingRefSeconds {
        /// The model missing the bound.
        model: String,
    },
    /// `clone_ref_seconds` was declared without `cloned` in `voice_kinds`.
    #[error("model `{model}` declares `clone_ref_seconds` without `cloned` in `voice_kinds`")]
    CloneRefSecondsRequiresCloned {
        /// The model with the stray bound.
        model: String,
    },
    /// `clone_ref_seconds` is not a positive, finite number of seconds.
    #[error("model `{model}` declares a non-positive `clone_ref_seconds`")]
    CloneRefSecondsInvalid {
        /// The model with the bad bound.
        model: String,
    },
    /// `clone_needs_transcript` was declared without `cloned` in `voice_kinds`.
    #[error("model `{model}` declares `clone_needs_transcript` without `cloned` in `voice_kinds`")]
    CloneTranscriptRequiresCloned {
        /// The model with the stray flag.
        model: String,
    },
    /// `max_input_chars` was declared as zero, which would admit no text.
    #[error("model `{model}` declares `max_input_chars = 0`")]
    MaxInputCharsZero {
        /// The model with the unusable bound.
        model: String,
    },
    /// `output_sample_rate` is outside the range any audio device serves.
    #[error("model `{model}` declares `output_sample_rate = {rate}`, outside 8000..=192000")]
    OutputSampleRateOutOfRange {
        /// The model with the implausible rate.
        model: String,
        /// The declared rate.
        rate: u32,
    },
    /// `[capabilities] streaming_input` is reserved and not yet implemented.
    #[error("`[capabilities] streaming_input` is reserved and not yet supported")]
    StreamingInputReserved,
}

impl From<TtsManifestError> for ManifestError {
    fn from(e: TtsManifestError) -> Self {
        ManifestError::Product(Box::new(e))
    }
}

impl Product for Tts {
    type Contract = Contract;
    type Capabilities = TtsCapabilities;
    type Model = TtsModel;
    type IndexModel = TtsIndexModel;

    const SLUG: &'static str = "super-tts";

    /// Adding a field to a v2 means adding a row here, and both the parser
    /// and the published JSON Schema learn the rule from it.
    const CONTRACT_FIELDS: &'static [ContractField<Contract>] = &[
        // From the first generation. Super STT reads the same manifest format
        // and numbers its contracts from `v1` too, and its backends were
        // published before the field existed, so it reads a manifest without
        // one as its own. Requiring it here is what keeps a Super TTS backend
        // out of Super STT. No Super TTS backend was published without it.
        ContractField {
            since: Contract::V1,
            rule: FieldRule::RequiredFrom,
            table: "backend",
            key: "product",
        },
    ];

    const SCHEMA: SchemaNames = SchemaNames {
        backend_id: "https://jorge-menjivar.github.io/super-tts/backend.schema.json",
        backend_title: "Super TTS backend manifest (backend.toml)",
        registry_id: "https://jorge-menjivar.github.io/super-tts/registry.schema.json",
        registry_title: "Super TTS backend registry",
    };

    fn validate(manifest: &Manifest<Self>) -> Result<(), ManifestError> {
        // The shared parser requires the key but takes any string, and `""`
        // satisfies it. The Library and the manual-install preview put this
        // line under the backend's name, and for a backend the registry does
        // not list it is the only description there is, so an empty one leaves
        // the card saying nothing about what it installs.
        if manifest.backend.description.trim().is_empty() {
            return Err(TtsManifestError::EmptyDescription.into());
        }
        // Reserved capability: refuse rather than ignore, so an author who
        // declares it learns the path does not exist instead of shipping a
        // backend that silently never receives incremental text.
        if manifest.capabilities.product.streaming_input {
            return Err(TtsManifestError::StreamingInputReserved.into());
        }
        for model in &manifest.models {
            validate_voices(model)?;
        }
        Ok(())
    }

    fn index_model(_model: &ModelEntry<TtsModel>) -> TtsIndexModel {
        TtsIndexModel {}
    }
}

/// Voice and synthesis-bound coherence for one `[[models]]` entry.
///
/// Kept in the canonical parser alongside the path and asset guards, so the
/// daemon and the indexer reject the same manifests — a voice the picker can
/// offer but the backend cannot resolve is a runtime failure the user sees as
/// a dead entry in a dropdown.
fn validate_voices(model: &ModelEntry<TtsModel>) -> Result<(), TtsManifestError> {
    let name = || model.name.clone();

    if model.product.voice_kinds.is_empty() {
        return Err(TtsManifestError::VoiceKindsEmpty { model: name() });
    }

    let mut seen = std::collections::BTreeSet::new();
    for voice in &model.product.voices {
        if !seen.insert(voice.id.as_str()) {
            return Err(TtsManifestError::DuplicateVoiceId {
                model: name(),
                id: voice.id.clone(),
            });
        }
        // A voice pinned to a language the model does not accept can never
        // be selected successfully, so it is a manifest bug, not a runtime
        // one.
        if let Some(lang) = &voice.language
            && !model.supported_languages.contains(lang)
        {
            return Err(TtsManifestError::VoiceLanguageUnsupported {
                model: name(),
                id: voice.id.clone(),
                language: lang.clone(),
            });
        }
    }

    match &model.product.default_voice {
        None if !model.product.voices.is_empty() => {
            return Err(TtsManifestError::MissingDefaultVoice { model: name() });
        }
        // A default naming no declared voice is only detectable here: the
        // backend would answer `unsupported_voice` for every request that
        // omitted a voice, which reads as "the model is broken".
        Some(v) if !seen.contains(v.as_str()) => {
            return Err(TtsManifestError::UnknownDefaultVoice {
                model: name(),
                voice: v.clone(),
            });
        }
        _ => {}
    }

    let clones = model.product.voice_kinds.contains(&VoiceKind::Cloned);
    match model.product.clone_ref_seconds {
        None if clones => {
            return Err(TtsManifestError::CloneMissingRefSeconds { model: name() });
        }
        Some(_) if !clones => {
            return Err(TtsManifestError::CloneRefSecondsRequiresCloned { model: name() });
        }
        Some(s) if !(s.is_finite() && s > 0.0) => {
            return Err(TtsManifestError::CloneRefSecondsInvalid { model: name() });
        }
        _ => {}
    }
    if model.product.clone_needs_transcript && !clones {
        return Err(TtsManifestError::CloneTranscriptRequiresCloned { model: name() });
    }

    if model.product.max_input_chars == Some(0) {
        return Err(TtsManifestError::MaxInputCharsZero { model: name() });
    }

    if let Some(rate) = model.product.output_sample_rate
        && !(8000..=192_000).contains(&rate)
    {
        return Err(TtsManifestError::OutputSampleRateOutOfRange {
            model: name(),
            rate,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Product, SUPER_TTS, Tts, TtsManifestError, VoiceKind};
    use crate::manifest::{Manifest, ManifestError};

    const VALID: &str = r#"
        [backend]
        source = "github.com/x/y"
        name = "Y"
        version = "1.0.0"
        kind = "wasm"
        entrypoint = "y.wasm"
        product = "super-tts"
        contract = "v1"
        description = "Test backend."
        "#;

    /// The Super TTS rule `text` breaks. Panics when it parses, or when
    /// something other than a Super TTS rule refuses it.
    fn refusal(text: &str) -> TtsManifestError {
        match Manifest::parse(text) {
            Err(ManifestError::Product(e)) => *e
                .downcast::<TtsManifestError>()
                .expect("the refusal is a Super TTS rule"),
            other => panic!("expected a Super TTS rule to refuse this, got {other:?}"),
        }
    }

    /// `VALID` plus one `[[models]]` entry, so the voice rules have something
    /// to attach to. `body` is appended inside the model table.
    fn with_model(body: &str) -> String {
        format!(
            "{VALID}
            [[models]]
            name = \"m\"
            primary_language = \"en\"
            supported_languages = [\"en\", \"es\"]
            supported_devices = [\"cpu\"]
            {body}
            "
        )
    }

    /// The shared parser already refuses a manifest without the key; this is
    /// the half it lets through, a key holding nothing to show.
    #[test]
    fn an_empty_description_is_refused() {
        for blank in ["\"\"", "\"   \""] {
            let text = VALID.replace("\"Test backend.\"", blank);
            assert!(
                matches!(refusal(&text), TtsManifestError::EmptyDescription),
                "description = {blank} must be refused"
            );
        }
    }

    #[test]
    fn a_model_declaring_no_voice_fields_accepts_presets_only() {
        let m = Manifest::parse(&with_model("")).expect("voice fields are all optional");
        let model = &m.models[0];
        assert_eq!(model.product.voice_kinds, vec![VoiceKind::Preset]);
        assert!(model.product.voices.is_empty());
        assert!(model.product.default_voice.is_none());
        assert!(model.product.max_input_chars.is_none());
        assert!(model.product.output_sample_rate.is_none());
    }

    #[test]
    fn parses_preset_voices_with_a_default() {
        let m = Manifest::parse(&with_model(
            r#"
            default_voice = "bella"
            output_sample_rate = 24000
            max_input_chars = 500
            voices = [
                { id = "bella", label = "Bella", language = "en", tags = ["warm"] },
                { id = "nova" },
            ]
            "#,
        ))
        .expect("a well-formed voice table parses");
        let model = &m.models[0];
        assert_eq!(model.product.voices.len(), 2);
        assert_eq!(model.product.voices[0].display_name(), "Bella");
        // `label` absent falls back to the bare id.
        assert_eq!(model.product.voices[1].display_name(), "nova");
        assert_eq!(model.product.default_voice.as_deref(), Some("bella"));
        assert_eq!(model.product.max_input_chars, Some(500));
        assert_eq!(model.product.output_sample_rate, Some(24000));
    }

    #[test]
    fn rejects_duplicate_voice_ids() {
        let err = refusal(&with_model(
            r#"
            default_voice = "bella"
            voices = [{ id = "bella" }, { id = "bella" }]
            "#,
        ));
        assert!(matches!(err, TtsManifestError::DuplicateVoiceId { .. }));
    }

    #[test]
    fn rejects_voices_without_a_default() {
        let err = refusal(&with_model(r#"voices = [{ id = "bella" }]"#));
        assert!(matches!(err, TtsManifestError::MissingDefaultVoice { .. }));
    }

    #[test]
    fn rejects_a_default_voice_that_names_nothing() {
        let err = refusal(&with_model(
            r#"
            default_voice = "ghost"
            voices = [{ id = "bella" }]
            "#,
        ));
        assert!(matches!(err, TtsManifestError::UnknownDefaultVoice { .. }));
    }

    #[test]
    fn rejects_a_voice_language_the_model_does_not_support() {
        let err = refusal(&with_model(
            r#"
            default_voice = "bella"
            voices = [{ id = "bella", language = "ja" }]
            "#,
        ));
        assert!(matches!(
            err,
            TtsManifestError::VoiceLanguageUnsupported { .. }
        ));
    }

    #[test]
    fn rejects_an_empty_voice_kinds_list() {
        let err = refusal(&with_model("voice_kinds = []"));
        assert!(matches!(err, TtsManifestError::VoiceKindsEmpty { .. }));
    }

    #[test]
    fn cloning_requires_a_reference_length_bound() {
        let err = refusal(&with_model(r#"voice_kinds = ["cloned"]"#));
        assert!(matches!(
            err,
            TtsManifestError::CloneMissingRefSeconds { .. }
        ));

        let m = Manifest::parse(&with_model(
            r#"
            voice_kinds = ["preset", "cloned"]
            clone_ref_seconds = 12.0
            "#,
        ))
        .expect("cloned + a bound parses");
        assert_eq!(m.models[0].product.clone_ref_seconds, Some(12.0));
    }

    #[test]
    fn rejects_a_reference_bound_without_cloning() {
        let err = refusal(&with_model("clone_ref_seconds = 12.0"));
        assert!(matches!(
            err,
            TtsManifestError::CloneRefSecondsRequiresCloned { .. }
        ));
    }

    #[test]
    fn rejects_a_non_positive_reference_bound() {
        for bad in ["0.0", "-3.0", "nan"] {
            let err = refusal(&with_model(&format!(
                "voice_kinds = [\"cloned\"]\nclone_ref_seconds = {bad}"
            )));
            assert!(
                matches!(err, TtsManifestError::CloneRefSecondsInvalid { .. }),
                "clone_ref_seconds = {bad} must be rejected, got {err:?}"
            );
        }
    }

    /// The transcript requirement rides on cloning: on its own it would
    /// promise a rule the daemon has no cloned voice to apply it to.
    #[test]
    fn a_transcript_requirement_needs_cloning() {
        let err = refusal(&with_model("clone_needs_transcript = true"));
        assert!(matches!(
            err,
            TtsManifestError::CloneTranscriptRequiresCloned { .. }
        ));

        let m = Manifest::parse(&with_model(
            r#"
            voice_kinds = ["cloned"]
            clone_ref_seconds = 20.0
            clone_needs_transcript = true
            "#,
        ))
        .expect("cloning may require a transcript");
        assert!(m.models[0].product.clone_needs_transcript);
        assert!(
            !Manifest::parse(&with_model(""))
                .expect("defaults parse")
                .models[0]
                .product
                .clone_needs_transcript,
            "a model that says nothing needs no transcript"
        );
    }

    #[test]
    fn rejects_a_zero_max_input_chars() {
        let err = refusal(&with_model("max_input_chars = 0"));
        assert!(matches!(err, TtsManifestError::MaxInputCharsZero { .. }));
    }

    #[test]
    fn rejects_an_implausible_output_sample_rate() {
        for bad in [7999, 192_001] {
            let err = refusal(&with_model(&format!("output_sample_rate = {bad}")));
            assert!(matches!(
                err,
                TtsManifestError::OutputSampleRateOutOfRange { .. }
            ));
        }
        assert!(Manifest::parse(&with_model("output_sample_rate = 48000")).is_ok());
    }

    /// A manifest names Super TTS by the slug it goes by everywhere else.
    #[test]
    fn a_manifest_names_super_tts_by_its_slug() {
        assert_eq!(Tts::SLUG, SUPER_TTS.slug);
    }

    /// v1 requires `[backend].product`, and there is no older contract to
    /// drop to.
    #[test]
    fn a_manifest_must_name_its_product() {
        let without = VALID.replace("        product = \"super-tts\"\n", "");
        let err = Manifest::parse(&without).expect_err("v1 requires the product");
        assert_eq!(
            err.to_string(),
            "`contract = \"v1\"` requires `[backend].product`, which this manifest does \
             not declare — add it"
        );
    }

    /// `prepare_files` is a v1 field. No Super TTS backend shipped before it,
    /// so the first generation has it, where Super STT's has it from its `v4`.
    #[test]
    fn prepare_files_parse_under_v1() {
        let m = Manifest::parse(&with_model(
            r#"prepare_files = ["m/config.json"]
            files = [{ url = "https://h/config.json", destination = "m/config.json" }]"#,
        ))
        .expect("prepare_files under v1 parses");
        assert_eq!(m.models[0].prepare_files, ["m/config.json"]);
    }

    /// A manifest naming any product but Super TTS is refused. Super STT's
    /// backends name theirs from its `v4`, and before that leave it out, which
    /// the rule above refuses too.
    #[test]
    fn a_manifest_for_another_product_is_refused() {
        for product in ["super-stt", "super-ttss"] {
            let other = VALID.replace("super-tts", product);
            match Manifest::parse(&other) {
                Err(ManifestError::OtherProduct { declared, expected }) => {
                    assert_eq!(declared, product);
                    assert_eq!(expected, "super-tts");
                }
                other => panic!("{product} must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_the_reserved_streaming_input_capability() {
        let t = format!("{VALID}\n[capabilities]\nstreaming_input = true\n");
        let err = refusal(&t);
        assert!(matches!(err, TtsManifestError::StreamingInputReserved));
        // The neighbouring capability is unaffected.
        let ok = format!("{VALID}\n[capabilities]\nwebsocket = true\n");
        assert!(Manifest::parse(&ok).is_ok());
    }
}
