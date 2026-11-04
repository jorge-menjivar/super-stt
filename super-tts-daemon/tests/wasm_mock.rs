// SPDX-License-Identifier: GPL-3.0-only
//! Drives the daemon's real `WasmBackend` orchestration (component load, link,
//! `/v1` ping/status/synthesize) against a generic mock WASM component fixture —
//! no real backend, model, or network. The WASM analog of `subprocess_mock.rs`;
//! unlike that test it needs no systemd session, so it runs in hosted CI.
//!
//! Requires the fixture to be built first:
//!   just build-mock-wasm-backend
#![cfg(feature = "wasm-backends")]

use std::path::PathBuf;
use std::time::Duration;

use super_tts_daemon::tts_models::synthesize::ModelInfoData;
use super_tts_daemon::tts_models::v1::{CollectingSink, SynthesizeRequest};
use super_tts_daemon::tts_models::wasm::WasmBackend;
use super_tts_shared::audio::frames::{FrameKind, Mark, SampleFormat};

/// Path to the prebuilt mock component (`just build-mock-wasm-backend`).
fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-backend/target/wasm32-wasip2/release/mock_wasm_backend.wasm",
    );
    p.exists().then_some(p)
}

/// Load the mock through the real host and drive the no-network lifecycle
/// routes the daemon hits: ping → status (ready). Proves the daemon's component
/// load/link/invoke path without any real backend; `/v1/synthesize` — the route
/// with a body shape worth checking — is the next test's subject.
#[tokio::test]
async fn wasm_orchestration_against_mock() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };

    let backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");

    let ping = backend.ping().await.expect("ping");
    assert_eq!(ping["status"], "success");
    assert_eq!(ping["message"], "pong");

    let status = backend.status().await.expect("status");
    assert_eq!(status["status"], "success");
    assert_eq!(status["state"], "ready");
}

/// `/v1/synthesize` end to end through the real host: the mock emits a known
/// s16le ramp across four audio frames plus a mark and a `done`, and the host
/// must reassemble exactly that.
///
/// The ramp is what makes this test load-bearing — silence or a constant would
/// survive a decoder that dropped a frame, misordered chunks, or lost the
/// bytes straddling a frame boundary.
#[tokio::test]
async fn wasm_synthesize_streams_known_pcm() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");

    let mut sink = CollectingSink::default();
    backend
        .synthesize(
            &SynthesizeRequest {
                text: "hello",
                ..Default::default()
            },
            &mut sink,
        )
        .await
        .expect("synthesis should succeed");

    let params = sink.params.expect("params arrive before any frame");
    assert_eq!(params.sample_rate, 24000);
    assert_eq!(params.channels, 1);
    assert_eq!(params.format, SampleFormat::S16Le);

    let audio: Vec<_> = sink
        .frames
        .iter()
        .filter(|f| f.kind == FrameKind::Audio)
        .collect();
    assert_eq!(audio.len(), 4, "the mock splits the ramp across 4 frames");

    let samples = sink.samples();
    assert_eq!(samples.len(), 960);
    for (i, got) in samples.iter().enumerate() {
        let want = f32::from(i16::try_from(i).unwrap()) / 32768.0;
        assert!(
            (got - want).abs() < 1e-6,
            "sample {i}: got {got}, want {want}"
        );
    }

    let mark = sink
        .frames
        .iter()
        .find(|f| f.kind == FrameKind::Mark)
        .expect("the mock emits one mark");
    let mark: Mark = serde_json::from_slice(&mark.payload).expect("mark is JSON");
    assert_eq!(mark.start_char, Some(0));
    assert_eq!(mark.end_char, Some(5));

    assert_eq!(
        sink.frames.last().expect("frames were received").kind,
        FrameKind::Done,
        "the stream must end with a terminal frame"
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
            "github.com/super-tts/mock",
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
