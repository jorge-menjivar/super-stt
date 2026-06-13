// SPDX-License-Identifier: GPL-3.0-only
//! Settings-scope HTTP smoke test for the **backend-management** surface:
//! `/backends`, `/active_backend`, and `/gpu_info`. These endpoints drive
//! the settings app's per-backend configuration section and were the
//! largest untested slice of the settings scope (`http_smoke_settings.rs`
//! covers the model/device/theme settings but not backend selection).
//!
//! Hermetic: an isolated `XDG_DATA_HOME` means the daemon discovers no
//! installed backends, so it comes up idle. That's exactly the state these
//! assertions pin — empty catalog, null active backend, and the documented
//! error paths for selecting / uninstalling something that isn't there:
//!
//! - `GET    /backends`           → `{ status: success, backends: [] }`
//! - `GET    /active_backend`     → `{ status: success, active_backend: null }`
//! - `POST   /active_backend`     → `400 invalid_backend` for an unknown source
//! - `DELETE /active_backend`     → `{ status: success }` (idempotent when idle)
//! - `GET    /gpu_info`           → `{ status: success, gpu_info: [...] }`
//! - `DELETE /backends/{source}`  → `404 not_found` for an unknown backend
//! - scope enforcement            → a `client`-scope token gets `403 scope_denied`
//!
//! Uses `SUPER_STT_AUTO_APPROVE=1` (no GUI) and `SUPER_STT_KEYRING_MOCK=1`
//! (in-memory keyring), so it's part of the default `cargo test` flow.

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

/// Monotonic per-call counter so concurrent tests in the same test binary
/// get unique paths.
fn next_test_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static UNIQ: AtomicU64 = AtomicU64::new(0);
    UNIQ.fetch_add(1, Ordering::Relaxed)
}

async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let unique = format!("stt-backends-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");
    // Empty, isolated data dir → no backends discovered; daemon comes up idle.
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&data_home).expect("create test data dir");

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
            && http_client::auth_request(http_socket.clone(), "backends-smoke", &["status"])
                .await
                .is_ok()
        {
            return (
                DaemonGuard {
                    child,
                    cleanup_paths: vec![http_socket.clone(), config_home, data_home],
                },
                http_socket,
            );
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
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

    // --- GET /backends: hermetic daemon → empty catalog ---
    let (s, body) = get(&sock, "/backends", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /backends: {body}");
    assert_eq!(body["status"], "success");
    let backends = body["backends"]
        .as_array()
        .unwrap_or_else(|| panic!("backends should be an array: {body}"));
    assert!(
        backends.is_empty(),
        "no backends are installed in the isolated data dir, got: {body}"
    );

    // --- GET /active_backend: idle daemon → null ---
    let (s, body) = get(&sock, "/active_backend", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /active_backend: {body}");
    assert_eq!(body["status"], "success");
    assert!(
        body["active_backend"].is_null(),
        "no backend should be selected on a fresh daemon: {body}"
    );

    // --- POST /active_backend with an unknown source → 400 invalid_backend ---
    let (s, body) = post(
        &sock,
        "/active_backend",
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
    let (_, after) = get(&sock, "/active_backend", &settings_token).await;
    assert!(
        after["active_backend"].is_null(),
        "a rejected selection must leave the daemon idle: {after}"
    );

    // --- DELETE /active_backend: idempotent when already idle ---
    let (s, body) = delete(&sock, "/active_backend", &settings_token).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "clearing an already-idle backend should be 200: {body}"
    );
    assert_eq!(body["status"], "success");

    // --- GET /gpu_info: always succeeds; array is empty when no GPU/driver ---
    let (s, body) = get(&sock, "/gpu_info", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /gpu_info: {body}");
    assert_eq!(body["status"], "success");
    assert!(
        body["gpu_info"].is_array(),
        "gpu_info must be an array (possibly empty): {body}"
    );

    // --- DELETE /backends/{source}: unknown backend → 404 not_found ---
    // The source is a single path segment, so its slashes are percent-encoded.
    let (s, body) = delete(
        &sock,
        "/backends/github.com%2Fdoes-not%2Fexist",
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

    let client_token = http_client::auth_request(
        sock.clone(),
        "backends client smoke",
        &["transcribe", "status"],
    )
    .await
    .expect("auth_request client")
    .session_token;

    for (method, path) in [
        (Method::GET, "/backends"),
        (Method::GET, "/active_backend"),
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
        "/active_backend",
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
