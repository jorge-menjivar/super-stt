// SPDX-License-Identifier: GPL-3.0-only
use super::*;
use crate::tts::daemon::types::test_daemon;
use super_voice_shared::tts::models::protocol::ErrorCode;

fn make_request(command: &str) -> DaemonRequest {
    DaemonRequest {
        command: command.to_string(),
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

/// Unloading mid-utterance stops the utterance. It used to be refused with
/// `speech_in_progress`, which left the audio playing after the user pressed
/// the button meant to make it stop.
#[tokio::test]
async fn unloading_the_model_stops_what_it_is_saying() {
    let daemon = test_daemon().await;
    seed_recording_model(&daemon, "m", "github.com/example/stop").await;
    let _claim = daemon.speech.claim_for_test("u-unload");

    let resp = daemon.handle_unload_active_model().await;

    assert_eq!(resp.status, "success", "{resp:?}");
    assert!(
        !daemon.is_busy(),
        "the utterance was stopped, not waited on"
    );
    assert!(
        daemon.model.read().await.is_none(),
        "and the model unloaded"
    );
}

/// Deselecting the backend is the same: the user's action wins.
#[tokio::test]
async fn deselecting_the_backend_stops_what_it_is_saying() {
    let daemon = test_daemon().await;
    seed_recording_model(&daemon, "m", "github.com/example/stop").await;
    let _claim = daemon.speech.claim_for_test("u-deselect");

    let resp = daemon.handle_clear_active_backend().await;

    assert_eq!(resp.status, "success", "{resp:?}");
    assert!(
        !daemon.is_busy(),
        "the utterance was stopped, not waited on"
    );
    assert!(
        daemon.model.read().await.is_none(),
        "and the model unloaded"
    );
}

/// Asking for the model that is already loaded changes nothing, so it must not
/// interrupt the model either.
#[tokio::test]
async fn switching_to_the_loaded_model_leaves_its_speech_alone() {
    let daemon = test_daemon().await;
    seed_recording_model(&daemon, "m", "github.com/example/stop").await;
    let _claim = daemon.speech.claim_for_test("u-noop");

    let resp = daemon
        .handle_set_model("m".to_string(), "github.com/example/stop".to_string())
        .await;

    assert_eq!(resp.status, "success", "{resp:?}");
    assert!(
        daemon.is_busy(),
        "a no-op switch does not stop the utterance"
    );
}

/// `handle_status` reports `busy` from the speech engine's in-flight slot, so a
/// client can tell whether a `speak` would interrupt something. The slot is the
/// only record of that fact — this pins that `/status` actually reads it rather
/// than a flag that could go stale.
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

    // An utterance in flight is what busy means now.
    {
        let _claim = daemon.speech.claim_for_test("u-status");
        let response = daemon.handle_status().await;
        assert_eq!(
            response.busy,
            Some(true),
            "busy=true must be surfaced by handle_status"
        );
    }

    // Recovery: releasing the slot flips the response field back.
    let response = daemon.handle_status().await;
    assert_eq!(response.busy, Some(false));
}

/// An omitted `source` resolves to the active backend — not to whichever
/// installed backend happens to serve the name. Two backends serve
/// `kokoro-tiny` here; the selected one must win regardless of their order in
/// the registry, since the backend that wins is persisted as the active one.
#[tokio::test]
async fn an_omitted_source_resolves_to_the_active_backend() {
    let daemon = test_daemon().await;
    // `kokoro` sorts before `zeta`, and is listed second — a scan-order
    // resolution would return `zeta` here.
    *daemon.backends.write().await = vec![
        fixture_backend_local("zeta", "github.com/other/zeta", "Zeta", "kokoro-tiny"),
        fixture_backend_local(
            "kokoro",
            "github.com/super-tts/kokoro",
            "Kokoro",
            "kokoro-tiny",
        ),
    ];
    *daemon.active_backend.write().await = Some("kokoro".to_string());

    assert_eq!(
        daemon.active_backend_source().await.as_deref(),
        Some("github.com/super-tts/kokoro"),
        "an omitted source must resolve to the selected backend, not the first scanned"
    );
}

/// With nothing selected there is no defensible guess, so the switch fails
/// instead of binding to an arbitrary backend and persisting that choice.
#[tokio::test]
async fn an_omitted_source_with_no_active_backend_is_an_error() {
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        "github.com/super-tts/kokoro",
        "Kokoro",
        "kokoro-tiny",
    )];
    assert!(daemon.active_backend.read().await.is_none());

    let mut request = make_request("set_model");
    request.data = Some(serde_json::json!({ "model": "kokoro-tiny" }));

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

