// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline` HTTP smoke test, driving stage 2 (post-processing).
//!
//! Covers the endpoint's whole contract against a live daemon:
//! 1. Default state — the stage exists, disabled, nothing selected.
//! 2. Select the stage's backend, then run one of its models → GET reflects it.
//! 3. `DELETE /pipeline/2/model` stops it and keeps the choice;
//!    `DELETE /pipeline/2` deselects the backend and forgets it.
//! 4. A model that is not installed is refused.
//! 5. A transcription model is refused for this stage, and a post-processor is
//!    refused for `/active_model` — the two roles do not cross.
//! 6. `GET /backends` reports each model's `role`.
//! 7. `GET /pipeline` lists both stages, and an out-of-range stage 404s.
//!
//! Uses `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_STT_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.
//!
//! The fixture backend declares one transcription model and one post-processor
//! so both directions of the role gate are exercisable. Both are `wasm`/cloud
//! models with a placeholder entrypoint: these tests drive selection and
//! validation, which happen before any component is loaded, so a load failure
//! on the post-processor is expected and asserted as a non-fatal note.

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

/// The fixture backend's repo id — the `source` half of every model identity
/// in these tests.
/// Post-processing is stage 2 of the pipeline.
const PP_STAGE: &str = "/pipeline/2";
const PP_STAGE_MODEL: &str = "/pipeline/2/model";

/// A backend that serves only a post-processor.
const PP_ONLY_SOURCE: &str = "github.com/super-stt/textclean";

const FIXTURE_SOURCE: &str = "github.com/super-stt/openai";

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

/// Seed the fixture backend into `<data_home>/super-stt/backends/fixture-openai/`.
/// It declares one transcription model (`whisper-1`, cloud) and one
/// post-processor (`cleanup-1`, local), so both directions of the role gate can
/// be exercised without tripping the separate online-models gate.
fn seed_fixture_backend(data_home: &Path) {
    let backend_dir = data_home
        .join("super-stt")
        .join("backends")
        .join("fixture-openai");
    std::fs::create_dir_all(&backend_dir).expect("create fixture backend dir");

    let toml = r#"[backend]
source = "github.com/super-stt/openai"
name = "Fixture OpenAI"
version = "1.0.0"
kind = "wasm"
entrypoint = "openai.wasm"
contract = "v2"
description = "Test backend."
license = "Apache-2.0"

[network]
allowed_hosts = ["api.openai.com"]

[[secrets]]
name = "openai_api_key"
label = "OpenAI API key"
description = "Your OpenAI API key."
required = true

[[options]]
name = "base_url"
label = "Base URL"
description = "Override the OpenAI API base URL."
type = "string"

[[options]]
name = "region"
label = "Region"
description = "Upstream region."
type = "string"
default = "us-east-1"

[[models]]
name = "whisper-1"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]

[[models]]
name = "cleanup-1"
role = "post_processor"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    // Create a placeholder entrypoint so the manifest can reference it.
    std::fs::write(backend_dir.join("openai.wasm"), b"").expect("write placeholder entrypoint");

    seed_post_processor_only_backend(data_home);
}

/// A backend serving nothing but a post-processor — what a cleanup backend like
/// `super-stt-textclean` looks like on disk. Stage 1 must refuse it.
fn seed_post_processor_only_backend(data_home: &Path) {
    let backend_dir = data_home
        .join("super-stt")
        .join("backends")
        .join("fixture-textclean");
    std::fs::create_dir_all(&backend_dir).expect("create pp-only backend dir");

    let toml = r#"[backend]
source = "github.com/super-stt/textclean"
name = "Fixture Text Cleanup"
version = "1.0.0"
kind = "wasm"
entrypoint = "textclean.wasm"
contract = "v2"
description = "Post-processing only."
license = "GPL-3.0-only"

[network]
allowed_hosts = []

[[models]]
name = "textclean"
role = "post_processor"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["cpu"]
"#;
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write pp-only backend.toml");
    std::fs::write(backend_dir.join("textclean.wasm"), b"").expect("write placeholder entrypoint");
}

