// SPDX-License-Identifier: GPL-3.0-only
//! The `POST /v1/load` body a subprocess backend receives.

use super::*;
use crate::stt_models::backends::manifest::Manifest;

/// Backends released against the earlier `(name, provider)` model identity
/// validate `provider` on load and answer `400 invalid_model` when it is
/// absent — the shipped qwen3-asr backend does exactly this. Dropping the key
/// from the body makes every model of every such backend unloadable, with no
/// version gate that could soften it, so a manifest that declares `provider`
/// must still have it forwarded.
///
/// This is the test that fails if the compatibility echo is deleted before
/// those backends have rolled over.
#[test]
fn load_forwards_the_provider_a_manifest_declares() {
    let body = load_body("whisper-tiny", Some("local_whisper"), "cuda");
    assert_eq!(
        body.get("provider").and_then(serde_json::Value::as_str),
        Some("local_whisper"),
        "/v1/load dropped `provider`; backends validating it answer 400 invalid_model: {body}"
    );
    assert_eq!(
        body.get("name").and_then(serde_json::Value::as_str),
        Some("whisper-tiny")
    );
    assert_eq!(
        body.get("device").and_then(serde_json::Value::as_str),
        Some("cuda")
    );
}

/// The echo is driven by the manifest, not synthesized: a model that declares
/// no `provider` must not gain one, or a backend that *does* validate the key
/// would start rejecting a load it previously accepted.
#[test]
fn load_omits_provider_and_device_when_unset() {
    let body = load_body("whisper-tiny", None, "");
    assert!(
        body.get("provider").is_none(),
        "manifest declared no provider but the load body invented one: {body}"
    );
    assert!(
        body.get("device").is_none(),
        "empty device_pref sent: {body}"
    );
    assert_eq!(
        body.as_object().map(serde_json::Map::len),
        Some(1),
        "load body carries unexpected keys: {body}"
    );
}

/// `device` carries the resolved accelerator, so the daemon sends it only
/// when it has resolved one. A `cpu` preference always resolves — it is its
/// own accelerator — and must keep reaching the backend, since that is what
/// pins a load onto the CPU on a machine that has a GPU.
#[test]
fn load_sends_the_resolved_accelerator_and_never_the_bare_preference() {
    for accel in ["cpu", "cuda", "rocm", "vulkan", "metal"] {
        let body = load_body("whisper-tiny", None, accel);
        assert_eq!(
            body.get("device").and_then(serde_json::Value::as_str),
            Some(accel),
            "the resolved accelerator must reach the backend: {body}"
        );
    }
    // What `resolve_accel` produces when it cannot name an accelerator — an
    // install with no record, which is every backend installed before the
    // record existed. `gpu` is the user's preference, and the contract says
    // this field is not that; an absent `device` means "auto-select", which is
    // the honest signal.
    let unresolved = load_body("whisper-tiny", None, "");
    assert!(
        unresolved.get("device").is_none(),
        "an unresolved accel must omit `device`, not send a preference: {unresolved}"
    );
}

/// End-to-end over the real parser: the value reaching the wire is the one
/// written in `backend.toml`. Guards the whole path, not just `load_body` —
/// a `ModelEntry::provider` that stopped deserializing would leave the unit
/// tests above passing while every real load lost the key.
#[test]
fn a_manifests_provider_reaches_the_load_body() {
    let toml = r#"
[backend]
source = "github.com/jorge-menjivar/super-stt-qwen-asr"
name = "Qwen3 ASR"
version = "0.1.0"
kind = "subprocess"
entrypoint = "super-stt-qwen-asr"
contract = "v1"
description = "Test backend."

[[models]]
name = "qwen3-asr-flash"
provider = "local_qwen3_asr"
multilingual = true
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cuda"]
"#;
    let manifest = Manifest::parse(toml).expect("fixture manifest parses");
    let model = &manifest.models[0];
    assert_eq!(model.provider.as_deref(), Some("local_qwen3_asr"));

    let body = load_body(&model.name, model.provider.as_deref(), "cuda");
    assert_eq!(
        body.get("provider").and_then(serde_json::Value::as_str),
        Some("local_qwen3_asr"),
        "the manifest's provider did not reach the load body: {body}"
    );
}

/// The instance key separates the daemon's two concurrent backends. Keyed by
/// model name alone — as it was before post-processing — a second spawn would
/// unlink the first's live socket and clash on its systemd unit name.
#[test]
fn the_instance_key_distinguishes_backend_and_model() {
    let a = instance_key(Path::new("/backends/app.super-stt.whisper"), "whisper-tiny");
    let b = instance_key(Path::new("/backends/app.super-stt.whisper"), "cleanup");
    let c = instance_key(Path::new("/backends/com.example.whisper"), "whisper-tiny");

    assert_eq!(a, "app-super-stt-whisper-whisper-tiny");
    assert_ne!(a, b, "two models in one backend must not share an instance");
    assert_ne!(
        a, c,
        "two backends serving the same model name must not share an instance"
    );
}

/// A backend `id` may be up to 255 bytes, and it names the install directory.
/// Left whole, the socket path would exceed `sun_path` (108 bytes) and the bind
/// would fail; the key is bounded instead, and stays unique and deterministic.
#[test]
fn an_over_long_instance_key_is_bounded_but_still_unique() {
    let long = format!("/backends/{}", "a".repeat(255));
    let a = instance_key(Path::new(&long), "whisper-tiny");
    let b = instance_key(Path::new(&long), "cleanup");

    assert!(a.len() <= MAX_INSTANCE_KEY, "key must fit the socket path");
    assert_ne!(
        a, b,
        "truncation must not collapse distinct models together"
    );
    assert_eq!(
        a,
        instance_key(Path::new(&long), "whisper-tiny"),
        "the same input must yield the same key on every spawn"
    );
}
