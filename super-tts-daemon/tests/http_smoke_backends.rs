// SPDX-License-Identifier: GPL-3.0-only
//! Settings-scope HTTP smoke test for the **backend-management** surface:
//! `/backend/list`, `/pipeline/1`, and `/gpu_info`. These endpoints drive
//! the settings app's per-backend configuration section and were the
//! largest untested slice of the settings scope (`http_smoke_settings.rs`
//! covers the model/device/theme settings but not backend selection).
//!
//! Hermetic: an isolated `XDG_DATA_HOME` means the daemon discovers no
//! installed backends, so it comes up idle. That's exactly the state these
//! assertions pin — empty catalog, an empty synthesis stage, and the documented
//! error paths for selecting / uninstalling something that isn't there:
//!
//! - `GET    /backend/list`       → `{ status: success, backends: [] }`
//! - `GET    /pipeline/1`         → `{ status: success, stage: { source: null } }`
//! - `POST   /pipeline/1`         → `400 invalid_backend` for an unknown source
//! - `DELETE /pipeline/1`         → `{ status: success }` (idempotent when idle)
//! - `GET    /gpu_info`           → `{ status: success, gpu_info: [...] }`
//! - `DELETE /backend/{backend_id}` → `404 not_found` for an unknown backend
//! - scope enforcement            → a `client`-scope token gets `403 scope_denied`
//!
//! Uses `SUPER_TTS_AUTO_APPROVE=1` (no GUI) and `SUPER_TTS_KEYRING_MOCK=1`
//! (in-memory keyring), so it's part of the default `cargo test` flow.

mod common;

use common::{Method, StatusCode, TestDaemon};

use std::path::PathBuf;
use super_tts_shared::daemon::http_client;

async fn start_daemon() -> (TestDaemon, PathBuf) {
    let daemon = common::daemon("backends").start().await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

/// Open a fresh HTTP/1 connection over the Unix socket and issue `method`
/// to `/v1{path}` with the given bearer token and optional JSON body.
/// Returns `(status, parsed JSON body)`.
async fn raw_request(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    common::request(socket_path, method, path, Some(token), body.as_ref()).await
}

async fn get(p: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::GET, path, token, None).await
}

async fn post(
    p: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::POST, path, token, Some(body)).await
}

async fn delete(p: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::DELETE, path, token, None).await
}

