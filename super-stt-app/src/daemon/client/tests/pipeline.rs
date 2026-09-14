// SPDX-License-Identifier: GPL-3.0-only
//! `v1/pipeline` — the stages a transcript passes through: the backend filling
//! one, the model it runs, and that model's device and language.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::pipeline::{self, backend, device, language, model, stage};

const SOURCE: &str = "github.com/super-stt/openai";

/// A `/pipeline/{stage}` envelope. Every field is required on the wire, so the
/// fixture carries them all and each test varies the one it is about.
fn stage_envelope(source: Option<&str>, enabled: bool) -> serde_json::Value {
    json!({
        "status": "success",
        "stage": {
            "stage": 1,
            "role": "transcription",
            "source": source,
            "name": source.map(|_| "OpenAI"),
            "enabled": enabled,
        },
    })
}

#[tokio::test]
async fn reading_a_stage_reports_the_backend_filling_it() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&stage_envelope(Some(SOURCE), true)));

    let filled = stage::get_stage(1).await.expect("stage read");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/pipeline/1");
    assert_eq!(filled.source.as_deref(), Some(SOURCE));
    assert_eq!(filled.name.as_deref(), Some("OpenAI"));
    assert!(filled.enabled);
}

/// Every stage answers the same path, so the position is the only thing that
/// distinguishes the two requests. It used to be a `&str` constant per stage
/// and the copies drifted.
#[tokio::test]
async fn each_stage_is_addressed_by_its_position() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&stage_envelope(Some(SOURCE), true)))
        .reply(Reply::json(&stage_envelope(None, false)));

    stage::get_stage(1).await.expect("stage 1 read");
    stage::get_stage(2).await.expect("stage 2 read");

    let requests = daemon.requests();
    assert_eq!(requests[0].path(), "/pipeline/1");
    assert_eq!(requests[1].path(), "/pipeline/2");
}

/// A daemon that predates the pipeline omits the object entirely. Reading that
/// as "empty, nothing selected" keeps the settings page loading; failing the
/// read would blank it.
#[tokio::test]
async fn a_stage_missing_from_the_envelope_reads_as_empty() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "success" })));

    let filled = stage::get_stage(1).await.expect("stage read");

    assert_eq!(filled, stage::StageBackend::default());
}

#[tokio::test]
async fn selecting_a_backend_posts_the_source_to_the_stage() {
    let daemon = FakeDaemon::start().await;

    stage::set_stage_backend(2, SOURCE.to_string())
        .await
        .expect("backend selected");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/pipeline/2");
    assert_eq!(request.json(), json!({ "source": SOURCE }));
}

#[tokio::test]
async fn deselecting_a_backend_deletes_the_stage() {
    let daemon = FakeDaemon::start().await;

    stage::clear_stage_backend(2).await.expect("stage emptied");

    let request = daemon.request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path(), "/pipeline/2");
}

#[tokio::test]
async fn the_backends_offered_for_a_stage_come_from_the_daemon() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "backends": [{
            "source": SOURCE,
            "name": "OpenAI",
            "models": [],
            "secrets": [],
            "options": [],
        }],
    })));

    let offered = backend::list_stage_backends(2).await.expect("menu read");

    let request = daemon.request();
    assert_eq!(request.path(), "/pipeline/2/backend/list");
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].source, SOURCE);
}

/// A menu this cannot read is an empty menu. `list_backends` answers the same
/// malformed payload with an error instead — the two wrappers differ, and the
/// difference shows up here rather than in whichever view notices first.
#[tokio::test]
async fn a_stage_menu_that_cannot_be_parsed_offers_nothing() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "backends": [{ "source": SOURCE }],
    })));

    let offered = backend::list_stage_backends(2).await.expect("menu read");

    assert!(offered.is_empty());
}

/// The resolution block is deserialized at the boundary so the views are not
/// poking at a `Value` field by field. One the daemon sent in a shape this
/// build does not know becomes the empty default — the language row renders as
/// unset rather than the page failing to load.
#[tokio::test]
async fn a_language_block_in_an_unknown_shape_reads_as_unset() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "language": "en",
    })));

    let resolution = language::get_model_language(1, "whisper".to_string())
        .await
        .expect("language read");

    assert_eq!(resolution.effective, None);
    assert_eq!(resolution.source, "");
    assert_eq!(resolution.primary, "");
}

