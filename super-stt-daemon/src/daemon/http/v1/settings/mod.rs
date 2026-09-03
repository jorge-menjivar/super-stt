// SPDX-License-Identifier: GPL-3.0-only
//! The `settings` scope: everything a client configures.
//!
//! Most endpoints here are one line apiece, because they are the same endpoint
//! with a different value in it: read a setting, write a setting, acknowledge.
//! The three macros below generate that endpoint — handler, request body, and
//! the `#[utoipa::path]` the `OpenAPI` document is built from — so a new setting
//! is one macro call rather than four things to keep in agreement.
//!
//! The path lives in the macro call and nowhere else: [`routes`] registers each
//! handler through `routes!`, which reads the path back off the attribute. A
//! setting cannot be served at one path and documented at another.

/// A no-body handler: dispatch `$cmd` and acknowledge.
///
/// Used for reads whose value rides in `message`, and for the `test` endpoints
/// that fire a cue and report what they did.
macro_rules! settings_dispatch {
    (
        $fn:ident, $cmd:literal, $method:ident $path:literal, $resp:ty,
        $summary:literal, $description:literal $(,)?
    ) => {
        #[utoipa::path(
            $method,
            path = $path,
            tag = "settings",
            summary = $summary,
            description = $description,
            security(("session_token" = ["settings"])),
            responses(
                (status = 200, description = "Done.", body = $resp),
                (status = 401, description = "Token unknown, expired, or its binary changed.",
                 body = $crate::daemon::http::wire::ReasonEnvelope),
                (status = 403, description = "The token lacks the `settings` scope.",
                 body = $crate::daemon::http::wire::ErrorEnvelope),
                (status = 429, description = "Per-client rate limit hit; back off and retry.",
                 body = $crate::daemon::http::wire::ErrorEnvelope),
            ),
        )]
        pub(crate) async fn $fn(
            ::axum::extract::State(s): ::axum::extract::State<
                $crate::daemon::http::state::AppState,
            >,
        ) -> ::axum::response::Response {
            use $crate::daemon::http::internal::helpers::dispatch;
            use $crate::daemon::http::v1::settings::wire::FromDaemon;
            let resp = dispatch::dispatch(&s.daemon, dispatch::build_request($cmd, None)).await;
            dispatch::narrowed(resp, <$resp>::from_daemon)
        }
    };
}

/// A single-field `POST`: deserialize `$body { $field: $ty }` and dispatch
/// `$cmd` with `{ $key: field }` in the request `data`.
macro_rules! settings_setter {
    (
        $fn:ident, $body:ident { $field:ident : $ty:ty }, $cmd:literal, $key:literal,
        $path:literal, $resp:ty, $summary:literal, $description:literal, $fielddoc:literal $(,)?
    ) => {
        #[doc = $summary]
        #[derive(::serde::Deserialize, ::utoipa::ToSchema)]
        pub(crate) struct $body {
            #[doc = $fielddoc]
            pub(crate) $field: $ty,
        }

        #[utoipa::path(
            post,
            path = $path,
            tag = "settings",
            summary = $summary,
            description = $description,
            request_body = $body,
            security(("session_token" = ["settings"])),
            responses(
                (status = 200, description = "Applied.", body = $resp),
                (status = 400, description = "The value was rejected — out of range, or not one of the accepted tokens.",
                 body = $crate::daemon::http::wire::ErrorEnvelope),
                (status = 401, description = "Token unknown, expired, or its binary changed.",
                 body = $crate::daemon::http::wire::ReasonEnvelope),
                (status = 403, description = "The token lacks the `settings` scope.",
                 body = $crate::daemon::http::wire::ErrorEnvelope),
                (status = 429, description = "Per-client rate limit hit; back off and retry.",
                 body = $crate::daemon::http::wire::ErrorEnvelope),
            ),
        )]
        pub(crate) async fn $fn(
            ::axum::extract::State(s): ::axum::extract::State<$crate::daemon::http::state::AppState>,
            ::axum::Json(body): ::axum::Json<$body>,
        ) -> ::axum::response::Response {
            use $crate::daemon::http::internal::helpers::dispatch;
            use $crate::daemon::http::v1::settings::wire::FromDaemon;
            let req = dispatch::build_request($cmd, Some(::serde_json::json!({ $key: body.$field })));
            let resp = dispatch::dispatch(&s.daemon, req).await;
            dispatch::narrowed(resp, <$resp>::from_daemon)
        }
    };
}

