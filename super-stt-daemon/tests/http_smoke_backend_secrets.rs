// SPDX-License-Identifier: GPL-3.0-only
//! Secrets-scope HTTP smoke test: `/backend/{source}/secret/...`
//!
//! Three test cases:
//! 1. Round-trip: set → listed-configured → delete → unset.
//! 2. Scope denial: a `settings`-only token is rejected on secret endpoints.
//! 3. Undeclared secret: POST to an unknown `{name}` → 404 `unknown_secret`.
//!
//! Uses `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_STT_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.
//!
//! The fixture backend (`fixture-openai/backend.toml`) is written into the
//! isolated `XDG_DATA_HOME/super-stt/backends/` tree so the daemon discovers
//! it on startup. It declares one secret (`openai_api_key`) and one model.

mod common;

use common::{Method, StatusCode, TestDaemon};

use std::path::{Path, PathBuf};

/// URL-encoded `{source}` path segment for the fixture backend.
/// The raw source id is `github.com/super-stt/openai`; slashes become `%2F`.
const FIXTURE_SOURCE_ENC: &str = "github.com%2Fsuper-stt%2Fopenai";

/// Seed the fixture backend into `<data_home>/super-stt/backends/fixture-openai/`.
/// The manifest declares `openai_api_key` as a required secret and one model.
fn seed_fixture_backend(data_home: &Path) {
    let backend_dir = data_home
        .join("super-stt")
        .join("backends")
        .join("fixture-openai");
    std::fs::create_dir_all(&backend_dir).expect("create fixture backend dir");

    // Write a minimal backend.toml that the daemon can discover.
    // The entrypoint file must exist (daemon validates it at load time for
    // subprocess kind; for wasm it's only needed when actually running the
    // backend — discovery does not exec it). We create a placeholder.
    let toml = r#"[backend]
source = "github.com/super-stt/openai"
name = "Fixture OpenAI"
version = "1.0.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v1"
description = "Test backend."
license = "Apache-2.0"

[network]
allowed_hosts = ["api.openai.com"]

[[secrets]]
name = "openai_api_key"
label = "OpenAI API key"
description = "Your OpenAI API key."
required = true

[[models]]
name = "whisper-1"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    // Create a placeholder entrypoint so the manifest can reference it.
    std::fs::write(backend_dir.join("openai.wasm"), b"").expect("write placeholder entrypoint");
}

async fn start_daemon(scopes: &[&str]) -> (TestDaemon, PathBuf, String) {
    let daemon = common::daemon("secrets");
    seed_fixture_backend(&daemon.home().data);
    let daemon = daemon.start().await;
    let token = daemon.token("secrets-smoke", scopes).await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket, token)
}

/// Issue an HTTP request and return `(status, json_body)`.
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

async fn delete_req(p: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::DELETE, path, token, None).await
}

#[tokio::test]
async fn secret_set_then_listed_configured_then_cleared() {
    let (_guard, sock, token) = start_daemon(&["secrets"]).await;

    let sec_path = format!("/backend/{FIXTURE_SOURCE_ENC}/secret/openai_api_key");
    let list_path = format!("/backend/{FIXTURE_SOURCE_ENC}/secret/list");

    // Initially not configured.
    let (s, body) = get(&sock, &sec_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET secret before set: {body}");
    assert_eq!(
        body["configured"], false,
        "should start unconfigured: {body}"
    );

    // Set the secret.
    let (s, body) = post(
        &sock,
        &sec_path,
        &token,
        serde_json::json!({ "value": "sk-abc" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST secret: {body}");
    assert_eq!(body["configured"], true, "configured after set: {body}");

    // List shows it configured, never reveals the value.
    let (s, body) = get(&sock, &list_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET secrets: {body}");
    assert_eq!(body["status"], "success", "list status: {body}");
    let secrets = body["secrets"].as_array().expect("secrets array");
    assert_eq!(secrets.len(), 1, "one declared secret: {body}");
    let s0 = &secrets[0];
    assert_eq!(s0["name"], "openai_api_key", "secret name: {body}");
    assert_eq!(s0["configured"], true, "configured flag: {body}");
    assert!(
        s0.get("value").is_none(),
        "secret value must never be returned: {body}"
    );

    // Delete resets to unset.
    let (s, body) = delete_req(&sock, &sec_path, &token).await;
    assert_eq!(s, StatusCode::OK, "DELETE secret: {body}");
    assert_eq!(
        body["configured"], false,
        "unconfigured after delete: {body}"
    );
}

#[tokio::test]
async fn secret_endpoints_require_the_secrets_scope() {
    // Only `settings` scope — no `secrets`.
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let secret_path = format!("/backend/{FIXTURE_SOURCE_ENC}/secret/openai_api_key");
    let list_path = format!("/backend/{FIXTURE_SOURCE_ENC}/secret/list");

    let (s, body) = post(
        &sock,
        &secret_path,
        &token,
        serde_json::json!({ "value": "x" }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "settings-only token must be 403 on secret POST: {body}"
    );
    assert_eq!(body["message"], "scope_denied", "error code: {body}");

    // The scope guard is a router-layer middleware — it must fire on GET too.
    let (s, body) = get(&sock, &secret_path, &token).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "settings-only token must be 403 on secret GET: {body}"
    );
    assert_eq!(body["message"], "scope_denied", "error code: {body}");

    let (s, body) = get(&sock, &list_path, &token).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "settings-only token must be 403 on the secrets list GET: {body}"
    );
    assert_eq!(body["message"], "scope_denied", "error code: {body}");

    // And on DELETE.
    let (s, body) = delete_req(&sock, &secret_path, &token).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "settings-only token must be 403 on secret DELETE: {body}"
    );
    assert_eq!(body["message"], "scope_denied", "error code: {body}");
}

#[tokio::test]
async fn undeclared_secret_is_404() {
    let (_guard, sock, token) = start_daemon(&["secrets"]).await;

    let path = format!("/backend/{FIXTURE_SOURCE_ENC}/secret/not_a_real_secret");

    let (s, body) = post(&sock, &path, &token, serde_json::json!({ "value": "x" })).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "undeclared secret must be 404: {body}"
    );
    assert_eq!(
        body["message"], "unknown_secret",
        "error code for undeclared secret: {body}"
    );
}
