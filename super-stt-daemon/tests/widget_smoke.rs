// SPDX-License-Identifier: GPL-3.0-only
//! Widget `/events` SSE subscription smoke test.
//!
//! Validates that the shared `run_widget_subscription` helper actually
//! recovers from a daemon restart end-to-end — i.e. the applet won't
//! get permanently stuck on stale data when the daemon goes away.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p super-stt-daemon --test widget_smoke -- --nocapture
//! ```

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

/// Stable AppId used across the test run. We don't need a per-run
/// unique id here: the daemon-side persisted token survives restart by
/// design, and that's exactly what we want to verify. We do
/// `session::forget` in the test's drop guard so we don't leak entries
/// on the developer's keyring.
const TEST_APP_ID: AppId = AppId("widget-smoke-test");
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
        let _ = self.child.kill();
        let _ = self.child.wait();
        for p in &self.cleanup_paths {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Forget the test's keyring entry on test exit so we don't leak.
struct KeyringCleanupGuard;
impl Drop for KeyringCleanupGuard {
    fn drop(&mut self) {
        let _ = session::forget(TEST_APP_ID);
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

async fn spawn_daemon(_legacy_socket: &Path, http_socket: &Path) -> Child {
    // Isolate XDG_CONFIG_HOME so the test daemon doesn't overwrite
    // the developer's real config when applying `--audio-theme` /
    // `--device` CLI overrides.
    let config_home = std::env::temp_dir().join(format!(
        "stt-widget-cfg-{}-{}",
        std::process::id(),
        next_test_uniq()
    ));
    std::fs::create_dir_all(&config_home).expect("create test config dir");

    Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1") // in-memory keyring (no secret-service prompt in tests/CI)
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon")
}

async fn wait_for_daemon_ready(http_socket: &Path) {
    let deadline = Instant::now() + Duration::from_secs(120);
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
        let remaining = deadline - start.elapsed();
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(update)) => {
                let hit = matches(&update);
                seen.push(update);
                if hit {
                    return Some(seen);
                }
            }
            Ok(None) => return None, // stream ended unexpectedly
            Err(_) => return None,   // deadline hit
        }
    }
    None
}

/// End-to-end: start daemon → open subscription → wait for `Connected`
/// → kill daemon → wait for `Disconnected` → restart daemon → wait for
/// the *second* `Connected`. Any of those phases failing means the
/// applet would be stuck on stale data after a daemon restart.
///
/// `#[ignore]` because this spawns the real daemon, which writes to
/// the developer's system keyring on each mint. Run explicitly with:
///
/// ```bash
/// cargo test -p super-stt-daemon --test widget_smoke -- --ignored --test-threads=1
/// ```
///
/// (A locked or unresponsive secret-service will hang the daemon's
/// first session-mint flush — there's no infrastructure for an
/// in-process keyring shim from an integration test crate.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns real daemon; requires a responsive system keyring"]
async fn subscription_recovers_from_daemon_restart() {
    let _keyring_guard = KeyringCleanupGuard;
    let (legacy_socket, http_socket) = unique_socket_paths("widget-restart");

    // 1. Boot the daemon and confirm /auth/request works under
    //    SUPER_STT_AUTO_APPROVE so the subscription's session::obtain
    //    will succeed silently.
    let mut guard = DaemonGuard {
        child: spawn_daemon(&legacy_socket, &http_socket).await,
        cleanup_paths: vec![legacy_socket.clone(), http_socket.clone()],
    };
    wait_for_daemon_ready(&http_socket).await;

    // 2. Drive the subscription with tight timings so the test doesn't
    //    sit on the default backoff for tens of seconds.
    let mut config =
        WidgetSubscriptionConfig::new(TEST_APP_ID, TEST_APP_NAME, TEST_SCOPES, TEST_TOPICS);
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
    guard = DaemonGuard {
        child: spawn_daemon(&legacy_socket, &http_socket).await,
        cleanup_paths: vec![legacy_socket.clone(), http_socket.clone()],
    };
    wait_for_daemon_ready(&http_socket).await;

    // 6. Wait for the SECOND Connected — this is the actual "no stale
    //    widget" assertion. If `run_widget_subscription` had given up,
    //    we'd time out here.
    let post = drain_until(&mut stream, Duration::from_secs(60), |u| {
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
/// forever. See note on `subscription_recovers_from_daemon_restart`
/// re: `#[ignore]`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns real daemon; requires a responsive system keyring"]
async fn subscription_emits_idle_timeout_when_daemon_goes_quiet() {
    let _keyring_guard = KeyringCleanupGuard;
    let (legacy_socket, http_socket) = unique_socket_paths("widget-idle");

    let _guard = DaemonGuard {
        child: spawn_daemon(&legacy_socket, &http_socket).await,
        cleanup_paths: vec![legacy_socket.clone(), http_socket.clone()],
    };
    wait_for_daemon_ready(&http_socket).await;

    // Tight idle timeout — the daemon's first keepalive is 30 s out
    // and there are no recordings in flight, so a 2 s deadline will
    // fire before any natural traffic.
    let mut config =
        WidgetSubscriptionConfig::new(TEST_APP_ID, TEST_APP_NAME, TEST_SCOPES, TEST_TOPICS);
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
/// subscription would loop forever on the same dead token. See note on
/// `subscription_recovers_from_daemon_restart` re: `#[ignore]`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spawns real daemon + writes to system keyring; requires responsive secret-service"]
async fn subscription_recovers_from_invalid_session() {
    let _keyring_guard = KeyringCleanupGuard;
    let (legacy_socket, http_socket) = unique_socket_paths("widget-invalid");

    let _guard = DaemonGuard {
        child: spawn_daemon(&legacy_socket, &http_socket).await,
        cleanup_paths: vec![legacy_socket.clone(), http_socket.clone()],
    };
    wait_for_daemon_ready(&http_socket).await;

    // Plant a token the daemon has never seen. The first
    // `events_stream` call will get 401 invalid_session; the helper
    // must `session::forget` and re-`obtain` (which under
    // SUPER_STT_AUTO_APPROVE returns a fresh real token).
    session::save(TEST_APP_ID, "deadbeef_never_minted_by_daemon")
        .expect("plant fake token in keyring");

    let mut config =
        WidgetSubscriptionConfig::new(TEST_APP_ID, TEST_APP_NAME, TEST_SCOPES, TEST_TOPICS);
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
