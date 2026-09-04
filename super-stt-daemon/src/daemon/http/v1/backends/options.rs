// SPDX-License-Identifier: GPL-3.0-only
use super::{decode_source, find_backend, json_error, ok};
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use crate::daemon::http::wire::{ErrorEnvelope, ReasonEnvelope};
use crate::stt_models::backends::DiscoveredBackend;
use crate::stt_models::backends::manifest::OptionType;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(get_one, set, delete_option))
}

/// The value to store for an option.
#[derive(Deserialize, ToSchema)]
struct OptionBody {
    /// The new value. Empty is refused — clear an override with `DELETE`
    /// instead, which is the operation that reverts to the manifest default.
    #[serde(default)]
    value: String,
}

/// One option a backend declares, with the value in effect.
#[derive(Serialize, ToSchema)]
struct BackendOptionValue {
    /// The option's identifier, as the backend's manifest declares it.
    name: String,
    /// Human-readable label for a settings UI; falls back to `name`.
    label: String,
    /// The declared type — `string`, `number`, and so on.
    #[serde(rename = "type")]
    kind: String,
    /// The manifest's default, or `null` when it declares none.
    default: Option<String>,
    /// Whether the backend refuses to load without a value.
    required: bool,
    /// What is actually in effect: the user's override if set, otherwise the
    /// default.
    value: Option<String>,
}

/// Every option a backend declares.
#[derive(Serialize, ToSchema)]
struct BackendOptions {
    #[schema(example = "success")]
    status: &'static str,
    options: Vec<BackendOptionValue>,
}

/// One option's effective value.
#[derive(Serialize, ToSchema)]
struct OptionValue {
    #[schema(example = "success")]
    status: &'static str,
    name: String,
    /// What is in effect now.
    value: Option<String>,
    /// The manifest default, so a UI can show what clearing would revert to.
    default: Option<String>,
}

/// Compute the effective value + metadata for a single option.
///
/// Returns `None` if `name` is not declared by the backend.
fn effective(
    b: &DiscoveredBackend,
    cfg_value: Option<&str>,
    name: &str,
) -> Option<BackendOptionValue> {
    let opt = b.options.iter().find(|o| o.name == name)?;
    let default = opt.default.as_ref().map(ToString::to_string);
    let value = cfg_value.map(str::to_string).or_else(|| default.clone());
    Some(BackendOptionValue {
        name: opt.name.clone(),
        label: opt.label.clone().unwrap_or_else(|| opt.name.clone()),
        kind: opt.r#type.map_or("string", OptionType::as_str).to_string(),
        default,
        required: opt.required,
        value,
    })
}

#[utoipa::path(
    get,
    path = "/backends/{backend_id}/options/list",
    tag = "backends",
    summary = "List a backend's options",
    description = "\
Every option this backend declares, each with its label, type, default, and the \
value actually in effect. This is what a settings UI renders a form from.

Options are the backend's own configuration — an endpoint URL, a model parameter — \
declared in its manifest. Credentials are not options; those are secrets, at \
`/backends/{backend_id}/secrets/list`.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backends` reports it — percent-encoded, e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The backend's options.", body = BackendOptions),
        (status = 404, description = "No installed backend has that `source` (`unknown_backend`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn list(State(s): State<AppState>, Path(source): Path<String>) -> Response {
    let source = decode_source(&source);
    let Some(b) = find_backend(&s, &source).await else {
        return json_error(StatusCode::NOT_FOUND, "unknown_backend");
    };
    let cfg = s.daemon.config.read().await;
    let out: Vec<_> = b
        .options
        .iter()
        .filter_map(|o| effective(&b, cfg.backend_option(&source, &o.name), &o.name))
        .collect();
    ok(&BackendOptions {
        status: "success",
        options: out,
    })
}

#[utoipa::path(
    get,
    path = "/backends/{backend_id}/options/{name}",
    tag = "backends",
    summary = "Read one option's effective value",
    description = "\
The value in effect for a single option, alongside the manifest default so a UI can \
show what clearing it would revert to.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backends` reports it — percent-encoded, e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
        ("name" = String, Path, description = "The option's name, as the backend's manifest declares it."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "The option's effective value.", body = OptionValue),
        (status = 404, description = "No such backend (`unknown_backend`) or no such option (`unknown_option`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn get_one(
    State(s): State<AppState>,
    Path((source, name)): Path<(String, String)>,
) -> Response {
    get_one_inner(s, decode_source(&source), name).await
}

async fn get_one_inner(s: AppState, source: String, name: String) -> Response {
    let Some(b) = find_backend(&s, &source).await else {
        return json_error(StatusCode::NOT_FOUND, "unknown_backend");
    };
    let cfg = s.daemon.config.read().await;
    match effective(&b, cfg.backend_option(&source, &name), &name) {
        Some(v) => ok(&OptionValue {
            status: "success",
            name,
            value: v.value,
            default: v.default,
        }),
        None => json_error(StatusCode::NOT_FOUND, "unknown_option"),
    }
}

#[utoipa::path(
    post,
    path = "/backends/{backend_id}/options/{name}",
    tag = "backends",
    summary = "Override an option",
    description = "\
Stores a value for this option, overriding the manifest default. Answers with the \
option's new effective value, so a UI can render the result without a second read.

A `base_url` value is canonicalized before storage, so the field reads back the \
endpoint that will actually be dialed — the scheme in particular, since whether a \
request is encrypted should not be invisible in the field the user is looking at. \
A value that yields no host is stored as typed rather than refused; model load \
rejects it by name. Every other option is stored verbatim, whitespace included, \
because it may carry meaning the daemon does not interpret.

