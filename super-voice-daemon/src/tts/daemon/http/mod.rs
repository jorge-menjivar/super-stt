// SPDX-License-Identifier: GPL-3.0-only
//! Text to speech's routes, `/v1/tts`. The listener, auth and the routes the
//! daemon has once are `crate::daemon::http`'s, which merges these in.

#[cfg(test)]
mod error_envelope_contract;
mod internal;
mod openapi;
#[cfg(test)]
mod speak_resolution_contract;
pub(crate) mod state;
pub(crate) mod v1;
mod wire;

/// The generated `OpenAPI` document for `/v1/tts`. Built from the same route
/// registrations the live router uses, so it needs no daemon.
#[must_use]
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    v1::openapi()
}
