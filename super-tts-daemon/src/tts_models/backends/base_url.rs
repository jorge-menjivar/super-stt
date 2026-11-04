// SPDX-License-Identifier: GPL-3.0-only
//! The `base_url` option — the convention for a backend's configurable
//! endpoint, and the one option whose value widens the sandbox.
//!
//! A configured value authorizes egress the SSRF guard would otherwise refuse,
//! so the daemon reads it from the user's config only: a `default` a
//! `backend.toml` declares for it is refused at publication, and dropped with a
//! warning if the backend was installed some other way. Deriving the endpoint
//! lives here rather than at either call site so the egress list the transport
//! enforces and the host the catalog discloses can never disagree about what a
//! given value means. See `docs/protocol/backend/config.md`.

/// Name of the option that carries a backend's configurable endpoint, from the
/// crate the daemon, the indexer, and the catalog synthesis all share.
pub(crate) const OPTION_NAME: &str = super_tts_registry_types::manifest::BASE_URL_OPTION;

/// Deriving the endpoint: `super_engine_daemon::wasm::base_url`, shared with
/// Super STT.
#[cfg(feature = "wasm-backends")]
pub(crate) use super_engine_daemon::wasm::base_url::normalize;
