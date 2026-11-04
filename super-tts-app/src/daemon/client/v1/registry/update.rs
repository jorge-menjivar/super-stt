// SPDX-License-Identifier: GPL-3.0-only
//! `/registry/backend/update` — move an installed backend to a newer release.
//!
//! Distinct from installing it again: the daemon picks the newest release this
//! build is compatible with rather than whatever the registry lists first, so a
//! backend is never updated into a version this daemon cannot load.

use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::HttpResult;
use super_tts_shared::daemon::http_client::transport;
use super_tts_shared::registry::UpdateResponse;

/// `POST /registry/backend/update` — update an installed backend to the
/// latest compatible release.
pub async fn update(source: &str) -> HttpResult<UpdateResponse> {
    let source = source.to_string();
    with_settings_token(move |socket, token| {
        let source = source.clone();
        async move {
            transport::post_json::<UpdateResponse>(
                socket,
                &token,
                "/registry/backend/update",
                &serde_json::json!({ "source": source }),
            )
            .await
        }
    })
    .await
}
