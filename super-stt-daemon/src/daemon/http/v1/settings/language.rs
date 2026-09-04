// SPDX-License-Identifier: GPL-3.0-only
//! `/language` \u{2014} the transcription language every model uses by default.
//!
//! The per-model override lives at
//! `/pipeline/{stage}/model/{model}/language` (see
//! `crate::daemon::http::v1::backends::model_language`), and a single request
//! can override both by sending `language` in the `POST /transcribe` body.

use super::super::wire::LanguageState;

settings_dispatch!(
    get_language,
    "get_primary_language",
    get "/settings/language",
    LanguageState,
    "Read the default transcription language",
    "A BCP-47 tag, `auto`, or `null` when nothing is configured."
);
settings_setter!(
    set_language,
    SetLanguageBody { language: String },
    "set_primary_language",
    "language",
    "/settings/language",
    LanguageState,
    "Set the default transcription language",
    "Sets the language every model transcribes in unless something more specific \
overrides it: a per-model setting, or a `language` field in a single \
`POST /transcribe` body.",
    "A BCP-47 tag such as `es`, or `auto` to let the model detect the language.",
);
settings_dispatch!(
    clear_language,
    "clear_primary_language",
    delete "/settings/language",
    LanguageState,
    "Clear the default transcription language",
    "Removes the global setting, returning every model to detecting the language \
itself. Per-model overrides are untouched."
);
