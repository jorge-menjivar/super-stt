// SPDX-License-Identifier: GPL-3.0-only
settings_setter!(
    set_volume,
    SetVolumeBody { volume: u8 },
    "set_volume",
    "volume",
    "/settings/volume",
    crate::daemon::http::wire::Ack,
    "Set the audio cue volume",
    "Sets the loudness of the recording cues on a 0\u{2013}100 scale. `0` silences them \
without changing which theme is selected; the theme itself is read and written at \
`/settings/audio_theme`.",
    "Integer in `0..=100`. Anything outside that range is a `400`.",
);
settings_dispatch!(
    get_volume,
    "get_volume",
    get "/settings/volume",
    crate::daemon::http::wire::Ack,
    "Read the audio cue volume",
    "The level rides in `message` as a bare number \u{2014} `\"75\"` \u{2014} not as a field of \
its own, so parse it back out."
);
