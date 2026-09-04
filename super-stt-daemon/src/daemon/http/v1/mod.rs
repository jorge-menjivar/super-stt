// SPDX-License-Identifier: GPL-3.0-only
//! `/v1` — one module per path, named for the path it answers on.
//!
//! A directory where a path has sub-resources worth separating ([`auth`],
//! [`backends`], [`pipeline`], [`registry`]); a file otherwise, holding that
//! path and any sub-path small enough to read beside it. Two modules are named
//! for something other than a path because they are not endpoints: [`macros`],
//! which generates the one-value settings handlers, and [`wire`], the narrow
//! response bodies they answer with.

// Must come first: `#[macro_use]` puts the settings-endpoint macros in scope for
// every module declared after it.
#[macro_use]
mod macros;

pub(crate) mod audio_theme;
pub(crate) mod audio_themes;
pub(crate) mod auth;
pub(crate) mod backends;
pub(crate) mod custom_models_dir;
pub(crate) mod events;
pub(crate) mod gpu_info;
pub(crate) mod language;
pub(crate) mod models;
pub(crate) mod notification_method;
pub(crate) mod ping;
pub(crate) mod pipeline;
pub(crate) mod preview_typing;
pub(crate) mod recording_stop_mode;
pub(crate) mod registry;
pub(crate) mod status;
pub(crate) mod transcribe;
pub(crate) mod update;
pub(crate) mod update_beta_optin;
pub(crate) mod update_check_enabled;
pub(crate) mod volume;
pub(crate) mod wire;
pub(crate) mod write_method;

use crate::daemon::http::internal::auth::middleware::{
    require_any_authenticated, require_rate_limit, require_secrets_scope, require_settings_scope,
    require_status_scope, require_transcribe_scope,
};
use crate::daemon::http::openapi::ApiDoc;
use crate::daemon::http::state::AppState;
use axum::Router;
use axum::middleware;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// The `/v1` routes, grouped by the guard they share.
///
/// Registering a route and documenting it is one act here: every entry goes in
/// through [`routes!`], which reads the `#[utoipa::path]` attribute off the
/// handler it names. A handler without one does not compile, and a path spelled
/// two ways cannot exist, because there is only one spelling.
///
/// The guards are applied separately, in [`router`], for a practical reason:
/// `from_fn_with_state` needs a live [`AppState`], and building one opens the
/// system keyring. Keeping route registration free of it is what lets
/// [`openapi`] generate the document from these same registrations without a
/// running daemon — see `src/bin/gen_openapi.rs`.
struct ScopeGroups {
    /// Any valid token, whatever its scopes. `GET /events` lives here because
    /// its per-topic scope is enforced inside the handler, against the topics
    /// actually requested.
    any: OpenApiRouter<AppState>,
    status: OpenApiRouter<AppState>,
    transcribe: OpenApiRouter<AppState>,
    /// The configuration surface: every module whose paths the `settings`
    /// scope guards, gathered by [`settings_routes`].
    settings: OpenApiRouter<AppState>,
    secrets: OpenApiRouter<AppState>,
    /// Reachable without a token — it is how a caller gets one.
    unauthenticated: OpenApiRouter<AppState>,
}

/// Every `/v1` route, in its group. Paths are bare here (`/ping`); the `/v1`
/// prefix is applied once, in [`assemble`].
fn scope_groups() -> ScopeGroups {
    ScopeGroups {
        any: OpenApiRouter::new()
            .routes(routes!(ping::ping))
            .routes(routes!(auth::status::auth_status))
            .merge(events::routes()),
        status: OpenApiRouter::new().routes(routes!(status::status)),
        transcribe: transcribe::routes(),
        settings: settings_routes(),
        secrets: backends::secrets::routes(),
        unauthenticated: OpenApiRouter::new().routes(routes!(auth::request::auth_request)),
    }
}

