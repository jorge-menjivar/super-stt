// SPDX-License-Identifier: GPL-3.0-only
//! One backend serving both features: speech to text and text to speech.
//!
//! A manifest may hold models of every role, and each feature's model picker
//! must show only that feature's models. The app fills each picker from
//! `GET /{feature}/pipeline/{stage}/backend/list`, the backends that can fill
//! the stage narrowed to the models they serve there, so that answer is what
//! these tests pin, together with `.../model/list` for the selected backend and
//! the role gate on picking a model.
//!
//! The fixture's entrypoint is a placeholder: selection and validation happen
//! before any component is loaded.
//!
//! Uses `SUPER_VOICE_AUTO_APPROVE=1` (no GUI), so it is part of the default
//! `cargo test` flow.

mod common;

use common::{Method, StatusCode, TestDaemon};

use std::path::{Path, PathBuf};

const SOURCE: &str = "github.com/test/mixed";

const TRANSCRIPTION: [&str; 2] = ["listen-small", "listen-large"];
const POST_PROCESSOR: [&str; 1] = ["tidy-up"];
const SYNTHESIS: [&str; 2] = ["speak-en", "speak-es"];

/// Seed a backend holding two transcription models, a post-processor and two
/// synthesis models, declared interleaved so that a list in manifest order
/// cannot pass by taking a prefix.
fn seed_mixed_backend(data_home: &Path) {
    let dir = data_home
        .join("super-voice")
        .join("backends")
        .join("fixture-mixed");
    std::fs::create_dir_all(&dir).expect("create the fixture backend dir");
    let toml = r#"[backend]
source = "github.com/test/mixed"
name = "Fixture Mixed"
version = "1.0.0"
kind = "wasm"
entrypoint = "mixed.wasm"
contract = "v4"
product = "super-voice"
id = "app.test.mixed"
description = "Serves both features."
license = "Apache-2.0"

[network]
allowed_hosts = []

[[models]]
name = "listen-small"
role = "transcription"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]

[[models]]
name = "speak-en"
role = "synthesis"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]

[[models]]
name = "tidy-up"
role = "post_processor"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]

[[models]]
name = "listen-large"
role = "transcription"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]

[[models]]
name = "speak-es"
role = "synthesis"
primary_language = "es"
supported_languages = ["es"]
supported_devices = ["cpu"]
"#;
    std::fs::write(dir.join("backend.toml"), toml).expect("write the fixture backend.toml");
    std::fs::write(dir.join("mixed.wasm"), b"").expect("write the placeholder entrypoint");
}

async fn start_daemon() -> (TestDaemon, PathBuf, String) {
    let daemon = common::daemon("mixed");
    seed_mixed_backend(&daemon.home().data);
    let daemon = daemon.start().await;
    let token = daemon.token("mixed-backend", &["settings"]).await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket, token)
}

async fn get(socket: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    common::request(socket, Method::GET, path, Some(token), None).await
}

async fn post(
    socket: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    common::request(socket, Method::POST, path, Some(token), Some(&body)).await
}

/// The model names `path` lists for the fixture backend, in the order given.
async fn listed_models(socket: &PathBuf, path: &str, token: &str) -> Vec<String> {
    let (status, body) = get(socket, path, token).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
    let backend = body["backends"]
        .as_array()
        .expect("backends is a list")
        .iter()
        .find(|b| b["source"] == SOURCE)
        .unwrap_or_else(|| panic!("GET {path} must offer the mixed backend: {body}"));
    names(&backend["models"])
}

/// The `name` of every entry in a JSON list of models.
fn names(models: &serde_json::Value) -> Vec<String> {
    models
        .as_array()
        .expect("models is a list")
        .iter()
        .map(|m| {
            m["name"]
                .as_str()
                .expect("every model has a name")
                .to_string()
        })
        .collect()
}

