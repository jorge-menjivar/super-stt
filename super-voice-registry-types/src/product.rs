// SPDX-License-Identifier: GPL-3.0-only
//! What Super Voice adds to the backend contract `super-engine-spec`
//! describes: its contract generations, the fields its models declare for each
//! of its features, and the rules those fields carry.
//!
//! Super Voice is one product with two features, speech to text and text to
//! speech, and one backend manifest serves both. A model's
//! [`role`](VoiceModel::role) says which feature it is for, and a backend may
//! hold models of several roles.

use std::fmt;

use serde::{Deserialize, Serialize};
use super_engine_protocol::ProductSpec;
use super_engine_spec::manifest::{ContractField, FieldRule, Manifest, ManifestError, ModelEntry};
pub use super_engine_spec::product::{Generation, Product};
use super_engine_spec::product::{SchemaNames, generation_from_str};

/// Where Super Voice's backend registry is published, for every daemon that
/// reads it.
pub const INDEX_URL: &str = "https://jorge-menjivar.github.io/super-voice/index.json";

/// The speech-to-text daemon's names: everything it and its clients meet on,
/// from the socket to the scopes a token can carry. See [`ProductSpec`].
///
/// The daemons for the two features merge into one Super Voice daemon; until
/// they do, each keeps its own names, and both read the one registry.
pub static SUPER_STT: ProductSpec = ProductSpec {
    display_name: "Super STT",
    slug: "super-stt",
    short_name: "stt",
    env_prefix: "SUPER_STT",
    tcp_port: 7300,
    repo: "github.com/jorge-menjivar/super-stt",
    index_url: INDEX_URL,
    scopes: &["transcribe", "recording_events", "global_transcriptions"],
    topics: &[
        ("recording_started", "recording_events"),
        ("recording_stopped", "recording_events"),
        ("recording_state", "recording_events"),
        ("transcribing_started", "recording_events"),
        ("transcribing_stopped", "recording_events"),
        ("partial_stt", "global_transcriptions"),
        ("final_stt", "global_transcriptions"),
    ],
};

/// The text-to-speech daemon's names. See [`SUPER_STT`].
pub static SUPER_TTS: ProductSpec = ProductSpec {
    display_name: "Super TTS",
    slug: "super-tts",
    short_name: "tts",
    env_prefix: "SUPER_TTS",
    tcp_port: 7301,
    repo: "github.com/jorge-menjivar/super-tts",
    index_url: INDEX_URL,
    scopes: &["speak", "voices", "playback_events"],
    topics: &[
        ("speaking_state", "playback_events"),
        ("speech_progress", "playback_events"),
    ],
};

/// Super Voice, as a [`Product`] of the backend contract.
#[derive(Debug, Clone, Copy)]
pub enum Voice {}

impl Voice {
    /// What Super Voice sends as its `User-Agent` to forges and download
    /// hosts, version-stamped so their logs and rate limiters can tell which
    /// release made a request.
    pub const USER_AGENT: &str = concat!("super-voice/", env!("CARGO_PKG_VERSION"));
}

/// Backend-protocol contract version: the one thing a manifest declares about
/// what it implements.
///
/// A contract generation names a set of manifest fields and backend routes.
/// Each generation is additive over the one before, so a backend declares the
/// *lowest* generation whose fields it uses, and a daemon supports every
/// generation up to the one it was built with.
///
/// Super Voice starts a line of its own. Its `v1` holds every field Super
/// STT's `v4` and Super TTS's `v1` had, so no backend needs a later one yet.
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
            // The first Super Voice release.
            Self::V1 => "0.3.0",
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

/// The `[capabilities]` keys Super Voice adds beside `websocket`.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VoiceCapabilities {
    /// Opt into being handed the user's dictation context: an `x-stt-prompt`
    /// header and an `x-stt-vocabulary` header, on every `/v1` request to a
    /// speech-to-text model. Default `false`, which means neither header is
    /// sent.
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