/// `handle_list_backends` builds the catalog JSON (models, secrets,
/// options) from the discovered backends, and an in-memory config option
/// override is reflected in an option's effective `value`.
#[tokio::test]
async fn list_backends_catalog_and_option_override() {
    use crate::tts::daemon::test_fixtures::openai_backend;
    use crate::tts::tts_models::ModelDefinition;
    use std::time::Duration;

    let daemon = test_daemon().await;
    let source = "github.com/super-tts/openai";
    let backend = openai_backend(
        source,
        vec![ModelDefinition {
            name: "kokoro-1".to_string(),
            source: source.to_string(),
            is_multilingual: true,
            primary_language: "en".to_string(),
            supported_languages: vec!["en".to_string()],
            estimated_vram_bytes: 0,
            processing_interval: Duration::from_secs(1),
            supported_devices: vec![super_voice_registry_types::manifest::Device::None],
            realtime: false,
            provider: None,
            product: super_voice_registry_types::manifest::VoiceModel::new(
                super_voice_registry_types::manifest::ModelRole::Synthesis,
            ),
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
    assert_eq!(cat[0]["models"][0]["name"], "kokoro-1");
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
    use crate::tts::daemon::test_fixtures::openai_backend;

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
        let source = "github.com/super-tts/openai";
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
) -> crate::tts::tts_models::backends::DiscoveredBackend {
    fixture_backend_devices(
        dir_name,
        source,
        name,
        model_name,
        vec![super_voice_registry_types::manifest::Device::None],
    )
}

/// [`fixture_backend`], but serving a **local** model — one the online gate
/// must let through.
fn fixture_backend_local(
    dir_name: &str,
    source: &str,
    name: &str,
    model_name: &str,
) -> crate::tts::tts_models::backends::DiscoveredBackend {
    fixture_backend_devices(
        dir_name,
        source,
        name,
        model_name,
        vec![super_voice_registry_types::manifest::Device::Cpu],
    )
}

fn fixture_backend_devices(
    dir_name: &str,
    source: &str,
    name: &str,
    model_name: &str,
    supported_devices: Vec<super_voice_registry_types::manifest::Device>,
) -> crate::tts::tts_models::backends::DiscoveredBackend {
    use crate::tts::tts_models::ModelDefinition;
    use crate::tts::tts_models::backends::DiscoveredBackend;
    use std::time::Duration;

    DiscoveredBackend {
        dir: std::path::PathBuf::from("/tmp").join(dir_name),
        source: source.to_string(),
        id: None,
        name: name.to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        kind: "wasm".to_string(),
        entrypoint: format!("{dir_name}.wasm"),
        allowed_hosts: Vec::new(),
        secrets: Vec::new(),
        options: Vec::new(),
        capabilities: Default::default(),
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
            provider: None,
            product: super_voice_registry_types::manifest::VoiceModel::new(
                super_voice_registry_types::manifest::ModelRole::Synthesis,
            ),
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
    let resp = SuperTTSDaemon::handle_get_gpu_info().await;
    assert_eq!(resp.status, "success");
    assert!(resp.gpu_info.is_some(), "gpu_info must be present");
}

/// Real-hardware check: the daemon reports a non-empty, well-formed GPU
/// inventory. Ignored by default because CI runners have no GPU — run it on
/// a machine that does:
///   cargo test -p super-voice-daemon --all-features `gpu_info_reports_real_hardware` -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires a real GPU (NVML/sysfs); run with --ignored"]
async fn gpu_info_reports_real_hardware() {
    const VENDORS: [&str; 5] = ["nvidia", "amd", "intel", "apple", "unknown"];
    let resp = SuperTTSDaemon::handle_get_gpu_info().await;
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
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "kokoro-1")];

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
            .synthesis
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
        "github.com/super-tts/openai",
        "OpenAI",
        "kokoro-1",
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
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "kokoro-1")];

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
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "kokoro-1")];
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
            .synthesis
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
    let openai = "github.com/super-tts/openai";
    let mistral = "github.com/super-tts/mistral";
    *daemon.backends.write().await = vec![
        fixture_backend("openai", openai, "OpenAI", "kokoro-1"),
        fixture_backend("mistral", mistral, "Mistral", "piper-mini-latest"),
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
    assert_eq!(models[0].0, "kokoro-1");
    assert_eq!(models[0].1, openai);

    // Switch to Mistral → only its model.
    let _ = daemon.handle_set_active_backend(mistral.to_string()).await;
    let response = daemon.handle_list_models().await;
    let models = response.available_models.expect("available_models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].0, "piper-mini-latest");
    assert_eq!(models[0].1, mistral);
}

/// Trivial in-process `Synthesize` impl used to seed the `model` lock so
/// the always-unload semantics of `set_active_backend` can be observed
/// without touching real inference code. It produces no audio: these tests
/// are about model *lifecycle*, and what synthesis sounds like is
/// `tests/speak_pipeline.rs`'s subject.
struct MockModel {
    info: crate::tts::tts_models::synthesize::ModelInfoData,
    /// Every context this instance has been handed, in order.
    reconfigured: SeenContexts,
    /// The framework it reports running on.
    device: &'static str,
}
impl crate::tts::tts_models::synthesize::ModelInfo for MockModel {
    fn info(&self) -> &crate::tts::tts_models::synthesize::ModelInfoData {
        &self.info
    }
}
impl crate::tts::tts_models::synthesize::ModelState for MockModel {
    fn device(&self) -> String {
        self.device.to_string()
    }
}
#[async_trait::async_trait]
impl crate::tts::tts_models::synthesize::Synthesize for MockModel {
    async fn synthesize(
        &self,
        _request: &crate::tts::tts_models::v1::SynthesizeRequest<'_>,
        _sink: &mut (dyn crate::tts::tts_models::v1::SynthesisSink + Send),
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn reconfigure(&self, context: crate::tts::tts_models::synthesize::BackendContext) {
        self.reconfigured.lock().unwrap().push(context);
    }
}

/// The contexts a seeded [`MockModel`] has been handed, newest last.
type SeenContexts =
    std::sync::Arc<std::sync::Mutex<Vec<crate::tts::tts_models::synthesize::BackendContext>>>;

/// Place a loaded mock model in the daemon's `model` lock with the given
/// `(name, source)`. Used to verify the always-unload semantics in
/// `handle_set_active_backend`.
async fn seed_loaded_model(daemon: &SuperTTSDaemon, name: &str, source: &str) {
    seed_recording_model(daemon, name, source).await;
}

/// [`seed_loaded_model`], returning the handle on what the instance is handed
/// by [`Synthesize::reconfigure`](crate::tts::tts_models::synthesize::Synthesize::reconfigure).
///
/// Shared with the instance in the slot, so a test that finds its writes
/// recorded here has also established that the slot still holds *that*
/// instance — a reload would have replaced it.
async fn seed_recording_model(daemon: &SuperTTSDaemon, name: &str, source: &str) -> SeenContexts {
    seed_model_on(daemon, name, source, "cpu").await
}

/// [`seed_recording_model`], running on `device`.
async fn seed_model_on(
    daemon: &SuperTTSDaemon,
    name: &str,
    source: &str,
    device: &'static str,
) -> SeenContexts {
    use crate::tts::daemon::types::LoadedModel;
    use crate::tts::tts_models::ModelDefinition;
    use crate::tts::tts_models::synthesize::ModelInfoData;
    use std::time::Duration;

    let definition = ModelDefinition {
        name: name.to_string(),
        source: source.to_string(),
        is_multilingual: true,
        primary_language: "en".to_string(),
        supported_languages: vec!["en".to_string()],
        estimated_vram_bytes: 0,
        processing_interval: Duration::from_secs(1),
        supported_devices: vec![super_voice_registry_types::manifest::Device::None],
        realtime: false,
        provider: None,
        product: super_voice_registry_types::manifest::VoiceModel::new(
            super_voice_registry_types::manifest::ModelRole::Synthesis,
        ),
    };
    let info = ModelInfoData::new(name, source, true, true, Duration::from_secs(1));
    let reconfigured: SeenContexts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    crate::tts::daemon::types::put_model(
        &daemon.model,
        LoadedModel::new(
            definition,
            Box::new(MockModel {
                info,
                reconfigured: std::sync::Arc::clone(&reconfigured),
                device,
            }),
        ),
    )
    .await;
    reconfigured
}

/// [`fixture_backend`] plus one declared option, for the config-write paths.
fn backend_with_option(
    source: &str,
    option: &str,
) -> crate::tts::tts_models::backends::DiscoveredBackend {
    use super_voice_registry_types::manifest::{Opt, OptionType};

    let mut backend = fixture_backend("opts", source, "Opts", "m");
    backend.options = vec![Opt {
        name: option.to_string(),
        label: None,
        description: "an option".to_string(),
        r#type: Some(OptionType::String),
        default: None,
        choices: Vec::new(),
        min: None,
        max: None,
        step: None,
        required: false,
    }];
    backend
}

/// A value the transports cannot carry is refused, and not stored.
///
/// An option value is injected as an `x-voice-option-*` request header, and a
/// header holds no control character. Storing one and reporting success is the
/// worst outcome available: the settings UI then shows a value the backend is
/// not using, and every request the backend makes dies inside the transport
/// naming a header rather than the setting the user typed. Both halves are
/// asserted — the refusal, and that nothing was written behind it.
#[tokio::test]
async fn an_option_value_a_header_cannot_carry_is_refused() {
    use super_voice_shared::tts::models::protocol::ErrorCode;

    let daemon = test_daemon().await;
    let source = "github.com/x/opts";
    *daemon.backends.write().await = vec![backend_with_option(source, "style")];

    for bad in ["two\nlines", "a\tb", "x".repeat(4001).as_str()] {
        let resp = daemon
            .handle_set_backend_option(source.to_string(), "style".to_string(), bad.to_string())
            .await;

        assert_eq!(resp.status, "error", "{bad:?} should be refused");
        assert_eq!(
            resp.error_code,
            Some(ErrorCode::InvalidValue),
            "the code the endpoint already documents for a value it will not take"
        );
        assert!(
            daemon
                .config
                .read()
                .await
                .backend_option(source, "style")
                .is_none(),
            "a refused value must not be stored: {bad:?}"
        );
    }

    // The guard is about shape, not length alone — an ordinary value still
    // writes.
    let resp = daemon
        .handle_set_backend_option(source.to_string(), "style".to_string(), "terse".to_string())
        .await;
    assert_eq!(resp.status, "success");
}

/// An option no installed backend declares is left alone.
///
/// Nothing injects it, so there is no delivery to protect, and guarding it
/// here would refuse a write the HTTP layer has already refused with
/// `unknown_option` for a better reason.
#[tokio::test]
async fn the_shape_guard_only_speaks_for_options_a_backend_declares() {
    let daemon = test_daemon().await;
    let source = "github.com/x/opts";
    *daemon.backends.write().await = vec![backend_with_option(source, "style")];

    let resp = daemon
        .handle_set_backend_option(
            source.to_string(),
            "undeclared".to_string(),
            "two\nlines".to_string(),
        )
        .await;

    assert_eq!(resp.status, "success");
}

/// An option write for the *active* backend reaches the running model without
/// reloading it.
///
/// Both halves matter. The instance is handed the new value, so the setting is
/// live on the next request; and it is still the same instance afterwards, so
/// nothing unmapped and remapped the weights to deliver a header. The fixture
/// backend cannot actually be instantiated, so a reload would have emptied the
/// slot — finding a model there is what proves none happened.
#[tokio::test]
async fn an_option_write_reconfigures_the_running_model() {
    let daemon = test_daemon().await;
    let source = "github.com/x/opts";
    *daemon.backends.write().await = vec![backend_with_option(source, "temperature")];
    let seen = seed_recording_model(&daemon, "m", source).await;

    let resp = daemon
        .handle_set_backend_option(
            source.to_string(),
            "temperature".to_string(),
            "1.1".to_string(),
        )
        .await;

    assert_eq!(resp.status, "success");
    let msg = resp.message.unwrap_or_default();
    assert!(
        !msg.contains("kept the old value"),
        "the value reached the backend, so nothing should be warned about: {msg}"
    );

    let headers = {
        let contexts = seen.lock().unwrap();
        assert_eq!(contexts.len(), 1, "one write, one reconfigure");
        contexts[0].headers.clone()
    };
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "x-voice-option-temperature" && v == "1.1"),
        "the new value must be in the headers the instance now injects: {headers:?}"
    );

    assert!(
        daemon.model.read().await.is_some(),
        "the model must still be loaded — a reload would have failed and emptied the slot"
    );
}

