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
/// reads it: this repo's Pages site.
pub const INDEX_URL: &str = "https://jorge-menjivar.github.io/super-voice/index.json";

/// Super Voice's names: everything its daemon and its clients meet on, from
/// the socket to the scopes a token can carry. See [`ProductSpec`].
///
/// One product, so one list: the scopes and topics of both features, beside
/// the core ones every daemon has. The speech-to-text feature's come first.
pub static SUPER_VOICE: ProductSpec = ProductSpec {
    display_name: "Super Voice",
    slug: "super-voice",
    short_name: "voice",
    env_prefix: "SUPER_VOICE",
    // Backends published for the two products Super Voice continues read
    // their contract variables under those products' names.
    former_env_prefixes: &["SUPER_STT", "SUPER_TTS"],
    tcp_port: 7300,
    repo: "github.com/jorge-menjivar/super-voice",
    index_url: INDEX_URL,
    scopes: &[
        "transcribe",
        "recording_events",
        "global_transcriptions",
        "speak",
        "voices",
        "playback_events",
    ],
    topics: &[
        ("recording_started", "recording_events"),
        ("recording_stopped", "recording_events"),
        ("recording_state", "recording_events"),
        ("transcribing_started", "recording_events"),
        ("transcribing_stopped", "recording_events"),
        ("partial_stt", "global_transcriptions"),
        ("final_stt", "global_transcriptions"),
        ("speaking_state", "playback_events"),
        ("speech_progress", "playback_events"),
        ("playback_frequency_bands", "audio_visualization"),
    ],
    // The CLI, and each feature's app until the two merge. They skip the
    // consent dialog when they sit beside the daemon.
    clients: &["super-voice-app", "super-voice-cli"],
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
/// Each generation is additive over the one before (v2 is v1 plus the
/// `[[models]].role` field and the `POST /v1/process` route), so a backend
/// declares the *lowest* generation whose fields it uses, and a daemon
/// supports every generation up to the one it was built with.
///
/// Super Voice continues Super STT's line: every backend published for Super
/// STT declares one of these generations and keeps working. Text to speech
/// joins at v4, the first generation Super Voice ships.
///
/// Closed on purpose; see [`Generation`].
///
/// Variant order is generation order; the derived `Ord` relies on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Contract {
    /// The v1 contract (`docs/protocol/backend/contract.md`): transcription
    /// only.
    V1,
    /// v1 plus `[[models]].role` and `POST /v1/process`: a backend may serve
    /// transcript post-processors.
    V2,
    /// v2 plus nothing in the manifest: what v3 adds is what the daemon
    /// provides. A subprocess backend is granted a cache directory that
    /// survives the process (`SUPER_VOICE_BACKEND_CACHE_DIR`, with
    /// `XDG_CACHE_HOME` and `CUDA_CACHE_PATH` pointed into it), and the
    /// load-progress fields of `GET /v1/status` (`phase`, `step`, `progress`)
    /// are read. A backend that builds GPU kernels at load needs both: without
    /// the cache it rebuilds them on every load, and without the progress it
    /// runs into the flat load limit of a daemon that predates the stall rule.
    V3,
    /// v3 plus a required `[backend].product`, so the manifest says it is
    /// built for Super Voice, and `[[models]].prepare_files`: the daemon
    /// downloads those files first and sends `POST /v1/prepare`, so a first
    /// load's GPU kernels are built while the weights still download. Text to
    /// speech starts here: `role = "synthesis"`, its model keys, and
    /// `POST /v1/synthesize`.
    V4,
}

impl Generation for Contract {
    const ALL: &'static [Self] = &[Self::V1, Self::V2, Self::V3, Self::V4];
    const LATEST: Self = Self::V4;

    /// A new row is a forecast until its release ships, and nothing can check
    /// it: the version that introduces a generation is by definition not yet
    /// tagged when the row is written. Renumbering that release means
    /// renumbering here.
    fn min_client(self) -> &'static str {
        match self {
            // Not 0.1.0: `[backend].contract` (and the backend manifest
            // itself) first shipped in 0.2.0 (#212). A 0.1.x daemon has no
            // notion of an installable backend to gate.
            Self::V1 => "0.2.0",
            Self::V2 => "0.2.4-beta.1",
            // v4 ships in the same release as v3. There is no 0.2.5: the
            // release after 0.2.4 is 0.3.0, the first Super Voice release.
            Self::V3 | Self::V4 => "0.3.0",
        }
    }
}

