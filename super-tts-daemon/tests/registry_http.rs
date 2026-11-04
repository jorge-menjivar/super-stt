// SPDX-License-Identifier: GPL-3.0-only
//! HTTP integration tests for the `/registry/*` endpoints.
//!
//! These tests spawn the real `super-tts-daemon` binary, wire it to a
//! mockito HTTP server in place of the live registry, and exercise the
//! two core paths:
//!
//! - `GET  /v1/registry/backend/list`  — index fetched, compat evaluated, list
//!   returned.
//! - `POST /v1/registry/backend/install` + `GET /v1/events` — hash mismatch
//!   in the install pipeline surfaces as a `registry.install.failed` SSE
//!   event.
//!
//! Both tests carry `#[ignore]` so they are skipped by the automated
//! `cargo test --lib` run (which hangs on a locked keyring). Run them
//! manually with:
//!
//! ```bash
//! cargo test -p super-tts-daemon --test registry_http -- --ignored --nocapture
//! ```

mod common;

use common::{Method, StatusCode, TestDaemon};
use hyper::Request;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use std::path::PathBuf;
use std::time::Duration;
use super_tts_shared::daemon::http_client;
use super_tts_shared::registry::RegistryListResponse;
use tokio::net::UnixStream;

// ---------- harness ----------------------------------------------------------

async fn start_daemon_with_registry(registry_url: &str) -> (TestDaemon, PathBuf) {
    let daemon = common::daemon("registry")
        .env("SUPER_TTS_REGISTRY_URL", registry_url)
        .start()
        .await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

// ---------- raw HTTP helpers -------------------------------------------------

async fn raw_get_json(
    socket_path: &PathBuf,
    path: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    common::request(socket_path, Method::GET, path, Some(token), None).await
}

async fn raw_post_json(
    socket_path: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    common::request(socket_path, Method::POST, path, Some(token), Some(&body)).await
}

/// Open `GET /v1/events?topics=registry_install` and return the raw response
/// body stream, which the caller drives line-by-line.
///
/// Returns `(sender_handle, raw_bytes_receiver)` so the caller can drive
/// the connection without blocking. We return the body bytes one
/// chunk at a time via the collected stream.
///
/// Because SSE streams are infinite, we open the connection here and hand
/// back an `hyper::Response` whose body can be read with `.collect()` after
/// a timeout — we abort it via `tokio::time::timeout` at the call site.
async fn open_sse_stream(
    socket_path: &PathBuf,
    token: &str,
) -> hyper::Response<hyper::body::Incoming> {
    let stream = UnixStream::connect(socket_path).await.expect("connect sse");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Empty<Bytes>>(io)
        .await
        .expect("handshake sse");
    // Drive the connection in the background — it outlives the request.
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method(Method::GET)
        .uri("http://tts.local/v1/events?topics=registry_install")
        .header("host", "tts.local")
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "text/event-stream")
        .body(Empty::<Bytes>::new())
        .expect("build sse req");

    sender.send_request(req).await.expect("send sse req")
}

// ---------- SSE frame parser -------------------------------------------------

/// Parse SSE frames from a byte buffer. Returns all `data:` payloads whose
/// corresponding `event:` line equals `event_name`. Each item is the raw JSON
/// string from the `data:` line.
fn extract_sse_data_for_event(raw: &[u8], event_name: &str) -> Vec<String> {
    let text = String::from_utf8_lossy(raw);
    let mut out = Vec::new();
    // SSE frames are separated by blank lines. Within a frame, fields are
    // `key: value\n` lines. We collect (event, data) pairs.
    for frame in text.split("\n\n") {
        let mut cur_event: Option<&str> = None;
        let mut cur_data: Option<&str> = None;
        for line in frame.lines() {
            if let Some(rest) = line.strip_prefix("event: ") {
                cur_event = Some(rest.trim());
            } else if let Some(rest) = line.strip_prefix("data: ") {
                cur_data = Some(rest.trim());
            }
        }
        if cur_event == Some(event_name)
            && let Some(d) = cur_data
        {
            out.push(d.to_owned());
        }
    }
    out
}

// ---------- index.json fixture -----------------------------------------------