/// The `[[models]]` keys Super Voice adds beside the shared ones: what the
/// model is for, and the keys of its feature.
///
/// The keys of one feature are refused on a model of another
/// ([`VoiceManifestError::FieldNotForRole`]): a `voices` list on a
/// transcription model would mean nothing, and is a manifest bug.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VoiceModel {
    /// What the model is for: transcribing audio, post-processing a
    /// transcript, or speaking text. Required.
    pub role: ModelRole,

    /// Speech to text: force live previews onto a model that has none of its
    /// own. While the microphone is open, the daemon re-transcribes a sliding
    /// window of the capture every `processing_interval_ms` and shows the
    /// result. Each pass is a full transcription the final pass repeats — for
    /// an online model, a billed request per pass whose output is discarded —
    /// so it is off unless the manifest turns it on. Default `false`. Not
    /// consulted for a `realtime` model, which streams its own previews.
    #[serde(default)]
    pub force_preview_support: bool,

    /// Text to speech: longest `text` the model accepts in one
    /// `POST /v1/synthesize`. Absent means unbounded: the daemon sends whole
    /// utterances and never splits for length. When set, the daemon chunks on
    /// sentence boundaries to stay under it.
    #[serde(default)]
    pub max_input_chars: Option<u32>,
    /// Text to speech: native output sample rate in Hz, e.g. `24000`.
    ///
    /// Advisory. The authoritative rate is the `x-tts-sample-rate` response
    /// header on each synthesis, which is what the daemon resamples from — a
    /// manifest cannot know what a cloud provider will actually return. This
    /// value exists so the settings UI can show an output rate and the daemon
    /// can pre-size buffers before the first response arrives.
    #[serde(default)]
    pub output_sample_rate: Option<u32>,
    /// Text to speech: voice used when a synthesis request omits `voice`.
    /// Required when `voices` is non-empty; must name one of them.
    #[serde(default)]
    pub default_voice: Option<String>,
    /// Text to speech: which kinds of `voice` id this model accepts. Default
    /// `["preset"]`. The daemon refuses an id shape the model did not opt
    /// into, so a backend never sees one it cannot resolve.
    #[serde(default = "default_voice_kinds")]
    pub voice_kinds: Vec<VoiceKind>,
    /// Text to speech: longest reference audio accepted for a cloned voice,
    /// in seconds. Required when `voice_kinds` contains `cloned`, forbidden
    /// otherwise.
    #[serde(default)]
    pub clone_ref_seconds: Option<f32>,
    /// Text to speech: whether registering a cloned voice requires the
    /// reference clip's transcript as well as its audio. Default `false`.
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
    /// Text to speech: preset voices the model provides. Empty for models
    /// whose voices are entirely cloned or described.
    #[serde(default)]
    pub voices: Vec<VoiceEntry>,
}

impl VoiceModel {
    /// A model of `role` with every other key at its default, as a manifest
    /// that sets none of them reads.
    #[must_use]
    pub fn new(role: ModelRole) -> Self {
        Self {
            role,
            force_preview_support: false,
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

/// What a model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Speech to text, stage 1: audio in, text out.
    Transcription,
    /// Speech to text, a later stage: rewrites the text the stage before it
    /// produced. Driven over `POST /v1/process`.
    PostProcessor,
    /// Text to speech: text in, audio out, over `POST /v1/synthesize`.
    Synthesis,
}

impl fmt::Display for ModelRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transcription => write!(f, "transcription"),
            Self::PostProcessor => write!(f, "post_processor"),
            Self::Synthesis => write!(f, "synthesis"),
        }
    }
}

impl std::str::FromStr for ModelRole {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "transcription" => Ok(Self::Transcription),
            "post_processor" => Ok(Self::PostProcessor),
            "synthesis" => Ok(Self::Synthesis),
            _ => Err(format!(
                "Unknown model role: {s}. Expected transcription, post_processor or synthesis"
            )),
        }
    }
}

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
    /// Whether this is a post-processing stage.
    #[must_use]
    pub fn is_post_processor(self) -> bool {
        matches!(self, Self::PostProcessor)
    }

    /// Whether this model is for speech to text: transcription or
    /// post-processing.
    #[must_use]
    pub fn is_speech_to_text(self) -> bool {
        matches!(self, Self::Transcription | Self::PostProcessor)
    }

    /// Whether this model is for text to speech.
    #[must_use]
    pub fn is_synthesis(self) -> bool {
        matches!(self, Self::Synthesis)
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

/// The per-model fields Super Voice's index carries beside `name` and
/// `supported_devices`: the model's role, as its manifest spells it, so a
/// client can tell what a backend's models are for before installing it.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceIndexModel {
    /// `transcription`, `post_processor` or `synthesis`.
    pub role: String,
}

