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

/// The auth-failure envelope the guards answer with, shared with Super TTS.
pub(crate) use super_engine_daemon::http::wire::ReasonEnvelope;

/// The registry surface's error envelope, shared with Super TTS. See
/// `super_engine_daemon::registry::endpoints::RegistryError`.
pub(crate) use super_engine_daemon::registry::endpoints::RegistryError;
