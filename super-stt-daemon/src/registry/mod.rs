// SPDX-License-Identifier: GPL-3.0-only
//! Daemon-side registry client, compatibility evaluation, and install pipeline.

pub mod carry_over;
pub mod client;
pub mod compat;
pub mod custom_repo;
pub mod host_detect;
pub mod index_schema;
pub mod install;
pub mod installed;
pub mod local_dir;
pub mod reconcile;

/// Re-export the shared operator-base-URL gate from `super-stt-forge` so the
/// registry client and the forge adapters apply one identical rule.
pub(crate) use super_stt_forge::accept_base_url;

/// The directory name a backend installs into.
///
/// The reverse-DNS `[backend].id` when the entry carries one, so every install
/// route — registry, custom repository, local directory — lands on the same
/// path for the same backend. Falls back to the registry key for an entry that
/// predates the identifier, which is where such a backend is already
/// installed.
///
/// `backend_id` arrives from `index.json` over the network. This function
/// does not assume the registry-client boundary (`retain_safe_backends`)
/// already sanitized it: it re-checks the value itself and falls back to the
/// registry key whenever `backend_id` is absent or malformed.
///
/// The check is the full `[backend].id` format rule
/// ([`super_stt_registry_types::backend_id::is_valid`]), not merely "usable
/// as a path component". `Manifest::parse` already holds every other route
/// to that rule, so anything looser here would make `index.json` the one
/// input the daemon accepts below its own contract — and the gap is not
/// theoretical: `.staging` is a perfectly good path component but names the
/// shared staging root every install writes through.
#[must_use]
pub fn install_dir_name(entry: &index_schema::IndexBackend) -> &str {
    match entry.backend_id.as_deref() {
        Some(id) if super_stt_registry_types::backend_id::is_valid(id) => id,
        _ => &entry.id,
    }
}
