// SPDX-License-Identifier: GPL-3.0-only
//! Settings-scope HTTP endpoint smoke test.
//!
//! Exercises the verb-free settings surface
//! (`POST /audio_theme`, `GET /volume`, `GET /active_model`, etc.) plus
//! scope-aware rejection: a `client`-scope token must NOT be allowed to
//! hit settings endpoints.
//!
//! Uses `SUPER_STT_AUTO_APPROVE=1` so no GUI is needed — it's part of
//! the default `cargo test` flow.

use http_body_util::{BodyExt, Empty, Full};
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
        }
    }
}

async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let unique = format!(
        "stt-settings-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    );
    let tmp = std::env::temp_dir();
    let legacy_socket = tmp.join(format!("{unique}-legacy.sock"));
    let http_socket = tmp.join(format!("{unique}-http.sock"));

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .arg("--socket")
        .arg(&legacy_socket)
        .arg("--device")
        .arg("cpu")
        .arg("--audio-theme")
        .arg("silent")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    // Confirm the daemon is up by minting a throwaway token
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "settings-smoke", "client")
                .await
                .is_ok()
        {
            return (
                DaemonGuard {
                    child,
                    cleanup_paths: vec![legacy_socket.clone(), http_socket.clone()],
                },
                http_socket,
            );
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

/// Tiny GET helper for endpoints `super_stt_shared::daemon::http_client`
/// doesn't yet wrap. Returns (status_code, parsed JSON body).
async fn raw_get_json(
    socket_path: &PathBuf,
    path: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Empty<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("http://stt.local{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"))
        .body(Empty::<Bytes>::new())
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let body_bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    let body: serde_json::Value =
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn raw_post_json(
    socket_path: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let body_bytes = serde_json::to_vec(&body).expect("encode body");
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("http://stt.local{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("content-length", body_bytes.len().to_string())
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
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test]
async fn settings_scope_endpoints() {
    let (_guard, http_socket) = start_daemon().await;

    // Mint a settings-scope token.
    let settings_auth =
        http_client::auth_request(http_socket.clone(), "super-stt settings smoke", "settings")
            .await
            .expect("auth_request settings");
    let settings_token = settings_auth.session_token;

    // Mint a client-scope token (for the rejection check at the end).
    let client_auth =
        http_client::auth_request(http_socket.clone(), "super-stt client smoke", "client")
            .await
            .expect("auth_request client");
    let client_token = client_auth.session_token;

    // --- GET /audio_theme ---
    let (s, body) = raw_get_json(&http_socket, "/audio_theme", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /audio_theme: {body}");
    assert_eq!(body["status"], "success");
    let initial_theme = body["audio_theme"]
        .as_str()
        .unwrap_or("classic")
        .to_string();

    // --- POST /audio_theme: round-trip a different value ---
    let target_theme = if initial_theme == "silent" {
        "classic"
    } else {
        "silent"
    };
    let (s, body) = raw_post_json(
        &http_socket,
        "/audio_theme",
        &settings_token,
        serde_json::json!({ "theme": target_theme }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /audio_theme: {body}");
    assert_eq!(body["status"], "success");

    // Read it back.
    let (s, body) = raw_get_json(&http_socket, "/audio_theme", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["audio_theme"], target_theme);

    // Restore so other tests don't see persistent side-effects.
    let _ = raw_post_json(
        &http_socket,
        "/audio_theme",
        &settings_token,
        serde_json::json!({ "theme": initial_theme }),
    )
    .await;

    // --- GET /volume ---
    let (s, body) = raw_get_json(&http_socket, "/volume", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");

    // --- POST /volume / GET /volume round-trip ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/volume",
        &settings_token,
        serde_json::json!({ "volume": 75 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // --- GET /active_model: composed shape with `current` + `switch` ---
    let (s, body) = raw_get_json(&http_socket, "/active_model", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /active_model: {body}");
    assert_eq!(body["status"], "success");
    let active_model = &body["active_model"];
    assert!(
        active_model["current"]["model"].is_string(),
        "active_model.current.model missing: {active_model}"
    );
    // No switch in flight at startup
    assert!(active_model["switch"].is_null());

    // --- GET /models ---
    let (s, body) = raw_get_json(&http_socket, "/models", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");
    assert!(body["available_models"].is_array());

    // --- GET /audio_themes ---
    let (s, body) = raw_get_json(&http_socket, "/audio_themes", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["available_audio_themes"].is_array());

    // --- GET /preview_typing ---
    let (s, body) = raw_get_json(&http_socket, "/preview_typing", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");

    // --- POST /preview_typing ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/preview_typing",
        &settings_token,
        serde_json::json!({ "enabled": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // --- GET /custom_models_dir (new endpoint) ---
    let (s, body) = raw_get_json(&http_socket, "/custom_models_dir", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");
    // The field is Option<Option<String>>; present, possibly null.
    assert!(body.get("custom_models_dir").is_some());

    // --- Scope enforcement: client-scope token MUST be rejected ---
    let (s, body) = raw_get_json(&http_socket, "/audio_theme", &client_token).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "client token should get 403 on settings endpoint, got {s}: {body}"
    );
    assert_eq!(body["message"], "scope_denied");

    let (s, body) = raw_post_json(
        &http_socket,
        "/volume",
        &client_token,
        serde_json::json!({ "volume": 50 }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "client token should get 403 on settings POST, got {s}: {body}"
    );
    assert_eq!(body["message"], "scope_denied");
}