/// Writing the value already stored is not a write.
///
/// The settings UI cannot help sending these: a slider commits on release, so
/// picking one up and putting it back is a write, and re-picking the value
/// already showing is another. Each one used to reload the model.
#[tokio::test]
async fn an_unchanged_option_write_does_nothing() {
    let daemon = test_daemon().await;
    let source = "github.com/x/opts";
    *daemon.backends.write().await = vec![backend_with_option(source, "temperature")];
    let seen = seed_recording_model(&daemon, "m", source).await;

    for _ in 0..2 {
        daemon
            .handle_set_backend_option(
                source.to_string(),
                "temperature".to_string(),
                "1.1".to_string(),
            )
            .await;
    }

    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the second write stores the same value and must not reach the backend again"
    );

    // And clearing an option that was never set is equally nothing.
    let daemon = test_daemon().await;
    *daemon.backends.write().await = vec![backend_with_option(source, "temperature")];
    let seen = seed_recording_model(&daemon, "m", source).await;
    let resp = daemon
        .handle_set_backend_option(source.to_string(), "temperature".to_string(), String::new())
        .await;
    assert_eq!(resp.status, "success");
    assert!(
        seen.lock().unwrap().is_empty(),
        "clearing an unset option changes nothing"
    );
}

/// The option write succeeds even when the new value cannot be delivered — it
/// is stored either way — but the response has to say so.
///
/// Here the loaded model names a backend that is not installed, so there is no
/// manifest to resolve a context against. Silence would leave the settings UI
/// showing a value the backend is not using, with nothing to say which.
#[tokio::test]
async fn set_backend_option_surfaces_an_undeliverable_value() {
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
        msg.contains("kept the old value"),
        "an undeliverable value should be surfaced, got: {msg}"
    );
}

/// Switching from backend A (with a loaded model) to backend B drops the
/// loaded model — the documented postcondition: after `set_active_backend`,
/// `/active_model` returns `null` until the user explicitly picks one.
#[tokio::test]
async fn set_active_backend_unloads_model_on_dir_change() {
    let daemon = test_daemon().await;
    let a = "github.com/super-tts/openai";
    let b = "github.com/super-tts/mistral";
    *daemon.backends.write().await = vec![
        fixture_backend("openai", a, "OpenAI", "kokoro-1"),
        fixture_backend("mistral", b, "Mistral", "piper-mini-latest"),
    ];

    // Start active on A with a model loaded.
    let _ = daemon.handle_set_active_backend(a.to_string()).await;
    seed_loaded_model(&daemon, "kokoro-1", a).await;
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
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "kokoro-1")];

    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    seed_loaded_model(&daemon, "kokoro-1", source).await;
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
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "kokoro-1")];

    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    assert!(daemon.model.read().await.is_none());
}

/// `unload_active_model` is a no-op when no model is loaded — success
/// with a clear message — and otherwise drops the model lock while
/// leaving `active_backend` selected so the user can pick another model.
#[tokio::test]
async fn unload_active_model_drops_model_keeps_backend() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "kokoro-1")];

    // No-op case: nothing to unload.
    let resp = daemon.handle_unload_active_model().await;
    assert_eq!(resp.status, "success");
    assert_eq!(resp.message.as_deref(), Some("No model to unload"));

    // Activate backend + seed a loaded model, then unload.
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    seed_loaded_model(&daemon, "kokoro-1", source).await;
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
            .synthesis
            .preferred_model
            .is_empty()
    );
}

/// The three new commands dispatch through `handle_command`, exercising the
/// shared protocol parse → core dispatch → handler chain end-to-end.
#[tokio::test]
async fn active_backend_commands_dispatch_through_handle_command() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await = vec![fixture_backend("openai", source, "OpenAI", "kokoro-1")];

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
    use crate::tts::daemon::events::Topic;

    let daemon = test_daemon().await;
    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);

    daemon.broadcast_model_active("piper-mini", "github.com/super-tts/mistral", "cuda");

    let (_topic, switched) = rx.recv_json().await.expect("model_switched event");
    assert_eq!(switched["status"], "model_switched");
    assert_eq!(switched["model_name"], "piper-mini");
    assert_eq!(switched["source"], "github.com/super-tts/mistral");

    let (_topic, ready) = rx.recv_json().await.expect("ready event");
    assert_eq!(ready["status"], "ready");
    assert_eq!(ready["model_loaded"], true);
    assert_eq!(ready["model_name"], "piper-mini");
}

