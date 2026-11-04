// SPDX-License-Identifier: GPL-3.0-only
//! `/backend/{backend_id}/secret/list` — a backend's credentials.
//!
//! Contract: `docs/protocol/endpoints/v1/backends/secrets.md`.
//!
//! The only part of the backend family that is not `settings`-scoped. These
//! paths carry the `secrets` scope and are registered against their own guard
//! in [`crate::daemon::http::v1`], so an app granted the run of the
//! configuration surface still cannot read whether — let alone which — API keys
//! the user has stored.
//!
//! Values leave in one direction only. They are written to the system keyring
//! and read back solely at model-load time, where they become
//! `x-tts-secret-*` request headers; no endpoint here returns one, and the
//! response types below have no field that could carry one.
use super::{decode_source, find_backend, json_error, json_error_msg, ok};
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_backend_secrets))
        .routes(routes!(
            get_backend_secret,
            set_backend_secret,
            delete_backend_secret
        ))
}

/// The secret to store.
#[derive(Deserialize, ToSchema)]
struct SecretBody {
    /// The credential itself. Written to the system keyring, never returned by
    /// any endpoint. Empty is refused — clear a secret with `DELETE`.
    #[serde(default)]
    value: String,
}

/// One secret a backend declares, and whether it is set.
#[derive(Serialize, ToSchema)]
struct SecretState {
    /// The secret's identifier, as the backend's manifest declares it.
    name: String,
    /// Human-readable label for a settings UI; falls back to `name`.
    label: String,
    /// Whether the backend refuses to load without it.
    required: bool,
    /// Whether a value is stored. The value itself is never returned.
    configured: bool,
}

/// Every secret a backend declares.
#[derive(Serialize, ToSchema)]
struct SecretList {
    #[schema(example = "success")]
    status: &'static str,
    secrets: Vec<SecretState>,
}

/// Whether one secret is set.
#[derive(Serialize, ToSchema)]
struct SecretConfigured {
    #[schema(example = "success")]
    status: &'static str,
    /// Absent on write and clear, which answer about the secret just addressed.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Whether a value is stored.
    configured: bool,
}

#[utoipa::path(
    get,
    path = "/backend/{backend_id}/secret/list",
    tag = "backends",
    summary = "List a backend's secrets",
    description = "\
Every credential this backend declares, with whether each is currently set. \
**Values are never returned** — not here, not anywhere. They live in the system \
keyring, and the only operations are write and clear.

Call this to decide what a configuration screen must ask for: a `required` secret \
that is not `configured` is the reason a model will refuse to load.

This is a separate scope from `settings` precisely because it is the credential \
surface: a token granted `settings` cannot reach it.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backend/list` reports it — percent-encoded, e.g. `github.com%2Fsuper-tts%2Fopenai`.",
         example = "github.com%2Fsuper-tts%2Fopenai"),
    ),
    security(("session_token" = ["secrets"])),
    responses(
        (status = 200, description = "The declared secrets and whether each is set.", body = SecretList),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `secrets` scope.", body = ErrorEnvelope),
        (status = 404, description = "No installed backend has that `source` (`unknown_backend`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn list_backend_secrets(State(s): State<AppState>, Path(source): Path<String>) -> Response {
    let source = decode_source(&source);
    let Some(backend) = find_backend(&s, &source).await else {
        return json_error(StatusCode::NOT_FOUND, "unknown_backend");
    };
    let mut out = Vec::with_capacity(backend.secrets.len());
    for sec in &backend.secrets {
        let configured = crate::keyring::has_backend_secret_async(source.clone(), sec.name.clone())
            .await
            .unwrap_or(false);
        out.push(SecretState {
            name: sec.name.clone(),
            label: sec.label.clone().unwrap_or_else(|| sec.name.clone()),
            required: sec.required,
            configured,
        });
    }
    ok(&SecretList {
        status: "success",
        secrets: out,
    })
}

#[utoipa::path(
    get,
    path = "/backend/{backend_id}/secret/{name}",
    tag = "backends",
    summary = "Check whether one secret is set",
    description = "\
Reports existence only. There is no endpoint that returns a stored credential, so a \
client that has forgotten what it wrote can only overwrite it.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backend/list` reports it — percent-encoded, e.g. `github.com%2Fsuper-tts%2Fopenai`.",
         example = "github.com%2Fsuper-tts%2Fopenai"),
        ("name" = String, Path, description = "The secret's name, as the backend's manifest declares it."),
    ),
    security(("session_token" = ["secrets"])),
    responses(
        (status = 200, description = "Whether a value is stored.", body = SecretConfigured),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `secrets` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such backend (`unknown_backend`) or no such secret (`unknown_secret`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_backend_secret(
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
            ok(&SecretConfigured {
                status: "success",
                name: Some(name),
                configured,
            })
        }
    }
}

#[utoipa::path(
    post,
    path = "/backend/{backend_id}/secret/{name}",
    tag = "backends",
    summary = "Store a secret",
    description = "\
Writes the credential to the system keyring, replacing whatever was there. It cannot \
be read back afterwards; only replaced or cleared.

If the stage is currently running a model from this backend, the daemon reloads it so \
the new credential is used at once — a key rotated while a model is loaded takes \
effect without the user knowing a reload was needed.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backend/list` reports it — percent-encoded, e.g. `github.com%2Fsuper-tts%2Fopenai`.",
         example = "github.com%2Fsuper-tts%2Fopenai"),
        ("name" = String, Path, description = "The secret's name, as the backend's manifest declares it."),
    ),
    request_body = SecretBody,
    security(("session_token" = ["secrets"])),
    responses(
        (status = 200, description = "Stored.", body = SecretConfigured),
        (status = 400, description = "The value was empty (`invalid_request`). Use `DELETE` to clear a secret.", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `secrets` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such backend (`unknown_backend`) or no such secret (`unknown_secret`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The keyring could not be written (`keyring_unavailable`).", body = ErrorEnvelope),
    ),
)]
async fn set_backend_secret(
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

#[utoipa::path(
    delete,
    path = "/backend/{backend_id}/secret/{name}",
    tag = "backends",
    summary = "Clear a secret",
    description = "\
Removes the stored credential from the keyring, returning the secret to unset. A \
backend that requires it will refuse to load until one is stored again.

This is the operation to reach for when a user disconnects an account: the value is \
gone from the keyring, not merely ignored.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backend/list` reports it — percent-encoded, e.g. `github.com%2Fsuper-tts%2Fopenai`.",
         example = "github.com%2Fsuper-tts%2Fopenai"),
        ("name" = String, Path, description = "The secret's name, as the backend's manifest declares it."),
    ),
    security(("session_token" = ["secrets"])),
    responses(
        (status = 200, description = "Cleared.", body = SecretConfigured),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `secrets` scope.", body = ErrorEnvelope),
        (status = 404, description = "No such backend (`unknown_backend`) or no such secret (`unknown_secret`).", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
        (status = 503, description = "The keyring could not be written (`keyring_unavailable`).", body = ErrorEnvelope),
    ),
)]
async fn delete_backend_secret(
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
    resp: &super_tts_shared::models::protocol::DaemonResponse,
    configured: bool,
) -> Response {
    if resp.status == "success" {
        ok(&SecretConfigured {
            status: "success",
            name: None,
            configured,
        })
    } else {
        json_error_msg(
            StatusCode::SERVICE_UNAVAILABLE,
            "keyring_unavailable",
            resp.message.as_deref().unwrap_or("keyring is unavailable"),
        )
    }
}
