// SPDX-License-Identifier: GPL-3.0-only
//! GET /v1/update and POST /v1/update/check.
//! Contract: docs/protocol/endpoints/v1/update.md

use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::daemon::http_client::{HttpError, HttpResult};
use super_stt_shared::models::self_update::SelfUpdateStatus;

use crate::daemon::client::internal::session::with_settings_token;

/// `Ok(None)` when the daemon predates `/v1/update` (404).
pub async fn get_update_status() -> HttpResult<Option<SelfUpdateStatus>> {
    with_settings_token(|socket, token| async move {
        match transport::get_json::<SelfUpdateStatus>(socket, &token, "/update").await {
            Ok(s) => Ok(Some(s)),
            Err(e) if is_not_found(&e) => Ok(None),
            Err(e) => Err(e),
        }
    })
    .await
}

pub async fn check_update_now() -> HttpResult<SelfUpdateStatus> {
    with_settings_token(|socket, token| async move {
        transport::post_json::<SelfUpdateStatus>(
            socket,
            &token,
            "/update/check",
            &serde_json::json!({}),
        )
        .await
    })
    .await
}

/// True for a "route not found" 404 — the signal that the connected daemon
/// predates `/v1/update` entirely. `HttpError` has no typed status (only
/// `InvalidSession`/`AuthDenied` are distinguished at the transport layer;
/// see `transport.rs:128-151`, `daemon_error`), so this falls back to a
/// string match on the status `daemon_error` folds into the message.
///
/// An old daemon has no `/update` route at all, so axum's default fallback
/// (no JSON body) answers — `daemon_error` then has no `error_code`/`message`
/// to extract and produces the undetailed form, `"daemon returned HTTP
/// 404"` (no parentheses). A *known* route rejecting the request with a
/// classified 404 (unlikely for this endpoint, but `daemon_error` is shared
/// machinery) would instead produce `"{detail} (HTTP 404)"`. Matching on
/// `"HTTP 404"` without the parentheses covers both shapes.
fn is_not_found(e: &HttpError) -> bool {
    matches!(e, HttpError::Other(msg) if msg.contains("HTTP 404"))
}

#[cfg(test)]
mod tests {
    use super::is_not_found;
    use super_stt_shared::daemon::http_client::HttpError;

    /// The shape an *old* daemon (no `/update` route registered) actually
    /// produces: axum's default fallback has no body, so `daemon_error` has
    /// no `error_code`/`message` to report and falls back to the undetailed
    /// form.
    #[test]
    fn detects_undetailed_404() {
        assert!(is_not_found(&HttpError::Other(
            "daemon returned HTTP 404".to_string()
        )));
    }

    /// The shape a *known* route's classified 404 would produce, if this
    /// endpoint ever grew one.
    #[test]
    fn detects_detailed_404() {
        assert!(is_not_found(&HttpError::Other(
            "not_found (HTTP 404)".to_string()
        )));
    }

    #[test]
    fn does_not_match_other_statuses_or_variants() {
        assert!(!is_not_found(&HttpError::Other(
            "daemon returned HTTP 500".to_string()
        )));
        assert!(!is_not_found(&HttpError::InvalidSession {
            reason: "expired".to_string()
        }));
    }
}
