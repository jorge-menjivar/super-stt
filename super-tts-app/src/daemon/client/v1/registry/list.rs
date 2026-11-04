// SPDX-License-Identifier: GPL-3.0-only
//! `/registry/backend/list` — the catalog of backends available to install.
//!
//! What *could* be installed, as opposed to
//! [`crate::daemon::client::v1::backends::list_backends`], which is what
//! already is. The Download tab renders from this one and the Installed tab
//! from the other.

use crate::daemon::client::internal::session::with_settings_token;
use crate::daemon::client::v1::registry::ListFilters;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;
use super_tts_shared::registry::RegistryListResponse;

/// `GET /registry/backend/list` — fetch the backend catalog, optionally
/// filtered.
///
/// The filters go in the query string rather than being applied here so the
/// daemon decides compatibility: whether a backend can run on this host depends
/// on the assets it ships and the accelerators the machine has, which the app
/// does not know.
pub async fn list(filters: &ListFilters) -> HttpResult<RegistryListResponse> {
    let query = filters.to_query_string();
    with_settings_token(move |socket, token| {
        let query = query.clone();
        async move {
            let path = if query.is_empty() {
                "/registry/backend/list".to_string()
            } else {
                format!("/registry/backend/list?{query}")
            };
            transport::get_json::<RegistryListResponse>(socket, &token, &path).await
        }
    })
    .await
}
