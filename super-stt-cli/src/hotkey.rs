// SPDX-License-Identifier: GPL-3.0-only
//! `stt hotkey` — a global keyboard shortcut that starts and stops a
//! recording, for macOS.
//!
//! On Linux the desktop environment owns global shortcuts (Wayland gives no
//! client a system-wide key grab), and it is the DE that runs
//! `stt record --write` when the key is pressed. macOS has no equivalent place
//! to bind a key to a command, so this subcommand stands in for the DE: a
//! long-running client that owns one shortcut and, on each press, does exactly
//! what `stt record --write` does, through the same function. The daemon never
//! learns anything about input.
//!
//! Registration goes through Carbon's `RegisterEventHotKey`, by way of the
//! `global-hotkey` crate. That API needs no Accessibility or Input Monitoring
//! grant, which matters here more than usual: a TCC grant is one more thing
//! that silently stops matching when a binary changes. The crate switches to a
//! `CGEventTap` — which does need Input Monitoring — only for media keys, so a
//! binding naming one of those will want that permission.

use anyhow::{Context, Result, anyhow};
use clap::{Arg, ArgAction, Command};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use std::path::PathBuf;
use super_stt_shared::daemon::session::AppId;

/// The listener's own identity to the daemon, apart from `stt`'s.
///
/// In the macOS app bundle the two are copies of one binary at different
/// paths: `stt` in `Contents/MacOS`, and the listener in `Contents/Helpers`,
/// since beside the settings app its event loop would register with macOS
/// as the app itself. The daemon binds each session to the path that
/// obtained it, so sharing `stt`'s keychain entry would have each revoke the
/// other's session every time it was used.
const APP_ID: AppId = AppId("super-stt-hotkey");
const APP_NAME: &str = "Super STT Shortcut";

/// ⌃⌥Space. The obvious candidates are taken: ⌘Space is Spotlight, ⌃Space
/// switches input sources, and ⌥Space is the `ChatGPT` app's default.
pub const DEFAULT_BINDING: &str = "ctrl+alt+space";

pub fn command() -> Command {
    Command::new("hotkey")
        .about("Listen for a global shortcut that starts and stops recording (macOS)")
        .long_about(
            "Listen for a global shortcut that starts and stops recording.\n\n\
             Runs until stopped. Each press does what `stt record --write` does: \
             starts a recording, or stops the one in progress and types the result \
             into the focused window. The Super STT app runs this as a \
             LaunchAgent so it is available from login.",
        )
        .arg(
            Arg::new("key")
                .long("key")
                .value_name("BINDING")
                .default_value(DEFAULT_BINDING)
                .help(
                    "Shortcut to listen for, as modifiers and a key joined by `+`: \
                     ctrl, alt/option, cmd/super, shift, then a key such as space or KeyR",
                ),
        )
        .arg(
            Arg::new("check")
                .long("check")
                .help("Only validate the binding, then exit")
                .action(ArgAction::SetTrue),
        )
}

/// Parse a binding, with an error that says what the accepted form is.
fn parse_binding(binding: &str) -> Result<HotKey> {
    binding.parse().map_err(|e| {
        anyhow!(
            "`{binding}` is not a shortcut ({e}). \
             Use modifiers and one key joined by `+`, e.g. `{DEFAULT_BINDING}` or `cmd+shift+KeyR`."
        )
    })
}

/// `--check`: validate without registering anything, so an installer can
/// refuse a bad binding before writing it into a `LaunchAgent` that would
/// otherwise restart the listener every few seconds, failing the same way
/// each time.
pub fn check(binding: &str) -> Result<()> {
    parse_binding(binding).map(|_| ())
}

/// Register `binding` and serve presses until the process is stopped.
///
/// Must be called on the main thread before any tokio runtime exists: the
/// crate's macOS backend needs an event loop running on the main thread, and
/// that loop — `NSApplication::run` — never returns. The runtime that talks to
/// the daemon lives on worker threads instead.
pub fn run(socket_path: PathBuf, binding: &str) -> Result<()> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| anyhow!("`stt hotkey` must run on the process's main thread"))?;

    let hotkey = parse_binding(binding)?;
    // Held for the life of the process: dropping the manager unregisters the
    // shortcut.
    let manager = GlobalHotKeyManager::new().context("could not start the hotkey manager")?;
    manager.register(hotkey).with_context(|| {
        format!("could not register {binding}; another app may already be using it")
    })?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?;
    let handle = runtime.handle().clone();
    let id = hotkey.id();
    let label = binding.to_string();

    // Presses are handled one at a time, in order, on this thread. The
    // start-or-stop decision in `cmd_record` comes from the daemon's `busy`
    // flag, so two presses in flight together would both read "not busy" and
    // both try to start; serialized, the second press sees the recording the
    // first one began and stops it.
    std::thread::Builder::new()
        .name("hotkey-dispatch".into())
        .spawn(move || {
            while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
                if event.id != id || event.state != HotKeyState::Pressed {
                    continue;
                }
                log::debug!("{label} pressed");
                let result = handle.block_on(crate::run_with_token_as(
                    APP_ID,
                    APP_NAME,
                    socket_path.clone(),
                    |token| crate::cmd_record(socket_path.clone(), token, true, false, None),
                ));
                if let Err(e) = result {
                    // Logged, not fatal: the daemon being down or restarting is
                    // no reason to stop listening for the next press.
                    log::warn!("{label}: {e:#}");
                }
            }
        })
        .context("could not start the hotkey dispatch thread")?;

    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon, no menu bar, never frontmost. This process has no UI; the
    // application object exists only to run the event loop the shortcut is
    // delivered on.
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);

    log::info!("Listening for {binding}; press it to start or stop a recording");
    app.run();

    // `run` returns only if something sends NSApp `stop:`, which nothing here
    // does. Named so the order they are released in is not left to chance.
    drop(manager);
    drop(runtime);
    Ok(())
}
