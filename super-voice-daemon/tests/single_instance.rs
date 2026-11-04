// SPDX-License-Identifier: GPL-3.0-only
//! One daemon per socket.
//!
//! A daemon started on a socket another one was serving used to run its whole
//! startup, then delete that socket and bind its own. The first daemon kept
//! running with its models loaded, unreachable. Now the second one must exit
//! at once, and the first must keep serving.

mod common;

use std::time::{Duration, Instant};

#[tokio::test]
async fn a_second_daemon_on_a_live_socket_exits_and_the_first_keeps_serving() {
    let first = common::daemon("single-first").start().await;
    let token = first.token("single-instance-test", &["status"]).await;

    // Its socket answers at once, since the first daemon serves it, so
    // `start` returns while this one is still starting up.
    let mut second = common::daemon("single-second")
        .socket_at(first.socket())
        .start()
        .await;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = second.child().try_wait().expect("try_wait") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "a second daemon on {} is still running after 30s; it should have \
             refused to start",
            first.socket().display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(
        !status.success(),
        "a refused start must exit non-zero, got {status:?}"
    );

    // The token was minted by the first daemon, and only it knows it, so a 200
    // here means the first daemon still owns the socket.
    let (code, body) = first.get("/ping", &token).await;
    assert_eq!(code, 200, "the first daemon stopped answering: {body}");
}
