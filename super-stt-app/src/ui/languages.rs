// SPDX-License-Identifier: GPL-3.0-only
//! Curated, region-qualified language tags for the global Primary Language
//! picker, plus human-friendly names. Wire form is the BCP-47 tag; names are
//! display-only. Per-model pickers reuse `friendly_name` for the model's own
//! `supported_languages` (which may include base tags).

/// `(bcp47_tag, friendly_name)` — region-qualified only. Country codes for
/// single countries; UN M.49 (`es-419`) for multi-country regions.
pub const GLOBAL_LANGUAGES: &[(&str, &str)] = &[
    ("en-US", "English (United States)"),
    ("en-GB", "English (United Kingdom)"),
    ("es-419", "Spanish (Latin America)"),
    ("es-MX", "Spanish (Mexico)"),
    ("es-US", "Spanish (United States)"),
    ("es-ES", "Spanish (Spain)"),
    ("pt-BR", "Portuguese (Brazil)"),
    ("pt-PT", "Portuguese (Portugal)"),
    ("fr-FR", "French (France)"),
    ("fr-CA", "French (Canada)"),
    ("de-DE", "German (Germany)"),
    ("it-IT", "Italian (Italy)"),
    ("ja-JP", "Japanese (Japan)"),
    ("ko-KR", "Korean (South Korea)"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("hi-IN", "Hindi (India)"),
    ("ar-SA", "Arabic (Saudi Arabia)"),
    ("ru-RU", "Russian (Russia)"),
    ("nl-NL", "Dutch (Netherlands)"),
];

/// Friendly display name for any tag. Falls back to the raw tag.
#[must_use]
pub fn friendly_name(tag: &str) -> String {
    if tag == "auto" {
        return "Auto-detect".to_string();
    }
    GLOBAL_LANGUAGES
        .iter()
        .find(|(t, _)| *t == tag)
        .map_or_else(|| tag.to_string(), |(_, name)| (*name).to_string())
}
