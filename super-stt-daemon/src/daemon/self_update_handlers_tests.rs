// SPDX-License-Identifier: GPL-3.0-only
//! Coalescing side-effect gating: two overlapping
//! `run_self_update_check_and_notify` calls must not double-publish the
//! `UpdateAvailable` event or double-notify for the same version (task
//! review round 1, Important finding).

use crate::daemon::events::Topic;
use crate::daemon::types::test_daemon;
use crate::output::notification::Notifier;
use std::time::Duration;
use super_stt_shared::models::notification_method::NotificationMethod;

/// Serializes every test in this file that points the daemon's self-update
/// forge client at a mock server via `GITHUB_API_BASE`: the env var is
/// process-wide, and cargo runs test functions concurrently on separate OS
/// threads, so two such tests could otherwise stomp each other's mock URL
/// mid-check. Held for the whole test body (across awaits), which is why
/// this is a `tokio::sync::Mutex` rather than a `std::sync::Mutex` (safe to
/// hold across `.await` — no clippy `await_holding_lock` concern, and no
/// deadlock risk since each test's own runtime never needs the lock twice).
fn github_env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// RAII guard for `GITHUB_API_BASE`: sets it on construction, restores
/// (removes) it on drop — including on an unwind, e.g. an assertion panic
/// partway through a test body. Without this, a plain `set_var` paired with
/// a `remove_var` at the end of the test function leaks the var into every
/// later test whenever a panic lands between the two (this file's own
/// `github_env_lock` only serializes the tests against each other; it can't
/// undo a set that never got cleaned up).
struct GithubApiBaseGuard;

impl GithubApiBaseGuard {
    fn set(url: &str) -> Self {
        // SAFETY: serialized against every other `GITHUB_API_BASE` mutation
        // in this file by `github_env_lock`, held across the whole test
        // body (including this guard's lifetime).
        unsafe {
            std::env::set_var("GITHUB_API_BASE", url);
        }
        Self
    }
}

impl Drop for GithubApiBaseGuard {
    fn drop(&mut self) {
        // SAFETY: see `set` above.
        unsafe {
            std::env::remove_var("GITHUB_API_BASE");
        }
    }
}

/// Two overlapping calls race a mocked GitHub response reporting one new
/// version. `before` is read by both calls before either check completes, so
/// without gating the event/notify block on `run_check`'s "did I actually
/// perform the check" flag, both would see a stale "before" and both publish
/// + notify for the same version. This proves at most one notification is
/// recorded (and, via `.expect(1)`, that only one HTTP request went out).
#[tokio::test]
async fn overlapping_checks_notify_at_most_once() {
    let _env_guard = github_env_lock().lock().await;
    crate::install_crypto_provider();
    let mut s = mockito::Server::new_async().await;
    let mock = s
        .mock(
            "GET",
            "/repos/jorge-menjivar/super-stt/releases?per_page=100",
        )
        .with_status(200)
        .with_body(r#"[{"tag_name":"v55.0.0","prerelease":false,"assets":[]}]"#)
        .expect(1)
        .create_async()
        .await;

    // Route the daemon's self-update forge client at the mock instead of the
    // real GitHub API (mirrors the existing
    // `unsafe { set_var(XDG_RUNTIME_DIR, ..) }` idiom in
    // `super-stt-shared/src/validation/paths.rs`). Serialized against the
    // other `GITHUB_API_BASE`-touching tests in this file by
    // `github_env_lock()` above; restored on drop (even on panic) by
    // `GithubApiBaseGuard`.
    let _api_base = GithubApiBaseGuard::set(&s.url());

    let mut daemon = test_daemon().await;
    // `test_daemon()`'s default notifier is set to fail delivery (so it
    // never pops a real desktop notification if a test forgets to swap it
    // in) — this test needs delivery to succeed so `record_notified` runs,
    // per the override the notifier field's doc comment calls out.
    let (notifier, sent) = Notifier::fake(false);
    daemon.notifier = std::sync::Arc::new(tokio::sync::Mutex::new(notifier));
    let a = daemon.clone();
    let b = daemon.clone();

    let _ = tokio::join!(
        a.run_self_update_check_and_notify(),
        b.run_self_update_check_and_notify(),
    );

    mock.assert_async().await;
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "overlapping checks for the same version must notify at most once"
    );
}

