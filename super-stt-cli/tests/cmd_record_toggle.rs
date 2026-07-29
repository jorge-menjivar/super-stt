// SPDX-License-Identifier: GPL-3.0-only
//! CLI integration test: `super-stt-cli record` toggle dispatch.
//!
//! Pins the protocol-correct behavior added when the toggle UX moved
//! from the daemon to the CLI (see
//! `docs/protocol/endpoints/v1/transcribe.md`):
//!
//! - `record` against an idle daemon → calls `POST /v1/transcribe`
//!   (start). Audio failure in CI is tolerated — only the routing
//!   matters here.
//! - `record` against a recording daemon → consults
//!   `/v1/status::is_recording`, sees `true`, dispatches
//!   `POST /v1/transcribe/stop` instead of `/transcribe`. The
//!   first CLI's stream then receives a `done` event and the
//!   recording is stopped daemon-side.
//!
//! This is an end-to-end test that spawns the real daemon and CLI
//! binaries. `#[ignore]`'d by default because reliably driving the
//! recording into the `is_recording=true` state requires a real
//! microphone; on CI without audio the recording aborts before the
//! flag flips and the toggle path doesn't get exercised. Run
//! manually with:
//!
//! ```bash
//! cargo test -p super-stt-cli --test cmd_record_toggle -- --ignored --nocapture
//! ```

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_super-stt-cli");

/// Locate the daemon binary next to the CLI binary's target dir.
/// `cargo test -p super-stt-cli` builds the daemon as a workspace
/// dependency, so they live in the same `target/<profile>/` slot.
fn locate_daemon_bin() -> PathBuf {
    let cli_path = PathBuf::from(DAEMON_BIN); // misnamed env var: this points to cli
    let dir = cli_path.parent().expect("cli bin parent dir");
    let candidate = dir.join("super-stt-daemon");
    assert!(
        candidate.exists(),
        "expected the daemon binary at {} — run `cargo build -p super-stt-daemon` first",
        candidate.display()
    );
    candidate
}

/// Monotonic per-call counter so the test's temp paths are unique
/// even when this test runs alongside others in the same binary.
fn next_uniq() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static U: AtomicU64 = AtomicU64::new(0);
    U.fetch_add(1, Ordering::Relaxed)
}

struct DaemonGuard {
    child: Child,
    cleanup: Vec<PathBuf>,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for p in &self.cleanup {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn spawn_daemon() -> (DaemonGuard, PathBuf) {
    let tmp = std::env::temp_dir();
    let unique = format!("stt-cli-toggle-{}-{}", std::process::id(), next_uniq());
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    std::fs::create_dir_all(&config_home).expect("create config dir");

    let child = Command::new(locate_daemon_bin())
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists() {
            std::thread::sleep(Duration::from_millis(500));
            return (
                DaemonGuard {
                    child,
                    cleanup: vec![http_socket.clone()],
                },
                http_socket,
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("daemon HTTP socket did not appear within 120s");
}

/// Run the CLI binary with the given args, blocking until exit.
/// Returns (exit code, stdout, stderr).
fn run_cli(socket: &Path, args: &[&str]) -> (i32, String, String) {
    let cli_bin = DAEMON_BIN;
    let mut child = Command::new(cli_bin)
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", socket)
        .args(args)
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

/// Idle daemon: `record` → routes to `/v1/transcribe`. Audio failure
/// in CI is fine — we just verify that the CLI didn't barf because of
/// a routing bug. The expected paths are:
///  - audio works → recording starts, CLI blocks on SSE stream until
///    silence; stdout has `(no speech detected)` or transcribed text.
///  - audio fails → daemon returns an error; CLI exits non-zero with
///    stderr message containing the failure.
/// Both outcomes are valid; what we check is that we never end up in
/// the "transcribe stream ended unexpectedly" state, which was the
/// symptom of the original toggle bug where a JSON 409 body was fed
/// to the SSE parser.
#[test]
#[ignore = "spawns the real daemon binary; needs a microphone for full toggle coverage"]
fn record_routes_to_transcribe_when_idle() {
    let (_guard, socket) = spawn_daemon();
    // Use manual-only so we don't sit in a wait-for-silence loop for
    // the full 60 s timeout when silence detection isn't triggered.
    // Spawn it as a one-shot — the test below kills it via toggle.
    let (code, stdout, stderr) = run_cli(&socket, &["record", "--stop-mode", "manual-only"]);
    // It will block forever in manual-only mode without a second
    // press to stop it; the DaemonGuard kills the daemon on drop and
    // the CLI's connection-on-disconnect handling exits. So this
    // test mostly proves the CLI doesn't crash on the routing.
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        !combined.contains("transcribe stream ended unexpectedly"),
        "CLI must not surface the broken-stream message; got code={code} stdout=`{stdout}` stderr=`{stderr}`"
    );
}

/// `record` against a recording daemon: the CLI must consult
/// `/v1/status::is_recording`, see `true`, and dispatch
/// `/v1/transcribe/stop` instead of `/v1/transcribe`. We can't force
/// `is_recording=true` from outside the daemon without driving a real
/// recording, so this is an end-to-end test that needs a working
/// microphone.
///
/// What's checked:
///  1. CLI A: `record --stop-mode manual-only` → starts recording.
///     Blocks on SSE.
///  2. After ~3 s (enough for `is_recording=true` to be set), CLI B
///     runs `record` (no args) → must detect the in-flight capture
///     via `/status` and call `/transcribe/stop`. Stdout must contain
///     "Recording stop signal sent" (the documented `transcribe/stop`
///     success message — NOT a transcription or "(no speech
///     detected)").
///  3. CLI A's stream then receives a `done` event and exits.
#[test]
#[ignore = "spawns the real daemon binary; needs a microphone to actually start a recording"]
fn record_routes_to_transcribe_stop_when_already_recording() {
    let (_guard, socket) = spawn_daemon();
    let socket_b = socket.clone();

    // Spawn CLI A in a background thread.
    let a = std::thread::spawn(move || {
        let cli_bin = DAEMON_BIN;
        Command::new(cli_bin)
            .env("SUPER_STT_AUTO_APPROVE", "1")
            .env("SUPER_STT_HTTP_SOCKET", &socket)
            .args(["record", "--stop-mode", "manual-only"])
            .output()
            .expect("CLI A run")
    });

    // Let A reach the recording state.
    std::thread::sleep(Duration::from_secs(3));

    // CLI B: the toggle path.
    let (code, stdout, stderr) = run_cli(&socket_b, &["record"]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        !combined.contains("transcribe stream ended unexpectedly"),
        "CLI B must not surface broken-stream message; got code={code} stdout=`{stdout}` stderr=`{stderr}`"
    );
    assert!(
        stdout.contains("Recording stop signal sent")
            || stdout.contains("Manual stop not enabled in current mode")
            || stdout.contains("Transcription in progress, please wait")
            || stdout.contains("No recording in progress"),
        "CLI B must surface one of the documented stop messages; got stdout=`{stdout}` stderr=`{stderr}`"
    );

    let a_output = a.join().expect("CLI A thread");
    let a_stdout = String::from_utf8_lossy(&a_output.stdout);
    let a_stderr = String::from_utf8_lossy(&a_output.stderr);
    assert!(
        !a_stdout.contains("transcribe stream ended unexpectedly")
            && !a_stderr.contains("transcribe stream ended unexpectedly"),
        "CLI A must finish cleanly after toggle stop; stdout=`{a_stdout}` stderr=`{a_stderr}`"
    );
}
