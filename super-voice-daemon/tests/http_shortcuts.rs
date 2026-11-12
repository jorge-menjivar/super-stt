// SPDX-License-Identifier: GPL-3.0-only
//! `/v1/shortcuts` end to end: a real daemon, in a home of its own, adding and
//! removing COSMIC shortcuts in the shortcuts file under its own
//! `XDG_CONFIG_HOME`, and refusing what it cannot do.
//!
//! The desktop is set with `XDG_CURRENT_DESKTOP`, so these run the same on a
//! machine with no COSMIC, CI included. The session bus is one that is not
//! there, so no daemon here finds the desktop's own portal: the portal's
//! tests are `shortcuts_portal.rs`, against a fake one.

mod common;

use common::{Method, StatusCode, TestDaemon};
use serde_json::Value;
use std::path::{Path, PathBuf};
use super_voice_shared::daemon::http_client;

/// COSMIC's custom-shortcuts file under a test home's config.
fn shortcuts_file(config: &Path) -> PathBuf {
    config.join("cosmic/com.system76.CosmicSettings.Shortcuts/v1/custom")
}

/// A daemon on `desktop`, with no session bus, with `file` as COSMIC's
/// shortcuts file when given, and a token for it with the `settings` scope.
async fn start(label: &str, desktop: &str, file: Option<&str>) -> (TestDaemon, String) {
    let builder = common::daemon(label)
        .env("XDG_CURRENT_DESKTOP", desktop)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus");
    if let Some(content) = file {
        let path = shortcuts_file(&builder.home().config);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, content).expect("write fixture");
    }
    let daemon = builder.start().await;
    let token = http_client::auth_request(
        daemon.socket().to_path_buf(),
        "shortcuts test",
        &["settings"],
    )
    .await
    .expect("auth_request")
    .session_token;
    (daemon, token)
}

async fn call(daemon: &TestDaemon, token: &str, method: Method, path: &str) -> (StatusCode, Value) {
    common::request(daemon.socket(), method, path, Some(token), None).await
}

/// Each shortcut's `state`, by id.
fn states(list: &Value) -> Vec<(String, String)> {
    list["shortcuts"]
        .as_array()
        .expect("shortcuts")
        .iter()
        .map(|s| {
            (
                s["id"].as_str().expect("id").to_string(),
                s["state"].as_str().expect("state").to_string(),
            )
        })
        .collect()
}

fn pair(id: &str, state: &str) -> (String, String) {
    (id.to_string(), state.to_string())
}

/// One of the user's own shortcuts, which adding and removing Super Voice's
/// must leave alone.
const THEIRS: &str = r#"{ (modifiers: [Ctrl, Alt], key: "t", description: Some("Terminal")): Spawn("cosmic-term"), }"#;

/// On COSMIC, a shortcut is added to the file, adding it again changes
/// nothing, and removing it takes it out. The user's own is left alone
/// throughout.
#[tokio::test]
async fn cosmic_shortcuts_are_added_and_removed() {
    let (daemon, token) = start("shortcuts-cosmic", "COSMIC", Some(THEIRS)).await;
    let file = shortcuts_file(&daemon.home().config);

    let (status, list) = call(&daemon, &token, Method::GET, "/shortcuts").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["method"], "cosmic");
    assert_eq!(states(&list), [pair("dictate", "missing")]);

    let (status, list) = call(&daemon, &token, Method::POST, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(states(&list), [pair("dictate", "installed")]);
    assert_eq!(list["shortcuts"][0]["keys"], "Super+Space");
    let written = std::fs::read_to_string(&file).expect("the file was written");
    assert!(written.contains("voice record --write"), "{written}");
    assert!(written.contains(r#"Spawn("cosmic-term")"#), "{written}");

    let (status, again) = call(&daemon, &token, Method::POST, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(std::fs::read_to_string(&file).expect("file"), written);

    let (status, list) = call(&daemon, &token, Method::DELETE, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(states(&list), [pair("dictate", "missing")]);
    let left = std::fs::read_to_string(&file).expect("file");
    assert!(!left.contains("voice record --write"), "{left}");
    assert!(left.contains(r#"Spawn("cosmic-term")"#), "{left}");

    let (status, _) = call(&daemon, &token, Method::DELETE, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::OK, "removing it again changes nothing");
}

/// A shortcut whose keys another shortcut holds is never added over it.
#[tokio::test]
async fn cosmic_never_adds_over_taken_keys() {
    let taken = r#"{ (modifiers: [Super], key: "space", description: Some("Launcher")): Spawn("cosmic-launcher"), }"#;
    let (daemon, token) = start("shortcuts-taken", "COSMIC", Some(taken)).await;

    let (_, list) = call(&daemon, &token, Method::GET, "/shortcuts").await;
    assert_eq!(states(&list), [pair("dictate", "taken")]);

    let (status, body) = call(&daemon, &token, Method::POST, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error_code"], "shortcut_keys_taken", "{body}");
    let file = std::fs::read_to_string(shortcuts_file(&daemon.home().config)).expect("file");
    assert_eq!(file, taken, "the file is left as it was");
}

/// A desktop the daemon has no method for lists each shortcut for the user
/// to bind, and refuses to add one.
#[tokio::test]
async fn other_desktops_are_left_to_the_user() {
    let (daemon, token) = start("shortcuts-manual", "Sway", None).await;

    let (_, list) = call(&daemon, &token, Method::GET, "/shortcuts").await;
    assert_eq!(list["method"], "manual");
    assert_eq!(states(&list), [pair("dictate", "manual")]);
    assert_eq!(list["shortcuts"][0]["command"], "voice record --write");

    let (status, body) = call(&daemon, &token, Method::POST, "/shortcuts/dictate").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error_code"], "shortcuts_unsupported", "{body}");
}

/// A path that names no shortcut is a `404`, and the routes need `settings`.
#[tokio::test]
async fn unknown_shortcuts_and_narrow_tokens_are_refused() {
    let (daemon, token) = start("shortcuts-refusals", "COSMIC", None).await;

    let (status, body) = call(&daemon, &token, Method::POST, "/shortcuts/screenshot").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["message"], "unknown_shortcut", "{body}");

    let status_only = http_client::auth_request(
        daemon.socket().to_path_buf(),
        "shortcuts test, status only",
        &["status"],
    )
    .await
    .expect("auth_request")
    .session_token;
    let (status, _) = call(&daemon, &status_only, Method::GET, "/shortcuts").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