/// A device belongs to a model, and setting it for a model that is not
/// loaded only records the choice — it does not error, and it loads nothing.
/// The model's next load picks it up. This is what lets the device picker
/// work before Load is pressed, rather than forcing a load on the wrong
/// device just to move the model off it.
#[tokio::test]
async fn set_model_device_when_not_loaded_only_records_the_preference() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_devices(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
        vec![
            super_voice_registry_types::manifest::Device::Cpu,
            super_voice_registry_types::manifest::Device::Gpu,
        ],
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    assert!(daemon.model.read().await.is_none());
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .model_device(source, "kokoro-82m"),
        None,
        "no choice of its own yet"
    );

    // The CPU, the one framework every machine offers, so the test holds on
    // a runner with no GPU.
    let response = daemon
        .handle_set_model_device("kokoro-82m".to_string(), "cpu".to_string())
        .await;
    assert_eq!(
        response.status, "success",
        "setting a device for an unloaded model must succeed; got error: {:?}",
        response.message,
    );
    assert_eq!(response.device.as_deref(), Some("cpu"));
    assert_eq!(
        response.resolved_accel,
        Some(Some("cpu".to_string())),
        "the CPU needs no load to resolve"
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
        daemon
            .config
            .read()
            .await
            .model_device(source, "kokoro-82m"),
        Some("cpu"),
        "the choice is the model's own now"
    );

    // The getter answers from the same state.
    let response = daemon
        .handle_get_model_device("kokoro-82m".to_string())
        .await;
    assert_eq!(response.status, "success");
    assert_eq!(response.device.as_deref(), Some("cpu"));
}

/// A build runs on the frameworks its `accel` lists: one listing the CPU
/// moves a model there at once, with no CPU build to download first.
#[tokio::test]
async fn a_build_listing_the_cpu_moves_there_without_an_install() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let folder = folder_with_builds(source, "kokoro-82m", "synthesis", r#"["vulkan", "cpu"]"#);
    seed_folder_backend(&daemon, source, folder.path()).await;

    let response = daemon
        .handle_set_model_device("kokoro-82m".to_string(), "cpu".to_string())
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .model_device(source, "kokoro-82m"),
        Some("cpu")
    );
}

/// A framework the backend publishes a build for, but the installed build
/// does not list, is refused with `build_not_installed` and nothing recorded:
/// the build is installed first, so a model is never pointed at one it does
/// not have. A Vulkan build that does not list the CPU has no CPU path.
#[tokio::test]
async fn a_framework_whose_build_is_not_installed_is_refused() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let folder = folder_with_builds(source, "kokoro-82m", "synthesis", r#""vulkan""#);
    seed_folder_backend(&daemon, source, folder.path()).await;

    let response = daemon
        .handle_set_model_device("kokoro-82m".to_string(), "cpu".to_string())
        .await;
    assert_eq!(response.status, "error");
    assert_eq!(
        response.error_code,
        Some(ErrorCode::BuildNotInstalled),
        "{:?}",
        response.message
    );
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .model_device(source, "kokoro-82m"),
        None,
        "nothing is recorded"
    );
}

/// A switch is from the framework the model is running on, not the one its
/// setting names. After the Vulkan build was replaced by the CPU build, a
/// model still running on Vulkan and switched to the CPU was announced as a
/// switch "from cpu to cpu": its setting resolves to the CPU, the one
/// framework the installed build runs.
#[tokio::test]
async fn a_switch_is_announced_from_where_the_model_runs() {
    use crate::tts::daemon::events::Topic;
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let folder = folder_with_builds(source, "kokoro-82m", "synthesis", r#""vulkan""#);
    // The CPU build replaced the Vulkan one the model was loaded from.
    std::fs::write(
        folder.path().join("installed.json"),
        r#"{"selected":{"target":"x86_64-unknown-linux-gnu","accel":"cpu"}}"#,
    )
    .expect("record");
    seed_folder_backend(&daemon, source, folder.path()).await;
    seed_model_on(&daemon, "kokoro-82m", source, "vulkan").await;
    daemon
        .model
        .write()
        .await
        .get_mut()
        .expect("seeded")
        .definition
        .supported_devices = vec![
        super_voice_registry_types::manifest::Device::Cpu,
        super_voice_registry_types::manifest::Device::Gpu,
    ];

    let mut rx = daemon.events.subscribe(Topic::DaemonStatusChanged);
    // The fixture has no binary, so the reload fails; the announcement is
    // what is being checked.
    let _ = daemon
        .handle_set_model_device("kokoro-82m".to_string(), "cpu".to_string())
        .await;

    let switching = loop {
        let (_topic, event) =
            tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv_json())
                .await
                .expect("a switching_device event")
                .expect("the event stream is open");
        if event["status"] == "switching_device" {
            break event;
        }
    };
    assert_eq!(switching["from_device"], "vulkan", "{switching}");
    assert_eq!(switching["target_device"], "cpu", "{switching}");
}

/// There is no global default: a model nobody chose a framework for runs on
/// the best one its installed build runs, which is the first offered, not
/// the CPU.
#[tokio::test]
async fn a_model_with_no_choice_runs_on_the_best_installed_framework() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let folder = folder_with_builds(source, "kokoro-82m", "synthesis", r#"["vulkan", "cpu"]"#);
    seed_folder_backend(&daemon, source, folder.path()).await;

    let response = daemon
        .handle_get_model_device("kokoro-82m".to_string())
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    let offered = response.available_devices.unwrap_or_default();
    assert_eq!(
        response.device.as_deref(),
        offered.first().map(String::as_str),
        "every framework offered here is installed, so the best one: {offered:?}"
    );
}

/// Select a synthesis backend whose folder is `dir`, serving `kokoro-82m`.
async fn seed_folder_backend(daemon: &SuperTTSDaemon, source: &str, dir: &std::path::Path) {
    let mut backend = fixture_backend_devices(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
        vec![
            super_voice_registry_types::manifest::Device::Cpu,
            super_voice_registry_types::manifest::Device::Gpu,
        ],
    );
    backend.dir = dir.to_path_buf();
    *daemon.backends.write().await = vec![backend];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
}

