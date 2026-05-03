// SPDX-License-Identifier: GPL-3.0-only
//! Registry of built-in models known to Super STT.
//!
//! [`ModelDefinition`] is the unified model identity used everywhere a fully
//! resolved model is needed. Built-in entries are listed inline in [`ALL`]
//! as a real `const`. Custom models discovered from `custom_models_dir` are
//! constructed at resolution time with [`ModelSource::Custom`] and use the
//! same type — name fields are `Cow<'static, str>` so they can carry either
//! a static literal (built-ins) or an owned `String` (customs).
//!
//! ## Identity
//!
//! `(name, provider, source_kind)` is the canonical wire-level identity.
//! `Provider` carries the engine family (Whisper / Voxtral / Online API);
//! `SourceKind` distinguishes Builtin (HF cache) vs. Custom (user disk) vs.
//! Online (no files). This lets a custom model and a built-in coexist with
//! the same name + provider — they're genuinely different models, not a
//! shadowing case.
//!
//! See `docs/model_definition.md` for the broader design.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;

use crate::models::provider::{OnlineProvider, Provider};

const MB: u64 = 1_048_576;
const GB: u64 = 1_073_741_824;

const WHISPER_ARCH: &str = "WhisperForConditionalGeneration";
const VOXTRAL_ARCH: &str = "VoxtralForConditionalGeneration";

/// Where a model lives. Each variant fully describes how to fetch / address
/// the model files (or, for online providers, that there are none).
#[derive(Clone, Debug)]
pub enum ModelSource {
    /// Built-in model fetched from a `HuggingFace` repo into the local cache.
    /// `repo` and `revision` are `&'static str` because every entry that
    /// uses this variant is listed inline in [`ALL`].
    HuggingFace {
        repo: &'static str,
        revision: &'static str,
    },
    /// User-provided model loaded from a directory under `custom_models_dir`.
    Custom { path: PathBuf },
    /// Served by an external API; no local files.
    Online,
}

impl ModelSource {
    /// The kind of source — used as an identity discriminator.
    #[must_use]
    pub fn kind(&self) -> SourceKind {
        match self {
            Self::HuggingFace { .. } => SourceKind::Builtin,
            Self::Custom { .. } => SourceKind::Custom,
            Self::Online => SourceKind::Online,
        }
    }
}

/// Identity discriminator carried on the wire alongside `(name, provider)`.
///
/// `Provider` says *what kind* of engine; `SourceKind` says *where it
/// comes from*. The pair lets a custom and built-in model with the same
/// name + provider coexist without shadowing.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    /// Built-in model from the registry (`HuggingFace` cache).
    #[default]
    Builtin,
    /// User-provided model from `custom_models_dir`.
    Custom,
    /// Served by an external API.
    Online,
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => f.write_str("builtin"),
            Self::Custom => f.write_str("custom"),
            Self::Online => f.write_str("online"),
        }
    }
}

impl std::str::FromStr for SourceKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "builtin" => Ok(Self::Builtin),
            "custom" => Ok(Self::Custom),
            "online" => Ok(Self::Online),
            other => Err(format!("Unknown source kind: {other}")),
        }
    }
}

/// Static metadata for a single way of serving a model.
///
/// Built-in entries live in [`ALL`]; custom entries are produced on the fly
/// by the daemon when resolving an identity that points at a custom model.
#[derive(Clone, Debug)]
pub struct ModelDefinition {
    /// Wire identity. `Cow<'static, str>` so built-ins can be const-borrowed
    /// and customs can carry an owned `String`.
    pub name: Cow<'static, str>,
    /// Engine family + routing class.
    pub provider: Provider,
    /// Whether the model supports multiple languages. Custom models default
    /// to `true` (we don't know).
    pub is_multilingual: bool,
    /// Conservative GPU memory estimate including weights, KV cache, and
    /// overhead. `0` for online models.
    pub estimated_vram_bytes: u64,
    /// Suggested minimum interval between real-time processing chunks.
    pub processing_interval: Duration,
    /// Where the model lives.
    pub source: ModelSource,
}

