// SPDX-License-Identifier: GPL-3.0-only
//! Language-settings HTTP smoke test: `/v1/language` + `/v1/active_model/language`
//!
//! Three test cases:
//! 1. Global round-trip: GET → null; POST `es-MX` → 200 + language; GET → `es-MX`; DELETE → null.
//! 2. Idle per-model: no model loaded → `GET /v1/active_model/language` → 409 (`not_ready`).
//! 3. Scope denial: a `status`-scoped token → GET/POST/GET → 403 on both language endpoints.
//!
//! Uses `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_STT_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.
//!
//! The fixture backend (`fixture-openai/backend.toml`) is seeded so the daemon
//! discovers it on startup. Its model is multilingual and secret-gated, so the
//! daemon comes up idle (no model auto-loaded).

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

/// Seed a multilingual fixture backend into `<data_home>/super-stt/backends/fixture-openai/`.
/// The model is multilingual and requires a secret, so the daemon starts idle.
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
provider = "openai"
primary_language = "en"
multilingual = true
supported_languages = ["en", "es", "es-MX", "fr", "de"]
supported_devices = ["none"]
"#;
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    // Placeholder entrypoint so the manifest parser does not reject the missing file.
    std::fs::write(backend_dir.join("openai.wasm"), b"").expect("write placeholder entrypoint");
}

async fn start_daemon(scopes: &[&str]) -> (DaemonGuard, PathBuf, String) {
    let unique = format!("stt-language-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));

    std::fs::create_dir_all(&config_home).expect("create test config dir");
    std::fs::create_dir_all(&data_home).expect("create test data dir");

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
            && http_client::auth_request(http_socket.clone(), "language-smoke-probe", &["status"])
                .await
                .is_ok()
        {
            // Mint the token with the caller-specified scopes.
            let auth = http_client::auth_request(http_socket.clone(), "language-smoke", scopes)
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

/// Case 1 — GET → null; POST `es-MX` → 200 + language; GET → `es-MX`; DELETE → null.
#[tokio::test]
async fn global_language_round_trips() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    // GET → null initially
    let (st, body) = get(&sock, "/language", &token).await;
    assert_eq!(st, StatusCode::OK, "initial GET: {body}");
    assert_eq!(
        body["language"],
        serde_json::Value::Null,
        "language must be null before any SET: {body}"
    );

    // POST es-MX
    let (st, body) = post_req(
        &sock,
        "/language",
        &token,
        serde_json::json!({ "language": "es-MX" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "POST language: {body}");
    assert_eq!(
        body["language"], "es-MX",
        "POST must echo set value: {body}"
    );

    // GET → es-MX persisted
    let (st, body) = get(&sock, "/language", &token).await;
    assert_eq!(st, StatusCode::OK, "GET after POST: {body}");
    assert_eq!(
        body["language"], "es-MX",
        "GET must return persisted value: {body}"
    );

    // DELETE → null
    let (st, body) = delete_req(&sock, "/language", &token).await;
    assert_eq!(st, StatusCode::OK, "DELETE language: {body}");
    assert_eq!(
        body["language"],
        serde_json::Value::Null,
        "DELETE must clear language to null: {body}"
    );
}

/// Case 2 — No model loaded → `GET /active_model/language` → 409.
#[tokio::test]
async fn active_model_language_is_not_ready_when_idle() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (st, body) = get(&sock, "/active_model/language", &token).await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "per-model language must be 409 when idle: {body}"
    );
}

/// Case 3 — A `status`-scoped token must be denied (403) on both language endpoints.
#[tokio::test]
async fn language_endpoints_require_settings_scope() {
    let (_guard, sock, token) = start_daemon(&["status"]).await;

    for (method, path) in [
        (Method::GET, "/language"),
        (Method::POST, "/language"),
        (Method::GET, "/active_model/language"),
    ] {
        let body = (method == Method::POST).then(|| serde_json::json!({ "language": "es" }));
        let (st, resp) = raw_request(&sock, method.clone(), path, &token, body).await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "{method} /v1{path} should be 403 for status-scoped token: {resp}"
        );
    }
}
