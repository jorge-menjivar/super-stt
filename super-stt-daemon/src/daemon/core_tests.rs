// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use crate::daemon::types::test_daemon;
use super_stt_shared::models::protocol::ErrorCode;
use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
use tokio::time::{Duration, timeout};

fn make_request(command: &str) -> DaemonRequest {
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
        data: None,
        language: None,
        enabled: None,
    }
}

fn make_record_request(data: Option<serde_json::Value>) -> DaemonRequest {
    DaemonRequest {
        command: "record".to_string(),
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

#[tokio::test]
async fn stop_signal_sent_on_second_press_with_default_mode() {
    // Default config mode is SilenceAndManual, which allows manual stop
    let daemon = test_daemon().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(1);

    *daemon.busy.write().await = true;
    *daemon.manual_stop_tx.write().await = Some(tx);

    let request = make_record_request(Some(serde_json::json!({
        "write_mode": false,
    })));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.message.as_deref(),
        Some(DaemonResponse::RECORDING_STOP_SIGNAL_MSG)
    );

    let recv = timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(recv.is_ok(), "expected stop signal to be sent");
}

#[tokio::test]
async fn guard_model_mutation_flags_recording_in_progress() {
    use super_stt_shared::models::protocol::ErrorCode;
    let daemon = test_daemon().await;
    // Idle: the mutation is allowed.
    assert!(daemon.guard_model_mutation("switch models").await.is_none());
    // Recording: the unified guard rejects with the machine-readable
    // RecordingInProgress code, independent of the human `action` wording.
    *daemon.busy.write().await = true;
    let resp = daemon
        .guard_model_mutation("switch models")
        .await
        .expect("mutation must be rejected while recording");
    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_code, Some(ErrorCode::RecordingInProgress));
}

#[tokio::test]
async fn second_press_ignored_in_silence_only_mode() {
    let daemon = test_daemon().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(1);

    // Set daemon config to SilenceOnly
    {
        use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
        let mut config = daemon.config.write().await;
        config.transcription.recording_stop_mode = RecordingStopMode::SilenceOnly;
    }

    *daemon.busy.write().await = true;
    *daemon.manual_stop_tx.write().await = Some(tx);

    // No stop_mode in request → uses daemon config (SilenceOnly)
    let request = make_record_request(Some(serde_json::json!({
        "write_mode": false,
    })));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.message.as_deref(),
        Some("Manual stop not enabled in current mode")
    );

    // Stop signal should NOT have been sent
    let recv = timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(
        recv.is_err(),
        "stop signal should not be sent in SilenceOnly mode"
    );
}

#[tokio::test]
async fn per_request_stop_mode_overrides_config() {
    let daemon = test_daemon().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(1);

    // Daemon config is SilenceOnly (no manual stop)
    {
        use super_stt_shared::models::recording_stop_mode::RecordingStopMode;
        let mut config = daemon.config.write().await;
        config.transcription.recording_stop_mode = RecordingStopMode::SilenceOnly;
    }

    *daemon.busy.write().await = true;
    *daemon.manual_stop_tx.write().await = Some(tx);

    // But the request explicitly asks for manual_only mode
    let request = make_record_request(Some(serde_json::json!({
        "write_mode": false,
        "stop_mode": "manual_only",
    })));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.message.as_deref(),
        Some(DaemonResponse::RECORDING_STOP_SIGNAL_MSG)
    );

    let recv = timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(
        recv.is_ok(),
        "per-request override should allow manual stop"
    );
}

#[tokio::test]
async fn second_press_during_transcription_returns_wait_message() {
    let daemon = test_daemon().await;

    // Transcribing state: busy=true, manual_stop_tx=None
    *daemon.busy.write().await = true;
    // manual_stop_tx is already None by default

    let request = make_record_request(Some(serde_json::json!({
        "write_mode": false,
    })));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.message.as_deref(),
        Some("Transcription in progress, please wait")
    );
}

#[tokio::test]
async fn per_request_silence_only_overrides_manual_config() {
    let daemon = test_daemon().await;
    let (tx, mut rx) = tokio::sync::broadcast::channel(1);

    // Config allows manual stop (default SilenceAndManual)
    *daemon.busy.write().await = true;
    *daemon.manual_stop_tx.write().await = Some(tx);

    // But request forces SilenceOnly
    let request = make_record_request(Some(serde_json::json!({
        "write_mode": false,
        "stop_mode": "silence_only",
    })));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.message.as_deref(),
        Some("Manual stop not enabled in current mode")
    );

    let recv = timeout(Duration::from_millis(100), rx.recv()).await;
    assert!(
        recv.is_err(),
        "stop signal should not be sent in SilenceOnly mode"
    );
}

#[tokio::test]
async fn stop_signal_succeeds_even_with_no_receivers() {
    let daemon = test_daemon().await;
    let (tx, _rx) = tokio::sync::broadcast::channel::<()>(1);
    // Drop _rx so there are no receivers

    *daemon.busy.write().await = true;
    *daemon.manual_stop_tx.write().await = Some(tx);

    let request = make_record_request(Some(serde_json::json!({
        "write_mode": false,
    })));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.message.as_deref(),
        Some(DaemonResponse::RECORDING_STOP_SIGNAL_MSG)
    );
}

/// Verify that `handle_status` reports `busy` correctly.
/// The CLI's record subcommand uses this to decide between
/// starting a fresh recording (`POST /v1/transcribe`) and stopping
/// an in-flight one (`POST /v1/transcribe/stop`).
#[tokio::test]
async fn handle_status_reports_busy_correctly() {
    let daemon = test_daemon().await;

    // Idle: must report `busy: Some(false)`.
    let response = daemon.handle_status().await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.busy,
        Some(false),
        "fresh daemon must report busy=false; got {response:?}"
    );

    // Force busy state and confirm the handler tracks it.
    *daemon.busy.write().await = true;
    let response = daemon.handle_status().await;
    assert_eq!(
        response.busy,
        Some(true),
        "busy=true must be surfaced by handle_status"
    );

    // Recovery: clearing the flag flips the response field back.
    *daemon.busy.write().await = false;
    let response = daemon.handle_status().await;
    assert_eq!(response.busy, Some(false));
}

#[tokio::test]
async fn busy_reset_after_error_cleanup() {
    // Verify that the error cleanup pattern in handle_record_internal
    // correctly resets busy. We can't trigger the full recording
    // pipeline in CI (requires audio hardware + display server for Typer),
    // so we simulate the state and verify cleanup.
    let daemon = test_daemon().await;

    // Simulate: setup_recording_session ran and set busy = true,
    // then record_and_transcribe failed.
    *daemon.busy.write().await = true;
    assert!(*daemon.busy.read().await);

    // The error path in handle_record_internal does:
    //   *self.busy.write().await = false;
    // (on a setup failure, no recording_started event went out, so no state change to undo)
    // Verify the daemon can recover from this state.
    {
        let mut guard = daemon.busy.write().await;
        *guard = false;
    }

    assert!(
        !*daemon.busy.read().await,
        "busy must be false after error cleanup"
    );

    // And a new recording request should NOT hit the toggle path
    // (it should try to start, not return "transcription in progress")
    // We can't fully test starting a recording here, but we verify the
    // state allows it by checking the guard is clear.
    assert!(daemon.manual_stop_tx.read().await.is_none());
}

/// An unknown theme name is a client error: `docs/protocol/endpoints/v1/audio_theme.md`
/// documents `400 invalid_audio_theme`. The daemon must reject it (not silently
/// apply the default theme and report success).
#[tokio::test]
async fn set_audio_theme_rejects_unknown_theme() {
    let daemon = test_daemon().await;
    let before = daemon.get_audio_theme();

    let resp = daemon.handle_set_audio_theme("definitely-not-a-theme".to_string());

    assert_eq!(resp.status, "error");
    assert_eq!(resp.message.as_deref(), Some("invalid_audio_theme"));
    // The rejected value must not have changed the active theme.
    assert_eq!(daemon.get_audio_theme(), before);
}

