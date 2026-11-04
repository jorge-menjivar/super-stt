// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/update_beta_optin` — which release channel the self-updater
//! considers. About Super TTS itself; the backends under `/registry` are
//! versioned and updated separately.

use super::super::wire::UpdateBetaOptinState;

settings_setter!(
    set_update_beta_optin,
    SetUpdateBetaOptinBody { value: String },
    "set_update_beta_optin",
    "value",
    "/settings/update_beta_optin",
    UpdateBetaOptinState,
    "Choose whether updates include prereleases",
    "Selects which releases the update check will offer: `enabled` always considers \
prereleases, `disabled` never does, and `auto` opts in exactly when the running build is itself \
a prerelease — so someone already on a beta keeps getting betas without having said so twice. \
The three-way value is stored as written; what it resolved to for this build is reported as \
`beta_optin_effective` by `GET /update`. An unrecognized value is rejected with no state change: \
the degrade-to-`auto` fallback exists for a config file that has gone stale on disk, never for a \
wire write.",
    "One of the accepted `snake_case` opt-in tokens — `auto`, `enabled`, `disabled`. An unknown token is a `400`.",
);
settings_dispatch!(
    get_update_beta_optin,
    "get_update_beta_optin",
    get "/settings/update_beta_optin",
    UpdateBetaOptinState,
    "Read the update channel opt-in",
    "Answers with the stored token, `auto` included — not with what `auto` resolves to on this \
build, which is `beta_optin_effective` on `GET /update`. Read this one to render the setting and \
that one to explain the candidate a check found. This concerns Super TTS itself, not the \
backends under `/registry`, which carry their own versions."
);
