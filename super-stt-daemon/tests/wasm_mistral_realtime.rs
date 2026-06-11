// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end test of the Mistral realtime WASM backend against a mock
//! Mistral realtime WebSocket upstream. Drives a consumer transport through
//! `WasmBackend::realtime_session` and asserts preview + done frames return.
//!
//! Requires the component built first: `just build-mistral-backend`.
#![cfg(feature = "wasm-backends")]

use std::path::PathBuf;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super_stt_daemon::stt_models::transcribe::Transcribe;
use super_stt_daemon::stt_models::wasm::WasmBackend;
use super_stt_daemon::stt_models::wasm::ws_host::{ConsumerStreamTransport, WsFrame};

fn component_path() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../backends/mistral/target/wasm32-wasip2/release/super_stt_backend_mistral.wasm");
    p.exists().then_some(p)
}

/// Yield the component path, or skip the test (print + early return) when it
/// isn't built — CI doesn't build backends, and a backend may have moved to
/// its own repo. Build locally with `just build-mistral-backend`.
macro_rules! component_or_skip {
    () => {
        match component_path() {
            Some(p) => p,
            None => {
                eprintln!(
                    "skipping: WASM component not built (run the matching `just build-*-backend`)"
                );
                return;
            }
        }
    };
}

/// Mock Mistral realtime upstream. Accepts the WS upgrade, consumes
/// session.update + input_audio_buffer.append*, and on commit replies with two
/// `...delta` events then one `...completed`. Returns the bound authority
/// (host:port) and the accept task handle.
async fn start_mock_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let authority = listener.local_addr().unwrap().to_string(); // "127.0.0.1:PORT"
    let handle = tokio::spawn(async move {
        // Accept exactly one upstream connection (the guest's).
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut ws) = accept_async(tcp).await else {
            return;
        };
        while let Some(Ok(msg)) = ws.next().await {
            // session.update / append: consume silently; only react to commit.
            if let WsMessage::Text(t) = msg
                && t.as_str().contains("input_audio_buffer.commit")
            {
                let _ = ws
                    .send(WsMessage::Text(
                        r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"hello "}"#
                            .into(),
                    ))
                    .await;
                let _ = ws
                    .send(WsMessage::Text(
                        r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"world"}"#
                            .into(),
                    ))
                    .await;
                let _ = ws
                    .send(WsMessage::Text(
                        r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"hello world"}"#
                            .into(),
                    ))
                    .await;
                // Done; the guest will close after `completed`.
                break;
            }
        }
    });
    (authority, handle)
}

#[tokio::test]
async fn realtime_round_trip() {
    let (authority, _mock) = start_mock_upstream().await;

    // The guest builds the upstream URL from x-stt-option-base_url. Point it at
    // the mock over plaintext ws:// (http:// -> ws:// in the guest). The mock is
    // on loopback, which the SSRF guard blocks for untrusted backends, so the
    // test opts in via `permit_loopback_egress` below.
    let backend = WasmBackend::new_realtime(
        &component_or_skip!(),
        vec![authority.clone()],
        "voxtral-mini-transcribe-realtime-2602".to_string(),
        vec![
            (
                "x-stt-secret-mistral_api_key".to_string(),
                "test-key".to_string(),
            ),
            (
                "x-stt-option-base_url".to_string(),
                format!("http://{authority}"),
            ),
        ],
    )
    .expect("load backend")
    .permit_loopback_egress();

    // Channels: consumer_tx -> guest (incoming); guest -> guest_rx (outgoing).
    let (consumer_tx, consumer_rx) = tokio::sync::mpsc::unbounded_channel::<WsFrame>();
    let (guest_tx, mut guest_rx) = tokio::sync::mpsc::unbounded_channel::<WsFrame>();
    let transport = ConsumerStreamTransport {
        incoming: consumer_rx,
        outgoing: guest_tx,
    };

    // Drive the consumer side concurrently with the session.
    let driver = tokio::spawn(async move {
        consumer_tx
            .send(WsFrame::Text(
                r#"{"type":"start","sample_rate":16000}"#.to_string(),
            ))
            .unwrap();
        // A couple of PCM chunks (silence is fine for the mock).
        consumer_tx.send(WsFrame::Binary(vec![0u8; 3200])).unwrap();
        consumer_tx.send(WsFrame::Binary(vec![0u8; 3200])).unwrap();
        consumer_tx
            .send(WsFrame::Text(r#"{"type":"stop"}"#.to_string()))
            .unwrap();
        // Keep consumer_tx alive briefly so the guest doesn't see an early close
        // before reading `stop`. Dropping it after is harmless.
        consumer_tx
    });

    // Run the session with a timeout so a hang fails loudly.
    let session =
        tokio::time::timeout(Duration::from_secs(30), backend.realtime_session(transport));
    let result = session.await.expect("session timed out");
    let _held = driver.await.unwrap(); // keep consumer_tx alive until session ends
    result.expect("session returned an error");

    // Collect everything the guest sent to the consumer.
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
        done.contains("hello world"),
        "done frame should contain the transcript; got {done}"
    );
}
