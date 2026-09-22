// SPDX-License-Identifier: GPL-3.0-only
//! Contexts HTTP smoke test: `/context/...` and `/backend/{source}/context`.
//!
//! What is checked here is the part only a live daemon can answer: that the
//! paths resolve the way the router claims, that a write survives to the file
//! and back, and that the three per-backend states are three distinct answers
//! on the wire.
//!
//! The one that would be easy to lose is `/context/active` against
//! `/context/{id}`. Those are siblings, one literal and one a parameter, and
//! nothing in the daemon's own tests would notice the router preferring the
//! wrong one — `GET /context/active` would simply start answering with a
//! context whose id happened to be `active`, which is exactly why that id is
//! refused.
//!
//! Uses `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_STT_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.

mod common;

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use hyper::{Method, Request, StatusCode};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");

/// The fixture backend's `source`, percent-encoded for a path segment.
const FIXTURE_SOURCE_ENC: &str = "github.com%2Fsuper-stt%2Fwhisper";

const FIXTURE_MANIFEST: &str = r#"[backend]
source = "github.com/super-stt/whisper"
name = "Fixture Whisper"
version = "1.0.0"
kind = "wasm"
entrypoint = "whisper.wasm"
contract = "v1"
description = "Test backend."
license = "Apache-2.0"

[[models]]
name = "whisper-1"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;

struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        common::shutdown(&mut self.child);
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