/// An unknown notification method is a client error:
/// `docs/protocol/endpoints/v1/notification_method.md` documents `400
/// invalid_notification_method`. The daemon must reject it (not silently
/// apply the default and report success), and the config must not be
/// mutated by a rejected wire set.
#[tokio::test]
async fn set_notification_method_rejects_unknown_method() {
    let daemon = test_daemon().await;
    let before = daemon.config.read().await.transcription.notification_method;

    let resp = daemon
        .handle_set_notification_method("definitely-not-a-method".to_string())
        .await;

    assert_eq!(resp.status, "error");
    assert_eq!(resp.message.as_deref(), Some("invalid_notification_method"));
    assert_eq!(resp.error_code, Some(ErrorCode::InvalidValue));
    assert_eq!(resp.error_code.map(ErrorCode::http_status), Some(400));
    // The rejected value must not have changed the persisted setting.
    assert_eq!(
        daemon.config.read().await.transcription.notification_method,
        before
    );
}

/// A valid value round-trips through `set_notification_method` and
/// `get_notification_method` end to end (dispatch parse -> handler -> config).
#[tokio::test]
async fn set_notification_method_round_trips_through_set_and_get() {
    let daemon = test_daemon().await;

    let mut set_request = make_request("set_notification_method");
    set_request.data = Some(serde_json::json!({ "method": "dbus" }));
    let set_response = daemon.handle_command(set_request).await;
    assert_eq!(set_response.status, "success");
    assert_eq!(set_response.notification_method.as_deref(), Some("dbus"));

    let get_response = daemon
        .handle_command(make_request("get_notification_method"))
        .await;
    assert_eq!(get_response.notification_method.as_deref(), Some("dbus"));
}

/// `get_update_check_enabled` reflects the config default before any write.
#[tokio::test]
async fn get_update_check_enabled_returns_default() {
    let daemon = test_daemon().await;
    let resp = daemon
        .handle_command(make_request("get_update_check_enabled"))
        .await;
    assert_eq!(resp.update_check_enabled, Some(true));
}

/// A valid value round-trips through `set_update_check_enabled` and
/// `get_update_check_enabled`, and lands in the persisted config.
#[tokio::test]
async fn set_update_check_enabled_round_trips_through_set_and_get() {
    let daemon = test_daemon().await;

    let mut set_request = make_request("set_update_check_enabled");
    set_request.enabled = Some(false);
    let set_response = daemon.handle_command(set_request).await;
    assert_eq!(set_response.status, "success");
    assert_eq!(set_response.update_check_enabled, Some(false));

    let get_response = daemon
        .handle_command(make_request("get_update_check_enabled"))
        .await;
    assert_eq!(get_response.update_check_enabled, Some(false));
    assert!(!daemon.config.read().await.update.check_enabled);
}

/// `get_update_beta_optin` reflects the config default (`auto`) before any
/// write.
#[tokio::test]
async fn get_update_beta_optin_returns_default() {
    let daemon = test_daemon().await;
    let resp = daemon
        .handle_command(make_request("get_update_beta_optin"))
        .await;
    assert_eq!(resp.update_beta_optin.as_deref(), Some("auto"));
}

/// A valid value round-trips through `set_update_beta_optin` and
/// `get_update_beta_optin` end to end (dispatch parse -> handler -> config).
#[tokio::test]
async fn set_update_beta_optin_round_trips_through_set_and_get() {
    let daemon = test_daemon().await;

    let mut set_request = make_request("set_update_beta_optin");
    set_request.data = Some(serde_json::json!({ "value": "enabled" }));
    let set_response = daemon.handle_command(set_request).await;
    assert_eq!(set_response.status, "success");
    assert_eq!(set_response.update_beta_optin.as_deref(), Some("enabled"));

    let get_response = daemon
        .handle_command(make_request("get_update_beta_optin"))
        .await;
    assert_eq!(get_response.update_beta_optin.as_deref(), Some("enabled"));
}

/// An unknown `update_beta_optin` value is a client error:
/// `docs/protocol/endpoints/v1/update_beta_optin.md` documents `400
/// invalid_update_beta_optin`. The daemon must reject it (not silently
/// apply the default and report success), and the config must not be
/// mutated by the rejected wire set.
#[tokio::test]
async fn set_update_beta_optin_rejects_unknown_value() {
    let daemon = test_daemon().await;
    let before = daemon.config.read().await.update.beta_optin;

    let resp = daemon
        .handle_set_update_beta_optin("not-a-real-value".to_string())
        .await;

    assert_eq!(resp.status, "error");
    assert_eq!(resp.message.as_deref(), Some("invalid_update_beta_optin"));
    assert_eq!(resp.error_code, Some(ErrorCode::InvalidValue));
    assert_eq!(resp.error_code.map(ErrorCode::http_status), Some(400));
    assert_eq!(daemon.config.read().await.update.beta_optin, before);
}

/// An omitted `source` resolves to the active backend — not to whichever
/// installed backend happens to serve the name. Two backends serve
/// `whisper-tiny` here; the selected one must win regardless of their order in
/// the registry, since the backend that wins is persisted as the active one.
#[tokio::test]
async fn an_omitted_source_resolves_to_the_active_backend() {
    let daemon = test_daemon().await;
    // `whisper` sorts before `zeta`, and is listed second — a scan-order
    // resolution would return `zeta` here.
    *daemon.backends.write().await = vec![
        fixture_backend_local("zeta", "github.com/other/zeta", "Zeta", "whisper-tiny"),
        fixture_backend_local(
            "whisper",
            "github.com/super-stt/whisper",
            "Whisper",
            "whisper-tiny",
        ),
    ];
    *daemon.active_backend.write().await = Some("whisper".to_string());

    assert_eq!(
        daemon.active_backend_source().await.as_deref(),
        Some("github.com/super-stt/whisper"),
        "an omitted source must resolve to the selected backend, not the first scanned"
    );
}

/// With nothing selected there is no defensible guess, so the switch fails
/// instead of binding to an arbitrary backend and persisting that choice.
#[tokio::test]
async fn an_omitted_source_with_no_active_backend_is_an_error() {
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![fixture_backend_local(
        "whisper",
        "github.com/super-stt/whisper",
        "Whisper",
        "whisper-tiny",
    )];
    assert!(daemon.active_backend.read().await.is_none());

    let mut request = make_request("set_model");
    request.data = Some(serde_json::json!({ "model": "whisper-tiny" }));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "error");
    assert_eq!(
        response.error_code,
        Some(ErrorCode::InvalidBackend),
        "expected the switch to refuse to guess a backend, got: {:?} / {:?}",
        response.error_code,
        response.message
    );
    assert!(
        daemon.active_backend.read().await.is_none(),
        "a refused switch must not select a backend"
    );
}

#[tokio::test]
async fn list_models_reflects_discovered_backends() {
    // With no backends installed, the list is empty but well-formed.
    let daemon = test_daemon().await;

    let request = make_request("list_models");
    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");

    let models = response
        .available_models
        .expect("available_models should be present");
    assert!(
        models.is_empty(),
        "no backends installed in test → empty model list, got {models:?}"
    );
}

/// Online and local models load alike: there is no runtime gate on online
/// providers. The choice to send audio to a third party is made once, when
/// an online backend is installed, and a model's own `supported_devices`
/// (`none`) is what marks it online for the UI. The load itself fails here —
/// the fixture has no component on disk — but it must get *past* resolution.
#[tokio::test]
async fn an_online_model_loads_without_a_gate() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];

    let mut request = make_request("set_model");
    request.data = Some(serde_json::json!({ "model": "whisper-1", "source": source }));

    let response = daemon.handle_command(request).await;
    assert_ne!(
        response.error_code,
        Some(ErrorCode::InvalidModel),
        "the model did not resolve: {:?}",
        response.message
    );
    assert!(
        !response
            .message
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains("online models are disabled"),
        "an online model was blocked: {:?}",
        response.message
    );
}

