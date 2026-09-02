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
        // Point the daemon's self-update forge client at a guaranteed-refused
        // loopback port (`accept_base_url` allows loopback `http://`), so
        // `POST /update/check` below fails deterministically and offline
        // instead of making a real call to api.github.com under CI.
        .env("GITHUB_API_BASE", "http://127.0.0.1:9")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    // Confirm the daemon is up by minting a throwaway token
    // Hand the child to the guard before the readiness loop: the timeout
    // panic below must still kill and reap the daemon, not leak it.
    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone()],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "settings-smoke", &["status"])
                .await
                .is_ok()
        {
            return (guard, http_socket);
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

/// Tiny GET helper for endpoints `super_stt_shared::daemon::http_client`
/// doesn't yet wrap. Returns (`status_code`, parsed JSON body).
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
#[allow(
    clippy::too_many_lines,
    reason = "one daemon spawn covers the whole settings surface; splitting would spawn one per case"
)]
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

    // --- GET /pipeline/1: the transcription stage, with `switch` progress ---
    let (s, body) = raw_get_json(&http_socket, "/pipeline/1", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /pipeline/1: {body}");
    assert_eq!(body["status"], "success");
    let stage = &body["stage"];
    assert_eq!(stage["stage"], 1);
    assert_eq!(stage["role"], "transcription");
    // With no backends installed (hermetic test), the daemon is idle and the
    // current model is null; with a backend it would be a string. Accept both.
    let current_model = &stage["model"];
    assert!(
        current_model.is_string() || current_model.is_null(),
        "stage.model has unexpected shape: {stage}"
    );
    // No switch in flight at startup
    assert!(stage["switch"].is_null());

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

    // --- /active_device is gone: a device belongs to a model, and is read
    // and set at /pipeline/{stage}/model/{model}/device (covered by
    // http_smoke_pipeline.rs). ---
    let (s, _) = raw_get_json(&http_socket, "/active_device", &settings_token).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "GET /active_device must be gone");

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

    // --- GET /update_check_enabled ---
    let (s, body) = raw_get_json(&http_socket, "/update_check_enabled", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /update_check_enabled: {body}");
    assert_eq!(body["status"], "success");
    let initial_update_check_enabled = body["update_check_enabled"].as_bool().unwrap_or(true);

    // --- POST /update_check_enabled: round-trip the inverse ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/update_check_enabled",
        &settings_token,
        serde_json::json!({ "enabled": !initial_update_check_enabled }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /update_check_enabled");
    let (_, body) = raw_get_json(&http_socket, "/update_check_enabled", &settings_token).await;
    assert_eq!(body["update_check_enabled"], !initial_update_check_enabled);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/update_check_enabled",
        &settings_token,
        serde_json::json!({ "enabled": initial_update_check_enabled }),
    )
    .await;

    // --- GET /update_beta_optin ---
    let (s, body) = raw_get_json(&http_socket, "/update_beta_optin", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /update_beta_optin: {body}");
    assert_eq!(body["status"], "success");
    let initial_beta_optin = body["update_beta_optin"]
        .as_str()
        .unwrap_or("auto")
        .to_string();

    // --- POST /update_beta_optin: round-trip ---
    let target_beta_optin = if initial_beta_optin == "enabled" {
        "disabled"
    } else {
        "enabled"
    };
    let (s, _) = raw_post_json(
        &http_socket,
        "/update_beta_optin",
        &settings_token,
        serde_json::json!({ "value": target_beta_optin }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /update_beta_optin");
    let (_, body) = raw_get_json(&http_socket, "/update_beta_optin", &settings_token).await;
    assert_eq!(body["update_beta_optin"], target_beta_optin);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/update_beta_optin",
        &settings_token,
        serde_json::json!({ "value": initial_beta_optin }),
    )
    .await;

    // --- GET /update: a read-only snapshot. `latest_version` must still be
    // null: `GITHUB_API_BASE` points at a refused loopback port (see
    // `start_daemon`), so no candidate can ever resolve, whether this is the
    // checker's untouched initial state or the background check's initial
    // delay has already elapsed and a failed check has already run. Don't
    // assert `checked_at` is null here — that only holds within the
    // background check's initial delay (currently 60s), which this test's
    // runtime isn't guaranteed to stay under.
    let (s, body) = raw_get_json(&http_socket, "/update", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /update: {body}");
    assert!(body["current_version"].is_string(), "{body}");
    assert!(body["latest_version"].is_null(), "{body}");
    assert_eq!(body["update_available"], false);

    // --- POST /update/check: forces a check. `GITHUB_API_BASE` points the
    // daemon at a refused loopback port (see `start_daemon`), so the network
    // call fails deterministically — the response is still 200 (never a
    // 5xx), with the failure recorded in `last_check_error` and the (empty)
    // previous state preserved rather than clobbered.
    let (s, body) = raw_post_json(
        &http_socket,
        "/update/check",
        &settings_token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /update/check: {body}");
    assert!(body["checked_at"].is_string(), "{body}");
    assert!(
        body["last_check_error"].is_string(),
        "the refused loopback call must fail: {body}"
    );
    assert!(
        body["latest_version"].is_null(),
        "no prior successful check to preserve: {body}"
    );
    assert_eq!(body["update_available"], false);
    assert!(body["installer_asset"].is_null(), "{body}");

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

    // --- POST /pipeline/1/model/cancel: with no switch in flight, the
    // daemon returns 409 Conflict with
    // `{ "status": "error", "message": "No download in progress" }`.
    let (s, body) = raw_post_json(
        &http_socket,
        "/pipeline/1/model/cancel",
        &settings_token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "cancel with no switch in flight should be 409: {body}"
    );
    assert_eq!(body["status"], "error");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("No download")),
        "cancel response missing expected message: {body}"
    );

    // --- POST /pipeline/1/model: unknown model name should be 400 Bad
    // Request with status:"error". We don't want to trigger an
    // actual model download in CI, so we probe the error path.
    let (s, body) = raw_post_json(
        &http_socket,
        "/pipeline/1/model",
        &settings_token,
        serde_json::json!({
            "model": "definitely-not-a-real-model-xyz",
            "source": "builtin",
        }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "POST /pipeline/1/model unknown-model expected 400: {body}"
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