impl ModelDefinition {
    /// Whether this online model only accepts streaming/WebSocket requests
    /// (e.g. Mistral's `voxtral-mini-transcribe-realtime-*` series rejects
    /// `/v1/audio/transcriptions` with `Invalid model`). Detected from the
    /// vendor-stable naming convention: any name containing "realtime".
    #[must_use]
    pub fn is_realtime_only(&self) -> bool {
        self.name.contains("realtime")
    }

    /// Construct a custom-model definition from a discovered directory.
    /// `provider` must be `Provider::LocalWhisper` or `Provider::LocalVoxtral`
    /// (the architecture detected from `config.json`).
    #[must_use]
    pub fn custom(name: impl Into<String>, path: PathBuf, provider: Provider) -> Self {
        Self {
            name: Cow::Owned(name.into()),
            provider,
            is_multilingual: true,
            estimated_vram_bytes: 0,
            processing_interval: Duration::from_secs(2),
            source: ModelSource::Custom { path },
        }
    }
}

// ── Built-in model registry ──────────────────────────────────────────────
// All built-in models defined inline as a real `const`. The order is the
// order the daemon returns when listing available models, and the first
// entry is the fresh-install default returned by `default_definition()`.

/// All built-in models.
pub const ALL: &[ModelDefinition] = &[
    // ── Local Whisper ────────────────────────────────────────────────────
    ModelDefinition {
        name: Cow::Borrowed("whisper-tiny"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 250 * MB,
        processing_interval: Duration::from_secs(1),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-tiny",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-tiny.en"),
        provider: Provider::LocalWhisper,
        is_multilingual: false,
        estimated_vram_bytes: 250 * MB,
        processing_interval: Duration::from_secs(1),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-tiny.en",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-base"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 500 * MB,
        processing_interval: Duration::from_millis(1500),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-base",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-base.en"),
        provider: Provider::LocalWhisper,
        is_multilingual: false,
        estimated_vram_bytes: 500 * MB,
        processing_interval: Duration::from_millis(1500),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-base.en",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-small"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-small",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-small.en"),
        provider: Provider::LocalWhisper,
        is_multilingual: false,
        estimated_vram_bytes: GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-small.en",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-medium"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 2 * GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-medium",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-medium.en"),
        provider: Provider::LocalWhisper,
        is_multilingual: false,
        estimated_vram_bytes: 2 * GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-medium.en",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-large"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 4 * GB,
        processing_interval: Duration::from_secs(5),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-large",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-large-v2"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 4 * GB,
        processing_interval: Duration::from_secs(5),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-large-v2",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-large-v3"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 4 * GB,
        processing_interval: Duration::from_secs(5),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-large-v3",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-large-v3-turbo"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 3 * GB,
        processing_interval: Duration::from_secs(3),
        source: ModelSource::HuggingFace {
            repo: "openai/whisper-large-v3-turbo",
            revision: "main",
        },
    },
    // ── Local distil-Whisper ─────────────────────────────────────────────
    ModelDefinition {
        name: Cow::Borrowed("whisper-distil-medium.en"),
        provider: Provider::LocalWhisper,
        is_multilingual: false,
        estimated_vram_bytes: 2 * GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "distil-whisper/distil-medium.en",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-distil-large-v2"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 3 * GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "distil-whisper/distil-large-v2",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("whisper-distil-large-v3"),
        provider: Provider::LocalWhisper,
        is_multilingual: true,
        estimated_vram_bytes: 3 * GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "distil-whisper/distil-large-v3",
            revision: "main",
        },
    },
    // ── Local Voxtral ────────────────────────────────────────────────────
    ModelDefinition {
        name: Cow::Borrowed("voxtral-small"),
        provider: Provider::LocalVoxtral,
        is_multilingual: true,
        estimated_vram_bytes: 50 * GB,
        processing_interval: Duration::from_secs(3),
        source: ModelSource::HuggingFace {
            repo: "mistralai/Voxtral-Small-24B-2507",
            revision: "main",
        },
    },
    ModelDefinition {
        name: Cow::Borrowed("voxtral-mini"),
        provider: Provider::LocalVoxtral,
        is_multilingual: true,
        estimated_vram_bytes: 8 * GB,
        processing_interval: Duration::from_secs(2),
        source: ModelSource::HuggingFace {
            repo: "mistralai/Voxtral-Mini-3B-2507",
            revision: "main",
        },
    },
    // ── OpenAI ───────────────────────────────────────────────────────────
    ModelDefinition {
        name: Cow::Borrowed("whisper-1"),
        provider: Provider::Online(OnlineProvider::OpenAI),
        is_multilingual: true,
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        source: ModelSource::Online,
    },
    ModelDefinition {
        name: Cow::Borrowed("gpt-4o-transcribe"),
        provider: Provider::Online(OnlineProvider::OpenAI),
        is_multilingual: true,
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        source: ModelSource::Online,
    },
    ModelDefinition {
        name: Cow::Borrowed("gpt-4o-mini-transcribe"),
        provider: Provider::Online(OnlineProvider::OpenAI),
        is_multilingual: true,
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        source: ModelSource::Online,
    },
    // ── Mistral ──────────────────────────────────────────────────────────
    // Names match the exact model IDs the Mistral API accepts.
    ModelDefinition {
        name: Cow::Borrowed("voxtral-mini-latest"),
        provider: Provider::Online(OnlineProvider::Mistral),
        is_multilingual: true,
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        source: ModelSource::Online,
    },
    ModelDefinition {
        name: Cow::Borrowed("voxtral-mini-transcribe-realtime-latest"),
        provider: Provider::Online(OnlineProvider::Mistral),
        is_multilingual: true,
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_millis(200),
        source: ModelSource::Online,
    },
    // ── Deepgram ─────────────────────────────────────────────────────────
    ModelDefinition {
        name: Cow::Borrowed("nova-3"),
        provider: Provider::Online(OnlineProvider::Deepgram),
        is_multilingual: true,
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        source: ModelSource::Online,
    },
];

