// SPDX-License-Identifier: GPL-3.0-only
//! `/backend/{backend_id}/context` — which dictation context one backend uses.
//!
//! Contract: `docs/protocol/endpoints/v1/context.md`.
//!
//! Singular where its siblings are plural ([`super::options`],
//! [`super::secrets`]) because a backend declares many options and many secrets
//! and has exactly one of these.
//!
//! Three states, and three verbs for them, so the wire never has to carry a
//! magic value: `DELETE` clears the override and the backend follows the active
//! context, `POST {"id": "coding"}` pins it to one, and `POST {"id": null}`
//! means send this backend no context at all. The daemon stores the last of
//! those as an empty string — an id no context can have — but that is storage's
//! business and not a client's.
//!
//! Tagged `contexts` rather than `backends`, unlike the paths beside it: a
//! reader looking up how contexts work needs this endpoint in front of them,
//! and the tag is what decides which page it renders on.

use super::{decode_source, find_backend};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::contexts::write;
use crate::daemon::http::v1::wire::{json_error, ok};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use super_stt_shared::models::contexts::DictationContext;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(
        get_backend_context,
        set_backend_context,
        delete_backend_context
    ))
}

/// Which context a backend uses, and the one it resolves to.
#[derive(Serialize, ToSchema)]
struct BackendContext {
    #[schema(example = "success")]
    status: &'static str,
    /// Which of the three this backend is in: `active` (it follows the active
    /// context, the default), `pinned` (it uses `id`), or `none` (it is sent no
    /// context at all).
    #[schema(example = "active")]
    mode: &'static str,
    /// The id this backend is pinned to. Only meaningful when `mode` is
    /// `pinned`, and `null` otherwise.
    id: Option<String>,
    /// The context this backend will actually be sent, resolved. `null` when
    /// nothing resolves — including when it is pinned to a context that has
    /// since been deleted, which falls back to nothing rather than to the
    /// active one.
    context: Option<DictationContext>,
}

/// Which context to point a backend at.
#[derive(Deserialize, ToSchema)]
struct BackendContextWrite {
    /// The context id to pin this backend to, or `null` to send it no context
    /// at all. To put it back on the active context, `DELETE` instead.
    #[serde(default)]
    id: Option<String>,
}

#[utoipa::path(
    get,
    path = "/backend/{backend_id}/context",
    tag = "contexts",
    summary = "Read a backend's dictation context",
    description = "\
Which context this backend uses, and the one that resolves for it.

`mode` says which of three: `active` means it follows the context in force \
(`GET /context/active`), which is the default and what almost every backend should be; \
`pinned` means it uses `id` whatever is active; `none` means it is sent no context.

`context` is what it will actually be sent, already resolved — a client showing what a \
backend will hear should read that rather than re-derive it.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backend/list` reports it — percent-encoded, e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "What this backend uses.", body = BackendContext),
        (status = 404, description = "No installed backend has that `source` (`unknown_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_backend_context(State(s): State<AppState>, Path(source): Path<String>) -> Response {
    read(&s, decode_source(&source)).await
}

async fn read(s: &AppState, source: String) -> Response {
    if find_backend(s, &source).await.is_none() {
        return json_error(StatusCode::NOT_FOUND, "unknown_backend");
    }
    let config = s.daemon.config.read().await;
    let (mode, id) = match config.backend_context_override(&source) {
        None => ("active", None),
        Some("") => ("none", None),
        Some(id) => ("pinned", Some(id.to_string())),
    };
    ok(&BackendContext {
        status: "success",
        mode,
        id,
        context: config.resolve_context(&source).cloned(),
    })
}

#[utoipa::path(
    post,
    path = "/backend/{backend_id}/context",
    tag = "contexts",
    summary = "Point a backend at a dictation context",
    description = "\
Pins this backend to `id`, or — with `id` as `null` — stops sending it any context at \
all. Either way it stops following the active one; `DELETE` is how it goes back to \
that.

The pin is what makes one backend differ from the rest: a cloud transcriber that bills \
per token can be given a short vocabulary while everything else follows the context the \
user picked, without either choice disturbing the other.

Takes effect on the backend's next request, not on a reload.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backend/list` reports it — percent-encoded, e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
    ),
    request_body = BackendContextWrite,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Stored; this is what the backend uses now.", body = BackendContext),
        (status = 404, description = "No installed backend has that `source` (`unknown_backend`), or no context has that id (`not_found`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn set_backend_context(
    State(s): State<AppState>,
    Path(source): Path<String>,
    axum::Json(body): axum::Json<BackendContextWrite>,
) -> Response {
    let source = decode_source(&source);
    if find_backend(&s, &source).await.is_none() {
        return json_error(StatusCode::NOT_FOUND, "unknown_backend");
    }
    // `null` on the wire is "no context at all", which the daemon stores as the
    // empty string. The two spellings differ because the wire has `DELETE` to
    // mean "follow the active one" and storage has only the one field.
    let id = body.id.unwrap_or_default();
    let payload = serde_json::json!({ "source": source, "id": id });
    if let Some(failed) = write(&s, "set_backend_context", payload).await {
        return failed;
    }
    read(&s, source).await
}

#[utoipa::path(
    delete,
    path = "/backend/{backend_id}/context",
    tag = "contexts",
    summary = "Put a backend back on the active context",
    description = "\
Clears the override, so this backend follows whichever context is active — the default \
for every backend that has never been pointed elsewhere.

Distinct from `POST {\"id\": null}`, which pins it to *no* context and keeps it there \
while the active one changes around it.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backend/list` reports it — percent-encoded, e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Cleared; this backend now follows the active context.", body = BackendContext),
        (status = 404, description = "No installed backend has that `source` (`unknown_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
/// Named for the resource rather than the verb, so it does not shadow
/// `axum::routing::delete` and so the generated operation id is unique.
async fn delete_backend_context(State(s): State<AppState>, Path(source): Path<String>) -> Response {
    let source = decode_source(&source);
    if find_backend(&s, &source).await.is_none() {
        return json_error(StatusCode::NOT_FOUND, "unknown_backend");
    }
    let payload = serde_json::json!({ "source": source, "id": null });
    if let Some(failed) = write(&s, "set_backend_context", payload).await {
        return failed;
    }
    read(&s, source).await
}
