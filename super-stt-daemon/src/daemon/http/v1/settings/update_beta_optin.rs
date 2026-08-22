// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_update_beta_optin,
    SetUpdateBetaOptinBody { value: String },
    "set_update_beta_optin",
    "value"
);
settings_dispatch!(get_update_beta_optin, "get_update_beta_optin");
