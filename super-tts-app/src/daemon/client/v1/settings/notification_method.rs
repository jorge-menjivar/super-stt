// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/notification_method` — how synthesis failures are surfaced
//! (`auto`, `off`).
//!
//! The default is `auto` rather than `off`, and the getter's fallback says so:
//! a daemon that answers without the field is one that has never had it set,
//! and reading that as "notifications are off" would silently swallow the
//! errors this setting exists to show.

settings_getter!(
    get_notification_method -> String, "/settings/notification_method", "get_notification_method",
    |resp| resp.notification_method.unwrap_or_else(|| "auto".to_string())
);
settings_setter!(
    set_notification_method,
    method: String,
    "/settings/notification_method",
    "method",
    "set_notification_method"
);
