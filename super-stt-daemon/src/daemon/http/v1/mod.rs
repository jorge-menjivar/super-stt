// SPDX-License-Identifier: GPL-3.0-only
pub(crate) mod auth;
pub(crate) mod backends;
pub(crate) mod events;
pub(crate) mod health;
pub(crate) mod registry;
pub(crate) mod settings;
pub(crate) mod transcribe;

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
    /// The configuration surface, which the registry and backend-option routes
    /// share.
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
            .routes(routes!(health::ping))
            .routes(routes!(auth::status::auth_status))
            .merge(events::routes()),
        status: OpenApiRouter::new().routes(routes!(health::status)),
        transcribe: transcribe::routes(),
        settings: settings::routes(),
        secrets: backends::secrets::routes(),
        unauthenticated: OpenApiRouter::new().routes(routes!(auth::request::auth_request)),
    }
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
