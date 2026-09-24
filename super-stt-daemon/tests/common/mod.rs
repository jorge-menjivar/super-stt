// SPDX-License-Identifier: GPL-3.0-only
//! Shared harness for the tests that spawn a real `super-stt-daemon`: the
//! daemon itself is `super_engine_test_daemon`'s, shared with Super TTS, and
//! the backend fixtures are here.
//!
//! This module is compiled into each test binary that declares `mod common;`,
//! and every one of them uses only the part it needs — so anything the others
//! use reads as dead code here. Hence the crate-level allow rather than a
//! per-item one.
#![allow(dead_code, unused_imports)]

use std::path::{Path, PathBuf};

pub use super_engine_test_daemon::{Method, StatusCode, TestDaemon, shutdown};
use super_stt_shared::product::SUPER_STT;

/// The daemon binary under test.
pub const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-daemon");

/// A `super-stt-daemon` to start, in a home of its own named after `label`.
/// See `super_engine_test_daemon` for the home and the test switches.
pub fn daemon(label: &str) -> super_engine_test_daemon::Builder {
    TestDaemon::build(&SUPER_STT, DAEMON_BIN, label)
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
    super_engine_test_daemon::request(&SUPER_STT, socket, method, path, token, body).await
}

// ---------- backend fixtures -------------------------------------------------

/// Where the daemon discovers installed backends, under an isolated
/// `XDG_DATA_HOME`. Written down once here because a fixture that lands
/// anywhere else is simply not found, and the daemon reports that the same way
/// it reports having no backends at all.
#[must_use]
pub fn backends_dir(data_home: &Path) -> PathBuf {
    data_home.join("super-stt").join("backends")
}

/// The prebuilt mock component (`just build-mock-wasm-backend`), or `None` when
/// it has not been built. A test that needs a backend which actually *answers*
/// — rather than one that merely exists on disk — stages this as its
/// entrypoint; it serves canned `/v1` responses, including a `transcribe` that
/// returns [`MOCK_TRANSCRIPTION`].
#[must_use]
pub fn mock_component() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/mock-wasm-backend/target/wasm32-wasip2/release/mock_wasm_backend.wasm",
    );
    p.exists().then_some(p)
}

/// The transcript [`mock_component`] returns from `POST /v1/transcribe`. Kept
/// beside the path that stages it so an assertion cannot drift from the
/// component it is asserting against.
pub const MOCK_TRANSCRIPTION: &str = "mock transcription";

/// A backend as the daemon finds it on disk: a `backend.toml` and the
/// entrypoint that manifest names.
///
/// Two kinds of fixture come out of this, and the difference is `component`.
/// With `None` the entrypoint is an empty placeholder — enough for discovery,
/// which reads the manifest and never execs the file, and all a test needs when
/// it is asking *about* a backend (what it advertises, what version is
/// installed, what options it declares). With `Some`, real component bytes are
/// staged, and the backend can be selected, loaded and asked to transcribe.
pub struct BackendFixture<'a> {
    /// Directory under [`backends_dir`]. Only has to be unique.
    pub dir_name: &'a str,
    /// The whole `backend.toml`. Written verbatim, so a test can express
    /// exactly the manifest it means — including one that is deliberately odd.
    pub manifest: &'a str,
    /// The file name the manifest's `entrypoint` names.
    pub entrypoint: &'a str,
    /// Component bytes to stage, or `None` for an empty placeholder.
    pub component: Option<&'a Path>,
}

impl BackendFixture<'_> {
    /// Write this backend into `data_home` so the daemon discovers it at
    /// startup. Call before spawning the daemon: discovery runs once, during
    /// `post_init`.
    pub fn install(&self, data_home: &Path) {
        let dir = backends_dir(data_home).join(self.dir_name);
        std::fs::create_dir_all(&dir).expect("create fixture backend dir");
        std::fs::write(dir.join("backend.toml"), self.manifest).expect("write backend.toml");
        match self.component {
            Some(src) => {
                std::fs::copy(src, dir.join(self.entrypoint)).expect("stage mock component");
            }
            None => std::fs::write(dir.join(self.entrypoint), b"")
                .expect("write placeholder entrypoint"),
        }
    }
}

// ---------- client-side keyring ----------------------------------------------

/// Route *this process's* keyring access to the in-memory mock.
///
/// The daemon subprocess gets the mock from `SUPER_STT_KEYRING_MOCK=1` in its
/// environment. A test that drives a client helper — anything reaching
/// `session::obtain`/`save`/`forget` — does that work in the test process
/// itself, which has no such routing and so reaches the real secret service:
/// it writes entries into the developer's keyring, and on a headless CI runner
/// it blocks on an unlock prompt that never comes. That is what kept the widget
/// subscription tests behind `#[ignore]`.
///
/// `session::install_mock_keyring_if_requested` is the production entry point,
/// but it is gated on reading that env var, and *setting* an env var from a
/// process that already has threads running is unsound under edition 2024. So
/// this calls the same builder directly. Idempotent, and safe to call from
/// every test: the default builder must be set before any keyring access, and
/// `Once` makes the first caller win while the rest wait.
pub fn install_mock_keyring() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    });
}