/// `handle_list_backends` builds the catalog JSON (models, secrets,
/// options) from the discovered backends, and an in-memory config option
/// override is reflected in an option's effective `value`. Keyring-free.
#[tokio::test]
async fn list_backends_catalog_and_option_override() {
    use crate::daemon::test_fixtures::openai_backend;
    use crate::stt_models::ModelDefinition;
    use std::time::Duration;

    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    let backend = openai_backend(
        source,
        vec![ModelDefinition {
            name: "whisper-1".to_string(),
            source: source.to_string(),
            is_multilingual: true,
            primary_language: "en".to_string(),
            supported_languages: vec!["en".to_string()],
            estimated_vram_bytes: 0,
            processing_interval: Duration::from_secs(1),
            supported_devices: vec![super_stt_registry_types::manifest::Device::None],
            realtime: false,
            role: super_stt_registry_types::manifest::ModelRole::Transcription,
            provider: None,
        }],
        // A manifest may not declare a default for `base_url`, so the catalog's
        // effective value starts unset and only the override fills it in.
        None,
    );
    *daemon.backends.write().await = vec![backend];

    let resp = daemon.handle_list_backends().await;
    assert_eq!(resp.status, "success");
    let cat = resp.backends.expect("backends catalog");
    assert_eq!(cat[0]["source"], source);
    // The backend's `[network].allowed_hosts` reaches the catalog JSON so the
    // app's "Online model" badge can name where a cloud backend's audio goes.
    assert_eq!(cat[0]["allowed_hosts"][0], "api.openai.com");
    assert_eq!(cat[0]["models"][0]["name"], "whisper-1");
    assert_eq!(cat[0]["secrets"][0]["name"], "openai_api_key");
    assert_eq!(cat[0]["secrets"][0]["label"], "OpenAI API key");
    // No override yet, and `base_url` may carry no manifest default → no value.
    assert!(cat[0]["options"][0]["value"].is_null());

    // In-memory override (avoids a config disk write in tests).
    daemon
        .config
        .write()
        .await
        .backends
        .options
        .entry(source.to_string())
        .or_default()
        .insert("base_url".to_string(), "https://gw.example".to_string());

    let cat = daemon
        .handle_list_backends()
        .await
        .backends
        .expect("backends catalog");
    assert_eq!(cat[0]["options"][0]["value"], "https://gw.example");
    // `allowed_hosts` stays the manifest's own declaration; the user's gateway
    // is reported through the option's value, which is what the settings UI
    // reads to say a user-set URL exists.
    assert_eq!(
        cat[0]["allowed_hosts"][0], "api.openai.com",
        "a user-set base_url must not be folded into the manifest's list: {:?}",
        cat[0]["allowed_hosts"]
    );
    assert!(cat[0]["allowed_hosts"][1].is_null());
}

/// `installed.json` records `"wasm"` as the accel of a wasm-kind backend's
/// installed asset, but `"wasm"` is a transport, not an accelerator. A client
/// deriving an offered device list from a non-empty `installed_accel` would
/// otherwise conclude a WebAssembly backend has real GPU compute and offer a
/// device picker it has no business showing. The companion `"cuda"` case pins
/// the actual headline behaviour of `GET /backends`: a real accel written to
/// `installed.json` must reach the wire catalog verbatim — stubbing the
/// `installed_accel` expression in `backend_config_handlers.rs` to
/// `Vec::new()` keeps the `"wasm"` case green but fails this one.
#[tokio::test]
async fn wasm_backend_reports_no_installed_accel() {
    use crate::daemon::test_fixtures::openai_backend;

    async fn catalog_installed_accel(accel_json: &str) -> serde_json::Value {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("installed.json"),
            format!(
                r#"{{"selected":{{"target":"x86_64-unknown-linux-gnu","accel":{accel_json}}}}}"#
            ),
        )
        .expect("writes");

        let daemon = test_daemon().await;
        let source = "github.com/super-stt/openai";
        let mut backend = openai_backend(source, Vec::new(), None);
        backend.dir = dir.path().to_path_buf();
        *daemon.backends.write().await = vec![backend];

        daemon
            .handle_list_backends()
            .await
            .backends
            .expect("backends catalog")[0]["installed_accel"]
            .clone()
    }

    assert_eq!(
        catalog_installed_accel(r#"["wasm"]"#).await,
        serde_json::json!([]),
        "\"wasm\" is a transport, not an accelerator, and must not surface here"
    );
    assert_eq!(
        catalog_installed_accel(r#"["cuda"]"#).await,
        serde_json::json!(["cuda"]),
        "a real accel must reach the wire catalog, not just installed::read"
    );
}

/// Build a `DiscoveredBackend` whose `dir` ends in `dir_name` and that
/// serves a single **online** model (the `none` device sentinel) — enough
/// surface for the active-backend handlers and the online gate.
fn fixture_backend(
    dir_name: &str,
    source: &str,
    name: &str,
    model_name: &str,
) -> crate::stt_models::backends::DiscoveredBackend {
    fixture_backend_devices(
        dir_name,
        source,
        name,
        model_name,
        vec![super_stt_registry_types::manifest::Device::None],
    )
}

/// [`fixture_backend`], but serving a **local** model — one the online gate
/// must let through.
fn fixture_backend_local(
    dir_name: &str,
    source: &str,
    name: &str,
    model_name: &str,
) -> crate::stt_models::backends::DiscoveredBackend {
    fixture_backend_devices(
        dir_name,
        source,
        name,
        model_name,
        vec![super_stt_registry_types::manifest::Device::Cpu],
    )
}

fn fixture_backend_devices(
    dir_name: &str,
    source: &str,
    name: &str,
    model_name: &str,
    supported_devices: Vec<super_stt_registry_types::manifest::Device>,
) -> crate::stt_models::backends::DiscoveredBackend {
    use crate::stt_models::ModelDefinition;
    use crate::stt_models::backends::DiscoveredBackend;
    use std::time::Duration;

    DiscoveredBackend {
        description: String::new(),
        dir: std::path::PathBuf::from("/tmp").join(dir_name),
        source: source.to_string(),
        id: None,
        name: name.to_string(),
        version: "1.0.0".to_string(),
        kind: "wasm".to_string(),
        entrypoint: format!("{dir_name}.wasm"),
        allowed_hosts: Vec::new(),
        secrets: Vec::new(),
        options: Vec::new(),
        models: vec![ModelDefinition {
            name: model_name.to_string(),
            source: source.to_string(),
            is_multilingual: true,
            primary_language: "en".to_string(),
            supported_languages: vec!["en".to_string()],
            estimated_vram_bytes: 0,
            processing_interval: Duration::from_secs(1),
            supported_devices,
            realtime: false,
            role: super_stt_registry_types::manifest::ModelRole::Transcription,
            provider: None,
        }],
    }
}

/// `handle_get_active_backend` returns `null` when nothing is selected.
#[tokio::test]
async fn get_active_backend_returns_null_when_idle() {
    let daemon = test_daemon().await;
    let resp = daemon.handle_get_active_backend().await;
    assert_eq!(resp.status, "success");
    assert_eq!(
        resp.active_backend,
        Some(serde_json::Value::Null),
        "idle daemon → active_backend: null"
    );
}

/// `handle_get_gpu_info` always succeeds and returns the typed GPU list (empty
/// on headless/CI hosts). Hardware-independent: asserts presence only.
#[tokio::test]
async fn get_gpu_info_returns_success_array() {
    let resp = SuperSTTDaemon::handle_get_gpu_info().await;
    assert_eq!(resp.status, "success");
    assert!(resp.gpu_info.is_some(), "gpu_info must be present");
}

/// Real-hardware check: the daemon reports a non-empty, well-formed GPU
/// inventory. Ignored by default because CI runners have no GPU — run it on
/// a machine that does:
///   cargo test -p super-stt-daemon --all-features `gpu_info_reports_real_hardware` -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires a real GPU (NVML/sysfs); run with --ignored"]
async fn gpu_info_reports_real_hardware() {
    const VENDORS: [&str; 5] = ["nvidia", "amd", "intel", "apple", "unknown"];
    let resp = SuperSTTDaemon::handle_get_gpu_info().await;
    assert_eq!(resp.status, "success");
    let gpus = resp.gpu_info.expect("gpu_info present");
    eprintln!(
        "daemon gpu_info = {}",
        serde_json::to_string_pretty(&gpus).unwrap()
    );
    assert!(!gpus.is_empty(), "expected at least one GPU on this host");
    for gpu in &gpus {
        assert!(!gpu.name.is_empty(), "each GPU needs a non-empty name");
        assert!(gpu.total_bytes > 0, "each GPU needs total_bytes > 0");
        assert!(
            VENDORS.contains(&gpu.vendor.as_str()),
            "vendor {:?} must be a known snake_case tag",
            gpu.vendor
        );
    }
}

/// Happy path: setting the active backend to an installed source records
/// the install dir in both the runtime lock and the in-memory config, and
/// the response payload carries `{source, name, model_loaded: false}`.
#[tokio::test]
async fn set_active_backend_records_dir_and_returns_payload() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];

    let resp = daemon.handle_set_active_backend(source.to_string()).await;
    assert_eq!(resp.status, "success");
    let payload = resp.active_backend.expect("payload");
    assert_eq!(payload["source"], source);
    assert_eq!(payload["name"], "OpenAI");
    assert_eq!(
        payload["model_loaded"], false,
        "selecting a backend does not load a model"
    );

    // Runtime lock + config mirror the relative install dir, not the source.
    assert_eq!(
        daemon.active_backend.read().await.as_deref(),
        Some("openai")
    );
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .transcription
            .active_backend
            .as_deref(),
        Some("openai")
    );
}

