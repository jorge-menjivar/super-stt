// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/idle_unload` — seconds an unused model stays loaded; `0` keeps
//! models loaded.

settings_getter!(
    get_idle_unload -> u64, "/settings/idle_unload", "get_idle_unload",
    |resp| resp.idle_unload_after_secs.unwrap_or(0)
);
settings_setter!(
    set_idle_unload,
    after_secs: u64,
    "/settings/idle_unload",
    "after_secs",
    "set_idle_unload"
);
