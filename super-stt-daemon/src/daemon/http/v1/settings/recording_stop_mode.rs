// SPDX-License-Identifier: GPL-3.0-only
use super::super::wire::RecordingStopModeState;

settings_setter!(
    set_recording_stop_mode,
    SetRecordingStopModeBody { mode: String },
    "set_recording_stop_mode",
    "mode",
    "/settings/recording_stop_mode",
    RecordingStopModeState,
    "Choose what ends a recording",
    "Sets whether a recording stops on an explicit signal, on a period of silence, or \
on either. This governs daemon-mic captures started through `POST /transcribe`; it \
has no bearing on the pre-captured path, which has no capture to end.",
    "One of the accepted `snake_case` mode tokens. An unknown token is a `400`.",
);
settings_dispatch!(
    get_recording_stop_mode,
    "get_recording_stop_mode",
    get "/settings/recording_stop_mode",
    RecordingStopModeState,
    "Read what currently ends a recording",
    "Answers with the configured mode. It governs daemon-mic captures started \
through `POST /transcribe`; the pre-captured path has no capture to end, and a \
`wait: true` caller can always end one by closing the connection."
);
