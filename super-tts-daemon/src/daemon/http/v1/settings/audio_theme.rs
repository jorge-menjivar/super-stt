// SPDX-License-Identifier: GPL-3.0-only
//! `/settings/audio_theme` — the selected audio cue theme, the themes on offer,
//! and a preview of the selection.
//!
//! The cues are the short tones that mark the start and end of an utterance,
//! not the speech itself: silencing them (the `silent` theme, or volume `0`)
//! does not silence synthesis. Which theme is selected and how loud it plays
//! are separate settings on purpose — a user who wants quieter cues usually
//! does not want different ones — so the loudness lives at `/settings/volume`.

use super::super::wire::{AudioThemeList, AudioThemeState};

settings_setter!(
    set_audio_theme,
    SetAudioThemeBody { theme: String },
    "set_audio_theme",
    "theme",
    "/settings/audio_theme",
    AudioThemeState,
    "Select the audio cue theme",
    "Chooses which set of tones marks the start and end of an utterance. List the accepted \
values with `GET /settings/audio_theme/list` rather than hard-coding them; audition the choice \
with `POST /settings/audio_theme/test`; set the loudness at `/settings/volume`. An unknown token \
is rejected rather than quietly falling back to the default, so a typo cannot look like success.",
    "A theme token from `GET /settings/audio_theme/list`, e.g. `classic`. An unknown token is a `400`.",
);
settings_dispatch!(
    get_audio_theme,
    "get_audio_theme",
    get "/settings/audio_theme",
    AudioThemeState,
    "Read the selected audio cue theme",
    "Answers with the selected theme's token — one of those `GET /settings/audio_theme/list` \
offers. `silent` is a real selection, not an absent one: it means the user chose to hear no cues, \
which is distinct from the volume being turned down."
);
settings_dispatch!(
    test_audio_theme,
    "test_audio_theme",
    post "/settings/audio_theme/test",
    crate::daemon::http::wire::Ack,
    "Play the selected theme's cues",
    "Plays the start and stop cues once, at the configured volume, so a settings UI can \
audition a theme without speaking anything. Changes nothing. The sound comes out of the host the \
daemon runs on, not the client's speakers, so a remote or sandboxed caller gets a success nobody \
hears; the `silent` theme succeeds without playing at all and says so in `message`."
);
settings_dispatch!(
    list_audio_themes,
    "list_audio_themes",
    get "/settings/audio_theme/list",
    AudioThemeList,
    "List the available audio cue themes",
    "Every theme `POST /settings/audio_theme` accepts. The set is fixed in the daemon build \
rather than user-extensible, but it is still read from here rather than hard-coded in a client, \
so a build that adds a theme does not need every client rebuilt to offer it."
);
