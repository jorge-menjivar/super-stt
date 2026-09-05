// SPDX-License-Identifier: GPL-3.0-only
//! The `/v1/settings` protocol, driven over the wire against a live daemon.
//!
//! `http_smoke_settings` walks the surface value by value — set a theme, read
//! it back. This walks it *as a namespace*: the properties that have to hold
//! for every path under `/v1/settings`, whichever value it carries.
//!
//! That is the half a per-endpoint test cannot cover. A settings endpoint added
//! next year gets its own round-trip test if someone remembers; it is held to
//! the rules below whether they remember or not, because the table is the
//! namespace and every case iterates it.
//!
//! Four properties, each a way the namespace has actually been got wrong:
//!
//! - every path answers, so a rename that misses a registration is a `404` here
//!   rather than an empty settings page in front of a user;
//! - every path is behind the `settings` scope, in both directions — a token
//!   without it is refused, and no token at all is refused earlier;
//! - reads round-trip through writes, which is the only thing that makes a
//!   setting a setting;
//! - the pre-namespace spellings are gone, not quietly still served. A rename
//!   that leaves both live is the worst outcome: clients split across two
//!   spellings and the daemon looks fine from either.
//!
//! One daemon spawn for the whole file. Spawning per case would multiply a
//! ~200ms startup by every path in the table.

use http_body_util::{BodyExt, Empty, Full};
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
        }
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

