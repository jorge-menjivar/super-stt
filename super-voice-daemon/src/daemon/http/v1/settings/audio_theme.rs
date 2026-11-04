// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/audio_theme` — the selected audio cue theme, the themes on offer,
//! and a preview of the selection.

use super::super::wire::{AudioThemeList, AudioThemeState};

settings_setter!(
    set_audio_theme,
    SetAudioThemeBody { theme: String },
    "set_audio_theme",
    "theme",
    "/settings/audio_theme",
    AudioThemeState,
    "Select the audio cue theme",
    "Chooses which set of sounds marks the start and end of a recording. List the \
accepted values with `GET /settings/audio_theme/list`; set the loudness at \
`/settings/volume`.",
    "A theme token from `GET /settings/audio_theme/list`, e.g. `classic`. An unknown token is a `400`.",
);
settings_dispatch!(
    get_audio_theme,
    "get_audio_theme",
    get "/settings/audio_theme",
    AudioThemeState,
    "Read the selected audio cue theme",
    "Answers with the selected theme's token."
);
settings_dispatch!(
    test_audio_theme,
    "test_audio_theme",
    post "/settings/audio_theme/test",
    crate::daemon::http::wire::Ack,
    "Play the selected theme's cues",
    "Plays the start and stop cues once, at the configured volume, so a settings UI \
can preview a theme without starting a recording. Changes nothing."
);
settings_dispatch!(
    list_audio_themes,
    "list_audio_themes",
    get "/settings/audio_theme/list",
    AudioThemeList,
    "List the available audio cue themes",
    "Every theme `POST /settings/audio_theme` accepts. The set is fixed in the daemon build, \
not user-extensible."
);
