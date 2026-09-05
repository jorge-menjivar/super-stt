// SPDX-License-Identifier: GPL-3.0-only
//! Drives the daemon's real `SubprocessBackend` orchestration (manifest parse,
//! socket, systemd-run spawn, ping/load/status/transcribe, teardown) against the
//! `mock_backend` fixture — no GPU, model, or network. Needs a systemd `--user`
//! session, so it is gated behind `SUPER_STT_TEST_SUBPROCESS=1` and skipped on
//! hosted CI runners.
//!
//! Run: `SUPER_STT_TEST_SUBPROCESS=1` cargo test -p super-stt-daemon \
//!        --features test-fixtures --test `subprocess_mock` -- --nocapture
#![cfg(all(feature = "subprocess-backends", feature = "test-fixtures"))]

use super_stt_daemon::stt_models::subprocess::SubprocessBackend;
use super_stt_daemon::stt_models::transcribe::Transcribe;

/// Install the process-wide rustls crypto provider.
///
/// `SubprocessBackend::spawn` provisions the model's files first, which builds
/// a `reqwest` client — and that panics with "No provider set" when no default
/// provider is installed. The daemon binary installs one at startup; a test
/// driving `spawn` directly has to do it itself. Idempotent, so every test can
/// call it (the `Err` is the "already installed" case).
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Removes the per-test backend dir on scope exit — including panic unwinds, so a
/// failed assertion doesn't leak `~/.cache/super-stt-mock-test-<pid>`.
struct CleanupDir(std::path::PathBuf);
impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const MOCK_TOML: &str = r#"
[backend]
source = "github.com/jorge-menjivar/super-stt-voxtral"
name = "Mock"
version = "0.0.0"
kind = "subprocess"
entrypoint = "mock-backend"
contract = "v2"
id = "app.super-stt.mock"
description = "Test backend."

[[models]]
name = "mock"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]

[[models]]
name = "mock-cleanup"
role = "post_processor"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;

