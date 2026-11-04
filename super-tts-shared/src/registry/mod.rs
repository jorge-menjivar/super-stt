// SPDX-License-Identifier: GPL-3.0-only
//! Wire types for `/registry/backends` and friends. All fields `snake_case`.
//!
//! They are `super_engine_spec::registry`'s, shared with Super STT, with the
//! ones that carry model entries bound to Super TTS's index fields. The
//! uninstall reply is Super TTS's own.

use serde::{Deserialize, Serialize};

pub use super_engine_spec::registry::{
    Compatibility, IndexStale, InstallAccepted, InstallRequest, RefreshResponse, RegistryOption,
    RegistrySecret, SelectedAsset, UpdateRequest, UpdateResponse, events,
};
pub use super_tts_registry_types::index::IndexModel as RegistryModel;

/// The registry listing, with Super TTS's model fields.
pub type RegistryListResponse = super_engine_spec::registry::RegistryListResponse<RegistryBackend>;
/// One backend in [`RegistryListResponse`].
pub type RegistryBackend = super_engine_spec::registry::RegistryBackend<RegistryModel>;
/// What a source would install.
pub type PreviewResponse = super_engine_spec::registry::PreviewResponse<RegistryBackend>;

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallResponse {
    pub uninstalled: bool,
    pub was_active: bool,
}

pub use super_tts_registry_types::{is_safe_component, is_safe_relative_path};
