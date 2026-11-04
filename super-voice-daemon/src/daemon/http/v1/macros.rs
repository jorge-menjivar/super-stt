// SPDX-License-Identifier: GPL-3.0-only
//! The three macros that generate a one-value settings endpoint.
//!
//! Most of the settings surface is the same endpoint with a different value in
//! it: read a setting, write a setting, acknowledge. These generate that
//! endpoint — handler, request body, and the `#[utoipa::path]` the `OpenAPI`
//! document is built from — so a new setting is one macro call rather than four
//! things to keep in agreement.
//!
//! The path lives in the macro call and nowhere else: each module registers its
//! handler through `routes!`, which reads the path back off the attribute. A
//! setting cannot be served at one path and documented at another.
//!
//! Not an endpoint module, so it is not named for a path. `#[macro_use]` in
//! [`super`] is what puts these in scope for the modules declared after it.

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
            use $crate::daemon::http::v1::wire::FromDaemon;
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
            use $crate::daemon::http::v1::wire::FromDaemon;
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
            use $crate::daemon::http::v1::wire::FromDaemon;
            let mut req = dispatch::build_request($cmd, None);
            req.enabled = Some(body.enabled);
            let resp = dispatch::dispatch(&s.daemon, req).await;
            dispatch::narrowed(resp, <$resp>::from_daemon)
        }
    };
}