/// Minimal `index.json` with a single wasm backend whose `source` is
/// `"github.com/x/y"` and `id` is `"openai"`.
fn fixture_index_wasm(asset_url: &str) -> String {
    serde_json::json!({
        "schema_version": 1,
        "generated_at": "2026-01-01T00:00:00Z",
        "min_client": "0.0.0",
        "backends": [{
            "id": "openai",
            "source": "github.com/x/y",
            "version": "1.0.0",
            "tag": "v1.0.0",
            "name": "OpenAI",
            "description": null,
            "license": "Apache-2.0",
            "kind": "wasm",
            "contract": "v1",
            "entrypoint": "openai.wasm",
            "allowed_hosts": ["api.openai.com"],
            "online": true,
            "supports_gpu": false,
            "supports_cpu": true,
            "models": [],
            "secrets": [],
            "options": [],
            "assets": {
                "wasm": {
                    "url": asset_url,
                    "size": 4,
                    "sha256": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
                }
            }
        }]
    })
    .to_string()
}

/// Same fixture but with a correct sha256 for the wasm-magic bytes
/// `\x00\x61\x73\x6d` (never actually used here, kept for reference).
fn fixture_index_correct_hash(asset_url: &str) -> String {
    serde_json::json!({
        "schema_version": 1,
        "generated_at": "2026-01-01T00:00:00Z",
        "min_client": "0.0.0",
        "backends": [{
            "id": "openai",
            "source": "github.com/x/y",
            "version": "1.0.0",
            "tag": "v1.0.0",
            "name": "OpenAI",
            "description": null,
            "license": "Apache-2.0",
            "kind": "wasm",
            "contract": "v1",
            "entrypoint": "openai.wasm",
            "allowed_hosts": ["api.openai.com"],
            "online": true,
            "supports_gpu": false,
            "supports_cpu": true,
            "models": [],
            "secrets": [],
            "options": [],
            "assets": {
                "wasm": {
                    "url": asset_url,
                    "size": 4,
                    "sha256": "cd5d4935a48c0672cb06407bb443bc0087aff947c6b864bac886982c73b3027f"
                }
            }
        }]
    })
    .to_string()
}

// ---------- tests -------------------------------------------------------------

