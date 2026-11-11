// SPDX-License-Identifier: GPL-3.0-only
//! `/read_method` — where the read-aloud shortcut reads from: the selected
//! text or the clipboard.

settings_getter!(
    get_read_method -> String, "/settings/read_method", "get_read_method",
    |resp| resp
        .read_method
        .unwrap_or_else(|| "selection".to_string())
);
settings_setter!(
    set_read_method,
    method: String,
    "/settings/read_method",
    "method",
    "set_read_method"
);