/// A boolean-toggle `POST`. These commands read `enabled` from the top level of
/// `DaemonRequest` rather than from `data`, so they build the request directly
/// instead of going through `ack`.
macro_rules! settings_toggle {
    (
        $fn:ident, $body:ident, $cmd:literal,
        $path:literal, $resp:ty, $summary:literal, $description:literal $(,)?
    ) => {
        #[doc = $summary]
        #[derive(::serde::Deserialize, ::utoipa::ToSchema)]
        pub(crate) struct $body {
            /// Whether the feature is on.
            pub(crate) enabled: bool,
        }

        #[utoipa::path(
            post,
            path = $path,
            tag = "settings",
            summary = $summary,
            description = $description,
            request_body = $body,
            security(("session_token" = ["settings"])),
            responses(
                (status = 200, description = "Applied.", body = $resp),
                (status = 401, description = "Token unknown, expired, or its binary changed.",
                 body = $crate::daemon::http::wire::ReasonEnvelope),
                (status = 403, description = "The token lacks the `settings` scope.",
                 body = $crate::daemon::http::wire::ErrorEnvelope),
                (status = 429, description = "Per-client rate limit hit; back off and retry.",
                 body = $crate::daemon::http::wire::ErrorEnvelope),
            ),
        )]
        pub(crate) async fn $fn(
            ::axum::extract::State(s): ::axum::extract::State<
                $crate::daemon::http::state::AppState,
            >,
            ::axum::Json(body): ::axum::Json<$body>,
        ) -> ::axum::response::Response {
            use $crate::daemon::http::internal::helpers::dispatch;
            use $crate::daemon::http::v1::settings::wire::FromDaemon;
            let mut req = dispatch::build_request($cmd, None);
            req.enabled = Some(body.enabled);
            let resp = dispatch::dispatch(&s.daemon, req).await;
            dispatch::narrowed(resp, <$resp>::from_daemon)
        }
    };
}

pub(crate) mod wire;

pub(crate) mod audio_theme;
pub(crate) mod backends;
pub(crate) mod custom_models_dir;
pub(crate) mod language;
pub(crate) mod models;
pub(crate) mod notification_method;
pub(crate) mod pipeline;
pub(crate) mod preview_typing;
pub(crate) mod recording_stop_mode;
pub(crate) mod self_update;
pub(crate) mod update_beta_optin;
pub(crate) mod update_check_enabled;
pub(crate) mod volume;
pub(crate) mod write_method;

use crate::daemon::http::state::AppState;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// All settings-scope routes. Registry and backend-option routes share the
/// settings scope, so they are merged in here.
///
/// No path appears in this function: `routes!` reads each one off the handler's
/// `#[utoipa::path]`, which is also what the `OpenAPI` document is generated from.
/// Handlers sharing a path are registered together, which is what makes them one
/// path item with several methods.
pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(models::list_models))
        .routes(routes!(
            audio_theme::get_audio_theme,
            audio_theme::set_audio_theme
        ))
        .routes(routes!(audio_theme::test_audio_theme))
        .routes(routes!(audio_theme::list_audio_themes))
        .routes(routes!(volume::get_volume, volume::set_volume))
        .routes(routes!(
            recording_stop_mode::get_recording_stop_mode,
            recording_stop_mode::set_recording_stop_mode
        ))
        .routes(routes!(
            write_method::get_write_method,
            write_method::set_write_method
        ))
        .routes(routes!(write_method::test_write_method))
        .routes(routes!(
            notification_method::get_notification_method,
            notification_method::set_notification_method
        ))
        .routes(routes!(pipeline::get_pipeline))
        .routes(routes!(
            pipeline::get_stage,
            pipeline::set_stage_backend,
            pipeline::clear_stage_backend
        ))
        .routes(routes!(
            pipeline::set_stage_model,
            pipeline::clear_stage_model
        ))
        .routes(routes!(pipeline::cancel_stage_model))
        .routes(routes!(pipeline::reload_stage_model))
        .routes(routes!(pipeline::list_stage_devices))
        .routes(routes!(
            pipeline::get_model_device,
            pipeline::set_model_device
        ))
        .routes(routes!(pipeline::list_model_devices))
        .routes(routes!(
            preview_typing::get_preview_typing,
            preview_typing::set_preview_typing
        ))
        .routes(routes!(
            custom_models_dir::get_custom_models_dir,
            custom_models_dir::set_custom_models_dir
        ))
        .routes(routes!(
            update_check_enabled::get_update_check_enabled,
            update_check_enabled::set_update_check_enabled
        ))
        .routes(routes!(
            update_beta_optin::get_update_beta_optin,
            update_beta_optin::set_update_beta_optin
        ))
        .routes(routes!(self_update::get_update))
        .routes(routes!(self_update::post_check))
        .routes(routes!(backends::list_backends))
        .routes(routes!(backends::uninstall_backend))
        .routes(routes!(backends::get_gpu_info))
        .routes(routes!(
            language::get_language,
            language::set_language,
            language::clear_language
        ))
        .merge(super::registry::routes())
        .merge(super::backends::options::routes())
        .merge(super::backends::model_language::routes())
}