/// `GET /v1/registry/backend/list` returns the list from the mockito index, with
/// `compatibility.compatible == true` for a wasm backend (wasm is always
/// compatible on any host) and `id == "openai"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns real daemon; requires a responsive system keyring"]
async fn list_registry_backends_returns_compat() {
    let mut mock_server = mockito::Server::new_async().await;

    // Serve the index with a placeholder asset URL (not fetched by this test).
    let asset_url = format!("{}/openai.wasm", mock_server.url());
    let index_body = fixture_index_correct_hash(&asset_url);

    let _mock = mock_server
        .mock("GET", "/index.json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(index_body)
        .create_async()
        .await;

    let registry_url = format!("{}/index.json", mock_server.url());
    let (_guard, http_socket) = start_daemon_with_registry(&registry_url).await;

    // Mint a settings-scope token (registry endpoints require settings scope).
    let auth = http_client::auth_request(http_socket.clone(), "registry-test", &["settings"])
        .await
        .expect("auth_request should succeed under SUPER_TTS_AUTO_APPROVE=1");
    let token = auth.session_token;

    let (status, body) = raw_get_json(&http_socket, "/registry/backend/list", &token).await;
    assert_eq!(status, StatusCode::OK, "GET /registry/backend/list: {body}");

    let list: RegistryListResponse =
        serde_json::from_value(body.clone()).expect("body should decode as RegistryListResponse");

    assert_eq!(
        list.backends.len(),
        1,
        "expected 1 backend; got {}: {body}",
        list.backends.len()
    );
    let backend = &list.backends[0];
    assert_eq!(backend.id, "openai", "id mismatch: {backend:?}");
    assert!(
        backend.compatibility.compatible,
        "wasm backend must always be compatible; got: {backend:?}"
    );
}

/// `POST /v1/registry/backend/install` with a backend whose index entry
/// advertises a wrong sha256 surfaces a `registry.install.failed` SSE event
/// with `error == "asset_hash_mismatch"`.
///
/// Setup:
/// - Mockito index server hosts the index with `sha256 = "deadbeef..."`.
/// - Mockito asset server serves the 4-byte wasm magic `\x00\x61\x73\x6d`.
///   Real sha256 of those bytes: `cd5d4935...`, not `deadbeef...`.
/// - Daemon is pointed at the index server via `SUPER_TTS_REGISTRY_URL`.
/// - Subscribe to `/events?topics=registry_install` before posting the
///   install so no events are missed.
/// - POST returns `202 Accepted` with an `install_id`.
/// - Drain the SSE stream (with a timeout) looking for
///   `registry.install.failed` carrying `error == "asset_hash_mismatch"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns real daemon; requires a responsive system keyring"]
async fn install_pipeline_rejects_hash_mismatch() {
    // Two independent mockito servers: one for the registry index, one for
    // the asset download. Using separate servers avoids path-collision issues
    // when both would otherwise share the same `mockito::Server`.
    let mut index_server = mockito::Server::new_async().await;
    let mut asset_server = mockito::Server::new_async().await;

    // Asset server: serve 4-byte wasm magic (sha256 ≠ "deadbeef...").
    let wasm_magic: &[u8] = &[0x00, 0x61, 0x73, 0x6d];
    let _asset_mock = asset_server
        .mock("GET", "/openai.wasm")
        .with_status(200)
        .with_header("content-type", "application/wasm")
        .with_body(wasm_magic)
        .create_async()
        .await;

    // Index server: advertise deadbeef sha256 so the pipeline must fail.
    let asset_url = format!("{}/openai.wasm", asset_server.url());
    let index_body = fixture_index_wasm(&asset_url);
    let _index_mock = index_server
        .mock("GET", "/index.json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(index_body)
        .create_async()
        .await;

    let registry_url = format!("{}/index.json", index_server.url());
    let (_guard, http_socket) = start_daemon_with_registry(&registry_url).await;

    // `settings` for the install POST, `daemon_status` for the SSE topic:
    // `/events` scopes per *topic*, and `registry_install` sits under
    // `daemon_status`. A settings-only token opens the stream with a `403`.
    let auth = http_client::auth_request(
        http_socket.clone(),
        "registry-test",
        &["settings", "daemon_status"],
    )
    .await
    .expect("auth_request should succeed");
    let token = auth.session_token;

    // Open SSE subscription BEFORE posting the install so no events are
    // lost between the POST returning and the consumer subscribing.
    let sse_resp = open_sse_stream(&http_socket, &token).await;
    assert_eq!(
        sse_resp.status(),
        StatusCode::OK,
        "SSE stream must open successfully"
    );

    // POST install.
    let (status, body) = raw_post_json(
        &http_socket,
        "/registry/backend/install",
        &token,
        serde_json::json!({ "source": "github.com/x/y" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "install POST must return 202; got {status}: {body}"
    );
    let install_id = body["install_id"]
        .as_str()
        .expect("response must contain install_id")
        .to_owned();
    assert!(!install_id.is_empty(), "install_id must not be empty");

    // Read the stream frame by frame until the failed event shows up.
    //
    // Not `collect()`: an SSE stream does not end, so collecting the whole
    // body never resolves, and a `timeout` around it drops the future along
    // with every byte it had buffered, leaving nothing to match on.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut sse_bytes: Vec<u8> = Vec::new();
    let mut body = sse_resp.into_body();

    let failed = loop {
        if let Some(found) = extract_sse_data_for_event(&sse_bytes, "registry_install")
            .into_iter()
            .find(|data| {
                let v: serde_json::Value = serde_json::from_str(data).unwrap_or_default();
                v["type"] == "registry.install.failed" && v["install_id"] == install_id.as_str()
            })
        {
            break found;
        }

        match tokio::time::timeout_at(deadline, body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Some(chunk) = frame.data_ref() {
                    sse_bytes.extend_from_slice(chunk);
                }
            }
            Ok(Some(Err(e))) => panic!("SSE stream errored while waiting: {e}"),
            Ok(None) => panic!(
                "SSE stream closed before registry.install.failed for \
                 install_id={install_id}; saw: {:?}",
                String::from_utf8_lossy(&sse_bytes)
            ),
            Err(_) => panic!(
                "no registry.install.failed event for install_id={install_id} within 30 s; \
                 saw: {:?}",
                String::from_utf8_lossy(&sse_bytes)
            ),
        }
    };

    let v: serde_json::Value = serde_json::from_str(&failed).expect("parse failed frame");
    assert_eq!(
        v["error"], "asset_hash_mismatch",
        "wrong error variant: {v}"
    );
    assert_eq!(v["source"], "github.com/x/y", "wrong source in event: {v}");
}
