// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_audio_theme,
    SetAudioThemeBody { theme: String },
    "set_audio_theme",
    "theme"
);
settings_dispatch!(get_audio_theme, "get_audio_theme");
settings_dispatch!(test_audio_theme, "test_audio_theme");
settings_dispatch!(list_audio_themes, "list_audio_themes");
