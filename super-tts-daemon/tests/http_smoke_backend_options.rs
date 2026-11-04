// SPDX-License-Identifier: GPL-3.0-only
//! Options-scope HTTP smoke test: `/backend/{backend_id}/option/...`
//!
//! Three test cases:
//! 1. Round-trip: read default → set override → reset to default.
//! 2. Unknown option: GET/POST to a `{name}` not declared → 404 `unknown_option`.
//! 3. Unknown backend: GET on a source not installed → 404 `unknown_backend`.
//!
//! Uses `SUPER_TTS_KEYRING_MOCK=1` (in-memory keyring) and
//! `SUPER_TTS_AUTO_APPROVE=1` (no GUI) — hermetic, part of default CI.
//!
//! The fixture backend (`fixture-openai/backend.toml`) is written into the
//! isolated `XDG_DATA_HOME/super-tts/backends/` tree so the daemon discovers
//! it on startup. It declares three options: `base_url` and `region` are
//! open-ended, and `styling` offers a closed set, so both sides of the
//! `choices` gate are exercisable.

mod common;

use common::{Method, StatusCode, TestDaemon};

use std::path::{Path, PathBuf};

/// URL-encoded `{source}` path segment for the fixture backend.
/// The raw source id is `github.com/super-tts/openai`; slashes become `%2F`.
const FIXTURE_SOURCE_ENC: &str = "github.com%2Fsuper-tts%2Fopenai";

/// Seed the fixture backend into `<data_home>/super-tts/backends/fixture-openai/`.
/// The manifest declares `base_url` as an option with a default value.
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

[[options]]
name = "styling"
label = "Styling"
description = "The register."
type = "string"
default = "formal"
choices = ["casual", "formal"]

[[options]]
name = "temperature"
label = "Temperature"
description = "How freely the model samples."
type = "float"
default = 0.9
min = 0.6
max = 1.2
step = 0.1

[[options]]
name = "retries"
label = "Retries"
description = "How many times to try again."
type = "integer"

[[models]]
name = "kokoro-1"
primary_language = "en"
supported_languages = ["en"]
supported_devices = ["none"]
"#;
    std::fs::write(backend_dir.join("backend.toml"), toml).expect("write fixture backend.toml");
    // Create a placeholder entrypoint so the manifest can reference it.
    std::fs::write(backend_dir.join("openai.wasm"), b"").expect("write placeholder entrypoint");
}

