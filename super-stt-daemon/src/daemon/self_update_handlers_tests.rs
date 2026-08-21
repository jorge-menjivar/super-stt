// SPDX-License-Identifier: GPL-3.0-only
//! Coalescing side-effect gating: two overlapping
//! `run_self_update_check_and_notify` calls must not double-publish the
//! `UpdateAvailable` event or double-notify for the same version (task
//! review round 1, Important finding).

use crate::daemon::types::test_daemon;
use crate::output::notification::Notifier;

/// Two overlapping calls race a mocked GitHub response reporting one new
/// version. `before` is read by both calls before either check completes, so
/// without gating the event/notify block on `run_check`'s "did I actually
/// perform the check" flag, both would see a stale "before" and both publish
/// + notify for the same version. This proves at most one notification is
/// recorded (and, via `.expect(1)`, that only one HTTP request went out).
#[tokio::test]
async fn overlapping_checks_notify_at_most_once() {
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
    // real GitHub API. Safe here: no other `--lib` test in this crate reads
    // or writes `GITHUB_API_BASE` (mirrors the existing
    // `unsafe { set_var(XDG_RUNTIME_DIR, ..) }` idiom in
    // `super-stt-shared/src/validation/paths.rs`).
    unsafe {
        std::env::set_var("GITHUB_API_BASE", s.url());
    }

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

    unsafe {
        std::env::remove_var("GITHUB_API_BASE");
    }

    mock.assert_async().await;
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "overlapping checks for the same version must notify at most once"
    );
}
