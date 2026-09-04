// SPDX-License-Identifier: GPL-3.0-only
//! `/audio_themes` — every audio cue theme the daemon ships.
//!
//! A sibling path of [`super::audio_theme`], not a sub-path of it: this lists
//! what is on offer, that one holds the selection.

use super::wire::AudioThemeList;

settings_dispatch!(
    list_audio_themes,
    "list_audio_themes",
    get "/audio_themes",
    AudioThemeList,
    "List the available audio cue themes",
    "Every theme `POST /audio_theme` accepts. The set is fixed in the daemon build, \
not user-extensible."
);
