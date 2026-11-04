// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/language` — the language multilingual models speak by default.
//!
//! The middle term of a three-step resolution: a model's own override wins,
//! then this global setting, then the model's declared primary language. That
//! ordering is why this endpoint answers with a bare tag while
//! `/pipeline/{stage}/model/{model}/language` answers with a block explaining
//! where the effective tag came from — a global value is a stated preference,
//! not a promise that any particular model honors it. A model that cannot speak
//! the chosen tag falls through to its own default rather than refusing to
//! synthesize.
//!
//! What this setting *accepts* is a separate path, `/settings/language/list`,
//! for the reason every "is set" / "may be set" pair on this surface is split:
//! choosing a language changes one of the two and not the other, and a client
//! that re-read both after every click would be re-reading a list that only
//! moves when a backend is installed or removed.

use super::super::wire::{LanguageList, LanguageState};

settings_dispatch!(
    get_language,
    "get_primary_language",
    get "/settings/language",
    LanguageState,
    "Read the default synthesis language",
    "A BCP-47 tag, `auto`, or `null` when nothing is configured. This is the stored preference \
rather than what any one model ended up speaking — for that, read \
`GET /pipeline/{stage}/model/{model}/language`, which reports how the three settings resolved."
);
settings_setter!(
    set_language,
    SetLanguageBody { language: String },
    "set_primary_language",
    "language",
    "/settings/language",
    LanguageState,
    "Set the default synthesis language",
    "Sets the language every multilingual model speaks unless something more specific overrides \
it: a per-model setting, or a `language` field in a single `POST /speak` body. It spares a user \
who speaks one language from pinning every model individually. A model that does not serve the \
chosen tag ignores it and uses its own default, so this never makes a model fail to speak.",
    "A tag `GET /settings/language/list` offers, such as `es-MX`, or `auto` to let each model \
choose; any other is refused. Use `DELETE` to clear it.",
);
settings_dispatch!(
    clear_language,
    "clear_primary_language",
    delete "/settings/language",
    LanguageState,
    "Clear the default synthesis language",
    "Removes the global preference, returning every model to its own declared language. \
Per-model overrides are untouched — this clears the middle term of the resolution, not the one \
above it. Distinct from setting `auto`, which is itself a stated preference that a model may act \
on."
);

settings_dispatch!(
    list_languages,
    "list_primary_languages",
    get "/settings/language/list",
    LanguageList,
    "List the languages the global setting accepts",
    "The tags `POST /settings/language` will take, plus the reserved `auto`.

Fill the global language picker from this rather than from a BCP-47 list of your own: the answer \
is the union of what the installed models can actually speak, so a tag no model serves is never \
offered — a setting that changed nothing is worse than one that is absent. It follows that the \
list grows and shrinks as backends are installed and removed, so re-read it after \
`POST /registry/backend/install` or `DELETE /backend/{backend_id}` instead of caching it for the \
session. `auto` is always present, even with nothing installed.

A stored value whose last supporting model was uninstalled stays put — it is still the user's \
preference — so tolerate a current value that is not on this list rather than silently rewriting \
it. A tag for one particular model belongs on that model, at \
`POST /pipeline/{stage}/model/{model}/language`, whose own list may be narrower."
);
