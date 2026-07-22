// SPDX-License-Identifier: GPL-3.0-only
//! GUI smoke tests for the consent helper.
//!
//! These tests **actually launch the libcosmic window** and therefore need
//! a working desktop session (Wayland or X11). They are
//! `#[ignore]`'d so the default `cargo test` skips them; CI never runs
//! them unless explicitly opted-in. To run them locally:
//!
//! ```bash
//! cargo test -p super-stt-consent -- --ignored --nocapture
//! ```
//!
//! What's covered:
//!
//! - **renders_and_decides**: the binary launches, opens its window,
//!   and after a short wait either gets SIGTERM'd (simulating the user
//!   closing the dialog → expect `dismissed`) or — if the human running
//!   the test clicks Allow / Deny during the wait — writes `allow` /
//!   `deny` instead. The assertion accepts any of the three valid
//!   decisions so the test doesn't fail spuriously when run
//!   interactively.
//!
//!   This validates:
//!     - `cosmic::app::run` boots without panicking against the real
//!       compositor;
//!     - the env-var contract (`STT_AUTH_APP_NAME` / `STT_AUTH_SCOPES` /
//!       `STT_AUTH_EXE_PATH`) is wired up;
//!     - all three decision paths (Allow / Deny / dismissed) write a
//!       recognizable line to stdout.
//!
//! - **surface_survives_compositor_handshake**: the helper is still alive
//!   after the surface is up, and the compositor never raised a Wayland
//!   protocol error against it. See that test for why a dialog that dies
//!   during the handshake is invisible in production.

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HELPER_BIN: &str = env!("CARGO_BIN_EXE_super-stt-consent");

/// How long to let the surface settle before judging it healthy. The
/// corner-radius protocol violation this guards against killed the helper
/// ~500 ms in, so this leaves generous headroom on a slow machine.
const HANDSHAKE_GRACE: Duration = Duration::from_millis(2500);

/// Returns Some(reason) if no display is available — caller should skip
/// the test rather than fail.
fn skip_if_no_display() -> Option<&'static str> {
    let has_x11 = std::env::var_os("DISPLAY").is_some();
    let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if has_x11 || has_wayland {
        None
    } else {
        Some("no DISPLAY / WAYLAND_DISPLAY — skipping GUI test")
    }
}

#[test]
#[ignore = "spawns a real libcosmic window — run manually with cargo test -- --ignored"]
fn renders_and_decides() {
    if let Some(reason) = skip_if_no_display() {
        eprintln!("{reason}");
        return;
    }

    let mut child = Command::new(HELPER_BIN)
        .env("STT_AUTH_APP_NAME", "Smoke Test App")
        .env("STT_AUTH_SCOPES", "transcribe status")
        .env("STT_AUTH_EXE_PATH", "/usr/bin/smoke-test")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn super-stt-consent");

    // Give the window a couple of seconds to render. We don't have a
    // signal that says "window is up" without GUI-automation tooling,
    // so we just wait long enough that any startup panic would have
    // surfaced. If the human running this clicks Allow / Deny during
    // this window, the helper exits on its own — we'll observe that
    // via try_wait below and skip the SIGTERM step.
    std::thread::sleep(Duration::from_secs(2));

    let already_exited = matches!(child.try_wait(), Ok(Some(_)));

    if !already_exited {
        // Helper hasn't been clicked; close it via SIGTERM (exercises
        // the dismissed-on-close path). SIGTERM lets the signal handler
        // write "dismissed" before the helper exits.
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }
    }

    // Bound the wait so a runaway child can't hang the test.
    let deadline = Instant::now() + Duration::from_secs(10);
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("consent helper did not exit within 10s of SIGTERM");
            }
            Err(e) => panic!("error waiting on child: {e}"),
        }
    };

    let mut stdout = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }

    // Accept any of the three valid decisions on stdout. SIGTERM →
    // "dismissed"; user-clicked Allow → "allow"; user-clicked Deny →
    // "deny". This way the test isn't path-coupled to the SIGTERM
    // dismissal flow, so a human running interactively can click any
    // button without breaking the test.
    let valid = ["allow", "deny", "dismissed"];
    let decision = stdout
        .lines()
        .find(|l| valid.contains(&l.trim()))
        .map(str::trim);

    assert!(
        decision.is_some(),
        "expected one of {valid:?} on stdout;\n\
         got: {stdout:?}\n\
         exit status: {exit_status}"
    );
    eprintln!("[smoke] consent helper decision: {decision:?}");
}

