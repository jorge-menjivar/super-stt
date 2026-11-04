// SPDX-License-Identifier: GPL-3.0-only
use super::super::wire::UpdateBetaOptinState;

settings_setter!(
    set_update_beta_optin,
    SetUpdateBetaOptinBody { value: String },
    "set_update_beta_optin",
    "value",
    "/settings/update_beta_optin",
    UpdateBetaOptinState,
    "Choose whether updates include prereleases",
    "Selects which release channel the update check considers. Opting in offers beta \
builds as they are published; opting out considers stable releases only.",
    "One of the accepted `snake_case` opt-in tokens. An unknown token is a `400`.",
);
settings_dispatch!(
    get_update_beta_optin,
    "get_update_beta_optin",
    get "/settings/update_beta_optin",
    UpdateBetaOptinState,
    "Read the update channel opt-in",
    "Answers with the configured opt-in \u{2014} whether the update check considers beta \
builds or stable releases only. This is about Super STT itself, not about the \
backends under `/registry`, which are versioned separately."
);
