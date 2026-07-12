// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::registry::UpdateResponse;

/// `POST /registry/backends/update` — update an installed backend to the
/// latest compatible release.
pub async fn update(source: &str) -> HttpResult<UpdateResponse> {
    let source = source.to_string();
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            transport::post_json::<UpdateResponse>(
                socket,
                &token,
                "/registry/backends/update",
                &serde_json::json!({ "source": source }),
            )
            .await
        }
    })
    .await
}
