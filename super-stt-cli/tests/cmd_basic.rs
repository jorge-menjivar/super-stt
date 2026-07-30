// SPDX-License-Identifier: GPL-3.0-only
//! End-to-end tests for the CLI's non-recording subcommands: `ping`,
//! `status`, `stop`, and `logout`.
//!
//! Complements `cmd_record_toggle.rs` (which covers the `record` toggle
//! path and is `#[ignore]`'d because it needs a real microphone). These
//! commands are fully hermetic — none touch the audio pipeline — so they
//! run as part of the default `cargo test` flow:
//!
//! - `ping`   → `GET  /v1/ping`            : liveness, prints the daemon's reply.
//! - `status` → `GET  /v1/status`          : prints `Model:` / `Device:` lines.
//! - `stop`   → `POST /v1/transcribe/stop` : idempotent against an idle daemon.
//! - `logout` → local keyring only         : forgets the cached session token.
//!
//! Both processes run with `SUPER_STT_KEYRING_MOCK=1` (in-memory keyring,
//! no secret-service prompt) and `SUPER_STT_AUTO_APPROVE=1` (the daemon
//! auto-approves `/auth/request`, so no consent popup is spawned).
//!
//! ```bash
//! cargo test -p super-stt-cli --test cmd_basic -- --nocapture
//! ```

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CLI_BIN: &str = env!("CARGO_BIN_EXE_super-stt-cli");

/// Locate the daemon binary next to the CLI binary's target dir. `cargo
/// test -p super-stt-cli` builds the daemon as a workspace dependency, so
/// they share the same `target/<profile>/` slot.
fn locate_daemon_bin() -> PathBuf {
    let dir = PathBuf::from(CLI_BIN)
        .parent()
        .expect("cli bin parent dir")
        .to_path_buf();
    let candidate = dir.join("super-stt-daemon");
    assert!(
        candidate.exists(),
        "expected the daemon binary at {} — run `cargo build -p super-stt-daemon` first",
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
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

/// Spawn a hermetic daemon on an isolated socket / config / data dir and
/// wait for the HTTP socket to appear. Returns the guard plus the socket
/// path the CLI should target via `--socket`.
fn spawn_daemon() -> (DaemonGuard, PathBuf) {
    let tmp = std::env::temp_dir();
    let unique = format!("stt-cli-basic-{}-{}", std::process::id(), next_uniq());
    let http_socket = tmp.join(format!("{unique}-http.sock"));
    let config_home = tmp.join(format!("{unique}-config"));
    let data_home = tmp.join(format!("{unique}-data"));
    std::fs::create_dir_all(&config_home).expect("create config dir");
    // Empty, isolated data dir → the daemon discovers no backends and comes
    // up idle and fast, which is exactly what these commands need.
    std::fs::create_dir_all(&data_home).expect("create data dir");

    let child = Command::new(locate_daemon_bin())
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
        .env("SUPER_STT_HTTP_SOCKET", &http_socket)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");

    // Hand the child to the guard before the readiness loop: the timeout
    // panic below must still kill and reap the daemon, not leak it.
    let guard = DaemonGuard {
        child,
        cleanup: vec![http_socket.clone(), config_home, data_home],
    };

    let deadline = Instant::now() + Duration::from_mins(2);
    while Instant::now() < deadline {
        if Path::new(&http_socket).exists() {
            // Give the listener a beat to finish binding before the CLI connects.
            std::thread::sleep(Duration::from_millis(500));
            return (guard, http_socket);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("daemon HTTP socket did not appear within 120s");
}

/// Run the CLI binary against `socket`, blocking until it exits. Returns
/// `(exit code, stdout, stderr)`.
fn run_cli(socket: &Path, args: &[&str]) -> (i32, String, String) {
    let mut full_args = vec!["--socket", socket.to_str().expect("socket utf8")];
    full_args.extend_from_slice(args);

    let mut child = Command::new(CLI_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
        .env("SUPER_STT_AUTO_APPROVE", "1")
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
}

/// `stop` against an idle daemon is idempotent: the daemon answers
/// `200 { message: "No recording in progress" }` and the CLI surfaces that
/// message verbatim with a zero exit. This is the path the toggle hotkey
/// relies on when the user double-presses after audio failed to start.
#[test]
fn stop_is_idempotent_when_idle() {
    let (_guard, socket) = spawn_daemon();
    let (code, stdout, stderr) = run_cli(&socket, &["stop"]);
    assert_eq!(
        code, 0,
        "stop on idle daemon should exit 0; stdout=`{stdout}` stderr=`{stderr}`"
    );
    assert!(
        stdout.contains("No recording in progress"),
        "stop should surface the idle message; stdout=`{stdout}` stderr=`{stderr}`"
    );
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
        "stt-cli-logout-{}-{}.sock",
        std::process::id(),
        next_uniq()
    ));
    let mut child = Command::new(CLI_BIN)
        .env("SUPER_STT_KEYRING_MOCK", "1")
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
