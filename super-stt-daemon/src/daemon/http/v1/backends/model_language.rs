// SPDX-License-Identifier: GPL-3.0-only
//! `/backends/{source}/models/{model}/language` — per-model language override.
//!
//! Re-keys the old "active model" language endpoint to an explicit
//! `(source, model)` pair, so a model's override can be read or set whether or
//! not it is currently loaded. See
//! `docs/protocol/endpoints/v1/backends/model-language.md`.
use super::{decode_source, find_backend, json_error};
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(
        "/backends/{source}/models/{model}/language",
        get(get_one).post(set).delete(clear),
    )
}

#[derive(Deserialize)]
struct LanguageBody {
    #[serde(default)]
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

/// `GET /backends/{source}/models/{model}/language` — the resolution block.
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
    let (code, _hdrs, body_str) = dispatch_command(
        &s.daemon,
        "get_model_language",
        Some(serde_json::json!({ "source": source, "model": model })),
    )
    .await;
    (code, [("content-type", "application/json")], body_str).into_response()
}

/// `POST /backends/{source}/models/{model}/language` — set the override.
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
    let (code, _hdrs, body_str) = dispatch_command(
        &s.daemon,
        "set_model_language",
        Some(serde_json::json!({ "source": source, "model": model, "language": body.language })),
    )
    .await;
    (code, [("content-type", "application/json")], body_str).into_response()
}

/// `DELETE /backends/{source}/models/{model}/language` — clear the override.
async fn clear(
    State(s): State<AppState>,
    Path((source, model)): Path<(String, String)>,
) -> Response {
    let source = decode_source(&source);
    if let Some(r) = guard_missing(&s, &source, &model).await {
        return r;
    }
    let (code, _hdrs, body_str) = dispatch_command(
        &s.daemon,
        "clear_model_language",
        Some(serde_json::json!({ "source": source, "model": model })),
    )
    .await;
    (code, [("content-type", "application/json")], body_str).into_response()
}
