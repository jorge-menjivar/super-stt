// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use crate::models::recording_stop_mode::RecordingStopMode;
use serde_json::{Value, json};

fn make_request(command: &str, data: Option<Value>) -> DaemonRequest {
    DaemonRequest {
        command: command.to_string(),
        audio_data: None,
        sample_rate: None,
        client_id: None,
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        data,
        language: None,
        enabled: None,
    }
}

#[test]
fn record_command_parses_stop_mode() {
    let request = make_request(
        "record",
        Some(json!({
            "write_mode": false,
            "stop_mode": "manual_only",
        })),
    );
    let command = Command::try_from(request).expect("record command should parse");
    match command {
        Command::Record {
            write_mode,
            stop_mode,
            ..
        } => {
            assert!(!write_mode);
            assert_eq!(stop_mode, Some(RecordingStopMode::ManualOnly));
        }
        _ => panic!("expected Command::Record"),
    }
}

#[test]
fn record_command_without_stop_mode_defaults_to_none() {
    let request = make_request("record", Some(json!({ "write_mode": true })));
    let command = Command::try_from(request).expect("record command should parse");
    match command {
        Command::Record {
            write_mode,
            stop_mode,
            ..
        } => {
            assert!(write_mode);
            assert_eq!(stop_mode, None);
        }
        _ => panic!("expected Command::Record"),
    }
}

#[test]
fn record_command_wait_true() {
    let request = make_request(
        "record",
        Some(json!({
            "write_mode": false,
            "stop_mode": "manual_only",
            "wait": true,
        })),
    );
    let command = Command::try_from(request).expect("record command should parse");
    match command {
        Command::Record { wait, .. } => assert!(wait),
        _ => panic!("expected Command::Record"),
    }
}

#[test]
fn record_command_wait_defaults_to_false() {
    let request = make_request("record", Some(json!({ "write_mode": false })));
    let command = Command::try_from(request).expect("record command should parse");
    match command {
        Command::Record { wait, .. } => assert!(!wait),
        _ => panic!("expected Command::Record"),
    }
}

#[test]
fn record_command_invalid_stop_mode_is_rejected() {
    // Tier 1 #26: a present-but-unknown stop_mode is a bad request — reject it
    // (not silently drop to None), consistent with the SET path.
    let request = make_request(
        "record",
        Some(json!({
            "write_mode": false,
            "stop_mode": "not_a_real_mode",
        })),
    );
    assert!(Command::try_from(request).is_err());
}

fn record_audio_cues(data: Value) -> Result<Option<bool>, String> {
    match Command::try_from(make_request("record", Some(data)))? {
        Command::Record { audio_cues, .. } => Ok(audio_cues),
        _ => panic!("expected Command::Record"),
    }
}

#[test]
fn record_command_reads_audio_cues() {
    assert_eq!(
        record_audio_cues(json!({ "audio_cues": false })),
        Ok(Some(false))
    );
    assert_eq!(
        record_audio_cues(json!({ "audio_cues": true })),
        Ok(Some(true))
    );
    // Absent and null both leave the choice to the configured theme.
    assert_eq!(record_audio_cues(json!({})), Ok(None));
    assert_eq!(record_audio_cues(json!({ "audio_cues": null })), Ok(None));
}

#[test]
fn record_command_invalid_audio_cues_is_rejected() {
    // Dropping `"false"` to `None` would play the cues the caller asked to
    // silence, so anything but a boolean is a bad request.
    for bad in [json!("false"), json!(0), json!({})] {
        assert!(
            record_audio_cues(json!({ "audio_cues": bad })).is_err(),
            "audio_cues {bad} must be rejected"
        );
    }
}

#[test]
fn set_recording_stop_mode_invalid_is_rejected() {
    // Tier 1 #26: an unknown mode returns an error and leaves the stored
    // setting unchanged, rather than silently persisting the default.
    let request = make_request(
        "set_recording_stop_mode",
        Some(json!({ "mode": "not_a_real_mode" })),
    );
    assert!(Command::try_from(request).is_err());
}

#[test]
fn record_command_valid_stop_mode_parses() {
    // A well-formed override still resolves to the parsed value.
    let request = make_request(
        "record",
        Some(json!({ "write_mode": false, "stop_mode": "manual_only" })),
    );
    match Command::try_from(request).expect("record command should parse") {
        Command::Record { stop_mode, .. } => {
            assert_eq!(stop_mode, Some(RecordingStopMode::ManualOnly));
        }
        _ => panic!("expected Command::Record"),
    }
}