/// A Super Voice rule a manifest breaks, reported as
/// [`ManifestError::Product`].
#[derive(Debug, thiserror::Error)]
pub enum VoiceManifestError {
    #[error("`[backend] description` is empty; say in one line what the backend is")]
    EmptyDescription,
    #[error("`[capabilities] streaming_input` is reserved and not yet supported")]
    StreamingInputReserved,
    #[error("model `{model}` has role = {role}, which does not take `{key}`")]
    FieldNotForRole {
        model: String,
        role: ModelRole,
        key: &'static str,
    },
    #[error(
        "model `{model}`: file `{destination}` names a host (`accel`), and speech to text \
         does not pick between file variants yet"
    )]
    FileVariantNotSupported { model: String, destination: String },
    #[error(
        "model `{model}` has role = post_processor but realtime = true; post-processors are \
         driven over POST /v1/process, not the realtime path"
    )]
    RealtimePostProcessor { model: String },
    #[error("model `{model}` declares voice id `{id}` more than once")]
    DuplicateVoiceId { model: String, id: String },
    #[error("model `{model}` declares voices but no `default_voice`")]
    MissingDefaultVoice { model: String },
    #[error("model `{model}` sets `default_voice = {voice:?}`, which is not a declared voice")]
    UnknownDefaultVoice { model: String, voice: String },
    #[error(
        "model `{model}` voice `{id}` declares language `{language}`, \
         which is not in `supported_languages`"
    )]
    VoiceLanguageUnsupported {
        model: String,
        id: String,
        language: String,
    },
    #[error("model `{model}` declares an empty `voice_kinds`")]
    VoiceKindsEmpty { model: String },
    #[error("model `{model}` accepts cloned voices but declares no `clone_ref_seconds`")]
    CloneMissingRefSeconds { model: String },
    #[error("model `{model}` declares `clone_ref_seconds` without `cloned` in `voice_kinds`")]
    CloneRefSecondsRequiresCloned { model: String },
    #[error("model `{model}` declares a non-positive `clone_ref_seconds`")]
    CloneRefSecondsInvalid { model: String },
    #[error("model `{model}` declares `clone_needs_transcript` without `cloned` in `voice_kinds`")]
    CloneTranscriptRequiresCloned { model: String },
    #[error("model `{model}` declares `max_input_chars = 0`")]
    MaxInputCharsZero { model: String },
    #[error("model `{model}` declares `output_sample_rate = {rate}`, outside 8000..=192000")]
    OutputSampleRateOutOfRange { model: String, rate: u32 },
}

impl From<VoiceManifestError> for ManifestError {
    fn from(e: VoiceManifestError) -> Self {
        ManifestError::Product(Box::new(e))
    }
}

impl Product for Voice {
    type Contract = Contract;
    type Capabilities = VoiceCapabilities;
    type Model = VoiceModel;
    type IndexModel = VoiceIndexModel;

    const SLUG: &'static str = "super-voice";

    const CONTRACT_FIELDS: &'static [ContractField<Contract>] = &[
        // `id` names the install directory and is what the registry matches
        // an entry against, and `product` is what keeps another product's
        // backend out. Both are required from the first generation: every
        // Super Voice manifest is written for this contract line.
        ContractField {
            since: Contract::V1,
            rule: FieldRule::RequiredFrom,
            table: "backend",
            key: "id",
        },
        ContractField {
            since: Contract::V1,
            rule: FieldRule::RequiredFrom,
            table: "backend",
            key: "product",
        },
    ];

    const SCHEMA: SchemaNames = SchemaNames {
        backend_id: "https://jorge-menjivar.github.io/super-voice/backend.schema.json",
        backend_title: "Super Voice backend manifest (backend.toml)",
        registry_id: "https://jorge-menjivar.github.io/super-voice/registry.schema.json",
        registry_title: "Super Voice backend registry",
    };

    fn validate(manifest: &Manifest<Self>) -> Result<(), ManifestError> {
        // The shared parser requires the key but takes any string, and `""`
        // satisfies it. The Library and the manual-install preview put this
        // line under the backend's name, and for a backend the registry does
        // not list it is the only description there is, so an empty one leaves
        // the card saying nothing about what it installs.
        if manifest.backend.description.trim().is_empty() {
            return Err(VoiceManifestError::EmptyDescription.into());
        }
        // Reserved capability: refuse rather than ignore, so an author who
        // declares it learns the path does not exist instead of shipping a
        // backend that silently never receives incremental text.
        if manifest.capabilities.product.streaming_input {
            return Err(VoiceManifestError::StreamingInputReserved.into());
        }
        for model in &manifest.models {
            if model.product.role.is_synthesis() {
                validate_synthesis(model)?;
            } else {
                validate_speech_to_text(model)?;
            }
        }
        Ok(())
    }

    fn index_model(model: &ModelEntry<VoiceModel>) -> VoiceIndexModel {
        VoiceIndexModel {
            role: model.product.role.to_string(),
        }
    }
}

