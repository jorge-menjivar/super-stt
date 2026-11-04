// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/allow_online_models` — the privacy gate on backends that
//! synthesize over the network.
//!
//! The one preference here that can change what is *running*. Every other
//! setting stores a value and stops; turning this one off while a hosted model
//! is loaded has to do something about that model, because leaving it up would
//! mean the gate reads "closed" while text is still leaving the machine. The
//! daemon reverts the synthesis stage to a local model, or empties it when the
//! host has no local backend installed, and says which in `message` — that
//! sentence is the only place the fallback is reported, so a client that
//! ignores it can show "all synthesis is local" over a stage with nothing in
//! it.

use super::super::wire::AllowOnlineModelsState;

settings_toggle!(
    set_allow_online_models,
    AllowOnlineModelsBody,
    "set_allow_online_models",
    "/settings/allow_online_models",
    AllowOnlineModelsState,
    "Allow or forbid models that synthesize over the network",
    "The privacy gate for hosted providers. While it is off, `POST /pipeline/{stage}/model` \
refuses any model whose backend would send text to a third party, and turning it off while such a \
model is loaded reverts the stage to a local model — or empties the stage, when this host has no \
local backend to fall back to. Read `message` to learn which of those two happened before \
reporting to the user that everything is now local. Subscribers to \
`GET /events?topics=daemon_status_changed` see the follow-up event for whatever model ends up \
loaded."
);
settings_dispatch!(
    get_allow_online_models,
    "get_allow_online_models",
    get "/settings/allow_online_models",
    AllowOnlineModelsState,
    "Read whether network models may be used",
    "Answers with the gate's state. `false` is a guarantee about where audio is produced: no \
model that would send text off this machine can be loaded while it holds, so a client can \
truthfully label the daemon as local-only. It says nothing about which model is loaded — that is \
`GET /pipeline/{stage}/model`."
);
