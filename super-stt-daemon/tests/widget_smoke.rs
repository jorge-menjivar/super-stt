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

use common::TestDaemon;

use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::session::{self, AppId};
use super_stt_shared::daemon::widget_subscription::{
    DEFAULT_INITIAL_BACKOFF, DEFAULT_MAX_BACKOFF, WidgetSubscriptionConfig,
    WidgetSubscriptionUpdate, run_widget_subscription,
};

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
const APP_ID_RESTART: AppId = session::app_id("widget-smoke-restart");
const APP_ID_IDLE: AppId = session::app_id("widget-smoke-idle");
const APP_ID_INVALID: AppId = session::app_id("widget-smoke-invalid");
const TEST_APP_NAME: &str = "widget-smoke-test";
const TEST_SCOPES: &[&str] = &["recording_events", "audio_visualization"];
const TEST_TOPICS: &[&str] = &["recording_state", "frequency_bands"];

/// Forget the test's session entry on exit. With the mock keyring installed
/// this never reaches the developer's secret service, but the *cache* it also
/// clears is process-wide, so dropping it still matters.
struct KeyringCleanupGuard(AppId);
impl Drop for KeyringCleanupGuard {
    fn drop(&mut self) {
        let _ = session::forget(self.0);
    }
}

/// A socket path in a directory of the test's own, removed with it. Outside
/// any daemon's home, so a daemon restarted onto it outlives the one before.
fn socket_in_temp_dir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create a socket dir");
    let socket = dir.path().join("http.sock");
    (dir, socket)
}

/// Start a hermetic daemon on `http_socket`. See `super_engine_test_daemon`.
async fn start_daemon(http_socket: &Path) -> TestDaemon {
    common::daemon("widget")
        .socket_at(http_socket)
        .start()
        .await
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
/// `SUPER_STT_KEYRING_MOCK=1`, which every test daemon is started with, and
/// the client side of the mint is routed by [`common::install_mock_keyring`].
/// Neither side reaches the real secret service, so it runs with the suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscription_recovers_from_daemon_restart() {
    common::install_mock_keyring();
    let _keyring_guard = KeyringCleanupGuard(APP_ID_RESTART);
    let (_socket_dir, http_socket) = socket_in_temp_dir();

    // 1. Boot the daemon and confirm /auth/request works under
    //    SUPER_STT_AUTO_APPROVE so the subscription's session::obtain
    //    will succeed silently.
    let mut guard = start_daemon(&http_socket).await;

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
    let pid = guard.child().id();
    eprintln!("[smoke] killing daemon pid={pid}");
    let _ = guard.child().kill();
    let _ = guard.child().wait();

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
    // Assigning over `guard` drops the old one, which removes the home the
    // stopped daemon was using. The restart gets a fresh one, on the same
    // socket.
    guard = start_daemon(&http_socket).await;

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
    let (_socket_dir, http_socket) = socket_in_temp_dir();

    let _guard = start_daemon(&http_socket).await;

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
    let (_socket_dir, http_socket) = socket_in_temp_dir();

    let _guard = start_daemon(&http_socket).await;

    // Plant a token the daemon has never seen. The first
    // `events_stream` call will get 401 invalid_session; the helper
    // must `session::forget` and re-`obtain` (which under
    // SUPER_STT_AUTO_APPROVE returns a fresh real token). It is planted
    // as minted for `TEST_SCOPES`, or `obtain` would replace it before the
    // daemon ever saw it and the 401 path would go untested.
    let granted: Vec<String> = TEST_SCOPES.iter().map(|s| (*s).to_string()).collect();
    session::save(
        APP_ID_INVALID,
        "deadbeef_never_minted_by_daemon",
        TEST_SCOPES,
        &granted,
    )
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