async fn start_daemon() -> (DaemonGuard, PathBuf) {
    let unique = format!("stt-settings-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    // Isolate XDG_CONFIG_HOME so the test daemon doesn't write the
    // developer's real `~/.config/super-stt/daemon.toml` (e.g.
    // overriding their audio theme to Silent or device to cpu via
    // `apply_cli_overrides_to_config`).
    let config_home = tmp.join(format!("{unique}-config"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");
    // Isolate XDG_DATA_HOME too: the daemon discovers backends under
    // `<data_dir>/super-stt/backends`. An empty isolated dir keeps the smoke
    // test hermetic and fast — no real backend is spawned at startup, so the
    // daemon comes up idle (which the assertions below tolerate).
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&data_home).expect("create test data dir");

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1") // in-memory keyring (no secret-service prompt in tests/CI)
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        // Point the daemon's self-update forge client at a guaranteed-refused
        // loopback port (`accept_base_url` allows loopback `http://`), so
        // `POST /update/check` below fails deterministically and offline
        // instead of making a real call to api.github.com under CI.
        .env("GITHUB_API_BASE", "http://127.0.0.1:9")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    // Confirm the daemon is up by minting a throwaway token
    // Hand the child to the guard before the readiness loop: the timeout
    // panic below must still kill and reap the daemon, not leak it.
    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone()],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "settings-smoke", &["status"])
                .await
                .is_ok()
        {
            return (guard, http_socket);
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

/// Tiny GET helper for endpoints `super_stt_shared::daemon::http_client`
/// doesn't yet wrap. Returns (`status_code`, parsed JSON body).
async fn raw_get_json(
    socket_path: &PathBuf,
    path: &str,
    token: &str,
) -> (StatusCode, serde_json::Value) {
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Empty<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method(Method::GET)
        .uri(format!("http://stt.local/v1{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"))
        .body(Empty::<Bytes>::new())
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let body_bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    let body: serde_json::Value =
        serde_json::from_slice(&body_bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn raw_post_json(
    socket_path: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let body_bytes = serde_json::to_vec(&body).expect("encode body");
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("http://stt.local/v1{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("content-length", body_bytes.len().to_string())
        .body(Full::new(Bytes::from(body_bytes)))
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

/// Every `GET`-able path under `/v1/settings`, with the value key it answers
/// with — the field a client actually reads.
///
/// This table *is* the namespace. Adding a settings endpoint means adding a row,
/// and `url_surface_contract::the_url_surface_is_exactly_this` is what makes
/// forgetting to loud rather than silent.
const SETTINGS_READS: &[(&str, &str)] = &[
    ("/settings/audio_theme", "audio_theme"),
    ("/settings/audio_theme/list", "available_audio_themes"),
    ("/settings/custom_models_dir", "custom_models_dir"),
    ("/settings/language", "language"),
    ("/settings/notification_method", "notification_method"),
    ("/settings/preview_typing", "preview_typing_enabled"),
    ("/settings/recording_stop_mode", "recording_stop_mode"),
    ("/settings/update_beta_optin", "update_beta_optin"),
    ("/settings/update_check_enabled", "update_check_enabled"),
    ("/settings/volume", "message"),
    ("/settings/write_method", "write_method"),
];

/// The spellings these endpoints answered on before the namespace existed.
///
/// Kept as data rather than prose so the migration is checkable: each must be
/// `404` now. A path serving both spellings is the failure this catches.
const RETIRED_PATHS: &[&str] = &[
    "/audio_theme",
    "/audio_themes",
    "/custom_models_dir",
    "/language",
    "/notification_method",
    "/preview_typing",
    "/recording_stop_mode",
    "/update_beta_optin",
    "/update_check_enabled",
    "/volume",
    "/write_method",
];

/// Mint a token with the given scopes.
async fn token_for(socket: &Path, scopes: &[&str]) -> String {
    http_client::auth_request(socket.to_path_buf(), "super-stt settings protocol", scopes)
        .await
        .expect("auth_request")
        .session_token
}

/// `GET` without going through the JSON helper, so the status of a non-JSON
/// failure is still observable.
async fn raw_get_status(socket: &Path, path: &str, token: Option<&str>) -> StatusCode {
    let stream = UnixStream::connect(socket).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let mut req = Request::builder()
        .method(Method::GET)
        .uri(format!("http://stt.local/v1{path}"))
        .header("host", "stt.local");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let req = req.body(Empty::<Bytes>::new()).expect("build req");
    sender.send_request(req).await.expect("send req").status()
}

/// Every settings path answers, and says it succeeded.
///
/// The blunt one, and the one that earns its keep: it is what turns a missed
/// route registration into a red test instead of a settings page that renders
/// empty with nothing in the log.
#[tokio::test]
async fn every_settings_path_answers() {
    let (_guard, sock) = start_daemon().await;
    let token = token_for(&sock, &["settings"]).await;

    for (path, key) in SETTINGS_READS {
        let (status, body) = raw_get_json(&sock, path, &token).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "GET {path} answered {status}: {body}"
        );
        assert_eq!(body["status"], "success", "GET {path}: {body}");
        assert!(
            body.get(key).is_some(),
            "GET {path} answered without its value in {key:?}: {body}\n\
             (null is fine — an unset setting reads null; absent is not)"
        );
    }
}

/// The namespace is closed to a token that lacks the `settings` scope.
///
/// A `status`/`transcribe` token is what a recording client holds. Reaching a
/// settings read with it would leak the machine's configuration to every app
/// the user ever approved for dictation.
#[tokio::test]
async fn every_settings_path_refuses_a_token_without_the_scope() {
    let (_guard, sock) = start_daemon().await;
    let wrong = token_for(&sock, &["transcribe", "status"]).await;

    for (path, _) in SETTINGS_READS {
        let status = raw_get_status(&sock, path, Some(&wrong)).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "GET {path} with a non-settings token answered {status}, wanted 403"
        );
    }
}

/// And to no token at all.
///
/// Checked separately from the wrong-scope case because they fail at different
/// layers — this one never reaches the scope guard — and a regression in either
/// is invisible from the other.
#[tokio::test]
async fn every_settings_path_refuses_an_anonymous_request() {
    let (_guard, sock) = start_daemon().await;

    for (path, _) in SETTINGS_READS {
        let status = raw_get_status(&sock, path, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "GET {path} unauthenticated answered {status}, wanted 401"
        );
    }
}

/// The spellings from before the namespace are gone.
///
/// Not merely "the new path works" — that would pass with both live, which is
/// the outcome worth catching: clients would split across two spellings and
/// each would look correct in isolation until one was finally removed.
#[tokio::test]
async fn the_pre_namespace_paths_are_gone() {
    let (_guard, sock) = start_daemon().await;
    let token = token_for(&sock, &["settings"]).await;

    for path in RETIRED_PATHS {
        let status = raw_get_status(&sock, path, Some(&token)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{path} still answers ({status}) — it moved under /v1/settings, and \
             serving both spellings splits clients across them"
        );
    }
}

/// A write is visible to the next read.
///
/// The property that makes a setting a setting. Uses `notification_method`,
/// which had no smoke coverage at all before this, and `volume`, which is
/// numeric and answers with a bare `Ack` — between them they exercise both the
/// enum and the scalar setter macro, and both response shapes.
#[tokio::test]
async fn a_write_is_visible_to_the_next_read() {
    let (_guard, sock) = start_daemon().await;
    let token = token_for(&sock, &["settings"]).await;

    // An enum-valued setting.
    let (status, body) = raw_post_json(
        &sock,
        "/settings/notification_method",
        &token,
        serde_json::json!({ "method": "off" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "POST notification_method: {body}");

    let (status, body) = raw_get_json(&sock, "/settings/notification_method", &token).await;
    assert_eq!(status, StatusCode::OK, "GET notification_method: {body}");
    assert!(
        body["notification_method"]
            .as_str()
            .is_some_and(|m| m == "off"),
        "notification_method did not read back as off: {body}"
    );

    // A numeric setting, through the other setter macro.
    let (status, body) = raw_post_json(
        &sock,
        "/settings/volume",
        &token,
        serde_json::json!({ "volume": 40 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "POST volume: {body}");

    let (status, body) = raw_get_json(&sock, "/settings/volume", &token).await;
    assert_eq!(status, StatusCode::OK, "GET volume: {body}");
    assert!(
        body["message"].as_str().is_some_and(|m| m.contains("40")),
        "volume did not read back as 40: {body}"
    );
}

/// A value the setting does not accept is refused, and changes nothing.
///
/// The second half matters more than the first. A rejected write that has
/// already clobbered the stored value leaves the daemon in a state the client
/// was told it had not reached.
#[tokio::test]
async fn a_rejected_write_leaves_the_setting_alone() {
    let (_guard, sock) = start_daemon().await;
    let token = token_for(&sock, &["settings"]).await;

    let (_, before) = raw_get_json(&sock, "/settings/audio_theme", &token).await;
    let before_theme = before["audio_theme"]
        .as_str()
        .unwrap_or("classic")
        .to_string();

    let (status, body) = raw_post_json(
        &sock,
        "/settings/audio_theme",
        &token,
        serde_json::json!({ "theme": "not-a-theme" }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unknown theme was accepted: {body}"
    );

    let (_, after) = raw_get_json(&sock, "/settings/audio_theme", &token).await;
    assert_eq!(
        after["audio_theme"].as_str().unwrap_or("classic"),
        before_theme,
        "a rejected write changed the stored theme"
    );
}

/// `POST /settings/write_method/test` is routed, scoped, and — where it is safe
/// to run — names its outcome.
///
/// The one settings endpoint with a side effect and no stored value, and one of
/// the two that had no coverage at all.
///
/// The side effect is the catch: the probe *types* its fixed string into
/// whatever window has focus. On a developer's machine that is the window they
/// are working in, which is not something a `just test` run may do. So the
/// probe itself runs only where there is nothing to type into — a headless CI
/// runner — and the parts that are true everywhere are checked everywhere:
/// the path is routed, and it is behind the settings scope. A `404` means a
/// rename missed it; a `403` for a settings token means it left its scope.
///
/// Where the probe does run, either answer is correct and both are asserted.
/// Whether a write method exists is a property of the host: headless, there is
/// no window and it answers `500 write_method_unavailable`. Asserting `200`
/// alone made the test a report on where it happened to run, and it went red on
/// CI for exactly that reason.
#[tokio::test]
async fn the_write_method_probe_reports_what_it_did() {
    let (_guard, sock) = start_daemon().await;

    // Routed and scoped, checked without firing the probe: a token that lacks
    // the scope is refused before the handler runs, so nothing is typed.
    let wrong = token_for(&sock, &["transcribe", "status"]).await;
    let (status, body) = raw_post_json(
        &sock,
        "/settings/write_method/test",
        &wrong,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "POST /settings/write_method/test answered {status} to a non-settings token: {body}\n\
         404 means the rename missed it; 200 means it is not behind the settings scope"
    );

    let Some(session) = graphical_session() else {
        return run_the_probe(&sock).await;
    };
    eprintln!(
        "skipping the write-method probe: it types into the focused window, and \
         this host has a graphical session ({session})"
    );
}

/// The window server this host has, or `None` when it has none and the probe is
/// safe to fire.
fn graphical_session() -> Option<String> {
    for key in ["WAYLAND_DISPLAY", "DISPLAY"] {
        if let Ok(value) = std::env::var(key)
            && !value.is_empty()
        {
            return Some(format!("{key}={value}"));
        }
    }
    match std::env::var("XDG_SESSION_TYPE") {
        Ok(kind) if kind != "tty" && !kind.is_empty() => Some(format!("XDG_SESSION_TYPE={kind}")),
        _ => None,
    }
}

/// Fire the probe and check it names its outcome. Headless only — see
/// [`the_write_method_probe_reports_what_it_did`].
async fn run_the_probe(sock: &PathBuf) {
    let token = token_for(sock, &["settings"]).await;
    let (status, body) = raw_post_json(
        sock,
        "/settings/write_method/test",
        &token,
        serde_json::json!({}),
    )
    .await;

    match status {
        StatusCode::OK => {
            assert_eq!(
                body["status"], "success",
                "a 200 that is not a success: {body}"
            );
        }
        StatusCode::INTERNAL_SERVER_ERROR => {
            // The headless case: no window to type into. It still has to say so.
            assert_eq!(
                body["status"], "error",
                "a 500 that is not an error: {body}"
            );
            assert!(
                body["error_code"].as_str().is_some(),
                "the probe failed without an error_code: {body}"
            );
            assert!(
                body["message"].as_str().is_some_and(|m| !m.is_empty()),
                "the probe failed without saying why: {body}"
            );
        }
        other => panic!(
            "POST /settings/write_method/test answered {other}: {body}\n\
             404 means the rename missed it; 401/403 means it left the settings scope"
        ),
    }
}
