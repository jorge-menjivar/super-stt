// SPDX-License-Identifier: GPL-3.0-only
//! The guard for the one scope Super STT adds to the ones every daemon has.
//! Tokens, consent and the other guards are `super_engine_daemon::auth`.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use super_engine_daemon::auth::Auth;
use super_engine_daemon::auth::middleware::require_scope;

/// The `transcribe` scope: record from the microphone and receive this app's
/// own transcriptions.
pub(crate) async fn require_transcribe_scope(
    State(auth): State<Auth>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope("transcribe", auth, headers, request, next).await
}
