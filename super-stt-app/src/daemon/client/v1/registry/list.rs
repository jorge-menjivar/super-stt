// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::client::internal::session::with_settings_token;
use crate::daemon::client::v1::registry::ListFilters;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::registry::RegistryListResponse;

/// `GET /registry/backends` — fetch the backend catalog, optionally filtered.
pub async fn list(filters: &ListFilters) -> HttpResult<RegistryListResponse> {
    let query = filters.to_query_string();
    with_settings_token(move |socket, token| {
        let query = query.clone();
        async move {
            let path = if query.is_empty() {
                "/registry/backends".to_string()
            } else {
                format!("/registry/backends?{query}")
            };
            transport::get_json::<RegistryListResponse>(socket, &token, &path).await
        }
    })
    .await
}