/// Unknown source → error. The runtime lock stays unset; no foreign-model
/// unload happens.
#[tokio::test]
async fn set_active_backend_unknown_source_errors() {
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![fixture_backend(
        "openai",
        "github.com/super-stt/openai",
        "OpenAI",
        "whisper-1",
    )];

    let resp = daemon
        .handle_set_active_backend("github.com/example/unknown".to_string())
        .await;
    assert_eq!(resp.status, "error");
    assert!(
        resp.message
            .as_deref()
            .unwrap_or("")
            .contains("github.com/example/unknown"),
        "error should name the offending source: {:?}",
        resp.message
    );
    assert!(daemon.active_backend.read().await.is_none());
}

/// `handle_get_active_backend` after a successful set returns the payload
/// for the same backend.
#[tokio::test]
async fn get_active_backend_reflects_set() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];

    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    let resp = daemon.handle_get_active_backend().await;
    let payload = resp.active_backend.expect("payload");
    assert_eq!(payload["source"], source);
    assert_eq!(payload["name"], "OpenAI");
}

/// `handle_clear_active_backend` returns the daemon to idle: runtime lock
/// unset, config field unset, `get_active_backend` reports null.
#[tokio::test]
async fn clear_active_backend_returns_to_idle() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    assert!(daemon.active_backend.read().await.is_some());

    let resp = daemon.handle_clear_active_backend().await;
    assert_eq!(resp.status, "success");
    assert!(daemon.active_backend.read().await.is_none());
    assert!(
        daemon
            .config
            .read()
            .await
            .transcription
            .active_backend
            .is_none()
    );

    let resp = daemon.handle_get_active_backend().await;
    assert_eq!(resp.active_backend, Some(serde_json::Value::Null));
}

/// `handle_list_models` is scoped to the active backend's models. With no
/// active backend, the list is empty even when backends are installed.
/// With one selected, only its models appear.
#[tokio::test]
async fn list_models_is_scoped_to_active_backend() {
    let daemon = test_daemon().await;
    let openai = "github.com/super-stt/openai";
    let mistral = "github.com/super-stt/mistral";
    *daemon.backends.write().await = vec![
        fixture_backend("openai", openai, "OpenAI", "whisper-1"),
        fixture_backend("mistral", mistral, "Mistral", "voxtral-mini-latest"),
    ];

    // Idle → empty list (even though two backends are installed).
    let response = daemon.handle_list_models().await;
    let models = response.available_models.expect("available_models");
    assert!(
        models.is_empty(),
        "no active backend → empty list, got {models:?}"
    );

    // Select OpenAI → only its model.
    let _ = daemon.handle_set_active_backend(openai.to_string()).await;
    let response = daemon.handle_list_models().await;
    let models = response.available_models.expect("available_models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].0, "whisper-1");
    assert_eq!(models[0].1, openai);

    // Switch to Mistral → only its model.
    let _ = daemon.handle_set_active_backend(mistral.to_string()).await;
    let response = daemon.handle_list_models().await;
    let models = response.available_models.expect("available_models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].0, "voxtral-mini-latest");
    assert_eq!(models[0].1, mistral);
}

/// Trivial in-process `Transcribe` impl used to seed the `model` lock so
/// the always-unload semantics of `set_active_backend` can be observed
/// without touching real inference code.
struct MockTranscribe {
    info: crate::stt_models::transcribe::ModelInfoData,
}
impl crate::stt_models::transcribe::ModelInfo for MockTranscribe {
    fn info(&self) -> &crate::stt_models::transcribe::ModelInfoData {
        &self.info
    }
}
impl crate::stt_models::transcribe::ModelState for MockTranscribe {
    fn device(&self) -> String {
        "cpu".to_string()
    }
}
#[async_trait::async_trait]
impl crate::stt_models::transcribe::Transcribe for MockTranscribe {
    async fn transcribe_audio(
        &mut self,
        _audio: &[f32],
        _sample_rate: u32,
        _language: Option<&str>,
    ) -> anyhow::Result<String> {
        Ok(String::new())
    }
}

/// Place a loaded mock model in the daemon's `model` lock with the given
/// `(name, source)`. Used to verify the always-unload semantics in
/// `handle_set_active_backend`.
async fn seed_loaded_model(daemon: &SuperSTTDaemon, name: &str, source: &str) {
    use crate::daemon::types::LoadedModel;
    use crate::stt_models::ModelDefinition;
    use crate::stt_models::transcribe::ModelInfoData;
    use std::time::Duration;

    let definition = ModelDefinition {
        name: name.to_string(),
        source: source.to_string(),
        is_multilingual: true,
        primary_language: "en".to_string(),
        supported_languages: vec!["en".to_string()],
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        supported_devices: vec![super_stt_registry_types::manifest::Device::None],
        realtime: false,
        role: super_stt_registry_types::manifest::ModelRole::Transcription,
        provider: None,
    };
    let info = ModelInfoData::new(name, source, true, true, Duration::from_secs(1));
    *daemon.model.write().await = Some(LoadedModel {
        definition,
        instance: Box::new(MockTranscribe { info }),
    });
}

/// A backend option write for the *active* backend triggers a reload so the
/// change takes effect. If that reload fails (here: the backend isn't installed,
/// so re-instantiation errors), the option write still succeeds — but the reload
/// failure must be surfaced in the response, not silently swallowed.
#[tokio::test]
async fn set_backend_option_surfaces_reload_failure() {
    let daemon = test_daemon().await;
    let source = "github.com/x/not-installed";
    seed_loaded_model(&daemon, "m", source).await;

    let resp = daemon
        .handle_set_backend_option(
            source.to_string(),
            "base_url".to_string(),
            "https://x".to_string(),
        )
        .await;

    assert_eq!(resp.status, "success", "the option write itself succeeds");
    let msg = resp.message.unwrap_or_default();
    assert!(
        msg.to_lowercase().contains("reload"),
        "reload failure should be surfaced, got: {msg}"
    );
}

/// Switching from backend A (with a loaded model) to backend B drops the
/// loaded model — the documented postcondition: after `set_active_backend`,
/// `/active_model` returns `null` until the user explicitly picks one.
#[tokio::test]
async fn set_active_backend_unloads_model_on_dir_change() {
    let daemon = test_daemon().await;
    let a = "github.com/super-stt/openai";
    let b = "github.com/super-stt/mistral";
    *daemon.backends.write().await = vec![
        fixture_backend("openai", a, "OpenAI", "whisper-1"),
        fixture_backend("mistral", b, "Mistral", "voxtral-mini-latest"),
    ];

    // Start active on A with a model loaded.
    let _ = daemon.handle_set_active_backend(a.to_string()).await;
    seed_loaded_model(&daemon, "whisper-1", a).await;
    assert!(daemon.model.read().await.is_some());

    // Switch to B — model must be gone.
    let resp = daemon.handle_set_active_backend(b.to_string()).await;
    assert_eq!(resp.status, "success");
    assert!(
        daemon.model.read().await.is_none(),
        "switching backends must leave the daemon idle"
    );
    assert_eq!(
        daemon.active_backend.read().await.as_deref(),
        Some("mistral")
    );
}

/// Re-selecting the same backend (same install dir) is a no-op for the
/// loaded model — there's no reason to disturb it.
#[tokio::test]
async fn set_active_backend_same_source_does_not_unload() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];

    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    seed_loaded_model(&daemon, "whisper-1", source).await;
    assert!(daemon.model.read().await.is_some());

    // Redundant set — should not touch the model.
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    assert!(
        daemon.model.read().await.is_some(),
        "re-selecting the same backend must not unload the loaded model"
    );
}