#[test]
fn set_model_parses_online_models() {
    let cases: &[&str] = &[
        "whisper-1",
        "gpt-4o-transcribe",
        "gpt-4o-mini-transcribe",
        "voxtral-mini-latest",
        "nova-3",
    ];
    for model_name in cases {
        let request = make_request("set_model", Some(json!({ "model": model_name})));
        let command = Command::try_from(request)
            .unwrap_or_else(|e| panic!("set_model should parse {model_name}: {e}"));
        match command {
            Command::SetModel { model, source } => {
                assert_eq!(model.clone(), *model_name);
                // No source supplied → empty; the daemon resolves it against
                // the active backend (see endpoints/v1/active_model.md).
                assert_eq!(source, "");
            }
            _ => panic!("expected Command::SetModel for {model_name}"),
        }
    }
}

#[test]
fn set_model_parses_local_name() {
    let request = make_request(
        "set_model",
        Some(json!({ "model": "whisper-tiny", "provider": "local_whisper" })),
    );
    let command = Command::try_from(request).expect("should parse");
    match command {
        Command::SetModel { model, source } => {
            assert_eq!(model, "whisper-tiny");
            assert_eq!(source, "");
        }
        _ => panic!("expected Command::SetModel"),
    }
}

#[test]
fn set_model_passes_source_repo_through() {
    let request = make_request(
        "set_model",
        Some(json!({
            "model": "voxtral-mini",
            "provider": "local_voxtral",
            "source": "github.com/super-stt/voxtral",
        })),
    );
    let command = Command::try_from(request).expect("should parse");
    match command {
        Command::SetModel { source, .. } => {
            assert_eq!(source, "github.com/super-stt/voxtral");
        }
        _ => panic!("expected Command::SetModel"),
    }
}

#[test]
fn set_recording_stop_mode_parses() {
    let request = make_request(
        "set_recording_stop_mode",
        Some(json!({ "mode": "silence_only" })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetRecordingStopMode { mode } => {
            assert_eq!(mode, RecordingStopMode::SilenceOnly);
        }
        _ => panic!("expected Command::SetRecordingStopMode"),
    }
}

#[test]
fn set_custom_models_dir_parses_with_path() {
    let request = make_request(
        "set_custom_models_dir",
        Some(json!({ "path": "/tmp/models" })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetCustomModelsDir { path } => {
            assert_eq!(path.as_deref(), Some("/tmp/models"));
        }
        _ => panic!("expected Command::SetCustomModelsDir"),
    }
}

#[test]
fn set_custom_models_dir_parses_with_null() {
    let request = make_request("set_custom_models_dir", Some(json!({ "path": null })));
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetCustomModelsDir { path } => {
            assert!(path.is_none());
        }
        _ => panic!("expected Command::SetCustomModelsDir"),
    }
}

#[test]
fn set_custom_models_dir_parses_without_data() {
    let request = make_request("set_custom_models_dir", None);
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetCustomModelsDir { path } => {
            assert!(path.is_none());
        }
        _ => panic!("expected Command::SetCustomModelsDir"),
    }
}

#[test]
fn list_backends_parses() {
    let request = make_request("list_backends", None);
    let command = Command::try_from(request).expect("command should parse");
    assert!(matches!(command, Command::ListBackends));
}

#[test]
fn unload_active_model_parses() {
    let request = make_request("unload_active_model", None);
    let command = Command::try_from(request).expect("command should parse");
    assert!(matches!(command, Command::UnloadActiveModel));
}

