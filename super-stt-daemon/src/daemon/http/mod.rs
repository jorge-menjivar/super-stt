// SPDX-License-Identifier: GPL-3.0-only
//! HTTP server for the daemon protocol. See `server.rs` for the listener
//! and `v1/` for the versioned endpoint tree.

#[cfg(test)]
mod error_envelope_contract;
mod internal;
mod openapi;
#[cfg(test)]
mod openapi_contract;
mod server;
mod state;
mod v1;
mod wire;

/// Re-exported so the daemon's shutdown path can drain queued session-store
/// writes before `process::exit` (audit 2 Tier 1 #5) without reaching into the
/// private `internal` module tree.
pub(crate) use internal::auth::tokens::flush_persisted_sessions;
pub use server::AUTO_APPROVE_ENV;
pub use server::start_http_server;

/// The generated `OpenAPI` document for the `/v1` surface. Built from the same
/// route registrations the live router uses, so it needs no daemon and no
/// keyring — see `src/bin/gen_openapi.rs`.
#[must_use]
pub fn openapi_document() -> utoipa::openapi::OpenApi {
    v1::openapi()
}
