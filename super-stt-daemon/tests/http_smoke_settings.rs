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

mod common;

use common::{Method, StatusCode, TestDaemon};

use std::path::PathBuf;
use super_stt_shared::daemon::http_client;

async fn start_daemon() -> (TestDaemon, PathBuf) {
    let daemon = common::daemon("settings").start().await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

/// Tiny GET helper for endpoints `super_stt_shared::daemon::http_client`
/// doesn't yet wrap. Returns (`status_code`, parsed JSON body).
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
    let (s, body) = raw_get_json(&http_socket, "/settings/audio_theme", &settings_token).await;
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
        "/settings/audio_theme",
        &settings_token,
        serde_json::json!({ "theme": target_theme }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /audio_theme: {body}");
    assert_eq!(body["status"], "success");

    // Read it back.
    let (s, body) = raw_get_json(&http_socket, "/settings/audio_theme", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["audio_theme"], target_theme);

    // Restore so other tests don't see persistent side-effects.
    let _ = raw_post_json(
        &http_socket,
        "/settings/audio_theme",
        &settings_token,
        serde_json::json!({ "theme": initial_theme }),
    )
    .await;

    // --- GET /volume ---
    let (s, body) = raw_get_json(&http_socket, "/settings/volume", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");

    // --- POST /volume / GET /volume round-trip ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/settings/volume",
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

    // --- GET /pipeline/{stage}/model/list (was the stage-1-only GET /models) ---
    for stage in [1, 2] {
        let path = format!("/pipeline/{stage}/model/list");
        let (s, body) = raw_get_json(&http_socket, &path, &settings_token).await;
        assert_eq!(s, StatusCode::OK, "GET {path}: {body}");
        assert_eq!(body["status"], "success", "GET {path}: {body}");
        assert!(
            body["available_models"].is_array(),
            "GET {path} did not answer with a list: {body}"
        );
    }

    // --- GET /audio_themes ---
    // Pin the wire values, not just the shape: they must be the documented
    // snake_case tokens (docs/protocol/endpoints/v1/audio_themes.md), e.g.
    // `scifi` — not the PascalCase variant names.
    let (s, body) = raw_get_json(&http_socket, "/settings/audio_theme/list", &settings_token).await;
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
    let (s, body) = raw_get_json(&http_socket, "/settings/preview_typing", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");

    // --- POST /preview_typing ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/settings/preview_typing",
        &settings_token,
        serde_json::json!({ "enabled": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // --- GET /custom_models_dir (new endpoint) ---
    let (s, body) =
        raw_get_json(&http_socket, "/settings/custom_models_dir", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");
    // The field is Option<Option<String>>; present, possibly null.
    assert!(body.get("custom_models_dir").is_some());

    // --- POST /custom_models_dir: null clears the override, then read-back ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/settings/custom_models_dir",
        &settings_token,
        serde_json::json!({ "path": null }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /custom_models_dir null");
    let (s, body) =
        raw_get_json(&http_socket, "/settings/custom_models_dir", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["custom_models_dir"].is_null());

    // --- /active_device is gone: a device belongs to a model, and is read
    // and set at /pipeline/{stage}/model/{model}/device (covered by
    // http_smoke_pipeline.rs). ---
    let (s, _) = raw_get_json(&http_socket, "/active_device", &settings_token).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "GET /active_device must be gone");

    // --- GET /recording_stop_mode ---
    let (s, body) = raw_get_json(
        &http_socket,
        "/settings/recording_stop_mode",
        &settings_token,
    )
    .await;
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
        "/settings/recording_stop_mode",
        &settings_token,
        serde_json::json!({ "mode": target_stop_mode }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /recording_stop_mode");
    let (_, body) = raw_get_json(
        &http_socket,
        "/settings/recording_stop_mode",
        &settings_token,
    )
    .await;
    assert_eq!(body["recording_stop_mode"], target_stop_mode);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/settings/recording_stop_mode",
        &settings_token,
        serde_json::json!({ "mode": initial_stop_mode }),
    )
    .await;

    // --- GET /write_method ---
    let (s, body) = raw_get_json(&http_socket, "/settings/write_method", &settings_token).await;
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
        "/settings/write_method",
        &settings_token,
        serde_json::json!({ "method": target_write_method }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /write_method");
    let (_, body) = raw_get_json(&http_socket, "/settings/write_method", &settings_token).await;
    assert_eq!(body["write_method"], target_write_method);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/settings/write_method",
        &settings_token,
        serde_json::json!({ "method": initial_write_method }),
    )
    .await;

    // --- GET /update_check_enabled ---
    let (s, body) = raw_get_json(
        &http_socket,
        "/settings/update_check_enabled",
        &settings_token,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "GET /update_check_enabled: {body}");
    assert_eq!(body["status"], "success");
    let initial_update_check_enabled = body["update_check_enabled"].as_bool().unwrap_or(true);

    // --- POST /update_check_enabled: round-trip the inverse ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/settings/update_check_enabled",
        &settings_token,
        serde_json::json!({ "enabled": !initial_update_check_enabled }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /update_check_enabled");
    let (_, body) = raw_get_json(
        &http_socket,
        "/settings/update_check_enabled",
        &settings_token,
    )
    .await;
    assert_eq!(body["update_check_enabled"], !initial_update_check_enabled);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/settings/update_check_enabled",
        &settings_token,
        serde_json::json!({ "enabled": initial_update_check_enabled }),
    )
    .await;

    // --- GET /update_beta_optin ---
    let (s, body) =
        raw_get_json(&http_socket, "/settings/update_beta_optin", &settings_token).await;
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
        "/settings/update_beta_optin",
        &settings_token,
        serde_json::json!({ "value": target_beta_optin }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /update_beta_optin");
    let (_, body) =
        raw_get_json(&http_socket, "/settings/update_beta_optin", &settings_token).await;
    assert_eq!(body["update_beta_optin"], target_beta_optin);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/settings/update_beta_optin",
        &settings_token,
        serde_json::json!({ "value": initial_beta_optin }),
    )
    .await;

    // --- GET /gpu_info: a live probe of the host's accelerators. Settings
    // scope guards it, but it is not a stored preference — it reports what
    // this machine has right now. A host with no GPU (every CI runner) is a
    // 200 with an empty list, never a 404 or a 500, so the shape below is
    // what a client can rely on everywhere.
    let (s, body) = raw_get_json(&http_socket, "/gpu_info", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /gpu_info: {body}");
    assert_eq!(body["status"], "success", "{body}");
    let gpus = body["gpu_info"]
        .as_array()
        .unwrap_or_else(|| panic!("gpu_info must be an array, got: {body}"));
    // Each entry names a GPU and its memory; nothing here assumes one exists.
    for gpu in gpus {
        assert!(gpu["name"].is_string(), "gpu entry without a name: {gpu}");
    }
    // `host` is the driver/runtime inventory, reported whether or not any GPU
    // was found — it is what decides which backend builds will run here.
    assert!(
        body["host"].is_object(),
        "host toolchain versions must be present even with no GPU: {body}"
    );

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
        "/settings/audio_theme/test",
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
    let (s, body) = raw_get_json(&http_socket, "/settings/audio_theme", &client_token).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "client token should get 403 on settings endpoint, got {s}: {body}"
    );
    assert_eq!(body["message"], "scope_denied");

    let (s, body) = raw_post_json(
        &http_socket,
        "/settings/volume",
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
