// SPDX-License-Identifier: GPL-3.0-only
//! True end-to-end test for the Voxtral subprocess backend: the daemon
//! downloads the model from HuggingFace into the per-backend directory, spawns
//! the self-contained backend in a hardened systemd unit, loads it on the GPU,
//! and transcribes a real WAV.
//!
//! Heavy + GPU-bound, so gated. Build the backend first (with CUDA) and stop
//! the daemon to free VRAM:
//!   systemctl --user stop super-stt
//!   just build-voxtral-backend --features cuda
//!   SUPER_STT_TEST_VOXTRAL=1 SUPER_STT_TEST_AUDIO=/tmp/jfk.wav \
//!     cargo test -p super-stt-daemon --features subprocess-backends \
//!     --test voxtral_subprocess -- --nocapture
//! First run downloads ~9 GB; a stable backend dir (SUPER_STT_TEST_BACKEND_DIR
//! or ~/.local/share/super-stt-voxtral-backend) is reused on later runs.
#![cfg(feature = "subprocess-backends")]

use std::path::PathBuf;

use super_stt_daemon::stt_models::subprocess::SubprocessBackend;
use super_stt_daemon::stt_models::transcribe::Transcribe;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn backend_dir() -> PathBuf {
    if let Ok(p) = std::env::var("SUPER_STT_TEST_BACKEND_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").expect("HOME");
    PathBuf::from(home).join(".local/share/super-stt-voxtral-backend")
}

/// Decode a mono WAV to f32 samples.
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

#[tokio::test]
async fn voxtral_subprocess_e2e() {
    if std::env::var("SUPER_STT_TEST_VOXTRAL").is_err() {
        return;
    }
    let audio_path = std::env::var("SUPER_STT_TEST_AUDIO").expect("SUPER_STT_TEST_AUDIO");

    let src_toml = repo_root().join("backends/voxtral/backend.toml");
    let src_bin = repo_root().join("backends/voxtral/target/release/super-stt-backend-voxtral");
    assert!(
        src_bin.exists(),
        "build the backend first: just build-voxtral-backend --features cuda (missing {})",
        src_bin.display()
    );

    // Set up the per-backend directory (backend.toml + the entrypoint binary).
    let dir = backend_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&src_toml, dir.join("backend.toml")).unwrap();
    std::fs::copy(&src_bin, dir.join("super-stt-backend-voxtral")).unwrap();

    // Daemon: download from HF → place in backend dir → spawn sandboxed → load on GPU.
    let mut backend = SubprocessBackend::spawn(&dir, "voxtral-mini", "", None)
        .await
        .expect("spawn + load voxtral subprocess backend");

    let (samples, sample_rate) = read_wav_mono_f32(&audio_path);
    let text = backend
        .transcribe_audio(&samples, sample_rate)
        .await
        .expect("transcription should succeed");

    println!(
        "\n=== VOXTRAL SUBPROCESS TRANSCRIPTION ===\n{text}\n=======================================\n"
    );
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
