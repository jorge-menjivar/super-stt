// SPDX-License-Identifier: GPL-3.0-only
//! Contract: every HTTP path that opens an utterance resolves the user's
//! configured voice and language first.
//!
//! The daemon stores a per-model voice (`POST /pipeline/{stage}/model/{model}/voice`)
//! precisely so that a caller with no UI for choosing — a keyboard shortcut, the
//! applet, a browser client — still speaks in the voice the user picked. That
//! fallback lives in [`SuperTTSDaemon::stored_defaults`], reached through
//! [`SuperTTSDaemon::begin_speech`].
//!
//! It is one call, and for a while only one of the two speak endpoints made it.
//! `POST /speak` resolved; `GET /speak/stream` handed the `start` frame straight
//! to `SpeechEngine::begin`. Nothing failed for a model with a `default_voice`,
//! because the backend filled the gap. A model whose voices are all *cloned* has
//! no default to fall back on, so every streamed utterance came back
//! `400 this model speaks in a cloned voice, so a request has to name one` —
//! while the voice the user had chosen sat in the config, unread.
//!
//! Nothing about the types prevents that: `SpeechEngine::begin` is public and
//! takes exactly the options the frame carries, so the unresolved call compiles
//! and reads correctly. This test is the thing that says it is wrong.

use std::path::{Path, PathBuf};

/// Walk `dir` and hand back every `.rs` file under it.
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

/// The HTTP layer must open utterances through `begin_speech`, never through
/// the engine directly.
#[test]
fn the_http_layer_never_opens_an_utterance_without_resolving_defaults() {
    let http_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/daemon/http");
    let mut offenders = Vec::new();

    for file in rust_sources(&http_root) {
        // This file quotes the call it is banning.
        if file.ends_with("speak_resolution_contract.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        // Comments stripped, then the whole file squashed onto one line:
        // rustfmt writes the call as `.speech\n    .begin(`, so a per-line
        // match would miss exactly the shape this is looking for.
        let code = text
            .lines()
            .map(|l| l.split("//").next().unwrap_or(l).trim())
            .collect::<Vec<_>>()
            .join(" ");
        if code.replace(" .", ".").contains(".speech.begin(") {
            offenders.push(file.display().to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "these HTTP sources call `SpeechEngine::begin` directly, skipping the \
         configured voice/language fallback — call `SuperTTSDaemon::begin_speech` \
         instead:\n  {}",
        offenders.join("\n  ")
    );
}
