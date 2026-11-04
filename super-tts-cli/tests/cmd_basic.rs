// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end tests for every CLI subcommand.
//!
//! All of them are hermetic: the daemon comes up with no backends, so `speak`
//! is refused before anything reaches an audio device, and none of the others
//! touch one either. They run as part of the default `cargo test` flow:
//!
//! - `ping`   → `GET  /v1/ping`       : liveness, prints the daemon's reply.
//! - `status` → `GET  /v1/status`     : prints `Model:` / `Device:` / `State:`.
//! - `speak`  → `POST /v1/speak`      : text from arguments or from stdin.
//! - `stop`   → `POST /v1/speak/stop` : idempotent against an idle daemon.
//! - `logout` → local keyring only    : forgets the cached session token.
//!
//! Both processes run with `SUPER_TTS_KEYRING_MOCK=1` (in-memory keyring,
//! no secret-service prompt) and `SUPER_TTS_AUTO_APPROVE=1` (the daemon
//! auto-approves `/auth/request`, so no consent popup is spawned).
//!
//! ```bash
//! cargo test -p super-tts-cli --test cmd_basic -- --nocapture
//! ```

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use super_engine_test_daemon::TestDaemon;
use super_tts_shared::product::SUPER_TTS;

const CLI_BIN: &str = env!("CARGO_BIN_EXE_super-tts-cli");

/// Locate the daemon binary next to the CLI binary's target dir. `cargo
/// test -p super-tts-cli` builds the daemon as a workspace dependency, so
/// they share the same `target/<profile>/` slot.
fn locate_daemon_bin() -> PathBuf {
    let dir = PathBuf::from(CLI_BIN)
        .parent()
        .expect("cli bin parent dir")
        .to_path_buf();
    let candidate = dir.join("super-tts-daemon");
    assert!(
        candidate.exists(),
        "expected the daemon binary at {} — run `cargo build -p super-tts-daemon` first",
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
/// socket. Returns it with the socket path the CLI should target via
/// `--socket`.
///
/// These tests are synchronous, and the CLI they run blocks, so the wait gets
/// a runtime of its own.
fn spawn_daemon() -> (TestDaemon, PathBuf) {
    let daemon = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime to wait on the daemon")
        .block_on(TestDaemon::build(&SUPER_TTS, locate_daemon_bin(), "cli-basic").start());
    let socket = daemon.socket().to_path_buf();
    (daemon, socket)
}

/// Run the CLI binary against `socket`, blocking until it exits. Returns
/// `(exit code, stdout, stderr)`.
fn run_cli(socket: &Path, args: &[&str]) -> (i32, String, String) {
    let mut full_args = vec!["--socket", socket.to_str().expect("socket utf8")];
    full_args.extend_from_slice(args);

    let mut child = Command::new(CLI_BIN)
        .env("SUPER_TTS_KEYRING_MOCK", "1")
        .env("SUPER_TTS_AUTO_APPROVE", "1")
        .env("SUPER_TTS_MUTE_CUES", "1")
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

/// `ping` must reach the daemon, auto-mint a token (auto-approve), and
/// print the daemon's liveness reply with a clean exit.
#[test]
fn ping_reports_daemon_alive() {
    let (_guard, socket) = spawn_daemon();
    let (code, stdout, stderr) = run_cli(&socket, &["ping"]);
    assert_eq!(
        code, 0,
        "ping should exit 0; stdout=`{stdout}` stderr=`{stderr}`"
    );
    let out = stdout.to_lowercase();
    assert!(
        out.contains("pong") || out.contains("running") || out.contains("alive"),
        "ping should print a liveness reply; stdout=`{stdout}` stderr=`{stderr}`"
    );
}

/// `status` must print the documented `Model:` and `Device:` lines. With
/// no backend installed the daemon is idle, so the model line reports
/// `(none loaded)` — but both labels are always present.
#[test]
fn status_reports_model_and_device() {
    let (_guard, socket) = spawn_daemon();
    let (code, stdout, stderr) = run_cli(&socket, &["status"]);
    assert_eq!(
        code, 0,
        "status should exit 0; stdout=`{stdout}` stderr=`{stderr}`"
    );
    assert!(
        stdout.contains("Model:"),
        "status should print a Model: line; stdout=`{stdout}` stderr=`{stderr}`"
    );
    assert!(
        stdout.contains("Device:"),
        "status should print a Device: line; stdout=`{stdout}` stderr=`{stderr}`"
    );
    assert!(
        stdout.contains("State:  idle"),
        "an idle daemon should report State: idle; stdout=`{stdout}` stderr=`{stderr}`"
    );
}

/// `speak` reaches `POST /v1/speak` and surfaces the daemon's refusal when no
/// model is loaded — the routing is the subject, not the audio. A hermetic
/// daemon has no backends, so this is the reachable end of the path.
#[test]
fn speak_routes_to_the_daemon_and_surfaces_its_refusal() {
    let (_guard, socket) = spawn_daemon();
    let (code, stdout, stderr) = run_cli(&socket, &["speak", "hello", "there"]);
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

/// Text on stdin is what makes `... | super-tts-cli speak` work, which is how a
/// shell pipeline reaches the daemon without a client of its own. It must route
/// the same way as trailing arguments rather than silently speaking nothing.
#[test]
fn speak_reads_text_from_stdin_when_no_arguments_are_given() {
    let (_guard, socket) = spawn_daemon();

    let mut child = Command::new(CLI_BIN)
        .env("SUPER_TTS_KEYRING_MOCK", "1")
        .env("SUPER_TTS_AUTO_APPROVE", "1")
        .env("SUPER_TTS_MUTE_CUES", "1")
        .args(["--socket", socket.to_str().expect("socket utf8"), "speak"])
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
fn stop_is_idempotent_when_idle() {
    let (_guard, socket) = spawn_daemon();
    for attempt in 1..=2 {
        let (code, stdout, stderr) = run_cli(&socket, &["stop"]);
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

/// `logout` is local-only — it forgets the cached session token via the
/// keyring and never contacts the daemon. Under the mock keyring there is
/// nothing stored, but `forget` is idempotent, so it still reports success
/// and exits cleanly. No daemon needed.
#[test]
fn logout_clears_cached_token() {
    // logout ignores the socket, but `--socket` is a global flag so passing
    // a throwaway path keeps `run_cli` uniform.
    let throwaway = std::env::temp_dir().join(format!(
        "tts-cli-logout-{}-{}.sock",
        std::process::id(),
        next_uniq()
    ));
    let mut child = Command::new(CLI_BIN)
        .env("SUPER_TTS_KEYRING_MOCK", "1")
        .args(["--socket", throwaway.to_str().unwrap(), "logout"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cli logout");
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut h) = child.stdout.take() {
        let _ = h.read_to_string(&mut stdout);
    }
    if let Some(mut h) = child.stderr.take() {
        let _ = h.read_to_string(&mut stderr);
    }
    let code = child.wait().expect("wait cli").code().unwrap_or(-1);
    assert_eq!(
        code, 0,
        "logout should exit 0; stdout=`{stdout}` stderr=`{stderr}`"
    );
    assert!(
        stdout.contains("Cached session token removed"),
        "logout should confirm the token was forgotten; stdout=`{stdout}` stderr=`{stderr}`"
    );
}
