// SPDX-License-Identifier: GPL-3.0-only
settings_toggle!(
    set_allow_online_models,
    AllowOnlineModelsBody,
    "set_allow_online_models"
);
settings_dispatch!(get_allow_online_models, "get_allow_online_models");
