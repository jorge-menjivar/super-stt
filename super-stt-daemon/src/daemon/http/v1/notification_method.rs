// SPDX-License-Identifier: GPL-3.0-only
use super::wire::NotificationMethodState;

settings_setter!(
    set_notification_method,
    SetNotificationMethodBody { method: String },
    "set_notification_method",
    "method",
    "/notification_method",
    NotificationMethodState,
    "Choose how failures are announced",
    "Selects how the daemon tells the user when something goes wrong \u{2014} a desktop \
notification, or nothing at all. This is about failures the user would otherwise \
never see, such as a transcript that could not be written to the focused window.",
    "One of the accepted `snake_case` method tokens. An unknown token is a `400`.",
);
settings_dispatch!(
    get_notification_method,
    "get_notification_method",
    get "/notification_method",
    NotificationMethodState,
    "Read how failures are announced",
    "Answers with the configured method. This governs only failures the user \
would otherwise never learn about \u{2014} a transcript that could not be written to the \
focused window, say \u{2014} not routine status, which rides on the event stream."
);