impl fmt::Display for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::V1 => write!(f, "v1"),
            Self::V2 => write!(f, "v2"),
            Self::V3 => write!(f, "v3"),
            Self::V4 => write!(f, "v4"),
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
    /// Opt into being handed the user's dictation context: an `x-voice-prompt`
    /// header and an `x-voice-vocabulary` header, on every `/v1` request to a
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
    /// transcript, or speaking text. Default [`ModelRole::Transcription`], so
    /// every manifest written before the field existed keeps its models
    /// transcribing.
    ///
    /// A v2 field, and `synthesis` a v4 value.
    #[serde(default)]
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
    /// Advisory. The authoritative rate is the `x-voice-sample-rate` response
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    /// Speech to text, stage 1: audio in, text out. The default, and what
    /// every model was before the field existed.
    #[default]
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
    /// `transcription`, `post_processor` or `synthesis`. An index published
    /// before the field existed has none, and its models transcribe.
    #[serde(default = "default_role")]
    pub role: String,
}

/// The index's role when an entry has none: [`ModelRole`]'s default, spelled,
/// so the two cannot drift.
fn default_role() -> String {
    ModelRole::default().to_string()
}

/// A Super Voice rule a manifest breaks, reported as
/// [`ManifestError::Product`].
#[derive(Debug, thiserror::Error)]
pub enum VoiceManifestError {
    #[error("`[backend] description` is empty; say in one line what the backend is")]
    EmptyDescription,
    #[error("`[capabilities] streaming_input` is reserved and not yet supported")]
    StreamingInputReserved,
    #[error(
        "model `{model}` has role = synthesis, which requires `contract = \"v4\"`; this \
         manifest declares `contract = \"{declared}\"`"
    )]
    SynthesisRequiresV4 { model: String, declared: Contract },
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

/// A text-to-speech model key: added in v4.
const fn synthesis_key(key: &'static str) -> ContractField<Contract> {
    ContractField {
        since: Contract::V4,
        rule: FieldRule::Added,
        table: "models",
        key,
    }
}

impl Product for Voice {
    type Contract = Contract;
    type Capabilities = VoiceCapabilities;
    type Model = VoiceModel;
    type IndexModel = VoiceIndexModel;

    const SLUG: &'static str = "super-voice";

    /// Super Voice continues Super STT, so a manifest written for Super STT is
    /// one of its own: every backend published before the rename names it.
    /// Super TTS's are not, since its contract line was its own.
    const FORMER_SLUGS: &'static [&'static str] = &["super-stt"];

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
        // entry against, so a published backend has always needed one: the
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
        // Every backend published before the field existed is a speech-to-text
        // backend of this line, so it stays optional below v4 and an absent one
        // still reads as this product's. Another product's manifest always
        // names its product, so the rule that refuses it needs no generation.
        ContractField {
            since: Contract::V4,
            rule: FieldRule::RequiredFrom,
            table: "backend",
            key: "product",
        },
        ContractField {
            since: Contract::V4,
            rule: FieldRule::Added,
            table: "models",
            key: "prepare_files",
        },
        // Text to speech's model keys. A daemon of an older generation cannot
        // speak, so a backend using them declares v4; the role value itself
        // is checked in `validate`, since this table holds keys only.
        synthesis_key("max_input_chars"),
        synthesis_key("output_sample_rate"),
        synthesis_key("default_voice"),
        synthesis_key("voice_kinds"),
        synthesis_key("clone_ref_seconds"),
        synthesis_key("clone_needs_transcript"),
        synthesis_key("voices"),
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
                // Older generations are read by daemons that have no
                // synthesis, which would list the backend and then fail to
                // install it.
                if manifest.backend.contract < Contract::V4 {
                    return Err(VoiceManifestError::SynthesisRequiresV4 {
                        model: model.name.clone(),
                        declared: manifest.backend.contract,
                    }
                    .into());
                }
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
