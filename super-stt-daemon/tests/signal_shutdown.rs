// SPDX-License-Identifier: GPL-3.0-only
//! The daemon must shut down gracefully on the signals that actually reach it.
//!
//! `systemctl stop` and a plain `kill` send SIGTERM. A daemon that watched only
//! for SIGINT died to SIGTERM's default disposition, skipping the path that
//! stops the `systemd-run --user` backend unit — and since that unit is not a
//! child in the daemon's cgroup, nothing else reaps it. The result was a
//! subprocess backend left running with a whole model resident.
//!
//! These tests read the exit status, which is what separates the two cases: a
//! graceful stop reaches `std::process::exit(0)`, while a default-disposition
//! kill leaves the process signalled, with no exit code at all.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");

/// Stops the daemon if a test failed before it could exit on its own, and
/// removes the test's directories.
struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        common::shutdown(&mut self.child);
        for p in &self.cleanup_paths {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

fn next_test_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static UNIQ: AtomicU64 = AtomicU64::new(0);
    UNIQ.fetch_add(1, Ordering::Relaxed)
}

/// Spawn a daemon against isolated XDG dirs and wait for its socket.
///
/// `XDG_DATA_HOME` is isolated so no backend is discovered: these tests are
/// about the signal path, and a real backend would make them depend on a
/// systemd user session.
async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let unique = format!("stt-signal-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));
    let cache_home = tmp.join(format!("{unique}-cache"));
    for dir in [&config_home, &data_home, &cache_home] {
        std::fs::create_dir_all(dir).expect("create test dir");
    }

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    // Hand the child to the guard before the readiness loop, so the timeout
    // panic below still stops it.
    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone(), config_home, data_home, cache_home],
    };

    // The handlers are installed before the listener binds, so the socket
    // appearing means a signal from here on is handled.
    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists() {
            return (guard, http_socket);
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "daemon did not bind its socket within 120s ({})",
        http_socket.display()
    );
}

/// Send `signal` to the daemon and wait for it to exit.
///
/// Returns the exit status, or `None` if it was still running at the deadline.
async fn signal_and_wait(
    guard: &mut DaemonGuard,
    signal: i32,
    timeout: Duration,
) -> Option<ExitStatus> {
    let pid = i32::try_from(guard.child.id()).expect("pid fits i32");
    // SAFETY: `pid` is this test's own child, still unreaped, so the id cannot
    // have been recycled onto an unrelated process.
    let sent = unsafe { libc::kill(pid, signal) };
    assert_eq!(sent, 0, "kill({pid}, {signal}) failed");

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match guard.child.try_wait().expect("try_wait") {
            Some(status) => return Some(status),
            None => sleep(Duration::from_millis(100)).await,
        }
    }
    None
}

/// The regression this file exists for. Before the fix the daemon exited
/// *signalled* rather than with a code, because SIGTERM fell through to the
/// default disposition and the graceful path never ran.
#[tokio::test]
async fn sigterm_shuts_down_gracefully() {
    use std::os::unix::process::ExitStatusExt;

    let (mut guard, _socket) = start_daemon().await;
    let status = signal_and_wait(&mut guard, libc::SIGTERM, Duration::from_secs(30))
        .await
        .expect("daemon must exit within 30s of SIGTERM");

    assert_eq!(
        status.signal(),
        None,
        "SIGTERM killed the daemon outright instead of being handled; \
         the graceful path (which stops the backend unit) never ran"
    );
    assert_eq!(
        status.code(),
        Some(0),
        "graceful shutdown must exit 0, got {status:?}"
    );
}

/// SIGINT already worked; it must keep working now that it shares the wait.
#[tokio::test]
async fn sigint_still_shuts_down_gracefully() {
    use std::os::unix::process::ExitStatusExt;

    let (mut guard, _socket) = start_daemon().await;
    let status = signal_and_wait(&mut guard, libc::SIGINT, Duration::from_secs(30))
        .await
        .expect("daemon must exit within 30s of SIGINT");

    assert_eq!(status.signal(), None, "SIGINT was not handled: {status:?}");
    assert_eq!(
        status.code(),
        Some(0),
        "expected a clean exit, got {status:?}"
    );
}

/// A second witness that the graceful path ran: the listener task unlinks its
/// socket on the way out, which a killed process never does.
#[tokio::test]
async fn graceful_shutdown_removes_the_listener_socket() {
    let (mut guard, socket) = start_daemon().await;
    assert!(socket.exists(), "socket should exist while running");

    signal_and_wait(&mut guard, libc::SIGTERM, Duration::from_secs(30))
        .await
        .expect("daemon must exit within 30s of SIGTERM");

    assert!(
        !socket.exists(),
        "graceful shutdown left {} behind, so the listener's cleanup never ran",
        socket.display()
    );
}
