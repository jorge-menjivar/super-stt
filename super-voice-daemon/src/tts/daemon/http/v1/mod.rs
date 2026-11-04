// SPDX-License-Identifier: GPL-3.0-only
//! `/v1/tts` — text to speech's own routes, one module per path.
//!
//! What is text to speech's: speaking ([`speak`], [`speak_stream`]), cloned
//! voices ([`voice`]) and its pipeline ([`pipeline`]). Everything the daemon
//! has once — auth, events, status, backends, the registry, settings,
//! updates — is served at `/v1` by `crate::daemon::http::v1`, which merges
//! this router into its own.
//!
//! [`wire`] is named for something other than a path because it is not an
//! endpoint: it holds the narrow response bodies the routes answer with.

pub(crate) mod backends;
pub(crate) mod pipeline;
pub(crate) mod speak;
pub(crate) mod speak_stream;
pub(crate) mod voice;
pub(crate) mod wire;

use crate::tts::daemon::http::internal::auth::{require_speak_scope, require_voices_scope};
use crate::tts::daemon::http::state::AppState;
use axum::Router;
use axum::middleware;
use super_engine_daemon::auth::middleware::{require_rate_limit, require_settings_scope};
use utoipa_axum::router::OpenApiRouter;

/// The prefix text to speech's routes are served under.
pub(crate) const TTS: &str = "/tts";

/// The `/v1/tts` routes, grouped by the guard they share. Registering a route
/// and documenting it is one act: every entry goes in through `routes!`, which
/// reads the `#[utoipa::path]` attribute off the handler it names.
struct ScopeGroups {
    speak: OpenApiRouter<AppState>,
    /// The synthesis stage: what model and backend fill it, and how.
    settings: OpenApiRouter<AppState>,
    /// The cloned-voice library. Its own scope rather than part of `settings`:
    /// these are recordings of a person, and an app that manages models has no
    /// business reading them.
    voices: OpenApiRouter<AppState>,
}

/// Every `/v1/tts` route, in its group. Paths are bare here (`/speak`); the
/// prefix is applied once, in [`assemble`].
fn scope_groups() -> ScopeGroups {
    ScopeGroups {
        speak: speak::routes().merge(speak_stream::routes()),
        settings: pipeline::routes(),
        voices: voice::routes(),
    }
}

/// Apply each group's guard. Rate limiting is layered under the scope check, so
/// an unauthenticated flood is rejected on the cheaper test first.
fn guarded(groups: ScopeGroups, state: &AppState) -> ScopeGroups {
    macro_rules! guard {
        ($group:expr, $scope:expr) => {
            $group
                .layer(middleware::from_fn_with_state(state.auth.clone(), $scope))
                .layer(middleware::from_fn_with_state(
                    state.auth.clone(),
                    require_rate_limit,
                ))
        };
    }

    ScopeGroups {
        speak: guard!(groups.speak, require_speak_scope),
        settings: guard!(groups.settings, require_settings_scope),
        voices: guard!(groups.voices, require_voices_scope),
    }
}

/// Merge the groups under `/v1/tts`. The live router and the generated spec both
/// come through here, so the two cannot describe different route sets.
fn assemble(groups: ScopeGroups) -> OpenApiRouter<AppState> {
    let tts = OpenApiRouter::new()
        .merge(groups.speak)
        .merge(groups.settings)
        .merge(groups.voices);
    OpenApiRouter::new().nest(&format!("/v1{TTS}"), tts)
}

/// The live `/v1/tts` router, guards applied and state bound. The daemon's
/// router merges it and wraps both in the origin gate.
pub(crate) fn router(state: AppState) -> Router {
    let (router, _spec) = assemble(guarded(scope_groups(), &state)).split_for_parts();
    router.with_state(state)
}

/// The generated document for the same routes, to merge into the daemon's.
pub(crate) fn openapi() -> utoipa::openapi::OpenApi {
    let (_router, mut spec) = assemble(scope_groups()).split_for_parts();
    // Has to happen here, not as an `ApiDoc` modifier: the paths only exist
    // once the router has been assembled.
    crate::tts::daemon::http::openapi::annotate_scopes(&mut spec);
    spec
}

#[cfg(test)]
pub(crate) fn enforced_scopes() -> std::collections::BTreeMap<String, Option<Vec<&'static str>>> {
    let groups = scope_groups();
    let by_group = [
        (groups.speak, Some(vec!["speak"])),
        (groups.settings, Some(vec!["settings"])),
        (groups.voices, Some(vec!["voices"])),
    ];

    let mut enforced = std::collections::BTreeMap::new();
    for (router, scopes) in by_group {
        // The groups hold bare paths; `assemble` is what applies the prefix,
        // so apply it here too to match the published document.
        let (_router, spec) = router.split_for_parts();
        for path in spec.paths.paths.keys() {
            enforced.insert(format!("/v1{TTS}{path}"), scopes.clone());
        }
    }
    enforced
}