/// Going from idle (no active backend, no model) to selecting a backend
/// unloads nothing (already idle) and the model stays `None`.
#[tokio::test]
async fn set_active_backend_from_idle_keeps_model_none() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];

    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    assert!(daemon.model.read().await.is_none());
}

/// A device belongs to a model, and setting it for a model that is not
/// loaded only records the choice — it does not error, and it loads nothing.
/// The model's next load picks it up. This is what lets the device picker
/// work before Load is pressed.
#[tokio::test]
async fn set_model_device_when_not_loaded_only_records_the_preference() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/whisper";
    *daemon.backends.write().await = vec![fixture_backend_devices(
        "whisper",
        source,
        "Whisper",
        "large",
        vec![
            super_stt_registry_types::manifest::Device::Cpu,
            super_stt_registry_types::manifest::Device::Gpu,
        ],
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    assert!(daemon.model.read().await.is_none());
    assert_eq!(
        daemon.config.read().await.effective_device(source, "large"),
        "cpu"
    );

    // "cuda" is the deprecated spelling, accepted and normalized to "gpu".
    let response = daemon
        .handle_set_model_device(
            PipelineStage::Transcription,
            "large".to_string(),
            "cuda".to_string(),
        )
        .await;
    assert_eq!(
        response.status, "success",
        "setting a device for an unloaded model must succeed; got error: {:?}",
        response.message,
    );
    assert_eq!(response.device.as_deref(), Some("gpu"));
    assert_eq!(
        response.resolved_accel,
        Some(None),
        "a gpu choice has resolved to nothing before a load confirms it"
    );
    assert!(
        response
            .available_devices
            .as_ref()
            .is_some_and(|d| d.iter().any(|d| d == "cpu")),
        "the model declares the CPU and every host has one: {:?}",
        response.available_devices
    );
    assert!(daemon.model.read().await.is_none(), "nothing was loaded");
    assert_eq!(
        daemon.config.read().await.model_device(source, "large"),
        Some("gpu"),
        "the choice is the model's own now"
    );
    assert_eq!(
        daemon.config.read().await.device.preferred_device,
        "cpu",
        "the global default is not what a per-model setter writes"
    );

    // The getter answers from the same state.
    let response = daemon
        .handle_get_model_device(PipelineStage::Transcription, "large".to_string())
        .await;
    assert_eq!(response.status, "success");
    assert_eq!(response.device.as_deref(), Some("gpu"));
}

/// An invalid device value is rejected before anything is looked up or
/// stored, with the documented `invalid_device` code.
#[tokio::test]
async fn set_model_device_rejects_invalid_device() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;

    for device in ["xpu", "none"] {
        let response = daemon
            .handle_set_model_device(
                PipelineStage::Transcription,
                "large".to_string(),
                device.to_string(),
            )
            .await;
        assert_eq!(response.status, "error", "{device}");
        assert_eq!(
            response.error_code,
            Some(ErrorCode::InvalidDevice),
            "the documented 400 invalid_device carries its code, or an uncoded \
             error would map to 500 instead ({device})"
        );
    }
    assert!(daemon.config.read().await.backends.models.is_empty());
}

/// The path names a stage, so the model resolves against that stage's
/// selected backend — and with none selected there is nothing to resolve
/// against.
#[tokio::test]
async fn model_device_needs_the_stage_to_have_a_backend() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;

    let response = daemon
        .handle_get_model_device(PipelineStage::Transcription, "large".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidBackend));

    let response = daemon
        .handle_set_model_device(
            PipelineStage::PostProcessor,
            "cleanup".to_string(),
            "cpu".to_string(),
        )
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidBackend));
}

/// A model the stage's backend does not serve, or one of the other stage's
/// role, is `invalid_model`.
#[tokio::test]
async fn model_device_refuses_a_model_the_stage_cannot_run() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/both";
    let mut backend = fixture_backend_local("both", source, "Both", "whisper");
    let mut cleanup = backend.models[0].clone();
    cleanup.name = "cleanup".to_string();
    cleanup.role = super_stt_registry_types::manifest::ModelRole::PostProcessor;
    backend.models.push(cleanup);
    *daemon.backends.write().await = vec![backend];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    daemon.config.write().await.post_processor.source = source.to_string();

    let response = daemon
        .handle_get_model_device(PipelineStage::Transcription, "absent".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidModel));

    // Right backend, wrong stage: the post-processor is not a stage-1 model.
    let response = daemon
        .handle_get_model_device(PipelineStage::Transcription, "cleanup".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidModel));
    let response = daemon
        .handle_get_model_device(PipelineStage::PostProcessor, "whisper".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidModel));

    // Each through its own stage resolves.
    let response = daemon
        .handle_get_model_device(PipelineStage::Transcription, "whisper".to_string())
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    let response = daemon
        .handle_get_model_device(PipelineStage::PostProcessor, "cleanup".to_string())
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
}

/// An online model runs remotely: it has no device to set, and reading one
/// reports the manifest's own `none` with nothing offered.
#[tokio::test]
async fn an_online_model_has_no_device() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    seed_loaded_model(&daemon, "whisper-1", source).await;

    let response = daemon
        .handle_set_model_device(
            PipelineStage::Transcription,
            "whisper-1".to_string(),
            "cuda".to_string(),
        )
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidDevice));
    assert!(
        daemon.model.read().await.is_some(),
        "an online model must not be unloaded by a device request"
    );

    let response = daemon
        .handle_get_model_device(PipelineStage::Transcription, "whisper-1".to_string())
        .await;
    assert_eq!(response.status, "success");
    assert_eq!(response.device.as_deref(), Some("none"));
    assert_eq!(response.resolved_accel, Some(None));
    assert_eq!(response.available_devices, Some(Vec::new()));
}

/// The manifest rules: a model declaring only the CPU cannot be sent to the
/// GPU, and the refusal leaves nothing stored.
#[tokio::test]
async fn set_model_device_refuses_a_device_the_model_does_not_declare() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/tiny";
    *daemon.backends.write().await = vec![fixture_backend_local("tiny", source, "Tiny", "tiny")];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let response = daemon
        .handle_set_model_device(
            PipelineStage::Transcription,
            "tiny".to_string(),
            "gpu".to_string(),
        )
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidDevice));
    assert_eq!(
        daemon.config.read().await.model_device(source, "tiny"),
        None
    );
}

/// Asking the loaded model for the device it is already on reloads nothing
/// — but still makes the device the model's own, since it may have been on
/// it only through the global default.
#[tokio::test]
async fn set_model_device_on_the_device_in_use_reloads_nothing() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/whisper";
    *daemon.backends.write().await =
        vec![fixture_backend_local("whisper", source, "Whisper", "small")];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    // A loaded local model, reporting `cpu` from its instance.
    seed_loaded_model(&daemon, "small", source).await;
    daemon
        .model
        .write()
        .await
        .as_mut()
        .unwrap()
        .definition
        .supported_devices = vec![super_stt_registry_types::manifest::Device::Cpu];

    let response = daemon
        .handle_set_model_device(
            PipelineStage::Transcription,
            "small".to_string(),
            "cpu".to_string(),
        )
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    assert_eq!(response.device.as_deref(), Some("cpu"));
    assert_eq!(
        response.resolved_accel,
        Some(Some("cpu".to_string())),
        "a loaded model reports the device it is on"
    );
    assert!(daemon.model.read().await.is_some(), "not unloaded");
    assert_eq!(
        daemon.config.read().await.model_device(source, "small"),
        Some("cpu")
    );
}