/// A backend folder whose `backend.toml` publishes a CPU build and a Vulkan
/// build of `model` declaring `vulkan_accel`, with that Vulkan build on disk.
fn folder_with_builds(
    source: &str,
    model: &str,
    role: &str,
    vulkan_accel: &str,
) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let manifest = format!(
        r#"[backend]
source = "{source}"
name = "Fixture"
version = "0.0.0"
kind = "subprocess"
entrypoint = "fixture"
contract = "v4"
product = "super-voice"
id = "app.test.fixture"
description = "Test backend."

[[assets.subprocess]]
file = "fixture-cpu.tar.gz"
target = "x86_64-unknown-linux-gnu"
accel = "cpu"

[[assets.subprocess]]
file = "fixture-cpu-mac.tar.gz"
target = "aarch64-apple-darwin"
accel = "cpu"

[[assets.subprocess]]
file = "fixture-vulkan.tar.gz"
target = "x86_64-unknown-linux-gnu"
accel = {vulkan_accel}

[[models]]
name = "{model}"
role = "{role}"
multilingual = false
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu", "gpu"]
"#
    );
    std::fs::write(dir.path().join("backend.toml"), manifest).expect("manifest");
    std::fs::write(
        dir.path().join("installed.json"),
        format!(r#"{{"selected":{{"target":"x86_64-unknown-linux-gnu","accel":{vulkan_accel}}}}}"#),
    )
    .expect("record");
    dir
}

/// An invalid device value is rejected before anything is looked up or
/// stored, with the documented `invalid_device` code. `none` parses as a
/// device but is a per-model manifest sentinel, not a preference a client
/// may set.
#[tokio::test]
async fn set_model_device_rejects_invalid_device() {
    let daemon = test_daemon().await;

    for device in ["xpu", "none"] {
        let response = daemon
            .handle_set_model_device("kokoro-82m".to_string(), device.to_string())
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

/// A device command names a model, not a `(source, model)` pair, so the model
/// resolves against the active backend — and with none selected there is
/// nothing to resolve against.
#[tokio::test]
async fn model_device_needs_an_active_backend() {
    let daemon = test_daemon().await;

    let response = daemon
        .handle_get_model_device("kokoro-82m".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidBackend));

    let response = daemon
        .handle_set_model_device("kokoro-82m".to_string(), "cpu".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidBackend));

    let response = daemon.handle_list_active_backend_devices().await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidBackend));
}

/// A model the active backend does not serve is `invalid_model` — the device
/// verbs never fall through to some other installed backend that happens to
/// serve the name.
#[tokio::test]
async fn model_device_refuses_a_model_the_backend_does_not_serve() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let response = daemon.handle_get_model_device("absent".to_string()).await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidModel));

    let response = daemon
        .handle_get_model_device("kokoro-82m".to_string())
        .await;
    assert_eq!(response.status, "success", "{:?}", response.message);
}

/// An online model runs remotely: it has no device to set, and reading one
/// reports the manifest's own `none` with nothing offered.
#[tokio::test]
async fn an_online_model_has_no_device() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/openai";
    *daemon.backends.write().await =
        vec![fixture_backend("openai", source, "OpenAI", "gpt-4o-tts")];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    seed_loaded_model(&daemon, "gpt-4o-tts", source).await;

    let response = daemon
        .handle_set_model_device("gpt-4o-tts".to_string(), "cuda".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidDevice));
    assert!(
        daemon.model.read().await.is_some(),
        "an online model must not be unloaded by a device request"
    );

    let response = daemon
        .handle_get_model_device("gpt-4o-tts".to_string())
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
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/piper";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "piper",
        source,
        "Piper",
        "piper-mini",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let response = daemon
        .handle_set_model_device("piper-mini".to_string(), "gpu".to_string())
        .await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidDevice));
    assert_eq!(
        daemon
            .config
            .read()
            .await
            .model_device(source, "piper-mini"),
        None
    );
}

/// Asking the loaded model for the device it is already on reloads nothing
/// — but still makes the device the model's own, since it may have been on
/// it only through the global default, and a later change to that default
/// would then move it.
#[tokio::test]
async fn set_model_device_on_the_device_in_use_reloads_nothing() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    // `seed_loaded_model` seeds an online definition; this model is local and
    // reports `cpu` from its instance.
    seed_loaded_model(&daemon, "kokoro-82m", source).await;
    daemon
        .model
        .write()
        .await
        .get_mut()
        .expect("seeded")
        .definition
        .supported_devices = vec![super_voice_registry_types::manifest::Device::Cpu];

    let response = daemon
        .handle_set_model_device("kokoro-82m".to_string(), "cpu".to_string())
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
        daemon
            .config
            .read()
            .await
            .model_device(source, "kokoro-82m"),
        Some("cpu")
    );
}

/// The list verbs answer from the same narrowing the device verb reports as
/// `available_devices`: per model, its own list; per backend, the union over
/// the models it serves — so an online model beside a local one contributes
/// nothing.
#[tokio::test]
async fn device_lists_answer_per_model_and_per_backend() {
    use super_voice_registry_types::manifest::Device;
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/mixed";
    let mut backend = fixture_backend_devices("mixed", source, "Mixed", "local", vec![Device::Cpu]);
    let mut online = backend.models[0].clone();
    online.name = "online".to_string();
    online.supported_devices = vec![Device::None];
    let mut accelerated = backend.models[0].clone();
    accelerated.name = "accelerated".to_string();
    accelerated.supported_devices = vec![Device::Cpu, Device::Gpu];
    backend.models.extend([online, accelerated]);
    *daemon.backends.write().await = vec![backend];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let response = daemon.handle_list_model_devices("local".to_string()).await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    assert_eq!(response.available_devices, Some(vec!["cpu".to_string()]));

    let response = daemon.handle_list_model_devices("online".to_string()).await;
    assert_eq!(
        response.available_devices,
        Some(Vec::new()),
        "a remote model runs on nothing local"
    );

    let response = daemon.handle_list_model_devices("absent".to_string()).await;
    assert_eq!(response.error_code, Some(ErrorCode::InvalidModel));

    // The backend's list is the union: the CPU everywhere, and the GPU
    // frameworks this host actually has.
    let response = daemon.handle_list_active_backend_devices().await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    // Best first, so the CPU, which every machine has, comes last; what is
    // before it is this machine's GPU frameworks.
    let devices = response.available_devices.expect("a list");
    assert_eq!(devices.last().map(String::as_str), Some("cpu"));
    assert!(
        devices
            .iter()
            .all(|d| ["cuda", "rocm", "metal", "vulkan", "cpu"].contains(&d.as_str())),
        "{devices:?}"
    );
}

/// The per-model device verbs dispatch through `handle_command`, exercising
/// the shared protocol parse → core dispatch → handler chain end-to-end. The
/// command names are wire contract: the HTTP layer calls them by name.
#[tokio::test]
async fn model_device_commands_dispatch_through_handle_command() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let mut request = make_request("set_model_device");
    request.data = Some(serde_json::json!({ "model": "kokoro-82m", "device": "cpu" }));
    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success", "{:?}", response.message);
    assert_eq!(response.device.as_deref(), Some("cpu"));

    let mut request = make_request("get_model_device");
    request.data = Some(serde_json::json!({ "model": "kokoro-82m" }));
    let response = daemon.handle_command(request).await;
    assert_eq!(response.status, "success");
    assert_eq!(response.device.as_deref(), Some("cpu"));

    let mut request = make_request("list_model_devices");
    request.data = Some(serde_json::json!({ "model": "kokoro-82m" }));
    let response = daemon.handle_command(request).await;
    assert_eq!(response.available_devices, Some(vec!["cpu".to_string()]));

    let response = daemon
        .handle_command(make_request("list_active_backend_devices"))
        .await;
    assert_eq!(response.available_devices, Some(vec!["cpu".to_string()]));
}

