// SPDX-License-Identifier: GPL-3.0-only
//! `/notification_method` — how recording failures are surfaced (auto, dbus,
//! typed, off).
settings_getter!(
    get_notification_method -> String, "/notification_method", "get_notification_method",
    |resp| resp.notification_method.unwrap_or_else(|| "auto".to_string())
);
settings_setter!(
    set_notification_method,
    method: String,
    "/notification_method",
    "method",
    "set_notification_method"
);
