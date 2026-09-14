// SPDX-License-Identifier: GPL-3.0-only
//! `v1/ping` — the connectivity check behind the settings page's daemon
//! indicator.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::ping;

#[tokio::test]
async fn a_ping_returns_the_daemons_own_greeting() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(
        &json!({ "status": "success", "message": "pong" }),
    ));

    let greeting = ping::ping_daemon().await.expect("daemon answered");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/ping");
    assert_eq!(greeting, "pong");
}

/// A daemon that answers without a message is still up, and "is it up" is the
/// entire question — so the wrapper supplies the wording rather than failing.
#[tokio::test]
async fn a_ping_without_a_message_still_counts_as_running() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "success" })));

    let greeting = ping::ping_daemon().await.expect("daemon answered");

    assert_eq!(greeting, "Daemon is running");
}

#[tokio::test]
async fn the_connection_test_keeps_only_the_verdict() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(
        &json!({ "status": "success", "message": "pong" }),
    ));

    ping::test_daemon_connection()
        .await
        .expect("connection is good");

    assert_eq!(daemon.request().path(), "/ping");
}

/// Nothing is listening: the transport has to say so in words an operator can
/// act on, rather than surfacing a raw `ENOENT`.
#[tokio::test]
async fn a_daemon_that_is_not_there_says_to_start_it() {
    let daemon = FakeDaemon::start().await;
    daemon.stop_listening();

    let error = ping::ping_daemon()
        .await
        .expect_err("nothing to connect to");

    assert!(
        error.to_string().contains("Start the daemon first"),
        "got {error}"
    );
}
