// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use serde_json::{Value, json};

fn make_request(command: &str, data: Option<Value>) -> DaemonRequest {
    DaemonRequest {
        command: command.to_string(),
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
    let cases: &[&str] = &[
        "kokoro-1",
        "gpt-4o-mini-tts",
        "tts-1-hd",
        "piper-mini-latest",
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
        Some(json!({ "model": "kokoro-tiny", "provider": "local_kokoro" })),
    );
    let command = Command::try_from(request).expect("should parse");
    match command {
        Command::SetModel { model, source } => {
            assert_eq!(model, "kokoro-tiny");
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
            "model": "piper-mini",
            "provider": "local_piper",
            "source": "github.com/super-tts/piper",
        })),
    );
    let command = Command::try_from(request).expect("should parse");
    match command {
        Command::SetModel { source, .. } => {
            assert_eq!(source, "github.com/super-tts/piper");
        }
        _ => panic!("expected Command::SetModel"),
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
            "source": "github.com/super-tts/openai",
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
            assert_eq!(source, "github.com/super-tts/openai");
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
        Some(json!({ "source": "github.com/super-tts/openai" })),
    );
    let command = Command::try_from(request).expect("command should parse");
    match command {
        Command::SetActiveBackend { source } => {
            assert_eq!(source, "github.com/super-tts/openai");
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
        "source": "github.com/super-tts/openai",
        "name": "OpenAI",
        "model_loaded": false,
    });
    let response = DaemonResponse::success().with_active_backend(payload);
    assert!(response.active_backend.is_some());

    let serialized = serde_json::to_value(&response).unwrap();
    assert_eq!(
        serialized["active_backend"]["source"],
        "github.com/super-tts/openai"
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

/// The per-model device verbs carry the model they address in `data`, the
/// same place every other per-model command carries it — and the setter
/// carries the raw device string, unvalidated here so the daemon can answer
/// `invalid_device` for a value it recognizes as a device but refuses as a
/// preference.
#[test]
fn model_device_commands_parse_the_model_and_device() {
    let req = make_request(
        "set_model_device",
        Some(json!({ "model": "kokoro-82m", "device": "gpu" })),
    );
    match Command::try_from(req).expect("parses") {
        Command::SetModelDevice { model, device } => {
            assert_eq!(model, "kokoro-82m");
            assert_eq!(device, "gpu");
        }
        other => panic!("wrong command: {other:?}"),
    }

    let req = make_request("get_model_device", Some(json!({ "model": "kokoro-82m" })));
    match Command::try_from(req).expect("parses") {
        Command::GetModelDevice { model } => assert_eq!(model, "kokoro-82m"),
        other => panic!("wrong command: {other:?}"),
    }
}

/// The list verbs: per model it names one, per backend it names nothing —
/// the daemon's own selection is the backend.
#[test]
fn device_list_commands_parse() {
    let req = make_request("list_model_devices", Some(json!({ "model": "kokoro-82m" })));
    match Command::try_from(req).expect("parses") {
        Command::ListModelDevices { model } => assert_eq!(model, "kokoro-82m"),
        other => panic!("wrong command: {other:?}"),
    }
    assert!(matches!(
        Command::try_from(make_request("list_active_backend_devices", None)),
        Ok(Command::ListActiveBackendDevices)
    ));
}

/// Neither half may be omitted: a setter without a device has nothing to set,
/// and either verb without a model has nothing to address. An empty string is
/// as absent as a missing key — it would otherwise reach the daemon and fail
/// as "no such model", blaming the backend for a malformed request.
#[test]
fn model_device_commands_require_both_halves() {
    for (command, data) in [
        ("set_model_device", json!({ "device": "gpu" })),
        ("set_model_device", json!({ "model": "", "device": "gpu" })),
        ("set_model_device", json!({ "model": "kokoro-82m" })),
        ("get_model_device", json!({})),
        ("get_model_device", json!({ "model": "" })),
        ("list_model_devices", json!({})),
        ("list_model_devices", json!({ "model": "" })),
    ] {
        assert!(
            Command::try_from(make_request(command, Some(data.clone()))).is_err(),
            "{command} with {data} must be refused"
        );
    }
}

/// `get_pipeline` takes nothing: the pipeline is the daemon's own, and a
/// `stage` parameter would only let a client ask for a stage that does not
/// exist. The whole list comes back and the client picks the row it wants.
#[test]
fn get_pipeline_parses_without_data() {
    assert!(matches!(
        Command::try_from(make_request("get_pipeline", None)),
        Ok(Command::GetPipeline)
    ));
    // A body is ignored rather than refused: a client that sends `{}` because
    // its HTTP helper always sends one must not be told the command is
    // malformed.
    assert!(matches!(
        Command::try_from(make_request("get_pipeline", Some(json!({})))),
        Ok(Command::GetPipeline)
    ));
}

/// A one-element pipeline is still a list on the wire, and the stage carries
/// its own number. A client reads the array and matches on `stage`, so the
/// shape has to survive serialization as an array even with a single row —
/// collapsing it to an object here is a change no second stage could undo
/// without breaking every reader.
#[test]
fn a_response_carries_the_pipeline_as_a_list_of_stages() {
    let response = DaemonResponse {
        pipeline: Some(vec![StageReport {
            stage: SYNTHESIS_STAGE,
            role: StageRole::Synthesis,
            source: Some("github.com/super-tts/kokoro".to_string()),
            name: Some("Kokoro".to_string()),
            enabled: true,
        }]),
        ..DaemonResponse::success()
    };
    let json = serde_json::to_value(&response).expect("serialize");
    assert!(json["pipeline"].is_array(), "pipeline must stay a list");
    assert_eq!(json["pipeline"][0]["stage"], SYNTHESIS_STAGE);
    assert_eq!(json["pipeline"][0]["role"], "synthesis");
    assert_eq!(json["pipeline"][0]["enabled"], true);

    // And it stays off the wire on every response that is not about the
    // pipeline — this type is shared by every command.
    let untouched = serde_json::to_value(DaemonResponse::success()).expect("serialize");
    assert!(untouched.get("pipeline").is_none());
}

/// `stage_model` rides on the same response type and must be omitted unless
/// the command actually answered for a model slot, or a client polling any
/// other endpoint would read a slot that was never filled in.
#[test]
fn a_response_carries_the_stage_model_slot_only_when_set() {
    let untouched = serde_json::to_value(DaemonResponse::success()).expect("serialize");
    assert!(untouched.get("stage_model").is_none());

    let response = DaemonResponse {
        stage_model: Some(StageModelReport {
            stage: SYNTHESIS_STAGE,
            model: Some("kokoro-82m".to_string()),
            loaded: false,
            device: Some(StageModelDevice {
                preference: "gpu".to_string(),
                resolved_accel: None,
            }),
            switch: None,
        }),
        ..DaemonResponse::success()
    };
    let json = serde_json::to_value(&response).expect("serialize");
    assert_eq!(json["stage_model"]["stage"], SYNTHESIS_STAGE);
    assert_eq!(json["stage_model"]["model"], "kokoro-82m");
    // Selected but not loaded is a real state, not a contradiction.
    assert_eq!(json["stage_model"]["loaded"], false);
    assert!(json["stage_model"]["switch"].is_null());
}

/// The two language-listing verbs: the global one addresses nothing, the
/// per-model one carries the same `(source, model)` pair its setter does.
#[test]
fn language_list_commands_parse() {
    assert!(matches!(
        Command::try_from(make_request("list_primary_languages", None)),
        Ok(Command::ListPrimaryLanguages)
    ));

    let request = make_request(
        "list_model_languages",
        Some(json!({ "source": "github.com/super-tts/kokoro", "model": "kokoro-82m" })),
    );
    match Command::try_from(request).expect("parses") {
        Command::ListModelLanguages { source, model } => {
            assert_eq!(source, "github.com/super-tts/kokoro");
            assert_eq!(model, "kokoro-82m");
        }
        other => panic!("wrong command: {other:?}"),
    }
}

/// A model's language list addresses one model, so neither half of its
/// identity may be omitted. Falling back to "the active one" would answer for
/// a model the client never named, and its picker would then be filled with
/// another model's languages — a mismatch nothing downstream could detect,
/// because every tag in it is a legitimate tag.
#[test]
fn list_model_languages_requires_both_halves() {
    for data in [
        json!({}),
        json!({ "model": "kokoro-82m" }),
        json!({ "source": "github.com/super-tts/kokoro" }),
    ] {
        assert!(
            Command::try_from(make_request("list_model_languages", Some(data.clone()))).is_err(),
            "list_model_languages with {data} must be refused"
        );
    }
}

/// `available_languages` answers two different endpoints — the global setting's
/// list and one model's — so it must be omitted from every response that
/// answers neither, and must survive as an empty list when a monolingual model
/// genuinely has nothing to offer. An empty list and an absent key mean
/// different things to a picker: "hide the control" versus "this reply is not
/// about languages".
#[test]
fn available_languages_distinguishes_empty_from_absent() {
    let untouched = serde_json::to_value(DaemonResponse::success()).expect("serialize");
    assert!(untouched.get("available_languages").is_none());

    let empty = serde_json::to_value(DaemonResponse {
        available_languages: Some(Vec::new()),
        ..DaemonResponse::success()
    })
    .expect("serialize");
    assert_eq!(empty["available_languages"], json!([]));

    let filled = serde_json::to_value(DaemonResponse {
        available_languages: Some(vec!["auto".to_string(), "en-US".to_string()]),
        ..DaemonResponse::success()
    })
    .expect("serialize");
    assert_eq!(filled["available_languages"][0], "auto");
}

/// The global default keeps its own verbs. Per-model devices are an addition,
/// not a replacement: a client with no model in hand still has something to
/// set, and every client shipped before the per-model verbs keeps working.
#[test]
fn the_global_device_commands_still_parse() {
    let req = make_request("set_device", Some(json!({ "device": "cpu" })));
    match Command::try_from(req).expect("parses") {
        Command::SetDevice { device } => assert_eq!(device, "cpu"),
        other => panic!("wrong command: {other:?}"),
    }
    assert!(matches!(
        Command::try_from(make_request("get_device", None)),
        Ok(Command::GetDevice)
    ));
}
