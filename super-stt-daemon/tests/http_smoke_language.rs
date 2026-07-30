// SPDX-License-Identifier: GPL-3.0-only
//! Language-settings HTTP smoke test: `/v1/language` (global) +
//! `/v1/backends/{source}/models/{model}/language` (per-model).
//!
//! Test cases:
//! 1. Global round-trip: GET → null; POST `es-MX` → 200 + language; GET → `es-MX`; DELETE → null.
//! 2. Per-model round-trip (no model loaded): GET fixture model → 200 + resolution
//!    block (`multilingual: true`); POST a supported tag → 200; GET reflects it; DELETE → 200.
//! 3. Per-model 404s: unknown model under fixture source → 404; unknown source → 404.
//! 4. Per-model 400: POST an unsupported tag → 400 (`unsupported_language`).
//! 5. Scope denial: a `status`-scoped token → 403 on both the global and per-model paths.
//!
//! Uses `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_STT_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.
//!
//! The fixture backend (`fixture-openai/backend.toml`) is seeded so the daemon
//! discovers it on startup. Its model is multilingual and secret-gated, so the
//! daemon comes up idle (no model auto-loaded) — the per-model language
//! endpoint resolves against the discovered backend, so it works regardless.

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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    // Hand the child to the guard before the readiness loop: the timeout
    // panic below must still kill and reap the daemon, not leak it.
    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone(), config_home, data_home],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
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
            return (guard, http_socket, token);
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

/// The fixture backend's source, URL-percent-encoded for the path segment.
const FIXTURE_SOURCE_ENC: &str = "github.com%2Fsuper-stt%2Fopenai";

/// Per-model language path for the fixture's `whisper-1` model.
fn fixture_model_lang_path() -> String {
    format!("/backends/{FIXTURE_SOURCE_ENC}/models/whisper-1/language")
}

/// Case 2 — Per-model round-trip without a loaded model.
/// GET resolves the fixture model (multilingual: true); POST a supported tag →
/// 200 + override reflected; GET reflects it; DELETE clears back to default.
#[tokio::test]
async fn per_model_language_round_trips_without_loaded_model() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let path = fixture_model_lang_path();

    // GET resolves even though no model is loaded (resolves against discovery).
    let (st, body) = get(&sock, &path, &token).await;
    assert_eq!(st, StatusCode::OK, "initial per-model GET: {body}");
    assert_eq!(
        body["language"]["multilingual"], true,
        "fixture model is multilingual: {body}"
    );
    assert_eq!(
        body["language"]["override"],
        serde_json::Value::Null,
        "no override before any POST: {body}"
    );
    assert_eq!(
        body["language"]["primary"], "en",
        "fixture model's primary_language is en: {body}"
    );

    // POST a supported tag.
    let (st, body) = post_req(
        &sock,
        &path,
        &token,
        serde_json::json!({ "language": "es-MX" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "POST per-model language: {body}");
    assert_eq!(
        body["language"]["override"], "es-MX",
        "POST must echo the stored override: {body}"
    );
    assert_eq!(
        body["language"]["source"], "override",
        "resolution source is override after POST: {body}"
    );

    // GET reflects the persisted override.
    let (st, body) = get(&sock, &path, &token).await;
    assert_eq!(st, StatusCode::OK, "GET after POST: {body}");
    assert_eq!(
        body["language"]["override"], "es-MX",
        "GET must return the persisted override: {body}"
    );

    // DELETE clears back to default.
    let (st, body) = delete_req(&sock, &path, &token).await;
    assert_eq!(st, StatusCode::OK, "DELETE per-model language: {body}");
    assert_eq!(
        body["language"]["override"],
        serde_json::Value::Null,
        "DELETE clears the override: {body}"
    );
    assert_eq!(
        body["language"]["source"], "default",
        "resolution source is default after DELETE: {body}"
    );
}

/// Case 3 — Unknown model / unknown source under the per-model path → 404.
#[tokio::test]
async fn per_model_language_unknown_targets_are_404() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    // Unknown model under a known (fixture) source.
    let unknown_model = format!("/backends/{FIXTURE_SOURCE_ENC}/models/does-not-exist/language");
    let (st, body) = get(&sock, &unknown_model, &token).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "GET unknown model must be 404: {body}"
    );
    assert_eq!(body["message"], "unknown_model", "{body}");

    let (st, body) = post_req(
        &sock,
        &unknown_model,
        &token,
        serde_json::json!({ "language": "es" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "POST unknown model must be 404: {body}"
    );
    assert_eq!(body["message"], "unknown_model", "{body}");

    // Unknown source entirely.
    let unknown_source = "/backends/github.com%2Fno%2Fsuch/models/whisper-1/language";
    let (st, body) = get(&sock, unknown_source, &token).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "GET unknown source must be 404: {body}"
    );
    assert_eq!(body["message"], "unknown_backend", "{body}");

    let (st, body) = post_req(
        &sock,
        unknown_source,
        &token,
        serde_json::json!({ "language": "es" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "POST unknown source must be 404: {body}"
    );
    assert_eq!(body["message"], "unknown_backend", "{body}");
}

/// Case 4 — POST an unsupported tag for a known model → 400.
#[tokio::test]
async fn per_model_language_unsupported_tag_is_400() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let path = fixture_model_lang_path();

    // `ja` is not in the fixture's supported_languages (["en","es","es-MX","fr","de"]).
    let (st, body) = post_req(
        &sock,
        &path,
        &token,
        serde_json::json!({ "language": "ja" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "POST unsupported tag must be 400: {body}"
    );
    assert_eq!(body["message"], "unsupported_language", "{body}");
}

/// Case 5 — A `status`-scoped token must be denied (403) on the global and
/// per-model language paths.
#[tokio::test]
async fn language_endpoints_require_settings_scope() {
    let (_guard, sock, token) = start_daemon(&["status"]).await;
    let per_model = fixture_model_lang_path();

    for (method, path) in [
        (Method::GET, "/language".to_string()),
        (Method::POST, "/language".to_string()),
        (Method::GET, per_model.clone()),
        (Method::POST, per_model.clone()),
        (Method::DELETE, per_model),
    ] {
        let body = (method == Method::POST).then(|| serde_json::json!({ "language": "es" }));
        let (st, resp) = raw_request(&sock, method.clone(), &path, &token, body).await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "{method} /v1{path} should be 403 for status-scoped token: {resp}"
        );
    }
}
