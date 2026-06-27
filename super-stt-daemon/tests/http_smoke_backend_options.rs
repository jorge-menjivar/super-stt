// SPDX-License-Identifier: GPL-3.0-only
//! Options-scope HTTP smoke test: `/backends/{source}/options/...`
//!
//! Three test cases:
//! 1. Round-trip: read default → set override → reset to default.
//! 2. Unknown option: GET/POST to a `{name}` not declared → 404 `unknown_option`.
//! 3. Unknown backend: GET on a source not installed → 404 `unknown_backend`.
//!
//! Uses `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_STT_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.
//!
//! The fixture backend (`fixture-openai/backend.toml`) is written into the
//! isolated `XDG_DATA_HOME/super-stt/backends/` tree so the daemon discovers
//! it on startup. It declares one option (`base_url`) with default
//! `"https://api.openai.com"`.

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use hyper::{Method, Request, StatusCode};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");

/// URL-encoded `{source}` path segment for the fixture backend.
/// The raw source id is `github.com/super-stt/openai`; slashes become `%2F`.
const FIXTURE_SOURCE_ENC: &str = "github.com%2Fsuper-stt%2Fopenai";

struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for p in &self.cleanup_paths {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

fn next_test_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static UNIQ: AtomicU64 = AtomicU64::new(0);
    UNIQ.fetch_add(1, Ordering::Relaxed)
}

/// Seed the fixture backend into `<data_home>/super-stt/backends/fixture-openai/`.
/// The manifest declares `base_url` as an option with a default value.
fn seed_fixture_backend(data_home: &Path) {
    let backend_dir = data_home
        .join("super-stt")
        .join("backends")
        .join("fixture-openai");
    std::fs::create_dir_all(&backend_dir).expect("create fixture backend dir");

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

[[options]]
name = "base_url"
label = "Base URL"
description = "Override the OpenAI API base URL."
type = "string"
default = "https://api.openai.com"

[[models]]
name = "whisper-1"
provider = "openai"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    // Create a placeholder entrypoint so the manifest can reference it.
    std::fs::write(backend_dir.join("openai.wasm"), b"").expect("write placeholder entrypoint");
}

async fn start_daemon(scopes: &[&str]) -> (DaemonGuard, PathBuf, String) {
    let unique = format!("stt-options-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));

    std::fs::create_dir_all(&config_home).expect("create test config dir");
    std::fs::create_dir_all(&data_home).expect("create test data dir");

    // Seed the fixture backend so the daemon has something with declared options.
    seed_fixture_backend(&data_home);

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .arg("--device")
        .arg("cpu")
        .arg("--audio-theme")
        .arg("silent")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "options-smoke-probe", &["status"])
                .await
                .is_ok()
        {
            // Mint the token with the caller-specified scopes.
            let auth = http_client::auth_request(http_socket.clone(), "options-smoke", scopes)
                .await
                .expect("auth_request for test scopes");
            let token = auth.session_token;
            return (
                DaemonGuard {
                    child,
                    cleanup_paths: vec![http_socket.clone(), config_home, data_home],
                },
                http_socket,
                token,
            );
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

/// Issue an HTTP request and return `(status, json_body)`.
async fn raw_request(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body_bytes = body
        .map(|b| serde_json::to_vec(&b).expect("encode body"))
        .unwrap_or_default();

    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://stt.local/v1{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"));
    if !body_bytes.is_empty() {
        builder = builder
            .header("content-type", "application/json")
            .header("content-length", body_bytes.len().to_string());
    }
    let req = builder
        .body(Full::new(Bytes::from(body_bytes)))
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn get(p: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::GET, path, token, None).await
}

async fn post_req(
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

/// Round-trip: default → override → reset to default.
#[tokio::test]
async fn option_set_get_and_reset_to_default() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backends/{FIXTURE_SOURCE_ENC}/options/base_url");

    // Default before any override.
    let (s, body) = get(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET option before set: {body}");
    assert_eq!(
        body["value"], "https://api.openai.com",
        "should start at manifest default: {body}"
    );

    // Set override.
    let (s, body) = post_req(
        &sock,
        &opt_path,
        &token,
        serde_json::json!({ "value": "https://gw.example.com" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST option: {body}");
    assert_eq!(
        body["value"], "https://gw.example.com",
        "value should reflect override: {body}"
    );

    // Reset to default (DELETE clears the override).
    let (s, body) = delete_req(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "DELETE option: {body}");
    assert_eq!(
        body["value"], "https://api.openai.com",
        "value should revert to manifest default after DELETE: {body}"
    );
}

/// Listing all options for the backend.
#[tokio::test]
async fn option_list_returns_declared_options() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let list_path = format!("/backends/{FIXTURE_SOURCE_ENC}/options/list");

    let (s, body) = get(&sock, &list_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET options/list: {body}");
    assert_eq!(body["status"], "success", "list status: {body}");
    let options = body["options"].as_array().expect("options array");
    assert_eq!(options.len(), 1, "one declared option: {body}");
    let o0 = &options[0];
    assert_eq!(o0["name"], "base_url", "option name: {body}");
    assert_eq!(
        o0["value"], "https://api.openai.com",
        "default value in list: {body}"
    );
}

/// GET on an undeclared option name returns 404 `unknown_option`.
#[tokio::test]
async fn undeclared_option_is_404() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let path = format!("/backends/{FIXTURE_SOURCE_ENC}/options/not_a_real_option");

    let (s, body) = get(&sock, &path, &token).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "undeclared option must be 404: {body}"
    );
    assert_eq!(
        body["message"], "unknown_option",
        "error code for undeclared option: {body}"
    );
}

/// POST with an empty `value` returns 400 `invalid_request`.
#[tokio::test]
async fn set_empty_value_is_400() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backends/{FIXTURE_SOURCE_ENC}/options/base_url");

    let (s, body) = post_req(&sock, &opt_path, &token, serde_json::json!({ "value": "" })).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "empty value must be 400: {body}"
    );
    assert_eq!(
        body["message"], "invalid_request",
        "error code for empty value: {body}"
    );
}

/// GET on an unknown backend returns 404 `unknown_backend`.
#[tokio::test]
async fn unknown_backend_is_404() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let path = "/backends/github.com%2Fnot%2Finstalled/options/base_url";

    let (s, body) = get(&sock, path, &token).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "unknown backend must be 404: {body}"
    );
    assert_eq!(
        body["message"], "unknown_backend",
        "error code for unknown backend: {body}"
    );
}
