// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end test for the `super-stt-indexer local` subcommand — the
//! offline index generator that builds an `index.json` from locally-staged
//! `backend.toml` files (no GitHub, no Pages). It backs the daemon's
//! download/install pipeline tests and replaced the old Python offline
//! generator.
//!
//! `local`'s building blocks are unit-tested in `src/local.rs`; this is the
//! binary-level counterpart to `tests/integration.rs` (which covers the
//! GitHub-backed `build` path). It drives the real binary end to end:
//! staging assets, hashing them, multi-manifest output, the
//! `--allow-missing-assets` placeholder path, and the hard-error path when
//! a required asset isn't staged.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_super-stt-indexer");

/// `[0x00, 0x61, 0x73, 0x6d]` is the WebAssembly magic number — a valid,
/// tiny stand-in for a real `.wasm` artifact. Its SHA-256 is fixed, so the
/// index the binary emits is fully deterministic.
const WASM_BYTES: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];
const WASM_SHA256: &str = "cd5d4935a48c0672cb06407bb443bc0087aff947c6b864bac886982c73b3027f";

fn manifest(source: &str, name: &str, version: &str, wasm_file: &str) -> String {
    format!(
        r#"
[backend]
source = "{source}"
name = "{name}"
version = "{version}"
kind = "wasm"
entrypoint = "{wasm_file}"
contract = "v1"
description = "Test backend."
license = "Apache-2.0"

[assets]
wasm = "{wasm_file}"

[[models]]
name = "m-1"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#
    )
}

/// Happy path: a real staged `.wasm` is hashed and given a URL under the
/// `--base-url`, and `<out>/index.json` matches the published `build`
/// shape (id from the source's last segment, `vX.Y.Z` tag, real sha256).
#[test]
fn local_indexes_a_staged_wasm_backend() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();

    // Stage the asset in the output dir, where `local` looks for it.
    std::fs::write(out.join("dummy.wasm"), WASM_BYTES).unwrap();
    // The manifest can live anywhere; pass its path on the command line.
    let manifest_path = dir.path().join("backend.toml");
    std::fs::write(
        &manifest_path,
        manifest(
            "github.com/jorge-menjivar/dummy",
            "Dummy",
            "1.2.3",
            "dummy.wasm",
        ),
    )
    .unwrap();

    let status = Command::new(BIN)
        .arg("local")
        .arg("--out")
        .arg(out)
        .arg("--base-url")
        .arg("http://localhost:8787")
        .arg(&manifest_path)
        .status()
        .expect("run indexer local");
    assert!(status.success(), "indexer local failed");

    let text = std::fs::read_to_string(out.join("index.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();

    assert_eq!(v["backends"].as_array().map(Vec::len), Some(1));
    let b = &v["backends"][0];
    // Registry keys entries by the last path segment of `source`.
    assert_eq!(b["id"], "dummy");
    assert_eq!(b["source"], "github.com/jorge-menjivar/dummy");
    assert_eq!(b["version"], "1.2.3");
    assert_eq!(b["tag"], "v1.2.3");
    assert_eq!(b["kind"], "wasm");
    assert_eq!(b["license"], "Apache-2.0");
    // Only-`none` model → the backend is an online provider.
    assert_eq!(b["online"], true);

    let wasm = &b["assets"]["wasm"];
    assert_eq!(wasm["url"], "http://localhost:8787/dummy.wasm");
    assert_eq!(wasm["size"], WASM_BYTES.len());
    assert_eq!(wasm["sha256"], WASM_SHA256);
}

/// `--allow-missing-assets` lets listing/read tests build an index without
/// staging artifacts: the missing asset gets a zeroed placeholder sha and
/// `size: 0`, but the entry is otherwise complete.
#[test]
fn local_emits_placeholder_for_missing_asset() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    let manifest_path = dir.path().join("backend.toml");
    std::fs::write(
        &manifest_path,
        manifest("github.com/x/ghost", "Ghost", "0.1.0", "ghost.wasm"),
    )
    .unwrap();

    let status = Command::new(BIN)
        .arg("local")
        .arg("--out")
        .arg(out)
        .arg("--allow-missing-assets")
        .arg(&manifest_path)
        .status()
        .expect("run indexer local --allow-missing-assets");
    assert!(status.success(), "indexer local with placeholder failed");

    let text = std::fs::read_to_string(out.join("index.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let wasm = &v["backends"][0]["assets"]["wasm"];
    assert_eq!(wasm["size"], 0, "missing asset → placeholder size 0");
    assert_eq!(
        wasm["sha256"], "0000000000000000000000000000000000000000000000000000000000000000",
        "missing asset → all-zero placeholder sha"
    );
    // The URL is still derived from the (default) base-url + declared file.
    assert_eq!(wasm["url"], "http://localhost:8787/ghost.wasm");
}

/// Without `--allow-missing-assets`, a declared-but-unstaged wasm asset is
/// a hard error: the binary exits non-zero and writes no index.
#[test]
fn local_errors_on_unstaged_asset_without_flag() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    let manifest_path = dir.path().join("backend.toml");
    std::fs::write(
        &manifest_path,
        manifest("github.com/x/ghost", "Ghost", "0.1.0", "ghost.wasm"),
    )
    .unwrap();

    let output = Command::new(BIN)
        .arg("local")
        .arg("--out")
        .arg(out)
        .arg(&manifest_path)
        .output()
        .expect("run indexer local (missing asset)");
    assert!(
        !output.status.success(),
        "a missing required asset must fail the build"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ghost.wasm") || stderr.to_lowercase().contains("not found"),
        "error should name the missing asset; stderr=`{stderr}`"
    );
    assert!(
        !out.join("index.json").exists(),
        "no index.json should be written when a required asset is missing"
    );
}

/// Multiple `backend.toml` paths produce one index entry each, in order.
#[test]
fn local_indexes_multiple_manifests() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path();
    std::fs::write(out.join("a.wasm"), WASM_BYTES).unwrap();
    std::fs::write(out.join("b.wasm"), WASM_BYTES).unwrap();

    let m_a = dir.path().join("a.toml");
    let m_b = dir.path().join("b.toml");
    std::fs::write(
        &m_a,
        manifest("github.com/x/alpha", "Alpha", "1.0.0", "a.wasm"),
    )
    .unwrap();
    std::fs::write(
        &m_b,
        manifest("github.com/x/bravo", "Bravo", "2.0.0", "b.wasm"),
    )
    .unwrap();

    let status = Command::new(BIN)
        .arg("local")
        .arg("--out")
        .arg(out)
        .arg(&m_a)
        .arg(&m_b)
        .status()
        .expect("run indexer local (multi)");
    assert!(status.success(), "indexer local multi-manifest failed");

    let text = std::fs::read_to_string(out.join("index.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let backends = v["backends"].as_array().expect("backends array");
    assert_eq!(backends.len(), 2, "one entry per manifest");
    let ids: Vec<&str> = backends.iter().filter_map(|b| b["id"].as_str()).collect();
    assert!(ids.contains(&"alpha"), "alpha indexed: {ids:?}");
    assert!(ids.contains(&"bravo"), "bravo indexed: {ids:?}");
}