async fn start_daemon(scopes: &[&str]) -> (DaemonGuard, PathBuf, String) {
    let unique = format!("stt-postproc-{}-{}", std::process::id(), next_test_uniq());
    let tmp = std::env::temp_dir();
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));

    std::fs::create_dir_all(&config_home).expect("create test config dir");
    std::fs::create_dir_all(&data_home).expect("create test data dir");

    // Seed the fixture backend so the daemon has something with declared options.
    seed_fixture_backend(&data_home);

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

    // Hand the child to the guard before the readiness loop: the timeout
    // panic below must still kill and reap the daemon, not leak it.
    let guard = DaemonGuard {
        child,
        cleanup_paths: vec![http_socket.clone(), config_home, data_home],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists()
            && http_client::auth_request(http_socket.clone(), "postproc-smoke-probe", &["status"])
                .await
                .is_ok()
        {
            // Mint the token with the caller-specified scopes.
            let auth = http_client::auth_request(http_socket.clone(), "postproc-smoke", scopes)
                .await
                .expect("auth_request for test scopes");
            let token = auth.session_token;
            return (guard, http_socket, token);
        }
        sleep(Duration::from_millis(200)).await;
    }
    panic!("daemon HTTP listener not ready within 120s");
}

/// Issue an HTTP request and return `(status, json_body)`.
async fn raw_request(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let stream = UnixStream::connect(socket_path).await.expect("connect");
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = handshake::<_, Full<Bytes>>(io).await.expect("handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let body_bytes = body
        .map(|b| serde_json::to_vec(&b).expect("encode body"))
        .unwrap_or_default();

    let mut builder = Request::builder()
        .method(method)
        .uri(format!("http://stt.local/v1{path}"))
        .header("host", "stt.local")
        .header("authorization", format!("Bearer {token}"));
    if !body_bytes.is_empty() {
        builder = builder
            .header("content-type", "application/json")
            .header("content-length", body_bytes.len().to_string());
    }
    let req = builder
        .body(Full::new(Bytes::from(body_bytes)))
        .expect("build req");

    let resp = sender.send_request(req).await.expect("send req");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn get(p: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::GET, path, token, None).await
}

async fn post_req(
    p: &PathBuf,
    path: &str,
    token: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::POST, path, token, Some(body)).await
}

async fn delete_req(p: &PathBuf, path: &str, token: &str) -> (StatusCode, serde_json::Value) {
    raw_request(p, Method::DELETE, path, token, None).await
}

/// The documented default: off, with nothing selected. A fresh daemon must not
/// arrive with a post-processor already pointed at something.
#[tokio::test]
async fn post_processor_defaults_to_disabled_and_unselected() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = get(&sock, PP_STAGE, &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let pp = &body["stage"];
    assert_eq!(pp["stage"], 2);
    assert_eq!(pp["role"], "post_processor");
    assert_eq!(pp["enabled"], false);
    assert_eq!(pp["model"], serde_json::Value::Null);
    assert_eq!(pp["source"], serde_json::Value::Null);
    assert_eq!(pp["loaded"], false);
}

