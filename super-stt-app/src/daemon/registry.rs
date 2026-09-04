// SPDX-License-Identifier: GPL-3.0-only
//! Public re-export of the registry client (implemented under
//! [`crate::daemon::client::v1::registry`]). Kept as a stable module path
//! for call sites that use `crate::daemon::registry::*`.
//!
//! `uninstall` is re-exported alongside them for the same reason, though it is
//! served at `DELETE /backends/{source}` and so lives with the other backend
//! endpoints in [`crate::daemon::client::v1::backends`]. Installing comes from
//! the registry; removing is a property of what is already installed.
pub use crate::daemon::client::v1::backends::uninstall;
pub use crate::daemon::client::v1::registry::{
    ListFilters, install_by_local_path, install_by_repo_url, install_by_source, list, refresh,
    update,
};