/// The list verbs answer from the same narrowing the device verb reports as
/// `available_devices`: per model, its own list; per stage, the union over
/// the models the stage can run — so a backend's online model contributes
/// nothing and its post-processor is stage 2's business, not stage 1's.
#[tokio::test]
async fn device_lists_answer_per_model_and_per_stage() {
    use crate::daemon::device_management::PipelineStage;
    use super_stt_registry_types::manifest::{Device, ModelRole};
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/mixed";
    let mut backend = fixture_backend_devices("mixed", source, "Mixed", "local", vec![Device::Cpu]);
    let mut online = backend.models[0].clone();
    online.name = "online".to_string();
    online.supported_devices = vec![Device::None];
    let mut cleanup = backend.models[0].clone();
    cleanup.name = "cleanup".to_string();
    cleanup.role = ModelRole::PostProcessor;
    cleanup.supported_devices = vec![Device::Cpu, Device::Gpu];
    backend.models.extend([online, cleanup]);
    *daemon.backends.write().await = vec![backend];

    // Nothing selected: nothing to list against.
    let response = daemon
        .handle_list_stage_devices(PipelineStage::Transcription)
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidBackend));

    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    daemon.config.write().await.post_processor.source = source.to_string();

    let response = daemon
        .handle_list_model_devices(PipelineStage::Transcription, "local".to_string())
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    assert_eq!(response.available_devices, Some(vec!["cpu".to_string()]));
    let response = daemon
        .handle_list_model_devices(PipelineStage::Transcription, "online".to_string())
        .await;
    assert_eq!(response.available_devices, Some(Vec::new()));
    let response = daemon
        .handle_list_model_devices(PipelineStage::Transcription, "cleanup".to_string())
        .await;
    assert_eq!(
        response.error_code,
        Some(ErrorCode::InvalidModel),
        "a post-processor is not a stage-1 model"
    );

    // Stage 1 sees only the transcription models: the CPU-only one.
    let response = daemon
        .handle_list_stage_devices(PipelineStage::Transcription)
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    assert_eq!(response.available_devices, Some(vec!["cpu".to_string()]));

    // Stage 2 sees the post-processor, which declares the GPU too — offered
    // only where the host has one, and the CPU everywhere.
    let response = daemon
        .handle_list_stage_devices(PipelineStage::PostProcessor)
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    let devices = response.available_devices.expect("a list");
    assert_eq!(devices.first().map(String::as_str), Some("cpu"));
    assert!(devices.iter().all(|d| d == "cpu" || d == "gpu"));
}

/// Stage 2 stores the same way: a post-processor that is not loaded gets
/// its device recorded against its own `(source, model)`, leaving stage 1's
/// models alone.
#[tokio::test]
async fn set_post_processor_device_when_not_loaded_only_records_the_preference() {
    use crate::daemon::device_management::PipelineStage;
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/cleanup";
    let mut backend = fixture_backend_devices(
        "cleanup",
        source,
        "Cleanup",
        "cleanup",
        vec![
            super_stt_registry_types::manifest::Device::Cpu,
            super_stt_registry_types::manifest::Device::Gpu,
        ],
    );
    backend.models[0].role = super_stt_registry_types::manifest::ModelRole::PostProcessor;
    *daemon.backends.write().await = vec![backend];
    daemon.config.write().await.post_processor.source = source.to_string();

    let response = daemon
        .handle_set_model_device(
            PipelineStage::PostProcessor,
            "cleanup".to_string(),
            "gpu".to_string(),
        )
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    assert_eq!(response.device.as_deref(), Some("gpu"));
    assert!(
        daemon.post_processor.read().await.is_none(),
        "nothing was loaded"
    );
    assert_eq!(
        daemon.config.read().await.model_device(source, "cleanup"),
        Some("gpu")
    );
}

/// `unload_active_model` is a no-op when no model is loaded — success
/// with a clear message — and otherwise drops the model lock while
/// leaving `active_backend` selected so the user can pick another model.
#[tokio::test]
async fn unload_active_model_drops_model_keeps_backend() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];

    // No-op case: nothing to unload.
    let resp = daemon.handle_unload_active_model().await;
    assert_eq!(resp.status, "success");
    assert_eq!(resp.message.as_deref(), Some("No model to unload"));

    // Activate backend + seed a loaded model, then unload.
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    seed_loaded_model(&daemon, "whisper-1", source).await;
    assert!(daemon.model.read().await.is_some());

    let resp = daemon.handle_unload_active_model().await;
    assert_eq!(resp.status, "success");
    assert!(daemon.model.read().await.is_none(), "model lock cleared");
    assert_eq!(
        daemon.active_backend.read().await.as_deref(),
        Some("openai"),
        "active backend stays selected after unload",
    );
    // The persisted preferred_model is cleared so a daemon restart stays idle.
    assert!(
        daemon
            .config
            .read()
            .await
            .transcription
            .preferred_model
            .is_empty()
    );
}

/// The three new commands dispatch through `handle_command`, exercising the
/// shared protocol parse → core dispatch → handler chain end-to-end.
#[tokio::test]
async fn active_backend_commands_dispatch_through_handle_command() {
    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "whisper-1")];

    // set_active_backend
    let mut request = make_request("set_active_backend");
    request.data = Some(serde_json::json!({ "source": source }));
    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(response.active_backend.as_ref().unwrap()["source"], source);

    // get_active_backend
    let response = daemon
        .handle_command(make_request("get_active_backend"))
        .await;
    assert_eq!(response.status, "success");
    assert_eq!(response.active_backend.as_ref().unwrap()["name"], "OpenAI");

    // clear_active_backend
    let response = daemon
        .handle_command(make_request("clear_active_backend"))
        .await;
    assert_eq!(response.status, "success");
    assert!(daemon.active_backend.read().await.is_none());
}

/// A successful model load — whether a user-initiated switch or the daemon's
/// startup load of the persisted model — must broadcast a self-contained
/// `model_switched` event carrying the model's full identity (`model_name`,
/// `source`) followed by the operational `ready` event. A settings
/// app reconnecting after a daemon restart has no prior `current_source` to
/// fall back to, so `source` must be on the wire for it to mark the model
/// loaded — otherwise the model loads (visible in logs / htop) but the UI keeps
/// showing "no model loaded".
#[tokio::test]
async fn broadcast_model_active_carries_full_identity() {
    use crate::daemon::events::Topic;

    let daemon = test_daemon().await;
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    daemon.broadcast_model_active(
        "voxtral-mini",
        "github.com/super-stt/mistral",
        "cuda",
        crate::daemon::device_management::PipelineStage::Transcription,
    );

    let (_topic, switched) = rx.recv_json().await.expect("model_switched event");
    assert_eq!(switched["status"], "model_switched");
    assert_eq!(switched["model_name"], "voxtral-mini");
    assert_eq!(switched["source"], "github.com/super-stt/mistral");

    let (_topic, ready) = rx.recv_json().await.expect("ready event");
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["model_loaded"], true);
    assert_eq!(ready["model_name"], "voxtral-mini");
}

/// Stage 2 reports its own model lifecycle. Without that, the only notice a
/// client got of a post-processor load was the download ticks, which named a
/// model and left the client to guess whose it was — and a load with nothing
/// to download, or an unload, was announced not at all.
#[tokio::test]
async fn a_post_processor_load_announces_itself_as_stage_two() {
    use crate::daemon::device_management::PipelineStage;
    use crate::daemon::events::Topic;

    let daemon = test_daemon().await;
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    daemon.broadcast_model_loading_status("s1-mini", PipelineStage::PostProcessor);
    daemon.broadcast_model_active(
        "s1-mini",
        "github.com/super-stt/s1-mini",
        "cpu",
        PipelineStage::PostProcessor,
    );

    let (_topic, loading) = rx.recv_json().await.expect("loading_model event");
    assert_eq!(loading["status"], "loading_model");
    assert_eq!(loading["stage"], 2);

    let (_topic, switched) = rx.recv_json().await.expect("model_switched event");
    assert_eq!(switched["status"], "model_switched");
    assert_eq!(switched["model_name"], "s1-mini");
    assert_eq!(switched["stage"], 2);

    let (_topic, ready) = rx.recv_json().await.expect("ready event");
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["model_loaded"], true);
    assert_eq!(ready["stage"], 2);
}