A loaded model does not pick this up on its own — reload the stage with \
`POST /pipeline/{stage}/model/reload`.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backends` reports it — percent-encoded, e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
        ("name" = String, Path, description = "The option's name, as the backend's manifest declares it."),
    ),
    request_body = OptionBody,
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Stored; this is the new effective value.", body = OptionValue),
        (status = 400, description = "The value was empty (`invalid_request`). Use `DELETE` to clear an override.", body = ErrorEnvelope),
        (status = 404, description = "No such backend (`unknown_backend`) or no such option (`unknown_option`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
async fn set(
    State(s): State<AppState>,
    Path((source, name)): Path<(String, String)>,
    axum::Json(body): axum::Json<OptionBody>,
) -> Response {
    let source = decode_source(&source);
    // `base_url` is stored canonical — the same rewrite model load applies, run
    // here so the settings field reads back the endpoint that will actually be
    // dialed. The scheme is the reason it matters: a value naming none is read
    // by its host, and whether the request is encrypted is not something to
    // leave invisible in the field the user is looking at. Other options keep
    // the value verbatim, whitespace included: it may carry meaning in one the
    // daemon does not interpret.
    //
    // Canonicalized, not validated: a value that yields no host is stored as
    // typed rather than refused. Rejecting it here would catch garbage but not
    // the mistake that actually misleads people — a well-formed URL naming the
    // wrong port — and model load already refuses it with a message naming the
    // option. What this must not do is quietly drop it.
    let value = if name == crate::stt_models::backends::base_url::OPTION_NAME {
        canonical_base_url(&body.value)
    } else {
        body.value.clone()
    };
    let value = value.as_str();
    if value.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "invalid_request");
    }
    if let Some(r) = guard_missing(&s, &source, &name).await {
        return r;
    }
    let (code, _hdrs, body_str) = dispatch_command(
        &s.daemon,
        "set_backend_option",
        Some(serde_json::json!({ "source": source, "name": name, "value": value })),
    )
    .await;
    if code != StatusCode::OK {
        return (code, [("content-type", "application/json")], body_str).into_response();
    }
    get_one_inner(s, source, name).await
}

#[utoipa::path(
    delete,
    path = "/backends/{backend_id}/options/{name}",
    tag = "backends",
    summary = "Clear an option override",
    description = "\
Removes the stored value, reverting the option to the manifest default. Answers with \
the effective value that results, which is the default when one is declared and \
`null` when none is.",
    params(
        ("backend_id" = String, Path,
         description = "The backend's id — its `source` as `GET /backends` reports it — percent-encoded, e.g. `github.com%2Facme%2Fwhisper`.",
         example = "github.com%2Facme%2Fwhisper"),
        ("name" = String, Path, description = "The option's name, as the backend's manifest declares it."),
    ),
    security(("session_token" = ["settings"])),
    responses(
        (status = 200, description = "Cleared; this is the value now in effect.", body = OptionValue),
        (status = 404, description = "No such backend (`unknown_backend`) or no such option (`unknown_option`).", body = ErrorEnvelope),
        (status = 401, description = "Token unknown, expired, or its binary changed.", body = ReasonEnvelope),
        (status = 403, description = "The token lacks the `settings` scope.", body = ErrorEnvelope),
        (status = 429, description = "Per-client rate limit hit; back off and retry.", body = ErrorEnvelope),
    ),
)]
/// Renamed from `delete` to avoid shadowing `axum::routing::delete`.
async fn delete_option(
    State(s): State<AppState>,
    Path((source, name)): Path<(String, String)>,
) -> Response {
    let source = decode_source(&source);
    if let Some(r) = guard_missing(&s, &source, &name).await {
        return r;
    }
    // Empty value clears the override → reverts to the manifest default.
    let (code, _hdrs, body_str) = dispatch_command(
        &s.daemon,
        "set_backend_option",
        Some(serde_json::json!({ "source": source, "name": name, "value": "" })),
    )
    .await;
    if code != StatusCode::OK {
        return (code, [("content-type", "application/json")], body_str).into_response();
    }
    get_one_inner(s, source, name).await
}

/// The `base_url` form to store: canonical when the value can be read as a
/// URL, trimmed otherwise.
///
/// A value that yields no host is kept as the user typed it so model load can
/// refuse it by name; dropping it here would leave the backend on its built-in
/// endpoint, sending the user's audio and credentials to the vendor they had
/// configured their way out of.
#[cfg(feature = "wasm-backends")]
fn canonical_base_url(value: &str) -> String {
    crate::stt_models::backends::base_url::normalize(value)
        .unwrap_or_else(|| value.trim().to_string())
}

/// Without the wasm transport nothing derives an endpoint from this value, so
/// there is no canonical form to agree on — only the trim that keeps a padded
/// value from reading back padded.
#[cfg(not(feature = "wasm-backends"))]
fn canonical_base_url(value: &str) -> String {
    value.trim().to_string()
}

/// Returns an error `Response` when the backend or the named option is missing,
/// `None` when the option is present and a write can proceed.
async fn guard_missing(s: &AppState, source: &str, name: &str) -> Option<Response> {
    match find_backend(s, source).await {
        None => Some(json_error(StatusCode::NOT_FOUND, "unknown_backend")),
        Some(b) if b.options.iter().any(|o| o.name == name) => None,
        Some(_) => Some(json_error(StatusCode::NOT_FOUND, "unknown_option")),
    }
}
