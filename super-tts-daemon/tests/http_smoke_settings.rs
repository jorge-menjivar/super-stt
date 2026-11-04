// SPDX-License-Identifier: GPL-3.0-only
//! Settings-scope HTTP endpoint smoke test.
//!
//! Exercises the verb-free settings surface
//! (`POST /settings/audio_theme`, `GET /settings/volume`, `GET /pipeline/1/model`, etc.) plus
//! scope-aware rejection: a `client`-scope token must NOT be allowed to
//! hit settings endpoints.
//!
//! Uses `SUPER_TTS_AUTO_APPROVE=1` so no GUI is needed — it's part of
//! the default `cargo test` flow.

mod common;

use common::{Method, StatusCode, TestDaemon};

use std::path::PathBuf;
use super_tts_shared::daemon::http_client;

async fn start_daemon() -> (TestDaemon, PathBuf) {
    let daemon = common::daemon("settings").start().await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

/// Tiny GET helper for endpoints `super_tts_shared::daemon::http_client`
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
        "super-tts settings smoke",
        &["settings"],
    )
    .await
    .expect("auth_request settings");
    let settings_token = settings_auth.session_token;

    // Mint a client-scope token (for the rejection check at the end).
    let client_auth = http_client::auth_request(
        http_socket.clone(),
        "super-tts client smoke",
        &["speak", "status"],
    )
    .await
    .expect("auth_request client");
    let client_token = client_auth.session_token;

    // --- GET /settings/audio_theme ---
    let (s, body) = raw_get_json(&http_socket, "/settings/audio_theme", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /settings/audio_theme: {body}");
    assert_eq!(body["status"], "success");
    let initial_theme = body["audio_theme"]
        .as_str()
        .unwrap_or("classic")
        .to_string();

    // --- POST /settings/audio_theme: round-trip a different value ---
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
    assert_eq!(s, StatusCode::OK, "POST /settings/audio_theme: {body}");
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

    // --- GET /settings/volume ---
    let (s, body) = raw_get_json(&http_socket, "/settings/volume", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");

    // --- POST /settings/volume / GET /settings/volume round-trip ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/settings/volume",
        &settings_token,
        serde_json::json!({ "volume": 75 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // --- GET /pipeline/1/model: the stage's model slot ---
    let (s, body) = raw_get_json(&http_socket, "/pipeline/1/model", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /pipeline/1/model: {body}");
    assert_eq!(body["status"], "success");
    let slot = &body["model"];
    assert_eq!(slot["stage"], 1, "the slot names its own stage: {slot}");
    // With no backends installed (hermetic test), the daemon is idle and the
    // slot is empty; with a backend it would name a model. Accept both — but
    // the slot is always reported, never an absent key.
    assert!(
        slot["model"].is_string() || slot["model"].is_null(),
        "model slot has unexpected shape: {slot}"
    );
    assert_eq!(
        slot["loaded"], false,
        "a hermetic daemon has loaded nothing: {slot}"
    );
    // No switch in flight at startup
    assert!(slot["switch"].is_null());

    // --- GET /pipeline/1/model/list ---
    let (s, body) = raw_get_json(&http_socket, "/pipeline/1/model/list", &settings_token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["status"], "success");
    assert!(body["available_models"].is_array());

    // --- GET /settings/audio_theme/list ---
    // Pin the wire values, not just the shape: they must be the documented
    // snake_case tokens (docs/protocol/endpoints/v1/settings/audio_theme/list.md), e.g.
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

    // --- The device is per model, and reached through the stage that runs it.
    // There is no daemon-wide device setting to read or write here any more:
    // the stage is empty on a hermetic daemon, so there is nothing to resolve
    // a model name against and both the read and the write say so rather than
    // answering about some global value. `http_smoke_pipeline.rs` drives the
    // same paths with a backend selected.
    let (s, body) = raw_get_json(
        &http_socket,
        "/pipeline/1/model/kokoro-82m/device",
        &settings_token,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "with no backend selected there is no model to read a device for: {body}"
    );
    assert_eq!(body["error_code"], "invalid_backend", "{body}");

    let (s, body) = raw_post_json(
        &http_socket,
        "/pipeline/1/model/kokoro-82m/device",
        &settings_token,
        serde_json::json!({ "device": "definitely-not-a-real-device" }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a bogus device must be refused, not stored: {body}"
    );
    assert_eq!(
        body["error_code"], "invalid_device",
        "the device is validated before the stage is resolved, so the client \
         is told which of the two was wrong: {body}"
    );

    // And the pre-model list is the same shape of refusal.
    let (s, body) = raw_get_json(&http_socket, "/pipeline/1/device/list", &settings_token).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error_code"], "invalid_backend", "{body}");

    // --- GET /settings/allow_online_models ---
    let (s, body) = raw_get_json(
        &http_socket,
        "/settings/allow_online_models",
        &settings_token,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "GET /settings/allow_online_models: {body}"
    );
    assert_eq!(body["status"], "success");
    let initial_allow = body["allow_online_models"].as_bool().unwrap_or(false);

    // --- POST /settings/allow_online_models: round-trip the inverse ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/settings/allow_online_models",
        &settings_token,
        serde_json::json!({ "enabled": !initial_allow }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /settings/allow_online_models");
    let (_, body) = raw_get_json(
        &http_socket,
        "/settings/allow_online_models",
        &settings_token,
    )
    .await;
    assert_eq!(body["allow_online_models"], !initial_allow);
    // Restore.
    let _ = raw_post_json(
        &http_socket,
        "/settings/allow_online_models",
        &settings_token,
        serde_json::json!({ "enabled": initial_allow }),
    )
    .await;

    // --- GET /settings/update_check_enabled ---
    let (s, body) = raw_get_json(
        &http_socket,
        "/settings/update_check_enabled",
        &settings_token,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "GET /settings/update_check_enabled: {body}"
    );
    assert_eq!(body["status"], "success");
    let initial_update_check_enabled = body["update_check_enabled"].as_bool().unwrap_or(true);

    // --- POST /settings/update_check_enabled: round-trip the inverse ---
    let (s, _) = raw_post_json(
        &http_socket,
        "/settings/update_check_enabled",
        &settings_token,
        serde_json::json!({ "enabled": !initial_update_check_enabled }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /settings/update_check_enabled");
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

    // --- GET /settings/update_beta_optin ---
    let (s, body) =
        raw_get_json(&http_socket, "/settings/update_beta_optin", &settings_token).await;
    assert_eq!(s, StatusCode::OK, "GET /settings/update_beta_optin: {body}");
    assert_eq!(body["status"], "success");
    let initial_beta_optin = body["update_beta_optin"]
        .as_str()
        .unwrap_or("auto")
        .to_string();

    // --- POST /settings/update_beta_optin: round-trip ---
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
    assert_eq!(s, StatusCode::OK, "POST /settings/update_beta_optin");
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

    // --- POST /settings/audio_theme/test: just verifies the endpoint accepts the
    // request and returns success. Audio playback is best-effort under
    // CI (no PulseAudio) but the handler always returns 200 with status:"success".
    let (s, body) = raw_post_json(
        &http_socket,
        "/settings/audio_theme/test",
        &settings_token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST /settings/audio_theme/test: {body}");

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
        "POST /pipeline/1/model/cancel with no switch should be 409: {body}"
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
        "POST /pipeline/1/model unknown-model message should mention no backend serves it, got: {msg:?}"
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
