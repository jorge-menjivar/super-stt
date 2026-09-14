// SPDX-License-Identifier: GPL-3.0-only
//! `v1/update` — the self-update status the Updates page renders.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::update;

fn status_body() -> serde_json::Value {
    json!({
        "current_version": "0.2.4",
        "latest_version": "v0.2.5",
        "update_available": true,
        "checked_at": "2026-09-14T00:00:00Z",
        "last_check_error": null,
        "beta_optin_effective": false,
        "installer_asset": {
            "name": "super-stt-install-x86_64-unknown-linux-gnu",
            "url": "https://example.test/installer",
            "size": 12_345_678_u64,
            "sha256": "0".repeat(64),
        },
    })
}

#[tokio::test]
async fn the_status_comes_back_typed() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&status_body()));

    let status = update::get_update_status()
        .await
        .expect("status read")
        .expect("this daemon serves /update");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/update");
    assert!(status.update_available);
    assert_eq!(status.latest_version.as_deref(), Some("v0.2.5"));
    assert_eq!(
        status.installer_asset.expect("an asset").name,
        "super-stt-install-x86_64-unknown-linux-gnu"
    );
}

#[tokio::test]
async fn checking_now_posts_and_returns_the_fresh_status() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&status_body()));

    let status = update::check_update_now()
        .await
        .expect("check ran")
        .expect("this daemon serves /update/check");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/update/check");
    assert_eq!(request.json(), json!({}));
    assert!(status.update_available);
}

/// A daemon older than `/v1/update` has no route at all, so axum's fallback
/// answers `404` with no body. `Ok(None)` is the honest reading — the Updates
/// page hides itself rather than reporting a failure the user cannot act on.
///
/// Both 404 shapes have to map: the bare fallback, and the detailed envelope a
/// classified 404 would carry if this endpoint ever grew one.
#[tokio::test]
async fn a_daemon_without_the_route_reads_as_no_status_rather_than_an_error() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::raw(404, "text/plain", ""))
        .reply(Reply::status(
            404,
            &json!({ "status": "error", "error_code": "not_found" }),
        ));

    assert!(
        update::get_update_status()
            .await
            .expect("no route is not a failure")
            .is_none()
    );
    assert!(
        update::check_update_now()
            .await
            .expect("no route is not a failure")
            .is_none()
    );
}

/// Only `404` means "no such route". Any other failure is a real one and has
/// to reach the page, or a daemon that is merely broken looks like a daemon
/// that is merely old.
#[tokio::test]
async fn any_other_failure_is_still_a_failure() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::status(
        500,
        &json!({ "status": "error", "message": "release feed unreachable" }),
    ));

    let error = update::get_update_status()
        .await
        .expect_err("a 500 is not an old daemon");

    assert_eq!(error.to_string(), "release feed unreachable (HTTP 500)");
}
