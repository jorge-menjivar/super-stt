// SPDX-License-Identifier: GPL-3.0-only
//! Wire types for `/registry/backend/list` and friends. All fields `snake_case`.
//!
//! They are `super_engine_spec::registry`'s, shared with Super TTS, with the
//! ones that carry model entries bound to Super STT's index fields. The
//! uninstall reply is Super STT's own: it says which of its two pipeline
//! stages the backend was filling.

use serde::{Deserialize, Serialize};

pub use super_engine_spec::registry::{
    Compatibility, IndexStale, InstallAccepted, InstallRequest, RefreshResponse, RegistryOption,
    RegistrySecret, SelectedAsset, UpdateRequest, UpdateResponse, events,
};
pub use super_stt_registry_types::index::IndexModel as RegistryModel;

/// The registry listing, with Super STT's model fields.
pub type RegistryListResponse = super_engine_spec::registry::RegistryListResponse<RegistryBackend>;
/// One backend in [`RegistryListResponse`].
pub type RegistryBackend = super_engine_spec::registry::RegistryBackend<RegistryModel>;
/// What a source would install.
pub type PreviewResponse = super_engine_spec::registry::PreviewResponse<RegistryBackend>;

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallResponse {
    pub uninstalled: bool,
    /// The backend was filling stage 1, which was emptied before the files
    /// went.
    pub was_active: bool,
    /// The backend was filling stage 2 — selected as the post-processor
    /// backend, loaded or not — which was emptied before the files went.
    /// Absent from an older daemon's answer, which reads as `false`.
    #[serde(default)]
    pub was_post_processor: bool,
}

pub use super_stt_registry_types::{is_safe_component, is_safe_relative_path};
