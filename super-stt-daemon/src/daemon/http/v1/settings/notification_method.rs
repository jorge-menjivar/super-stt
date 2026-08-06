// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_notification_method,
    SetNotificationMethodBody { method: String },
    "set_notification_method",
    "method"
);
settings_dispatch!(get_notification_method, "get_notification_method");
