// SPDX-License-Identifier: GPL-3.0-only
pub(crate) mod install;
pub(crate) mod list;
pub(crate) mod refresh;
pub(crate) mod update;

use crate::daemon::http::state::AppState;
use axum::Router;
use axum::routing::{get, post};

/// Registry browse/refresh/install/update routes (settings-scope).
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/registry/backends", get(list::list_registry_backends))
        .route(
            "/registry/backends/refresh",
            post(refresh::refresh_registry),
        )
        .route(
            "/registry/backends/install",
            post(install::install_registry_backend),
        )
        .route(
            "/registry/backends/update",
            post(update::update_registry_backend),
        )
}