/// The model slot's envelope names the slot `model`, where the shared
/// `DaemonResponse` names its own field `stage_model`. Parsing this one off the
/// shared type failed soft — `None` on every call, no error, a card that came
/// up with nothing selected — so the wrapper parses its own envelope and this
/// checks it against the key the daemon actually sends.
#[tokio::test]
async fn the_model_slot_is_read_from_the_envelopes_own_key() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "model": {
            "model": "whisper-large-v3",
            "loaded": true,
            "device": { "preference": "gpu", "resolved_accel": "cuda" },
        },
    })));

    let slot = model::get_stage_model(1).await.expect("slot read");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/pipeline/1/model");
    assert_eq!(slot.model.as_deref(), Some("whisper-large-v3"));
    assert!(slot.loaded);
    assert_eq!(slot.running_device(), Some("cuda"));
}

/// The selection survives an unload, so the card can offer to load the same
/// model again. `loaded` is the only thing that says whether it is up — and an
/// unloaded model is on no device at all, whatever the last resolution said.
#[tokio::test]
async fn an_unloaded_model_keeps_its_selection_and_reports_no_device() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "model": {
            "model": "whisper-large-v3",
            "loaded": false,
            "device": { "preference": "gpu", "resolved_accel": "cuda" },
        },
    })));

    let slot = model::get_stage_model(1).await.expect("slot read");

    assert_eq!(slot.model.as_deref(), Some("whisper-large-v3"));
    assert!(!slot.loaded);
    assert_eq!(slot.running_device(), None);
}

#[tokio::test]
async fn a_slot_read_that_the_daemon_did_not_call_a_success_is_an_error() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "error", "model": {} })));

    let error = model::get_stage_model(1)
        .await
        .expect_err("slot read failed");

    assert!(
        error.to_string().contains("daemon answered error"),
        "got {error}"
    );
}

/// Loading is `POST` with the model, and `source` only when the caller has one
/// — omitted, the daemon resolves the name against the backend already filling
/// the stage.
#[tokio::test]
async fn loading_a_model_sends_the_source_only_when_there_is_one() {
    let daemon = FakeDaemon::start().await;

    model::set_stage_model(1, "whisper-large-v3".to_string(), Some(SOURCE.to_string()))
        .await
        .expect("model loaded");
    model::set_stage_model(2, "qwen3".to_string(), None)
        .await
        .expect("model loaded");

    let requests = daemon.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path(), "/pipeline/1/model");
    assert_eq!(
        requests[0].json(),
        json!({ "model": "whisper-large-v3", "source": SOURCE })
    );
    assert_eq!(requests[1].path(), "/pipeline/2/model");
    assert_eq!(requests[1].json(), json!({ "model": "qwen3" }));
}

#[tokio::test]
async fn unloading_keeps_the_stage_and_cancel_is_addressed_to_it() {
    let daemon = FakeDaemon::start().await;

    model::unload_stage_model(2).await.expect("model unloaded");
    model::cancel_download(2).await.expect("download cancelled");

    let requests = daemon.requests();
    assert_eq!(requests[0].method, "DELETE");
    assert_eq!(requests[0].path(), "/pipeline/2/model");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path(), "/pipeline/2/model/cancel");
    assert_eq!(requests[1].json(), json!({}));
}