#[tokio::test]
async fn backend_management_endpoints() {
    let (_guard, sock) = start_daemon().await;

    let settings_token = http_client::auth_request(sock.clone(), "backends smoke", &["settings"])
        .await
        .expect("auth_request settings")
        .session_token;

    // --- GET /backend/list: hermetic daemon → empty catalog ---
    let (s, body) = get(&sock, "/backend/list", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /backend/list: {body}");
    assert_eq!(body["status"], "success");
    let backends = body["backends"]
        .as_array()
        .unwrap_or_else(|| panic!("backends should be an array: {body}"));
    assert!(
        backends.is_empty(),
        "no backends are installed in the isolated data dir, got: {body}"
    );

    // --- GET /pipeline/1: idle daemon → an empty stage ---
    // The stage exists whether or not it is filled: `source` is an explicit
    // `null`, not an absent key, so a card can read it without guarding.
    let (s, body) = get(&sock, "/pipeline/1", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /pipeline/1: {body}");
    assert_eq!(body["status"], "success");
    assert_eq!(body["stage"]["stage"], 1, "{body}");
    assert_eq!(body["stage"]["role"], "synthesis", "{body}");
    assert_eq!(body["stage"]["enabled"], false, "{body}");
    assert!(
        body["stage"]["source"].is_null(),
        "no backend should be selected on a fresh daemon: {body}"
    );

    // --- POST /pipeline/1 with an unknown source → 400 invalid_backend ---
    let (s, body) = post(
        &sock,
        "/pipeline/1",
        &settings_token,
        serde_json::json!({ "source": "github.com/does-not/exist" }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "selecting an uninstalled backend must be 400: {body}"
    );
    assert_eq!(body["status"], "error");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("No installed backend")),
        "error should explain no backend serves that source: {body}"
    );

    // The failed selection must not have changed state.
    let (_, after) = get(&sock, "/pipeline/1", &settings_token).await;
    assert!(
        after["stage"]["source"].is_null(),
        "a rejected selection must leave the daemon idle: {after}"
    );

    // --- DELETE /pipeline/1: idempotent when already idle ---
    let (s, body) = delete(&sock, "/pipeline/1", &settings_token).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "clearing an already-idle backend should be 200: {body}"
    );
    assert_eq!(body["status"], "success");

    // --- GET /pipeline: the same stage, in the ordered report ---
    // Narrowed from one source, so the single-stage read and the whole-pipeline
    // read can never disagree about what fills a position.
    let (s, body) = get(&sock, "/pipeline", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /pipeline: {body}");
    assert_eq!(body["status"], "success");
    let stages = body["pipeline"]
        .as_array()
        .unwrap_or_else(|| panic!("pipeline should be an array: {body}"));
    assert_eq!(stages.len(), 1, "this build has one stage: {body}");
    assert_eq!(stages[0]["stage"], 1, "{body}");
    assert_eq!(stages[0]["role"], "synthesis", "{body}");

    // --- GET /pipeline/1/backend/list: what POST /pipeline/1 would accept ---
    let (s, body) = get(&sock, "/pipeline/1/backend/list", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /pipeline/1/backend/list: {body}");
    assert_eq!(body["status"], "success");
    assert_eq!(
        body["backends"].as_array().map(Vec::len),
        Some(0),
        "nothing installed, so nothing can fill the stage: {body}"
    );

    // --- GET /pipeline/1/model/list: empty reads as "choose a backend first" ---
    let (s, body) = get(&sock, "/pipeline/1/model/list", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /pipeline/1/model/list: {body}");
    assert_eq!(body["status"], "success");
    assert_eq!(
        body["available_models"].as_array().map(Vec::len),
        Some(0),
        "an empty stage runs no models: {body}"
    );

    // --- GET /gpu_info: always succeeds; array is empty when no GPU/driver ---
    let (s, body) = get(&sock, "/gpu_info", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /gpu_info: {body}");
    assert_eq!(body["status"], "success");
    assert!(
        body["gpu_info"].is_array(),
        "gpu_info must be an array (possibly empty): {body}"
    );

    // --- DELETE /backend/{backend_id}: unknown backend → 404 not_found ---
    // The id is the backend's `source` in a single path segment, so its
    // slashes are percent-encoded.
    let (s, body) = delete(
        &sock,
        "/backend/github.com%2Fdoes-not%2Fexist",
        &settings_token,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "uninstalling an unknown backend must be 404: {body}"
    );
    assert_eq!(body["error"], "not_found", "got: {body}");
}

/// A `client`-scope token (no `settings`) must be rejected on every
/// backend-management endpoint with `403 scope_denied` — these are
/// settings-scope routes.
#[tokio::test]
async fn backend_endpoints_reject_client_scope() {
    let (_guard, sock) = start_daemon().await;

    let client_token =
        http_client::auth_request(sock.clone(), "backends client smoke", &["speak", "status"])
            .await
            .expect("auth_request client")
            .session_token;

    for (method, path) in [
        (Method::GET, "/backend/list"),
        (Method::GET, "/pipeline"),
        (Method::GET, "/pipeline/1"),
        (Method::GET, "/pipeline/1/backend/list"),
        (Method::GET, "/pipeline/1/model/list"),
        (Method::GET, "/gpu_info"),
    ] {
        let (s, body) = raw_request(&sock, method.clone(), path, &client_token, None).await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "client token must be 403 on {method} {path}: {body}"
        );
        assert_eq!(
            body["message"], "scope_denied",
            "on {method} {path}: {body}"
        );
    }

    // A settings POST must be rejected too.
    let (s, body) = post(
        &sock,
        "/pipeline/1",
        &client_token,
        serde_json::json!({ "source": "github.com/x/y" }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "client token must be 403 on POST: {body}"
    );
    assert_eq!(body["message"], "scope_denied");
}
