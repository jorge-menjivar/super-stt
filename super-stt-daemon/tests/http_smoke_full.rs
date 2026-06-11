// SPDX-License-Identifier: GPL-3.0-only
//! Full end-to-end auth smoke test with the real consent helper.
//!
//! Unlike `http_smoke.rs` (which uses `SUPER_STT_AUTO_APPROVE=1` to
//! bypass the popup entirely) and `http_smoke_gui.rs` (which exercises
//! the *dismiss* path by SIGTERM'ing the helper), this test runs the
//! full chain:
//!
//! 1. Daemon starts WITHOUT `SUPER_STT_AUTO_APPROVE`, but WITH
//!    `STT_AUTH_AUTO_APPROVE_AFTER_MS=2000` in its environment.
//! 2. A test client calls `POST /auth/request`.
//! 3. The daemon spawns the real `super-stt-consent` helper, which
//!    inherits `STT_AUTH_AUTO_APPROVE_AFTER_MS` from the daemon's env.
//! 4. The helper renders the libcosmic layer-shell dialog (visible for
//!    ~5 seconds during the test), then writes `allow` to stdout via a
//!    background timer.
//! 5. The daemon mints a session token and returns it to the client.
//! 6. The client uses the token on `GET /ping` and `GET /status`.
//!
//! This is the most thorough smoke test — it validates the helper's
//! actual rendering path, the daemon ↔ helper IPC, the env-var
//! contract (including the auto-approve timer), and per-request token
//! validation, all in one run.
//!
//! `#[ignore]`'d by default because it needs a working compositor:
//!
//! ```bash
//! cargo test -p super-stt --test http_smoke_full -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");
const APP_NAME: &str = "super-stt full smoke test";
const SCOPES: &[&str] = &["transcribe", "status"];
const AUTO_APPROVE_MS: u64 = 5_000;

fn skip_if_no_display() -> Option<&'static str> {
    let has_x11 = std::env::var_os("DISPLAY").is_some();
    let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if has_x11 || has_wayland {
        None
    } else {
        Some("no DISPLAY / WAYLAND_DISPLAY — skipping GUI test")
    }
}

/// Build `super-stt-consent` and confirm it lives next to the daemon
/// binary (which is where the daemon's `locate_consent_helper` looks
/// first). `cargo test` builds both into the same `target/<profile>/`
/// directory, so this is just an explicit cargo build to be safe.
fn ensure_consent_helper_built() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "super-stt-consent"])
        .status()
        .expect("invoke cargo to build super-stt-consent");
    assert!(status.success(), "cargo build -p super-stt-consent failed");

    let daemon_dir = Path::new(DAEMON_BIN)
        .parent()
        .expect("daemon binary parent dir");
    let helper = daemon_dir.join("super-stt-consent");
    assert!(
        helper.exists(),
        "expected consent helper to be co-located with daemon at {} \
         (daemon's locate logic uses this path)",
        helper.display()
    );
    helper
}

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