/// The polled download is composed out of the slot's `switch` sub-object, and
/// the stage it belongs to is the stage that was asked — the slot reports only
/// its own. A post-processor's progress bar reading stage 1 would put a
/// transcription download's bytes under the wrong card.
#[tokio::test]
async fn a_polled_download_is_attributed_to_the_stage_that_was_asked() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "model": {
            "model": "qwen3",
            "loaded": false,
            "switch": {
                "phase": "downloading",
                "target": { "model": "qwen3", "source": SOURCE },
                "started_at": "2026-09-14T00:00:00Z",
                "download": {
                    "current_file": "model.safetensors",
                    "file_index": 1,
                    "total_files": 3,
                    "bytes_downloaded": 1024,
                    "total_bytes": 4096,
                    "percentage": 25.0,
                    "eta_seconds": 30,
                },
            },
        },
    })));

    let progress = model::get_download_status(2)
        .await
        .expect("status read")
        .expect("a download is in flight");

    assert_eq!(daemon.request().path(), "/pipeline/2/model");
    assert_eq!(progress.stage, 2);
    assert_eq!(progress.model_name, "qwen3");
    assert_eq!(progress.source, SOURCE);
    assert_eq!(progress.current_file, "model.safetensors");
    assert_eq!(progress.status, "downloading");
    assert_eq!(progress.eta_seconds, Some(30));
    assert!(
        progress.error.is_none(),
        "the polled shape carries no error detail"
    );
}

/// A stage with nothing in flight, and one mid-switch but not downloading, are
/// both "no progress to draw" rather than a zeroed bar.
#[tokio::test]
async fn no_switch_and_no_download_both_report_nothing_in_flight() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&json!({
            "status": "success",
            "model": { "model": "qwen3", "loaded": true },
        })))
        .reply(Reply::json(&json!({
            "status": "success",
            "model": {
                "model": "qwen3",
                "loaded": false,
                "switch": { "phase": "loading", "target": {} },
            },
        })));

    assert!(
        model::get_download_status(1)
            .await
            .expect("status read")
            .is_none()
    );
    assert!(
        model::get_download_status(1)
            .await
            .expect("status read")
            .is_none()
    );
}

#[tokio::test]
async fn reading_a_models_device_addresses_it_through_the_stage() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "device": "gpu",
        "resolved_accel": "cuda",
        "available_devices": ["cpu", "gpu"],
    })));

    let read = device::get_model_device(1, "whisper-large-v3".to_string())
        .await
        .expect("device read");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/pipeline/1/model/whisper-large-v3/device");
    assert_eq!(read.device, "gpu");
    assert_eq!(read.resolved_accel.as_deref(), Some("cuda"));
    assert_eq!(read.available_devices, vec!["cpu", "gpu"]);
}

#[tokio::test]
async fn setting_a_models_device_posts_the_preference() {
    let daemon = FakeDaemon::start().await;

    device::set_model_device(1, "whisper-large-v3".to_string(), "cpu".to_string())
        .await
        .expect("device set");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/pipeline/1/model/whisper-large-v3/device");
    assert_eq!(request.json(), json!({ "device": "cpu" }));
}

/// Two lists, two paths: what one model can run on, and what the stage's
/// backend can run anything on. They are separate endpoints because they are
/// separate questions, and a picker that asked the wrong one would offer a
/// device the load then refuses.
#[tokio::test]
async fn the_two_device_lists_are_separate_paths() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&json!({
            "status": "success",
            "available_devices": ["cpu"],
        })))
        .reply(Reply::json(&json!({
            "status": "success",
            "available_devices": ["cpu", "gpu"],
        })));

    let for_model = device::list_model_devices(1, "whisper-large-v3".to_string())
        .await
        .expect("model devices listed");
    let for_stage = device::list_stage_devices(1)
        .await
        .expect("stage devices listed");

    let requests = daemon.requests();
    assert_eq!(
        requests[0].path(),
        "/pipeline/1/model/whisper-large-v3/device/list"
    );
    assert_eq!(requests[1].path(), "/pipeline/1/device/list");
    assert_eq!(for_model, vec!["cpu"]);
    assert_eq!(for_stage, vec!["cpu", "gpu"]);
}

/// An online model has no local compute and the daemon answers with no list at
/// all. Empty is the answer, not a failure.
#[tokio::test]
async fn a_model_with_no_devices_lists_none() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "success" })));

    let listed = device::list_model_devices(1, "gpt-4o-transcribe".to_string())
        .await
        .expect("devices listed");

    assert!(listed.is_empty());
}

