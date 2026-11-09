// SPDX-License-Identifier: GPL-3.0-only
//! The guards for the scopes Super TTS adds to the ones every daemon has.
//! Tokens, consent and the other guards are `super_engine_daemon::auth`.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;
use super_engine_daemon::auth::Auth;
use super_engine_daemon::auth::middleware::require_scope;

/// The `speak` scope gates synthesis and playback control — taking over the
/// user's speakers, including interrupting speech another app started. Kept
/// separate from `settings` so configuring the daemon does not imply the
/// ability to make it talk.
pub(crate) async fn require_speak_scope(
    State(auth): State<Auth>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope("speak", auth, headers, request, next).await
}

/// The `voices` scope: the library of recordings voices are cloned from.
pub(crate) async fn require_voices_scope(
    State(auth): State<Auth>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Response {
    require_scope("voices", auth, headers, request, next).await
}
