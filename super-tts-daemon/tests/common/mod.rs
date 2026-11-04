// SPDX-License-Identifier: GPL-3.0-only
//! Shared harness for the tests that spawn a real `super-tts-daemon`: the
//! daemon itself is `super_engine_test_daemon`'s, shared with Super STT.
//!
//! This module is compiled into each test binary that declares `mod common;`,
//! and every one of them uses only the part it needs — so anything the others
//! use reads as dead code here. Hence the crate-level allow rather than a
//! per-item one.
#![allow(dead_code, unused_imports)]

use std::path::Path;

pub use super_engine_test_daemon::{Method, StatusCode, TestDaemon, shutdown};
use super_tts_shared::product::SUPER_TTS;

/// The daemon binary under test.
pub const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-tts-daemon");

/// A `super-tts-daemon` to start, in a home of its own named after `label`.
/// See `super_engine_test_daemon` for the home and the test switches.
pub fn daemon(label: &str) -> super_engine_test_daemon::Builder {
    TestDaemon::build(&SUPER_TTS, DAEMON_BIN, label)
}

/// `method /v1{path}` on the daemon at `socket`. See
/// `super_engine_test_daemon::request`.
pub async fn request(
    socket: &Path,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<&serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    super_engine_test_daemon::request(&SUPER_TTS, socket, method, path, token, body).await
}

// ---------- client-side session tokens ---------------------------------------

/// Refuse to run a client in this process unless the run has a state
/// directory of its own.
///
/// A test that drives a client helper — anything reaching
/// `session::obtain`/`save`/`forget` — does that work in the test process
/// itself, and the client keeps its token in a file under
/// `$XDG_STATE_HOME/<product>/clients/`. The daemon subprocess gets a home of
/// its own from the harness; this process can only get one from its
/// environment, and setting a variable from a process that already has
/// threads running is unsound under edition 2024. `just test` gives the whole
/// run a fresh one. Without it, the test would write tokens into the
/// developer's real state directory.
pub fn require_own_state_dir() {
    let own = std::env::var_os("XDG_STATE_HOME")
        .is_some_and(|dir| std::path::Path::new(&dir).starts_with(std::env::temp_dir()));
    assert!(
        own,
        "XDG_STATE_HOME must be a directory under {} for this test, or it writes \
         client tokens into your real state directory. Run it with `just test`.",
        std::env::temp_dir().display()
    );
}
