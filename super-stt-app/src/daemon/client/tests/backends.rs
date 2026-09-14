// SPDX-License-Identifier: GPL-3.0-only
//! `v1/backends` — the installed catalog, one backend's removal, and the
//! options and secrets it declares.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::backends::{self, options, secrets};

const SOURCE: &str = "github.com/super-stt/openai";
/// Every source is a repo id with slashes in it, so every path that carries
/// one has to encode it. Unencoded, `/backend/github.com/super-stt/openai`
/// is four path segments and matches no route.
const SOURCE_ENCODED: &str = "github.com%2Fsuper-stt%2Fopenai";

fn backend_entry() -> serde_json::Value {
    json!({
        "source": SOURCE,
        "name": "OpenAI",
        "version": "1.2.3",
        "kind": "wasm",
        "models": [],
        "secrets": [],
        "options": [],
    })
}

#[tokio::test]
async fn list_backends_reads_the_catalog_out_of_the_envelope() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "backends": [backend_entry()],
    })));

    let installed = backends::list_backends().await.expect("catalog listed");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/backend/list");
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].source, SOURCE);
    assert_eq!(installed[0].name, "OpenAI");
}

/// A daemon with nothing installed omits the key rather than sending `[]`.
/// Reading that as an error would put a failure toast in front of every user
/// who has not installed a backend yet — which is every new one.
#[tokio::test]
async fn an_absent_catalog_is_an_empty_one() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "success" })));

    let installed = backends::list_backends().await.expect("catalog listed");

    assert!(installed.is_empty());
}

/// A `status:"error"` body is an operational failure even at HTTP 200 — the
/// settings envelope reports it in the field, not the status line.
#[tokio::test]
async fn a_status_error_envelope_fails_the_read() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "error",
        "message": "backend directory unreadable",
    })));

    let error = backends::list_backends().await.expect_err("read failed");

    assert_eq!(error.to_string(), "backend directory unreadable");
}

/// The daemon's message is the whole error when it sends one; when it sends
/// none, the caller still needs to know which call failed.
#[tokio::test]
async fn an_error_envelope_without_a_message_names_the_call() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "error" })));

    let error = backends::list_backends().await.expect_err("read failed");

    assert_eq!(error.to_string(), "list_backends failed");
}

/// The installed catalog is the settings page's whole Models tab, so a payload
/// it cannot read is reported rather than quietly rendered as "nothing
/// installed" — which is what an empty list would say, and it would be a lie.
#[tokio::test]
async fn a_catalog_that_cannot_be_parsed_is_an_error_not_an_empty_list() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "backends": [{ "source": SOURCE }],
    })));

    let error = backends::list_backends().await.expect_err("unparseable");

    assert!(
        error.to_string().starts_with("failed to parse backends:"),
        "got {error}"
    );
}

#[tokio::test]
async fn uninstall_encodes_the_source_into_the_path() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "uninstalled": true,
        "was_active": true,
        "was_post_processor": false,
    })));

    let removed = backends::uninstall(SOURCE).await.expect("backend removed");

    let request = daemon.request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(request.path(), format!("/backend/{SOURCE_ENCODED}"));
    assert!(removed.uninstalled);
    assert!(removed.was_active);
}

/// `was_post_processor` postdates the second stage. An older daemon omits it,
/// and the absence has to read as "no", not as an unparseable answer — the
/// uninstall already happened by then.
#[tokio::test]
async fn an_uninstall_answer_without_the_post_processor_flag_still_parses() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(
        &json!({ "uninstalled": true, "was_active": false }),
    ));

    let removed = backends::uninstall(SOURCE).await.expect("backend removed");

    assert!(!removed.was_post_processor);
}

#[tokio::test]
async fn setting_an_option_posts_the_value_to_the_named_path() {
    let daemon = FakeDaemon::start().await;

    options::set_backend_option(
        SOURCE.to_string(),
        "base url".to_string(),
        "https://example.test".to_string(),
    )
    .await
    .expect("option set");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.path(),
        format!("/backend/{SOURCE_ENCODED}/option/base%20url")
    );
    assert_eq!(request.json(), json!({ "value": "https://example.test" }));
}

#[tokio::test]
async fn clearing_an_option_deletes_the_same_path() {
    let daemon = FakeDaemon::start().await;

    options::clear_backend_option(SOURCE.to_string(), "base url".to_string())
        .await
        .expect("option cleared");

    let request = daemon.request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(
        request.path(),
        format!("/backend/{SOURCE_ENCODED}/option/base%20url")
    );
    assert!(request.body.is_empty());
}

#[tokio::test]
async fn setting_a_secret_posts_the_value_to_the_secret_path() {
    let daemon = FakeDaemon::start().await;

    secrets::set_backend_secret(
        SOURCE.to_string(),
        "api_key".to_string(),
        "sk-test".to_string(),
    )
    .await
    .expect("secret set");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.path(),
        format!("/backend/{SOURCE_ENCODED}/secret/api_key")
    );
    assert_eq!(request.json(), json!({ "value": "sk-test" }));
}

#[tokio::test]
async fn clearing_a_secret_deletes_the_same_path() {
    let daemon = FakeDaemon::start().await;

    secrets::clear_backend_secret(SOURCE.to_string(), "api_key".to_string())
        .await
        .expect("secret cleared");

    let request = daemon.request();
    assert_eq!(request.method, "DELETE");
    assert_eq!(
        request.path(),
        format!("/backend/{SOURCE_ENCODED}/secret/api_key")
    );
}

/// The list reports which secrets are *configured*, never their values — the
/// daemon holds those in the keyring and the app never sees one.
#[tokio::test]
async fn listing_secrets_reports_configured_without_values() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "secrets": [
            { "name": "api_key", "configured": true },
            { "name": "org_id", "configured": false },
        ],
    })));

    let listed = secrets::list_backend_secrets(SOURCE.to_string())
        .await
        .expect("secrets listed");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.path(),
        format!("/backend/{SOURCE_ENCODED}/secret/list")
    );
    assert_eq!(
        listed,
        vec![("api_key".to_string(), true), ("org_id".to_string(), false),]
    );
}

/// An entry missing either half says nothing a checkbox could render, so it is
/// dropped rather than defaulted — a secret shown as "not configured" when the
/// daemon never said so is the one mistake this readout must not make.
#[tokio::test]
async fn a_malformed_secret_entry_is_dropped_rather_than_guessed() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "status": "success",
        "secrets": [
            { "name": "api_key" },
            { "configured": true },
            { "name": "org_id", "configured": true },
        ],
    })));

    let listed = secrets::list_backend_secrets(SOURCE.to_string())
        .await
        .expect("secrets listed");

    assert_eq!(listed, vec![("org_id".to_string(), true)]);
}

#[tokio::test]
async fn a_backend_declaring_no_secrets_lists_none() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({ "status": "success" })));

    let listed = secrets::list_backend_secrets(SOURCE.to_string())
        .await
        .expect("secrets listed");

    assert!(listed.is_empty());
}
