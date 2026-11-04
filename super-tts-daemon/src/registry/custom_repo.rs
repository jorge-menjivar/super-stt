// SPDX-License-Identifier: GPL-3.0-only
//! Resolve an arbitrary repo URL into an [`IndexBackend`] for the Custom-repo
//! install path: `super_engine_daemon::registry::custom_repo`, shared with
//! Super STT, for Super TTS's manifests.

use super_engine_forge::ForgeClient;

pub use super_engine_daemon::registry::custom_repo::ResolveError;

use crate::registry::index_schema::IndexBackend;

/// Resolve `repo_url`'s latest release into an index entry. See
/// `super_engine_daemon::registry::custom_repo::resolve`.
///
/// # Errors
/// See [`ResolveError`].
pub async fn resolve(
    client: &dyn ForgeClient,
    repo_url: &str,
) -> Result<IndexBackend, ResolveError> {
    super_engine_daemon::registry::custom_repo::resolve::<super_tts_registry_types::Tts>(
        client, repo_url,
    )
    .await
}
