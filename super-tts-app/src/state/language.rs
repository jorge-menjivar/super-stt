// SPDX-License-Identifier: GPL-3.0-only

//! Speech-language UI state, extracted from `AppModel` following the
//! `RegistryState` template (App Tier 3 #15).

use crate::state::LanguageResolution;

/// The global Primary Language plus the per-model override picker state.
///
/// Each half is two values, not one: what is set, and what may be set. They
/// arrive from different endpoints because only one of them changes when the
/// user picks a language, and the app keeps them apart for the same reason —
/// re-reading the offered set on every pick would be a request that can only
/// ever return what it returned before.
#[derive(Debug, Clone, Default)]
pub struct LanguageState {
    /// Global Primary Language from the daemon (`None` = unset). Display-only cache.
    pub primary_language: Option<String>,
    /// The tags the global setting accepts, from `GET /settings/language/list`.
    ///
    /// The daemon's answer, not a table this app ships: the list is the union
    /// of what the installed models can speak, so it changes as backends are
    /// installed and removed. Re-read whenever the picker opens rather than
    /// cached for the session — a stale copy offers tags that are refused on
    /// click and hides ones a new backend just added.
    pub primary_languages: Vec<String>,
    /// Resolution block from `GET /pipeline/{stage}/model/{model}/language`
    /// for the model identified by `model_language_for`.
    pub model_language: Option<LanguageResolution>,
    /// The tags that model can be pinned to, from the sibling
    /// `.../language/list`. Empty for a monolingual model, which is what tells
    /// the picker there is nothing to choose.
    ///
    /// Guarded by the same `model_language_for` pair as the block above, since
    /// both describe one model and both are refetched together.
    pub model_languages: Vec<String>,
    /// Which `(source, model)` pair `model_language` and `model_languages`
    /// belong to. Guards stale display: only use them when this matches the
    /// target `(source, model)`.
    pub model_language_for: Option<(String, String)>,
    /// The `(source, model)` pair the open per-model language sheet configures.
    /// `None` when the sheet is in global mode.
    pub language_picker_target: Option<(String, String)>,
    /// Live query text for the language search sheet.
    pub language_picker_query: String,
}