/// The `UpdateAvailable` event is published when a check newly finds an
/// update, but a subsequent check that finds the *same* candidate again
/// must not publish a second one (`docs/protocol/endpoints/v1/events.md`:
/// "emitted when a check newly finds an available update or the candidate
/// version changes").
#[tokio::test]
async fn update_available_event_published_only_when_the_candidate_is_new() {
    let _env_guard = github_env_lock().lock().await;
    crate::install_crypto_provider();
    let mut s = mockito::Server::new_async().await;
    let mock = s
        .mock(
            "GET",
            "/repos/jorge-menjivar/super-stt/releases?per_page=100",
        )
        .with_status(200)
        .with_body(r#"[{"tag_name":"v77.0.0","prerelease":false,"assets":[]}]"#)
        .expect(2)
        .create_async()
        .await;
    let _api_base = GithubApiBaseGuard::set(&s.url());

    let mut daemon = test_daemon().await;
    let (notifier, _sent) = Notifier::fake(false);
    daemon.notifier = std::sync::Arc::new(tokio::sync::Mutex::new(notifier));
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    // First check newly finds v77.0.0: the event must be published.
    let status1 = daemon.run_self_update_check_and_notify().await;
    assert_eq!(status1.latest_version.as_deref(), Some("v77.0.0"));
    let (topic, payload) = tokio::time::timeout(Duration::from_secs(2), rx.recv_json())
        .await
        .expect("event published within timeout")
        .expect("event");
    assert_eq!(topic, "daemon_status_changed");
    assert_eq!(payload["status"], "update_available");
    assert_eq!(payload["latest_version"], "v77.0.0");

    // Second check finds the same candidate again: no new event.
    let status2 = daemon.run_self_update_check_and_notify().await;
    assert_eq!(status2.latest_version.as_deref(), Some("v77.0.0"));
    let no_more = tokio::time::timeout(Duration::from_millis(200), rx.recv_json()).await;
    assert!(
        no_more.is_err(),
        "an unchanged candidate must not publish a second event"
    );

    mock.assert_async().await;
}

/// A failed check must publish no event and send no notification, even when
/// it preserves a stale candidate from the last success (the failure path
/// is purely informational — `last_check_error` — not a fresh "found an
/// update" signal).
#[tokio::test]
async fn failed_check_publishes_no_event_and_notifies_nobody() {
    let _env_guard = github_env_lock().lock().await;
    crate::install_crypto_provider();
    let mut s = mockito::Server::new_async().await;
    s.mock(
        "GET",
        "/repos/jorge-menjivar/super-stt/releases?per_page=100",
    )
    .with_status(200)
    .with_body(r#"[{"tag_name":"v81.0.0","prerelease":false,"assets":[]}]"#)
    .create_async()
    .await;
    let _api_base = GithubApiBaseGuard::set(&s.url());

    let mut daemon = test_daemon().await;
    let (notifier, sent) = Notifier::fake(false);
    daemon.notifier = std::sync::Arc::new(tokio::sync::Mutex::new(notifier));
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    // First: succeed, notify once, and drain the event this establishes.
    let status1 = daemon.run_self_update_check_and_notify().await;
    assert_eq!(status1.latest_version.as_deref(), Some("v81.0.0"));
    let _ = tokio::time::timeout(Duration::from_secs(2), rx.recv_json())
        .await
        .expect("first update_available event")
        .expect("event");
    assert_eq!(sent.lock().unwrap().len(), 1);

    // Now the server is gone: the next check fails but (same channel)
    // preserves the stale candidate.
    drop(s);
    let status2 = daemon.run_self_update_check_and_notify().await;
    assert!(status2.last_check_error.is_some());
    assert_eq!(status2.latest_version.as_deref(), Some("v81.0.0"));

    let no_more = tokio::time::timeout(Duration::from_millis(200), rx.recv_json()).await;
    assert!(
        no_more.is_err(),
        "a failed check must not publish an UpdateAvailable event"
    );
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "a failed check must not send a second notification"
    );
}

/// `Dbus` and `Auto` both deliver a desktop notification for a newly found
/// update (and, since delivery succeeds, record it as notified).
async fn assert_method_notifies(method: NotificationMethod, tag: &str) {
    let _env_guard = github_env_lock().lock().await;
    crate::install_crypto_provider();
    let mut s = mockito::Server::new_async().await;
    s.mock(
        "GET",
        "/repos/jorge-menjivar/super-stt/releases?per_page=100",
    )
    .with_status(200)
    .with_body(format!(
        r#"[{{"tag_name":"{tag}","prerelease":false,"assets":[]}}]"#
    ))
    .create_async()
    .await;
    let _api_base = GithubApiBaseGuard::set(&s.url());

    let mut daemon = test_daemon().await;
    daemon
        .config
        .write()
        .await
        .transcription
        .notification_method = method;
    let (notifier, sent) = Notifier::fake(false);
    daemon.notifier = std::sync::Arc::new(tokio::sync::Mutex::new(notifier));

    let status = daemon.run_self_update_check_and_notify().await;
    assert_eq!(status.latest_version.as_deref(), Some(tag));

    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "{method:?} must send a notification"
    );
    assert!(
        !daemon.self_update.should_notify(tag).await,
        "{method:?} must record the version as notified once delivery succeeds"
    );
}

