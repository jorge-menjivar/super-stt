// SPDX-License-Identifier: GPL-3.0-only
use super::super::wire::ReadMethodState;

settings_setter!(
    set_read_method,
    SetReadMethodBody { method: String },
    "set_read_method",
    "method",
    "/settings/read_method",
    ReadMethodState,
    "Choose where the read-aloud shortcut reads from",
    "Sets where `POST /tts/speak/read` takes the text it speaks from: `selection`, the \
highlighted text, or `clipboard`, the text last copied. Text to speech's counterpart \
to `/settings/write_method`.",
    "`selection` or `clipboard`. An unknown token is a `400`.",
);
settings_dispatch!(
    get_read_method,
    "get_read_method",
    get "/settings/read_method",
    ReadMethodState,
    "Read where the read-aloud shortcut reads from",
    "Answers with `selection` or `clipboard`: where `POST /tts/speak/read` takes the \
text it speaks from."
);
