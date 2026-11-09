// SPDX-License-Identifier: GPL-3.0-only
//! `v1/pipeline/voice` — text to speech's per-model voice.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::pipeline::voice;

fn voice_envelope(model_override: Option<&str>) -> serde_json::Value {
    json!({
        "status": "success",
        "voice": {
            "effective": model_override.unwrap_or("af_heart"),
            "override": model_override,
            "default": "af_heart",
            "kinds": ["preset"],
        },
    })
}

#[tokio::test]
async fn a_models_voice_is_read_on_the_synthesis_stage() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&voice_envelope(Some("am_adam"))));

    let block = voice::get_model_voice("kokoro/82m".to_string())
        .await
        .expect("voice read");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/tts/pipeline/1/model/kokoro%2F82m/voice");
    assert_eq!(block.model_override.as_deref(), Some("am_adam"));
    assert_eq!(block.default.as_deref(), Some("af_heart"));
    assert_eq!(block.kinds, ["preset"]);
}

#[tokio::test]
async fn setting_a_voice_sends_it_and_returns_the_resolution() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&voice_envelope(Some("am_adam"))));

    let block = voice::set_model_voice("kokoro-82m".to_string(), "am_adam".to_string())
        .await
        .expect("voice set");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/tts/pipeline/1/model/kokoro-82m/voice");
    assert_eq!(request.json(), json!({ "voice": "am_adam" }));
    assert_eq!(block.effective.as_deref(), Some("am_adam"));
}

#[tokio::test]
async fn clearing_a_voice_falls_back_to_the_models_default() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&voice_envelope(None)));

    let block = voice::clear_model_voice("kokoro-82m".to_string())
        .await
        .expect("voice cleared");

    let request = daemon.request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path(), "/tts/pipeline/1/model/kokoro-82m/voice");
    assert_eq!(block.model_override, None);
    assert_eq!(block.effective.as_deref(), Some("af_heart"));
}

#[tokio::test]
async fn the_voice_list_carries_presets_and_cloned_voices() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "available_voices": [
            { "id": "af_heart", "label": "Heart", "kind": "preset" },
            { "id": "c-1", "label": "My voice", "kind": "cloned" },
        ],
    })));

    let voices = voice::list_model_voices("kokoro-82m".to_string())
        .await
        .expect("voice list");

    assert_eq!(
        daemon.request().path(),
        "/tts/pipeline/1/model/kokoro-82m/voice/list"
    );
    let ids: Vec<&str> = voices.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, ["af_heart", "c-1"]);
    assert_eq!(voices[1].kind, "cloned");
}

/// The daemon's refusal reaches the card as its own message.
#[tokio::test]
async fn a_refused_voice_reports_the_daemons_reason() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::status(
        400,
        &json!({ "status": "error", "message": "no such voice" }),
    ));

    let error = voice::set_model_voice("kokoro-82m".to_string(), "nope".to_string())
        .await
        .expect_err("refused");

    assert!(error.to_string().contains("no such voice"), "{error}");
}
