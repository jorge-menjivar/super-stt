// SPDX-License-Identifier: GPL-3.0-only
//! The synthesis model's voice: which one it speaks in, and which it could.
//!
//! Two values like a model's language, and for the same reason: what is set and
//! what may be set arrive from different endpoints, because picking a voice
//! rewrites the first and cannot change the second.

/// How a model's voice resolves, from
/// `GET /tts/pipeline/{stage}/model/{model}/voice`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct VoiceResolution {
    /// The voice an utterance naming none is spoken in. `None` when the model
    /// has neither a stored voice nor a `default_voice` — the state a cloning
    /// model starts in, and the one where speaking is refused until a voice is
    /// chosen. The card says so rather than showing an empty control.
    #[serde(default)]
    pub effective: Option<String>,
    /// The stored per-model voice, or `None`.
    #[serde(default, rename = "override")]
    pub model_override: Option<String>,
    /// The manifest's `default_voice`, or `None` when it declares none.
    #[serde(default)]
    pub default: Option<String>,
    /// Which id shapes the model accepts: `preset`, `cloned`, `described`.
    ///
    /// Read to tell two empty lists apart. A model that takes `described`
    /// voices has nothing to enumerate and is working as intended; one that
    /// clones and has an empty list has no voice to speak in at all, and the
    /// card says so rather than leaving the user to find out at the first
    /// utterance.
    #[serde(default)]
    pub kinds: Vec<String>,
}

/// One voice a model can be pinned to, from `.../voice/list`.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct VoiceChoice {
    /// The `voice` id to send.
    pub id: String,
    /// Display name for the picker.
    pub label: String,
    /// `preset` or `cloned`.
    #[serde(default)]
    pub kind: String,
}

/// The per-model voice control's state.
#[derive(Debug, Clone, Default)]
pub struct VoiceState {
    /// Resolution block for the model named by `target`.
    pub resolution: Option<VoiceResolution>,
    /// The voices that model can be pinned to. Empty for a model whose voices
    /// are all described, which is what tells the card to show nothing to pick
    /// from.
    pub choices: Vec<VoiceChoice>,
    /// Which `(source, model)` the two above describe. Guards stale display:
    /// a card only reads them when this matches the model it is drawing.
    pub target: Option<(String, String)>,
}