async fn start_daemon(scopes: &[&str]) -> (TestDaemon, PathBuf, String) {
    let daemon = common::daemon("options");
    seed_fixture_backend(&daemon.home().data);
    let daemon = daemon.start().await;
    let token = daemon.token("options-smoke", scopes).await;
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

/// Round-trip: default → override → reset to default.
#[tokio::test]
async fn option_set_get_and_reset_to_default() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/region");

    // Default before any override.
    let (s, body) = get(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET option before set: {body}");
    assert_eq!(
        body["value"], "us-east-1",
        "should start at manifest default: {body}"
    );

    // Set override.
    let (s, body) = post_req(
        &sock,
        &opt_path,
        &token,
        serde_json::json!({ "value": "eu-west-1" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST option: {body}");
    assert_eq!(
        body["value"], "eu-west-1",
        "value should reflect override: {body}"
    );

    // Reset to default (DELETE clears the override).
    let (s, body) = delete_req(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "DELETE option: {body}");
    assert_eq!(
        body["value"], "us-east-1",
        "value should revert to manifest default after DELETE: {body}"
    );
}

/// `base_url` is the one option a manifest may not supply a value for — its
/// host is authorized for egress, so only the user may name it. It therefore
/// round-trips through the unset state rather than through a default.
#[tokio::test]
async fn base_url_round_trips_through_unset() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/base_url");

    let (s, body) = get(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET base_url before set: {body}");
    assert!(
        body["value"].is_null() && body["default"].is_null(),
        "base_url starts unset, with no manifest default: {body}"
    );

    let (s, body) = post_req(
        &sock,
        &opt_path,
        &token,
        serde_json::json!({ "value": "https://gw.example.com" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "POST base_url: {body}");
    assert_eq!(
        body["value"], "https://gw.example.com",
        "value should reflect override: {body}"
    );

    let (s, body) = delete_req(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "DELETE base_url: {body}");
    assert!(
        body["value"].is_null(),
        "clearing the override leaves base_url unset, not defaulted: {body}"
    );
}

/// `base_url` is stored canonical, so the settings field reads back the
/// endpoint that will be dialed rather than the string that was posted. The
/// scheme is why it matters: a value naming none is read by its host, and
/// whether the request is encrypted must not be invisible in the field.
#[tokio::test]
async fn base_url_is_stored_canonical() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/base_url");

    for (posted, want) in [
        // A private gateway named without a scheme is plaintext.
        ("192.168.0.179:8080/v1", "http://192.168.0.179:8080/v1"),
        ("localhost:4000/v1", "http://localhost:4000/v1"),
        // A name the daemon cannot classify keeps https.
        ("gw.example.com/v1", "https://gw.example.com/v1"),
        // The rest of the canonical form travels with it.
        (
            "  HTTPS://user:pass@gw.example.com/v1/?k=v  ",
            "https://gw.example.com/v1",
        ),
    ] {
        let (s, body) = post_req(
            &sock,
            &opt_path,
            &token,
            serde_json::json!({ "value": posted }),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "POST {posted:?}: {body}");
        assert_eq!(body["value"], want, "POST {posted:?} stored: {body}");

        let (s, body) = get(&sock, &opt_path, &token).await;
        assert_eq!(s, StatusCode::OK, "GET after {posted:?}: {body}");
        assert_eq!(body["value"], want, "GET after {posted:?}: {body}");
    }

    // A value yielding no host is refused on the write. It used to be stored as
    // typed so the next model load could refuse it by name — but an option
    // write no longer reloads anything, so there is no later load to catch it,
    // and storing it would report success while the backend kept its old
    // endpoint. What must not happen either way is dropping it silently.
    let (s, body) = post_req(
        &sock,
        &opt_path,
        &token,
        serde_json::json!({ "value": "http://" }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "POST unreadable: {body}");
    assert_eq!(body["error_code"], "invalid_value", "{body}");

    // And the previous value is still the one in effect — a refused write
    // stores nothing.
    let (s, body) = get(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET after refusal: {body}");
    assert_eq!(body["value"], "https://gw.example.com/v1", "{body}");
}

/// Listing all options for the backend.
#[tokio::test]
async fn option_list_returns_declared_options() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let list_path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/list");

    let (s, body) = get(&sock, &list_path, &token).await;
    assert_eq!(s, StatusCode::OK, "GET option/list: {body}");
    assert_eq!(body["status"], "success", "list status: {body}");
    let options = body["options"].as_array().expect("options array");
    assert_eq!(options.len(), 5, "five declared options: {body}");
    let o0 = &options[0];
    assert_eq!(o0["name"], "base_url", "option name: {body}");
    assert!(
        o0["value"].is_null(),
        "base_url carries no default, so no value until the user sets one: {body}"
    );
    let o1 = &options[1];
    assert_eq!(o1["name"], "region", "option name: {body}");
    assert_eq!(o1["value"], "us-east-1", "default value in list: {body}");
    assert_eq!(
        o1["choices"],
        serde_json::json!([]),
        "an open-ended option offers no list, which is what a client renders \
         a text field from: {body}"
    );
    let o2 = &options[2];
    assert_eq!(o2["name"], "styling", "option name: {body}");
    assert_eq!(
        o2["choices"],
        serde_json::json!(["casual", "formal"]),
        "the values the option accepts, in manifest order: {body}"
    );
}

/// An option that declares `choices` accepts those and nothing else. The
/// settings app renders a dropdown that cannot offer anything else, so this is
/// the guard for every other client — and for the stored value staying one the
/// backend understands, since it is injected into the load headers verbatim.
#[tokio::test]
async fn a_value_the_option_does_not_offer_is_refused() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/styling");

    let (s, body) = post_req(
        &sock,
        &opt_path,
        &token,
        serde_json::json!({ "value": "casual" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "an offered value stores: {body}");
    assert_eq!(body["value"], "casual", "body: {body}");

    let (s, body) = post_req(
        &sock,
        &opt_path,
        &token,
        serde_json::json!({ "value": "formalish" }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a value off the list must be refused: {body}"
    );
    assert_eq!(body["error_code"], "invalid_value", "body: {body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("casual, formal"),
        "the refusal names what is on offer: {body}"
    );

    // Refused, so the earlier value is still what is in effect.
    let (s, body) = get(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["value"], "casual",
        "a refused write changes nothing: {body}"
    );

    // An option declaring no list still takes anything.
    let region = format!("/backend/{FIXTURE_SOURCE_ENC}/option/region");
    let (s, body) = post_req(
        &sock,
        &region,
        &token,
        serde_json::json!({ "value": "eu-west-9" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "an open-ended option is ungated: {body}");
}

/// A declared type is a promise about what the stored text parses back to, and
/// the daemon is where it is kept. Before this, an `integer` option would store
/// `banana` and inject it, leaving the backend to discover the problem at the
/// one point where the only way to report it is to fail a synthesis.
#[tokio::test]
async fn a_value_of_the_wrong_type_is_refused() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;

    let temperature = format!("/backend/{FIXTURE_SOURCE_ENC}/option/temperature");
    let (s, body) = post_req(
        &sock,
        &temperature,
        &token,
        serde_json::json!({ "value": "0.85" }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "a float stores in a float option: {body}"
    );
    assert_eq!(body["value"], "0.85", "body: {body}");

    let (s, body) = post_req(
        &sock,
        &temperature,
        &token,
        serde_json::json!({ "value": "warm" }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a word is not a float: {body}");
    assert_eq!(body["error_code"], "invalid_value", "body: {body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap_or_default()
            .contains("takes a number"),
        "the refusal names the type rather than a list of choices: {body}"
    );

    let retries = format!("/backend/{FIXTURE_SOURCE_ENC}/option/retries");
    let (s, body) = post_req(&sock, &retries, &token, serde_json::json!({ "value": "3" })).await;
    assert_eq!(s, StatusCode::OK, "an integer stores: {body}");
    let (s, body) = post_req(
        &sock,
        &retries,
        &token,
        serde_json::json!({ "value": "2.5" }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a fraction is not an integer: {body}"
    );
}

/// Declared bounds are a contract the daemon keeps, not a hint for the slider:
/// a value outside them is refused however it was written.
#[tokio::test]
async fn a_value_outside_the_declared_range_is_refused() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/temperature");

    for edge in ["0.6", "1.2"] {
        let (s, body) = post_req(&sock, &path, &token, serde_json::json!({ "value": edge })).await;
        assert_eq!(s, StatusCode::OK, "the bounds are inclusive: {body}");
    }

    // Between two notches of the step, but inside the range. The grid belongs
    // to the control; refusing this would make the option narrower than its
    // own bounds say.
    let (s, body) = post_req(&sock, &path, &token, serde_json::json!({ "value": "0.85" })).await;
    assert_eq!(s, StatusCode::OK, "`step` is not a bound: {body}");

    for outside in ["0.5", "1.3", "-1"] {
        let (s, body) = post_req(
            &sock,
            &path,
            &token,
            serde_json::json!({ "value": outside }),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::BAD_REQUEST,
            "{outside} is outside the range: {body}"
        );
        assert_eq!(body["error_code"], "invalid_value", "body: {body}");
        assert!(
            body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("between 0.6 and 1.2"),
            "the refusal names the range: {body}"
        );
    }
}

/// A float option reports its type and its default through the list the
/// settings UI renders a form from, so a client can offer a numeric field.
#[tokio::test]
async fn a_float_option_reports_its_type_and_default() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let (s, body) = get(
        &sock,
        &format!("/backend/{FIXTURE_SOURCE_ENC}/option/list"),
        &token,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {body}");
    let options = body["options"].as_array().expect("options array");
    let temperature = options
        .iter()
        .find(|o| o["name"] == "temperature")
        .expect("the float option is listed");
    assert_eq!(temperature["type"], "float", "body: {temperature}");
    // Both ends and a grid: what a client renders as a slider rather than a
    // field, the way a non-empty `choices` is what it renders as a dropdown.
    assert_eq!(temperature["min"], 0.6, "body: {temperature}");
    assert_eq!(temperature["max"], 1.2, "body: {temperature}");
    assert_eq!(temperature["step"], 0.1, "body: {temperature}");
    let retries = options
        .iter()
        .find(|o| o["name"] == "retries")
        .expect("the integer option is listed");
    assert!(
        retries["min"].is_null() && retries["step"].is_null(),
        "an unbounded option declares no range: {retries}"
    );
    // The shortest round-tripping form, not the binary fraction nearest 0.9.
    assert_eq!(temperature["default"], "0.9", "body: {temperature}");
    assert_eq!(temperature["value"], "0.9", "body: {temperature}");
}

/// Clearing a `choices` option returns it to the manifest default rather than
/// to nothing, which is how a client returns a dropdown to its declared value:
/// storing a copy of today's default instead would pin the user to it and stop
/// a later manifest from moving it.
#[tokio::test]
async fn clearing_a_choices_option_returns_it_to_the_declared_default() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/styling");

    let (s, body) = get(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["value"], "formal",
        "it starts on the manifest default: {body}"
    );

    let (s, body) = post_req(
        &sock,
        &opt_path,
        &token,
        serde_json::json!({ "value": "casual" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "body: {body}");
    assert_eq!(body["value"], "casual", "body: {body}");

    let (s, body) = delete_req(&sock, &opt_path, &token).await;
    assert_eq!(s, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["value"], "formal",
        "DELETE reverts to the declared default, not to unset: {body}"
    );
    assert_eq!(
        body["default"], "formal",
        "and says what clearing reverted to: {body}"
    );
}

/// GET on an undeclared option name returns 404 `unknown_option`.
#[tokio::test]
async fn undeclared_option_is_404() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/not_a_real_option");

    let (s, body) = get(&sock, &path, &token).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "undeclared option must be 404: {body}"
    );
    assert_eq!(
        body["message"], "unknown_option",
        "error code for undeclared option: {body}"
    );
}

/// POST with an empty `value` returns 400 `invalid_request`.
#[tokio::test]
async fn set_empty_value_is_400() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let opt_path = format!("/backend/{FIXTURE_SOURCE_ENC}/option/base_url");

    let (s, body) = post_req(&sock, &opt_path, &token, serde_json::json!({ "value": "" })).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "empty value must be 400: {body}"
    );
    assert_eq!(
        body["message"], "invalid_request",
        "error code for empty value: {body}"
    );
}

/// GET on an unknown backend returns 404 `unknown_backend`.
#[tokio::test]
async fn unknown_backend_is_404() {
    let (_guard, sock, token) = start_daemon(&["settings"]).await;
    let path = "/backend/github.com%2Fnot%2Finstalled/option/base_url";

    let (s, body) = get(&sock, path, &token).await;
    assert_eq!(
        s,
        StatusCode::NOT_FOUND,
        "unknown backend must be 404: {body}"
    );
    assert_eq!(
        body["message"], "unknown_backend",
        "error code for unknown backend: {body}"
    );
}

/// `GET /backend/list` reports the version on disk now, not the one the daemon
/// scanned at startup.
///
/// A client shows this beside an update badge judged from `installed_version`
/// on the registry listing, which is read per request — reported from the scan
/// instead, this would name the version the daemon started with while the badge
/// spoke for the one on disk.
#[tokio::test]
async fn backend_version_is_read_from_disk_per_request() {
    let (guard, sock, token) = start_daemon(&["settings"]).await;
    let manifest = guard
        .home()
        .data
        .join("super-tts")
        .join("backends")
        .join("fixture-openai")
        .join("backend.toml");

    let (s, body) = get(&sock, "/backend/list", &token).await;
    assert_eq!(s, StatusCode::OK, "GET /backend/list: {body}");
    assert_eq!(body["backends"][0]["version"], "1.0.0", "seeded: {body}");

    // Change it underneath the running daemon; nothing rescans.
    let edited = std::fs::read_to_string(&manifest)
        .expect("read fixture manifest")
        .replace("version = \"1.0.0\"", "version = \"2.5.0\"");
    std::fs::write(&manifest, edited).expect("write fixture manifest");

    let (s, body) = get(&sock, "/backend/list", &token).await;
    assert_eq!(s, StatusCode::OK, "GET /backend/list after edit: {body}");
    assert_eq!(
        body["backends"][0]["version"], "2.5.0",
        "version follows the manifest without a rescan: {body}"
    );
}

/// When the manifest cannot be read, the version falls back to what the last
/// scan recorded rather than blanking.
///
/// A backend whose `backend.toml` has gone missing is broken either way; the
/// last version the daemon actually loaded is more use to whoever is looking at
/// it than an empty field, and it is what the running model came from.
#[tokio::test]
async fn backend_version_falls_back_to_the_scan_when_the_manifest_is_gone() {
    let (guard, sock, token) = start_daemon(&["settings"]).await;
    let manifest = guard
        .home()
        .data
        .join("super-tts")
        .join("backends")
        .join("fixture-openai")
        .join("backend.toml");

    std::fs::remove_file(&manifest).expect("remove fixture manifest");

    let (s, body) = get(&sock, "/backend/list", &token).await;
    assert_eq!(s, StatusCode::OK, "GET /backend/list: {body}");
    assert_eq!(
        body["backends"][0]["version"], "1.0.0",
        "the scanned version stands in when the manifest cannot be read: {body}"
    );
}
