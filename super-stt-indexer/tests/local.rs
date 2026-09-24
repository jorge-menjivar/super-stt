// SPDX-License-Identifier: GPL-3.0-only
//! The `super-stt-indexer` binary, end to end: that it is wired to
//! `super_engine_indexer` and indexes a staged backend.
//!
//! What the indexer does is `super-engine-indexer`'s, and tested there; this
//! is only that Super STT's binary runs it.

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

/// A real staged `.wasm` is hashed and given a URL under the `--base-url`,
/// and `<out>/index.json` has the published shape.
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
