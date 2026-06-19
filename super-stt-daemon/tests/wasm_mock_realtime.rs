// SPDX-License-Identifier: GPL-3.0-only
//! Drives the daemon's real `WasmBackend` REALTIME orchestration (component
//! load, link, `ws-server.handle` via `realtime_session`, plus batch `/v1`
//! ping/status) against a generic mock realtime WASM component fixture — no real
//! backend, model, or network. The realtime analog of `wasm_mock.rs`; like it,
//! needs no systemd session, so it runs in hosted CI (the daemon's realtime
//! transport otherwise has none — the real-backend tests self-skip there).
//!
//! Requires the fixture to be built first:
//!   just build-mock-wasm-realtime-backend
#![cfg(feature = "wasm-backends")]

use std::path::PathBuf;
use std::time::Duration;

use super_stt_daemon::stt_models::transcribe::Transcribe;
use super_stt_daemon::stt_models::wasm::WasmBackend;
use super_stt_daemon::stt_models::wasm::ws_host::{ConsumerStreamTransport, WsFrame};

/// Path to the prebuilt mock realtime component
/// (`just build-mock-wasm-realtime-backend`).
fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-realtime-backend/target/wasm32-wasip2/release/mock_wasm_realtime_backend.wasm",
    );
    p.exists().then_some(p)
}

/// Load the mock through the real host and drive both halves of the realtime
/// world: batch `/v1` ping/status (through the realtime component's
/// incoming-handler), then a realtime `ws-server` session — the daemon pushes a
/// host consumer stream, the mock reads the `start` frame and emits preview +
/// done. Proves the daemon's realtime load/link/`realtime_session` path without
/// any real backend or upstream.
#[tokio::test]
async fn realtime_orchestration_against_mock() {
    let Some(path) = mock_component() else {
        eprintln!(
            "skipping: mock realtime component not built (run `just build-mock-wasm-realtime-backend`)"
        );
        return;
    };

    let backend = WasmBackend::new_realtime(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock realtime backend");

    // Batch `/v1` still works through the realtime world's incoming-handler.
    let ping = backend.ping().await.expect("ping");
    assert_eq!(ping["status"], "success");
    assert_eq!(ping["message"], "pong");
    let status = backend.status().await.expect("status");
    assert_eq!(status["status"], "success");
    assert_eq!(status["state"], "ready");

    // Realtime: drive one session. The mock emits a preview then a done frame
    // after reading the consumer's `start` frame, then closes — no upstream.
    let (consumer_tx, consumer_rx) = tokio::sync::mpsc::unbounded_channel::<WsFrame>();
    let (guest_tx, mut guest_rx) = tokio::sync::mpsc::unbounded_channel::<WsFrame>();
    let transport = ConsumerStreamTransport {
        incoming: consumer_rx,
        outgoing: guest_tx,
    };

    let driver = tokio::spawn(async move {
        consumer_tx
            .send(WsFrame::Text(
                r#"{"type":"start","sample_rate":16000}"#.to_string(),
            ))
            .unwrap();
        consumer_tx
            .send(WsFrame::Text(r#"{"type":"stop"}"#.to_string()))
            .unwrap();
        // Keep the sender alive until the session ends so the guest's first recv
        // (the start frame) doesn't race an early channel close.
        consumer_tx
    });

    let session =
        tokio::time::timeout(Duration::from_secs(30), backend.realtime_session(transport));
    let result = session.await.expect("session timed out");
    let _held = driver.await.unwrap();
    result.expect("session returned an error");

    let mut texts = Vec::new();
    while let Ok(frame) = guest_rx.try_recv() {
        if let WsFrame::Text(s) = frame {
            texts.push(s);
        }
    }
    assert!(
        texts.iter().any(|t| t.contains(r#""type":"preview""#)),
        "expected at least one preview frame; got {texts:?}"
    );
    let done = texts
        .iter()
        .find(|t| t.contains(r#""type":"done""#))
        .unwrap_or_else(|| panic!("expected a done frame; got {texts:?}"));
    assert!(
        done.contains("mock realtime transcription"),
        "done frame should carry the canned transcript; got {done}"
    );
}

/// A realtime model's batch `transcribe_audio` must route through an internal
/// realtime session (the model's batch endpoint would reject it) and return
/// only the final transcript. Loading the mock with `with_realtime()` flips the
/// instance into that mode; the canned session emits a preview + done, and we
/// assert `transcribe_audio` hands back just the `done` transcript.
#[tokio::test]
async fn realtime_model_transcribe_audio_returns_final_transcript() {
    let Some(path) = mock_component() else {
        eprintln!(
            "skipping: mock realtime component not built (run `just build-mock-wasm-realtime-backend`)"
        );
        return;
    };

    let mut backend = WasmBackend::new_realtime(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock realtime backend")
        .with_realtime();

    let text = backend
        .transcribe_audio(&[0.0_f32; 1600], 16000)
        .await
        .expect("realtime-routed transcribe should succeed");
    assert_eq!(text, "mock realtime transcription");
}
