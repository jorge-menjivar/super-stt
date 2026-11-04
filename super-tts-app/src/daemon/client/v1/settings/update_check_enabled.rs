// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/update_check_enabled` — the periodic self-update check and its
//! notification.
//!
//! The getter defaults to `true` when the daemon omits the field: the check is
//! on unless someone turned it off, and defaulting the other way would leave a
//! user who never touched the setting quietly unaware of a security release.

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
