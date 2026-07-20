// SPDX-License-Identifier: GPL-3.0-only
//! Contract: every error envelope the daemon emits must name its failure to
//! the client.
//!
//! The daemon builds error bodies through several constructors, each with a
//! slightly different shape (`error_code`, the registry's legacy `error`,
//! `message`, `data.reason`). The client collapses all of them in one place —
//! [`super_stt_shared::daemon::http_client::transport::daemon_error`] — into
//! the text a user actually sees.
//!
//! Nothing structural keeps those two halves in agreement: they live in
//! different crates, and neither crate's own tests exercise the pair. A new
//! envelope that spells its identifier differently would parse as valid JSON,
//! travel the wire intact, and reach the user as a bare `daemon returned HTTP
//! 500` — losing the identifier without failing anything. This test asserts
//! the round trip for every constructor in use, so that drift is a test
//! failure rather than a degraded error message.
//!
//! Adding an error envelope? Add it here too.

use super::internal::helpers::dispatch::json_response;
use super::internal::helpers::responses::{
    invalid_session, rate_limited, reason, recording_in_progress_response, scope_denied,
};
use super::v1::backends::{json_error, json_error_msg};
use super::v1::registry::{registry_error, registry_error_msg};
use axum::http::StatusCode;
use axum::response::Response;
use super_stt_shared::daemon::http_client::transport::error_for_status;
use super_stt_shared::models::protocol::{DaemonResponse, ErrorCode};

async fn parts_of(resp: Response) -> (StatusCode, Vec<u8>) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("collect body");
    (status, bytes.to_vec())
}

/// Run a daemon-built response through the client's mapping and return the
/// string a user would see.
async fn as_client_sees_it(resp: Response) -> String {
    let (status, body) = parts_of(resp).await;
    error_for_status(status, &body).to_string()
}

/// Each constructor, paired with the identifier that must survive the trip.
/// Failures the user can act on are worth naming precisely, so the assertion
/// is on the identifier itself rather than "some non-empty string".
#[tokio::test]
async fn every_error_envelope_reaches_the_client_naming_its_failure() {
    let cases: Vec<(&str, Response, &str)> = vec![
        (
            "registry_error",
            registry_error(StatusCode::NOT_FOUND, "not_found"),
            "not_found",
        ),
        (
            "registry_error_msg",
            registry_error_msg(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_failed",
                "Permission denied (os error 13)",
            ),
            "remove_failed",
        ),
        (
            "json_error",
            json_error(StatusCode::NOT_FOUND, "unknown_backend"),
            "unknown_backend",
        ),
        (
            "json_error_msg",
            json_error_msg(
                StatusCode::BAD_REQUEST,
                "invalid_option",
                "value out of range",
            ),
            "invalid_option",
        ),
        ("scope_denied", scope_denied(), "scope_denied"),
        ("rate_limited", rate_limited(), "rate_limited"),
        (
            "recording_in_progress",
            recording_in_progress_response(),
            "recording_in_progress",
        ),
    ];

    for (name, resp, identifier) in cases {
        let seen = as_client_sees_it(resp).await;
        assert!(
            seen.contains(identifier),
            "`{name}` envelope reaches the client as {seen:?}, which does not name `{identifier}`"
        );
        assert!(
            !seen.starts_with("daemon returned HTTP"),
            "`{name}` envelope fell back to the bare status ({seen:?}) — the client cannot read \
             the shape it emits"
        );
    }
}

/// `dispatch_command` answers on the `DaemonResponse` envelope, whose failures
/// carry a typed `ErrorCode` and a human message. Both halves must arrive.
#[tokio::test]
async fn dispatch_error_reaches_the_client_with_code_and_message() {
    let resp = DaemonResponse::error("No installed backend serves that model")
        .with_error_code(ErrorCode::InvalidModel);
    let (status, headers, body) = json_response(&resp);
    let seen = error_for_status(status, body.as_bytes()).to_string();

    assert_eq!(headers[0].1, "application/json");
    assert!(
        seen.contains("invalid_model"),
        "the machine-readable code must survive: {seen:?}"
    );
    assert!(
        seen.contains("No installed backend serves that model"),
        "the human message must survive: {seen:?}"
    );
}

/// A `401` is the one status the client must be able to route back to
/// re-authentication, so it keeps a typed variant rather than collapsing into
/// the operational-error text.
#[tokio::test]
async fn invalid_session_stays_routable_to_reauth() {
    let (status, body) = parts_of(invalid_session(reason::UNKNOWN)).await;
    let err = error_for_status(status, &body);

    assert!(
        err.is_invalid_session(),
        "401 must stay typed for the re-auth path, got: {err}"
    );
    assert_eq!(err.to_string(), "invalid_session (unknown)");
}
