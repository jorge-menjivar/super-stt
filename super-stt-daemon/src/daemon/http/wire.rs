// SPDX-License-Identifier: GPL-3.0-only
//! The envelopes many `/v1` endpoints answer with.
//!
//! Internally the daemon passes one [`DaemonResponse`] around: a single struct
//! carrying every field any command might return, each `Option` and each
//! skipped when absent. That is the right shape for a command bus — one type
//! crosses it — but it is the wrong shape to *document*, because a schema
//! generated from it tells a client that `GET /volume` may return forty-two
//! fields when it returns two.
//!
//! So the HTTP layer, which is the protocol boundary, names what each endpoint
//! actually answers with. The types here are the shapes shared across many
//! endpoints; a response peculiar to one endpoint is declared next to that
//! handler instead.
//!
//! [`DaemonResponse`]: super_stt_shared::models::protocol::DaemonResponse

use serde::Serialize;
use super_stt_shared::models::protocol::ErrorCode;
use utoipa::ToSchema;

/// The plain acknowledgement: an operation succeeded, with a sentence saying
/// what happened.
///
/// A good number of settings endpoints answer with exactly this, on `GET` as
/// well as `POST` — `GET /volume` reports the level inside `message` rather
/// than as a field of its own. Where that is true the endpoint's own
/// documentation says so, because a client has to parse the number back out.
#[derive(Serialize, ToSchema)]
pub(crate) struct Ack {
    /// Always `success`.
    #[schema(example = "success")]
    pub(crate) status: &'static str,
    /// Human-readable detail. Not a stable identifier — do not switch on it.
    ///
    /// Optional because a few commands acknowledge without a sentence, and the
    /// key is then absent rather than empty. Every endpoint documented as
    /// carrying its value in the message always sets it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

/// The error envelope, as [`transport.md`] specifies it.
///
/// `error_code` is the stable, machine-readable identifier clients switch on
/// and the field that determines the HTTP status; `message` is prose for a
/// human and may be reworded at any time.
///
/// [`transport.md`]: https://github.com/jorge-menjivar/super-stt/blob/main/docs/protocol/transport.md
#[derive(Serialize, ToSchema)]
pub(crate) struct ErrorEnvelope {
    /// Always `error`.
    #[schema(example = "error")]
    pub(crate) status: &'static str,
    /// The stable identifier for this failure. Absent only on an unclassified
    /// server-side error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error_code: Option<ErrorCode>,
    /// Human-readable detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}

/// The auth-failure envelope: `message` names the failure and `data.reason`
/// says which of its cases occurred.
///
/// Distinct from [`ErrorEnvelope`] because the auth surface predates
/// `error_code` and clients read `data.reason` there. Both are documented
/// rather than reconciled, since changing either is a breaking wire change.
#[derive(Serialize, ToSchema)]
pub(crate) struct ReasonEnvelope {
    /// Always `error`.
    #[schema(example = "error")]
    pub(crate) status: &'static str,
    /// The failure identifier, e.g. `invalid_session` or `auth_denied`.
    pub(crate) message: String,
    pub(crate) data: Reason,
}

/// The `data` object of a [`ReasonEnvelope`].
#[derive(Serialize, ToSchema)]
pub(crate) struct Reason {
    /// Which case of `message` occurred — e.g. `expired`, `exe_changed`,
    /// `user_denied`.
    pub(crate) reason: String,
}

/// The registry surface's error envelope, which carries one extra key.
///
/// These endpoints shipped before `error_code` existed, spelling the failure
/// identity as `error`. That key is still sent, because clients read it; the
/// standard `error_code` was added alongside rather than in place of it, so the
/// whole surface honors the "`error_code` on every error" rule without breaking
/// anyone. Both name the same failure and always agree.
///
/// Documented as its own shape rather than folded into [`ErrorEnvelope`]:
/// `error` appears *only* here, and putting it on the shared envelope would
/// tell every other endpoint's reader to expect a key they will never receive.
#[derive(Serialize, ToSchema)]
pub(crate) struct RegistryError {
    /// Always `error`.
    #[schema(example = "error")]
    pub(crate) status: &'static str,
    /// The stable identifier for this failure.
    #[schema(example = "not_found")]
    pub(crate) error_code: String,
    /// The same identifier under the key this surface has always used. Retained
    /// for clients written against it; prefer `error_code`.
    #[schema(example = "not_found")]
    pub(crate) error: String,
    /// Human-readable detail, when there is any to add.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
}
