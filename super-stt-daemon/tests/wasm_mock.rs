// SPDX-License-Identifier: GPL-3.0-only
//! Drives the daemon's real `WasmBackend` orchestration (component load, link,
//! `/v1` ping/status/transcribe) against a generic mock WASM component fixture —
//! no real backend, model, or network. The WASM analog of `subprocess_mock.rs`;
//! unlike that test it needs no systemd session, so it runs in hosted CI.
//!
//! Requires the fixture to be built first:
//!   just build-mock-wasm-backend
#![cfg(feature = "wasm-backends")]

use std::path::PathBuf;
use std::time::Duration;

use super_stt_daemon::stt_models::transcribe::{ModelInfoData, Transcribe};
use super_stt_daemon::stt_models::wasm::WasmBackend;

/// Path to the prebuilt mock component (`just build-mock-wasm-backend`).
fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-backend/target/wasm32-wasip2/release/mock_wasm_backend.wasm",
    );
    p.exists().then_some(p)
}

/// Load the mock through the real host and drive the no-network `/v1` routes the
/// daemon hits: ping → status (ready) → transcribe (canned text). Proves the
/// daemon's component load/link/invoke path without any real backend.
#[tokio::test]
async fn wasm_orchestration_against_mock() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };

    let mut backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");

    let ping = backend.ping().await.expect("ping");
    assert_eq!(ping["status"], "success");
    assert_eq!(ping["message"], "pong");

    let status = backend.status().await.expect("status");
    assert_eq!(status["status"], "success");
    assert_eq!(status["state"], "ready");

    let text = backend
        .transcribe_audio(&[0.0_f32; 1600], 16000, None)
        .await
        .expect("transcription should succeed");
    assert_eq!(text, "mock transcription");
}

/// The post-processing route rides the same host machinery as transcription:
/// the same `invoke` path, the same injected headers. The mock echoes the text
/// back prefixed, so this proves the transcript reached the component and the
/// rewritten answer came back — not merely that the call returned.
#[tokio::test]
async fn wasm_process_route_against_mock() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };

    let mut backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");

    let processed = backend
        .process_text("um so hello there", Some("en"))
        .await
        .expect("post-processing should succeed");
    assert_eq!(processed, "processed: um so hello there");
}

/// A backend answering `400 invalid_text` surfaces as an `Err`, so the daemon's
/// best-effort layer above can fall back to the raw transcript instead of
/// typing an error string.
#[tokio::test]
async fn wasm_process_surfaces_a_backend_refusal() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };

    let mut backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");

    let err = backend
        .process_text("", None)
        .await
        .expect_err("an empty text is refused by the backend");
    assert!(
        err.to_string().contains("invalid_text"),
        "the backend's own message should surface: {err}"
    );
}

/// The two egress lists must reach the hooks in the right slots. Nothing else
/// covers this: the guard's own tests build argument lists directly, and every
/// other harness here passes an empty user list, so swapping the two adjacent
/// `Vec<String>` parameters of `with_info` — which would hand a backend the SSRF
/// relaxation for hosts it declared in its own manifest — would leave the suite
/// green. Both invocation paths (batch and realtime) build their hooks through
/// `allowlist_hooks`, so asserting on it covers both.
#[tokio::test]
async fn egress_lists_reach_the_hooks_in_their_own_slots() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };

    let backend = WasmBackend::with_info(
        &path,
        vec!["manifest.example".to_string()],
        vec!["gw.example:8443".to_string(), "gw.example".to_string()],
        ModelInfoData::new(
            "mock",
            "github.com/super-stt/mock",
            false,
            true,
            Duration::from_secs(0),
        ),
        Vec::new(),
        false,
        false,
    )
    .expect("load mock backend");

    let hooks = backend.allowlist_hooks();
    assert_eq!(
        &*hooks.allowed_hosts,
        ["manifest.example".to_string()],
        "the manifest list must stay in the SSRF-guarded slot"
    );
    assert_eq!(
        &*hooks.user_allowed_hosts,
        ["gw.example:8443".to_string(), "gw.example".to_string()],
        "the user's endpoint must stay in the relaxed slot"
    );
    assert!(
        !hooks.allow_loopback,
        "loopback egress stays off unless explicitly opted into"
    );
}
