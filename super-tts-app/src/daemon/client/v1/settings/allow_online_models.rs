// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/allow_online_models` — whether models that synthesize over the
//! network may be used at all.
//!
//! Off keeps every utterance on this machine: a model whose backend would send
//! text to a third party cannot be loaded while it is off. Only the setter is
//! wrapped — the current value arrives with the backend catalog, so a separate
//! read would be a second request for something already on screen.

settings_setter!(
    set_allow_online_models,
    enabled: bool,
    "/settings/allow_online_models",
    "enabled",
    "set_allow_online"
);
