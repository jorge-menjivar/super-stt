// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/update_check_enabled` — whether the daemon looks for new releases
//! on its own schedule. The setting for the update machinery; the machinery
//! itself is `/update` and `/update/check`.

use super::super::wire::UpdateCheckEnabledState;

settings_toggle!(
    set_update_check_enabled,
    UpdateCheckEnabledBody,
    "set_update_check_enabled",
    "/settings/update_check_enabled",
    UpdateCheckEnabledState,
    "Turn the periodic update check on or off",
    "Controls whether the daemon checks for new Super TTS releases on its own schedule and \
raises a notification when it finds one. Turning it off stops both the check and the \
notification; it does not disable updating, since `POST /update/check` still runs a check on \
demand and `GET /update` still reports what the last one found. Which releases either check \
considers is `/settings/update_beta_optin`."
);
settings_dispatch!(
    get_update_check_enabled,
    "get_update_check_enabled",
    get "/settings/update_check_enabled",
    UpdateCheckEnabledState,
    "Read whether periodic update checks are on",
    "Answers with the current state; it defaults to on. Off means only that nothing happens \
unprompted — a client offering a \"Check now\" button should keep offering it, because \
`POST /update/check` works regardless of this setting."
);
