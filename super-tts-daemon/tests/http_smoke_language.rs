// SPDX-License-Identifier: GPL-3.0-only
//! Language-settings HTTP smoke test: `/v1/settings/language` (global) +
//! `/v1/pipeline/1/model/{model}/language` (per-model).
//!
//! Test cases:
//! 1. Global round-trip: GET → null; POST `es-MX` → 200 + language; GET → `es-MX`; DELETE → null.
//! 2. Per-model round-trip (no model loaded): GET fixture model → 200 + resolution
//!    block (`multilingual: true`); POST a supported tag → 200; GET reflects it; DELETE → 200.
//! 3. Per-model 404: a model the stage's backend does not serve → `unknown_model`.
//! 4. Per-model 400: an empty stage → `invalid_backend`; an unsupported tag →
//!    `unsupported_language`.
//! 5. What may be set: `/settings/language/list` and
//!    `/pipeline/1/model/{model}/language/list` are each the set their own
//!    setter accepts.
//! 6. Scope denial: a `status`-scoped token → 403 on every path above.
//!
//! The per-model paths carry only the model name — the source left the URL when
//! they moved under `/pipeline/{stage}` — so each per-model test first points
//! stage 1 at the fixture backend, which is what a bare name resolves against.
//!
//! Uses `SUPER_TTS_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_TTS_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.
//!
//! The fixture backend (`fixture-openai/backend.toml`) is seeded so the daemon
//! discovers it on startup. Its model is multilingual and secret-gated, so the
//! daemon comes up idle (no model auto-loaded) — the per-model language
//! endpoint resolves against the discovered backend, so it works regardless.

mod common;

use common::{Method, StatusCode, TestDaemon};

use std::path::{Path, PathBuf};

/// Seed a multilingual fixture backend into `<data_home>/super-tts/backends/fixture-openai/`.
/// The model is multilingual and requires a secret, so the daemon starts idle.
fn seed_fixture_backend(data_home: &Path) {
    let backend_dir = data_home
        .join("super-tts")
        .join("backends")
        .join("fixture-openai");
    std::fs::create_dir_all(&backend_dir).expect("create fixture backend dir");

    let toml = r#"[backend]
source = "github.com/super-tts/openai"
name = "Fixture OpenAI"
version = "1.0.0"
kind = "wasm"
entrypoint = "openai.wasm"
product = "super-tts"
contract = "v1"
description = "Test backend."
license = "Apache-2.0"

[network]
allowed_hosts = ["api.openai.com"]

[[secrets]]
name = "openai_api_key"
label = "OpenAI API key"
description = "Your OpenAI API key."
required = true

[[models]]
name = "kokoro-1"
primary_language = "en"
multilingual = true
supported_languages = ["en", "es", "es-MX", "fr", "de"]
supported_devices = ["none"]
"#;
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    // Placeholder entrypoint so the manifest parser does not reject the missing file.
    std::fs::write(backend_dir.join("openai.wasm"), b"").expect("write placeholder entrypoint");
}

async fn start_daemon(scopes: &[&str]) -> (TestDaemon, PathBuf, String) {
    let daemon = common::daemon("language");
    seed_fixture_backend(&daemon.home().data);
    let daemon = daemon.start().await;
    let token = daemon.token("language-smoke", scopes).await;
    let socket = daemon.socket().to_path_buf();
    (daemon, socket, token)
}