/// The rules of a transcription or post-processing model.
fn validate_speech_to_text(model: &ModelEntry<VoiceModel>) -> Result<(), VoiceManifestError> {
    let name = || model.name.clone();
    let product = &model.product;
    // A key keeps its default unless a manifest set it, so a non-default
    // value is a synthesis key this model declared.
    let synthesis_keys = [
        ("max_input_chars", product.max_input_chars.is_some()),
        ("output_sample_rate", product.output_sample_rate.is_some()),
        ("default_voice", product.default_voice.is_some()),
        ("voice_kinds", product.voice_kinds != default_voice_kinds()),
        ("clone_ref_seconds", product.clone_ref_seconds.is_some()),
        ("clone_needs_transcript", product.clone_needs_transcript),
        ("voices", !product.voices.is_empty()),
    ];
    if let Some((key, _)) = synthesis_keys.into_iter().find(|(_, set)| *set) {
        return Err(VoiceManifestError::FieldNotForRole {
            model: name(),
            role: product.role,
            key,
        });
    }
    if let Some(file) = model.files.iter().find(|f| f.is_conditional()) {
        return Err(VoiceManifestError::FileVariantNotSupported {
            model: name(),
            destination: file.destination.clone(),
        });
    }
    if model.realtime && product.role.is_post_processor() {
        return Err(VoiceManifestError::RealtimePostProcessor { model: name() });
    }
    Ok(())
}

/// The rules of a synthesis model.
fn validate_synthesis(model: &ModelEntry<VoiceModel>) -> Result<(), VoiceManifestError> {
    let name = || model.name.clone();
    let product = &model.product;
    if product.force_preview_support {
        return Err(VoiceManifestError::FieldNotForRole {
            model: name(),
            role: product.role,
            key: "force_preview_support",
        });
    }
    if product.voice_kinds.is_empty() {
        return Err(VoiceManifestError::VoiceKindsEmpty { model: name() });
    }
    let mut seen = std::collections::BTreeSet::new();
    for voice in &product.voices {
        if !seen.insert(voice.id.as_str()) {
            return Err(VoiceManifestError::DuplicateVoiceId {
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
            return Err(VoiceManifestError::VoiceLanguageUnsupported {
                model: name(),
                id: voice.id.clone(),
                language: lang.clone(),
            });
        }
    }
    match &product.default_voice {
        None if !product.voices.is_empty() => {
            return Err(VoiceManifestError::MissingDefaultVoice { model: name() });
        }
        // A default naming no declared voice is only detectable here: the
        // backend would answer `unsupported_voice` for every request that
        // omitted a voice, which reads as "the model is broken".
        Some(v) if !seen.contains(v.as_str()) => {
            return Err(VoiceManifestError::UnknownDefaultVoice {
                model: name(),
                voice: v.clone(),
            });
        }
        _ => {}
    }
    let clones = product.voice_kinds.contains(&VoiceKind::Cloned);
    match product.clone_ref_seconds {
        None if clones => {
            return Err(VoiceManifestError::CloneMissingRefSeconds { model: name() });
        }
        Some(_) if !clones => {
            return Err(VoiceManifestError::CloneRefSecondsRequiresCloned { model: name() });
        }
        Some(s) if !(s.is_finite() && s > 0.0) => {
            return Err(VoiceManifestError::CloneRefSecondsInvalid { model: name() });
        }
        _ => {}
    }
    if product.clone_needs_transcript && !clones {
        return Err(VoiceManifestError::CloneTranscriptRequiresCloned { model: name() });
    }
    if product.max_input_chars == Some(0) {
        return Err(VoiceManifestError::MaxInputCharsZero { model: name() });
    }
    if let Some(rate) = product.output_sample_rate
        && !(8_000..=192_000).contains(&rate)
    {
        return Err(VoiceManifestError::OutputSampleRateOutOfRange {
            model: name(),
            rate,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
