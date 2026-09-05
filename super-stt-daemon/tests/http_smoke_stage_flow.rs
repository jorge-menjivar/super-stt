// SPDX-License-Identifier: GPL-3.0-only
//! The card's whole flow, end to end, at every stage.
//!
//! Every other pipeline test drives one verb. This one walks the sequence a
//! user actually performs, against a live daemon and a backend that really
//! loads:
//!
//!   1. select a backend
//!   2. pick a model and a device
//!   3. Load
//!   4. change the device — **without** unloading
//!   5. Unload
//!   6. Load the same model again
//!
//! Every assertion runs at both positions, from one list. That is the point:
//! the stages are addressed by position so a client can learn one shape and
//! apply it anywhere, and for a long time they did not agree. Stage 1 reported
//! the model it had *loaded* while stage 2 reported the model it had
//! *selected*, and stage 1 had no `enabled` of its own — so its unload had to
//! erase the selection to keep a restart idle, and step 5 emptied the card that
//! step 6 needs. A regression in either direction fails this test at one
//! position and passes at the other, which is exactly what a single loop
//! catches.
//!
//! Needs the mock component built first — a placeholder entrypoint cannot load,
//! and steps 3 through 6 are all about what happens once something has:
//!   just build-mock-wasm-backend
//!
//! Uses `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_STT_AUTO_APPROVE=1` (no GUI), so it runs in the default flow.
#![cfg(feature = "wasm-backends")]

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http1::handshake;
use hyper::{Method, Request, StatusCode};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use super_stt_shared::daemon::http_client;
use tokio::net::UnixStream;
use tokio::time::sleep;

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");
const FIXTURE_SOURCE: &str = "github.com/super-stt/mock";

/// One position's half of the flow: which model it runs, since that is the only
/// thing that legitimately differs between stages.
struct StageUnderTest {
    position: u32,
    model: &'static str,
}

const STAGES: &[StageUnderTest] = &[
    StageUnderTest {
        position: 1,
        model: "echo-stt",
    },
    StageUnderTest {
        position: 2,
        model: "echo-clean",
    },
];

struct DaemonGuard {
    child: Child,
    cleanup_paths: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for p in &self.cleanup_paths {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

fn next_test_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static UNIQ: AtomicU64 = AtomicU64::new(0);
    UNIQ.fetch_add(1, Ordering::Relaxed)
}

/// Path to the prebuilt mock component (`just build-mock-wasm-backend`).
fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-backend/target/wasm32-wasip2/release/mock_wasm_backend.wasm",
    );
    p.exists().then_some(p)
}

/// One backend serving a model for each stage, both backed by the mock
/// component, both declaring `cpu` and `gpu`.
///
/// Declaring both devices is what makes step 4 a real device change. Whether
/// this host can *offer* the GPU is a separate question — the preference is
/// validated against the model's manifest, and a `gpu` that falls back to the
/// CPU is a case the daemon already reports honestly.
fn seed_mock_backend(data_home: &Path, component: &Path) {
    let backend_dir = data_home
        .join("super-stt")
        .join("backends")
        .join("mock-both-stages");
    std::fs::create_dir_all(&backend_dir).expect("create fixture backend dir");

    let toml = format!(
        r#"[backend]
source = "{FIXTURE_SOURCE}"
name = "Mock Both Stages"
version = "1.0.0"
kind = "wasm"
entrypoint = "mock.wasm"
contract = "v2"
id = "app.super-stt.mock"
description = "A backend that fills either stage."
license = "GPL-3.0-only"

[network]
allowed_hosts = []

[[models]]
name = "echo-stt"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu", "gpu"]

[[models]]
name = "echo-clean"
role = "post_processor"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu", "gpu"]
"#
    );
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    std::fs::copy(component, backend_dir.join("mock.wasm")).expect("stage mock component");
}

