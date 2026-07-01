// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end check that the daemon's per-request rate-limit middleware
//! (`require_rate_limit`) actually surfaces `429 rate_limited` on the wire.
//!
//! The limiting *logic* (`ResourceManager::record_request`) has unit tests
//! in `super-stt-shared`; this test pins the HTTP wiring — that exceeding
//! the quota for a client turns into a `429` response rather than being
//! silently dropped or mis-mapped.
//!
//! The daemon under `cargo test` is a debug build, so it uses
//! `ResourceManager::development()` (`max_requests_per_minute: 300`); the
//! check is `count > limit`, so the 301st request in the rolling minute is
//! the first to be rejected. All requests here share one `client_id`
//! (`uid:pid` of the test process) and connection registration is idempotent
//! per client, so the flood trips the *rate* limit, not the connection limit.
//!
//! `/auth/request` is exempt from rate limiting, so minting the token (and
//! the readiness probe) doesn't consume the quota the flood is measured
//! against.
//!
//! Hermetic: `SUPER_STT_AUTO_APPROVE=1` + `SUPER_STT_KEYRING_MOCK=1`.

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use hyper::{Method, Request, StatusCode};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");

/// `ResourceLimits::development().max_requests_per_minute` — the build the
/// test daemon runs. Kept in sync with
/// `super-stt-daemon/src/resource_management/mod.rs`.
const DEV_PER_MINUTE: usize = 300;

struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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

async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let unique = format!("stt-ratelimit-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&data_home).expect("create test data dir");

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .arg("--device")
        .arg("cpu")
        .arg("--audio-theme")
        .arg("silent")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "ratelimit-smoke", &["status"])
                .await
                .is_ok()
        {
            return (
                DaemonGuard {
                    child,
                    cleanup_paths: vec![http_socket.clone(), config_home, data_home],
                },
                http_socket,
            );
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

/// Send one `GET /v1/ping` on an existing keep-alive connection and drain
/// the response body (HTTP/1 requires the prior body consumed before the
/// next request). Returns `(status, body bytes)`.
///
/// Reusing a single connection is essential: the daemon registers a fresh
/// per-client connection record on every accept
/// (`ResourceManager::register_connection`), which resets that client's
/// rolling request window. A new connection per request would therefore
/// never accumulate toward the quota — the flood has to ride one connection.
async fn ping_on(
    sender: &mut hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    token: &str,
) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method(Method::GET)
        .uri("http://stt.local/v1/ping")
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"))
        .body(Empty::<Bytes>::new())
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();
    (status, bytes)
}

/// Flooding `/ping` past the per-minute quota yields `429 rate_limited`,
/// and every request up to the limit succeeds with `200`.
#[tokio::test]
async fn flood_trips_rate_limit() {
    let (_guard, sock) = start_daemon().await;
    let token = http_client::auth_request(sock.clone(), "ratelimit", &["status"])
        .await
        .expect("auth")
        .session_token;

    // One persistent connection for the whole flood (see `ping_on`).
    let stream = UnixStream::connect(&sock).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Empty<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // Cap the loop well above the limit so a regression that never trips
    // can't spin forever, but tight enough that the whole flood lands
    // inside the rolling 60s window.
    let cap = DEV_PER_MINUTE + 40;
    let mut first_status = None;
    let mut tripped_at = None;
    let mut limited_body = Vec::new();

    for i in 1..=cap {
        let (status, body) = ping_on(&mut sender, &token).await;
        if first_status.is_none() {
            first_status = Some(status);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            tripped_at = Some(i);
            limited_body = body;
            break;
        }
        assert_eq!(
            status,
            StatusCode::OK,
            "request {i} should be 200 until the quota trips, got {status}"
        );
    }

    assert_eq!(
        first_status,
        Some(StatusCode::OK),
        "the very first request must succeed"
    );
    let tripped_at =
        tripped_at.expect("a 429 must occur within the cap — rate limiting not wired?");
    assert!(
        tripped_at > DEV_PER_MINUTE,
        "the limit is `count > {DEV_PER_MINUTE}`, so the first 429 should be request \
         {} (>{DEV_PER_MINUTE}); tripped at {tripped_at}",
        DEV_PER_MINUTE + 1
    );

    let json: serde_json::Value = serde_json::from_slice(&limited_body).unwrap_or_default();
    assert_eq!(
        json["message"], "rate_limited",
        "429 body must carry the rate_limited identifier, got: {json}"
    );

    // Once over quota, the limit keeps rejecting — it's not a one-off blip.
    let (status, _) = ping_on(&mut sender, &token).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "still over quota → still 429"
    );
}