/// Look up a built-in model definition by `(name, provider)`. Built-ins are
/// uniquely keyed by `(name, provider)` within [`ALL`] — `SourceKind` is
/// always `Builtin`/`Online` for built-ins, never `Custom`.
#[must_use]
pub fn find_by(name: &str, provider: Provider) -> Option<&'static ModelDefinition> {
    ALL.iter()
        .find(|m| m.name == name && m.provider == provider)
}

/// Look up a built-in model definition by name only. Returns the first
/// matching entry. Prefer [`find_by`] when the provider is known.
#[must_use]
pub fn find(name: &str) -> Option<&'static ModelDefinition> {
    ALL.iter().find(|m| m.name == name)
}

/// All registry entries that share a wire name. Useful for the UI when
/// rendering the same model under multiple provider sub-tabs.
#[must_use]
pub fn find_all(name: &str) -> Vec<&'static ModelDefinition> {
    ALL.iter().filter(|m| m.name == name).collect()
}

/// The model used when no preference is configured (fresh install) — always
/// the first entry of [`ALL`].
#[must_use]
pub fn default_definition() -> &'static ModelDefinition {
    &ALL[0]
}

/// Architecture detection for a `HuggingFace` `architectures` class name.
/// Returns the corresponding local provider or `None` for unknown classes.
#[must_use]
pub fn provider_from_hf_class(name: &str) -> Option<Provider> {
    match name {
        WHISPER_ARCH => Some(Provider::LocalWhisper),
        VOXTRAL_ARCH => Some(Provider::LocalVoxtral),
        _ => None,
    }
}

/// Metadata for a user-provided custom model discovered from `custom_models_dir`.
/// Populated by the daemon's startup scan; resolved into a full
/// [`ModelDefinition`] via [`resolve`].
///
/// `provider` is the architecture detected from `config.json`
/// (`Provider::LocalWhisper` or `Provider::LocalVoxtral`).
#[derive(Clone, Debug)]
pub struct CustomModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub provider: Provider,
}

