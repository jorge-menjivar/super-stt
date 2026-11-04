// SPDX-License-Identifier: GPL-3.0-only
//! Wire types for `/registry/backends` and friends. All fields `snake_case`.
//!
//! They are `super_engine_spec::registry`'s, with the ones that carry model
//! entries bound to Super Voice's index fields. The uninstall reply is the
//! daemon's, shared with speech to text.

pub use super_engine_spec::registry::{
    Compatibility, IndexStale, InstallAccepted, InstallRequest, RefreshResponse, RegistryOption,
    RegistrySecret, SelectedAsset, UpdateRequest, UpdateResponse, events,
};
pub use super_voice_registry_types::index::IndexModel as RegistryModel;

/// The registry listing, with Super Voice's model fields.
pub type RegistryListResponse = super_engine_spec::registry::RegistryListResponse<RegistryBackend>;
/// One backend in [`RegistryListResponse`].
pub type RegistryBackend = super_engine_spec::registry::RegistryBackend<RegistryModel>;
/// What a source would install.
pub type PreviewResponse = super_engine_spec::registry::PreviewResponse<RegistryBackend>;

/// What `DELETE /backend/{backend_id}` answers: the daemon's, which says
/// which feature's stages the backend was filling.
pub use super_stt_shared::registry::UninstallResponse;

pub use super_voice_registry_types::{is_safe_component, is_safe_relative_path};
