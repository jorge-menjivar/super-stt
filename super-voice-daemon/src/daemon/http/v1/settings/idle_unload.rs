// SPDX-License-Identifier: GPL-3.0-only
use super::super::wire::IdleUnloadState;

settings_setter!(
    set_idle_unload,
    SetIdleUnloadBody { after_secs: u64 },
    "set_idle_unload",
    "after_secs",
    "/settings/idle_unload",
    IdleUnloadState,
    "Set how long an unused model stays loaded",
    "With a value set, the transcription and synthesis models are unloaded after that \
long without use and loaded again by the next request that needs them, which waits for \
the load. `0` keeps models loaded, and loads any selected model that is not.",
    "Seconds a model may sit unused before it is unloaded. `0` keeps models loaded.",
);
settings_dispatch!(
    get_idle_unload,
    "get_idle_unload",
    get "/settings/idle_unload",
    IdleUnloadState,
    "Read how long an unused model stays loaded",
    "Answers with the idle time in seconds. `0` means models stay loaded."
);