#[tokio::test]
async fn dbus_method_sends_a_notification() {
    assert_method_notifies(NotificationMethod::Dbus, "v82.0.0").await;
}

#[tokio::test]
async fn auto_method_sends_a_notification() {
    assert_method_notifies(NotificationMethod::Auto, "v83.0.0").await;
}

/// `Off` and `Typed` must send no notification AND must not record the
/// version as notified — the subtle half of the rule: recording it would
/// mean a later switch to `Dbus`/`Auto` silently never notifies for a
/// version the user was never actually shown.
async fn assert_method_does_not_notify_or_record(method: NotificationMethod, tag: &str) {
    let _env_guard = github_env_lock().lock().await;
    crate::install_crypto_provider();
    let mut s = mockito::Server::new_async().await;
    s.mock(
        "GET",
        "/repos/jorge-menjivar/super-stt/releases?per_page=100",
    )
    .with_status(200)
    .with_body(format!(
        r#"[{{"tag_name":"{tag}","prerelease":false,"assets":[]}}]"#
    ))
    .create_async()
    .await;
    let _api_base = GithubApiBaseGuard::set(&s.url());

    let mut daemon = test_daemon().await;
    daemon
        .config
        .write()
        .await
        .transcription
        .notification_method = method;
    // `fail: false` — irrelevant here since Off/Typed never call
    // `notifier.send` at all, which is exactly what this test proves.
    let (notifier, sent) = Notifier::fake(false);
    daemon.notifier = std::sync::Arc::new(tokio::sync::Mutex::new(notifier));

    let status = daemon.run_self_update_check_and_notify().await;
    assert_eq!(status.latest_version.as_deref(), Some(tag));

    assert!(
        sent.lock().unwrap().is_empty(),
        "{method:?} must not send a notification"
    );
    assert!(
        daemon.self_update.should_notify(tag).await,
        "{method:?} must NOT record the version as notified, so switching \
         the method later still notifies once"
    );
}

#[tokio::test]
async fn off_method_does_not_notify_or_record() {
    assert_method_does_not_notify_or_record(NotificationMethod::Off, "v84.0.0").await;
}

#[tokio::test]
async fn typed_method_does_not_notify_or_record() {
    assert_method_does_not_notify_or_record(NotificationMethod::Typed, "v85.0.0").await;
}
