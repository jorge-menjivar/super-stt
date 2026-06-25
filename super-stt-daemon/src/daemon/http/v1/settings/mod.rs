// SPDX-License-Identifier: GPL-3.0-only
pub(crate) mod active_device;
pub(crate) mod active_model;
pub(crate) mod allow_online_models;
pub(crate) mod audio_theme;
pub(crate) mod backends;
pub(crate) mod custom_models_dir;
pub(crate) mod language;
pub(crate) mod preview_typing;
pub(crate) mod recording_stop_mode;
pub(crate) mod volume;
pub(crate) mod write_method;

use crate::daemon::http::state::AppState;
use axum::Router;
use axum::routing::{delete, get, post};

/// All settings-scope routes. The registry sub-routes share the settings
/// scope, so they are merged in here.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/active_model",
            get(active_model::get_active_model)
                .post(active_model::set_active_model)
                .delete(active_model::unload_active_model),
        )
        .route(
            "/active_model/cancel",
            post(active_model::cancel_set_active_model),
        )
        .route(
            "/active_model/reload",
            post(active_model::reload_active_model),
        )
        .route("/models", get(active_model::list_models))
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
        .route("/backends", get(backends::list_backends))
        .route("/backends/{source}", delete(backends::uninstall_backend))
        .route(
            "/active_backend",
            get(backends::get_active_backend)
                .post(backends::set_active_backend)
                .delete(backends::clear_active_backend),
        )
        .route("/gpu_info", get(backends::get_gpu_info))
        .route(
            "/language",
            get(language::get_language)
                .post(language::set_language)
                .delete(language::clear_language),
        )
        .route(
            "/active_model/language",
            get(language::get_active_model_language)
                .post(language::set_active_model_language)
                .delete(language::clear_active_model_language),
        )
        .merge(super::registry::routes())
        .merge(super::backends::options::routes())
}