/// Every route the `settings` scope guards, gathered from the modules that hold
/// them. This is the one place the scope's membership is written down; the
/// paths themselves live in each module's `#[utoipa::path]`, which is what
/// `routes!` reads them back off.
///
/// The families with their own trees ([`backends`], [`pipeline`], [`registry`])
/// contribute their own `routes()`. [`backends::secrets`] is deliberately not
/// among them — those paths carry the `secrets` scope and are guarded
/// separately in [`scope_groups`].
fn settings_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(models::list_models))
        .routes(routes!(
            audio_theme::get_audio_theme,
            audio_theme::set_audio_theme
        ))
        .routes(routes!(audio_theme::test_audio_theme))
        .routes(routes!(audio_themes::list_audio_themes))
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
        .routes(routes!(update::get_update))
        .routes(routes!(update::post_check))
        .routes(routes!(gpu_info::get_gpu_info))
        .routes(routes!(
            language::get_language,
            language::set_language,
            language::clear_language
        ))
        .merge(backends::routes())
        .merge(pipeline::routes())
        .merge(registry::routes())
}

/// Apply each group's guard. Rate limiting is layered under the scope check on
/// every guarded group, so an unauthenticated flood is rejected on the cheaper
/// test first.
fn guarded(groups: ScopeGroups, state: &AppState) -> ScopeGroups {
    /// `.layer(scope).layer(rate_limit)` — the pair every guarded group takes.
    macro_rules! guard {
        ($group:expr, $scope:expr) => {
            $group
                .layer(middleware::from_fn_with_state(state.clone(), $scope))
                .layer(middleware::from_fn_with_state(
                    state.clone(),
                    require_rate_limit,
                ))
        };
    }

    ScopeGroups {
        any: guard!(groups.any, require_any_authenticated),
        status: guard!(groups.status, require_status_scope),
        transcribe: guard!(groups.transcribe, require_transcribe_scope),
        settings: guard!(groups.settings, require_settings_scope),
        secrets: guard!(groups.secrets, require_secrets_scope),
        // No guard: this is the endpoint that mints the token the others need.
        unauthenticated: groups.unauthenticated,
    }
}

/// Merge the groups under `/v1` and attach the base document. The live router
/// and the generated spec both come through here, so the two cannot describe
/// different route sets.
fn assemble(groups: ScopeGroups) -> OpenApiRouter<AppState> {
    let v1 = OpenApiRouter::new()
        .merge(groups.any)
        .merge(groups.status)
        .merge(groups.transcribe)
        .merge(groups.settings)
        .merge(groups.secrets)
        .merge(groups.unauthenticated);

    OpenApiRouter::with_openapi(ApiDoc::openapi()).nest("/v1", v1)
}

/// The live `/v1` router, guards applied and state bound.
pub(crate) fn router(state: AppState) -> Router {
    let (router, _spec) = assemble(guarded(scope_groups(), &state)).split_for_parts();
    router.with_state(state)
}

/// The generated document for the same surface. The guards are omitted because
/// they add no paths — what each endpoint requires is stated in its own
/// `security` and prose.
pub(crate) fn openapi() -> utoipa::openapi::OpenApi {
    let (_router, spec) = assemble(scope_groups()).split_for_parts();
    spec
}

/// Which scope the router *enforces* on each `/v1` path, read from the grouping
/// itself rather than restated.
///
/// `None` means the path is reachable without a token. `Some(vec![])` means any
/// valid token will do, whatever its scopes.
///
/// This exists so the contract test can check each endpoint's advertised
/// `security` against the guard it actually sits behind. The two are written in
/// different places — the scope in a `#[utoipa::path]` attribute, the guard in
/// [`guarded`] — and nothing else would notice them disagreeing. A client author
/// told they need `settings` for an endpoint the daemon guards with `secrets`
/// requests the wrong scope and gets a `403` they cannot explain.
#[cfg(test)]
pub(super) fn enforced_scopes() -> std::collections::BTreeMap<String, Option<Vec<&'static str>>> {
    let groups = scope_groups();
    let by_group = [
        (groups.any, Some(vec![])),
        (groups.status, Some(vec!["status"])),
        (groups.transcribe, Some(vec!["transcribe"])),
        (groups.settings, Some(vec!["settings"])),
        (groups.secrets, Some(vec!["secrets"])),
        (groups.unauthenticated, None),
    ];

    let mut enforced = std::collections::BTreeMap::new();
    for (router, scopes) in by_group {
        // The groups hold bare paths; `assemble` is what applies the `/v1`
        // prefix, so apply it here too to match the published document.
        let (_router, spec) = router.split_for_parts();
        for path in spec.paths.paths.keys() {
            enforced.insert(format!("/v1{path}"), scopes.clone());
        }
    }
    enforced
}
