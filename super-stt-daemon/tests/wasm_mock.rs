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

use super_stt_daemon::stt_models::transcribe::Transcribe;
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
        .transcribe_audio(&[0.0_f32; 1600], 16000)
        .await
        .expect("transcription should succeed");
    assert_eq!(text, "mock transcription");
}
