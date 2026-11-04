// SPDX-License-Identifier: GPL-3.0-only
//! `/pipeline/{stage}/backend/list` — the backends that can fill a stage.
//!
//! The slot itself is [`super::stage`], one level up: `GET` reports the backend
//! filling the position and `POST` chooses it. This is the menu that `POST`
//! will accept — the same relationship [`super::model`] has with its
//! `/model/list`, and [`super::device`] with its `/device/list`.
//!
//! Distinct from [`crate::daemon::client::v1::backends::list_backends`], which
//! is every backend installed. `POST /pipeline/{stage}` refuses a backend that
//! serves nothing this stage can run, so a picker built from the general
//! catalog offers choices the daemon then rejects — an error the user only
//! discovers by making the pick. The two lists hold the same backends while
//! synthesis is the only role, which is exactly why the wrong one is easy to
//! reach for and would go unnoticed until a second stage exists.

use crate::daemon::backends::BackendInfo;
use crate::daemon::client::internal::response::require_success;
use crate::daemon::client::internal::session::with_settings_token;
use super_tts_shared::daemon::http_client::transport;
use super_tts_shared::daemon::http_client::{HttpError, HttpResult};

/// The installed backends `stage` can be filled with
/// (HTTP `GET /pipeline/{stage}/backend/list`).
///
/// Each carries its models, voices, options and secrets, exactly as the full
/// catalog reports them. Empty means nothing installed serves this stage —
/// the state a client renders as "install a backend" and sends the user on to
/// the registry for, rather than as an empty dropdown.
pub async fn list_stage_backends(stage: u32) -> HttpResult<Vec<BackendInfo>> {
    let path = format!("/pipeline/{stage}/backend/list");
    with_settings_token(move |socket, token| {
        let path = path.clone();
        async move {
            let resp = require_success(
                transport::settings_get(socket, &token, &path).await?,
                "list_stage_backends",
            )?;
            match resp.backends {
                Some(value) => serde_json::from_value(value)
                    .map_err(|e| HttpError::Other(format!("failed to parse backends: {e}"))),
                None => Ok(Vec::new()),
            }
        }
    })
    .await
}
