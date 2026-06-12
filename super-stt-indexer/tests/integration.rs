// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end test: mock GitHub for `latest_release` + `contents` + asset
//! download; run the indexer binary as a subprocess via the
//! `GITHUB_API_BASE` env override.
//!
//! This is one happy-path test. Pure-unit tests cover failure cases in each
//! module's `#[cfg(test)]` block.

use std::process::Command;

use base64::Engine;

const MANIFEST_OK: &str = r#"
[backend]
source = "github.com/x/y/foo"
name = "Y"
version = "1.0.0"
kind = "wasm"
entrypoint = "y.wasm"
contract = "v1"
license = "Apache-2.0"

[assets]
wasm = "y.wasm"

[[secrets]]
name = "y_api_key"
description = "Key."

[[options]]
name = "base_url"
description = "Override."
default = "https://api.y.example"

[[models]]
name = "y-1"
provider = "openai"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;

const WASM_BYTES: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];
const WASM_SHA256: &str = "cd5d4935a48c0672cb06407bb443bc0087aff947c6b864bac886982c73b3027f";

#[tokio::test]
async fn end_to_end_indexes_a_single_wasm_backend() {
    let mut s = mockito::Server::new_async().await;
    let base = s.url();

    let releases_body = format!(
        r#"{{"tag_name":"v1.0.0","assets":[{{"name":"y.wasm","browser_download_url":"{base}/dl/y.wasm","size":4}}]}}"#
    );
    s.mock("GET", "/repos/x/y/releases/latest")
        .with_status(200)
        .with_body(releases_body)
        .create_async()
        .await;

    let content = base64::engine::general_purpose::STANDARD.encode(MANIFEST_OK);
    let contents_body = format!(r#"{{"content":"{content}","encoding":"base64"}}"#);
    s.mock(
        "GET",
        mockito::Matcher::Regex(r"^/repos/x/y/contents/backend\.toml.*".into()),
    )
    .with_status(200)
    .with_body(contents_body)
    .create_async()
    .await;

    s.mock("GET", "/dl/y.wasm")
        .with_status(200)
        .with_body(WASM_BYTES.as_slice())
        .create_async()
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let registry_path = tmp.path().join("registry.toml");
    std::fs::write(
        &registry_path,
        r#"
        [x-y]
        repo = "github.com/x/y"
    "#,
    )
    .unwrap();

    let out_path = tmp.path().join("index.json");
    let bin = env!("CARGO_BIN_EXE_super-stt-indexer");
    let status = Command::new(bin)
        .env("GITHUB_API_BASE", &base)
        .arg("build")
        .arg("--registry")
        .arg(&registry_path)
        .arg("--out")
        .arg(&out_path)
        .status()
        .expect("run binary");
    assert!(status.success(), "indexer binary failed");

    let text = std::fs::read_to_string(&out_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["backends"][0]["id"], "x-y");
    // The indexer must emit the manifest's namespaced `source`, not the repo
    // (regression guard for REG-01).
    assert_eq!(v["backends"][0]["source"], "github.com/x/y/foo");
    assert_eq!(v["backends"][0]["version"], "1.0.0");
    assert_eq!(v["backends"][0]["assets"]["wasm"]["sha256"], WASM_SHA256);
    assert_eq!(v["backends"][0]["kind"], "wasm");
    assert_eq!(v["backends"][0]["license"], "Apache-2.0");
    // Relaxation fallbacks: a secret without `label` falls back to `name`;
    // an option without `label`/`type` falls back to `name`/"string".
    assert_eq!(v["backends"][0]["secrets"][0]["label"], "y_api_key");
    assert_eq!(v["backends"][0]["options"][0]["label"], "base_url");
    assert_eq!(v["backends"][0]["options"][0]["type"], "string");
    assert_eq!(
        v["backends"][0]["options"][0]["default"],
        "https://api.y.example"
    );
    // The fixture's only model is served by an online provider ("openai") —
    // pins the Provider::Online → `online: true` mapping.
    assert_eq!(v["backends"][0]["online"], true);
}
