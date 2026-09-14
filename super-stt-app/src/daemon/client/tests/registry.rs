// SPDX-License-Identifier: GPL-3.0-only
//! `v1/registry` — the installable catalog: list, preview, install, refresh,
//! update.

use serde_json::json;

use super::fake_daemon::{FakeDaemon, Reply};
use crate::daemon::client::v1::registry::{self, ListFilters};

const SOURCE: &str = "github.com/super-stt/openai";

/// The minimal `InstallAccepted` the daemon answers an accepted install with.
fn install_accepted() -> serde_json::Value {
    json!({
        "install_id": "install-1",
        "source": SOURCE,
        "version": "1.2.3",
        "selected_asset": { "target": "x86_64-unknown-linux-gnu", "accel": "cpu" },
    })
}

/// The minimal catalog entry `/registry/backend/list` and `/preview` carry.
fn registry_backend() -> serde_json::Value {
    json!({
        "id": "openai",
        "source": SOURCE,
        "version": "1.2.3",
        "name": "OpenAI",
        "license": "Apache-2.0",
        "kind": "wasm",
        "contract": "v1",
        "online": true,
        "supports_gpu": false,
        "supports_cpu": false,
        "models": [],
        "secrets": [],
        "options": [],
        "compatibility": { "compatible": true },
    })
}

/// All three install routes are the same endpoint, told apart by the *key* in
/// the body: `source` is a registry identifier the daemon resolves against the
/// index, `repo_url` an arbitrary repository it clones, `local_path` a
/// directory it copies. `InstallRequest` is an untagged enum, so a URL sent
/// under `source` is not a malformed request — it parses as a registry lookup,
/// which the daemon then answers `not_found`. Nothing on either side of the
/// wire would object, which is why the key is asserted here.
#[tokio::test]
async fn the_three_install_routes_are_told_apart_by_their_body_key() {
    let daemon = FakeDaemon::start().await;
    daemon
        .reply(Reply::json(&install_accepted()))
        .reply(Reply::json(&install_accepted()))
        .reply(Reply::json(&install_accepted()));

    registry::install_by_source(SOURCE)
        .await
        .expect("by source");
    registry::install_by_repo_url("https://github.com/someone/backend")
        .await
        .expect("by repo url");
    registry::install_by_local_path("/home/me/backend")
        .await
        .expect("by local path");

    let requests = daemon.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path(), "/registry/backend/install");
    }
    assert_eq!(requests[0].json(), json!({ "source": SOURCE }));
    assert_eq!(
        requests[1].json(),
        json!({ "repo_url": "https://github.com/someone/backend" })
    );
    assert_eq!(
        requests[2].json(),
        json!({ "local_path": "/home/me/backend" })
    );
}

#[tokio::test]
async fn install_returns_the_accepted_install() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&install_accepted()));

    let accepted = registry::install_by_source(SOURCE)
        .await
        .expect("install accepted");

    assert_eq!(accepted.install_id, "install-1");
    assert_eq!(accepted.version, "1.2.3");
    assert_eq!(accepted.selected_asset.accel, vec!["cpu".to_string()]);
}

/// A refused install has to surface the daemon's own identifier. The transport
/// deserializes a 2xx body into the success type and *only* a 2xx body; an
/// error envelope reaching that deserializer reports whichever success field it
/// found missing instead of the refusal.
#[tokio::test]
async fn a_refused_install_reports_the_daemon_error_not_a_parse_failure() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::status(
        409,
        &json!({ "status": "error", "error_code": "already_installed" }),
    ));

    let error = registry::install_by_source(SOURCE)
        .await
        .expect_err("install refused");

    assert_eq!(error.to_string(), "already_installed (HTTP 409)");
    assert!(!error.to_string().contains("missing field"));
}

