// SPDX-License-Identifier: GPL-3.0-only

//! Transcription-language UI state, extracted from `AppModel` following the
//! `RegistryState` template (App Tier 3 #15).

use crate::state::LanguageResolution;

/// The global Primary Language plus the per-model override picker state.
#[derive(Debug, Clone, Default)]
pub struct LanguageState {
    /// Global Primary Language from the daemon (`None` = unset). Display-only cache.
    pub primary_language: Option<String>,
    /// Resolution block from `GET /backends/{source}/models/{model}/language`
    /// for the model identified by `model_language_for`.
    pub model_language: Option<LanguageResolution>,
    /// Which `(source, model)` pair `model_language` belongs to. Guards
    /// stale-block display: only use `model_language` when this matches the
    /// target `(source, model)`.
    pub model_language_for: Option<(String, String)>,
    /// The `(source, model)` pair the open per-model language sheet configures.
    /// `None` when the sheet is in global mode.
    pub language_picker_target: Option<(String, String)>,
    /// Live query text for the language search sheet.
    pub language_picker_query: String,
}
