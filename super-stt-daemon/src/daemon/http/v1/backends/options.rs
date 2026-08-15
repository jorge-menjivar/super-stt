// SPDX-License-Identifier: GPL-3.0-only
use super::{decode_source, find_backend, json_error, ok};
use crate::daemon::http::internal::helpers::dispatch::dispatch_command;
use crate::daemon::http::state::AppState;
use crate::stt_models::backends::DiscoveredBackend;
use crate::stt_models::backends::manifest::OptionType;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/backends/{source}/options/list", get(list))
        .route(
            "/backends/{source}/options/{name}",
            get(get_one).post(set).delete(delete_option),
        )
}

#[derive(Deserialize)]
struct OptionBody {
    #[serde(default)]
    value: String,
}

/// Compute the effective value + metadata for a single option.
///
/// Returns `None` if `name` is not declared by the backend.
fn effective(
    b: &DiscoveredBackend,
    cfg_value: Option<&str>,
    name: &str,
) -> Option<serde_json::Value> {
    let opt = b.options.iter().find(|o| o.name == name)?;
    let default = opt.default.as_ref().map(ToString::to_string);
    let value = cfg_value.map(str::to_string).or_else(|| default.clone());
    Some(serde_json::json!({
        "name":     opt.name,
        "label":    opt.label.clone().unwrap_or_else(|| opt.name.clone()),
        "type":     opt.r#type.map_or("string", OptionType::as_str),
        "default":  default,
        "required": opt.required,
        "value":    value,
    }))
}

/// `GET /backends/{source}/options/list` — all declared options with effective values.
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
    ok(&serde_json::json!({ "status": "success", "options": out }))
}

/// `GET /backends/{source}/options/{name}` — effective value for one option.
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
        Some(v) => ok(&serde_json::json!({
            "status":  "success",
            "name":    name,
            "value":   v["value"],
            "default": v["default"],
        })),
        None => json_error(StatusCode::NOT_FOUND, "unknown_option"),
    }
}

/// `POST /backends/{source}/options/{name}` — set an override via `set_backend_option`.
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

/// `DELETE /backends/{source}/options/{name}` — clear override (reverts to default).
///
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