/// Issue an HTTP request and return `(status, json_body)`.
async fn raw_request(
    socket_path: &PathBuf,
    method: Method,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    common::request(socket_path, method, path, Some(token), body.as_ref()).await
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

/// Case 1 — GET → null; POST `es-MX` → 200 + language; GET → `es-MX`; DELETE → null.
#[tokio::test]
async fn global_language_round_trips() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    // GET → null initially
    let (st, body) = get(&sock, "/settings/language", &token).await;
    assert_eq!(st, StatusCode::OK, "initial GET: {body}");
    assert_eq!(
        body["language"],
        serde_json::Value::Null,
        "language must be null before any SET: {body}"
    );

    // POST es-MX
    let (st, body) = post_req(
        &sock,
        "/settings/language",
        &token,
        serde_json::json!({ "language": "es-MX" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "POST language: {body}");
    assert_eq!(
        body["language"], "es-MX",
        "POST must echo set value: {body}"
    );

    // GET → es-MX persisted
    let (st, body) = get(&sock, "/settings/language", &token).await;
    assert_eq!(st, StatusCode::OK, "GET after POST: {body}");
    assert_eq!(
        body["language"], "es-MX",
        "GET must return persisted value: {body}"
    );

    // DELETE → null
    let (st, body) = delete_req(&sock, "/settings/language", &token).await;
    assert_eq!(st, StatusCode::OK, "DELETE language: {body}");
    assert_eq!(
        body["language"],
        serde_json::Value::Null,
        "DELETE must clear language to null: {body}"
    );
}

/// The fixture backend's source, as `GET /pipeline/1/backend/list` reports it.
const FIXTURE_SOURCE: &str = "github.com/super-tts/openai";

/// Per-model language path for the fixture's `kokoro-1` model.
///
/// The model name alone: it resolves against whichever backend fills the stage,
/// which is why every per-model test below selects one first.
const FIXTURE_MODEL_LANG: &str = "/pipeline/1/model/kokoro-1/language";

/// Point stage 1 at the fixture backend, which is what makes a bare model name
/// in the path resolvable. Nothing is loaded — selection is a durable choice,
/// and the language endpoints answer for a model that has never been up.
async fn select_fixture_backend(sock: &PathBuf, token: &str) {
    let (st, body) = post_req(
        sock,
        "/pipeline/1",
        token,
        serde_json::json!({ "source": FIXTURE_SOURCE }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "select the fixture backend: {body}");
}

/// Case 2 — Per-model round-trip without a loaded model.
/// GET resolves the fixture model (multilingual: true); POST a supported tag →
/// 200 + override reflected; GET reflects it; DELETE clears back to default.
#[tokio::test]
async fn per_model_language_round_trips_without_loaded_model() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    select_fixture_backend(&sock, &token).await;
    let path = FIXTURE_MODEL_LANG.to_string();

    // GET resolves even though no model is loaded (resolves against discovery).
    let (st, body) = get(&sock, &path, &token).await;
    assert_eq!(st, StatusCode::OK, "initial per-model GET: {body}");
    assert_eq!(
        body["language"]["multilingual"], true,
        "fixture model is multilingual: {body}"
    );
    assert_eq!(
        body["language"]["override"],
        serde_json::Value::Null,
        "no override before any POST: {body}"
    );
    assert_eq!(
        body["language"]["primary"], "en",
        "fixture model's primary_language is en: {body}"
    );

    // POST a supported tag.
    let (st, body) = post_req(
        &sock,
        &path,
        &token,
        serde_json::json!({ "language": "es-MX" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "POST per-model language: {body}");
    assert_eq!(
        body["language"]["override"], "es-MX",
        "POST must echo the stored override: {body}"
    );
    assert_eq!(
        body["language"]["source"], "override",
        "resolution source is override after POST: {body}"
    );

    // GET reflects the persisted override.
    let (st, body) = get(&sock, &path, &token).await;
    assert_eq!(st, StatusCode::OK, "GET after POST: {body}");
    assert_eq!(
        body["language"]["override"], "es-MX",
        "GET must return the persisted override: {body}"
    );

    // DELETE clears back to default.
    let (st, body) = delete_req(&sock, &path, &token).await;
    assert_eq!(st, StatusCode::OK, "DELETE per-model language: {body}");
    assert_eq!(
        body["language"]["override"],
        serde_json::Value::Null,
        "DELETE clears the override: {body}"
    );
    assert_eq!(
        body["language"]["source"], "default",
        "resolution source is default after DELETE: {body}"
    );
}

/// Case 3 — A model the stage's backend does not serve → 404 `unknown_model`.
///
/// The source left the URL when this moved under `/pipeline/{stage}`, so the
/// only miss left is the model name. The code is what a client branches on;
/// `message` is now a sentence naming the model, not the code repeated.
#[tokio::test]
async fn per_model_language_unknown_model_is_404() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    select_fixture_backend(&sock, &token).await;

    let unknown_model = "/pipeline/1/model/does-not-exist/language";
    let (st, body) = get(&sock, unknown_model, &token).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "GET unknown model must be 404: {body}"
    );
    assert_eq!(body["error_code"], "unknown_model", "{body}");
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|m| m.contains("does-not-exist")),
        "the refusal names the model it could not find: {body}"
    );

    let (st, body) = post_req(
        &sock,
        unknown_model,
        &token,
        serde_json::json!({ "language": "es" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "POST unknown model must be 404: {body}"
    );
    assert_eq!(body["error_code"], "unknown_model", "{body}");

    let (st, body) = delete_req(&sock, unknown_model, &token).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "DELETE unknown model must be 404: {body}"
    );
    assert_eq!(body["error_code"], "unknown_model", "{body}");
}

/// A stage with nothing selected has nothing to resolve a bare model name
/// against, and says so with `400 invalid_backend` rather than guessing which
/// installed backend serves a model of that name.
///
/// That guess is the regression this pins: two backends may serve the same
/// model name, and picking one of them would write one backend's language
/// preference onto the other's model.
#[tokio::test]
async fn per_model_language_needs_a_backend_in_the_stage() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    for (method, body) in [
        (Method::GET, None),
        (Method::POST, Some(serde_json::json!({ "language": "es" }))),
        (Method::DELETE, None),
    ] {
        let (st, resp) = raw_request(&sock, method.clone(), FIXTURE_MODEL_LANG, &token, body).await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "{method} with an empty stage must be 400: {resp}"
        );
        assert_eq!(resp["error_code"], "invalid_backend", "{method}: {resp}");
    }

    let (st, resp) = get(&sock, "/pipeline/1/model/kokoro-1/language/list", &token).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert_eq!(resp["error_code"], "invalid_backend", "{resp}");
}