/// A model name is not a path segment until it is encoded: the ones that carry
/// a `/` are the HuggingFace-style repo ids, which is most of them.
#[tokio::test]
async fn a_model_name_with_a_slash_is_encoded_into_the_language_path() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "language": { "effective": "en", "source": "override", "primary": "en" },
    })));

    let resolution = language::get_model_language(1, "openai/whisper-large-v3".to_string())
        .await
        .expect("language read");

    let request = daemon.request();
    assert_eq!(
        request.path(),
        "/pipeline/1/model/openai%2Fwhisper-large-v3/language"
    );
    assert_eq!(resolution.effective.as_deref(), Some("en"));
    assert_eq!(resolution.source, "override");
}

#[tokio::test]
async fn setting_and_clearing_a_language_override_share_the_path() {
    let daemon = FakeDaemon::start().await;
    let resolved = json!({
        "status": "success",
        "language": { "effective": "es", "source": "override", "primary": "en" },
    });
    let cleared = json!({
        "status": "success",
        "language": { "effective": "en", "source": "global", "primary": "en" },
    });
    daemon
        .reply(Reply::json(&resolved))
        .reply(Reply::json(&cleared));

    let after_set = language::set_model_language(1, "whisper".to_string(), "es".to_string())
        .await
        .expect("override set");
    let after_clear = language::clear_model_language(1, "whisper".to_string())
        .await
        .expect("override cleared");

    let requests = daemon.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path(), "/pipeline/1/model/whisper/language");
    assert_eq!(requests[0].json(), json!({ "language": "es" }));
    assert_eq!(requests[1].method, "DELETE");
    assert_eq!(requests[1].path(), "/pipeline/1/model/whisper/language");
    assert_eq!(after_set.effective.as_deref(), Some("es"));
    assert_eq!(after_clear.source, "global");
}

/// A monolingual model offers nothing to choose, and that empty list is what
/// tells the picker not to render itself.
#[tokio::test]
async fn the_languages_on_offer_are_a_separate_list() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&json!({
            "status": "success",
            "available_languages": ["auto", "en", "es"],
        })))
        .reply(Reply::json(&json!({ "status": "success" })));

    let multilingual = language::list_model_languages(1, "whisper".to_string())
        .await
        .expect("languages listed");
    let monolingual = language::list_model_languages(1, "parakeet".to_string())
        .await
        .expect("languages listed");

    assert_eq!(
        daemon.requests()[0].path(),
        "/pipeline/1/model/whisper/language/list"
    );
    assert_eq!(multilingual, vec!["auto", "en", "es"]);
    assert!(monolingual.is_empty());
}

/// A card draws a stage's backend and its model together, and those are two
/// endpoints — so the view is two requests, in that order, joined here.
#[tokio::test]
async fn a_stage_view_joins_the_backend_and_the_model_slot() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&stage_envelope(Some(SOURCE), true)))
        .reply(Reply::json(&json!({
            "status": "success",
            "model": {
                "model": "whisper-large-v3",
                "loaded": true,
                "device": { "preference": "gpu", "resolved_accel": "cuda" },
            },
        })));

    let view = pipeline::get_stage_view(1).await.expect("stage view read");

    let requests = daemon.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path(), "/pipeline/1");
    assert_eq!(requests[1].path(), "/pipeline/1/model");
    assert_eq!(view.source.as_deref(), Some(SOURCE));
    assert_eq!(view.name.as_deref(), Some("OpenAI"));
    assert!(view.enabled);
    assert_eq!(
        view.selection(),
        Some(("whisper-large-v3".to_string(), SOURCE.to_string()))
    );
    assert_eq!(view.running_device(), Some("cuda"));
}

/// A half-drawn card is worse than one that says it could not load, so a
/// failure in either half fails the view — and the second request is never
/// made.
#[tokio::test]
async fn a_stage_view_fails_whole_when_its_first_half_fails() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "error",
        "message": "no such stage",
    })));

    let error = pipeline::get_stage_view(9)
        .await
        .expect_err("stage view failed");

    assert_eq!(error.to_string(), "no such stage");
    assert_eq!(daemon.requests().len(), 1, "the model slot is never asked");
}
