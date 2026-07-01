// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_recording_stop_mode,
    SetRecordingStopModeBody { mode: String },
    "set_recording_stop_mode",
    "mode"
);
settings_dispatch!(get_recording_stop_mode, "get_recording_stop_mode");
