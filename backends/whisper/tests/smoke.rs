// SPDX-License-Identifier: GPL-3.0-only
//! CPU smoke tests. Ignored by default — opt in with
//! `cargo test --manifest-path backends/whisper/Cargo.toml -- --ignored`.
//!
//! Each test downloads its model into a per-user cache directory on first run
//! and reuses the cached files thereafter, so repeat runs are fast.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use hound::WavReader;
use reqwest::blocking::Client;
use super_stt_backend_whisper::inference::WhisperEngine;

fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .expect("cache dir")
        .join("super-stt-whisper-test")
}

fn ensure_model(name: &str, repo: &str) -> PathBuf {
    let dir = cache_dir().join(name);
    fs::create_dir_all(&dir).unwrap();
    let client = Client::builder()
        .timeout(std::time::Duration::from_mins(10))
        .build()
        .unwrap();
    for file in ["config.json", "tokenizer.json", "model.safetensors"] {
        let dest = dir.join(file);
        if dest.exists() && fs::metadata(&dest).unwrap().len() > 0 {
            continue;
        }
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        eprintln!("smoke: downloading {url}");
        let mut resp = client.get(&url).send().unwrap().error_for_status().unwrap();
        let tmp = dir.join(format!(".{file}.tmp"));
        let mut out = fs::File::create(&tmp).unwrap();
        resp.copy_to(&mut out).unwrap();
        out.flush().unwrap();
        fs::rename(&tmp, &dest).unwrap();
    }
    dir
}

fn model_name(dir: &Path) -> &str {
    dir.file_name().unwrap().to_str().unwrap()
}

fn load_wav(path: &Path) -> Vec<f32> {
    let mut reader = WavReader::open(path).expect("open wav");
    assert_eq!(reader.spec().sample_rate, 16000, "expected 16 kHz wav");
    assert_eq!(reader.spec().channels, 1, "expected mono wav");
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect()
}

#[test]
#[ignore = "pulls ~75 MB of weights and runs CPU inference"]
fn transcribes_tiny() {
    let dir = ensure_model("whisper-tiny", "openai/whisper-tiny");
    let audio = load_wav(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/jfk.wav"
    )));
    let mut engine =
        WhisperEngine::load(&dir, model_name(&dir), /* force_cpu */ true).expect("load");
    let text = engine.transcribe(&audio, 16000, None).expect("transcribe");
    eprintln!("tiny: {text:?}");
    let lower = text.to_lowercase();
    assert!(!text.trim().is_empty(), "tiny produced empty output");
    assert!(
        lower.contains("ask not") || lower.contains("country"),
        "tiny output should reference the JFK quote; got {text:?}"
    );
}

#[test]
#[ignore = "pulls ~75 MB of weights and runs CPU inference"]
fn transcribes_tiny_en() {
    let dir = ensure_model("whisper-tiny.en", "openai/whisper-tiny.en");
    let audio = load_wav(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/jfk.wav"
    )));
    let mut engine =
        WhisperEngine::load(&dir, model_name(&dir), /* force_cpu */ true).expect("load");
    assert!(engine.is_english_only(), ".en model should be english_only");
    let text = engine.transcribe(&audio, 16000, None).expect("transcribe");
    eprintln!("tiny.en: {text:?}");
    let lower = text.to_lowercase();
    assert!(!text.trim().is_empty(), "tiny.en produced empty output");
    assert!(
        lower.contains("ask not") || lower.contains("country"),
        "tiny.en output should reference the JFK quote; got {text:?}"
    );
}
