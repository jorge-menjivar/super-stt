// SPDX-License-Identifier: GPL-3.0-only
//! The error bodies the guards and `/events` answer with are the engine's,
//! re-exported for the envelope contract tests, which hold every error shape
//! the daemon sends against the ones it documents.

#[cfg(test)]
pub(crate) use super_engine_daemon::http::responses::{
    invalid_session, rate_limited, reason, scope_denied,
};
