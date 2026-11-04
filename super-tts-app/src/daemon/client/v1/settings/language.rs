// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/language` — the language multilingual models speak by default.
//!
//! The middle term of a three-step resolution: a model's own override wins,
//! then this global setting, then the model's declared primary language. That
//! is why these calls answer with a bare tag while the per-model ones in
//! [`crate::daemon::client::v1::pipeline::language`] answer with a block
//! explaining where the effective tag came from — a global value is a stated
//! preference, not a promise any particular model honors it.
//!
//! What the setting *accepts* is a separate call, [`list_primary_languages`],
//! for the reason every "is set" / "may be set" pair on this surface is split:
//! choosing a language changes one of the two and not the other.

use crate::daemon::client::internal::response::{require_success, require_unit};
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;

/// Read the global primary language (HTTP `GET /settings/language`).
///
/// `None` when unset — each model then falls back to its own declared
/// language. That is a different state from the tag `auto`, which is a stated
/// preference that a model may act on.
pub async fn get_primary_language() -> HttpResult<Option<String>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/settings/language").await?,
            "get_primary_language",
        )?;
        Ok(resp
            .language
            .and_then(|v| v.as_str().map(ToString::to_string)))
    })
    .await
}

/// Store the global primary language (HTTP `POST /settings/language`).
///
/// A model that does not serve the chosen tag ignores it and uses its own
/// default, so this can never make a model fail to speak — which is also why
/// the value is worth offering from [`list_primary_languages`] rather than a
/// list of every BCP-47 tag: a setting that silently changes nothing is worse
/// than one that is absent.
pub async fn set_primary_language(language: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let language = language.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/settings/language",
                &serde_json::json!({ "language": language }),
            )
            .await?;
            require_unit(resp, "set_primary_language")
        }
    })
    .await
}

/// Clear the global primary language (HTTP `DELETE /settings/language`).
///
/// Removes the middle term of the resolution. Per-model overrides are
/// untouched — this does not reset a model the user pinned deliberately.
pub async fn clear_primary_language() -> HttpResult<()> {
    with_settings_token(|socket, token| async move {
        let resp = transport::settings_delete(socket, &token, "/settings/language").await?;
        require_unit(resp, "clear_primary_language")
    })
    .await
}

/// The languages the global setting accepts
/// (HTTP `GET /settings/language/list`).
///
/// The daemon's own vocabulary, not a list this client curates: the answer is
/// the union of what the installed models can actually speak, so a tag no model
/// serves is never offered. Which of `en` and `en-US` to send is a rule only
/// the daemon's resolver knows.
///
/// It follows that the list grows and shrinks as backends are installed and
/// removed, so re-read it after an install or an uninstall rather than caching
/// it for the session. `auto` is always in it, even with nothing installed. A
/// currently stored value that is not on the list is still the user's
/// preference and should be tolerated rather than silently rewritten.
pub async fn list_primary_languages() -> HttpResult<Vec<String>> {
    with_settings_token(|socket, token| async move {
        let resp = require_success(
            transport::settings_get(socket, &token, "/settings/language/list").await?,
            "list_primary_languages",
        )?;
        Ok(resp.available_languages.unwrap_or_default())
    })
    .await
}
