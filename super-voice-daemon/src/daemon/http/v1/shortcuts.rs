// SPDX-License-Identifier: GPL-3.0-only
//! `/shortcuts` — Super Voice's keyboard shortcuts on this desktop: list,
//! add, remove. What they do and how they get onto the desktop is
//! [`crate::shortcuts`]; the work is `daemon::shortcut_handlers`.
//!
//! Contract: `docs/protocol/endpoints/v1/shortcuts.md`.

use crate::daemon::http::internal::helpers::dispatch::json_response;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use crate::shortcuts::Error;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use super_voice_shared::models::protocol::{DaemonResponse, ErrorCode};
use super_voice_shared::models::shortcuts::{ShortcutId, ShortcutList};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::daemon::http::state::AppState;

/// The list, or the coded refusal.
fn answer(result: Result<ShortcutList, Error>) -> Response {
    match result {
        Ok(list) => axum::Json(list).into_response(),
        Err(Error::KeysTaken) => json_response(&DaemonResponse::error_with_code(
            ErrorCode::ShortcutKeysTaken,
            "shortcut_keys_taken",
        ))
        .into_response(),
        Err(Error::Refused) => json_response(&DaemonResponse::error_with_code(
            ErrorCode::ShortcutRefused,
            "shortcut_refused",
        ))
        .into_response(),
        Err(Error::Unsupported) => json_response(&DaemonResponse::error_with_code(
            ErrorCode::ShortcutsUnsupported,
            "shortcuts_unsupported",
        ))
        .into_response(),
        Err(Error::Write(detail)) => json_response(&DaemonResponse::error_with_code(
            ErrorCode::Internal,
            &format!("could_not_write_shortcut: {detail}"),
        ))
        .into_response(),
    }
}

/// The `404` for a path that names no shortcut.
fn unknown_shortcut() -> Response {
    json_response(&DaemonResponse::error_with_code(
        ErrorCode::NotFound,
        "unknown_shortcut",
    ))
    .into_response()
}

#[utoipa::path(
    get,
    path = "/shortcuts",
    tag = "shortcuts",
    summary = "List the keyboard shortcuts",
    description = "\
Super Voice's keyboard shortcuts, `dictate` and `read_aloud`, and where each stands on \
this desktop: set up and on which keys, missing, blocked by another shortcut on its \
keys, or for the user to bind. `method` says how the daemon puts a shortcut on this \
desktop: `cosmic` writes COSMIC's custom shortcuts, `portal` binds it through the \
desktop's `GlobalShortcuts` portal, and `manual` means the user binds `command` in the \
desktop's settings.

Read from the desktop on every request, so a shortcut changed in its settings shows \
here at once. Through the portal, `keys` are as the desktop describes them, and empty \
when it does not say.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Every shortcut, and the method.", body = ShortcutList),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn list_shortcuts(State(state): State<AppState>) -> Response {
    answer(Ok(state.daemon.list_shortcuts().await))
}

#[utoipa::path(
    post,
    path = "/shortcuts/{id}",
    tag = "shortcuts",
    summary = "Add a keyboard shortcut",
    description = "\
Adds the shortcut on its own keys. Adding one that is set up already changes nothing \
and succeeds. Answers the whole list, as `GET /shortcuts` does.

It is never added on keys another shortcut holds: that is `409 shortcut_keys_taken`, \
and the user binds `command` to other keys instead. On a desktop the daemon has no \
method for, it is `409 shortcuts_unsupported`.

Through the portal the desktop may ask the user first, in a dialog, and the request \
waits on the answer, up to 60 seconds. The desktop may put it on other keys than it \
asked for. When the user says no, it is `409 shortcut_refused`.",
    params(("id" = ShortcutId, Path, description = "`dictate` or `read_aloud`.")),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Added, or set up already. Every shortcut.", body = ShortcutList),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No shortcut has that id (`unknown_shortcut`).", body = ErrorEnvelope),
        (status = 409, description = "Its keys are taken (`shortcut_keys_taken`), this desktop has no method (`shortcuts_unsupported`), or the user said no (`shortcut_refused`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 500, description = "The desktop's settings could not be changed.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn add_shortcut(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = id.parse::<ShortcutId>() else {
        return unknown_shortcut();
    };
    answer(state.daemon.add_shortcut(id).await)
}

#[utoipa::path(
    delete,
    path = "/shortcuts/{id}",
    tag = "shortcuts",
    summary = "Remove a keyboard shortcut",
    description = "\
Removes the shortcut, from whatever keys it is on. Removing one that is not set up \
changes nothing and succeeds. Answers the whole list, as `GET /shortcuts` does.

On COSMIC, removing `dictate` hands Super+Space back to COSMIC's own binding for it, \
switching the input source.",
    params(("id" = ShortcutId, Path, description = "`dictate` or `read_aloud`.")),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Removed, or not set up. Every shortcut.", body = ShortcutList),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No shortcut has that id (`unknown_shortcut`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 500, description = "The desktop's settings could not be changed.", body = ErrorEnvelope),
    ),
)]
pub(crate) async fn remove_shortcut(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let Ok(id) = id.parse::<ShortcutId>() else {
        return unknown_shortcut();
    };
    answer(state.daemon.remove_shortcut(id).await)
}

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_shortcuts))
        .routes(routes!(add_shortcut, remove_shortcut))
}
