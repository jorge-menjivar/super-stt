// SPDX-License-Identifier: GPL-3.0-only
//! Text to speech's part of the `OpenAPI` document. The base document — the
//! title, servers, tags and auth scheme — is the daemon's, in
//! `crate::daemon::http::openapi`, which merges these paths in.

/// Restate each operation's required scope in its summary, where a reader can
/// actually see it.
///
/// The scope is already declared, in `security` on every `#[utoipa::path]`, and
/// that declaration is the authority — a contract test checks it against the
/// guard the route actually sits behind. What it is not is *visible*: Swagger UI
/// renders scopes only for `OAuth2` and `OpenID` Connect schemes, where it runs
/// the flow itself. Ours is a plain bearer scheme, so the UI shows a padlock,
/// drops the scope array, and leaves a reader to work out which of six scopes an
/// endpoint wants by reading prose.
///
/// So this copies it into the summary, which is the line shown beside the path
/// in the collapsed operation list — the one place you can compare endpoints
/// without opening them. Copied rather than written by hand in every summary:
/// a second hand-maintained statement of the same fact is one that goes stale,
/// and a summary claiming the wrong scope is worse than one that omits it.
///
/// Runs after assembly rather than as an [`ApiDoc`] modifier, because the paths
/// are merged in by the router *after* the base document is built — a modifier
/// here would walk an empty map.
pub(crate) fn annotate_scopes(openapi: &mut utoipa::openapi::OpenApi) {
    for item in openapi.paths.paths.values_mut() {
        let operations = [
            &mut item.get,
            &mut item.put,
            &mut item.post,
            &mut item.delete,
            &mut item.options,
            &mut item.head,
            &mut item.patch,
            &mut item.trace,
        ];
        for operation in operations.into_iter().flatten() {
            let note = scope_note(operation.security.as_ref());
            // Test against the note itself, not against a trailing `)`: a
            // summary may legitimately end in a parenthetical of its own, and
            // a looser check would silently leave those endpoints as the only
            // ones without their scope.
            if let Some(summary) = operation.summary.as_mut()
                && !summary.ends_with(&note)
            {
                summary.push_str(&note);
            }
        }
    }
}

/// How an operation's `security` reads in plain words.
///
/// `None` means the operation declared none, which is the genuinely
/// unauthenticated case: `POST /v1/auth/request`, the endpoint that mints the
/// token every other one needs. An empty scope list means any valid token will
/// do whatever its scopes — `/v1/events` is that, because its scopes are
/// per-topic and enforced inside the handler against the topics asked for.
fn scope_note(security: Option<&Vec<utoipa::openapi::security::SecurityRequirement>>) -> String {
    let Some(requirements) = security else {
        return " (no token needed)".to_string();
    };
    // `SecurityRequirement` keeps its map private, so read it back the way it is
    // written out. This runs once, in a generator, not on a request path.
    let scopes: Vec<String> = requirements
        .iter()
        .filter_map(|requirement| serde_json::to_value(requirement).ok())
        .filter_map(|value| value.get("session_token").cloned())
        .filter_map(|scopes| serde_json::from_value::<Vec<String>>(scopes).ok())
        .flatten()
        .collect();

    if scopes.is_empty() {
        " (any valid token)".to_string()
    } else {
        format!(" (scope: {})", scopes.join(", "))
    }
}
