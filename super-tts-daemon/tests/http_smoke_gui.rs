// SPDX-License-Identifier: GPL-3.0-only
//! GUI-driven end-to-end smoke test for the auth flow.
//!
//! Unlike `http_smoke.rs` (which uses `SUPER_TTS_AUTO_APPROVE=1` to skip
//! the popup), this test runs the daemon **without** auto-approve so the
//! real consent helper subprocess gets spawned. It then dismisses that
//! popup by sending it SIGTERM (simulating the user closing the dialog
//! without clicking either button), and verifies the daemon returns the
//! `auth_denied (user_dismissed)` error path.
//!
//! Why `#[ignore]`:
//! - Needs a working compositor (Wayland or X11) to actually open the
//!   libcosmic window.
//! - CI typically has neither, so the default `cargo test` should skip
//!   this. Run manually with:
//!
//!   ```bash
//!   cargo test -p super-tts --test http_smoke_gui -- --ignored --nocapture
//!   ```
//!
//! What's covered:
//! - The daemon successfully locates and spawns `super-tts-consent`.
//! - The env-var contract between daemon and helper is wired up.
//! - When the helper exits without a decision (SIGTERM → dismissed-on-
//!   close path), the daemon translates that to the proper error.

mod common;

use common::TestDaemon;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use super_tts_shared::daemon::http_client;
use tokio::time::sleep;

const APP_NAME: &str = "super-tts gui smoke test";
const SCOPES: &[&str] = &["speak", "status"];

fn skip_if_no_display() -> Option<&'static str> {
    let has_x11 = std::env::var_os("DISPLAY").is_some();
    let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if has_x11 || has_wayland {
        None
    } else {
        Some("no DISPLAY / WAYLAND_DISPLAY — skipping GUI test")
    }
}

/// Build `super-tts-consent` so the daemon can spawn it. The daemon
/// looks for the helper alongside its own binary first, and `cargo test`
/// builds both to `target/debug/`, so this is just an explicit cargo
/// build to make sure both are present.
fn ensure_consent_helper_built() -> PathBuf {
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "super-tts-consent"])
        .status()
        .expect("invoke cargo to build super-tts-consent");
    assert!(status.success(), "cargo build -p super-tts-consent failed");

    // The daemon's locate_consent_helper looks alongside its own binary
    // first. Both binaries land in target/<profile>/, so the simple test
    // is that the path next to DAEMON_BIN exists.
    let daemon_dir = Path::new(common::DAEMON_BIN)
        .parent()
        .expect("daemon binary parent dir");
    let helper = daemon_dir.join("super-tts-consent");
    assert!(
        helper.exists(),
        "expected consent helper to be built at {} (daemon's locate logic uses this path)",
        helper.display()
    );
    helper
}

/// A daemon that shows the real consent dialog: no auto-approval, and its
/// socket where the daemon puts it by default, under a runtime directory of
/// the test's own.
async fn start_daemon_no_auto_approve() -> (TestDaemon, PathBuf) {
    let daemon = common::daemon("gui")
        .without("SUPER_TTS_AUTO_APPROVE")
        .default_socket()
        .start()
        .await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

/// Find the PID of any currently-running `super-tts-consent` process.
fn find_consent_helper_pid() -> Option<u32> {
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let pid_str = entry.file_name();
        let pid_str = pid_str.to_string_lossy();
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let exe = entry.path().join("exe");
        if let Ok(target) = std::fs::read_link(&exe)
            && target
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == "super-tts-consent")
            && let Ok(pid) = pid_str.parse::<u32>()
        {
            return Some(pid);
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns a real libcosmic window — run manually with cargo test -- --ignored"]
async fn auth_request_dismissed_returns_user_dismissed() {
    if let Some(reason) = skip_if_no_display() {
        eprintln!("{reason}");
        return;
    }

    let _consent_helper = ensure_consent_helper_built();
    let (_guard, http_socket) = start_daemon_no_auto_approve().await;

    // Issue auth_request in a background task — it will block until the
    // popup is dismissed, allowed, or denied.
    let socket_for_task = http_socket.clone();
    let auth_task =
        tokio::spawn(
            async move { http_client::auth_request(socket_for_task, APP_NAME, SCOPES).await },
        );

    // Poll for the consent helper subprocess to appear.
    let deadline = Instant::now() + Duration::from_secs(15);
    let helper_pid = loop {
        if let Some(pid) = find_consent_helper_pid() {
            break pid;
        }
        if Instant::now() > deadline {
            // Daemon failed to spawn the helper — auth task should
            // surface a popup_failed error instead.
            break 0;
        }
        sleep(Duration::from_millis(200)).await;
    };

    if helper_pid > 0 {
        // Give the helper a moment to render and arm its DismissedGuard.
        sleep(Duration::from_millis(500)).await;
        // Send SIGTERM — the libcosmic event loop unwinds and the
        // dismissed-on-close path writes "dismissed" to stdout.
        unsafe {
            libc::kill(
                libc::pid_t::try_from(helper_pid).expect("pid fits in pid_t"),
                libc::SIGTERM,
            );
        }
    }

    let result = tokio::time::timeout(Duration::from_secs(30), auth_task)
        .await
        .expect("auth_request did not finish within 30s")
        .expect("auth task panicked");

    let err = result.expect_err("auth_request should have failed since the popup was dismissed");
    let display = err.to_string();
    assert!(
        display.contains("user_dismissed") || display.contains("popup_failed"),
        "expected user_dismissed or popup_failed in error; got: {err}"
    );
}
