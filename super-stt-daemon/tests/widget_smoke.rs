// SPDX-License-Identifier: GPL-3.0-only
//! Widget `/events` SSE subscription smoke test.
//!
//! Validates that the shared `run_widget_subscription` helper actually
//! recovers from a daemon restart end-to-end — i.e. the applet won't
//! get permanently stuck on stale data when the daemon goes away.
//!
//! Hermetic: the daemon gets isolated XDG dirs and an in-memory keyring, and
//! the client side of the session mint is mocked too, so this runs as part of
//! the ordinary suite rather than needing a desktop session.

mod common;

use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use super_stt_shared::daemon::session::{self, AppId};
use super_stt_shared::daemon::widget_subscription::{
    DEFAULT_INITIAL_BACKOFF, DEFAULT_MAX_BACKOFF, WidgetSubscriptionConfig,
    WidgetSubscriptionUpdate, run_widget_subscription,
};
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");

/// One `AppId` per test, because the session cache they share is
/// process-wide.
///
/// `session::save` writes an in-memory cache that `obtain` consults before the
/// keyring, and it is keyed by `AppId` alone. With a single id across the file,
/// `subscription_recovers_from_invalid_session` — which deliberately plants a
/// token the daemon has never seen — would hand that dead token to whichever
/// other test looked next. These ran `--test-threads=1` while they were
/// `#[ignore]`d, which hid it; running with the suite does not.
///
/// Each is stable across a restart *within* its own test, which is what
/// `subscription_recovers_from_daemon_restart` needs to verify.
const APP_ID_RESTART: AppId = AppId("widget-smoke-restart");
const APP_ID_IDLE: AppId = AppId("widget-smoke-idle");
const APP_ID_INVALID: AppId = AppId("widget-smoke-invalid");
const TEST_APP_NAME: &str = "widget-smoke-test";
const TEST_SCOPES: &[&str] = &["recording_events", "audio_visualization"];
const TEST_TOPICS: &[&str] = &["recording_state", "frequency_bands"];

struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl DaemonGuard {
    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        common::shutdown(&mut self.child);
        for p in &self.cleanup_paths {
            // Sockets are files, the XDG homes are directories; whichever does
            // not apply is a no-op.
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

/// Forget the test's session entry on exit. With the mock keyring installed
/// this never reaches the developer's secret service, but the *cache* it also
/// clears is process-wide, so dropping it still matters.
struct KeyringCleanupGuard(AppId);
impl Drop for KeyringCleanupGuard {
    fn drop(&mut self) {
        let _ = session::forget(self.0);
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

fn unique_socket_paths(label: &str) -> (PathBuf, PathBuf) {
    let unique = format!("stt-{label}-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    (
        tmp.join(format!("{unique}-legacy.sock")),
        tmp.join(format!("{unique}-http.sock")),
    )
}

/// Spawn a hermetic daemon. Returns the child and the XDG dirs it was given, so
/// the caller's guard can sweep them.
///
/// All three dirs are isolated, not just the config: an unisolated
/// `XDG_DATA_HOME` has the daemon discover whatever backends the developer has
/// installed (so the test is not hermetic and not reproducible), and an
/// unisolated `XDG_CACHE_HOME` has it read and overwrite the developer's real
/// registry index.
fn spawn_daemon(_legacy_socket: &Path, http_socket: &Path) -> (Child, Vec<PathBuf>) {
    let unique = format!("stt-widget-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));
    let cache_home = tmp.join(format!("{unique}-cache"));
    for d in [&config_home, &data_home, &cache_home] {
        std::fs::create_dir_all(d).expect("create test xdg dir");
    }

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1") // in-memory keyring (no secret-service prompt in tests/CI)
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CACHE_HOME", &cache_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    (child, vec![config_home, data_home, cache_home])
}

async fn wait_for_daemon_ready(http_socket: &Path) {
    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if http_socket.exists()
            && http_client::auth_request(http_socket.to_path_buf(), TEST_APP_NAME, TEST_SCOPES)
                .await
                .is_ok()
        {
            return;
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!(
        "daemon HTTP listener did not become ready within 120s (socket: {})",
        http_socket.display()
    );
}

/// Pull updates from the subscription stream until either we hit the
/// predicate or we exhaust the deadline. Returns `Some(update)` on
/// match, `None` on deadline. Drains all updates in between and
/// returns them so the caller can inspect the sequence.
async fn drain_until<F>(
    stream: &mut std::pin::Pin<
        Box<dyn futures_util::Stream<Item = WidgetSubscriptionUpdate> + Send + 'static>,
    >,
    deadline: Duration,
    matches: F,
) -> Option<Vec<WidgetSubscriptionUpdate>>
where
    F: Fn(&WidgetSubscriptionUpdate) -> bool,
{
    let start = Instant::now();
    let mut seen = Vec::new();
    while start.elapsed() < deadline {
        let remaining = deadline.checked_sub(start.elapsed()).unwrap();
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(update)) => {
                let hit = matches(&update);
                seen.push(update);
                if hit {
                    return Some(seen);
                }
            }
            // Stream ended unexpectedly, or the deadline hit.
            Ok(None) | Err(_) => return None,
        }
    }
    None
}

/// End-to-end: start daemon → open subscription → wait for `Connected`
/// → kill daemon → wait for `Disconnected` → restart daemon → wait for
/// the *second* `Connected`. Any of those phases failing means the
/// applet would be stuck on stale data after a daemon restart.
///
/// This once carried `#[ignore]`, on the grounds that minting a session writes
/// to the developer's system keyring and a locked secret-service would hang —
/// "there's no infrastructure for an in-process keyring shim from an
/// integration test crate". There is: the daemon takes
/// `SUPER_STT_KEYRING_MOCK=1`, which `spawn_daemon` has set for a while, and
/// the client side of the mint is routed by [`common::install_mock_keyring`].
/// Neither side reaches the real secret service, so it runs with the suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_recovers_from_daemon_restart() {
    common::install_mock_keyring();
    let _keyring_guard = KeyringCleanupGuard(APP_ID_RESTART);
    let (legacy_socket, http_socket) = unique_socket_paths("widget-restart");

    // 1. Boot the daemon and confirm /auth/request works under
    //    SUPER_STT_AUTO_APPROVE so the subscription's session::obtain
    //    will succeed silently.
    let (child, xdg_dirs) = spawn_daemon(&legacy_socket, &http_socket);
    let mut guard = DaemonGuard {
        child,
        cleanup_paths: [vec![legacy_socket.clone(), http_socket.clone()], xdg_dirs].concat(),
    };
    wait_for_daemon_ready(&http_socket).await;

    // 2. Drive the subscription with tight timings so the test doesn't
    //    sit on the default backoff for tens of seconds.
    let mut config =
        WidgetSubscriptionConfig::new(_keyring_guard.0, TEST_APP_NAME, TEST_SCOPES, TEST_TOPICS);
    config.idle_timeout = Duration::from_secs(5);
    config.initial_backoff = DEFAULT_INITIAL_BACKOFF;
    config.max_backoff = DEFAULT_MAX_BACKOFF;

    let mut stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = WidgetSubscriptionUpdate> + Send + 'static>,
    > = Box::pin(run_widget_subscription(http_socket.clone(), config));

    // 3. Wait for the first Connected. This proves auth + subscribe worked.
    let pre = drain_until(&mut stream, Duration::from_secs(15), |u| {
        matches!(u, WidgetSubscriptionUpdate::Connected)
    })
    .await
    .expect("expected first WidgetSubscriptionUpdate::Connected within 15s");
    assert!(
        matches!(pre.last(), Some(WidgetSubscriptionUpdate::Connected)),
        "first phase did not end on Connected: {pre:?}"
    );

    // 4. Kill the daemon mid-stream. The shared helper must observe
    //    the drop (via stream EOF or read error) and emit Disconnected.
    let pid = guard.pid();
    eprintln!("[smoke] killing daemon pid={pid}");
    let _ = guard.child.kill();
    let _ = guard.child.wait();

    let mid = drain_until(&mut stream, Duration::from_secs(15), |u| {
        matches!(u, WidgetSubscriptionUpdate::Disconnected { .. })
    })
    .await
    .expect("expected WidgetSubscriptionUpdate::Disconnected after daemon kill");
    assert!(
        matches!(
            mid.last(),
            Some(WidgetSubscriptionUpdate::Disconnected { .. })
        ),
        "drop phase did not end on Disconnected: {mid:?}"
    );

    // 5. Restart the daemon. The subscription should reconnect within
    //    a backoff window without external intervention.
    eprintln!("[smoke] restarting daemon");
    // Assigning over `guard` drops the old one, which sweeps the dirs the
    // stopped daemon was using. The restart gets a fresh set.
    let (child, xdg_dirs) = spawn_daemon(&legacy_socket, &http_socket);
    guard = DaemonGuard {
        child,
        cleanup_paths: [vec![legacy_socket.clone(), http_socket.clone()], xdg_dirs].concat(),
    };
    wait_for_daemon_ready(&http_socket).await;

    // 6. Wait for the SECOND Connected — this is the actual "no stale
    //    widget" assertion. If `run_widget_subscription` had given up,
    //    we'd time out here.
    let post = drain_until(&mut stream, Duration::from_mins(1), |u| {
        matches!(u, WidgetSubscriptionUpdate::Connected)
    })
    .await
    .expect("expected second WidgetSubscriptionUpdate::Connected within 60s after daemon restart");
    assert!(
        matches!(post.last(), Some(WidgetSubscriptionUpdate::Connected)),
        "post-restart phase did not end on Connected: {post:?}"
    );

    drop(stream);
    drop(guard);
}

/// Idle-timeout sanity: if the daemon doesn't send anything within
/// the configured idle window, the subscription must surface a
/// `Disconnected { reason: idle_timeout(...) }` rather than block
/// forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_emits_idle_timeout_when_daemon_goes_quiet() {
    common::install_mock_keyring();
    let _keyring_guard = KeyringCleanupGuard(APP_ID_IDLE);
    let (legacy_socket, http_socket) = unique_socket_paths("widget-idle");

    let (child, xdg_dirs) = spawn_daemon(&legacy_socket, &http_socket);
    let _guard = DaemonGuard {
        child,
        cleanup_paths: [vec![legacy_socket.clone(), http_socket.clone()], xdg_dirs].concat(),
    };
    wait_for_daemon_ready(&http_socket).await;

    // Tight idle timeout — the daemon's first keepalive is 30 s out
    // and there are no recordings in flight, so a 2 s deadline will
    // fire before any natural traffic.
    let mut config =
        WidgetSubscriptionConfig::new(_keyring_guard.0, TEST_APP_NAME, TEST_SCOPES, TEST_TOPICS);
    config.idle_timeout = Duration::from_secs(2);

    let mut stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = WidgetSubscriptionUpdate> + Send + 'static>,
    > = Box::pin(run_widget_subscription(http_socket.clone(), config));

    // First: connect.
    drain_until(&mut stream, Duration::from_secs(15), |u| {
        matches!(u, WidgetSubscriptionUpdate::Connected)
    })
    .await
    .expect("expected initial Connected");

    // Then: idle timeout fires (daemon never publishes during the test).
    let drained = drain_until(&mut stream, Duration::from_secs(10), |u| {
        matches!(
            u,
            WidgetSubscriptionUpdate::Disconnected { reason } if reason.contains("idle_timeout")
        )
    })
    .await
    .expect("expected Disconnected{idle_timeout(...)} within 10s of going quiet");
    assert!(
        drained
            .iter()
            .any(|u| matches!(u, WidgetSubscriptionUpdate::Disconnected { reason } if reason.contains("idle_timeout"))),
        "missing idle_timeout disconnect: {drained:?}"
    );

    drop(stream);
}

/// `invalid_session` recovery: forge a stale token in the keyring and
/// confirm the subscription drops it (`session::forget`) and re-mints
/// via the consent path on the next iteration. Without this fix the
/// subscription would loop forever on the same dead token.
///
/// The planted token rides the process-wide session cache, which is why this
/// test needs an `AppId` of its own — see [`APP_ID_INVALID`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_recovers_from_invalid_session() {
    common::install_mock_keyring();
    let _keyring_guard = KeyringCleanupGuard(APP_ID_INVALID);
    let (legacy_socket, http_socket) = unique_socket_paths("widget-invalid");

    let (child, xdg_dirs) = spawn_daemon(&legacy_socket, &http_socket);
    let _guard = DaemonGuard {
        child,
        cleanup_paths: [vec![legacy_socket.clone(), http_socket.clone()], xdg_dirs].concat(),
    };
    wait_for_daemon_ready(&http_socket).await;

    // Plant a token the daemon has never seen. The first
    // `events_stream` call will get 401 invalid_session; the helper
    // must `session::forget` and re-`obtain` (which under
    // SUPER_STT_AUTO_APPROVE returns a fresh real token).
    session::save(APP_ID_INVALID, "deadbeef_never_minted_by_daemon")
        .expect("plant fake token in keyring");

    let mut config =
        WidgetSubscriptionConfig::new(_keyring_guard.0, TEST_APP_NAME, TEST_SCOPES, TEST_TOPICS);
    config.idle_timeout = Duration::from_secs(5);

    let mut stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = WidgetSubscriptionUpdate> + Send + 'static>,
    > = Box::pin(run_widget_subscription(http_socket.clone(), config));

    // Sequence we expect: NeedsReauth (the daemon rejected the planted
    // token) → Connected (the helper forgot+reobtained successfully).
    let drained = drain_until(&mut stream, Duration::from_secs(15), |u| {
        matches!(u, WidgetSubscriptionUpdate::Connected)
    })
    .await
    .expect("expected Connected after the helper re-auths past the planted invalid session");

    assert!(
        drained
            .iter()
            .any(|u| matches!(u, WidgetSubscriptionUpdate::NeedsReauth { .. })),
        "expected at least one NeedsReauth before Connected, got: {drained:?}"
    );

    drop(stream);
}
