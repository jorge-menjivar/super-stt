// SPDX-License-Identifier: GPL-3.0-only
//! Hermetic, offline end-to-end test: drives the real `super-stt-install`
//! binary through resolve → download → verify → stage against a mocked
//! GitHub API (`GITHUB_API_BASE`, honored by `Github::from_env` /
//! `accept_base_url` — loopback `http://` is allowed for exactly this),
//! with `--dry-run` so nothing is installed and no escalation happens.
//! Safe to run locally: no root, no keyring, no real network.

use std::io::Write;

/// `--components=all` is passed explicitly rather than relying on
/// auto-detection: auto-detect reads the *real* `/usr/local/bin` on
/// whatever machine runs this test (a dev box with Super STT already
/// installed would otherwise flip into "update mode" and require applet
/// assets the fixture below may or may not carry, making the test's
/// outcome depend on host state instead of the fixture). An explicit
/// selection makes `stage::plan_components` skip detection entirely.
const COMPONENTS_ARG: &str = "--components=all";

/// The release tag this test's fake GitHub API always returns.
const FAKE_TAG: &str = "v9.9.9-beta.1";

/// This host's Rust target triple, computed the same way
/// `resolve::target_triple` does — duplicated here because the installer is
/// a binary crate (no lib target this integration test could import from).
fn target_triple() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-gnu",
        "aarch64" => "aarch64-unknown-linux-gnu",
        other => panic!("e2e_dry_run: no fixture support for host arch {other}"),
    }
}

/// sha256 hex digest of `bytes`, via `ring` (a dev-dependency of this crate
/// — the crate's own production code hashes files, not in-memory bytes, via
/// `super_stt_registry_types::verify::file_sha256_hex`).
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    hex::encode(digest.as_ref())
}

/// A minimal, valid gzip'd tar covering every file `--components=all`
/// requires (`super-stt-install/src/stage.rs::build_manifest`): the three
/// daemon binaries, the systemd unit, the app binary + desktop + icon, and
/// the applet binary + icon. Desktop-file globs (`super-stt-cosmic-applet-*.desktop`)
/// and the app metainfo are optional in `build_manifest` and omitted here.
fn build_fixture_tarball(dir: &std::path::Path) -> Vec<u8> {
    let tree = dir.join("tree");
    std::fs::create_dir_all(tree.join("systemd")).unwrap();
    std::fs::create_dir_all(tree.join("resources/icons/hicolor/scalable/apps")).unwrap();

    for bin in [
        "super-stt-daemon",
        "super-stt-cli",
        "super-stt-consent",
        "super-stt-app",
        "super-stt-cosmic-applet",
    ] {
        std::fs::write(tree.join(bin), b"#!/bin/sh\necho fake\n").unwrap();
    }
    std::fs::write(
        tree.join("systemd/super-stt.service"),
        b"[Unit]\nDescription=fake\n",
    )
    .unwrap();
    std::fs::write(
        tree.join("resources/super-stt-app.desktop"),
        b"[Desktop Entry]\nName=Super STT\n",
    )
    .unwrap();
    std::fs::write(
        tree.join("resources/icons/hicolor/scalable/apps/super-stt-app.svg"),
        b"<svg/>",
    )
    .unwrap();
    std::fs::write(
        tree.join("resources/icons/hicolor/scalable/apps/super-stt-cosmic-applet.svg"),
        b"<svg/>",
    )
    .unwrap();

    let tgz_path = dir.join("fixture.tar.gz");
    {
        let f = std::fs::File::create(&tgz_path).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all(".", &tree).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    std::fs::read(&tgz_path).unwrap()
}

#[tokio::test]
async fn dry_run_resolves_downloads_verifies_and_stages_against_a_mocked_release() {
    let triple = target_triple();
    let tarball_name = format!("super-stt-{triple}-beta.tar.gz");

    let dir = std::env::temp_dir().join(format!(
        "sstt-install-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let tarball_bytes = build_fixture_tarball(&dir);
    let sums_text = format!("{}  {tarball_name}\n", sha256_hex(&tarball_bytes));

    let mut server = mockito::Server::new_async().await;
    let base = server.url();

    let releases_json = serde_json::json!([{
        "tag_name": FAKE_TAG,
        "draft": false,
        "prerelease": true,
        "assets": [
            {
                "name": tarball_name,
                "browser_download_url": format!("{base}/tarball"),
                "size": tarball_bytes.len(),
            },
            {
                "name": "SHA256SUMS",
                "browser_download_url": format!("{base}/sums"),
                "size": sums_text.len(),
            },
        ],
    }])
    .to_string();

    let _releases_mock = server
        .mock(
            "GET",
            "/repos/jorge-menjivar/super-stt/releases?per_page=100",
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(&releases_json)
        .create_async()
        .await;
    let _tarball_mock = server
        .mock("GET", "/tarball")
        .with_status(200)
        .with_body(&tarball_bytes)
        .create_async()
        .await;
    let _sums_mock = server
        .mock("GET", "/sums")
        .with_status(200)
        .with_body(&sums_text)
        .create_async()
        .await;

    let out = tokio::task::spawn_blocking({
        let base = base.clone();
        move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_super-stt-install"))
                .env("GITHUB_API_BASE", base)
                .args([
                    "--non-interactive",
                    "--json-progress",
                    "--beta",
                    "--dry-run",
                    COMPONENTS_ARG,
                ])
                .output()
                .unwrap()
        }
    })
    .await
    .unwrap();

    if !out.status.success() {
        let _ = std::io::stderr().write_all(&out.stderr);
    }
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8(out.stdout).unwrap();
    let events: Vec<serde_json::Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad JSON line {l:?}: {e}")))
        .collect();
    assert!(!events.is_empty(), "expected at least one JSON event");
    assert!(
        events
            .iter()
            .any(|e| e["event"] == "phase" && e["phase"] == "download"),
        "expected a download phase event: {events:?}"
    );
    assert!(
        events.iter().any(|e| e["event"] == "progress"),
        "expected at least one progress event: {events:?}"
    );

    let complete = events.last().unwrap();
    assert_eq!(complete["event"], "complete");
    assert_eq!(complete["installed_version"], FAKE_TAG);
    let components: Vec<String> = complete["components"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(components, vec!["daemon", "app", "applet"]);

    let _ = std::fs::remove_dir_all(&dir);
}