/// The dialog's layer surface must survive the compositor handshake.
///
/// Regression guard. libcosmic applies the active theme's corner radii to
/// every surface it tracks, but this dialog is autosized — the surface exists
/// at its 1x1 `size_limits` floor before the dialog is measured, and a radius
/// wider than the surface is a protocol violation. cosmic-comp answers
/// `cosmic_corner_radius_layer_v1: error 1: corner radius too large` by
/// killing the client, so the helper died ~500 ms in without ever painting.
///
/// Nothing downstream reports that as a failure: the daemon reads the closed
/// stdout as [`ConsentDecision::Dismissed`], every consent prompt silently
/// denies, and the only user-visible symptom is a client stuck retrying
/// against `auth_denied (user_dismissed)`. Hence a test that watches the
/// process and the compositor's complaints rather than just the decision —
/// `renders_and_decides` treats an early exit as "the human clicked a button"
/// and sails straight past a crash this size.
#[test]
#[ignore = "spawns a real libcosmic window — run manually with cargo test -- --ignored"]
fn surface_survives_compositor_handshake() {
    if let Some(reason) = skip_if_no_display() {
        eprintln!("{reason}");
        return;
    }

    // Route stderr to a file rather than a pipe: libcosmic logs thousands of
    // lines at startup, and an undrained pipe would fill and block the child
    // — which looks exactly like the hang we're testing for.
    let log_path: PathBuf =
        std::env::temp_dir().join(format!("stt-consent-smoke-{}.log", std::process::id()));
    let log = File::create(&log_path).expect("create stderr capture file");

    let mut child = Command::new(HELPER_BIN)
        .env("STT_AUTH_APP_NAME", "Handshake Test App")
        .env("STT_AUTH_SCOPES", "transcribe status")
        .env("STT_AUTH_EXE_PATH", "/usr/bin/smoke-test")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn super-stt-consent");

    std::thread::sleep(HANDSHAKE_GRACE);

    let early_exit = match child.try_wait() {
        Ok(status) => status,
        Err(e) => panic!("error waiting on child: {e}"),
    };

    if early_exit.is_none() {
        #[cfg(unix)]
        unsafe {
            libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        {
            let _ = child.kill();
        }
    }

    let status = match early_exit {
        Some(status) => status,
        None => child.wait().expect("wait on child"),
    };

    let mut stdout = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    let stderr = std::fs::read_to_string(&log_path).unwrap_or_default();
    let _ = std::fs::remove_file(&log_path);

    // The compositor's verdict on our surface. Any protocol error means we
    // sent something it refused to accept, and it hung up on us.
    let protocol_error = stderr
        .lines()
        .find(|l| l.contains("Protocol error") || l.contains("corner radius too large"));
    assert!(
        protocol_error.is_none(),
        "compositor raised a Wayland protocol error against the consent \
         surface: {}\n(exit status: {status})",
        protocol_error.unwrap_or_default()
    );

    // An early exit is only legitimate if the helper reached a decision — a
    // human clicking Allow/Deny, or the debug auto-approve timer. Exiting
    // without one means it died on the way up.
    if early_exit.is_some() {
        let decided = stdout
            .lines()
            .any(|l| matches!(l.trim(), "allow" | "deny" | "dismissed"));
        assert!(
            decided,
            "consent helper exited within {HANDSHAKE_GRACE:?} without writing a \
             decision — it never got its dialog on screen.\n\
             exit status: {status}\n\
             stderr tail:\n{}",
            tail(&stderr, 20)
        );
    }
}

/// Last `n` lines of `text`, for assertion messages.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}