#[test]
fn set_backend_option_parses() {
    let request = make_request(
        "set_backend_option",
        Some(json!({
            "source": "github.com/super-stt/openai",
            "name": "base_url",
            "value": "https://gw.example",
        })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetBackendOption {
            source,
            name,
            value,
        } => {
            assert_eq!(source, "github.com/super-stt/openai");
            assert_eq!(name, "base_url");
            assert_eq!(value, "https://gw.example");
        }
        _ => panic!("expected Command::SetBackendOption"),
    }
}

#[test]
fn set_backend_option_absent_value_clears() {
    let request = make_request(
        "set_backend_option",
        Some(json!({ "source": "s", "name": "base_url" })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetBackendOption { value, .. } => assert_eq!(value, ""),
        _ => panic!("expected Command::SetBackendOption"),
    }
}

#[test]
fn set_backend_option_missing_source_fails() {
    let request = make_request("set_backend_option", Some(json!({ "name": "base_url" })));
    assert!(Command::try_from(request).is_err());
}

/// The first command whose payload is a struct, so this is the one that
/// checks the whole object survives the trip rather than a field or two.
#[test]
fn set_context_parses_the_whole_object() {
    let request = make_request(
        "set_context",
        Some(json!({
            "context": {
                "id": "coding",
                "name": "Coding",
                "prompt": "I dictate code.",
                "vocabulary": ["main branch", "Menjivar, Jorge"],
            }
        })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetContext { context } => {
            assert_eq!(context.id, "coding");
            assert_eq!(context.name, "Coding");
            assert_eq!(context.prompt, "I dictate code.");
            assert_eq!(
                context.vocabulary,
                vec!["main branch".to_string(), "Menjivar, Jorge".to_string()],
                "a term holding a comma is one term — that is why this is an array"
            );
        }
        _ => panic!("expected Command::SetContext"),
    }
}

/// Every field but the id defaults, so a client that knows less than the
/// current shape still parses.
#[test]
fn set_context_fills_in_what_a_client_omits() {
    let request = make_request(
        "set_context",
        Some(json!({ "context": { "id": "coding" } })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetContext { context } => {
            assert!(context.name.is_empty());
            assert!(context.vocabulary.is_empty());
        }
        _ => panic!("expected Command::SetContext"),
    }
}

/// A vocabulary of the wrong type is a parse failure, not a silently dropped
/// field — which is what picking the strings out of the `Value` by hand would
/// have given.
#[test]
fn set_context_refuses_a_malformed_object() {
    let bad_shape = make_request(
        "set_context",
        Some(json!({ "context": { "id": "coding", "vocabulary": [1, 2] } })),
    );
    assert!(Command::try_from(bad_shape).is_err());

    let missing = make_request("set_context", Some(json!({})));
    assert!(Command::try_from(missing).is_err());
}

#[test]
fn delete_context_needs_an_id() {
    let request = make_request("delete_context", Some(json!({ "id": "coding" })));
    match Command::try_from(request).expect("command should parse") {
        Command::DeleteContext { id } => assert_eq!(id, "coding"),
        _ => panic!("expected Command::DeleteContext"),
    }
    assert!(Command::try_from(make_request("delete_context", None)).is_err());
}

#[test]
fn set_active_context_takes_an_id_or_clears() {
    let chosen = make_request("set_active_context", Some(json!({ "id": "coding" })));
    match Command::try_from(chosen).expect("command should parse") {
        Command::SetActiveContext { id } => assert_eq!(id.as_deref(), Some("coding")),
        _ => panic!("expected Command::SetActiveContext"),
    }

    for cleared in [
        make_request("set_active_context", Some(json!({ "id": null }))),
        make_request("set_active_context", None),
    ] {
        match Command::try_from(cleared).expect("command should parse") {
            Command::SetActiveContext { id } => assert_eq!(id, None),
            _ => panic!("expected Command::SetActiveContext"),
        }
    }
}

/// The per-backend override has three states, and this is the command that has
/// to keep the two that look alike apart: null clears the override, `""` pins
/// the backend to no context at all.
#[test]
fn set_backend_context_keeps_absent_and_empty_apart() {
    let cases = [
        (json!({ "source": "s", "id": "coding" }), Some("coding")),
        (json!({ "source": "s", "id": "" }), Some("")),
        (json!({ "source": "s", "id": null }), None),
        (json!({ "source": "s" }), None),
    ];
    for (data, want) in cases {
        let request = make_request("set_backend_context", Some(data.clone()));
        match Command::try_from(request).expect("command should parse") {
            Command::SetBackendContext { source, id } => {
                assert_eq!(source, "s");
                assert_eq!(id.as_deref(), want, "for {data}");
            }
            _ => panic!("expected Command::SetBackendContext"),
        }
    }

    let no_source = make_request("set_backend_context", Some(json!({ "id": "coding" })));
    assert!(Command::try_from(no_source).is_err());
    let wrong_type = make_request(
        "set_backend_context",
        Some(json!({ "source": "s", "id": 7 })),
    );
    assert!(Command::try_from(wrong_type).is_err());
}

#[test]
fn response_with_backends_serializes() {
    let response =
        DaemonResponse::success().with_backends(json!([{ "source": "x", "models": [] }]));
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["backends"][0]["source"], "x");
}

#[test]
fn set_active_backend_parses() {
    let request = make_request(
        "set_active_backend",
        Some(json!({ "source": "github.com/super-stt/openai" })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetActiveBackend { source } => {
            assert_eq!(source, "github.com/super-stt/openai");
        }
        _ => panic!("expected Command::SetActiveBackend"),
    }
}

#[test]
fn set_active_backend_missing_source_fails() {
    let request = make_request("set_active_backend", Some(json!({})));
    assert!(
        Command::try_from(request).is_err(),
        "set_active_backend without source must be rejected"
    );
}

#[test]
fn set_active_backend_without_data_fails() {
    let request = make_request("set_active_backend", None);
    assert!(Command::try_from(request).is_err());
}

#[test]
fn get_active_backend_parses() {
    let request = make_request("get_active_backend", None);
    let command = Command::try_from(request).expect("command should parse");
    assert!(matches!(command, Command::GetActiveBackend));
}

#[test]
fn clear_active_backend_parses() {
    let request = make_request("clear_active_backend", None);
    let command = Command::try_from(request).expect("command should parse");
    assert!(matches!(command, Command::ClearActiveBackend));
}

#[test]
fn response_with_active_backend_payload_serializes() {
    let payload = json!({
        "source": "github.com/super-stt/openai",
        "name": "OpenAI",
        "model_loaded": false,
    });
    let response = DaemonResponse::success().with_active_backend(payload);
    assert!(response.active_backend.is_some());

    let serialized = serde_json::to_value(&response).unwrap();
    assert_eq!(
        serialized["active_backend"]["source"],
        "github.com/super-stt/openai"
    );
    assert_eq!(serialized["active_backend"]["model_loaded"], false);
}

/// `clear_active_backend` returns `active_backend: null` on the wire, which
/// the response carries as `Some(Value::Null)` (distinct from "field
/// absent"). The serde skip-if-None means `null` *is* serialized — only an
/// unset field is omitted.
#[test]
fn response_active_backend_null_round_trips() {
    let response = DaemonResponse::success().with_active_backend(Value::Null);
    let serialized = serde_json::to_value(&response).unwrap();
    assert_eq!(serialized["active_backend"], Value::Null);
}

#[test]
fn response_active_backend_absent_is_skipped() {
    let response = DaemonResponse::success();
    assert!(response.active_backend.is_none());

    let serialized = serde_json::to_value(&response).unwrap();
    assert!(
        serialized.get("active_backend").is_none(),
        "skip_serializing_if=Option::is_none should omit the field"
    );
}

#[test]
fn transcribe_command_carries_audio_sample_rate_and_language() {
    let mut request = make_request("transcribe", None);
    request.audio_data = Some(vec![0.1, -0.1, 0.2]);
    request.sample_rate = Some(16000);
    request.language = Some("en".to_string());
    match Command::try_from(request).expect("transcribe command should parse") {
        Command::Transcribe {
            audio_data,
            sample_rate,
            language,
            ..
        } => {
            assert_eq!(audio_data.len(), 3);
            assert_eq!(sample_rate, 16000);
            assert_eq!(language.as_deref(), Some("en"));
        }
        _ => panic!("expected Command::Transcribe"),
    }
}

#[test]
fn record_command_carries_language_override() {
    let mut request = make_request("record", Some(json!({ "write_mode": false })));
    request.language = Some("fr".to_string());
    match Command::try_from(request).expect("record command should parse") {
        Command::Record { language, .. } => assert_eq!(language.as_deref(), Some("fr")),
        _ => panic!("expected Command::Record"),
    }
}

/// `resolved_accel` is doubly `Option` so a response that never mentions
/// devices at all omits the key, while `/active_device` can still emit a
/// literal `null` for "gpu requested, nothing has resolved yet" rather than
/// dropping the key like every other irrelevant field on this kitchen-sink
/// response type.
#[test]
fn resolved_accel_is_omitted_unless_set_then_may_serialize_as_null() {
    let untouched = serde_json::to_value(DaemonResponse::success()).expect("serialize");
    assert!(
        untouched.get("resolved_accel").is_none(),
        "a response that never calls with_resolved_accel must not mention the key: {untouched}"
    );

    let unresolved =
        serde_json::to_value(DaemonResponse::success().with_resolved_accel(None)).expect("ser");
    assert_eq!(unresolved["resolved_accel"], Value::Null);

    let resolved = serde_json::to_value(
        DaemonResponse::success().with_resolved_accel(Some("cuda".to_string())),
    )
    .expect("ser");
    assert_eq!(resolved["resolved_accel"], "cuda");
}

/// The `host.{cuda,rocm,vulkan}` field names are published in
/// `docs/protocol/endpoints/v1/gpu_info.md` and other work builds against
/// them; pin the exact wire shape here rather than trusting the derive.
#[test]
fn gpu_host_info_serializes_the_published_field_names() {
    let host = GpuHostInfo {
        cuda: Some(CudaHostInfo {
            driver_version: "13.3".to_string(),
        }),
        rocm: None,
        vulkan: Some(VulkanHostInfo {
            api_version: "1.4.354".to_string(),
        }),
    };
    let value = serde_json::to_value(
        DaemonResponse::success()
            .with_gpu_info(vec![GpuInfo {
                name: "NVIDIA GeForce RTX 3090".to_string(),
                vendor: "nvidia".to_string(),
                total_bytes: 25_757_220_864,
                free_bytes: None,
                used_bytes: None,
                arch_target: Some("sm_86".to_string()),
            }])
            .with_gpu_host_info(host),
    )
    .expect("serialize");
    assert_eq!(value["host"]["cuda"]["driver_version"], "13.3");
    assert_eq!(value["host"]["rocm"], Value::Null);
    assert_eq!(value["host"]["vulkan"]["api_version"], "1.4.354");
    assert_eq!(value["gpu_info"][0]["arch_target"], "sm_86");
}

/// `set_post_processor` names the model to run. There is no `enabled` field —
/// the command *is* "enable with this model".
#[test]
fn set_post_processor_parses_the_model_and_source() {
    let req = make_request(
        "set_post_processor",
        Some(serde_json::json!({
            "model": "cleanup-small",
            "source": "github.com/super-stt/cleanup",
        })),
    );
    match Command::try_from(req).expect("parses") {
        Command::SetPostProcessor { model, source } => {
            assert_eq!(model, "cleanup-small");
            assert_eq!(source, "github.com/super-stt/cleanup");
        }
        other => panic!("wrong command: {other:?}"),
    }
}

/// An omitted `source` resolves to the selected post-processor backend at
/// handling time, the way `set_model` resolves against the active backend, so
/// the parser accepts a bare model name.
#[test]
fn set_post_processor_takes_a_bare_model_name() {
    let req = make_request(
        "set_post_processor",
        Some(serde_json::json!({ "model": "cleanup-small" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::SetPostProcessor { model, source } => {
            assert_eq!(model, "cleanup-small");
            assert_eq!(source, "");
        }
        other => panic!("wrong command: {other:?}"),
    }
}

/// Running nothing is `clear_post_processor`, not a `set` with an empty model,
/// so a missing name is refused rather than silently meaning "off".
#[test]
fn set_post_processor_requires_a_model() {
    for data in [
        serde_json::json!({}),
        serde_json::json!({ "source": "github.com/x/y" }),
        serde_json::json!({ "model": "" }),
    ] {
        let req = make_request("set_post_processor", Some(data));
        assert!(
            Command::try_from(req).is_err(),
            "a set without a model must be refused"
        );
    }
}

/// A device belongs to a model, so the per-model device commands name one and
/// carry no "current model" fallback: they must work for a model that is not
/// loaded.
#[test]
fn model_device_commands_parse_the_model_and_device() {
    let req = make_request(
        "set_model_device",
        Some(json!({ "model": "whisper-large-v3", "device": "gpu" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::SetModelDevice { model, device } => {
            assert_eq!(model, "whisper-large-v3");
            assert_eq!(device, "gpu");
        }
        other => panic!("wrong command: {other:?}"),
    }

    let req = make_request(
        "get_model_device",
        Some(json!({ "model": "whisper-large-v3" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::GetModelDevice { model } => assert_eq!(model, "whisper-large-v3"),
        other => panic!("wrong command: {other:?}"),
    }
}

/// The stage-2 twins carry the same shape.
#[test]
fn post_processor_device_commands_parse_the_model_and_device() {
    let req = make_request(
        "set_post_processor_device",
        Some(json!({ "model": "s1-mini-q4_k_m", "device": "cpu" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::SetPostProcessorDevice { model, device } => {
            assert_eq!(model, "s1-mini-q4_k_m");
            assert_eq!(device, "cpu");
        }
        other => panic!("wrong command: {other:?}"),
    }

    let req = make_request(
        "get_post_processor_device",
        Some(json!({ "model": "s1-mini-q4_k_m" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::GetPostProcessorDevice { model } => assert_eq!(model, "s1-mini-q4_k_m"),
        other => panic!("wrong command: {other:?}"),
    }
}

/// The list verbs: per model they name one, per backend they name nothing —
/// the stage's selection is the backend.
#[test]
fn device_list_commands_parse() {
    let req = make_request("list_model_devices", Some(json!({ "model": "whisper" })));
    match Command::try_from(req).expect("parses") {
        Command::ListModelDevices { model } => assert_eq!(model, "whisper"),
        other => panic!("wrong command: {other:?}"),
    }
    let req = make_request(
        "list_post_processor_devices",
        Some(json!({ "model": "s1-mini-q4_k_m" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::ListPostProcessorDevices { model } => assert_eq!(model, "s1-mini-q4_k_m"),
        other => panic!("wrong command: {other:?}"),
    }
    assert!(matches!(
        Command::try_from(make_request("list_active_backend_devices", None)),
        Ok(Command::ListActiveBackendDevices)
    ));
    assert!(matches!(
        Command::try_from(make_request("list_post_processor_backend_devices", None)),
        Ok(Command::ListPostProcessorBackendDevices)
    ));
}

/// Stage 2 owns the same in-flight verbs stage 1 does. Each cancels and
/// reloads only its own model: the stages provision independently, so a cancel
/// addressed to one must not abandon the other's load.
#[test]
fn the_post_processor_stage_has_its_own_cancel_and_reload() {
    assert!(matches!(
        Command::try_from(make_request("cancel_post_processor_download", None)),
        Ok(Command::CancelPostProcessorDownload)
    ));
    assert!(matches!(
        Command::try_from(make_request("reload_post_processor", None)),
        Ok(Command::ReloadPostProcessor)
    ));
    // And stage 1's keep their own names.
    assert!(matches!(
        Command::try_from(make_request("cancel_download", None)),
        Ok(Command::CancelDownload)
    ));
    assert!(matches!(
        Command::try_from(make_request("reload_active_model", None)),
        Ok(Command::ReloadActiveModel)
    ));
}

/// Neither half may be omitted: a setter without a device has nothing to set,
/// and either verb without a model has nothing to address.
#[test]
fn model_device_commands_require_both_halves() {
    for (command, data) in [
        ("set_model_device", json!({ "device": "gpu" })),
        ("set_model_device", json!({ "model": "", "device": "gpu" })),
        ("set_model_device", json!({ "model": "whisper" })),
        ("get_model_device", json!({})),
        ("list_model_devices", json!({})),
        ("set_post_processor_device", json!({ "device": "gpu" })),
        ("get_post_processor_device", json!({ "model": "" })),
        ("list_post_processor_devices", json!({ "model": "" })),
    ] {
        assert!(
            Command::try_from(make_request(command, Some(data.clone()))).is_err(),
            "{command} with {data} must be refused"
        );
    }
}

/// The backend half of the split: `set_post_processor_backend` takes the source
/// alone, exactly as `set_active_backend` does for transcription.
#[test]
fn set_post_processor_backend_parses_its_source() {
    let req = make_request(
        "set_post_processor_backend",
        Some(serde_json::json!({ "source": "github.com/super-stt/cleanup" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::SetPostProcessorBackend { source } => {
            assert_eq!(source, "github.com/super-stt/cleanup");
        }
        other => panic!("wrong command: {other:?}"),
    }
}

#[test]
fn set_post_processor_backend_requires_a_source() {
    assert!(Command::try_from(make_request("set_post_processor_backend", None)).is_err());
    assert!(
        Command::try_from(make_request(
            "set_post_processor_backend",
            Some(serde_json::json!({}))
        ))
        .is_err()
    );
}

#[test]
fn post_processor_getters_parse() {
    assert!(matches!(
        Command::try_from(make_request("get_post_processor", None)).expect("parses"),
        Command::GetPostProcessor
    ));
    assert!(matches!(
        Command::try_from(make_request("clear_post_processor", None)).expect("parses"),
        Command::ClearPostProcessor
    ));
    assert!(matches!(
        Command::try_from(make_request("clear_post_processor_backend", None)).expect("parses"),
        Command::ClearPostProcessorBackend
    ));
}