async fn start_daemon() -> (DaemonGuard, PathBuf, String) {
    let unique = format!("stt-contexts-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));
    let cache_home = tmp.join(format!("{unique}-cache"));
    for dir in [&config_home, &data_home, &cache_home] {
        std::fs::create_dir_all(dir).expect("create test dir");
    }

    common::BackendFixture {
        dir_name: "fixture-whisper",
        manifest: FIXTURE_MANIFEST,
        entrypoint: "whisper.wasm",
        component: None,
    }
    .install(&data_home);

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone(), config_home, data_home, cache_home],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "contexts-smoke-probe", &["status"])
                .await
                .is_ok()
        {
            let auth =
                http_client::auth_request(http_socket.clone(), "contexts-smoke", &["settings"])
                    .await
                    .expect("auth_request for settings scope");
            return (guard, http_socket, auth.session_token);
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

async fn raw_request(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn get(p: &PathBuf, path: &str, token: &str) -> (StatusCode, Value) {
    raw_request(p, Method::GET, path, token, None).await
}

async fn post_req(p: &PathBuf, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
    raw_request(p, Method::POST, path, token, Some(body)).await
}

async fn delete_req(p: &PathBuf, path: &str, token: &str) -> (StatusCode, Value) {
    raw_request(p, Method::DELETE, path, token, None).await
}

/// Create, read back, replace, and delete — the whole life of a context over
/// the wire.
#[tokio::test]
async fn a_context_round_trips_through_the_api() {
    let (_guard, sock, token) = start_daemon().await;

    let (s, body) = get(&sock, "/context/list", &token).await;
    assert_eq!(s, StatusCode::OK, "GET /context/list: {body}");
    assert_eq!(body["contexts"].as_array().expect("an array").len(), 0);
    assert!(
        body["active"].is_null(),
        "nothing is active on a fresh daemon"
    );

    // A blank term is dropped rather than refused: the settings UI keeps an
    // empty row for the next one.
    let (s, body) = post_req(
        &sock,
        "/context/coding",
        &token,
        json!({
            "name": "Coding",
            "prompt": "I dictate code.",
            "vocabulary": ["main branch", "", "  rebase  "],
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /context/coding: {body}");
    assert_eq!(body["context"]["id"], "coding");
    assert_eq!(
        body["context"]["vocabulary"],
        json!(["main branch", "rebase"]),
        "blank terms are dropped and the rest trimmed: {body}"
    );

    let (s, body) = get(&sock, "/context/coding", &token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["context"]["prompt"], "I dictate code.");

    // Re-posting the same id replaces in place rather than appending.
    let (s, _) = post_req(
        &sock,
        "/context/coding",
        &token,
        json!({
            "name": "Writing code",
            "prompt": "",
            "vocabulary": [],
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, body) = get(&sock, "/context/list", &token).await;
    assert_eq!(
        body["contexts"].as_array().expect("an array").len(),
        1,
        "an upsert replaces; it does not append: {body}"
    );
    assert_eq!(body["contexts"][0]["name"], "Writing code");

    let (s, body) = delete_req(&sock, "/context/coding", &token).await;
    assert_eq!(s, StatusCode::OK, "DELETE /context/coding: {body}");
    let (s, body) = get(&sock, "/context/coding", &token).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(body["error_code"], "unknown_context", "{body}");
}

/// `/context/active` is a literal sibling of `/context/{id}`. If the router
/// ever preferred the parameter, this is the test that says so.
#[tokio::test]
async fn the_active_selection_is_not_shadowed_by_a_context_id() {
    let (_guard, sock, token) = start_daemon().await;
    post_req(
        &sock,
        "/context/coding",
        &token,
        json!({ "name": "Coding" }),
    )
    .await;

    let (s, body) = get(&sock, "/context/active", &token).await;
    assert_eq!(s, StatusCode::OK, "GET /context/active: {body}");
    assert!(
        body["id"].is_null(),
        "creating a context does not activate it"
    );

    let (s, body) = post_req(&sock, "/context/active", &token, json!({ "id": "coding" })).await;
    assert_eq!(s, StatusCode::OK, "POST /context/active: {body}");
    assert_eq!(body["id"], "coding");
    assert_eq!(
        body["context"]["name"], "Coding",
        "the object comes back too, so a picker needs one request: {body}"
    );

    // The literal wins over the parameter, so a body shaped like a context
    // reaches the setter instead of creating one called `active`. Which is the
    // whole reason that id is refused.
    let (s, body) = post_req(&sock, "/context/active", &token, json!({ "name": "Nope" })).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(
        body["id"].is_null(),
        "the setter ran, with no `id` in the body, so it cleared the selection: {body}"
    );
    let (_, body) = get(&sock, "/context/list", &token).await;
    assert_eq!(
        body["contexts"].as_array().expect("an array").len(),
        1,
        "no context named `active` was created: {body}"
    );

    // `list` has no setter at all, so the same probe there is a 405 rather than
    // an upsert.
    let (s, body) = post_req(&sock, "/context/list", &token, json!({ "name": "Nope" })).await;
    assert_eq!(
        s,
        StatusCode::METHOD_NOT_ALLOWED,
        "`list` is a path too: {body}"
    );

    post_req(&sock, "/context/active", &token, json!({ "id": "coding" })).await;
    let (s, _) = post_req(
        &sock,
        "/context/active",
        &token,
        json!({ "id": Value::Null }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, body) = get(&sock, "/context/active", &token).await;
    assert!(body["id"].is_null(), "null clears the selection: {body}");

    let (s, body) = post_req(&sock, "/context/active", &token, json!({ "id": "ghost" })).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
}

/// The three states a backend can be in are three distinct answers, and the
/// two that look alike — `DELETE` and `POST {"id": null}` — are not the same.
#[tokio::test]
async fn a_backend_can_follow_pin_or_take_no_context() {
    let (_guard, sock, token) = start_daemon().await;
    let path = format!("/backend/{FIXTURE_SOURCE_ENC}/context");
    post_req(
        &sock,
        "/context/coding",
        &token,
        json!({
            "name": "Coding",
            "vocabulary": ["kubectl"],
        }),
    )
    .await;
    post_req(&sock, "/context/email", &token, json!({ "name": "Email" })).await;
    post_req(&sock, "/context/active", &token, json!({ "id": "coding" })).await;

    // Default: follows the active context.
    let (s, body) = get(&sock, &path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET {path}: {body}");
    assert_eq!(body["mode"], "active", "{body}");
    assert_eq!(body["context"]["id"], "coding", "{body}");

    // Pinned elsewhere.
    let (s, body) = post_req(&sock, &path, &token, json!({ "id": "email" })).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "pinned");
    assert_eq!(body["id"], "email");
    assert_eq!(body["context"]["id"], "email");

    // Pinned to nothing. Distinct from following the active context, which is
    // the whole reason `DELETE` exists as a separate verb.
    let (s, body) = post_req(&sock, &path, &token, json!({ "id": Value::Null })).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "none", "{body}");
    assert!(body["context"].is_null(), "{body}");

    // Back to following.
    let (s, body) = delete_req(&sock, &path, &token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "active", "{body}");
    assert_eq!(body["context"]["id"], "coding", "{body}");

    // A pin to a context that is then deleted resolves to nothing — never to
    // the active one, which would send this backend a context the user had
    // pointed it away from.
    post_req(&sock, &path, &token, json!({ "id": "email" })).await;
    delete_req(&sock, "/context/email", &token).await;
    let (s, body) = get(&sock, &path, &token).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["mode"], "pinned", "the pin is kept: {body}");
    assert!(
        body["context"].is_null(),
        "and resolves to nothing, not to the active context: {body}"
    );

    // Re-creating it restores what the user set.
    post_req(&sock, "/context/email", &token, json!({ "name": "Email" })).await;
    let (_, body) = get(&sock, &path, &token).await;
    assert_eq!(body["context"]["id"], "email", "{body}");
}

#[tokio::test]
async fn an_undeliverable_context_is_refused_and_an_unknown_backend_is_a_404() {
    let (_guard, sock, token) = start_daemon().await;

    let (s, body) = post_req(
        &sock,
        "/context/coding",
        &token,
        json!({
            "name": "Coding",
            "prompt": "x".repeat(4001),
        }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error_code"], "invalid_value", "{body}");

    let (s, body) = post_req(&sock, "/context/coding", &token, json!({ "name": "  " })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a context needs a name: {body}");

    let (s, body) = post_req(
        &sock,
        "/context/Coding",
        &token,
        json!({ "name": "Coding" }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "an id is a slug: {body}");

    let (s, body) = get(&sock, "/backend/github.com%2Fno%2Fsuch/context", &token).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error_code"], "unknown_backend", "{body}");
}
