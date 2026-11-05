// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end tests for `voice speak`, text to speech's commands.
//!
//! All of them are hermetic: the daemon comes up with no backends, so `speak`
//! is refused before anything reaches an audio device. They run as part of the
//! default `cargo test` flow:
//!
//! - `speak`      → `POST /v1/tts/speak`      : text from arguments or from stdin.
//! - `speak stop` → `POST /v1/tts/speak/stop` : idempotent against an idle daemon.
//!
//! `ping`, `status` and `logout` are the daemon's, and `cmd_basic.rs` covers them.
//!
//! The CLI keeps its token in the test daemon's home, never the developer's
//! state directory, and the daemon runs with `SUPER_VOICE_AUTO_APPROVE=1` (it
//! auto-approves `/auth/request`, so no consent popup is spawned).
//!
//! ```bash
//! cargo test -p super-voice-cli --test cmd_basic -- --nocapture
//! ```

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use super_engine_test_daemon::TestDaemon;
use super_voice_shared::product::SUPER_VOICE;

const CLI_BIN: &str = env!("CARGO_BIN_EXE_super-voice-cli");

/// Locate the daemon binary next to the CLI binary's target dir. `cargo
/// test -p super-voice-cli` builds the daemon as a workspace dependency, so
/// they share the same `target/<profile>/` slot.
fn locate_daemon_bin() -> PathBuf {
    let dir = PathBuf::from(CLI_BIN)
        .parent()
        .expect("cli bin parent dir")
        .to_path_buf();
    let candidate = dir.join("super-voice-daemon");
    assert!(
        candidate.exists(),
        "expected the daemon binary at {} — run `cargo build -p super-voice-daemon` first",
        candidate.display()
    );
    candidate
}

/// Monotonic per-call counter so temp paths are unique even when these
/// tests run concurrently in the same binary.
fn next_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static U: AtomicU64 = AtomicU64::new(0);
    U.fetch_add(1, Ordering::Relaxed)
}

/// Start a hermetic daemon (see `super_engine_test_daemon`) and wait for its
/// socket. The CLI targets it with `--socket`.
///
/// These tests are synchronous, and the CLI they run blocks, so the wait gets
/// a runtime of its own.
fn spawn_daemon() -> TestDaemon {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime to wait on the daemon")
        .block_on(TestDaemon::build(&SUPER_VOICE, locate_daemon_bin(), "cli-basic").start())
}

/// Run the CLI binary against `daemon`, blocking until it exits. Its token is
/// kept in the daemon's home. Returns `(exit code, stdout, stderr)`.
fn run_cli(daemon: &TestDaemon, args: &[&str]) -> (i32, String, String) {
    let mut full_args = vec!["--socket", daemon.socket().to_str().expect("socket utf8")];
    full_args.extend_from_slice(args);

    let mut child = Command::new(CLI_BIN)
        .env("XDG_STATE_HOME", &daemon.home().state)
        .env("SUPER_VOICE_AUTO_APPROVE", "1")
        .env("SUPER_VOICE_MUTE_CUES", "1")
        .args(&full_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cli");

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut h) = child.stdout.take() {
        let _ = h.read_to_string(&mut stdout);
    }
    if let Some(mut h) = child.stderr.take() {
        let _ = h.read_to_string(&mut stderr);
    }
    let status = child.wait().expect("wait cli");
    (status.code().unwrap_or(-1), stdout, stderr)
}

/// `speak` reaches `POST /v1/tts/speak` and surfaces the daemon's refusal when no
/// model is loaded — the routing is the subject, not the audio. A hermetic
/// daemon has no backends, so this is the reachable end of the path.
#[test]
fn speak_routes_to_the_daemon_and_surfaces_its_refusal() {
    let daemon = spawn_daemon();
    let (code, stdout, stderr) = run_cli(&daemon, &["speak", "hello", "there"]);
    let combined = format!("{stdout}\n{stderr}");
    assert_ne!(
        code, 0,
        "speak with no model loaded must not report success; got `{combined}`"
    );
    assert!(
        combined.contains("model_not_loaded"),
        "the daemon's coded refusal must reach the user; got `{combined}`"
    );
}

/// Text on stdin is what makes `... | voice speak` work, which is how a
/// shell pipeline reaches the daemon without a client of its own. It must route
/// the same way as trailing arguments rather than silently speaking nothing.
#[test]
fn speak_reads_text_from_stdin_when_no_arguments_are_given() {
    let daemon = spawn_daemon();

    let mut child = Command::new(CLI_BIN)
        .env("XDG_STATE_HOME", &daemon.home().state)
        .env("SUPER_VOICE_AUTO_APPROVE", "1")
        .env("SUPER_VOICE_MUTE_CUES", "1")
        .args([
            "--socket",
            daemon.socket().to_str().expect("socket utf8"),
            "speak",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cli speak");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().expect("stdin pipe");
        stdin
            .write_all(b"piped text reaches the daemon")
            .expect("write stdin");
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut h) = child.stdout.take() {
        let _ = h.read_to_string(&mut stdout);
    }
    if let Some(mut h) = child.stderr.take() {
        let _ = h.read_to_string(&mut stderr);
    }
    let _ = child.wait().expect("wait cli");

    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains("model_not_loaded"),
        "piped text must reach the daemon and get its refusal, not a local \
         'no text given' error; got `{combined}`"
    );
}

/// `stop` against an idle daemon is idempotent: the daemon answers `200` with
/// no utterance id and the CLI says so with a zero exit. A user cancelling on a
/// keypress should not get an error for having won the race.
#[test]
fn speak_stop_is_idempotent_when_idle() {
    let daemon = spawn_daemon();
    for attempt in 1..=2 {
        let (code, stdout, stderr) = run_cli(&daemon, &["speak", "stop"]);
        assert_eq!(
            code, 0,
            "stop attempt {attempt} on an idle daemon should exit 0; \
             stdout=`{stdout}` stderr=`{stderr}`"
        );
        assert!(
            stdout.contains("Nothing was speaking"),
            "stop attempt {attempt} should say nothing was speaking; \
             stdout=`{stdout}` stderr=`{stderr}`"
        );
    }
}
