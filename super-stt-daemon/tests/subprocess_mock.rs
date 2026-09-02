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
    let mut backend = SubprocessBackend::spawn(&dir, "mock", "cpu", None)
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

    let mut transcriber = SubprocessBackend::spawn(&dir, "mock", "cpu", None)
        .await
        .expect("spawn + load the transcription model");
    let mut processor = SubprocessBackend::spawn(&dir, "mock-cleanup", "cpu", None)
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
