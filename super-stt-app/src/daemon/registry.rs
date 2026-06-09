// SPDX-License-Identifier: GPL-3.0-only
//! Public re-export of the registry client (implemented under
//! [`crate::daemon::client::v1::registry`]). Kept as a stable module path
//! for call sites that use `crate::daemon::registry::*`.
pub use crate::daemon::client::v1::registry::{
    ListFilters, install_by_local_path, install_by_repo_url, install_by_source, list, refresh,
    uninstall, update,
};
