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

use super_voice_daemon::tts::tts_models::synthesize::ModelInfoData;
use super_voice_daemon::tts::tts_models::v1::{
    CollectingSink, RegisterVoiceRequest, SynthesizeRequest,
};
use super_voice_daemon::tts::tts_models::wasm::WasmBackend;
use super_voice_shared::tts::audio::frames::{FrameKind, Mark, SampleFormat};

/// Path to the prebuilt mock component (`just build-mock-wasm-backend`).
fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-tts-backend/target/wasm32-wasip2/release/mock_wasm_backend.wasm",
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

/// The mock's clip for a cloned voice: a ramp distinct from its own, so a
/// synthesis that ignored the voice would be told apart.
fn clip() -> Vec<u8> {
    (0..480i16).flat_map(|i| (1000 - i).to_le_bytes()).collect()
}

/// Register `voice` with `clip()`.
async fn register(backend: &WasmBackend, voice: &str) {
    backend
        .register_voice(&RegisterVoiceRequest {
            voice,
            transcript: None,
            sample_rate: 24000,
            channels: 1,
            format: "s16le",
            pcm: &clip(),
        })
        .await
        .expect("the mock takes the clip");
}

/// Synthesize in `voice`, answering the audio's bytes or the error.
async fn speak_in(backend: &WasmBackend, voice: &str) -> anyhow::Result<Vec<u8>> {
    let mut sink = CollectingSink::default();
    backend
        .synthesize(
            &SynthesizeRequest {
                text: "hello",
                voice: Some(voice),
                ..Default::default()
            },
            &mut sink,
        )
        .await?;
    Ok(sink
        .frames
        .iter()
        .filter(|f| f.kind == FrameKind::Audio)
        .flat_map(|f| f.payload.iter().copied())
        .collect())
}

/// A cloned voice is registered in one request and spoken in later ones, each
/// to a fresh instance of the component. The clip lives in between in the
/// component's private `/tmp`, which goes when the model unloads.
#[tokio::test]
async fn a_cloned_voice_is_kept_in_a_private_tmp_while_the_model_is_loaded() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let backend = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new())
        .expect("load mock backend");
    let tmp = backend
        .private_tmp()
        .expect("a component using the filesystem gets a private /tmp")
        .to_path_buf();
    assert!(tmp.is_dir());
    assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0, "it starts empty");

    let voice = "voice:2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4";
    register(&backend, voice).await;
    assert!(
        tmp.join("voices/2f8a2d0e-9c31-4e77-b0aa-1c6b2f0a51d4.pcm")
            .is_file(),
        "what the component wrote to its /tmp is in the host directory"
    );
    assert_eq!(speak_in(&backend, voice).await.unwrap(), clip());
    assert_eq!(
        speak_in(&backend, voice).await.unwrap(),
        clip(),
        "and in every request after that"
    );

    backend.unregister_voice(voice).await.expect("unregister");
    let gone = speak_in(&backend, voice).await.unwrap_err().to_string();
    assert!(gone.contains("unknown_voice"), "{gone}");

    register(&backend, voice).await;
    drop(backend);
    assert!(!tmp.exists(), "the private /tmp goes with the model");
}

/// Two loaded models never share a private `/tmp`: one cannot speak in, or
/// read, a voice registered with the other.
#[tokio::test]
async fn loaded_models_do_not_share_a_private_tmp() {
    let Some(path) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let a = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new()).unwrap();
    let b = WasmBackend::new(&path, Vec::new(), "mock".to_string(), Vec::new()).unwrap();
    assert_ne!(a.private_tmp(), b.private_tmp());

    let voice = "voice:5ce36c82-73c9-4740-9cac-c17ba933a1d3";
    register(&a, voice).await;
    assert_eq!(speak_in(&a, voice).await.unwrap(), clip());
    let other = speak_in(&b, voice).await.unwrap_err().to_string();
    assert!(other.contains("unknown_voice"), "{other}");
}

/// A component that does not import `wasi:filesystem` is given no directory
/// at all.
#[tokio::test]
async fn a_component_without_the_filesystem_gets_no_directory() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-backend/target/wasm32-wasip2/release/mock_wasm_backend.wasm",
    );
    if !path.exists() {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    }
    let component = super_engine_daemon::wasm::WasmComponent::load(
        &super_voice_shared::SUPER_VOICE,
        &path,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        false,
        &[],
    )
    .expect("load the speech-to-text mock");
    assert_eq!(component.private_tmp(), None);
}