/// Build a backend dir holding the mock manifest and binary, outside `/tmp`:
/// `PrivateTmp=yes` in the systemd sandbox makes `/tmp` private, so
/// `ReadOnlyPaths=/tmp/...` bind mounts fail at namespace setup.
///
/// `suffix` keeps concurrent tests in the same binary off each other's
/// directory.
fn seed_backend_dir(suffix: &str) -> (std::path::PathBuf, CleanupDir) {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(format!(
            "super-stt-mock-test-{}-{suffix}",
            std::process::id()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    let cleanup = CleanupDir(dir.clone());
    std::fs::write(dir.join("backend.toml"), MOCK_TOML).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_mock_backend"), dir.join("mock-backend")).unwrap();
    (dir, cleanup)
}

#[tokio::test]
async fn subprocess_orchestration_against_mock() {
    if std::env::var("SUPER_STT_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    install_crypto_provider();

    let (dir, _cleanup) = seed_backend_dir("orchestration");

    // Spawn + load via the real daemon orchestration.
    let mut backend = SubprocessBackend::spawn(&dir, "mock", "cpu", None, Vec::new())
        .await
        .expect("spawn + load mock backend");

    // Transcribe drives /v1/transcribe → canned text.
    let samples = vec![0.0f32; 1600];
    let text = backend
        .transcribe_audio(&samples, 16000, None)
        .await
        .expect("transcribe");
    assert_eq!(text, "mock transcription");

    backend.shutdown().await.expect("clean shutdown");
}

/// The daemon runs a transcription model and a post-processor at the same time,
/// so two `SubprocessBackend`s must coexist: distinct sockets, distinct systemd
/// units, and a teardown of one that leaves the other serving.
///
/// This is the test that pins the instance naming. Keyed by model name alone —
/// as it was before post-processing existed — the second spawn's
/// `remove_file(&socket)` would unlink the first's live socket and either
/// teardown would stop the other's unit.
#[tokio::test]
async fn two_backends_from_one_directory_run_concurrently() {
    if std::env::var("SUPER_STT_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    install_crypto_provider();

    let (dir, _cleanup) = seed_backend_dir("concurrent");

    let mut transcriber = SubprocessBackend::spawn(&dir, "mock", "cpu", None, Vec::new())
        .await
        .expect("spawn + load the transcription model");
    let mut processor = SubprocessBackend::spawn(&dir, "mock-cleanup", "cpu", None, Vec::new())
        .await
        .expect("spawn + load the post-processor alongside it");

    // Both are live: each answers on its own socket, over its own route.
    let text = transcriber
        .transcribe_audio(&vec![0.0f32; 1600], 16000, None)
        .await
        .expect("the transcription model still serves after the second spawn");
    assert_eq!(text, "mock transcription");

    let processed = processor
        .process_text("um so hello", Some("en"))
        .await
        .expect("the post-processor serves /v1/process");
    assert_eq!(processed, "processed: um so hello");

    // Tearing one down must not disturb the other — the failure mode when the
    // two share a socket path or unit name.
    processor
        .shutdown()
        .await
        .expect("clean processor shutdown");
    let text = transcriber
        .transcribe_audio(&vec![0.0f32; 1600], 16000, None)
        .await
        .expect("the transcription model survives the post-processor's teardown");
    assert_eq!(text, "mock transcription");

    transcriber.shutdown().await.expect("clean shutdown");
}

/// The reported bug, at the level it bites: reloading the *same* model in
/// place. An instance owns its `systemd-run --unit=` name and its socket, both
/// keyed on (backend, model), so a second instance of one model cannot be built
/// while the first still holds them — `systemd-run` refuses the duplicate unit
/// outright.
///
/// Worse, the attempt is not free: the spawn unlinks the socket path before it
/// reaches systemd, so the failed second spawn leaves the *first* instance
/// running but unreachable. That is why "build the replacement, keep the old
/// one if it fails" was never a policy a subprocess backend could honor — and
/// why every load path releases its instance before building the replacement.
/// Stage 2 loaded first, so every in-place reload it was asked for — a device
/// switch, an option change — failed with an opaque systemd error while the
/// card went on showing the model it had just broken.
#[tokio::test]
async fn a_model_reloads_only_once_its_instance_is_released() {
    if std::env::var("SUPER_STT_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    install_crypto_provider();

    let (dir, _cleanup) = seed_backend_dir("reload");

    let mut running = SubprocessBackend::spawn(&dir, "mock-cleanup", "cpu", None, Vec::new())
        .await
        .expect("spawn + load the post-processor");

    // The replacement cannot be built beside it.
    let clash = SubprocessBackend::spawn(&dir, "mock-cleanup", "cpu", None, Vec::new()).await;
    let error = clash
        .err()
        .expect("a second instance of one model must not spawn");
    assert!(
        error.to_string().contains("systemd-run failed"),
        "expected the duplicate unit name to be refused: {error}"
    );

    // And the attempt took the running instance's socket with it.
    assert!(
        running
            .process_text("um so hello", Some("en"))
            .await
            .is_err(),
        "the failed spawn unlinked the live instance's socket, so keeping it \
         was never an option"
    );

    // Released first, the same model comes straight back up — which is what
    // makes unload-then-load the only order that reloads anything.
    running.shutdown().await.expect("clean shutdown");
    let mut reloaded = SubprocessBackend::spawn(&dir, "mock-cleanup", "cpu", None, Vec::new())
        .await
        .expect("the model reloads once its instance is released");
    let processed = reloaded
        .process_text("um so hello", Some("en"))
        .await
        .expect("the reloaded instance serves");
    assert_eq!(processed, "processed: um so hello");

    reloaded.shutdown().await.expect("clean shutdown");
}

/// The contract says every `/v1` request carries the user's `[[options]]` as
/// `x-stt-option-*` headers, whichever transport. A subprocess backend that
/// steers on an option — a register, a style prompt — reads them off the
/// request, so the daemon has to put them there; the mock echoes what it got.
#[tokio::test]
async fn option_headers_reach_the_subprocess() {
    if std::env::var("SUPER_STT_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }
    install_crypto_provider();

    let (dir, _cleanup) = seed_backend_dir("headers");

    let headers = vec![
        ("x-stt-option-styling".to_string(), "formal".to_string()),
        ("x-stt-option-context".to_string(), "email".to_string()),
    ];
    let mut processor = SubprocessBackend::spawn(&dir, "mock-cleanup", "cpu", None, headers)
        .await
        .expect("spawn + load the post-processor");

    let processed = processor
        .process_text("um so hello", Some("en"))
        .await
        .expect("the post-processor serves /v1/process");
    assert_eq!(
        processed, "processed: um so hello [context=email styling=formal]",
        "the option headers must arrive on the request"
    );

    processor.shutdown().await.expect("clean shutdown");
}
