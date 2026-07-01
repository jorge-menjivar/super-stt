// SPDX-License-Identifier: GPL-3.0-only
//! `/language` (global) transcription-language settings routes.
//!
//! The per-model override moved to
//! `/backends/{source}/models/{model}/language` (see
//! `crate::daemon::http::v1::backends::model_language`).
settings_dispatch!(get_language, "get_primary_language");
settings_setter!(
    set_language,
    SetLanguageBody { language: String },
    "set_primary_language",
    "language"
);
settings_dispatch!(clear_language, "clear_primary_language");
