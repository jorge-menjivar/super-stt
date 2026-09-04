// SPDX-License-Identifier: GPL-3.0-only
//! `/backends/{source}/models/{model}/language` — per-model language override.
//!
//! Re-keys the old "active model" language endpoint to an explicit
//! `(source, model)` pair, so a model's override can be read or set whether or
//! not it is currently loaded. See
//! `docs/protocol/endpoints/v1/backends/model-language.md`.
use super::{decode_source, find_backend, json_error};
use crate::daemon::http::internal::helpers::dispatch::{build_request, dispatch, narrowed};
use crate::daemon::http::state::AppState;
use crate::daemon::http::v1::wire::{FromDaemon, ModelLanguageState};
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_one, set, clear))
}

/// The language this model should transcribe in.
#[derive(Deserialize, utoipa::ToSchema)]
struct LanguageBody {
    /// A BCP-47 tag such as `es`, or `auto` to let the model detect it. Empty is
    /// refused — clear an override with `DELETE`.
    #[serde(default)]
    #[schema(example = "es")]
    language: String,
}

/// Returns an error `Response` when the backend or the named model is missing
/// (`unknown_backend` / `unknown_model`), `None` when both exist and a
/// read/write can proceed. Mirrors the analogous guard in options.rs.
async fn guard_missing(s: &AppState, source: &str, model: &str) -> Option<Response> {
    match find_backend(s, source).await {
        None => Some(json_error(StatusCode::NOT_FOUND, "unknown_backend")),
        Some(b) if b.models.iter().any(|m| m.name == model) => None,
        Some(_) => Some(json_error(StatusCode::NOT_FOUND, "unknown_model")),
    }
}

#[utoipa::path(
    get,
    path = "/backends/{source}/models/{model}/language",
    tag = "backends",
    summary = "Read a model's language override",
    description = "\
The language this specific model transcribes in, which overrides the global \
`/language` setting. Addressed by `(source, model)` rather than \"the active \
model\", so it can be read whether or not the model is loaded.

`null` means no override: the model follows the global setting.",
    params(
        ("source" = String, Path,
         description = "The backend's `source`, percent-encoded — e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
        ("model" = String, Path, description = "The model's name, as `GET /backends` spells it."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "How this model's language resolves.", body = ModelLanguageState),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such backend (`unknown_backend`) or no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_one(
    State(s): State<AppState>,
    Path((source, model)): Path<(String, String)>,
) -> Response {
    get_one_inner(s, decode_source(&source), model).await
}

async fn get_one_inner(s: AppState, source: String, model: String) -> Response {
    if let Some(r) = guard_missing(&s, &source, &model).await {
        return r;
    }
    let req = build_request(
        "get_model_language",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelLanguageState::from_daemon)
}

#[utoipa::path(
    post,
    path = "/backends/{source}/models/{model}/language",
    tag = "backends",
    summary = "Set a model's language override",
    description = "\
Pins this model to a language regardless of the global `/language` setting. A tag \
the model does not serve is refused rather than silently ignored.

Overridden in turn by a `language` field in a single `POST /transcribe` body.",
    params(
        ("source" = String, Path,
         description = "The backend's `source`, percent-encoded — e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
        ("model" = String, Path, description = "The model's name, as `GET /backends` spells it."),
    ),
    request_body = LanguageBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Override set.", body = ModelLanguageState),
        (status = 400, description = "The body was empty (`invalid_request`), or this model does not serve that language (`unsupported_language`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such backend (`unknown_backend`) or no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn set(
    State(s): State<AppState>,
    Path((source, model)): Path<(String, String)>,
    axum::Json(body): axum::Json<LanguageBody>,
) -> Response {
    let source = decode_source(&source);
    if body.language.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if let Some(r) = guard_missing(&s, &source, &model).await {
        return r;
    }
    let req = build_request(
        "set_model_language",
        Some(serde_json::json!({ "source": source, "model": model, "language": body.language })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelLanguageState::from_daemon)
}

#[utoipa::path(
    delete,
    path = "/backends/{source}/models/{model}/language",
    tag = "backends",
    summary = "Clear a model's language override",
    description = "\
Removes the per-model pin, returning this model to the global `/language` setting.",
    params(
        ("source" = String, Path,
         description = "The backend's `source`, percent-encoded — e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
        ("model" = String, Path, description = "The model's name, as `GET /backends` spells it."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Override cleared.", body = ModelLanguageState),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such backend (`unknown_backend`) or no such model (`unknown_model`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn clear(
    State(s): State<AppState>,
    Path((source, model)): Path<(String, String)>,
) -> Response {
    let source = decode_source(&source);
    if let Some(r) = guard_missing(&s, &source, &model).await {
        return r;
    }
    let req = build_request(
        "clear_model_language",
        Some(serde_json::json!({ "source": source, "model": model })),
    );
    let resp = dispatch(&s.daemon, req).await;
    narrowed(resp, ModelLanguageState::from_daemon)
}
