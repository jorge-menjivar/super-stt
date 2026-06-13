// SPDX-License-Identifier: GPL-3.0-only
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::transport;
use super_stt_shared::registry::UninstallResponse;

/// `DELETE /backends/{source}` — uninstall a backend.
pub async fn uninstall(source: &str) -> Result<UninstallResponse, String> {
    let encoded = urlencoding::encode(source).into_owned();
    with_settings_token(move |socket, token| {
        let encoded = encoded.clone();
        async move {
            transport::delete_json::<UninstallResponse>(
                socket,
                &token,
                &format!("/backends/{encoded}"),
            )
            .await
            .map_err(String::from)
        }
    })
    .await
}