/// Case 4 — POST an unsupported tag for a known model → 400.
#[tokio::test]
async fn per_model_language_unsupported_tag_is_400() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    select_fixture_backend(&sock, &token).await;

    // `ja` is not in the fixture's supported_languages (["en","es","es-MX","fr","de"]).
    let (st, body) = post_req(
        &sock,
        FIXTURE_MODEL_LANG,
        &token,
        serde_json::json!({ "language": "ja" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "POST unsupported tag must be 400: {body}"
    );
    assert_eq!(body["message"], "unsupported_language", "{body}");
}

/// What is set and what may be set are separate paths, and the second is
/// exactly the set the first accepts.
///
/// A picker filled from the model's declared `supported_languages` gets two
/// things wrong, and both are only discoverable by choosing the wrong entry:
/// `auto` is accepted and is not declared, and the block that reports the
/// override no longer carries the list at all.
#[tokio::test]
async fn a_models_language_list_is_what_its_setter_accepts() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    select_fixture_backend(&sock, &token).await;

    let (st, body) = get(&sock, "/pipeline/1/model/kokoro-1/language/list", &token).await;
    assert_eq!(st, StatusCode::OK, "GET the model's list: {body}");
    assert_eq!(body["status"], "success", "{body}");
    let offered: Vec<&str> = body["available_languages"]
        .as_array()
        .expect("available_languages is a list")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        offered.first(),
        Some(&"auto"),
        "auto is accepted and is not declared, so the list has to add it: {offered:?}"
    );
    assert!(
        offered.contains(&"es-MX"),
        "the model's own tags follow it: {offered:?}"
    );

    // Every tag offered is one POST takes.
    for tag in &offered {
        let (st, body) = post_req(
            &sock,
            FIXTURE_MODEL_LANG,
            &token,
            serde_json::json!({ "language": tag }),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "offered {tag} but refused it: {body}");
    }

    // And a tag it does not offer is refused, so the list is the whole truth.
    let (st, body) = post_req(
        &sock,
        FIXTURE_MODEL_LANG,
        &token,
        serde_json::json!({ "language": "zz" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "a tag off the list must be refused: {body}"
    );
}

/// The global setting has a list of its own, and it is not the per-model one:
/// this is what `POST /settings/language` accepts, `auto` included.
///
/// A client that filled the global picker from a BCP-47 list of its own would
/// offer tags the setter refuses; one that filled it from a model's list would
/// narrow the global preference to whatever happens to be installed.
#[tokio::test]
async fn the_global_language_setting_lists_what_it_accepts() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let (st, body) = get(&sock, "/settings/language/list", &token).await;
    assert_eq!(st, StatusCode::OK, "GET /settings/language/list: {body}");
    assert_eq!(body["status"], "success", "{body}");
    let offered: Vec<&str> = body["available_languages"]
        .as_array()
        .expect("available_languages is a list")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        offered.first(),
        Some(&"auto"),
        "`auto` is always on offer, even with nothing installed: {offered:?}"
    );
    assert!(
        offered.contains(&"es-MX"),
        "a regional tag the setter takes: {offered:?}"
    );

    // The list is the promise: a tag on it round-trips through the setting.
    let (st, body) = post_req(
        &sock,
        "/settings/language",
        &token,
        serde_json::json!({ "language": "es-MX" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "offered es-MX but refused it: {body}");
    assert_eq!(body["language"], "es-MX", "{body}");

    // And a tag off it is refused, so the list is the whole truth.
    let (st, body) = post_req(
        &sock,
        "/settings/language",
        &token,
        serde_json::json!({ "language": "zz" }),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "a tag off the list must be refused: {body}"
    );
    assert_eq!(body["error_code"], "unsupported_language", "{body}");
}

/// Case 5 — A `status`-scoped token must be denied (403) on the global and
/// per-model language paths, and on the two lists beside them.
#[tokio::test]
async fn language_endpoints_require_settings_scope() {
    let (_guard, sock, token) = start_daemon(&["status"]).await;

    for (method, path) in [
        (Method::GET, "/settings/language"),
        (Method::POST, "/settings/language"),
        (Method::GET, "/settings/language/list"),
        (Method::GET, FIXTURE_MODEL_LANG),
        (Method::POST, FIXTURE_MODEL_LANG),
        (Method::DELETE, FIXTURE_MODEL_LANG),
        (Method::GET, "/pipeline/1/model/kokoro-1/language/list"),
    ] {
        let body = (method == Method::POST).then(|| serde_json::json!({ "language": "es" }));
        let (st, resp) = raw_request(&sock, method.clone(), path, &token, body).await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "{method} /v1{path} should be 403 for status-scoped token: {resp}"
        );
    }
}
