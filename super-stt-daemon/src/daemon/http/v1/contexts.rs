// SPDX-License-Identifier: GPL-3.0-only
//! `/context/...` — the named dictation contexts and which one is in force.
//!
//! Contract: `docs/protocol/endpoints/v1/context.md`.
//!
//! A context is what the user is dictating — a prompt for a model that follows
//! instructions, a vocabulary for one that does not — so it is neither a
//! backend's setting nor a single stored value. That is why it is not under
//! `/v1/settings/`, which is documented as one value apiece, and why it is
//! a family of its own rather than an option on each backend that would have
//! to be re-entered per backend.
//!
//! Hand-written, because the settings macros generate one-value endpoints with
//! no path parameter. The template is [`super::backends::options`]; the
//! envelope helpers are shared with it.
//!
//! The per-backend override is not here — it is addressed through the backend
//! it belongs to, at [`super::backends::context`].

use super::wire::{json_error, json_error_msg, ok};
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{Ack, ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use super_stt_shared::models::contexts::DictationContext;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_contexts))
        .routes(routes!(get_active_context, set_active_context))
        .routes(routes!(get_context, set_context, delete_context))
}

/// Every context, and which one is in force.
#[derive(Serialize, ToSchema)]
struct ContextCatalog {
    #[schema(example = "success")]
    status: &'static str,
    /// The id of the context in force, or `null` when none is.
    active: Option<String>,
    /// In the order the user arranged them, which is the order to render.
    contexts: Vec<DictationContext>,
}

/// One context.
#[derive(Serialize, ToSchema)]
struct OneContext {
    #[schema(example = "success")]
    status: &'static str,
    context: DictationContext,
}

/// Which context is in force by default.
#[derive(Serialize, ToSchema)]
struct ActiveContext {
    #[schema(example = "success")]
    status: &'static str,
    /// The id in force, or `null` when none is.
    id: Option<String>,
    /// The context that id names, so a picker can render the current choice
    /// without a second request. `null` whenever `id` is.
    context: Option<DictationContext>,
}

/// The context to store. Its id comes from the path.
#[derive(Deserialize, ToSchema)]
struct ContextWrite {
    /// What the user calls it. Required — a context with no name cannot be
    /// picked out of a list.
    #[serde(default)]
    name: String,
    /// Instructions for a model that follows them. Empty is normal.
    #[serde(default)]
    prompt: String,
    /// Terms to bias recognition toward, in the order to send them. Blank
    /// entries are dropped rather than refused: a settings UI editing one
    /// input per term keeps an empty row for the next one.
    #[serde(default)]
    vocabulary: Vec<String>,
}

/// Which context to put in force.
#[derive(Deserialize, ToSchema)]
struct ActiveWrite {
    /// The id to activate, or `null` to leave no context active.
    #[serde(default)]
    id: Option<String>,
}

/// The catalog and the active id, read straight from config.
async fn catalog(s: &AppState) -> ContextCatalog {
    let config = s.daemon.config.read().await;
    ContextCatalog {
        status: "success",
        active: config.active_context().map(|c| c.id.clone()),
        contexts: config.contexts.items.clone(),
    }
}

#[utoipa::path(
    get,
    path = "/context/list",
    tag = "contexts",
    summary = "List dictation contexts",
    description = "\
Every context the user has, in the order they arranged them, plus the id of the one \
in force.

A context describes what is being dictated rather than how one backend is configured: \
a `prompt` for a model that follows instructions, and a `vocabulary` of terms to bias \
recognition toward for one that does not — which is most transcription models. The \
daemon delivers whichever context resolves for a backend to that backend, if its \
manifest says it can use one.

`active` is the id of the context in force by default. A backend may be pointed at a \
different one, or at none — see `GET /backend/{backend_id}/context`.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The contexts and the active selection.", body = ContextCatalog),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn list_contexts(State(s): State<AppState>) -> Response {
    ok(&catalog(&s).await)
}

#[utoipa::path(
    get,
    path = "/context/{id}",
    tag = "contexts",
    summary = "Read one dictation context",
    description = "\
One context by id: its name, its prompt, and its vocabulary as an ordered list of \
terms.