#[tokio::test]
async fn list_without_filters_asks_for_no_query_string() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "schema_version": 1,
        "generated_at": "2026-09-14T00:00:00Z",
        "backends": [registry_backend()],
    })));

    let listed = registry::list(&ListFilters::default())
        .await
        .expect("catalog listed");

    let request = daemon.request();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path(), "/registry/backend/list");
    assert_eq!(request.query(), "", "no filters means no `?`");
    assert_eq!(listed.backends.len(), 1);
    assert_eq!(listed.backends[0].source, SOURCE);
}

/// The filters go in the query string, percent-encoded. A search term with a
/// space in it is the ordinary case — the Browse drawer's search box — and an
/// unencoded one makes a request the daemon rejects before it reaches a route.
#[tokio::test]
async fn list_encodes_its_filters_into_the_query() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "schema_version": 1,
        "generated_at": "2026-09-14T00:00:00Z",
        "backends": [],
    })));

    registry::list(&ListFilters {
        include_incompatible: Some(true),
        kind: Some("wasm".to_string()),
        online: Some(false),
        q: Some("open ai".to_string()),
    })
    .await
    .expect("catalog listed");

    let request = daemon.request();
    assert_eq!(request.path(), "/registry/backend/list");
    assert_eq!(
        request.query(),
        "include_incompatible=true&kind=wasm&online=false&q=open%20ai"
    );
}

#[tokio::test]
async fn preview_is_told_apart_by_its_body_key_too() {
    let daemon = FakeDaemon::start().await;
    let preview = json!({ "backend": registry_backend(), "warning": "unverified_source" });
    daemon
        .reply(Reply::json(&preview))
        .reply(Reply::json(&preview));

    let by_url = registry::preview_by_repo_url("https://github.com/someone/backend")
        .await
        .expect("preview by url");
    registry::preview_by_local_path("/home/me/backend")
        .await
        .expect("preview by path");

    let requests = daemon.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path(), "/registry/backend/preview");
    assert_eq!(
        requests[0].json(),
        json!({ "repo_url": "https://github.com/someone/backend" })
    );
    assert_eq!(
        requests[1].json(),
        json!({ "local_path": "/home/me/backend" })
    );
    assert_eq!(by_url.backend.name, "OpenAI");
    assert_eq!(by_url.warning.as_deref(), Some("unverified_source"));
}

/// `POST` with an empty object, not a `GET`: the refresh re-fetches the remote
/// index, which is a side effect, and the summary counts are what it returns.
#[tokio::test]
async fn refresh_posts_an_empty_body_and_reads_the_counts() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "schema_version": 1,
        "generated_at": "2026-09-14T00:00:00Z",
        "backend_count": 7,
    })));

    let refreshed = registry::refresh().await.expect("index refreshed");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/registry/backend/refresh");
    assert_eq!(request.json(), json!({}));
    assert_eq!(refreshed.backend_count, 7);
}

#[tokio::test]
async fn update_names_the_backend_and_reports_both_versions() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "install_id": "install-2",
        "from_version": "1.2.3",
        "to_version": "1.3.0",
        "noop": false,
    })));

    let updated = registry::update(SOURCE).await.expect("backend updated");

    let request = daemon.request();
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/registry/backend/update");
    assert_eq!(request.json(), json!({ "source": SOURCE }));
    assert_eq!(updated.from_version, "1.2.3");
    assert_eq!(updated.to_version, "1.3.0");
    assert!(!updated.noop);
}

/// "Already at the latest" is a success with `noop`, not an error. The Updates
/// page renders it as "up to date" rather than a failed action.
#[tokio::test]
async fn an_update_with_nothing_to_do_comes_back_as_a_noop() {
    let daemon = FakeDaemon::start().await;
    daemon.reply(Reply::json(&json!({
        "from_version": "1.3.0",
        "to_version": "1.3.0",
        "noop": true,
    })));

    let updated = registry::update(SOURCE).await.expect("update answered");

    assert!(updated.noop);
    assert!(updated.install_id.is_none());
}
