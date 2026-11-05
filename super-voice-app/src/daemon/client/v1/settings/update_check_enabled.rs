// SPDX-License-Identifier: GPL-3.0-only
//! `/update_check_enabled` — periodic self-update check + notification toggle.

settings_getter!(
    get_update_check_enabled -> bool, "/settings/update_check_enabled", "get_update_check_enabled",
    |resp| resp.update_check_enabled.unwrap_or(true)
);
settings_setter!(
    set_update_check_enabled,
    enabled: bool,
    "/settings/update_check_enabled",
    "enabled",
    "set_update_check_enabled"
);
