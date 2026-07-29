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
contract = "v1"
description = "Test backend."

[[models]]
name = "mock"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;

#[tokio::test]
async fn subprocess_orchestration_against_mock() {
    if std::env::var("SUPER_STT_TEST_SUBPROCESS").is_err() {
        return; // needs a systemd --user session
    }

    // Build a backend dir outside /tmp: PrivateTmp=yes in the systemd sandbox makes
    // /tmp private, so ReadOnlyPaths=/tmp/... bind mounts fail at namespace setup.
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(format!("super-stt-mock-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = CleanupDir(dir.clone());
    std::fs::write(dir.join("backend.toml"), MOCK_TOML).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_mock_backend"), dir.join("mock-backend")).unwrap();

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
