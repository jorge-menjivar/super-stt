// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/notification_method` — how a synthesis failure is surfaced to the
//! person at the keyboard, over and above the error the caller already gets.

use super::super::wire::NotificationMethodState;

settings_setter!(
    set_notification_method,
    SetNotificationMethodBody { method: String },
    "set_notification_method",
    "method",
    "/settings/notification_method",
    NotificationMethodState,
    "Choose how synthesis failures are announced",
    "Selects the additional, human-facing notice raised when synthesis fails — a desktop \
notification (`auto`), or nothing beyond the log (`off`). The caller is told either way, through \
the error response or an `error` frame on `/speak/stream`; this setting is about the user, who \
may not be the caller and may be looking at a different window entirely. Only failures the \
caller did not cause raise a notice: malformed input gets its coded `400` and no popup, since \
whoever sent it is by definition reading the response. Takes effect on the next utterance.",
    "One of the accepted `snake_case` method tokens — `auto` or `off`. An unknown token is a `400`.",
);
settings_dispatch!(
    get_notification_method,
    "get_notification_method",
    get "/settings/notification_method",
    NotificationMethodState,
    "Read how synthesis failures are announced",
    "Answers with the configured method. It governs only the extra notice for failures the user \
would otherwise never learn about — no model loaded, the output device refused to open, the \
backend returned no audio — not routine status, which rides on `GET /events`, and not the error \
the caller already receives."
);
