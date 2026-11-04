// SPDX-License-Identifier: GPL-3.0-only
//! `/registry/backend/*` — the published catalog and the acts that change what
//! is installed from it: list, preview, install, refresh, update.
//!
//! Removing an installed backend is not here. It is served at
//! `DELETE /backend/{source}` and lives in
//! [`crate::daemon::client::v1::backends`], because it is a property of what is
//! already on disk: installing comes from the registry, and a backend imported
//! from a local directory was never in the registry to be removed from it.

mod install;
mod list;
mod preview;
mod refresh;
mod update;

pub use install::{install_by_local_path, install_by_repo_url, install_by_source};
pub use list::list;
pub use preview::{preview_by_local_path, preview_by_repo_url};
pub use refresh::refresh;
pub use update::update;

/// Filters for the registry list endpoint.
#[derive(Debug, Clone, Default)]
pub struct ListFilters {
    pub include_incompatible: Option<bool>,
    pub kind: Option<String>,
    pub online: Option<bool>,
    pub q: Option<String>,
}

impl ListFilters {
    /// Encode the active filters as a query string (no leading `?`).
    /// Returns an empty string when no filters are set.
    pub fn to_query_string(&self) -> String {
        let mut pairs: Vec<String> = Vec::new();
        if let Some(b) = self.include_incompatible {
            pairs.push(format!("include_incompatible={b}"));
        }
        if let Some(k) = &self.kind {
            pairs.push(format!("kind={}", urlencoding::encode(k)));
        }
        if let Some(o) = self.online {
            pairs.push(format!("online={o}"));
        }
        if let Some(qq) = &self.q {
            pairs.push(format!("q={}", urlencoding::encode(qq)));
        }
        pairs.join("&")
    }
}
