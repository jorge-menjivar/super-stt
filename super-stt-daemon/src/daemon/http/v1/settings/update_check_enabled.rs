// SPDX-License-Identifier: GPL-3.0-only
use super::wire::UpdateCheckEnabledState;

settings_toggle!(
    set_update_check_enabled,
    UpdateCheckEnabledBody,
    "set_update_check_enabled",
    "/update_check_enabled",
    UpdateCheckEnabledState,
    "Turn the periodic update check on or off",
    "Controls whether the daemon checks for new Super STT releases on its own \
schedule. Turning it off does not disable updating \u{2014} `POST /update/check` still \
works on demand."
);
settings_dispatch!(
    get_update_check_enabled,
    "get_update_check_enabled",
    get "/update_check_enabled",
    UpdateCheckEnabledState,
    "Read whether periodic update checks are on",
    "Answers with the current state. Off stops the daemon checking on its own \
schedule; it does not disable updating, since `POST /update/check` still runs a \
check on demand."
);