// --- The pipeline report ----------------------------------------------------

/// `get_pipeline` answers with a list, not a bare stage, even though super-tts
/// has exactly one. Clients walk the array and match on `stage`, so the shape
/// has to be the plural one from the start — the alternative is that adding a
/// text-normalizer stage in front of synthesis breaks every reader at once.
#[tokio::test]
async fn get_pipeline_reports_one_synthesis_stage() {
    use super_voice_shared::tts::models::protocol::{SYNTHESIS_STAGE, StageRole};

    let daemon = test_daemon().await;

    let response = daemon.handle_get_pipeline().await;
    assert_eq!(response.status, "success");
    let stages = response.pipeline.expect("a pipeline report");
    assert_eq!(stages.len(), 1, "super-tts synthesizes and nothing else");
    assert_eq!(stages[0].stage, SYNTHESIS_STAGE);
    assert_eq!(stages[0].role, StageRole::Synthesis);
}

/// An idle daemon reports the stage as empty and switched off; selecting a
/// backend fills it and switches it on.
///
/// `enabled` is derived from the backend selection because that is the only
/// on/off state super-tts persists — see `synthesis_stage`. This pins that
/// derivation: a stage that reported `enabled: true` while nothing was
/// selected would have a client render a working pipeline over a daemon that
/// refuses every `speak`.
#[tokio::test]
async fn a_stage_is_enabled_exactly_when_a_backend_fills_it() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";

    let stages = daemon
        .handle_get_pipeline()
        .await
        .pipeline
        .expect("a pipeline report");
    assert!(stages[0].source.is_none(), "nothing is selected yet");
    assert!(stages[0].name.is_none());
    assert!(!stages[0].enabled, "an empty stage is not switched on");

    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let stages = daemon
        .handle_get_pipeline()
        .await
        .pipeline
        .expect("a pipeline report");
    assert_eq!(stages[0].source.as_deref(), Some(source));
    assert_eq!(
        stages[0].name.as_deref(),
        Some("Kokoro"),
        "the stage names the backend's display name, not its repo id"
    );
    assert!(stages[0].enabled);

    // Clearing the backend idles the daemon, which is what switching the stage
    // off means here.
    let _ = daemon.handle_clear_active_backend().await;
    let stages = daemon
        .handle_get_pipeline()
        .await
        .pipeline
        .expect("a pipeline report");
    assert!(stages[0].source.is_none());
    assert!(!stages[0].enabled);
}

/// A stage reports its backend and never its model. The two have different
/// lifetimes — a backend selection cannot fail, a model load can — and the
/// model slot is `get_model`'s answer. This is the same claim the shared
/// crate's serialization test makes, pinned here against the state the daemon
/// actually builds the report from.
#[tokio::test]
async fn a_stage_names_a_backend_that_serves_no_loaded_model() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let stages = daemon
        .handle_get_pipeline()
        .await
        .pipeline
        .expect("a pipeline report");
    assert!(stages[0].enabled, "the backend fills the stage");
    assert!(
        daemon.model.read().await.is_none(),
        "and nothing is loaded in it"
    );
    let slot = daemon.handle_get_model().await.stage_model.expect("a slot");
    assert!(
        !slot.loaded,
        "the model slot is what says nothing is running"
    );
}

// --- Stage 1's model slot ---------------------------------------------------

/// `get_model` keeps every field it always answered with — including the error
/// envelope for the idle case, which `GET /active_model` reads as the normal
/// "no model loaded" state rather than a failure — and gains the stage-1 model
/// slot beside them.
///
/// The slot is attached to that idle envelope on purpose. "Selected but not
/// loaded" is only reachable while nothing is loaded, so omitting it there
/// would leave the very state the field exists for unreadable.
#[tokio::test]
async fn get_model_answers_the_stage_slot_even_when_nothing_is_loaded() {
    use super_voice_shared::tts::models::protocol::SYNTHESIS_STAGE;

    let daemon = test_daemon().await;

    let response = daemon.handle_get_model().await;
    assert_eq!(
        response.status, "error",
        "the idle envelope is unchanged; clients parse it"
    );
    assert!(response.current_model.is_none());
    let slot = response.stage_model.expect("the slot rides along anyway");
    assert_eq!(slot.stage, SYNTHESIS_STAGE);
    assert!(slot.model.is_none(), "nothing is selected");
    assert!(!slot.loaded);
    assert!(slot.device.is_none(), "and nothing has a device");
    assert!(slot.switch.is_none(), "nothing is being fetched");
}

/// A selection with nothing loaded reports the model, `loaded: false`, and the
/// device it *would* load on.
///
/// This is the state a card renders after an unload, and the one that makes
/// re-loading onto another device a single choice rather than a re-selection
/// the user has to make from scratch. Reading `model` off the loaded instance
/// instead of off the persisted preference is what once made this state
/// indistinguishable from "nothing was ever chosen".
#[tokio::test]
async fn the_stage_slot_reports_a_selection_that_is_not_loaded() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    // Selecting the backend clears any model preference, so record the
    // selection after it — the order a real switch uses.
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    daemon
        .config
        .write()
        .await
        .update_synthesis_model("kokoro-82m".to_string(), source.to_string());

    let slot = daemon.handle_get_model().await.stage_model.expect("a slot");
    assert_eq!(slot.model.as_deref(), Some("kokoro-82m"));
    assert!(!slot.loaded, "nothing was ever loaded");
    let device = slot.device.expect("a selected model has a device");
    assert_eq!(device.preference, "cpu");
    assert_eq!(
        device.resolved_accel.as_deref(),
        Some("cpu"),
        "`cpu` needs no resolution, so it is reported without a load"
    );
}

/// The loaded model's `(name, source)` is what flips `loaded`, not merely the
/// fact that *something* is loaded.
///
/// Mid-switch those differ, and a client that was just told which model the
/// stage points at has to be told whether *that* model is running — otherwise
/// a switch from one backend's model to another's reads as "already up" for
/// the whole of the load.
#[tokio::test]
async fn the_stage_slot_reports_a_loaded_selection() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;
    daemon
        .config
        .write()
        .await
        .update_synthesis_model("kokoro-82m".to_string(), source.to_string());
    seed_loaded_model(&daemon, "kokoro-82m", source).await;

    let response = daemon.handle_get_model().await;
    assert_eq!(response.status, "success");
    assert_eq!(response.current_model.as_deref(), Some("kokoro-82m"));
    let slot = response.stage_model.expect("a slot");
    assert!(slot.loaded, "the selection is the instance that is up");
    assert_eq!(
        slot.device.expect("a device").resolved_accel.as_deref(),
        Some("cpu"),
        "resolved from the running instance, not from the preference"
    );

    // A different model loaded under the same selection is not this selection.
    seed_loaded_model(&daemon, "some-other-model", source).await;
    let slot = daemon.handle_get_model().await.stage_model.expect("a slot");
    assert_eq!(slot.model.as_deref(), Some("kokoro-82m"));
    assert!(
        !slot.loaded,
        "`loaded` answers about the selected model, not about the slot being full"
    );
}

