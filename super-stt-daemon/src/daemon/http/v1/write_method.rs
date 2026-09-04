// SPDX-License-Identifier: GPL-3.0-only
use super::wire::{WriteMethodState, WriteMethodTest};

settings_setter!(
    set_write_method,
    SetWriteMethodBody { method: String },
    "set_write_method",
    "method",
    "/write_method",
    WriteMethodState,
    "Choose how transcripts reach the focused window",
    "Selects the mechanism the daemon uses to deliver a finished transcript \u{2014} \
simulated typing, the clipboard, and so on. Which mechanisms work depends on the \
session: some need a compositor that permits synthetic input. Try one with \
`POST /write_method/test` before committing a user to it.",
    "One of the accepted `snake_case` method tokens. An unknown token is a `400`.",
);
settings_dispatch!(
    get_write_method,
    "get_write_method",
    get "/write_method",
    WriteMethodState,
    "Read how transcripts are delivered",
    "Answers with the configured preference. What it actually resolves to in this \
session can differ \u{2014} `POST /write_method/test` reports both."
);
settings_dispatch!(
    test_write_method,
    "test_write_method",
    post "/write_method/test",
    WriteMethodTest,
    "Write sample text using the configured method",
    "Delivers a short sample to the focused window exactly as a real transcript would \
be, so a user can confirm the method works before relying on it. Focus the window \
that should receive it first.\n\nThe response reports both the configured preference \
and what it resolved to, which is how a UI shows that a preferred mechanism silently \
fell back to another. Refused with `409` while a recording is in flight."
);
