// SPDX-License-Identifier: GPL-3.0-only
use super::wire::{AudioThemeList, AudioThemeState};

settings_setter!(
    set_audio_theme,
    SetAudioThemeBody { theme: String },
    "set_audio_theme",
    "theme",
    "/audio_theme",
    AudioThemeState,
    "Select the audio cue theme",
    "Chooses which set of sounds marks the start and end of a recording. List the \
accepted values with `GET /audio_themes`; set the loudness at `/volume`.",
    "A theme token from `GET /audio_themes`, e.g. `classic`. An unknown token is a `400`.",
);
settings_dispatch!(
    get_audio_theme,
    "get_audio_theme",
    get "/audio_theme",
    AudioThemeState,
    "Read the selected audio cue theme",
    "Answers with the selected theme's token."
);
settings_dispatch!(
    test_audio_theme,
    "test_audio_theme",
    post "/audio_theme/test",
    crate::daemon::http::wire::Ack,
    "Play the selected theme's cues",
    "Plays the start and stop cues once, at the configured volume, so a settings UI \
can preview a theme without starting a recording. Changes nothing."
);
settings_dispatch!(
    list_audio_themes,
    "list_audio_themes",
    get "/audio_themes",
    AudioThemeList,
    "List the available audio cue themes",
    "Every theme `POST /audio_theme` accepts. The set is fixed in the daemon build, \
not user-extensible."
);
