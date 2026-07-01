// SPDX-License-Identifier: GPL-3.0-only
//! `/recording_stop_mode` — how recording stops (silence, manual, or both).

settings_getter!(
    get_recording_stop_mode -> String, "/recording_stop_mode", "get_stop_mode",
    |resp| resp
        .recording_stop_mode
        .unwrap_or_else(|| "silence-and-manual".to_string())
);
settings_setter!(
    set_recording_stop_mode,
    mode: String,
    "/recording_stop_mode",
    "mode",
    "set_stop_mode"
);
