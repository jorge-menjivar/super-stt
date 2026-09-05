// SPDX-License-Identifier: GPL-3.0-only
//! `/language` \u{2014} the transcription language every model uses by default.
//!
//! The per-model override lives at
//! `/pipeline/{stage}/model/{model}/language` (see
//! `crate::daemon::http::v1::backends::model_language`), and a single request
//! can override both by sending `language` in the `POST /transcribe` body.

use super::super::wire::{LanguageList, LanguageState};

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

settings_dispatch!(
    list_languages,
    "list_primary_languages",
    get "/settings/language/list",
    LanguageList,
    "List the languages the global setting accepts",
    "The tags `POST /settings/language` will take, plus the reserved `auto`.

Fill the global language picker from this rather than from a BCP-47 list of your own: \
the tags are region-qualified on purpose, and a backend is free to declare either `en` \
or `en-US` for its models — the daemon narrows a qualified global to whichever a model \
actually serves, which is a rule no client can infer.

A tag for one particular model belongs on that model, at \
`POST /pipeline/{stage}/model/{model}/language`, whose own list may be narrower."
);