/// Resolve a wire-level `(name, provider, source)` triple into a fully-formed
/// [`ModelDefinition`].
///
/// - `SourceKind::Custom`: synthesize a custom definition from the matching
///   entry in `custom_models`. Match must agree on both `name` and `provider`.
/// - `SourceKind::Builtin` / `SourceKind::Online`: look up `(name, provider)`
///   in the registry and clone.
///
/// Returns `None` if no match is found.
#[must_use]
pub fn resolve(
    name: &str,
    provider: Provider,
    source: SourceKind,
    custom_models: &[CustomModelInfo],
) -> Option<ModelDefinition> {
    match source {
        SourceKind::Custom => custom_models
            .iter()
            .find(|cm| cm.name == name && cm.provider == provider)
            .map(|cm| ModelDefinition::custom(cm.name.as_str(), cm.path.clone(), cm.provider)),
        SourceKind::Builtin | SourceKind::Online => find_by(name, provider).cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_returns_known_models() {
        assert_eq!(find("whisper-tiny").unwrap().name, "whisper-tiny");
        assert_eq!(find("voxtral-mini").unwrap().name, "voxtral-mini");
        assert_eq!(find("nova-3").unwrap().name, "nova-3");
    }

    #[test]
    fn find_returns_none_for_unknown() {
        assert!(find("nonexistent-model").is_none());
        assert!(find("").is_none());
    }

    #[test]
    fn name_provider_pairs_are_unique() {
        let mut keys: Vec<(&str, Provider)> =
            ALL.iter().map(|m| (m.name.as_ref(), m.provider)).collect();
        let count = keys.len();
        keys.sort_unstable_by(|a, b| a.0.cmp(b.0));
        keys.dedup();
        assert_eq!(
            keys.len(),
            count,
            "duplicate (name, provider) pair in registry"
        );
    }

    #[test]
    fn find_by_disambiguates_provider() {
        let local = find_by("whisper-tiny", Provider::LocalWhisper).unwrap();
        assert!(matches!(local.source, ModelSource::HuggingFace { .. }));
        assert!(find_by("whisper-tiny", Provider::Online(OnlineProvider::OpenAI)).is_none());
    }

    #[test]
    fn local_models_have_huggingface_source() {
        for m in ALL.iter() {
            if matches!(m.provider, Provider::LocalWhisper | Provider::LocalVoxtral) {
                assert!(
                    matches!(m.source, ModelSource::HuggingFace { .. }),
                    "{}: local provider but source isn't HuggingFace",
                    m.name
                );
            }
        }
    }

    #[test]
    fn online_models_have_online_source() {
        for m in ALL.iter() {
            if matches!(m.provider, Provider::Online(_)) {
                assert!(matches!(m.source, ModelSource::Online), "{}", m.name);
            }
        }
    }

    #[test]
    fn default_is_whisper_tiny() {
        assert_eq!(default_definition().name, "whisper-tiny");
    }

    #[test]
    fn source_kind_disambiguates_builtin_and_custom() {
        // A custom whisper-tiny and the built-in whisper-tiny share name +
        // provider but differ in source kind.
        let customs = vec![CustomModelInfo {
            name: "whisper-tiny".to_string(),
            path: PathBuf::from("/tmp/my-whisper"),
            provider: Provider::LocalWhisper,
        }];

        let builtin = resolve(
            "whisper-tiny",
            Provider::LocalWhisper,
            SourceKind::Builtin,
            &customs,
        )
        .unwrap();
        let custom = resolve(
            "whisper-tiny",
            Provider::LocalWhisper,
            SourceKind::Custom,
            &customs,
        )
        .unwrap();

        assert!(matches!(builtin.source, ModelSource::HuggingFace { .. }));
        assert!(matches!(custom.source, ModelSource::Custom { .. }));
        assert_eq!(builtin.source.kind(), SourceKind::Builtin);
        assert_eq!(custom.source.kind(), SourceKind::Custom);
    }

    #[test]
    fn custom_constructor() {
        let def = ModelDefinition::custom(
            "my-fine-tune",
            PathBuf::from("/tmp/my-fine-tune"),
            Provider::LocalWhisper,
        );
        assert_eq!(def.provider, Provider::LocalWhisper);
        assert_eq!(def.source.kind(), SourceKind::Custom);
    }

    #[test]
    fn provider_from_hf_class() {
        assert_eq!(
            super::provider_from_hf_class("WhisperForConditionalGeneration"),
            Some(Provider::LocalWhisper)
        );
        assert_eq!(
            super::provider_from_hf_class("VoxtralForConditionalGeneration"),
            Some(Provider::LocalVoxtral)
        );
        assert_eq!(super::provider_from_hf_class("Unknown"), None);
    }
}