The vocabulary is a list rather than one blob of text because every consumer splits it \
differently \u{2014} Deepgram wants one `keyterm` per term, `whisper-1` wants them \
joined \u{2014} and a term holding whatever delimiter a blob chose, like \
`Menjivar, Jorge`, would be ambiguous. The daemon splits once so nobody downstream \
guesses.",
    params(
        ("id" = String, Path, description = "The context's id, as `GET /context/list` reports it.", example = "coding"),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The context.", body = OneContext),
        (status = 404, description = "No context has that id (`unknown_context`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_context(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    read_context(&s, &id).await
}

async fn read_context(s: &AppState, id: &str) -> Response {
    let config = s.daemon.config.read().await;
    match config.context(id) {
        Some(context) => ok(&OneContext {
            status: "success",
            context: context.clone(),
        }),
        None => json_error(StatusCode::NOT_FOUND, "unknown_context"),
    }
}

#[utoipa::path(
    post,
    path = "/context/{id}",
    tag = "contexts",
    summary = "Create or replace a dictation context",
    description = "\
Stores this context under `id`, replacing one already there. Answers with the context \
as stored, so a client can render the result without a second read.

Upsert rather than a create/update pair, because `id` is the client's to choose — \
there is no id-minting step for two verbs to straddle. An existing context keeps its \
place in the list; a new one is appended.

`id` is a slug: 1 to 64 characters of lowercase letters, digits, `-` and `_`. `active` \
and `list` are refused, since both are paths in this namespace and a context taking \
either would be stored and then be unreachable.

Blank vocabulary terms are dropped and each term is trimmed. The stored prompt is at \
most 4000 characters and the vocabulary at most 200 terms or 4000 characters, because \
a context is delivered to a backend as request headers and one that cannot be sent is \
better refused than stored.

A running backend picks this up on its next request — there is no reload to do.",
    params(
        ("id" = String, Path, description = "The id to store it under. A new one creates a context; an existing one replaces it.", example = "coding"),
    ),
    request_body = ContextWrite,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Stored; this is the context as saved.", body = OneContext),
        (status = 400, description = "The id is not a slug, is reserved, the name is empty, or the prompt or vocabulary is longer than can be delivered (`invalid_value`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn set_context(
    State(s): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<ContextWrite>,
) -> Response {
    let context = DictationContext {
        id: id.clone(),
        name: body.name,
        prompt: body.prompt,
        vocabulary: body.vocabulary,
    };
    // The limits are the daemon's, not this layer's: it is the one place every
    // caller reaches, and the one that knows what can actually be delivered.
    let payload = match serde_json::to_value(&context) {
        Ok(value) => serde_json::json!({ "context": value }),
        Err(e) => {
            return json_error_msg(
                StatusCode::BAD_REQUEST,
                "invalid_value",
                &format!("that context cannot be encoded: {e}"),
            );
        }
    };
    if let Some(failed) = write(&s, "set_context", payload).await {
        return failed;
    }
    read_context(&s, &id).await
}

#[utoipa::path(
    delete,
    path = "/context/{id}",
    tag = "contexts",
    summary = "Delete a dictation context",
    description = "\
Removes the context. If it was the active one, nothing is active afterwards.

A backend *pinned* to it keeps its pin, and resolves to no context until the id exists \
again — deleting a context is not the same as undoing the choice to pin a backend to \
it, and re-creating it restores what the user set.",
    params(
        ("id" = String, Path, description = "The context's id.", example = "coding"),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Deleted.", body = Ack),
        (status = 404, description = "No context has that id (`not_found`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
/// Named for the resource rather than the verb, so it does not shadow
/// `axum::routing::delete` and so the generated operation id is unique.
async fn delete_context(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    dispatched(&s, "delete_context", serde_json::json!({ "id": id })).await
}

#[utoipa::path(
    get,
    path = "/context/active",
    tag = "contexts",
    summary = "Read the active dictation context",
    description = "\
The context in force by default, as both its id and the object itself, so a picker can \
render the current choice in one request.

Default, not universal: a backend pointed somewhere else by \
`POST /backend/{backend_id}/context` ignores this.",
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The active selection; `id` is `null` when none is active.", body = ActiveContext),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_active_context(State(s): State<AppState>) -> Response {
    read_active(&s).await
}

async fn read_active(s: &AppState) -> Response {
    let config = s.daemon.config.read().await;
    let context = config.active_context().cloned();
    ok(&ActiveContext {
        status: "success",
        id: context.as_ref().map(|c| c.id.clone()),
        context,
    })
}

#[utoipa::path(
    post,
    path = "/context/active",
    tag = "contexts",
    summary = "Choose the active dictation context",
    description = "\
Puts a context in force by default, or — with `id` as `null` — leaves none active.

Every backend that has not been pointed elsewhere follows this, and picks it up on its \
next request rather than on a reload.",
    request_body = ActiveWrite,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Chosen; this is the selection now in force.", body = ActiveContext),
        (status = 404, description = "No context has that id (`not_found`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn set_active_context(
    State(s): State<AppState>,
    axum::Json(body): axum::Json<ActiveWrite>,
) -> Response {
    if let Some(failed) = write(
        &s,
        "set_active_context",
        serde_json::json!({ "id": body.id }),
    )
    .await
    {
        return failed;
    }
    read_active(&s).await
}

/// Run a write command, returning the error response when it failed and `None`
/// when it succeeded.
///
/// The caller answers a success by re-reading what it just wrote, so the shape
/// a client gets back from a write is the same one it gets from the matching
/// read — there is no second body to keep in step.
pub(super) async fn write(
    s: &AppState,
    command: &str,
    payload: serde_json::Value,
) -> Option<Response> {
    let (code, _hdrs, body) = dispatch_command(&s.daemon, command, Some(payload)).await;
    (code != StatusCode::OK)
        .then(|| (code, [("content-type", "application/json")], body).into_response())
}

/// Run a write command and answer with its own acknowledgement.
///
/// For the writes with nothing to read back afterwards.
async fn dispatched(s: &AppState, command: &str, payload: serde_json::Value) -> Response {
    let (code, _hdrs, body) = dispatch_command(&s.daemon, command, Some(payload)).await;
    (code, [("content-type", "application/json")], body).into_response()
}
