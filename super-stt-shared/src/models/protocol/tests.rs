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
fn set_allow_online_models_parses() {
    let mut request = make_request("set_allow_online_models", None);
    request.enabled = Some(true);
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetAllowOnlineModels { enabled } => assert!(enabled),
        _ => panic!("expected Command::SetAllowOnlineModels"),
    }
}

#[test]
fn set_allow_online_models_missing_enabled_fails() {
    let request = make_request("set_allow_online_models", None);
    let result = Command::try_from(request);
    assert!(result.is_err());
}

#[test]
fn get_allow_online_models_parses() {
    let request = make_request("get_allow_online_models", None);
    let command = Command::try_from(request).expect("command should parse");
    assert!(matches!(command, Command::GetAllowOnlineModels));
}

#[test]
fn response_with_allow_online_models() {
    let response = DaemonResponse::success().with_allow_online_models(true);
    assert_eq!(response.allow_online_models, Some(true));

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["allow_online_models"], true);
}

#[test]
fn set_allow_online_models_false() {
    let mut request = make_request("set_allow_online_models", None);
    request.enabled = Some(false);
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetAllowOnlineModels { enabled } => assert!(!enabled),
        _ => panic!("expected Command::SetAllowOnlineModels"),
    }
}

#[test]
fn response_allow_online_models_false_serializes() {
    let response = DaemonResponse::success().with_allow_online_models(false);
    assert_eq!(response.allow_online_models, Some(false));

    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["allow_online_models"], false);
}

#[test]
fn response_allow_online_models_skipped_when_none() {
    let response = DaemonResponse::success();
    assert_eq!(response.allow_online_models, None);

    let json = serde_json::to_value(&response).unwrap();
    assert!(json.get("allow_online_models").is_none());
}

#[test]
fn set_model_parses_online_models() {
    let cases: &[(&str, &str)] = &[
        ("whisper-1", "openai"),
        ("gpt-4o-transcribe", "openai"),
        ("gpt-4o-mini-transcribe", "openai"),
        ("voxtral-mini-latest", "mistral"),
        ("nova-3", "deepgram"),
    ];
    for (model_name, provider_str) in cases {
        let request = make_request(
            "set_model",
            Some(json!({ "model": model_name, "provider": provider_str })),
        );
        let command = Command::try_from(request)
            .unwrap_or_else(|e| panic!("set_model should parse {model_name}: {e}"));
        match command {
            Command::SetModel {
                model,
                provider,
                source,
            } => {
                assert_eq!(model.to_string(), *model_name);
                assert_eq!(provider.as_str(), *provider_str, "{model_name}");
                // No source supplied → empty (daemon picks the backend).
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
        Command::SetModel {
            model,
            provider,
            source,
        } => {
            assert_eq!(model, "whisper-tiny");
            assert_eq!(provider.as_str(), "local_whisper");
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
fn set_model_rejects_missing_provider() {
    let request = make_request("set_model", Some(json!({ "model": "whisper-tiny" })));
    let result = Command::try_from(request);
    assert!(
        result.is_err(),
        "set_model without provider should be rejected"
    );
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
