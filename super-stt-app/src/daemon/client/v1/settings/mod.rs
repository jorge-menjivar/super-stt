// SPDX-License-Identifier: GPL-3.0-only
//! Per-setting daemon endpoint wrappers.
//!
//! Most settings are a plain `GET`/`POST` pair over the settings token, so the
//! two macros below generate the boilerplate: `settings_getter!` reads a value
//! out of the response, `settings_setter!` posts `{ key: value }`. Endpoints
//! with richer shapes (e.g. `active_model`, `backends`, `language`) stay
//! hand-written.

/// Generate a settings **getter**: `GET <path>`, require success, then run
/// `|resp| <extract>` over the response body to produce the return value.
macro_rules! settings_getter {
    ($fn:ident -> $ret:ty, $path:literal, $label:literal, |$resp:ident| $extract:expr $(,)?) => {
        pub async fn $fn() -> Result<$ret, String> {
            crate::daemon::client::internal::session::with_settings_token(
                |socket, token| async move {
                    let $resp = crate::daemon::client::internal::response::require_success(
                        super_stt_shared::daemon::http_client::transport::settings_get(
                            socket, &token, $path,
                        )
                        .await?,
                        $label,
                    )?;
                    Ok($extract)
                },
            )
            .await
        }
    };
}

/// Generate a settings **setter**: `POST <path>` with `{ key: value }`, then
/// require success (unit result).
macro_rules! settings_setter {
    ($fn:ident, $param:ident : $ty:ty, $path:literal, $key:literal, $label:literal $(,)?) => {
        pub async fn $fn($param: $ty) -> Result<(), String> {
            // Build the body once (consuming the param); the retrying `Fn`
            // closure clones the `Value` each call — cheap and uniform for
            // `Copy` and owned params alike (a blanket `$param.clone()` would
            // trip `clone_on_copy` on e.g. `bool`).
            let body = serde_json::json!({ $key: $param });
            crate::daemon::client::internal::session::with_settings_token(
                move |socket, token| {
                    let body = body.clone();
                    async move {
                        let resp =
                            super_stt_shared::daemon::http_client::transport::settings_post(
                                socket, &token, $path, &body,
                            )
                            .await?;
                        crate::daemon::client::internal::response::require_unit(resp, $label)
                    }
                },
            )
            .await
        }
    };
}

pub(crate) mod active_device;
pub(crate) mod active_model;
pub(crate) mod allow_online_models;
pub(crate) mod audio_theme;
pub(crate) mod backend_secrets;
pub(crate) mod backends;
pub(crate) mod custom_models_dir;
pub(crate) mod language;
pub(crate) mod preview_typing;
pub(crate) mod recording_stop_mode;
pub(crate) mod volume;
pub(crate) mod write_method;
