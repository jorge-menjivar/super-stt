// SPDX-License-Identifier: GPL-3.0-only
//! Response-status helpers shared by every settings/registry call.
//!
//! The daemon answers settings reads/writes with a `DaemonResponse`
//! carrying `status: "success" | "error"` plus an optional human
//! `message`. These helpers collapse the repeated success check so each
//! endpoint wrapper reduces to "call transport, then read the field it
//! cares about".

use super_stt_shared::daemon::http_client::{HttpError, HttpResult};
use super_stt_shared::models::protocol::DaemonResponse;

/// Return `resp` when `status == "success"`, otherwise the daemon's `message`
/// (or `"<context> failed"` when absent) as an [`HttpError::Other`]. A daemon
/// `status:"error"` body is an operational failure, distinct from the transport
/// layer's typed `InvalidSession`/`AuthDenied`.
pub(crate) fn require_success(resp: DaemonResponse, context: &str) -> HttpResult<DaemonResponse> {
    if resp.status == "success" {
        Ok(resp)
    } else {
        Err(HttpError::Other(
            resp.message.unwrap_or_else(|| format!("{context} failed")),
        ))
    }
}

/// `require_success`, discarding the body — for writes that only need
/// success/failure.
pub(crate) fn require_unit(resp: DaemonResponse, context: &str) -> HttpResult<()> {
    require_success(resp, context).map(|_| ())
}

/// `require_success`, then return the daemon `message` (empty when absent)
/// — for writes whose caller surfaces the daemon's confirmation text.
pub(crate) fn require_message(resp: DaemonResponse, context: &str) -> HttpResult<String> {
    Ok(require_success(resp, context)?.message.unwrap_or_default())
}
