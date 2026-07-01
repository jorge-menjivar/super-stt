// SPDX-License-Identifier: GPL-3.0-only
//! `/allow_online_models` — gate for network-fetched model inference.

settings_setter!(
    set_allow_online_models,
    enabled: bool,
    "/allow_online_models",
    "enabled",
    "set_allow_online"
);