async fn start_daemon_with_auto_approve_timer() -> (DaemonGuard, PathBuf) {
    // Use the user's real $XDG_RUNTIME_DIR so the consent helper can
    // still find the Wayland socket. We isolate by using unique daemon
    // socket paths (legacy via --socket, HTTP via SUPER_STT_HTTP_SOCKET)
    // rather than redirecting XDG_RUNTIME_DIR.
    let unique = format!("stt-full-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let legacy_socket = tmp.join(format!("{unique}-legacy.sock"));
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    // Isolate XDG_CONFIG_HOME so the test daemon doesn't overwrite
    // the developer's real config via `apply_cli_overrides_to_config`.
    let config_home = tmp.join(format!("{unique}-config"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");

    // Capture daemon stderr so we can diagnose hangs during dev.
    // Set SUPER_STT_TEST_LOG=1 to also surface it on the test runner's
    // stderr.
    let stderr_target = if std::env::var("SUPER_STT_TEST_LOG").is_ok() {
        Stdio::inherit()
    } else {
        Stdio::null()
    };

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1") // in-memory keyring (no secret-service prompt in tests/CI)
        // No SUPER_STT_AUTO_APPROVE — the daemon will spawn the popup.
        // The timer below makes the helper auto-approve so the test
        // doesn't hang waiting for human input.
        .env_remove("SUPER_STT_AUTO_APPROVE")
        .env(
            "STT_AUTH_AUTO_APPROVE_AFTER_MS",
            AUTO_APPROVE_MS.to_string(),
        )
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env(
            "RUST_LOG",
            "info,super_stt_daemon::daemon::http_server=debug",
        )
        .arg("--device")
        .arg("cpu")
        .arg("--audio-theme")
        .arg("silent")
        .stdout(Stdio::null())
        .stderr(stderr_target)
        .spawn()
        .expect("spawn super-stt-daemon");

    // Without auto-approve we can't probe by issuing /auth/request,
    // so just wait for the socket file to exist plus a short settle.
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists() {
            sleep(Duration::from_millis(200)).await;
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
    panic!(
        "daemon HTTP listener did not become ready within 120s (socket: {})",
        http_socket.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns a real libcosmic dialog — run manually with cargo test -- --ignored"]
async fn auth_request_real_helper_returns_working_token() {
    if let Some(reason) = skip_if_no_display() {
        eprintln!("{reason}");
        return;
    }

    let _consent_helper = ensure_consent_helper_built();
    let (_guard, http_socket) = start_daemon_with_auto_approve_timer().await;

    // 1. Trigger /auth/request. The daemon spawns the consent helper.
    //    The helper renders the dialog for ~AUTO_APPROVE_MS, then its
    //    background timer writes "allow" and exits.
    eprintln!(
        "[smoke] issuing auth_request — dialog should appear for ~{}s (or until you click Allow)...",
        AUTO_APPROVE_MS / 1_000
    );
    let started = Instant::now();
    let auth = tokio::time::timeout(
        Duration::from_secs(30),
        http_client::auth_request(http_socket.clone(), APP_NAME, SCOPES),
    )
    .await
    .expect("auth_request did not finish within 30s")
    .expect("auth_request should succeed (helper auto-approve, or human Allow click)");
    let elapsed = started.elapsed();
    eprintln!("[smoke] auth_request completed in {elapsed:?}");

    // Don't assert on timing — a successful return either means the
    // auto-approve timer fired (~AUTO_APPROVE_MS later) or the human
    // running this test clicked Allow earlier. Both are valid.
    assert!(
        !auth.session_token.is_empty(),
        "session token should not be empty"
    );
    assert!(
        SCOPES
            .iter()
            .all(|s| auth.scopes.iter().any(|g| g.as_str() == *s)),
        "granted scopes {:?} should cover requested {SCOPES:?}",
        auth.scopes
    );

    let token = auth.session_token;

    // 2. The minted token must work on protected endpoints.
    let pong = http_client::ping(http_socket.clone(), &token)
        .await
        .expect("ping should succeed with the new token");
    assert!(
        pong.to_lowercase().contains("pong") || pong.to_lowercase().contains("running"),
        "unexpected ping response: {pong}"
    );

    let status = http_client::status(http_socket.clone(), &token)
        .await
        .expect("status should succeed with the new token");
    assert_eq!(status.status, "success");
    assert!(status.current_model.is_some());
    assert!(status.device.is_some());

    // 3. A bogus token should still be rejected even after a real
    //    one was minted, confirming token validation isn't blanket-
    //    accepting.
    let unauthorized = http_client::ping(http_socket.clone(), "not-a-real-token").await;
    let err = unauthorized.expect_err("ping with bogus token should be rejected");
    assert!(
        err.is_invalid_session(),
        "expected InvalidSession variant, got: {err}"
    );

    // (We deliberately skip exercising /transcribe here — the HTTP
    // path runs a real recording inline and doesn't yet have a fire-
    // and-forget short-circuit, so it'd block the test for the full
    // recording timeout. Auth + ping + status + bogus-token rejection
    // is enough to validate the end-to-end auth flow.)
}
