// SPDX-License-Identifier: GPL-3.0-only
//! `/preview_typing` — enable/disable live preview typing during recording.

settings_getter!(
    get_preview_typing -> bool, "/preview_typing", "get_preview_typing",
    |resp| resp.preview_typing_enabled.unwrap_or(false)
);
settings_setter!(
    set_preview_typing,
    enabled: bool,
    "/preview_typing",
    "enabled",
    "set_preview_typing"
);
