// SPDX-License-Identifier: GPL-3.0-only
use super::wire::PreviewTypingState;

settings_toggle!(
    set_preview_typing,
    PreviewTypingBody,
    "set_preview_typing",
    "/preview_typing",
    PreviewTypingState,
    "Turn live preview typing on or off",
    "When on, a realtime-capable model's incremental transcript is typed into the \
focused window as it forms, and corrected in place as later audio revises it. When \
off, nothing is written until the transcript is final."
);
settings_dispatch!(
    get_preview_typing,
    "get_preview_typing",
    get "/preview_typing",
    PreviewTypingState,
    "Read whether live preview typing is on",
    "Answers with the current state. On means a realtime-capable model's partial \
transcript is typed into the focused window as it forms; off means nothing is \
written until the transcript is final. A model without a realtime session ignores \
the setting either way."
);
