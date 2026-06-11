// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end harness for the Mistral WASM backend: loads the component,
//! injects the API-key header, and drives the `/v1` contract against a local
//! mock upstream — proving request dispatch, `x-stt-secret-*` injection,
//! `wasi:http` egress through the host allowlist, and response parsing.
//!
//! Requires the component to be built first:
//!   just build-mistral-backend
#![cfg(feature = "wasm-backends")]

use std::path::PathBuf;

use super_stt_daemon::stt_models::transcribe::Transcribe;
use super_stt_daemon::stt_models::wasm::WasmBackend;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Path to the prebuilt component (`just build-mistral-backend`).
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

const SECRET: &str = "x-stt-secret-mistral_api_key";
const BASE_URL: &str = "x-stt-option-base_url";

/// Happy path: the component shapes the Mistral request (bearer auth +
/// multipart model/file), the host permits the allowlisted upstream, and the
/// transcription comes back.
#[tokio::test]
async fn transcribe_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        // Proves the component forwarded the injected x-stt-secret-MISTRAL_API_KEY
        // as the upstream bearer token.
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hello world"
        })))
        .mount(&server)
        .await;

    let authority = server.address().to_string();
    let mut backend = WasmBackend::new_realtime(
        &component_or_skip!(),
        vec![authority.clone()],
        "voxtral-mini-latest".to_string(),
        vec![
            (SECRET.to_string(), "test-key".to_string()),
            (BASE_URL.to_string(), format!("http://{authority}")),
        ],
    )
    .expect("load backend")
    // Mock upstream on loopback; the SSRF guard blocks loopback otherwise.
    .permit_loopback_egress();

    let audio = vec![0.0_f32; 1600];
    let text = backend
        .transcribe_audio(&audio, 16000)
        .await
        .expect("transcription should succeed");
    assert_eq!(text, "hello world");
}

/// The host allowlist blocks egress to a host the configuration does not
/// permit, even though a server is listening there.
#[tokio::test]
async fn allowlist_blocks_disallowed_host() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "should never be reached"
        })))
        .mount(&server)
        .await;

    let mut backend = WasmBackend::new_realtime(
        &component_or_skip!(),
        // Allowlist a different host than the mock is listening on.
        vec!["api.mistral.ai".to_string()],
        "voxtral-mini-latest".to_string(),
        vec![
            (SECRET.to_string(), "test-key".to_string()),
            (BASE_URL.to_string(), server.uri()),
        ],
    )
    .expect("load backend");

    let result = backend.transcribe_audio(&vec![0.0_f32; 100], 16000).await;
    assert!(
        result.is_err(),
        "outbound call to a non-allowlisted host must be blocked"
    );
}

/// SSRF guard: an allowlisted *hostname* that resolves to a loopback address
/// is blocked, even though the host string itself is on the allowlist.
#[tokio::test]
async fn ssrf_blocks_hostname_resolving_to_loopback() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "should never be reached"
        })))
        .mount(&server)
        .await;

    let port = server.address().port();
    let mut backend = WasmBackend::new_realtime(
        &component_or_skip!(),
        // `localhost` is allowlisted by name, but resolves to 127.0.0.1 / ::1.
        vec!["localhost".to_string()],
        "voxtral-mini-latest".to_string(),
        vec![
            (SECRET.to_string(), "test-key".to_string()),
            (BASE_URL.to_string(), format!("http://localhost:{port}")),
        ],
    )
    .expect("load backend");

    let result = backend.transcribe_audio(&vec![0.0_f32; 100], 16000).await;
    assert!(
        result.is_err(),
        "a hostname resolving to loopback must be blocked by the SSRF guard"
    );
}

/// `GET /v1/ping` and `GET /v1/status` smoke test (no upstream needed).
#[tokio::test]
async fn ping_and_status() {
    let backend = WasmBackend::new_realtime(
        &component_or_skip!(),
        Vec::new(),
        "voxtral-mini-latest".to_string(),
        Vec::new(),
    )
    .expect("load backend");

    let ping = backend.ping().await.expect("ping");
    assert_eq!(ping["status"], "success");

    let status = backend.status().await.expect("status");
    assert_eq!(status["status"], "success");
    assert_eq!(status["state"], "ready");
}

/// Optional live test against the real Mistral API. Transcribes a real WAV.
/// Enable with:
///   SUPER_STT_TEST_MISTRAL=1 MISTRAL_API_KEY=... \
///   SUPER_STT_TEST_AUDIO=/path/to/mono.wav cargo test ... -- --nocapture
/// Optionally set SUPER_STT_TEST_EXPECT to a phrase the result must contain.
#[tokio::test]
async fn live_mistral() {
    if std::env::var("SUPER_STT_TEST_MISTRAL").is_err() {
        return;
    }
    let key = std::env::var("MISTRAL_API_KEY").expect("MISTRAL_API_KEY must be set for live test");
    let audio_path =
        std::env::var("SUPER_STT_TEST_AUDIO").expect("SUPER_STT_TEST_AUDIO must point to a WAV");
    let (samples, sample_rate) = read_wav_mono_f32(&audio_path);

    let mut backend = WasmBackend::new_realtime(
        &component_or_skip!(),
        vec!["api.mistral.ai".to_string()],
        "voxtral-mini-latest".to_string(),
        vec![(SECRET.to_string(), key)],
    )
    .expect("load backend");

    let text = backend
        .transcribe_audio(&samples, sample_rate)
        .await
        .expect("live transcription should succeed");
    println!("\n=== LIVE MISTRAL TRANSCRIPTION ===\n{text}\n==================================\n");
    assert!(
        !text.trim().is_empty(),
        "expected a non-empty transcription"
    );
    if let Ok(expect) = std::env::var("SUPER_STT_TEST_EXPECT") {
        assert!(
            text.to_lowercase().contains(&expect.to_lowercase()),
            "transcription {text:?} did not contain {expect:?}"
        );
    }
}

/// Decode a mono WAV file to f32 samples (test helper).
fn read_wav_mono_f32(path: &str) -> (Vec<f32>, u32) {
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .map(|s| f32::from(s.expect("sample")) / f32::from(i16::MAX))
            .collect(),
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .map(|s| s.expect("sample"))
            .collect(),
    };
    (samples, spec.sample_rate)
}
