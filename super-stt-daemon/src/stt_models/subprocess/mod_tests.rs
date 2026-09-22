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
/// unlink the first's live socket and clash on its sandbox name.
#[test]
fn the_instance_key_distinguishes_backend_and_model() {
    let a = instance_key(
        Path::new("/backends/app.super-stt.whisper"),
        "whisper-tiny",
        MAX_INSTANCE_KEY,
    );
    let b = instance_key(
        Path::new("/backends/app.super-stt.whisper"),
        "cleanup",
        MAX_INSTANCE_KEY,
    );
    let c = instance_key(
        Path::new("/backends/com.example.whisper"),
        "whisper-tiny",
        MAX_INSTANCE_KEY,
    );

    assert_eq!(a, "app-super-stt-whisper-whisper-tiny");
    assert_ne!(a, b, "two models in one backend must not share an instance");
    assert_ne!(
        a, c,
        "two backends serving the same model name must not share an instance"
    );
}

/// A backend `id` may be up to 255 bytes, and it names the install directory.
/// Left whole, the socket path would exceed `sun_path` and the bind would
/// fail; the key is bounded instead, and stays unique and deterministic.
///
/// Checked at both platforms' budgets, not just this host's: macOS leaves
/// roughly half the room Linux does, and a truncation that is only ever
/// exercised at the roomier bound is a bind failure waiting on the other
/// platform.
#[test]
fn an_over_long_instance_key_is_bounded_but_still_unique() {
    let long = format!("/backends/{}", "a".repeat(255));
    for max in [MIN_INSTANCE_KEY, 28, MAX_INSTANCE_KEY] {
        let a = instance_key(Path::new(&long), "whisper-tiny", max);
        let b = instance_key(Path::new(&long), "cleanup", max);

        assert!(a.len() <= max, "key must fit the socket path at max={max}");
        assert_ne!(
            a, b,
            "truncation must not collapse distinct models together at max={max}"
        );
        assert_eq!(
            a,
            instance_key(Path::new(&long), "whisper-tiny", max),
            "the same input must yield the same key on every spawn"
        );
    }
}

/// The budget is computed from the real socket directory, so a deeper
/// runtime path has to yield a shorter name — this is the whole reason the
/// bound is not a constant. macOS is the case that forced it: its per-user
/// runtime directory is around 70 bytes against Linux's 30.
#[test]
fn the_key_budget_shrinks_as_the_socket_directory_deepens() {
    let shallow = max_instance_key(Path::new("/run/user/1000/stt/backends")).expect("fits");
    let deep = max_instance_key(Path::new(
        "/private/var/folders/xt/qnfxwqr938s_rd96dcghph3c0000gn/T/stt/backends",
    ))
    .expect("fits");

    assert!(
        deep < shallow,
        "a deeper socket dir must leave less room: deep={deep} shallow={shallow}"
    );
    assert!(deep >= MIN_INSTANCE_KEY, "the macOS runtime dir must fit");

    // Every byte of the longest name the budget allows, plus the directory,
    // plus `.sock` and the terminator, must still fit `sun_path`.
    for (dir, budget) in [
        ("/run/user/1000/stt/backends", shallow),
        (
            "/private/var/folders/xt/qnfxwqr938s_rd96dcghph3c0000gn/T/stt/backends",
            deep,
        ),
    ] {
        let longest = format!("{dir}/{}.sock", "x".repeat(budget));
        assert!(
            longest.len() < super_stt_shared::validation::SUN_PATH_MAX,
            "{longest} is {} bytes, over sun_path",
            longest.len()
        );
    }
}

/// A runtime directory so deep that no usable name fits is reported as such,
/// rather than producing a name the kernel refuses with a bare `EINVAL` at
/// bind time.
#[test]
fn an_impossibly_deep_socket_directory_is_an_error() {
    let deep = format!(
        "/{}",
        "d".repeat(super_stt_shared::validation::SUN_PATH_MAX)
    );
    let err = max_instance_key(Path::new(&deep)).expect_err("must not claim a name fits");
    assert!(
        err.to_string().contains("too deep"),
        "the error should name the problem: {err}"
    );
}
