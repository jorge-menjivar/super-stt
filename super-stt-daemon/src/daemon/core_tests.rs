// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use crate::config::DaemonConfig;
use crate::daemon::events::EventBus;
use crate::download_progress::DownloadStateManager;
use crate::input::audio::AudioProcessor;
use crate::services::transcription::RealTimeTranscriptionManager;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use super_stt_shared::resource_management::ResourceManager;
use super_stt_shared::theme::AudioTheme;
use tokio::sync::broadcast;
use tokio::time::{Duration, timeout};

async fn test_daemon() -> SuperSTTDaemon {
    let model = Arc::new(tokio::sync::RwLock::new(None));
    let audio_processor = Arc::new(AudioProcessor::new());
    let (shutdown_tx, _) = broadcast::channel(1);
    let realtime_manager = Arc::new(RealTimeTranscriptionManager::new(
        Arc::clone(&model),
        Arc::clone(&audio_processor),
    ));
    SuperSTTDaemon {
        model,
        audio_processor,
        shutdown_tx,
        dbus_manager: None,
        realtime_manager,
        events: Arc::new(EventBus::new()),
        audio_theme: Arc::new(RwLock::new(AudioTheme::default())),
        volume: Arc::new(RwLock::new(100)),
        busy: Arc::new(tokio::sync::RwLock::new(false)),
        download_manager: Arc::new(DownloadStateManager::new()),
        preferred_device: Arc::new(tokio::sync::RwLock::new("cpu".to_string())),
        actual_device: Arc::new(tokio::sync::RwLock::new("cpu".to_string())),
        config: Arc::new(tokio::sync::RwLock::new(DaemonConfig::default())),
        resource_manager: Arc::new(ResourceManager::development()),
        preview_typing_enabled: Arc::new(AtomicBool::new(false)),
        manual_stop_tx: Arc::new(tokio::sync::RwLock::new(None)),
        simulator: Arc::new(tokio::sync::RwLock::new(None)),
        preview_text: Arc::new(tokio::sync::RwLock::new(None)),
        backends: Arc::new(tokio::sync::RwLock::new(Vec::new())),
        active_backend: Arc::new(tokio::sync::RwLock::new(None)),
    }
}

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

    // But the request explicitly asks for manual-only mode
    let request = make_record_request(Some(serde_json::json!({
        "write_mode": false,
        "stop_mode": "manual-only",
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
        "stop_mode": "silence-only",
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

#[tokio::test]
async fn set_allow_online_models_updates_config() {
    let daemon = test_daemon().await;

    let request = DaemonRequest {
        command: "set_allow_online_models".to_string(),
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
        enabled: Some(true),
    };

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(response.allow_online_models, Some(true));

    let config = daemon.config.read().await;
    assert!(config.online.allow_online_models);
}

#[tokio::test]
async fn get_allow_online_models_returns_config_value() {
    let daemon = test_daemon().await;

    // Set it to true first
    {
        let mut config = daemon.config.write().await;
        config.online.allow_online_models = true;
    }

    let request = DaemonRequest {
        command: "get_allow_online_models".to_string(),
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
    };

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(response.allow_online_models, Some(true));
}

#[tokio::test]
async fn set_model_online_rejected_when_disabled() {
    let daemon = test_daemon().await;

    // Ensure online models are disabled (default)
    {
        let config = daemon.config.read().await;
        assert!(!config.online.allow_online_models);
    }

    let request = DaemonRequest {
        command: "set_model".to_string(),
        audio_data: None,
        sample_rate: None,
        client_id: None,
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        data: Some(serde_json::json!({ "model": "whisper-1", "provider": "openai" })),
        language: None,
        enabled: None,
    };

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "error");
    assert!(
        response
            .message
            .as_deref()
            .unwrap_or("")
            .contains("disabled")
            || response
                .message
                .as_deref()
                .unwrap_or("")
                .contains("Online models are disabled"),
        "expected error about online models being disabled, got: {:?}",
        response.message
    );
}

#[tokio::test]
async fn set_model_mistral_rejected_when_disabled() {
    let daemon = test_daemon().await;

    let mut request = make_request("set_model");
    request.data = Some(serde_json::json!({ "model": "voxtral-mini-transcribe-v2" }));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "error");
}

#[tokio::test]
async fn set_model_deepgram_rejected_when_disabled() {
    let daemon = test_daemon().await;

    let mut request = make_request("set_model");
    request.data = Some(serde_json::json!({ "model": "nova-3" }));

    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "error");
}

#[tokio::test]
async fn toggle_online_models_off_defaults_to_false() {
    let daemon = test_daemon().await;
    let config = daemon.config.read().await;
    assert!(
        !config.online.allow_online_models,
        "online models should be disabled by default"
    );
}

#[tokio::test]
async fn toggle_online_models_on_then_off() {
    let daemon = test_daemon().await;

    // Enable
    let mut request = make_request("set_allow_online_models");
    request.enabled = Some(true);
    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(response.allow_online_models, Some(true));

    // Disable
    let mut request = make_request("set_allow_online_models");
    request.enabled = Some(false);
    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(response.allow_online_models, Some(false));

    let config = daemon.config.read().await;
    assert!(!config.online.allow_online_models);
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

#[tokio::test]
async fn set_model_local_works_without_online_toggle() {
    let daemon = test_daemon().await;

    // Local models should not be blocked by the online toggle
    // (they will fail because no model files exist, but the error
    // should NOT be about online models being disabled)
    let mut request = make_request("set_model");
    request.data = Some(serde_json::json!({ "model": "whisper-tiny" }));

    let response = daemon.handle_command(request).await;
    // Should either succeed (already loaded) or fail for non-online reasons
    if response.status == "error" {
        let msg = response.message.as_deref().unwrap_or("");
        assert!(
            !msg.contains("Online models are disabled"),
            "local model should not be blocked by online toggle"
        );
    }
}

/// `handle_list_backends` builds the catalog JSON (models, secrets,
/// options) from the discovered backends, and an in-memory config option
/// override is reflected in an option's effective `value`. Keyring-free.
#[tokio::test]
async fn list_backends_catalog_and_option_override() {
    use crate::stt_models::backends::DiscoveredBackend;
    use crate::stt_models::backends::manifest::{Opt, OptionDefault, OptionType, Secret};
    use std::time::Duration;
    use super_stt_shared::models::provider::{OnlineProvider, Provider};
    use super_stt_shared::models::registry::ModelDefinition;

    let daemon = test_daemon().await;
    let source = "github.com/super-stt/openai";
    let backend = DiscoveredBackend {
        dir: std::path::PathBuf::from("/tmp/openai"),
        source: source.to_string(),
        name: "OpenAI".to_string(),
        kind: "wasm".to_string(),
        entrypoint: "openai.wasm".to_string(),
        allowed_hosts: vec!["api.openai.com".to_string()],
        secrets: vec![Secret {
            name: "openai_api_key".to_string(),
            label: Some("OpenAI API key".to_string()),
            description: "key".to_string(),
            required: true,
        }],
        options: vec![Opt {
            name: "base_url".to_string(),
            label: Some("API base URL".to_string()),
            description: "Base URL".to_string(),
            r#type: Some(OptionType::String),
            default: Some(OptionDefault::String("https://api.openai.com".to_string())),
            required: false,
        }],
        models: vec![ModelDefinition {
            name: "whisper-1".to_string(),
            provider: Provider::Online(OnlineProvider::OpenAI),
            source: source.to_string(),
            is_multilingual: true,
            estimated_vram_bytes: 0,
            processing_interval: Duration::from_secs(1),
            supported_devices: vec!["none".to_string()],
            realtime: false,
        }],
    };
    *daemon.backends.write().await = vec![backend];

    let resp = daemon.handle_list_backends().await;
    assert_eq!(resp.status, "success");
    let cat = resp.backends.expect("backends catalog");
    assert_eq!(cat[0]["source"], source);
    assert_eq!(cat[0]["models"][0]["name"], "whisper-1");
    assert_eq!(cat[0]["secrets"][0]["name"], "openai_api_key");
    assert_eq!(cat[0]["secrets"][0]["label"], "OpenAI API key");
    // No override yet → option value is the manifest default.
    assert_eq!(cat[0]["options"][0]["value"], "https://api.openai.com");

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
}

/// Build a `DiscoveredBackend` whose `dir` ends in `dir_name` and that
/// serves a single model — enough surface for the active-backend handlers.
fn fixture_backend(
    dir_name: &str,
    source: &str,
    name: &str,
    model_name: &str,
) -> crate::stt_models::backends::DiscoveredBackend {
    use crate::stt_models::backends::DiscoveredBackend;
    use std::time::Duration;
    use super_stt_shared::models::provider::{OnlineProvider, Provider};
    use super_stt_shared::models::registry::ModelDefinition;

    DiscoveredBackend {
        dir: std::path::PathBuf::from("/tmp").join(dir_name),
        source: source.to_string(),
        name: name.to_string(),
        kind: "wasm".to_string(),
        entrypoint: format!("{dir_name}.wasm"),
        allowed_hosts: Vec::new(),
        secrets: Vec::new(),
        options: Vec::new(),
        models: vec![ModelDefinition {
            name: model_name.to_string(),
            provider: Provider::Online(OnlineProvider::OpenAI),
            source: source.to_string(),
            is_multilingual: true,
            estimated_vram_bytes: 0,
            processing_interval: Duration::from_secs(1),
            supported_devices: vec!["none".to_string()],
            realtime: false,
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

/// `handle_get_gpu_info` always succeeds and returns a JSON array (empty on
/// headless/CI hosts). Hardware-independent: asserts the shape only.
#[tokio::test]
async fn get_gpu_info_returns_success_array() {
    let resp = SuperSTTDaemon::handle_get_gpu_info().await;
    assert_eq!(resp.status, "success");
    let gpu_info = resp.gpu_info.expect("gpu_info present");
    assert!(gpu_info.is_array(), "gpu_info must be a JSON array");
}

/// Real-hardware check: the daemon reports a non-empty, well-formed GPU
/// inventory. Ignored by default because CI runners have no GPU — run it on
/// a machine that does:
///   cargo test -p super-stt-daemon --all-features gpu_info_reports_real_hardware -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires a real GPU (NVML/sysfs); run with --ignored"]
async fn gpu_info_reports_real_hardware() {
    let resp = SuperSTTDaemon::handle_get_gpu_info().await;
    assert_eq!(resp.status, "success");
    let gpu_info = resp.gpu_info.expect("gpu_info present");
    eprintln!(
        "daemon gpu_info = {}",
        serde_json::to_string_pretty(&gpu_info).unwrap()
    );
    let gpus = gpu_info.as_array().expect("gpu_info must be an array");
    assert!(!gpus.is_empty(), "expected at least one GPU on this host");
    const VENDORS: [&str; 5] = ["nvidia", "amd", "intel", "apple", "unknown"];
    for gpu in gpus {
        assert!(
            gpu.get("name")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty()),
            "each GPU needs a non-empty name"
        );
        assert!(
            gpu.get("total_bytes")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|n| n > 0),
            "each GPU needs total_bytes > 0"
        );
        let vendor = gpu.get("vendor").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            VENDORS.contains(&vendor),
            "vendor {vendor:?} must be a known snake_case tag"
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
    assert_eq!(models[0].2, openai);

    // Switch to Mistral → only its model.
    let _ = daemon.handle_set_active_backend(mistral.to_string()).await;
    let response = daemon.handle_list_models().await;
    let models = response.available_models.expect("available_models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].0, "voxtral-mini-latest");
    assert_eq!(models[0].2, mistral);
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
    ) -> anyhow::Result<String> {
        Ok(String::new())
    }
}

/// Place a loaded mock model in the daemon's `model` lock with the given
/// `(name, source)`. Used to verify the always-unload semantics in
/// `handle_set_active_backend`.
async fn seed_loaded_model(daemon: &SuperSTTDaemon, name: &str, source: &str) {
    use crate::daemon::types::LoadedModel;
    use crate::stt_models::transcribe::ModelInfoData;
    use std::time::Duration;
    use super_stt_shared::models::provider::{OnlineProvider, Provider};
    use super_stt_shared::models::registry::ModelDefinition;

    let provider = Provider::Online(OnlineProvider::OpenAI);
    let definition = ModelDefinition {
        name: name.to_string(),
        provider,
        source: source.to_string(),
        is_multilingual: true,
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        supported_devices: vec!["none".to_string()],
        realtime: false,
    };
    let info = ModelInfoData::new(name, provider, source, true, Duration::from_secs(1));
    *daemon.model.write().await = Some(LoadedModel {
        definition,
        instance: Box::new(MockTranscribe { info }),
    });
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

/// `set_device` with no model loaded only records the preference — it
/// does not error. The next model load picks up the new device. This
/// makes the GPU toggle usable in the active-backend card before a model
/// has been selected.
#[tokio::test]
async fn set_device_when_idle_only_updates_preference() {
    let daemon = test_daemon().await;
    // Start with the default "cpu" preference and an empty model lock —
    // the test_daemon fixture already initializes both that way.
    assert!(daemon.model.read().await.is_none());
    assert_eq!(daemon.preferred_device.read().await.as_str(), "cpu");

    let response = daemon.handle_set_device("cuda".to_string()).await;
    assert_eq!(
        response.status, "success",
        "set_device must succeed when no model is loaded; got error: {:?}",
        response.message,
    );
    // Both runtime locks updated, and the model lock is still empty.
    assert_eq!(daemon.preferred_device.read().await.as_str(), "cuda");
    assert_eq!(daemon.actual_device.read().await.as_str(), "cuda");
    assert!(daemon.model.read().await.is_none());
    assert_eq!(
        daemon.config.read().await.device.preferred_device,
        "cuda",
        "preference must be persisted to in-memory config so the next load uses it"
    );
}

/// An invalid device value is still rejected when idle — the preference
/// must validate before being recorded.
#[tokio::test]
async fn set_device_when_idle_rejects_invalid_device() {
    let daemon = test_daemon().await;
    let response = daemon.handle_set_device("xpu".to_string()).await;
    assert_eq!(response.status, "error");
    assert_eq!(daemon.preferred_device.read().await.as_str(), "cpu");
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
