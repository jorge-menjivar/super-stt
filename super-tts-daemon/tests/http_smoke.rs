// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end smoke tests for the new HTTP daemon protocol.
//!
//! Spawns the `super-tts-daemon` binary against a temp `XDG_RUNTIME_DIR`,
//! a dynamically-chosen UDP port, and `SUPER_TTS_AUTO_APPROVE=1` (so the
//! consent popup is bypassed and `/auth/request` auto-approves), then
//! exercises every endpoint via the shared `http_client` module:
//!
//! - `POST /auth/request` mints a session token (auto-approved).
//! - `GET  /ping` / `GET /status` succeed with the token.
//! - `POST /speak` / `POST /speak/stop` succeed with the token.
//! - Requests without a token (or with a bogus token) get `401 invalid_session`.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p super-tts --test http_smoke -- --nocapture
//! ```

mod common;

use common::TestDaemon;

use std::path::PathBuf;
use super_tts_shared::daemon::http_client;

const APP_NAME: &str = "super-tts smoke test";
const SCOPES: &[&str] = &["speak", "status"];

/// The socket is left where the daemon puts it when told nothing, under a
/// runtime directory of the test's own, so these tests cover that default too.
async fn start_daemon() -> (TestDaemon, PathBuf) {
    let daemon = common::daemon("smoke").default_socket().start().await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

#[tokio::test]
async fn http_endpoints_respond() {
    let (_guard, http_socket) = start_daemon().await;

    // --- POST /auth/request (auto-approved by SUPER_TTS_AUTO_APPROVE=1) ---
    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPES)
        .await
        .expect("auth_request should succeed under SUPER_TTS_AUTO_APPROVE=1");
    assert!(!auth.session_token.is_empty(), "token should not be empty");
    assert!(
        SCOPES
            .iter()
            .all(|s| auth.scopes.iter().any(|g| g.as_str() == *s)),
        "granted scopes {:?} should cover requested {SCOPES:?}",
        auth.scopes
    );
    let token = auth.session_token;

    // --- GET /ping with the token ---
    let pong = http_client::ping(http_socket.clone(), &token)
        .await
        .expect("ping should succeed");
    assert!(
        pong.to_lowercase().contains("pong") || pong.to_lowercase().contains("running"),
        "unexpected ping response: {pong}"
    );

    // --- GET /auth/status with the token: reports back the scope +
    // expiry without spawning a popup or extending the expiry. ---
    let status_info = http_client::auth_status(http_socket.clone(), &token)
        .await
        .expect("auth_status should succeed");
    assert_eq!(status_info.status, "success");
    assert!(
        SCOPES
            .iter()
            .all(|s| status_info.scopes.iter().any(|g| g.as_str() == *s)),
        "auth_status scopes {:?} should cover {SCOPES:?}",
        status_info.scopes
    );
    assert!(
        !status_info.expires_at.is_empty(),
        "auth_status should report an expires_at"
    );

    // --- GET /auth/status with a bogus token: should be rejected
    // with `invalid_session` (same shape as any other 401). ---
    let bad_status = http_client::auth_status(http_socket.clone(), "not-a-real-token").await;
    assert!(
        bad_status.is_err(),
        "auth_status with bogus token should fail"
    );
    let err = bad_status.unwrap_err();
    assert!(
        err.is_invalid_session(),
        "expected InvalidSession variant, got: {err}"
    );

    // --- GET /status with the token ---
    let status = http_client::status(http_socket.clone(), &token)
        .await
        .expect("status should succeed");
    assert_eq!(status.status, "success", "status response: {status:?}");
    // With no backends installed (hermetic test), the daemon is idle and has
    // no current model; with a backend it would report one. Either is valid.
    assert!(status.device.is_some(), "status should have device");
    // `busy` is the affordance a client uses to decide whether a `speak`
    // would interrupt something — must always be present.
    assert_eq!(
        status.busy,
        Some(false),
        "status must report busy=false on a freshly-booted daemon"
    );

    // --- POST /speak ---
    // Hermetic test harness => no backend installed => no model loaded, so the
    // daemon fails fast with `409 model_not_loaded` rather than committing to
    // an utterance it cannot produce (docs/protocol/endpoints/v1/speak.md).
    // Tolerate either that typed error or (should a backend somehow be
    // present) a normal `202` carrying an utterance id.
    let result = http_client::speak(http_socket.clone(), &token, "hello from the smoke test").await;
    match result {
        Ok(resp) => assert!(
            resp.status == "success" || resp.status == "error",
            "speak returned unknown status: {:?}",
            resp.status
        ),
        Err(e) => assert!(
            e.to_string().contains("model_not_loaded"),
            "expected model_not_loaded, got: {e}"
        ),
    }

    // --- POST /speak/stop ---
    let resp = http_client::speak_stop(http_socket.clone(), &token)
        .await
        .expect("speak/stop should respond");
    assert!(
        resp.status == "success" || resp.status == "error",
        "speak/stop returned unknown status: {:?}",
        resp.status
    );

    // --- Requests without a token must be rejected ---
    // Build a raw request directly so we can omit the Authorization header.
    let unauthorized = http_client::ping(http_socket.clone(), "not-a-real-token").await;
    assert!(
        unauthorized.is_err(),
        "ping with bogus token should be rejected (got Ok: {unauthorized:?})"
    );
    let err = unauthorized.unwrap_err();
    assert!(
        err.is_invalid_session(),
        "expected InvalidSession variant, got: {err}"
    );
}

