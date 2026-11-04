// SPDX-License-Identifier: GPL-3.0-only
//! Hermetic, offline end-to-end test: drives the real `super-tts-install`
//! binary through resolve → download → verify → stage against a mocked
//! GitHub API (`GITHUB_API_BASE`, honored by `Github::from_env` /
//! `accept_base_url` — loopback `http://` is allowed for exactly this),
//! with `--dry-run` so nothing is installed and no escalation happens.
//! Safe to run locally: no root, no keyring, no real network.

use std::io::Write;

/// `--components=all` is passed explicitly rather than relying on
/// auto-detection: auto-detect reads the *real* `/usr/local/bin` on
/// whatever machine runs this test (a dev box with Super TTS already
/// installed would otherwise flip into "update mode" and require applet
/// assets the fixture below may or may not carry, making the test's
/// outcome depend on host state instead of the fixture). An explicit
/// selection makes `stage::plan_components` skip detection entirely.
const COMPONENTS_ARG: &str = "--components=all";

/// The release tag this test's fake GitHub API always returns.
const FAKE_TAG: &str = "v9.9.9-beta.1";

/// The applet release's tag: newer than any applet a host running this test
/// may have installed, which the installer would otherwise keep, skipping
/// the download this test checks.
const FAKE_APPLET_TAG: &str = "v99.0.0-beta.1";

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
/// `super_tts_registry_types::verify::file_sha256_hex`).
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    hex::encode(digest.as_ref())
}

/// A gzip'd tar of `files` (path, contents) under `dir`, as `name`.
fn tarball(dir: &std::path::Path, name: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let tree = dir.join(format!("{name}.tree"));
    for (path, contents) in files {
        let full = tree.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, contents).unwrap();
    }
    let tgz_path = dir.join(name);
    {
        let f = std::fs::File::create(&tgz_path).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all(".", &tree).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    std::fs::read(&tgz_path).unwrap()
}

/// The product's release tarball, covering every file `--components=all`
/// requires of it (super-engine-installer's `stage::build_manifest`): the three daemon binaries, the
/// systemd unit, and the app binary, desktop file and icon. It carries no
/// applet: that comes from the applet's own release. The app metainfo is
/// optional in `build_manifest` and omitted.
fn product_tarball(dir: &std::path::Path, name: &str) -> Vec<u8> {
    tarball(
        dir,
        name,
        &[
            ("super-tts-daemon", b"#!/bin/sh\necho fake\n"),
            ("super-tts-cli", b"#!/bin/sh\necho fake\n"),
            ("super-tts-consent", b"#!/bin/sh\necho fake\n"),
            ("super-tts-app", b"#!/bin/sh\necho fake\n"),
            ("systemd/super-tts.service", b"[Unit]\nDescription=fake\n"),
            (
                "resources/super-tts-app.desktop",
                b"[Desktop Entry]\nName=Super TTS\n",
            ),
            (
                "resources/icons/hicolor/scalable/apps/super-tts-app.svg",
                b"<svg/>",
            ),
        ],
    )
}

/// The applet release's tarball: the binary, one launcher entry (at least one
/// is required, F5) and the icon.
fn applet_tarball(dir: &std::path::Path, name: &str) -> Vec<u8> {
    tarball(
        dir,
        name,
        &[
            ("super-cosmic-applet", b"#!/bin/sh\necho fake\n"),
            (
                "resources/super-cosmic-applet-full.desktop",
                b"[Desktop Entry]\nName=Super Applet\n",
            ),
            (
                "resources/icons/hicolor/scalable/apps/super-cosmic-applet.svg",
                b"<svg/>",
            ),
        ],
    )
}

/// A GitHub releases listing of one prerelease, `tag`, with a tarball and
/// its `SHA256SUMS`, served from `base` at `/<path>/tarball` and `/<path>/sums`.
fn releases_json(
    base: &str,
    path: &str,
    tag: &str,
    tarball: (&str, usize),
    sums_len: usize,
) -> String {
    serde_json::json!([{
        "tag_name": tag,
        "draft": false,
        "prerelease": true,
        "assets": [
            {
                "name": tarball.0,
                "browser_download_url": format!("{base}/{path}/tarball"),
                "size": tarball.1,
            },
            {
                "name": "SHA256SUMS",
                "browser_download_url": format!("{base}/{path}/sums"),
                "size": sums_len,
            },
        ],
    }])
    .to_string()
}

#[tokio::test]
async fn dry_run_resolves_downloads_verifies_and_stages_against_a_mocked_release() {
    let triple = target_triple();
    let tarball_name = format!("super-tts-{triple}-beta.tar.gz");

    let dir = std::env::temp_dir().join(format!(
        "super-engine-installer-e2e-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let tarball_bytes = product_tarball(&dir, &tarball_name);
    let sums_text = format!("{}  {tarball_name}\n", sha256_hex(&tarball_bytes));
    let applet_tarball_name = format!("super-cosmic-applet-{triple}-beta.tar.gz");
    let applet_bytes = applet_tarball(&dir, &applet_tarball_name);
    let applet_sums = format!("{}  {applet_tarball_name}\n", sha256_hex(&applet_bytes));

    let mut server = mockito::Server::new_async().await;
    let base = server.url();

    let mut mocks = Vec::new();
    for (repo, path, tag, name, bytes, sums) in [
        (
            "jorge-menjivar/super-tts",
            "product",
            FAKE_TAG,
            &tarball_name,
            &tarball_bytes,
            &sums_text,
        ),
        (
            "super-libre/super-cosmic-applet",
            "applet",
            FAKE_APPLET_TAG,
            &applet_tarball_name,
            &applet_bytes,
            &applet_sums,
        ),
    ] {
        let listing = releases_json(&base, path, tag, (name, bytes.len()), sums.len());
        mocks.push(
            server
                .mock(
                    "GET",
                    format!("/repos/{repo}/releases?per_page=100").as_str(),
                )
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(listing)
                .expect(1)
                .create_async()
                .await,
        );
        mocks.push(
            server
                .mock("GET", format!("/{path}/tarball").as_str())
                .with_status(200)
                .with_body(bytes)
                .expect(1)
                .create_async()
                .await,
        );
        mocks.push(
            server
                .mock("GET", format!("/{path}/sums").as_str())
                .with_status(200)
                .with_body(sums)
                .expect(1)
                .create_async()
                .await,
        );
    }

    let out = tokio::task::spawn_blocking({
        let base = base.clone();
        move || {
            std::process::Command::new(env!("CARGO_BIN_EXE_super-tts-install"))
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

    // Both releases were resolved, and both tarballs downloaded and checked:
    // the applet's from its own repo, since the product's carries none.
    for mock in &mocks {
        mock.assert_async().await;
    }

    let _ = std::fs::remove_dir_all(&dir);
}