/// An in-flight download reaches the slot as `switch`, with the model being
/// fetched and the backend it is being fetched from.
///
/// The tracker records only the model name, so the source comes from the
/// active backend — which the switch path records *before* the download
/// starts, precisely so it is the target's backend and not the previous one.
/// Leaving it empty would make the target unidentifiable whenever two
/// installed backends serve the same model name.
#[tokio::test]
async fn an_in_flight_download_reaches_the_stage_slot() {
    use crate::tts::download_progress::DownloadProgressTracker;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];
    let _ = daemon.handle_set_active_backend(source.to_string()).await;

    let tracker = Arc::new(DownloadProgressTracker::new(
        "kokoro-82m".to_string(),
        crate::tts::download_progress::SynthesisSlot,
        3,
        Arc::new(AtomicBool::new(false)),
    ));
    // A tracker opens in its `verifying` phase; this test is about a download
    // in flight, so put it in the phase a download is actually in.
    tracker.mark_downloading();
    daemon
        .download_manager
        .start_download(tracker)
        .expect("nothing else is downloading");

    let slot = daemon.handle_get_model().await.stage_model.expect("a slot");
    let switch = slot.switch.expect("the download is in flight");
    assert_eq!(
        switch.phase, "downloading",
        "`phase` is the tracker's own status, renamed and not re-derived"
    );
    assert_eq!(switch.target.model, "kokoro-82m");
    assert_eq!(
        switch.target.source, source,
        "the backend being fetched from is the one the switch already selected"
    );
    assert_eq!(switch.download.total_files, 3);
}

// --- The language lists -----------------------------------------------------

/// A multilingual model offers `auto` plus exactly what its manifest declares,
/// and a monolingual one offers nothing at all.
///
/// The list is built from the rule `set_model_language` enforces rather than
/// from `supported_languages` directly, because the two differ in both
/// directions. A picker filled from the manifest would offer a monolingual
/// model's tags — every one of which the setter answers `unsupported_language`
/// to — and would hide `auto`, which the setter always takes.
#[tokio::test]
async fn list_model_languages_follows_what_the_setter_accepts() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-82m",
    )];

    let response = daemon
        .handle_list_model_languages(source.to_string(), "kokoro-82m".to_string())
        .await;
    assert_eq!(response.status, "success");
    assert_eq!(
        response.available_languages,
        Some(vec!["auto".to_string(), "en".to_string()]),
        "the manifest's tags, with `auto` in front"
    );

    // And every offered tag is one the setter takes.
    for tag in ["auto", "en"] {
        let response = daemon
            .handle_set_model_language(
                source.to_string(),
                "kokoro-82m".to_string(),
                tag.to_string(),
            )
            .await;
        assert_eq!(
            response.status, "success",
            "the list offered `{tag}` but the setter refused it"
        );
    }

    // A monolingual model has nothing to choose: an empty list, not an error,
    // so a client can hide the control without special-casing a status code.
    daemon.backends.write().await[0].models[0].is_multilingual = false;
    let response = daemon
        .handle_list_model_languages(source.to_string(), "kokoro-82m".to_string())
        .await;
    assert_eq!(response.status, "success");
    assert_eq!(response.available_languages, Some(Vec::new()));
}

/// The voice list is what the setter takes, and the setter takes only what the
/// list offered. A picker filled from anywhere else offers ids that fail on
/// click, which is the failure this pairing exists to prevent.
#[tokio::test]
async fn list_model_voices_follows_what_the_setter_accepts() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let mut backend = fixture_backend_local("kokoro", source, "Kokoro", "kokoro-82m");
    backend.models[0].product.voices = vec![
        super_voice_registry_types::manifest::VoiceEntry {
            id: "af_bella".to_string(),
            label: Some("Bella (American, female)".to_string()),
            language: None,
            tags: Vec::new(),
        },
        super_voice_registry_types::manifest::VoiceEntry {
            id: "am_adam".to_string(),
            label: Some("Adam (American, male)".to_string()),
            language: None,
            tags: Vec::new(),
        },
    ];
    *daemon.backends.write().await = vec![backend];

    let response = daemon
        .handle_list_model_voices(source.to_string(), "kokoro-82m".to_string())
        .await;
    assert_eq!(response.status, "success");
    let offered = response.available_voices.expect("a list of voices");
    assert_eq!(offered.len(), 2);
    // The label rides along: a picker showing `af_bella` where the manifest
    // wrote "Bella (American, female)" is the reason this is not a list of ids.
    assert_eq!(offered[0]["label"], "Bella (American, female)");
    assert_eq!(offered[0]["kind"], "preset");

    for voice in ["af_bella", "am_adam"] {
        let response = daemon
            .handle_set_model_voice(
                source.to_string(),
                "kokoro-82m".to_string(),
                voice.to_string(),
            )
            .await;
        assert_eq!(
            response.status, "success",
            "the list offered `{voice}` but the setter refused it"
        );
    }
}

/// A voice the model does not declare is refused rather than stored. Storing it
/// would look accepted and then fail on every utterance that followed, which is
/// the worst of the three possible answers.
#[tokio::test]
async fn setting_a_voice_the_model_does_not_have_is_refused() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let mut backend = fixture_backend_local("kokoro", source, "Kokoro", "kokoro-82m");
    backend.models[0].product.voices = vec![super_voice_registry_types::manifest::VoiceEntry {
        id: "af_bella".to_string(),
        label: Some("Bella".to_string()),
        language: None,
        tags: Vec::new(),
    }];
    *daemon.backends.write().await = vec![backend];

    let response = daemon
        .handle_set_model_voice(
            source.to_string(),
            "kokoro-82m".to_string(),
            "ryan".to_string(),
        )
        .await;
    assert_eq!(response.status, "error");

    // A cloned id is refused too: this model's `voice_kinds` is preset-only, so
    // the shape is wrong whatever the library holds.
    let response = daemon
        .handle_set_model_voice(
            source.to_string(),
            "kokoro-82m".to_string(),
            "voice:0b2f8c1e-1111-2222-3333-444455556666".to_string(),
        )
        .await;
    assert_eq!(response.status, "error");
}

/// Setting, reading and clearing round-trip, and the block says which of the
/// two sources answered.
#[tokio::test]
async fn a_stored_voice_survives_until_it_is_cleared() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let mut backend = fixture_backend_local("kokoro", source, "Kokoro", "kokoro-82m");
    backend.models[0].product.voices = vec![super_voice_registry_types::manifest::VoiceEntry {
        id: "af_bella".to_string(),
        label: Some("Bella".to_string()),
        language: None,
        tags: Vec::new(),
    }];
    backend.models[0].product.default_voice = Some("af_bella".to_string());
    *daemon.backends.write().await = vec![backend];

    let block = |resp: super_voice_shared::tts::models::protocol::DaemonResponse| {
        resp.voice.expect("a voice block")
    };

    // Nothing stored: the manifest's default answers, and says so.
    let before = block(
        daemon
            .handle_get_model_voice(source.to_string(), "kokoro-82m".to_string())
            .await,
    );
    assert_eq!(before["effective"], "af_bella");
    assert_eq!(before["source"], "default");
    assert!(before["override"].is_null());

    let after = block(
        daemon
            .handle_set_model_voice(
                source.to_string(),
                "kokoro-82m".to_string(),
                "af_bella".to_string(),
            )
            .await,
    );
    assert_eq!(after["source"], "override");
    assert_eq!(after["override"], "af_bella");

    let cleared = block(
        daemon
            .handle_clear_model_voice(source.to_string(), "kokoro-82m".to_string())
            .await,
    );
    assert_eq!(cleared["source"], "default");
    assert!(cleared["override"].is_null());
}