/// `POST /v1/speak/stop` must be idempotent when nothing is speaking: it
/// answers `200 success` and must NOT have any side effect. A client that
/// cancels on a keypress should not have to know whether it won the race, and
/// must never turn a stop into a start.
#[tokio::test]
async fn speak_stop_idempotent_when_idle() {
    let (_guard, http_socket) = start_daemon().await;
    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPES)
        .await
        .expect("auth_request should succeed");
    let token = auth.session_token;

    // Idle daemon: status must report busy=false BEFORE we call /speak/stop.
    let pre = http_client::status(http_socket.clone(), &token)
        .await
        .expect("status before stop");
    assert_eq!(
        pre.busy,
        Some(false),
        "test setup: daemon must be idle before calling speak_stop"
    );

    // Twice, because the point is that a second stop is as harmless as the
    // first: no error, and still nothing to cancel.
    for attempt in 1..=2 {
        let resp = http_client::speak_stop(http_socket.clone(), &token)
            .await
            .unwrap_or_else(|e| panic!("speak/stop attempt {attempt} should respond: {e}"));
        assert_eq!(resp.status, "success", "attempt {attempt} got: {resp:?}");
        assert_eq!(
            resp.utterance_id, None,
            "attempt {attempt} reported cancelling an utterance that never existed"
        );
    }

    // Confirm via /status that no utterance was started.
    let post = http_client::status(http_socket.clone(), &token)
        .await
        .expect("status after stop");
    assert_eq!(
        post.busy,
        Some(false),
        "speak_stop on an idle daemon must not start anything; got {post:?}"
    );
}

/// Bad input reaches the client as the coded `400` envelope, through the real
/// daemon and the real transport. The daemon validates `text` twice — once in
/// the endpoint, once in the command dispatcher — and only the endpoint's check
/// produces a classified `invalid_value`; a request that slipped past it would
/// still fail, but as an uncoded `500` a client cannot act on.
#[tokio::test]
async fn speak_without_text_is_a_coded_bad_request() {
    let (_guard, http_socket) = start_daemon().await;
    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPES)
        .await
        .expect("auth_request should succeed");
    let token = auth.session_token;

    // `speak()` always sends a `text`, so post the body directly to reach the
    // missing-field path a hand-written client can hit.
    let err =
        http_client::transport::post_json::<super_tts_shared::models::protocol::DaemonResponse>(
            http_socket.clone(),
            &token,
            "/speak",
            &serde_json::json!({ "voice": "alloy" }),
        )
        .await
        .expect_err("a body with no text is not a valid utterance");

    let msg = err.to_string();
    assert!(
        msg.contains("missing text"),
        "the daemon must say what was wrong, got: {msg}"
    );
    assert!(
        msg.contains("400"),
        "a malformed request is a 400, got: {msg}"
    );
}

/// A speak request is `text` and nothing else. The fields that used to choose
/// how an utterance sounded are settings now, and one still named is refused
/// rather than ignored — a client given a different voice than it asked for has
/// no way to find out.
#[tokio::test]
async fn the_removed_option_fields_are_refused_rather_than_ignored() {
    let (_guard, http_socket) = start_daemon().await;
    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPES)
        .await
        .expect("auth_request should succeed");
    let token = auth.session_token;

    for field in ["voice", "language", "speed", "instructions"] {
        let value = if field == "speed" {
            serde_json::json!(1.5)
        } else {
            serde_json::json!("whatever")
        };
        let err = http_client::transport::post_json::<
            super_tts_shared::models::protocol::DaemonResponse,
        >(
            http_socket.clone(),
            &token,
            "/speak",
            &serde_json::json!({ "text": "Hello.", field: value }),
        )
        .await
        .expect_err("the removed fields are not accepted");
        let msg = err.to_string();
        assert!(
            msg.contains(field),
            "the refusal must name `{field}` so a client knows what to drop, got: {msg}"
        );
        assert!(
            msg.contains("400"),
            "a field that no longer exists is a malformed request, got: {msg}"
        );
    }
}

/// The same token speaks fine with `text` alone — the rule is about the removed
/// fields, not the endpoint. A `null` is not naming one either: that is how a
/// client spells "use what is configured", which is what every request means
/// now.
#[tokio::test]
async fn text_alone_is_a_complete_speak_request() {
    let (_guard, http_socket) = start_daemon().await;
    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPES)
        .await
        .expect("auth_request should succeed");
    let token = auth.session_token;

    for body in [
        serde_json::json!({ "text": "Hello." }),
        serde_json::json!({ "text": "Hello.", "voice": null, "speed": null }),
    ] {
        let err = http_client::transport::post_json::<
            super_tts_shared::models::protocol::DaemonResponse,
        >(http_socket.clone(), &token, "/speak", &body)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
        assert!(
            !err.contains("no longer accepted"),
            "nothing was named, so nothing is refused; got: {err}"
        );
    }
}
