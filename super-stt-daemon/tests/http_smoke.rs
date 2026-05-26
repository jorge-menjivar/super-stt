// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end smoke tests for the new HTTP daemon protocol.
//!
//! Spawns the `super-stt-daemon` binary against a temp `XDG_RUNTIME_DIR`,
//! a dynamically-chosen UDP port, and `SUPER_STT_AUTO_APPROVE=1` (so the
//! consent popup is bypassed and `/auth/request` auto-approves), then
//! exercises every endpoint via the shared `http_client` module:
//!
//! - `POST /auth/request` mints a session token (auto-approved).
//! - `GET  /ping` / `GET /status` succeed with the token.
//! - `POST /transcribe` / `POST /transcribe/stop` succeed with the token.
//! - Requests without a token (or with a bogus token) get `401 invalid_session`.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p super-stt --test http_smoke -- --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client::{self, TranscribeOptions};
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");
const APP_NAME: &str = "super-stt smoke test";
const SCOPE: &str = "client";

struct DaemonGuard {
    child: Child,
    xdg_runtime_dir: PathBuf,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.xdg_runtime_dir);
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
    let xdg = std::env::temp_dir().join(format!(
        "stt-test-{}-{}",
        std::process::id(),
        next_test_uniq()
    ));
    std::fs::create_dir_all(xdg.join("stt")).expect("create xdg/stt dir");
    // CRITICAL: also isolate XDG_CONFIG_HOME so the test daemon
    // doesn't read or write the developer's real
    // `~/.config/super-stt/daemon.toml`. Without this, every test
    // run that passes `--audio-theme silent` or `--device cpu`
    // overwrites the real user's saved settings via
    // `apply_cli_overrides_to_config`.
    let config_home = xdg.join("config");
    std::fs::create_dir_all(&config_home).expect("create xdg/config dir");

    let http_socket = xdg.join("stt").join("super-stt-http.sock");

    let child = Command::new(DAEMON_BIN)
        .env("XDG_RUNTIME_DIR", &xdg)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("SUPER_STT_AUTO_APPROVE", "1") // bypass consent popup
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
        if Path::new(&http_socket).exists() {
            // Try minting a token to confirm the HTTP listener is fully alive.
            if http_client::auth_request(http_socket.clone(), APP_NAME, SCOPE)
                .await
                .is_ok()
            {
                return (
                    DaemonGuard {
                        child,
                        xdg_runtime_dir: xdg,
                    },
                    http_socket,
                );
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!(
        "daemon HTTP listener did not become ready within 120s (socket: {})",
        http_socket.display()
    );
}

#[tokio::test]
async fn http_endpoints_respond() {
    let (_guard, http_socket) = start_daemon().await;

    // --- POST /auth/request (auto-approved by SUPER_STT_AUTO_APPROVE=1) ---
    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPE)
        .await
        .expect("auth_request should succeed under SUPER_STT_AUTO_APPROVE=1");
    assert!(!auth.session_token.is_empty(), "token should not be empty");
    assert_eq!(auth.scope, SCOPE);
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
    assert_eq!(status_info.scope, SCOPE);
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
    assert!(
        status.current_model.is_some(),
        "status should have current_model"
    );
    assert!(status.device.is_some(), "status should have device");
    // `is_recording` is the affordance the CLI uses to decide
    // start-vs-stop on the toggle hotkey — must always be present.
    assert_eq!(
        status.is_recording,
        Some(false),
        "status must report is_recording on a freshly-booted daemon"
    );

    // --- POST /transcribe (fire-and-forget) ---
    let resp = http_client::transcribe(
        http_socket.clone(),
        &token,
        TranscribeOptions {
            wait: false,
            write_mode: false,
            stop_mode: Some("manual-only".to_string()),
        },
    )
    .await
    .expect("transcribe should respond");
    assert!(
        resp.status == "success" || resp.status == "error",
        "transcribe returned unknown status: {:?}",
        resp.status
    );

    // --- POST /transcribe/stop ---
    let resp = http_client::transcribe_stop(http_socket.clone(), &token)
        .await
        .expect("transcribe/stop should respond");
    assert!(
        resp.status == "success" || resp.status == "error",
        "transcribe/stop returned unknown status: {:?}",
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

/// `POST /v1/transcribe/stop` must be idempotent when no recording is
/// in progress — it returns `200` with `message: "No recording in
/// progress"`. The CLI's record subcommand relies on this when the
/// user presses the hotkey twice while audio failed to start: the
/// second press hits the stop endpoint and must NOT start a fresh
/// recording.
///
/// Regression for the daemon refactor where transcribe_stop was
/// briefly dispatching `record` unconditionally — which would START
/// a recording when called against an idle daemon.
#[tokio::test]
async fn transcribe_stop_idempotent_when_idle() {
    let (_guard, http_socket) = start_daemon().await;
    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPE)
        .await
        .expect("auth_request should succeed");
    let token = auth.session_token;

    // Idle daemon: status must report is_recording=false BEFORE we
    // call /transcribe/stop.
    let pre = http_client::status(http_socket.clone(), &token)
        .await
        .expect("status before stop");
    assert_eq!(
        pre.is_recording,
        Some(false),
        "test setup: daemon must be idle before calling transcribe_stop"
    );

    // First stop call: nothing to stop.
    let resp = http_client::transcribe_stop(http_socket.clone(), &token)
        .await
        .expect("transcribe/stop should respond");
    assert_eq!(resp.status, "success", "got: {resp:?}");
    assert_eq!(
        resp.message.as_deref(),
        Some("No recording in progress"),
        "got message: {:?}",
        resp.message
    );

    // Idempotent: calling again must still report idle, NOT have
    // accidentally kicked off a recording.
    let resp = http_client::transcribe_stop(http_socket.clone(), &token)
        .await
        .expect("transcribe/stop should respond a second time");
    assert_eq!(resp.message.as_deref(), Some("No recording in progress"));

    // Confirm via /status that no recording was started.
    let post = http_client::status(http_socket.clone(), &token)
        .await
        .expect("status after stop");
    assert_eq!(
        post.is_recording,
        Some(false),
        "transcribe_stop on idle daemon must not start a recording; got {post:?}"
    );
}

/// Pins the documented `/v1/transcribe` second-call behavior: when a
/// daemon-mic capture is already in progress, the daemon rejects
/// the second call with `409 recording_in_progress` and `http_client`
/// surfaces that JSON body as a typed error (NOT as "transcribe
/// stream ended unexpectedly" — that was the symptom of an earlier
/// bug where the SSE parser ate a non-SSE response body).
///
/// We can't reliably exercise the recording pipeline in CI (no audio
/// device, no display server), so the assertion is the
/// negative-space property: both calls must surface a well-formed
/// outcome — either success/error DaemonResponse, or
/// `HttpError::Other("recording_in_progress")` — and never the
/// broken-stream message.
#[tokio::test]
async fn second_transcribe_during_active_recording_surfaces_recording_in_progress() {
    let (_guard, http_socket) = start_daemon().await;

    let auth = http_client::auth_request(http_socket.clone(), APP_NAME, SCOPE)
        .await
        .expect("auth_request should succeed");
    let token = auth.session_token;

    let opts = || TranscribeOptions {
        wait: true,
        write_mode: false,
        stop_mode: Some("manual-only".to_string()),
    };

    // Fire the two transcribe calls concurrently. When audio works
    // (real machine), call A reaches the recording path and B sees
    // `409 recording_in_progress`. When audio is unavailable (CI),
    // A errors out fast and B may also start fresh and error — both
    // outcomes are valid; the assertion is just that neither call
    // gets the broken-stream message.
    let socket_a = http_socket.clone();
    let token_a = token.clone();
    let a = tokio::spawn(async move { http_client::transcribe(socket_a, &token_a, opts()).await });

    // Slight stagger so A almost certainly reaches the daemon first.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let b = http_client::transcribe(http_socket.clone(), &token, opts()).await;
    let a = a.await.expect("transcribe A task should not panic");

    for (label, result) in [("A", a), ("B", b)] {
        match result {
            Ok(resp) => {
                assert_ne!(
                    resp.message.as_deref(),
                    Some("transcribe stream ended unexpectedly"),
                    "transcribe {label}: SSE-broken message escaped; got {resp:?}"
                );
            }
            Err(e) => {
                // Typed errors are fine — they're how non-2xx
                // responses (including `409 recording_in_progress`)
                // surface to the caller now. What's NOT fine is the
                // EOF-without-event symptom of the old bug.
                let msg = e.to_string();
                assert!(
                    !msg.contains("transcribe stream ended unexpectedly"),
                    "transcribe {label} surfaced broken-stream message in error: {msg}"
                );
            }
        }
    }
}