/// The handler path, not only the helper: selecting a post-processor over the
/// wire announces `loading_model` for stage 2 before the load is attempted.
/// That announcement is the only cue a client that did not ask for the load
/// ever gets, so it must not depend on the load succeeding. The fixture
/// cannot be instantiated, which is the point: the announcement arrives, the
/// selection is kept, and the response says the model is not loaded.
#[tokio::test]
async fn selecting_a_post_processor_announces_its_load_as_stage_two() {
    use crate::daemon::events::Topic;
    use super_stt_registry_types::manifest::ModelRole;

    let daemon = test_daemon().await;
    let source = "github.com/super-stt/textclean";
    let mut backend = fixture_backend_local("textclean", source, "TextClean", "cleanup");
    backend.models[0].role = ModelRole::PostProcessor;
    *daemon.backends.write().await = vec![backend];
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    let mut request = make_request("set_post_processor");
    request.data = Some(serde_json::json!({ "model": "cleanup", "source": source }));
    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    let message = response.message.unwrap_or_default();
    assert!(
        message.contains("not loaded"),
        "the fixture cannot load, and the answer must say so: {message}"
    );

    let (_topic, loading) = rx.recv_json().await.expect("loading_model event");
    assert_eq!(loading["status"], "loading_model");
    assert_eq!(loading["new_model"], "cleanup");
    assert_eq!(loading["stage"], 2);

    // The failed load did not undo the selection.
    let config = daemon.config.read().await;
    assert_eq!(config.post_processor.selection(), Some(("cleanup", source)));
}

/// The polled mirror of the same rule: `GET /pipeline` reports an in-flight
/// download under the stage that started it. The daemon runs one at a time,
/// and before it said which stage that was, a post-processor's download
/// surfaced as stage 1's — the progress bar under the transcription card.
#[tokio::test]
async fn a_stage_reports_only_its_own_download() {
    use crate::download_progress::DownloadProgressTracker;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use super_stt_shared::models::protocol::POST_PROCESSOR_STAGE;

    let daemon = test_daemon().await;
    let tracker = Arc::new(DownloadProgressTracker::new(
        "s1-mini-q4_k_m".to_string(),
        "github.com/super-stt/s1-mini".to_string(),
        POST_PROCESSOR_STAGE,
        2,
        Arc::new(AtomicBool::new(false)),
    ));
    daemon
        .download_manager
        .start_download(tracker)
        .expect("register download");

    let pipeline = daemon
        .handle_get_pipeline()
        .await
        .pipeline
        .expect("pipeline");
    let stages = pipeline.as_array().expect("stages");
    assert!(
        stages[0]["switch"].is_null(),
        "stage 1 must not report the post-processor's download"
    );
    assert_eq!(stages[1]["switch"]["target"]["model"], "s1-mini-q4_k_m");
    assert_eq!(
        stages[1]["switch"]["target"]["source"],
        "github.com/super-stt/s1-mini"
    );
}

/// Cancel is addressed to a stage, so a stage with nothing of its own in
/// flight has nothing to cancel — even while the other stage downloads. Before
/// stage 2 had a cancel at all, its card's Cancel button reached stage 1's.
#[tokio::test]
async fn cancel_abandons_only_the_addressed_stages_download() {
    use crate::daemon::device_management::PipelineStage;
    use crate::download_progress::DownloadProgressTracker;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use super_stt_shared::models::protocol::{ErrorCode, POST_PROCESSOR_STAGE};

    let daemon = test_daemon().await;
    let stage_two = Arc::new(DownloadProgressTracker::new(
        "s1-mini-q4_k_m".to_string(),
        "github.com/super-stt/s1-mini".to_string(),
        POST_PROCESSOR_STAGE,
        2,
        Arc::new(AtomicBool::new(false)),
    ));
    daemon
        .download_manager
        .start_download(Arc::clone(&stage_two))
        .expect("register download");

    // Stage 1 has nothing in flight, and the post-processor's download is not
    // its to abandon.
    let resp = daemon.handle_cancel_download(PipelineStage::Transcription);
    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_code, Some(ErrorCode::NoSwitchInProgress));
    assert!(!stage_two.is_cancelled());

    let resp = daemon.handle_cancel_download(PipelineStage::PostProcessor);
    assert_eq!(resp.status, "success");
    assert!(stage_two.is_cancelled());
}

/// Reloading an idle stage is a no-op, not an error: the caller asked for the
/// running model to pick up a change, and there is no running model.
#[tokio::test]
async fn reloading_an_idle_post_processor_is_a_no_op() {
    let daemon = test_daemon().await;
    let resp = daemon.handle_reload_post_processor().await;
    assert_eq!(resp.status, "success");
    assert_eq!(resp.message.as_deref(), Some("No post-processor to reload"));
}

/// The reported gap: an option or secret written for a backend reloaded only
/// the transcription model, so a post-processor kept running with the value
/// the user had just replaced — an API key change that silently did nothing.
/// Both stages are reloaded now; the fake's source resolves to no installed
/// backend, so the attempt fails and says so, which is what proves it ran.
#[tokio::test]
async fn changing_an_option_reloads_the_post_processor_too() {
    let daemon = test_daemon().await;
    seed_scripted_post_processor(&daemon).await;

    let resp = daemon
        .handle_set_backend_option(
            "github.com/super-stt/test".to_string(),
            "style".to_string(),
            "terse".to_string(),
        )
        .await;
    assert_eq!(resp.status, "success");
    let message = resp.message.unwrap_or_default();
    assert!(
        message.contains("reloading the running model failed"),
        "the post-processor's stage must be reloaded: {message}"
    );

    // A backend neither stage is running is not reloaded at all.
    let resp = daemon
        .handle_set_backend_option(
            "github.com/super-stt/other".to_string(),
            "style".to_string(),
            "terse".to_string(),
        )
        .await;
    assert_eq!(resp.message.as_deref(), Some("Option style updated"));
}

/// Stage 1's events keep saying stage 1, so a client filtering on the field
/// sees the transcription lifecycle exactly where it always was.
#[tokio::test]
async fn a_transcription_load_still_announces_itself_as_stage_one() {
    use crate::daemon::device_management::PipelineStage;
    use crate::daemon::events::Topic;

    let daemon = test_daemon().await;
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    daemon.broadcast_model_loading_status("whisper-tiny", PipelineStage::Transcription);
    let (_topic, loading) = rx.recv_json().await.expect("loading_model event");
    assert_eq!(loading["stage"], 1);
}

/// An unload that answers a user request says stage 2 went idle; an unload of
/// a stage that was already idle says nothing, since an event reporting a stop
/// should mean something was running.
#[tokio::test]
async fn unloading_the_post_processor_announces_only_when_one_was_loaded() {
    use crate::daemon::device_management::PipelineStage;
    use crate::daemon::events::Topic;

    let daemon = test_daemon().await;
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    // Nothing loaded: the unload must announce nothing. A marker event
    // published straight after is what proves it — the next event to arrive is
    // the marker, not an unload nobody performed.
    daemon.unload_post_processor_announced().await;
    daemon.broadcast_model_loading_status("marker", PipelineStage::Transcription);
    let (_topic, first) = rx.recv_json().await.expect("marker event");
    assert_eq!(
        first["status"], "loading_model",
        "an idle stage announces nothing"
    );

    seed_scripted_post_processor(&daemon).await;
    daemon.unload_post_processor_announced().await;
    let (_topic, ready) = rx.recv_json().await.expect("ready event");
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["model_loaded"], false);
    assert_eq!(ready["stage"], 2);
}

/// A `Transcribe` fake whose `transcribe_audio` returns a fixed text or fails,
/// for characterizing the one-shot `handle_transcribe` policy.
struct ScriptedTranscribe {
    info: crate::stt_models::transcribe::ModelInfoData,
    /// `Ok(text)` to return `text`; `Err(())` to fail like a real backend would.
    result: Result<String, ()>,
}
impl crate::stt_models::transcribe::ModelInfo for ScriptedTranscribe {
    fn info(&self) -> &crate::stt_models::transcribe::ModelInfoData {
        &self.info
    }
}
impl crate::stt_models::transcribe::ModelState for ScriptedTranscribe {
    fn device(&self) -> String {
        "cpu".to_string()
    }
}
#[async_trait::async_trait]
impl crate::stt_models::transcribe::Transcribe for ScriptedTranscribe {
    async fn transcribe_audio(
        &mut self,
        _audio: &[f32],
        _sample_rate: u32,
        _language: Option<&str>,
    ) -> anyhow::Result<String> {
        match &self.result {
            Ok(text) => Ok(text.clone()),
            Err(()) => anyhow::bail!("scripted backend failure"),
        }
    }
}