/// Each stage's picker list holds that stage's models of the mixed backend
/// and nothing else: transcription in stage 1, the post-processor in stage 2,
/// synthesis in text to speech. The whole catalog still carries all five.
#[tokio::test]
async fn each_stage_lists_only_its_own_models_of_a_mixed_backend() {
    let (_guard, sock, token) = start_daemon().await;

    let all = listed_models(&sock, "/backend/list", &token).await;
    let mut expected: Vec<&str> = [TRANSCRIPTION.as_slice(), &POST_PROCESSOR, &SYNTHESIS].concat();
    expected.sort_unstable();
    let mut all_sorted = all.clone();
    all_sorted.sort_unstable();
    assert_eq!(all_sorted, expected, "the catalog lists every model");

    for (path, want) in [
        ("/stt/pipeline/1/backend/list", TRANSCRIPTION.as_slice()),
        ("/stt/pipeline/2/backend/list", POST_PROCESSOR.as_slice()),
        ("/tts/pipeline/1/backend/list", SYNTHESIS.as_slice()),
    ] {
        assert_eq!(
            listed_models(&sock, path, &token).await,
            want,
            "{path} lists only its stage's models, in manifest order"
        );
    }
}

/// Selected in both features at once, the backend fills each stage, and each
/// stage's `model/list` still names only that stage's models.
#[tokio::test]
async fn both_features_select_the_same_backend_and_each_runs_its_own_models() {
    let (_guard, sock, token) = start_daemon().await;

    for stage in ["/stt/pipeline/1", "/stt/pipeline/2", "/tts/pipeline/1"] {
        let (status, body) = post(
            &sock,
            stage,
            &token,
            serde_json::json!({ "source": SOURCE }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "POST {stage}: {body}");
    }
    for stage in ["/stt/pipeline/1", "/stt/pipeline/2", "/tts/pipeline/1"] {
        let (status, body) = get(&sock, stage, &token).await;
        assert_eq!(status, StatusCode::OK, "GET {stage}: {body}");
        assert_eq!(
            body["stage"]["source"], SOURCE,
            "{stage} keeps its selection: {body}"
        );
    }

    for (path, want) in [
        ("/stt/pipeline/1/model/list", TRANSCRIPTION.as_slice()),
        ("/stt/pipeline/2/model/list", POST_PROCESSOR.as_slice()),
        ("/tts/pipeline/1/model/list", SYNTHESIS.as_slice()),
    ] {
        let (status, body) = get(&sock, path, &token).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {body}");
        // Each entry is the `[name, source]` pair `POST .../model` takes.
        let pairs: Vec<(String, String)> =
            serde_json::from_value(body["available_models"].clone()).expect("[name, source] pairs");
        assert!(
            pairs.iter().all(|(_, source)| source == SOURCE),
            "{path} names models of the selected backend only: {body}"
        );
        let names: Vec<&str> = pairs.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, want, "{path} names only its stage's models: {body}");
    }
}

/// Picking a model of another feature or stage is refused with
/// `invalid_model`, in every direction, before anything is loaded.
#[tokio::test]
async fn a_model_of_another_feature_is_refused() {
    let (_guard, sock, token) = start_daemon().await;

    for (path, model) in [
        ("/stt/pipeline/1/model", SYNTHESIS[0]),
        ("/stt/pipeline/1/model", POST_PROCESSOR[0]),
        ("/stt/pipeline/2/model", SYNTHESIS[0]),
        ("/stt/pipeline/2/model", TRANSCRIPTION[0]),
        ("/tts/pipeline/1/model", TRANSCRIPTION[0]),
        ("/tts/pipeline/1/model", POST_PROCESSOR[0]),
    ] {
        let (status, body) = post(
            &sock,
            path,
            &token,
            serde_json::json!({ "model": model, "source": SOURCE }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "POST {path} with {model}: {body}"
        );
        assert_eq!(
            body["error_code"], "invalid_model",
            "POST {path} with {model}: {body}"
        );
    }
}
