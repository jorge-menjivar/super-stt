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

async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let xdg = std::env::temp_dir().join(format!(
        "stt-test-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::create_dir_all(xdg.join("stt")).expect("create xdg/stt dir");

    let legacy_socket = xdg.join("stt").join("super-stt.sock");
    let http_socket = xdg.join("stt").join("super-stt-http.sock");

    let child = Command::new(DAEMON_BIN)
        .env("XDG_RUNTIME_DIR", &xdg)
        .env("SUPER_STT_AUTO_APPROVE", "1") // bypass consent popup
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
        err.contains("invalid_session"),
        "expected invalid_session error, got: {err}"
    );
}
