// SPDX-License-Identifier: GPL-3.0-only
use super::{decode_source, find_backend, json_error, ok};
use crate::daemon::http::state::AppState;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/backends/{source}/secrets/list", get(list))
        .route(
            "/backends/{source}/secrets/{name}",
            get(get_one).post(set).delete(delete_secret),
        )
}

#[derive(Deserialize)]
struct SecretBody {
    #[serde(default)]
    value: String,
}

/// `GET /backends/{source}/secrets/list` — declared secrets + configured flags, no values.
async fn list(State(s): State<AppState>, Path(source): Path<String>) -> Response {
    let source = decode_source(&source);
    let Some(backend) = find_backend(&s, &source).await else {
        return json_error(StatusCode::NOT_FOUND, "unknown_backend");
    };
    let mut out = Vec::with_capacity(backend.secrets.len());
    for sec in &backend.secrets {
        let configured = crate::keyring::has_backend_secret_async(source.clone(), sec.name.clone())
            .await
            .unwrap_or(false);
        out.push(serde_json::json!({
            "name": sec.name,
            "label": sec.label.clone().unwrap_or_else(|| sec.name.clone()),
            "required": sec.required,
            "configured": configured,
        }));
    }
    ok(&serde_json::json!({ "status": "success", "secrets": out }))
}

/// `GET /backends/{source}/secrets/{name}` — existence only.
async fn get_one(
    State(s): State<AppState>,
    Path((source, name)): Path<(String, String)>,
) -> Response {
    let source = decode_source(&source);
    match declared_secret(&s, &source, &name).await {
        Guard::NoBackend => json_error(StatusCode::NOT_FOUND, "unknown_backend"),
        Guard::NoItem => json_error(StatusCode::NOT_FOUND, "unknown_secret"),
        Guard::Ok => {
            let configured = crate::keyring::has_backend_secret_async(source.clone(), name.clone())
                .await
                .unwrap_or(false);
            ok(&serde_json::json!({ "status": "success", "name": name, "configured": configured }))
        }
    }
}

/// `POST /backends/{source}/secrets/{name}` — store a value (non-empty).
async fn set(
    State(s): State<AppState>,
    Path((source, name)): Path<(String, String)>,
    axum::Json(body): axum::Json<SecretBody>,
) -> Response {
    let source = decode_source(&source);
    if body.value.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    match declared_secret(&s, &source, &name).await {
        Guard::NoBackend => json_error(StatusCode::NOT_FOUND, "unknown_backend"),
        Guard::NoItem => json_error(StatusCode::NOT_FOUND, "unknown_secret"),
        Guard::Ok => {
            let resp = s
                .daemon
                .handle_set_backend_secret(source, name, body.value)
                .await;
            secret_result(&resp, true)
        }
    }
}

/// `DELETE /backends/{source}/secrets/{name}` — clear (reset to unset).
async fn delete_secret(
    State(s): State<AppState>,
    Path((source, name)): Path<(String, String)>,
) -> Response {
    let source = decode_source(&source);
    match declared_secret(&s, &source, &name).await {
        Guard::NoBackend => json_error(StatusCode::NOT_FOUND, "unknown_backend"),
        Guard::NoItem => json_error(StatusCode::NOT_FOUND, "unknown_secret"),
        Guard::Ok => {
            let resp = s.daemon.handle_clear_backend_secret(source, name).await;
            secret_result(&resp, false)
        }
    }
}

enum Guard {
    NoBackend,
    NoItem,
    Ok,
}

async fn declared_secret(s: &AppState, source: &str, name: &str) -> Guard {
    match find_backend(s, source).await {
        None => Guard::NoBackend,
        Some(b) if b.secrets.iter().any(|x| x.name == name) => Guard::Ok,
        Some(_) => Guard::NoItem,
    }
}

/// Maps a daemon response to an HTTP result for secret write/clear operations.
///
/// Precondition: only reached after all `unknown_backend`, `unknown_secret`,
/// and empty-value guards have passed, so the sole remaining failure mode is a
/// keyring write error — safely collapsed to `503 keyring_unavailable`.
fn secret_result(
    resp: &super_stt_shared::models::protocol::DaemonResponse,
    configured: bool,
) -> Response {
    if resp.status == "success" {
        ok(&serde_json::json!({ "status": "success", "configured": configured }))
    } else {
        json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            resp.message.as_deref().unwrap_or("keyring_unavailable"),
        )
    }
}
