// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/volume` — how loud the audio cues play.
//!
//! The odd pair on this surface: both methods answer with the bare
//! [`Ack`](crate::daemon::http::wire::Ack) rather than a typed state, because
//! the underlying command reports the level in `message` and nowhere else. Each
//! description says so, since a client that does not parse the number back out
//! has no way to render the slider it just moved.

settings_setter!(
    set_volume,
    SetVolumeBody { volume: u8 },
    "set_volume",
    "volume",
    "/settings/volume",
    crate::daemon::http::wire::Ack,
    "Set the audio cue volume",
    "Sets the loudness of the start and stop cues on a 0–100 scale. `0` silences them without \
changing which theme is selected, so a user who mutes the cues still gets their theme back when \
they turn the volume up; the theme itself is read and written at `/settings/audio_theme`. This \
governs the cues only — it does not change how loud synthesized speech is. The applied level \
comes back inside `message`, not as a field of its own.",
    "Integer in `0..=100`. Anything outside that range is a `400`.",
);
settings_dispatch!(
    get_volume,
    "get_volume",
    get "/settings/volume",
    crate::daemon::http::wire::Ack,
    "Read the audio cue volume",
    "The level rides in `message` as a bare number — `\"75\"` — not as a field of its own, so \
parse it back out. It is the cue volume on a 0–100 scale, not the loudness of synthesized \
speech, and `0` means the cues are muted while the selected theme is left as it was."
);