/// A model with no `default_voice` and nothing stored reports no voice at all,
/// rather than inventing one. That `null` is what a client renders as "choose a
/// voice" — and the state in which speaking is refused, which is what the user
/// hit before this control existed.
#[tokio::test]
async fn a_model_with_no_default_reports_no_voice() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    let mut backend = fixture_backend_local("kokoro", source, "Kokoro", "kokoro-82m");
    backend.models[0].product.voice_kinds =
        vec![super_voice_registry_types::manifest::VoiceKind::Cloned];
    *daemon.backends.write().await = vec![backend];

    let response = daemon
        .handle_get_model_voice(source.to_string(), "kokoro-82m".to_string())
        .await;
    let block = response.voice.expect("a voice block");
    assert!(block["effective"].is_null());
    assert!(block["default"].is_null());
    assert_eq!(block["kinds"][0], "cloned");
}

/// Listing the voices of a model no backend serves is `unknown_model`, the same
/// classified 404 its language sibling answers with.
#[tokio::test]
async fn list_model_voices_refuses_an_unknown_model() {
    let daemon = test_daemon().await;

    let response = daemon
        .handle_list_model_voices("github.com/x/absent".to_string(), "nope".to_string())
        .await;
    assert_eq!(response.status, "error");
    assert_eq!(response.error_code, Some(ErrorCode::InvalidModel));
    assert!(response.available_voices.is_none());
}

/// Listing the languages of a model no backend serves is `unknown_model`, the
/// same classified 404 its sibling verbs answer with — an empty list would say
/// "this model has no languages", which is a different and wrong claim.
#[tokio::test]
async fn list_model_languages_refuses_an_unknown_model() {
    let daemon = test_daemon().await;

    let response = daemon
        .handle_list_model_languages("github.com/x/absent".to_string(), "nope".to_string())
        .await;
    assert_eq!(response.status, "error");
    assert_eq!(response.error_code, Some(ErrorCode::InvalidModel));
    assert!(response.available_languages.is_none());
}

/// An idle unload frees the model and keeps it selected, so the next request
/// loads it again: an explicit unload clears the selection instead.
#[tokio::test]
async fn idle_unload_frees_the_model_and_keeps_it_selected() {
    use std::time::Duration;
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/openai";
    seed_loaded_model(&daemon, "kokoro-1", source).await;
    daemon
        .config
        .write()
        .await
        .update_synthesis_model("kokoro-1".into(), source.into());

    daemon.model.backdate(Duration::from_secs(10));
    assert!(!daemon.unload_if_idle(Duration::from_secs(30)).await);
    assert!(daemon.model.read().await.is_some());

    daemon.model.backdate(Duration::from_secs(60));
    assert!(daemon.unload_if_idle(Duration::from_secs(30)).await);

    assert!(daemon.model.read().await.is_none());
    assert_eq!(
        daemon.config.read().await.synthesis.preferred_model,
        "kokoro-1"
    );
}

/// Text that cannot be spoken is refused before a model is loaded for it, so
/// with idle unload on an empty request does not load a model only to fail.
#[tokio::test]
async fn speak_refuses_text_before_loading_a_model() {
    use std::time::Duration;
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/openai";
    {
        let mut config = daemon.config.write().await;
        config.update_synthesis_model("kokoro-1".into(), source.into());
        config.idle_unload.after_secs = 30;
    }
    // A load on demand waits for this one, so a refusal that returns at once
    // never tried one.
    let _load = daemon.model.begin_load().await.expect("a turn");

    for text in [
        "",
        "   ",
        &"a".repeat(crate::tts::daemon::speech::MAX_TEXT_CHARS + 1),
    ] {
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            daemon.handle_speak(text.to_string()),
        )
        .await
        .expect("refused without waiting for a load");
        assert_eq!(response.error_code, Some(ErrorCode::InvalidValue));
    }
}

/// With idle unload on, the model a request loads on demand is the selected
/// one, so the voices page can describe it while it is out of memory. With
/// it off, a model not in memory is not loaded by a request.
#[tokio::test]
async fn the_selected_model_is_loaded_on_demand_only_with_idle_unload_on() {
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-tiny",
    )];
    daemon
        .config
        .write()
        .await
        .update_synthesis_model("kokoro-tiny".into(), source.into());

    assert!(daemon.model_loaded_on_demand().await.is_none());

    daemon.config.write().await.idle_unload.after_secs = 300;
    let definition = daemon
        .model_loaded_on_demand()
        .await
        .expect("the selected model");
    assert_eq!(definition.name, "kokoro-tiny");
    assert_eq!(definition.source, source);
}

/// An unload during a load waits for it, then clears the selection, rather
/// than finding the slot empty and leaving the load to put the model back.
#[tokio::test]
async fn an_unload_during_a_load_waits_for_it_and_clears_the_selection() {
    use std::time::Duration;
    let daemon = test_daemon().await;
    daemon
        .config
        .write()
        .await
        .update_synthesis_model("kokoro-1".into(), "github.com/super-tts/openai".into());
    let slot = std::sync::Arc::clone(&daemon.model);
    let load = slot.begin_load().await.expect("a turn");

    let unload = tokio::spawn({
        let daemon = daemon.clone();
        async move { daemon.handle_unload_active_model().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!unload.is_finished(), "answered before the load finished");

    drop(load);
    let response = unload.await.expect("no panic");
    assert_eq!(response.status, "success");
    assert!(
        daemon
            .config
            .read()
            .await
            .synthesis
            .preferred_model
            .is_empty()
    );
}

/// A speak whose model fails to load on demand notifies that, not "no model
/// loaded": a model is selected, and loading another would not help.
#[tokio::test]
async fn a_speak_whose_model_fails_to_load_says_so() {
    use std::time::Duration;
    let daemon = test_daemon().await;
    let source = "github.com/super-tts/kokoro";
    // No component on disk, so its load fails.
    *daemon.backends.write().await = vec![fixture_backend_local(
        "kokoro",
        source,
        "Kokoro",
        "kokoro-tiny",
    )];
    {
        let mut config = daemon.config.write().await;
        config.update_synthesis_model("kokoro-tiny".into(), source.into());
        config.idle_unload.after_secs = 30;
    }
    let (notifier, sent) = crate::tts::output::notification::Notifier::fake(false);
    *daemon.notifier.lock().await = notifier;

    let response = tokio::time::timeout(
        Duration::from_secs(10),
        daemon.handle_speak("hello".to_string()),
    )
    .await
    .expect("the load failed rather than hung");

    assert_eq!(response.error_code, Some(ErrorCode::ModelNotLoaded));
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "Could not load the model");
}
