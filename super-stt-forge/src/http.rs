// SPDX-License-Identifier: GPL-3.0-only
//! Shared `reqwest` client factory for every registry/forge/download call.
//!
//! Two shapes cover all use: [`short_client`] for small metadata/index fetches
//! (a tight overall timeout), and [`download_client`] for large streaming asset
//! downloads (no short overall cap, but a connect timeout that still fails fast
//! on an unreachable host). Both carry the workspace user-agent. Replaces five
//! ad-hoc builders that variously used `expect`/`unwrap`/`unwrap_or_default`, no
//! user-agent, and — in the indexer — no timeout at all.
//!
//! The rustls crypto provider must be installed by the binary before the first
//! request (these builders don't touch it); building the client itself never
//! makes a request.

use std::time::Duration;

/// Workspace user-agent, version-stamped. Sent on every request so forge/CDN
/// logs and rate-limiters can attribute traffic.
pub const USER_AGENT: &str = concat!("super-stt/", env!("CARGO_PKG_VERSION"));

/// Install the `ring` rustls crypto provider — the workspace uses
/// `reqwest`/`rustls` with no bundled provider, so every binary must install one
/// once before its first request (these client builders deliberately don't).
/// Idempotent: the first call wins, later ones are ignored.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Client for small, quick requests (release metadata, `index.json`, manifest
/// assets): a 20 s overall timeout and a bounded redirect chain.
///
/// # Panics
/// Panics only if `reqwest` cannot build a client with these default settings
/// (not expected on any supported platform).
#[must_use]
pub fn short_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("build short reqwest client")
}

/// Client for large streaming downloads (multi-GB subprocess bundles / model
/// files): a generous 1 h overall timeout so a slow multi-GB transfer isn't
/// cut off, with a 30 s connect timeout to still fail fast on an unreachable
/// host, and a slightly longer redirect chain.
///
/// # Panics
/// Panics only if `reqwest` cannot build a client with these default settings
/// (not expected on any supported platform).
#[must_use]
pub fn download_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_hours(1))
        .connect_timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("build download reqwest client")
}
