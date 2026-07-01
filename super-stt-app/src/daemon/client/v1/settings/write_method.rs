// SPDX-License-Identifier: GPL-3.0-only
//! `/write_method` — text-output strategy (auto, xdotool, clipboard, …).

settings_getter!(
    get_write_method -> String, "/write_method", "get_write_method",
    |resp| resp.write_method.unwrap_or_else(|| "auto".to_string())
);
settings_setter!(set_write_method, method: String, "/write_method", "method", "set_write_method");
