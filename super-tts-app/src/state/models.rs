// SPDX-License-Identifier: GPL-3.0-only

//! Data models and types for the Super TTS application.

// Re-export AudioTheme from shared crate
pub use super_tts_shared::models::theme::AudioTheme;

/// Daemon connection status
#[derive(Debug, Clone, Default, PartialEq)]
pub enum DaemonStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
    /// User denied the settings-scope consent prompt (either just now
    /// or via the daemon's sticky deny cache). All settings-scope
    /// operations will fail until the user explicitly retries
    /// authorization — usually after `systemctl --user restart
    /// super-tts`. The auto-retry loop is suppressed in this state to
    /// avoid spamming the daemon's deny cache.
    Blocked(String),
}

/// Whether the daemon is speaking.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum SpeakingStatus {
    #[default]
    Idle,
    Speaking,
}

/// The page to display in the application
#[derive(Debug, Clone)]
pub enum Page {
    Connection,
    Customization,
    Speech,
    /// The active synthesis backend: its model picker, load/unload, and a
    /// side sheet for switching which installed backend is active.
    Models,
    /// Manage installed backends and browse installable ones (the old Models
    /// page's Installed / Browse tabs). No activation here — that lives on the
    /// Models page.
    Library,
    /// The cloned-voice library: record or import a reference clip, and manage
    /// the voices built from them.
    Voices,
    /// Self-update: current/latest version, automatic-check and beta-opt-in
    /// settings, and the apply flow.
    Updates,
}

/// The context page to display in the context drawer
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    /// Right-side sheet for adding a backend from a Git repo URL or a local
    /// directory. Scoped to the Models page — it closes on navigation away or
    /// daemon disconnect (see `AppModel::context_drawer`).
    AddBackend,
    /// Right-side sheet for editing the active/selected backend's secrets and
    /// options. Reuses the drawer instead of a full-page takeover so the
    /// backend list stays visible behind it. The backend is identified by
    /// `AppModel::configure_backend`; also Models-scoped.
    ConfigureBackend,
    /// Right-side search sheet for picking a speech language. Scope
    /// (global vs per-model) is carried by `AppModel::language_picker_target`.
    LanguagePicker,
    /// Right-side sheet for choosing which installed backend to activate (the
    /// Models page's "Select synthesis backend" flow). Scoped to the Models
    /// page; picking a backend activates it and closes the sheet.
    SelectBackend,
}

/// Where a transient action error belongs, so a failed save surfaces as an
/// inline banner on the owning surface instead of hijacking the whole UI
/// (Tier 1 #13) or being dropped to the log (Tier 1 #15). One scope-tagged
/// slot ([`ActionError`]) is held on `AppModel`; the audit's "one error
/// surface" (App Tier 3 #11) generalizes this.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ErrorScope {
    /// The Customization page's Audio section: theme / volume / feedback saves,
    /// and the global Primary Language save.
    Customization,
    /// The per-backend Configure sheet: secret / option saves.
    ConfigureBackend,
    /// The Speech page: notification-method saves and the test utterance.
    Speech,
    /// The Voices page: recording, upload, rename, and delete failures.
    Voices,
    /// The Models page's active-backend card: the per-model voice control.
    /// Its own scope rather than [`Self::ConfigureBackend`], which renders
    /// inside the Configure sheet — a sheet the user is not looking at when
    /// they pick a voice on the card.
    Models,
}

/// A scope-tagged, transient action failure rendered as an inline banner on the
/// page named by [`ErrorScope`]. Set on a failed save, cleared when the user
/// retries that action, reconnects, or closes the owning surface.
#[derive(Clone, Debug)]
pub struct ActionError {
    pub scope: ErrorScope,
    pub message: String,
}

/// The per-model language resolution block returned by
/// `GET /pipeline/{stage}/model/{model}/language`, deserialized once at the
/// client boundary instead of being poked field-by-field as a
/// `serde_json::Value` in the views. Unknown/absent fields default so a partial
/// or null block yields an empty, harmless resolution.
///
/// It says which language is in effect and why — not which languages *could*
/// be. That list is a separate call (`.../language/list`), because the two
/// change on different occasions: pinning a language rewrites this block and
/// leaves the offered set untouched. A `supported` field once rode along here
/// and was dropped from the wire; anything that reads one from this block is
/// reading a list that is always empty.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct LanguageResolution {
    /// The effective BCP-47 tag in use, if any (`None` when unresolved).
    #[serde(default)]
    pub effective: Option<String>,
    /// How `effective` was resolved: `"override"`, `"global"`, or `"default"`.
    #[serde(default)]
    pub source: String,
    /// The global Primary Language tag used as the default fallback.
    #[serde(default)]
    pub primary: String,
}

/// How a model's voice resolves, from
/// `GET /pipeline/{stage}/model/{model}/voice`.
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
///
/// Two values like the language half, and for the same reason: what is set and
/// what may be set arrive from different endpoints, because picking a voice
/// rewrites the first and cannot change the second.
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

/// Which tab of the Models page is active.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ModelsTab {
    /// Backends already installed and discovered by the daemon.
    #[default]
    Installed,
    /// Official backends the user can install.
    Download,
}

/// Menu actions
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
}