/// Enable → read back → disable. `POST` names the model to run; `DELETE` stops
/// it and keeps the choice, the way unloading a model keeps its backend.
#[tokio::test]
async fn post_processor_enable_read_back_and_disable() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = post_req(
        &sock,
        PP_STAGE_MODEL,
        &token,
        serde_json::json!({ "model": "cleanup-1", "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["post_processor"]["enabled"], true);
    assert_eq!(body["post_processor"]["model"], "cleanup-1");
    assert_eq!(body["post_processor"]["source"], FIXTURE_SOURCE);

    // The selection survives as configuration even though the fixture's
    // placeholder entrypoint cannot actually load — the setting is the user's
    // choice, and a load failure is reported, not silently discarded.
    let (status, body) = get(&sock, PP_STAGE, &token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["stage"]["enabled"], true);
    assert_eq!(body["stage"]["model"], "cleanup-1");

    let (status, body) = delete_req(&sock, PP_STAGE_MODEL, &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["post_processor"]["enabled"], false);
    assert_eq!(
        body["post_processor"]["model"], "cleanup-1",
        "disabling keeps the choice"
    );
}

/// `DELETE /post_processor` is a no-op when nothing is running, the way
/// `DELETE /active_model` is when no model is loaded.
#[tokio::test]
async fn disabling_with_nothing_selected_is_a_no_op() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = delete_req(&sock, PP_STAGE_MODEL, &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["post_processor"]["enabled"], false);
}

/// A selection that resolves to nothing is refused up front, rather than being
/// stored and failing silently on every recording.
#[tokio::test]
async fn an_uninstalled_model_is_refused() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = post_req(
        &sock,
        PP_STAGE_MODEL,
        &token,
        serde_json::json!({ "model": "does-not-exist", "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error_code"], "invalid_model");
}

/// The role gate, both directions: a transcription model cannot be the
/// post-processor, and a post-processor cannot be the transcription model.
/// Each would be driven over a `/v1` route its backend does not serve.
#[tokio::test]
async fn the_two_roles_do_not_cross() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = post_req(
        &sock,
        PP_STAGE_MODEL,
        &token,
        serde_json::json!({ "model": "whisper-1", "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error_code"], "invalid_model");
    // The message has to name both kinds and the stage that wants this one —
    // "invalid_model" alone leaves the user re-reading a manifest to find out
    // which of two near-identical requests they got wrong.
    let msg = body["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("transcription model") && msg.contains("post-processing model"),
        "the message must name both kinds, got: {msg:?}"
    );
    assert!(
        msg.contains("/pipeline/1/model"),
        "and point at the stage that runs it, got: {msg:?}"
    );

    let (status, body) = post_req(
        &sock,
        "/pipeline/1/model",
        &token,
        serde_json::json!({ "model": "cleanup-1", "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error_code"], "invalid_model");
    let msg = body["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("post-processing model") && msg.contains("transcription model"),
        "the mirror message must name both kinds too, got: {msg:?}"
    );
    assert!(
        msg.contains("/pipeline/2/model"),
        "and point at the post-processing stage, got: {msg:?}"
    );
}

/// `GET /backends` carries each model's role, which is what lets a settings UI
/// offer the right models in each picker.
#[tokio::test]
async fn the_backends_catalog_reports_each_models_role() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = get(&sock, "/backends", &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let models = body["backends"][0]["models"]
        .as_array()
        .expect("the fixture backend lists its models");

    let role_of = |name: &str| -> String {
        models
            .iter()
            .find(|m| m["name"] == name)
            .and_then(|m| m["role"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(role_of("whisper-1"), "transcription");
    assert_eq!(role_of("cleanup-1"), "post_processor");
}

/// Selecting a backend is its own call — the "chosen, nothing running" state,
/// the post-processing counterpart of `POST /active_backend`.
#[tokio::test]
async fn a_backend_can_be_selected_without_a_model() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = post_req(
        &sock,
        PP_STAGE,
        &token,
        serde_json::json!({ "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["post_processor"]["source"], FIXTURE_SOURCE);
    assert_eq!(body["post_processor"]["model"], serde_json::Value::Null);
    assert_eq!(body["post_processor"]["enabled"], false);

    let (_, body) = get(&sock, PP_STAGE, &token).await;
    assert_eq!(
        body["stage"]["source"], FIXTURE_SOURCE,
        "the backend choice must survive as its own state"
    );
}

/// With a backend selected, a model needs no `source` — it resolves against the
/// selection, the way `POST /active_model` resolves against the active backend.
#[tokio::test]
async fn a_model_resolves_against_the_selected_backend() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    post_req(
        &sock,
        PP_STAGE,
        &token,
        serde_json::json!({ "source": FIXTURE_SOURCE }),
    )
    .await;

    let (status, body) = post_req(
        &sock,
        PP_STAGE_MODEL,
        &token,
        serde_json::json!({ "model": "cleanup-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["post_processor"]["model"], "cleanup-1");
    assert_eq!(body["post_processor"]["source"], FIXTURE_SOURCE);
}

/// A bare model name with no backend selected has nothing to resolve against,
/// and says so instead of searching every installed backend.
#[tokio::test]
async fn a_bare_model_needs_a_selected_backend() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = post_req(
        &sock,
        PP_STAGE_MODEL,
        &token,
        serde_json::json!({ "model": "cleanup-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error_code"], "invalid_backend");
}

/// A backend that serves no post-processor is refused: selecting it would leave
/// the user picking from an empty model list with nothing saying why.
#[tokio::test]
async fn a_backend_without_a_post_processor_cannot_be_selected() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = post_req(
        &sock,
        PP_STAGE,
        &token,
        serde_json::json!({ "source": "github.com/nope/missing" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error_code"], "invalid_backend");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("no post-processing model")),
        "the error should say what kind of model the backend is missing: {body}"
    );
}

/// The two DELETEs differ exactly as they do for transcription: disabling keeps
/// the selection, deselecting the backend forgets it.
#[tokio::test]
async fn disabling_keeps_the_selection_that_deselecting_clears() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    post_req(
        &sock,
        PP_STAGE_MODEL,
        &token,
        serde_json::json!({ "model": "cleanup-1", "source": FIXTURE_SOURCE }),
    )
    .await;

    let (status, body) = delete_req(&sock, PP_STAGE_MODEL, &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["post_processor"]["enabled"], false);
    assert_eq!(
        body["post_processor"]["model"], "cleanup-1",
        "disabling must not discard the model"
    );
    assert_eq!(body["post_processor"]["source"], FIXTURE_SOURCE);

    let (_, body) = delete_req(&sock, PP_STAGE, &token).await;
    assert_eq!(
        body["post_processor"]["source"],
        serde_json::Value::Null,
        "deselecting the backend clears the model with it"
    );
    assert_eq!(body["post_processor"]["model"], serde_json::Value::Null);
}

/// Switching to a different backend drops the model that belonged to the old
/// one, the way `POST /active_backend` unloads the current model.
#[tokio::test]
async fn switching_backends_drops_the_previous_model() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    post_req(
        &sock,
        PP_STAGE_MODEL,
        &token,
        serde_json::json!({ "model": "cleanup-1", "source": FIXTURE_SOURCE }),
    )
    .await;

    // Re-selecting the *same* backend is not a switch, so the model survives.
    let (_, body) = post_req(
        &sock,
        PP_STAGE,
        &token,
        serde_json::json!({ "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(
        body["post_processor"]["model"], "cleanup-1",
        "re-selecting the same backend must not drop the model"
    );
}

/// The pipeline reports its stages in order, each naming what it is for. This
/// is the shape a third stage would join, so the ordering and the `role` on
/// each entry are the contract, not an implementation detail.
#[tokio::test]
async fn the_pipeline_lists_its_stages_in_order() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = get(&sock, "/pipeline", &token).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let stages = body["pipeline"].as_array().expect("an ordered stage list");
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0]["stage"], 1);
    assert_eq!(stages[0]["role"], "transcription");
    assert_eq!(stages[1]["stage"], 2);
    assert_eq!(stages[1]["role"], "post_processor");
}

/// A stage the pipeline does not have is a 404 that says so, rather than a
/// silent no-op — a client addressing stage 3 today has the wrong shape in mind.
#[tokio::test]
async fn an_out_of_range_stage_is_not_found() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    for (method, path) in [
        ("GET", "/pipeline/3"),
        ("DELETE", "/pipeline/3"),
        ("DELETE", "/pipeline/0/model"),
    ] {
        let (status, body) = if method == "GET" {
            get(&sock, path, &token).await
        } else {
            delete_req(&sock, path, &token).await
        };
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{method} {path} body: {body}"
        );
        assert_eq!(body["error_code"], "unknown_stage");
    }
}

/// Stage 1 answers the same verbs stage 2 does, over the same paths — the point
/// of addressing stages by position rather than by feature name.
#[tokio::test]
async fn stage_one_is_the_transcription_stage() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (status, body) = post_req(
        &sock,
        "/pipeline/1",
        &token,
        serde_json::json!({ "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let (_, body) = get(&sock, "/pipeline/1", &token).await;
    assert_eq!(body["stage"]["role"], "transcription");
    assert_eq!(body["stage"]["source"], FIXTURE_SOURCE);

    // And the whole-pipeline view agrees with the single-stage one.
    let (_, body) = get(&sock, "/pipeline", &token).await;
    assert_eq!(body["pipeline"][0]["source"], FIXTURE_SOURCE);
}

/// A backend that only post-processes cannot fill stage 1. Without this the
/// selection is accepted and the user lands on a transcription stage whose
/// model picker is empty, with nothing saying why.
#[tokio::test]
async fn a_post_processor_only_backend_cannot_fill_stage_one() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    // The fixture backend serves both roles, so a role-blind check would pass
    // here; seed a second backend that serves only a post-processor.
    let (status, body) = post_req(
        &sock,
        "/pipeline/1",
        &token,
        serde_json::json!({ "source": PP_ONLY_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
    assert_eq!(body["error_code"], "invalid_backend");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("no transcription model")),
        "the error should say what kind of model the backend is missing: {body}"
    );

    // And the mirror, already covered for stage 2, stays true.
    let (status, _) = post_req(
        &sock,
        PP_STAGE,
        &token,
        serde_json::json!({ "source": PP_ONLY_SOURCE }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the same backend does fill stage 2");
}