/// Seed the daemon's `model` lock with a [`ScriptedTranscribe`]. `online`
/// selects the async vs. blocking dispatch path; `result` is what the backend
/// produces.
async fn seed_scripted_model(daemon: &SuperSTTDaemon, online: bool, result: Result<String, ()>) {
    use crate::daemon::types::LoadedModel;
    use crate::stt_models::ModelDefinition;
    use crate::stt_models::transcribe::ModelInfoData;
    use std::time::Duration;

    let definition = ModelDefinition {
        name: "scripted".to_string(),
        source: "github.com/super-stt/test".to_string(),
        is_multilingual: true,
        primary_language: "en".to_string(),
        supported_languages: vec!["en".to_string()],
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        supported_devices: vec![super_stt_registry_types::manifest::Device::Cpu],
        realtime: false,
        role: super_stt_registry_types::manifest::ModelRole::Transcription,
        provider: None,
    };
    let info = ModelInfoData::new(
        "scripted",
        "github.com/super-stt/test",
        true,
        online,
        Duration::from_secs(1),
    );
    *daemon.model.write().await = Some(LoadedModel {
        definition,
        instance: Box::new(ScriptedTranscribe { info, result }),
    });
}

/// Seed the daemon's `post_processor` slot with a fake, so an unload has
/// something to unload. The instance never runs — only the slot's occupancy
/// matters here.
async fn seed_scripted_post_processor(daemon: &SuperSTTDaemon) {
    use crate::daemon::types::LoadedModel;
    use crate::stt_models::ModelDefinition;
    use crate::stt_models::transcribe::ModelInfoData;
    use std::time::Duration;

    let definition = ModelDefinition {
        name: "scripted-pp".to_string(),
        source: "github.com/super-stt/test".to_string(),
        is_multilingual: false,
        primary_language: "en".to_string(),
        supported_languages: vec!["en".to_string()],
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        supported_devices: vec![super_stt_registry_types::manifest::Device::Cpu],
        realtime: false,
        role: super_stt_registry_types::manifest::ModelRole::PostProcessor,
        provider: None,
    };
    let info = ModelInfoData::new(
        "scripted-pp",
        "github.com/super-stt/test",
        false,
        false,
        Duration::from_secs(1),
    );
    *daemon.post_processor.write().await = Some(LoadedModel {
        definition,
        instance: Box::new(ScriptedTranscribe {
            info,
            result: Ok(String::new()),
        }),
    });
}

/// One second of finite samples — passes `validate_audio` and survives
/// `process_audio` without resampling.
fn one_second_of_audio() -> Vec<f32> {
    vec![0.1_f32; 16000]
}

/// Happy path: the backend's text reaches the client in a `success` response.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_transcribe_returns_backend_text() {
    let daemon = test_daemon().await;
    seed_scripted_model(&daemon, true, Ok("hello world".to_string())).await;

    let resp = daemon
        .handle_transcribe(one_second_of_audio(), 16000, "c1".to_string(), None)
        .await;

    assert_eq!(resp.status, "success");
    assert_eq!(resp.transcription.as_deref(), Some("hello world"));
}

/// One-shot policy: a real backend failure is surfaced as an error response,
/// not masked as a successful empty transcription (audit Tier 1 #6). "No speech"
/// is an `Ok("")` from the backend and stays a success; a `Failed` dispatch is a
/// genuine error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_transcribe_reports_backend_failure_as_error() {
    let daemon = test_daemon().await;
    seed_scripted_model(&daemon, true, Err(())).await;

    let resp = daemon
        .handle_transcribe(one_second_of_audio(), 16000, "c1".to_string(), None)
        .await;

    assert_eq!(resp.status, "error");
    assert!(
        resp.message.as_deref().unwrap_or("").contains("failed"),
        "error message should name the failure: {:?}",
        resp.message
    );
}

/// With no model loaded, the one-shot path returns a coded error response —
/// `ErrorCode::ModelNotLoaded`, so an HTTP caller of the pre-captured
/// `audio_data` path gets the same `409 model_not_loaded` as the daemon-mic
/// paths (see docs/protocol/endpoints/v1/transcribe.md).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_transcribe_errors_when_no_model_loaded() {
    let daemon = test_daemon().await;

    let resp = daemon
        .handle_transcribe(one_second_of_audio(), 16000, "c1".to_string(), None)
        .await;

    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_code, Some(ErrorCode::ModelNotLoaded));
    assert_eq!(resp.message.as_deref(), Some("model_not_loaded"));
}

/// The whole point of the preflight: no capture, no beeps, and the user is told
/// in the field they are looking at.
// `start_paused` so the notice's key-release delay is virtual — this asserts
// what the preflight does, not how long the notice waits.
#[tokio::test(start_paused = true)]
async fn record_with_no_model_types_a_notice_in_write_mode() {
    // `test_daemon()` carries a notifier that always fails delivery (never
    // the real session bus — see its doc comment), so the config-default
    // `Auto` method falls through to typing here; that fallback is what this
    // test checks.
    let daemon = test_daemon().await;
    let (sim, buf) = crate::output::keyboard::Simulator::capture();
    let mut typer = crate::output::typer::Typer::new(sim);

    let resp = daemon
        .handle_record_internal(&mut typer, true, RecordingStopMode::ManualOnly, None)
        .await;

    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_code, Some(ErrorCode::ModelNotLoaded));
    assert_eq!(*buf.lock().unwrap(), "[Super STT: no model loaded]");
    // Capture must never have started.
    assert!(
        !*daemon.busy.read().await,
        "preflight must not leave the daemon busy"
    );
}

/// Without write mode there is no focused field to write into; the caller gets
/// the error response and nothing is typed.
#[tokio::test]
async fn record_with_no_model_types_nothing_without_write_mode() {
    // See the write-mode test above: `test_daemon()`'s notifier never reaches
    // the real session bus.
    let daemon = test_daemon().await;
    let (sim, buf) = crate::output::keyboard::Simulator::capture();
    let mut typer = crate::output::typer::Typer::new(sim);

    let resp = daemon
        .handle_record_internal(&mut typer, false, RecordingStopMode::ManualOnly, None)
        .await;

    assert_eq!(resp.error_code, Some(ErrorCode::ModelNotLoaded));
    assert_eq!(
        *buf.lock().unwrap(),
        "",
        "nothing may be typed outside write mode"
    );
}

/// The write-method test types the string the protocol doc promises, reports
/// the backend that typed it, and hands the simulator back to the cache. A
/// regression that skipped the cache return would rebuild the portal session —
/// three D-Bus round-trips and possibly an authorization prompt — per test.
#[tokio::test]
async fn write_method_test_types_the_documented_string_and_recaches() {
    let daemon = test_daemon().await;
    let (sim, buf) = crate::output::keyboard::Simulator::capture();
    *daemon.simulator.write().await = Some(sim);

    let resp = daemon
        .handle_command(make_request("test_write_method"))
        .await;

    assert_eq!(resp.status, "success");
    assert_eq!(*buf.lock().unwrap(), "Super STT input test 123");
    // The configured method (`auto` by default) and the backend that actually
    // typed are reported separately — the whole point of the endpoint.
    assert_eq!(resp.write_method.as_deref(), Some("auto"));
    assert!(
        resp.resolved_write_method.is_some(),
        "a client with `auto` configured has no other way to see the real backend"
    );
    assert!(
        daemon.simulator.read().await.is_some(),
        "a cacheable simulator must go back to the cache"
    );
}

/// A recording already owns the keyboard, so the test must refuse rather than
/// interleave its string into the user's dictation.
#[tokio::test]
async fn write_method_test_refuses_while_recording() {
    let daemon = test_daemon().await;
    let (sim, buf) = crate::output::keyboard::Simulator::capture();
    *daemon.simulator.write().await = Some(sim);
    *daemon.busy.write().await = true;

    let resp = daemon
        .handle_command(make_request("test_write_method"))
        .await;

    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_code, Some(ErrorCode::RecordingInProgress));
    assert_eq!(
        *buf.lock().unwrap(),
        "",
        "nothing may be typed while a recording holds the keyboard"
    );
}
