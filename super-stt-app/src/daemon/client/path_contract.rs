// SPDX-License-Identifier: GPL-3.0-only
//! Contract: every path this client calls is one the daemon serves.
//!
//! The client and the daemon are separate crates that never share a type for
//! the thing they most need to agree on — the URL. Each request is a string
//! built here and parsed there, and a mismatch is invisible until runtime,
//! where it surfaces as a `404` inside a settings page that simply renders
//! empty. Nothing in either crate's type system notices; nothing in either
//! crate's test suite did, either, before this.
//!
//! So the paths are read back out of the source and checked against the
//! published `openapi.json` — the same document a third-party client would
//! generate from. Renaming a daemon endpoint and forgetting a call site here
//! now fails the build rather than shipping.
//!
//! Reading source rather than a registry of constants is deliberate: a registry
//! is only honest while every call site uses it, and the first `format!` that
//! skips it is exactly the one that would drift. The literal in the argument is
//! what actually goes on the wire.

use std::collections::BTreeSet;

/// `{source}`, `{}` and `{stage}` all stand for "a value goes here"; the
/// document and the `format!` template spell that differently.
fn normalize(path: &str) -> String {
    let mut out = String::new();
    let mut in_param = false;
    for ch in path.chars() {
        match ch {
            '{' => {
                in_param = true;
                out.push_str("{}");
            }
            '}' => in_param = false,
            _ if in_param => {}
            _ => out.push(ch),
        }
    }
    // A query string is not part of the path.
    out.split('?').next().unwrap_or(&out).to_string()
}

/// Every path in the published document, `/v1` stripped and params normalized.
fn documented() -> BTreeSet<String> {
    let spec =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/protocol/openapi.json");
    let doc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&spec).unwrap_or_else(|e| panic!("read {}: {e}", spec.display())),
    )
    .expect("openapi.json is valid JSON");

    doc["paths"]
        .as_object()
        .expect("paths is an object")
        .keys()
        .map(|p| normalize(p.strip_prefix("/v1").unwrap_or(p)))
        .collect()
}

