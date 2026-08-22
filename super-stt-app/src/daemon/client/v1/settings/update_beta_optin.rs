// SPDX-License-Identifier: GPL-3.0-only
//! `/update_beta_optin` — whether the self-update candidate considers
//! prerelease versions (`auto` | `enabled` | `disabled`).

use crate::daemon::client::internal::response::require_unit;
use crate::daemon::client::internal::session::with_settings_token;
use super_stt_shared::daemon::http_client::HttpResult;
use super_stt_shared::daemon::http_client::transport;

/// Store the beta opt-in setting (HTTP `POST /update_beta_optin`).
/// Only a setter is needed — the toggler on the Updates page renders from
/// `SelfUpdateStatus::beta_optin_effective` instead of a separate GET.
pub async fn set_update_beta_optin(value: String) -> HttpResult<()> {
    with_settings_token(move |socket, token| {
        let value = value.clone();
        async move {
            let resp = transport::settings_post(
                socket,
                &token,
                "/update_beta_optin",
                &serde_json::json!({ "value": value }),
            )
            .await?;
            require_unit(resp, "set_update_beta_optin")
        }
    })
    .await
}