async fn start_daemon(component: &Path) -> (DaemonGuard, PathBuf, String) {
    let unique = format!("stt-stage-flow-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&config_home).expect("create test config dir");
    std::fs::create_dir_all(&data_home).expect("create test data dir");
    seed_mock_backend(&data_home, component);

    let child = Command::new(DAEMON_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-daemon");

    // Hand the child to the guard before the readiness loop: the timeout panic
    // below must still kill and reap the daemon, not leak it.
    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone(), config_home, data_home],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "stage-flow-probe", &["status"])
                .await
                .is_ok()
        {
            let auth = http_client::auth_request(http_socket.clone(), "stage-flow", &["settings"])
                .await
                .expect("auth_request for the settings scope");
            return (guard, http_socket, auth.session_token);
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

async fn request(
    socket: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let stream = UnixStream::connect(socket).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let payload = body
        .map(|b| serde_json::to_vec(&b).expect("serialize body"))
        .unwrap_or_default();
    let request = Request::builder()
        .method(method)
        .uri(format!("http://stt.local/v1{path}"))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(payload)))
        .expect("build request");

    let response = sender.send_request(request).await.expect("send request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("read body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn get(socket: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    request(socket, Method::GET, path, token, None).await
}

async fn post(
    socket: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    request(socket, Method::POST, path, token, Some(body)).await
}

async fn delete(socket: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    request(socket, Method::DELETE, path, token, None).await
}

/// The stage's model slot, as a card reads it.
async fn slot(socket: &PathBuf, stage: u32, token: &str) -> serde_json::Value {
    let (status, body) = get(socket, &format!("/pipeline/{stage}/model"), token).await;
    assert_eq!(status, StatusCode::OK, "GET stage {stage} model: {body}");
    body["model"].clone()
}

/// Whether the stage is switched on, from the stage itself.
async fn stage_enabled(socket: &PathBuf, stage: u32, token: &str) -> bool {
    let (status, body) = get(socket, &format!("/pipeline/{stage}"), token).await;
    assert_eq!(status, StatusCode::OK, "GET stage {stage}: {body}");
    body["stage"]["enabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("stage {stage} reports no `enabled`: {body}"))
}

/// Wait for the stage's model to come up. Stage 1 takes a load asynchronously
/// and answers before it finishes, so a card watches events; a test polls.
async fn await_loaded(socket: &PathBuf, stage: u32, token: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = serde_json::Value::Null;
    while Instant::now() < deadline {
        last = slot(socket, stage, token).await;
        if last["loaded"] == true {
            return last;
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("stage {stage} never came up within 30s; last slot: {last}");
}

/// The whole card flow, at every position.
#[tokio::test]
async fn a_card_can_select_load_move_unload_and_load_again_at_every_stage() {
    let Some(component) = mock_component() else {
        eprintln!("skipping: mock component not built (run `just build-mock-wasm-backend`)");
        return;
    };
    let (_guard, sock, token) = start_daemon(&component).await;

    for stage in STAGES {
        let (position, model) = (stage.position, stage.model);
        let stage_path = format!("/pipeline/{position}");
        let model_path = format!("/pipeline/{position}/model");
        let device_path = format!("/pipeline/{position}/model/{model}/device");

        // --- 1. select a backend -------------------------------------------
        let (status, body) = post(
            &sock,
            &stage_path,
            &token,
            serde_json::json!({ "source": FIXTURE_SOURCE }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "stage {position} select: {body}");

        assert!(
            !stage_enabled(&sock, position, &token).await,
            "stage {position}: selecting a backend must not switch the stage on"
        );
        let empty = slot(&sock, position, &token).await;
        assert_eq!(
            empty["model"],
            serde_json::Value::Null,
            "stage {position}: a backend is not a model selection"
        );
        assert_eq!(empty["loaded"], false);
        assert_eq!(empty["device"], serde_json::Value::Null);

        // --- 2. pick a model and a device ----------------------------------
        let (status, body) = get(&sock, &format!("{model_path}/list"), &token).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "stage {position} model list: {body}"
        );
        let listed: Vec<&str> = body["available_models"]
            .as_array()
            .expect("available_models is a list")
            .iter()
            .filter_map(|pair| pair[0].as_str())
            .collect();
        assert!(
            listed.contains(&model),
            "stage {position} does not list {model}: {listed:?}"
        );

        let (status, body) = get(&sock, &format!("{device_path}/list"), &token).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "stage {position} device list: {body}"
        );
        let offered: Vec<&str> = body["available_devices"]
            .as_array()
            .expect("available_devices is a list")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert!(
            offered.contains(&"cpu"),
            "stage {position}: every host offers the CPU: {offered:?}"
        );

        let (status, body) = post(
            &sock,
            &device_path,
            &token,
            serde_json::json!({ "device": "cpu" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "stage {position} set device: {body}"
        );
        assert_eq!(body["device"], "cpu");

        // A device is not a selection: choosing one before Load records the
        // preference and loads nothing, which is what lets the card's device
        // picker work before the button is pressed.
        let staged = slot(&sock, position, &token).await;
        assert_eq!(
            staged["model"],
            serde_json::Value::Null,
            "stage {position}: setting a device must not select a model"
        );
        assert_eq!(staged["loaded"], false);

        // --- 3. Load -------------------------------------------------------
        let (status, body) = post(
            &sock,
            &model_path,
            &token,
            serde_json::json!({ "model": model }),
        )
        .await;
        assert!(
            status.is_success(),
            "stage {position} load answered {status}: {body}"
        );
        let running = await_loaded(&sock, position, &token).await;
        assert_eq!(
            running["model"], model,
            "stage {position} is running {model}"
        );
        assert_eq!(
            running["device"]["preference"], "cpu",
            "stage {position}: the choice made in step 2"
        );
        // What it resolved to is the instance's own answer, not the preference:
        // this fixture is a WASM component with no local accelerator, so it
        // reports `remote`. The assertion that matters is that a loaded model
        // resolves to *something* — before a load there is nothing to report,
        // and reporting one anyway is the bug the field exists to avoid.
        assert!(
            running["device"]["resolved_accel"].is_string(),
            "stage {position}: a loaded model must say what it is running on: {running}"
        );
        assert!(
            stage_enabled(&sock, position, &token).await,
            "stage {position}: a running model switches the stage on"
        );

        // --- 4. change the device, without unloading ------------------------
        //
        // The daemon reloads the running model onto the new device in place.
        // It always could; the card used to make the user do it by hand as
        // unload, re-pick, load — which is the cycle that forced the app to
        // remember a selection the daemon had thrown away.
        let (status, body) = post(
            &sock,
            &device_path,
            &token,
            serde_json::json!({ "device": "gpu" }),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "stage {position} device change: {body}"
        );
        assert_eq!(body["device"], "gpu", "the new preference is recorded");

        let moved = slot(&sock, position, &token).await;
        assert_eq!(
            moved["loaded"], true,
            "stage {position}: changing the device must not leave the stage empty"
        );
        assert_eq!(moved["model"], model, "stage {position} still runs {model}");
        assert_eq!(
            moved["device"]["preference"], "gpu",
            "stage {position}: the picker's new value"
        );

        // --- 5. Unload ------------------------------------------------------
        let (status, body) = delete(&sock, &model_path, &token).await;
        assert!(
            status.is_success(),
            "stage {position} unload answered {status}: {body}"
        );

        assert!(
            !stage_enabled(&sock, position, &token).await,
            "stage {position}: an unload switches the stage off"
        );
        let idle = slot(&sock, position, &token).await;
        assert_eq!(
            idle["model"], model,
            "stage {position}: an unload must keep the selection — without it the \
             card empties and step 6 is a re-pick"
        );
        assert_eq!(idle["loaded"], false, "stage {position} is not running");
        assert_eq!(
            idle["device"]["preference"], "gpu",
            "stage {position}: the device chosen in step 4 outlives the unload too"
        );
        let (status, body) = get(&sock, &stage_path, &token).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["stage"]["source"], FIXTURE_SOURCE,
            "stage {position}: the backend stays selected, as it always did"
        );

        // --- 6. Load the same model again -----------------------------------
        let (status, body) = post(
            &sock,
            &model_path,
            &token,
            serde_json::json!({ "model": model }),
        )
        .await;
        assert!(
            status.is_success(),
            "stage {position} reload answered {status}: {body}"
        );
        let back = await_loaded(&sock, position, &token).await;
        assert_eq!(back["model"], model);
        assert_eq!(
            back["device"]["preference"], "gpu",
            "stage {position}: it comes back on the device it was left on"
        );
        assert!(stage_enabled(&sock, position, &token).await);
    }
}