/// Every `/`-leading string literal under `client/v1/`, with the file it is in.
///
/// Crude on purpose. It over-collects rather than under-collects: a literal
/// that is not a request path is a nuisance to exclude once, where a path this
/// misses is the bug the test exists to catch.
fn literals_in_client_sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("client/v1 is readable") {
            let path = entry.expect("a readable dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let name = path
                    .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let text = std::fs::read_to_string(&path).expect("a readable source file");
                for line in text.lines() {
                    // Doc comments describe paths in prose; they are not calls.
                    if line.trim_start().starts_with("//") {
                        continue;
                    }
                    for lit in string_literals(line) {
                        if lit.starts_with('/') && lit.len() > 1 {
                            out.push((lit, name.clone()));
                        }
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/daemon/client/v1"),
        &mut out,
    );
    assert!(
        !out.is_empty(),
        "found no path literals — the scan is broken"
    );
    out
}

/// The double-quoted literals on one line. Escapes do not appear in paths, so
/// the naive split is enough and stays readable.
fn string_literals(line: &str) -> Vec<String> {
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(std::string::ToString::to_string)
        .collect()
}

/// Every path the client builds is a path the daemon documents.
///
/// The failure names the file, because "some client path is wrong" is not
/// something a reader can act on at 2am.
#[test]
fn every_path_the_client_calls_is_one_the_daemon_serves() {
    let documented = documented();
    let unknown: Vec<String> = literals_in_client_sources()
        .into_iter()
        .filter(|(lit, _)| !documented.contains(&normalize(lit)))
        .map(|(lit, file)| format!("{lit:?} in {file}"))
        .collect();

    assert!(
        unknown.is_empty(),
        "the client calls paths the daemon does not serve:\n  {}\n\n\
         Either the daemon renamed them and this call site was missed, or the \
         literal is not a request path and belongs outside client/v1/.",
        unknown.join("\n  ")
    );
}

/// The scan actually sees the paths, rather than passing because it found none.
///
/// A test whose corpus silently empties is worse than no test: it goes green
/// forever. These are the paths the settings app cannot work without.
#[test]
fn the_scan_finds_the_paths_the_app_depends_on() {
    let found: BTreeSet<String> = literals_in_client_sources()
        .into_iter()
        .map(|(lit, _)| normalize(&lit))
        .collect();
    for required in [
        "/backend/list",
        "/backend/{}/option/{}",
        "/backend/{}/secret/{}",
        "/pipeline/{}",
        "/pipeline/{}/model",
        "/gpu_info",
        "/settings/volume",
    ] {
        assert!(
            found.contains(required),
            "the scan missed {required:?} — it is called from client/v1/, so the \
             scan is no longer reading what it thinks it is"
        );
    }
}

/// Contract: the response keys this client destructures are keys the daemon
/// actually sends.
///
/// The paths above are only half the agreement. A response body is the other
/// half, and it fails *softer*: the client deserializes into a type whose
/// fields are all `Option`, so a key the daemon does not send reads as "absent"
/// rather than as an error. Nothing logs, nothing 404s — a card simply comes up
/// with nothing in it.
///
/// That is not hypothetical. `GET /pipeline/{stage}/model` answers with its
/// slot under `model`, and the client read it off the shared `DaemonResponse`'s
/// `stage_model` field, which the envelope never carries. Every call returned
/// `None`, so the stage came back from a restart having forgotten a selection
/// the daemon had faithfully persisted — and every test passed, because both
/// sides were internally consistent.
///
/// One row per response the app takes apart by name. Only the keys the client
/// actually reads: this is a check on the agreement, not an inventory of the
/// schema.
const RESPONSE_KEYS: &[(&str, &str, &[&str])] = &[
    ("/v1/pipeline/{stage}", "get", &["stage"]),
    ("/v1/pipeline/{stage}/backend/list", "get", &["backends"]),
    ("/v1/pipeline/{stage}/model", "get", &["model"]),
    (
        "/v1/pipeline/{stage}/model/{model}/device",
        "get",
        &["device", "available_devices"],
    ),
    (
        "/v1/pipeline/{stage}/model/{model}/device/list",
        "get",
        &["available_devices"],
    ),
    (
        "/v1/pipeline/{stage}/model/{model}/language",
        "get",
        &["language"],
    ),
    (
        "/v1/pipeline/{stage}/model/{model}/language/list",
        "get",
        &["available_languages"],
    ),
    (
        "/v1/settings/language/list",
        "get",
        &["available_languages"],
    ),
    ("/v1/pipeline", "get", &["pipeline"]),
    ("/v1/backend/list", "get", &["backends"]),
    ("/v1/gpu_info", "get", &["gpu_info"]),
];

/// The property names of an operation's `200` body, resolved through `$ref`.
fn success_body_keys(doc: &serde_json::Value, path: &str, method: &str) -> BTreeSet<String> {
    let schema =
        &doc["paths"][path][method]["responses"]["200"]["content"]["application/json"]["schema"];
    let resolved = match schema["$ref"].as_str() {
        Some(reference) => {
            let name = reference
                .rsplit('/')
                .next()
                .unwrap_or_else(|| panic!("{method} {path}: malformed $ref {reference}"));
            &doc["components"]["schemas"][name]
        }
        None => schema,
    };
    resolved["properties"]
        .as_object()
        .unwrap_or_else(|| {
            panic!("{method} {path}: the 200 body has no properties in openapi.json")
        })
        .keys()
        .cloned()
        .collect()
}

#[test]
fn every_response_key_the_client_reads_is_one_the_daemon_sends() {
    let spec =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/protocol/openapi.json");
    let doc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&spec).unwrap_or_else(|e| panic!("read {}: {e}", spec.display())),
    )
    .expect("openapi.json is valid JSON");

    let mut missing = Vec::new();
    for (path, method, keys) in RESPONSE_KEYS {
        let sent = success_body_keys(&doc, path, method);
        for key in *keys {
            if !sent.contains(*key) {
                missing.push(format!(
                    "{} {}: client reads `{key}`, daemon sends {:?}",
                    method.to_uppercase(),
                    path,
                    sent
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "a response key the client destructures is not one the daemon sends. \
         This reads as an empty card at runtime, not an error:\n  {}",
        missing.join("\n  ")
    );
}
