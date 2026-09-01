// SPDX-License-Identifier: GPL-3.0-only

// The settings handlers are thin wrappers over `dispatch_command`: build a
// `DaemonRequest`, hand it to the daemon, shape the `DaemonResponse` into an
// HTTP response. These three macros collapse that boilerplate. They are defined
// here — before the `mod` declarations below — so textual macro scope makes
// them visible inside every child module without an import.

/// A no-payload settings handler: dispatch `$cmd` with no request body. Used by
/// `GET` readers and verb-free actions (`test_audio_theme`, `clear_language`).
macro_rules! settings_dispatch {
    ($fn:ident, $cmd:literal) => {
        pub(crate) async fn $fn(
            ::axum::extract::State(s): ::axum::extract::State<
                $crate::daemon::http::state::AppState,
            >,
        ) -> impl ::axum::response::IntoResponse {
            $crate::daemon::http::internal::helpers::dispatch::dispatch_command(
                &s.daemon, $cmd, None,
            )
            .await
        }
    };
}

/// A single-field `POST` handler: deserialize `$body { $field: $ty }` and
/// dispatch `$cmd` with `{ $key: field }` in the request `data`.
macro_rules! settings_setter {
    ($fn:ident, $body:ident { $field:ident : $ty:ty }, $cmd:literal, $key:literal) => {
        #[derive(::serde::Deserialize)]
        pub(crate) struct $body {
            pub(crate) $field: $ty,
        }
        pub(crate) async fn $fn(
            ::axum::extract::State(s): ::axum::extract::State<$crate::daemon::http::state::AppState>,
            ::axum::Json(body): ::axum::Json<$body>,
        ) -> impl ::axum::response::IntoResponse {
            $crate::daemon::http::internal::helpers::dispatch::dispatch_command(
                &s.daemon,
                $cmd,
                Some(::serde_json::json!({ $key: body.$field })),
            )
            .await
        }
    };
}

/// A boolean-toggle `POST` handler. These legacy commands read `enabled` from
/// the top level of `DaemonRequest` (not from `data`), so they build the
/// request directly rather than via `dispatch_command`.
macro_rules! settings_toggle {
    ($fn:ident, $body:ident, $cmd:literal) => {
        #[derive(::serde::Deserialize)]
        pub(crate) struct $body {
            pub(crate) enabled: bool,
        }
        pub(crate) async fn $fn(
            ::axum::extract::State(s): ::axum::extract::State<
                $crate::daemon::http::state::AppState,
            >,
            ::axum::Json(body): ::axum::Json<$body>,
        ) -> impl ::axum::response::IntoResponse {
            use $crate::daemon::http::internal::helpers::dispatch;
            let mut req = dispatch::build_request($cmd, None);
            req.enabled = Some(body.enabled);
            let resp = dispatch::dispatch(&s.daemon, req).await;
            dispatch::json_response(&resp)
        }
    };
}

pub(crate) mod active_device;
pub(crate) mod allow_online_models;
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
use axum::Router;
use axum::routing::{delete, get, post};

/// All settings-scope routes. The registry sub-routes share the settings
/// scope, so they are merged in here.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/models", get(models::list_models))
        .route(
            "/active_device",
            get(active_device::get_active_device).post(active_device::set_active_device),
        )
        .route(
            "/audio_theme",
            get(audio_theme::get_audio_theme).post(audio_theme::set_audio_theme),
        )
        .route("/audio_theme/test", post(audio_theme::test_audio_theme))
        .route("/audio_themes", get(audio_theme::list_audio_themes))
        .route("/volume", get(volume::get_volume).post(volume::set_volume))
        .route(
            "/recording_stop_mode",
            get(recording_stop_mode::get_recording_stop_mode)
                .post(recording_stop_mode::set_recording_stop_mode),
        )
        .route(
            "/write_method",
            get(write_method::get_write_method).post(write_method::set_write_method),
        )
        .route("/write_method/test", post(write_method::test_write_method))
        .route(
            "/notification_method",
            get(notification_method::get_notification_method)
                .post(notification_method::set_notification_method),
        )
        .route("/pipeline", get(pipeline::get_pipeline))
        .route(
            "/pipeline/{stage}",
            get(pipeline::get_stage)
                .post(pipeline::set_stage_backend)
                .delete(pipeline::clear_stage_backend),
        )
        .route(
            "/pipeline/{stage}/model",
            post(pipeline::set_stage_model).delete(pipeline::clear_stage_model),
        )
        .route(
            "/pipeline/{stage}/model/cancel",
            post(pipeline::cancel_stage_model),
        )
        .route(
            "/pipeline/{stage}/model/reload",
            post(pipeline::reload_stage_model),
        )
        .route(
            "/preview_typing",
            get(preview_typing::get_preview_typing).post(preview_typing::set_preview_typing),
        )
        .route(
            "/allow_online_models",
            get(allow_online_models::get_allow_online_models)
                .post(allow_online_models::set_allow_online_models),
        )
        .route(
            "/custom_models_dir",
            get(custom_models_dir::get_custom_models_dir)
                .post(custom_models_dir::set_custom_models_dir),
        )
        .route(
            "/update_check_enabled",
            get(update_check_enabled::get_update_check_enabled)
                .post(update_check_enabled::set_update_check_enabled),
        )
        .route(
            "/update_beta_optin",
            get(update_beta_optin::get_update_beta_optin)
                .post(update_beta_optin::set_update_beta_optin),
        )
        .route("/update", get(self_update::get_update))
        .route("/update/check", post(self_update::post_check))
        .route("/backends", get(backends::list_backends))
        .route("/backends/{source}", delete(backends::uninstall_backend))
        .route("/gpu_info", get(backends::get_gpu_info))
        .route(
            "/language",
            get(language::get_language)
                .post(language::set_language)
                .delete(language::clear_language),
        )
        .merge(super::registry::routes())
        .merge(super::backends::options::routes())
        .merge(super::backends::model_language::routes())
}
