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

/// Monotonic per-call counter so concurrent tests in the same test
/// binary get unique paths. `Instant::now().elapsed().as_nanos()`
/// returns 0 immediately after construction and would collide.
fn next_test_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static UNIQ: AtomicU64 = AtomicU64::new(0);
    UNIQ.fetch_add(1, Ordering::Relaxed)
}

async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let unique = format!("stt-settings-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    // Isolate XDG_CONFIG_HOME so the test daemon doesn't write the
    // developer's real `~/.config/super-stt/daemon.toml` (e.g.
    // overriding their audio theme to Silent or device to cpu via
    // `apply_cli_overrides_to_config`).
    let config_home = tmp.join(format!("{unique}-config"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");
    // Isolate XDG_DATA_HOME too: the daemon discovers backends under
    // `<data_dir>/super-stt/backends`. An empty isolated dir keeps the smoke
    // test hermetic and fast — no real backend is spawned at startup, so the
    // daemon comes up idle (which the assertions below tolerate).
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&data_home).expect("create test data dir");

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1") // in-memory keyring (no secret-service prompt in tests/CI)
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

    // Confirm the daemon is up by minting a throwaway token
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "settings-smoke", &["status"])
                .await
                .is_ok()
        {
            return (
                DaemonGuard {
                    child,
                    cleanup_paths: vec![http_socket.clone()],
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
        .uri(format!("http://stt.local/v1{path}"))
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
        .uri(format!("http://stt.local/v1{path}"))
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
    let settings_auth = http_client::auth_request(
        http_socket.clone(),
        "super-stt settings smoke",
        &["settings"],
    )
    .await
    .expect("auth_request settings");
    let settings_token = settings_auth.session_token;

    // Mint a client-scope token (for the rejection check at the end).
    let client_auth = http_client::auth_request(
        http_socket.clone(),
        "super-stt client smoke",
        &["transcribe", "status"],
    )
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
    // With no backends installed (hermetic test), the daemon is idle and the
    // current model is null; with a backend it would be a string. Accept both.
    let current_model = &active_model["current"]["model"];
    assert!(
        current_model.is_string() || current_model.is_null(),
        "active_model.current.model has unexpected shape: {active_model}"
    );
    // No switch in flight at startup
    assert!(active_model["switch"].is_null());

    // --- GET /models ---
    let (s, body) = raw_get_json(&http_socket, "/models", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");
    assert!(body["available_models"].is_array());

    // --- GET /audio_themes ---
    // Pin the wire values, not just the shape: they must be the documented
    // snake_case tokens (docs/protocol/endpoints/v1/audio_themes.md), e.g.
    // `scifi` — not the PascalCase variant names.
    let (s, body) = raw_get_json(&http_socket, "/audio_themes", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    let themes = body["available_audio_themes"]
        .as_array()
        .expect("available_audio_themes must be a JSON array");
    let names: Vec<&str> = themes.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "classic", "gentle", "minimal", "scifi", "musical", "nature", "retro", "silent",
        ],
        "audio themes must be the documented snake_case tokens"
    );

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

    // --- POST /custom_models_dir: null clears the override, then read-back ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/custom_models_dir",
        &settings_token,
        serde_json::json!({ "path": null }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /custom_models_dir null");
    let (s, body) = raw_get_json(&http_socket, "/custom_models_dir", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["custom_models_dir"].is_null());

    // --- GET /active_device ---
    let (s, body) = raw_get_json(&http_socket, "/active_device", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /active_device: {body}");
    assert_eq!(body["status"], "success");
    assert!(body["device"].is_string(), "device field missing: {body}");
    assert!(
        body["available_devices"].is_array(),
        "available_devices missing: {body}"
    );

    // --- POST /active_device: invalid device name should error ---
    let (s, body) = raw_post_json(
        &http_socket,
        "/active_device",
        &settings_token,
        serde_json::json!({ "device": "definitely-not-a-real-device" }),
    )
    .await;
    // The daemon returns 200 with status:"error" or 400 — both are
    // acceptable as long as the device wasn't switched. Verify we
    // didn't end up on a new device.
    let (_, after) = raw_get_json(&http_socket, "/active_device", &settings_token).await;
    assert_ne!(
        after["device"], "definitely-not-a-real-device",
        "bogus device should not have been accepted (response was {s}: {body})"
    );

    // --- GET /recording_stop_mode ---
    let (s, body) = raw_get_json(&http_socket, "/recording_stop_mode", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /recording_stop_mode: {body}");
    assert_eq!(body["status"], "success");
    let initial_stop_mode = body["recording_stop_mode"]
        .as_str()
        .unwrap_or("silence_and_manual")
        .to_string();

    // --- POST /recording_stop_mode: round-trip ---
    let target_stop_mode = if initial_stop_mode == "manual_only" {
        "silence_and_manual"
    } else {
        "manual_only"
    };
    let (s, _) = raw_post_json(
        &http_socket,
        "/recording_stop_mode",
        &settings_token,
        serde_json::json!({ "mode": target_stop_mode }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /recording_stop_mode");
    let (_, body) = raw_get_json(&http_socket, "/recording_stop_mode", &settings_token).await;
    assert_eq!(body["recording_stop_mode"], target_stop_mode);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/recording_stop_mode",
        &settings_token,
        serde_json::json!({ "mode": initial_stop_mode }),
    )
    .await;

    // --- GET /write_method ---
    let (s, body) = raw_get_json(&http_socket, "/write_method", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /write_method: {body}");
    assert_eq!(body["status"], "success");
    let initial_write_method = body["write_method"].as_str().unwrap_or("auto").to_string();

    // --- POST /write_method: round-trip ---
    let target_write_method = if initial_write_method == "ydotool" {
        "auto"
    } else {
        "ydotool"
    };
    let (s, _) = raw_post_json(
        &http_socket,
        "/write_method",
        &settings_token,
        serde_json::json!({ "method": target_write_method }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /write_method");
    let (_, body) = raw_get_json(&http_socket, "/write_method", &settings_token).await;
    assert_eq!(body["write_method"], target_write_method);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/write_method",
        &settings_token,
        serde_json::json!({ "method": initial_write_method }),
    )
    .await;

    // --- GET /allow_online_models ---
    let (s, body) = raw_get_json(&http_socket, "/allow_online_models", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /allow_online_models: {body}");
    assert_eq!(body["status"], "success");
    let initial_allow = body["allow_online_models"].as_bool().unwrap_or(false);

    // --- POST /allow_online_models: round-trip the inverse ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/allow_online_models",
        &settings_token,
        serde_json::json!({ "enabled": !initial_allow }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /allow_online_models");
    let (_, body) = raw_get_json(&http_socket, "/allow_online_models", &settings_token).await;
    assert_eq!(body["allow_online_models"], !initial_allow);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/allow_online_models",
        &settings_token,
        serde_json::json!({ "enabled": initial_allow }),
    )
    .await;

    // --- POST /audio_theme/test: just verifies the endpoint accepts the
    // request and returns success. Audio playback is best-effort under
    // CI (no PulseAudio) but the handler always returns 200 with status:"success".
    let (s, body) = raw_post_json(
        &http_socket,
        "/audio_theme/test",
        &settings_token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /audio_theme/test: {body}");

    // --- POST /active_model/cancel: with no switch in flight, the
    // daemon returns 409 Conflict with
    // `{ "status": "error", "message": "No download in progress" }`.
    let (s, body) = raw_post_json(
        &http_socket,
        "/active_model/cancel",
        &settings_token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "POST /active_model/cancel with no switch should be 409: {body}"
    );
    assert_eq!(body["status"], "error");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("No download")),
        "cancel response missing expected message: {body}"
    );

    // --- POST /active_model: unknown model name should be 400 Bad
    // Request with status:"error". We don't want to trigger an
    // actual model download in CI, so we probe the error path.
    let (s, body) = raw_post_json(
        &http_socket,
        "/active_model",
        &settings_token,
        serde_json::json!({
            "model": "definitely-not-a-real-model-xyz",
            "provider": "local_whisper",
            "source": "builtin",
        }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "POST /active_model unknown-model expected 400: {body}"
    );
    assert_eq!(body["status"], "error");
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("No installed backend"),
        "active_model unknown-model message should mention no backend serves it, got: {msg:?}"
    );

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
