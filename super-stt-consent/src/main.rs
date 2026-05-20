// SPDX-License-Identifier: GPL-3.0-only
//! Super STT consent dialog — a small floating libcosmic window that the
//! daemon spawns when an app asks to authenticate.
//!
//! The daemon spawns this binary with three env vars carrying the request
//! details:
//!
//! - `STT_AUTH_APP_NAME` — declared (untrusted) app name from the request
//! - `STT_AUTH_SCOPE`    — `client` / `settings` / `widget`
//! - `STT_AUTH_EXE_PATH` — peer `/proc/<pid>/exe` (trusted, kernel-resolved)
//!
//! The user clicks Allow or Deny. The dialog writes one of `allow`, `deny`,
//! or `dismissed` to stdout (newline-terminated) and exits.
//!
//! ## Floating-window behavior
//!
//! The dialog is rendered as a Wayland **layer-shell** surface
//! (`Layer::Overlay`, `KeyboardInteractivity::Exclusive`,
//! `anchor: Anchor::empty()`). Tiling compositors (cosmic-comp, sway,
//! Hyprland, niri, etc.) treat layer-shell surfaces as overlays — they
//! are NOT subject to tiling logic — so the consent dialog floats
//! everywhere by construction. This is the same protocol cosmic-osd
//! uses for the polkit-auth dialog and the volume/brightness OSDs.
//!
//! For X11 sessions, layer-shell isn't available; libcosmic falls back
//! to a regular window. On X11 tiling WMs you'd add a per-class float
//! rule using the `WM_CLASS` `super-stt-consent`.

use cosmic::iced::Limits;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, Layer, destroy_layer_surface, get_layer_surface,
};
use cosmic::iced::runtime::platform_specific::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::window::Id as SurfaceId;
use cosmic::prelude::*;
use cosmic::widget::{button, dialog, icon, text};
use std::io::Write;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

const APP_ID: &str = "ai.menjivar.super-stt-consent";

/// Stable id for the autosize widget that wraps the dialog body.
/// `widget::autosize::autosize` measures its child to size the parent
/// layer-shell surface; without it the surface is 0×0.
static AUTOSIZE_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("super-stt-consent-autosize"));

/// Decision payload written to stdout right before exit.
const ALLOW: &str = "allow";
const DENY: &str = "deny";
const DISMISSED: &str = "dismissed";

/// Set the moment the user picks Allow or Deny. Used by the
/// signal handler to skip an extra "dismissed" line if the user
/// already decided.
static DECIDED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
enum Message {
    Allow,
    Deny,
}

struct ConsentApp {
    core: cosmic::Core,
    surface_id: SurfaceId,
    app_name: String,
    scope: String,
    exe_path: String,
}

impl cosmic::Application for ConsentApp {
    type Executor = cosmic::executor::Default;
    type Flags = AuthRequestPayload;
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(
        core: cosmic::Core,
        flags: Self::Flags,
    ) -> (Self, cosmic::Task<cosmic::Action<Self::Message>>) {
        let surface_id = SurfaceId::unique();

        // Spawn a layer-shell overlay surface. Tiling compositors do
        // not tile layer-shell surfaces, so this is the protocol-level
        // way to guarantee the dialog floats. KeyboardInteractivity is
        // Exclusive so the dialog grabs all keyboard input until the
        // user makes a choice (matches cosmic-osd's polkit dialog).
        let task = get_layer_surface(SctkLayerSurfaceSettings {
            id: surface_id,
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            anchor: Anchor::empty(),
            namespace: "stt-consent".into(),
            layer: Layer::Overlay,
            size: None,
            size_limits: Limits::NONE.min_width(1.0).min_height(1.0),
            ..Default::default()
        });

        (
            Self {
                core,
                surface_id,
                app_name: flags.app_name,
                scope: flags.scope,
                exe_path: flags.exe_path,
            },
            task,
        )
    }

    fn view(&self) -> Element<'_, Self::Message> {
        // We have no main window; the surface is rendered by view_window.
        // Returning a tiny placeholder keeps libcosmic happy on the rare
        // build flavors that hit this path.
        cosmic::widget::text::body("").into()
    }

    fn view_window(&self, id: SurfaceId) -> Element<'_, Self::Message> {
        if id != self.surface_id {
            return cosmic::widget::text::body("").into();
        }

        // The layer-shell surface starts transparent; using
        // `widget::dialog::dialog()` gives us the proper themed
        // background, padding, and button layout — same widget the
        // cosmic-osd polkit dialog uses.
        let request_label = if self.app_name.is_empty() {
            "An application".to_string()
        } else {
            self.app_name.clone()
        };
        let body_text = format!(
            "{request_label} is requesting {} access to Super STT.",
            self.scope
        );

        let path_line = text::body(format!("Path:  {}", self.exe_path));
        let permission_lines = match self.scope.as_str() {
            "client" => cosmic::widget::column()
                .push(path_line)
                .push(text::body(""))
                .push(text::body("It will be able to:"))
                .push(text::body("• Start and stop recordings"))
                .push(text::body(
                    "• Receive its own preview text and final transcriptions",
                ))
                .spacing(2),
            "settings" => cosmic::widget::column()
                .push(path_line)
                .push(text::body(""))
                .push(text::body("It will be able to:"))
                .push(text::body("• Everything a client can do"))
                .push(text::body(
                    "• Read and modify any daemon configuration value",
                ))
                .push(text::body("• Receive cross-app state-change events"))
                .spacing(2),
            "widget" => cosmic::widget::column()
                .push(path_line)
                .push(text::body(""))
                .push(text::body("It will be able to:"))
                .push(text::body(
                    "• Subscribe to recording state, audio frames, and",
                ))
                .push(text::body("  optional transcription text (read-only)"))
                .spacing(2),
            other => cosmic::widget::column()
                .push(path_line)
                .push(text::body(""))
                .push(text::body(format!("Unknown scope: {other}")))
                .spacing(2),
        };

        let dialog_widget = dialog::dialog()
            .title("Allow access to Super STT?")
            .body(body_text)
            .icon(icon::from_name("dialog-question-symbolic").size(64))
            .control(permission_lines)
            .primary_action(button::suggested("Allow").on_press(Message::Allow))
            .secondary_action(button::destructive("Deny").on_press(Message::Deny));

        // `autosize` measures the dialog widget and resizes the
        // layer-shell surface to match. Without it the surface stays
        // 0×0 and the dialog is invisible.
        cosmic::widget::autosize::autosize(dialog_widget, AUTOSIZE_ID.clone())
            .min_width(1.0)
            .min_height(1.0)
            .into()
    }

    fn update(&mut self, message: Self::Message) -> cosmic::Task<cosmic::Action<Self::Message>> {
        let decision = match message {
            Message::Allow => ALLOW,
            Message::Deny => DENY,
        };
        // Tear down the layer surface gracefully so the compositor sees
        // a clean exit, then write the decision to stdout and exit.
        let _ = destroy_layer_surface::<Self::Message>(self.surface_id);
        emit_and_exit(decision);
    }
}

/// Write the decision to stdout, flush, then exit with `_exit(2)`.
/// Returns `!` so it can stand in for the `Task<...>` an
/// `Application::update` would normally return.
fn emit_and_exit(decision: &str) -> ! {
    DECIDED.store(true, Ordering::SeqCst);
    {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = writeln!(handle, "{decision}");
        let _ = handle.flush();
    }
    std::process::exit(0);
}

/// Async-signal-safe SIGTERM/SIGINT handler. Writes "dismissed\n" to
/// stdout (fd 1) using `write(2)`, then exits with `_exit(2)` —
/// both AS-Safe per POSIX. Skips the write if a button decision has
/// already been emitted.
extern "C" fn handle_termination_signal(_signum: libc::c_int) {
    if DECIDED.load(Ordering::SeqCst) {
        unsafe {
            libc::_exit(0);
        }
    }
    let msg = b"dismissed\n";
    unsafe {
        libc::write(1, msg.as_ptr().cast::<libc::c_void>(), msg.len());
        libc::_exit(0);
    }
}

fn install_termination_handlers() {
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_termination_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_termination_signal as *const () as libc::sighandler_t,
        );
    }
}

/// If `STT_AUTH_AUTO_APPROVE_AFTER_MS` is set to a parseable u64,
/// spawn a background thread that sleeps for that many milliseconds
/// and then writes "allow\n" to stdout + `_exit(0)`. Lets you (or a
/// test runner) actually see the dialog render for that long before
/// it auto-approves itself.
///
/// This is intended for integration tests / CI smoke runs, NOT for
/// production. The auto-approval bypasses any user input.
fn maybe_spawn_auto_approve_timer() {
    let Ok(raw) = std::env::var("STT_AUTH_AUTO_APPROVE_AFTER_MS") else {
        return;
    };
    let Ok(ms) = raw.parse::<u64>() else {
        log::warn!("STT_AUTH_AUTO_APPROVE_AFTER_MS={raw:?} is not a valid u64; ignoring");
        return;
    };
    log::info!("STT_AUTH_AUTO_APPROVE_AFTER_MS={ms}; auto-approving after {ms}ms");
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        // Mark decided so the SIGTERM/atexit paths don't also write
        // "dismissed" on top of our "allow".
        DECIDED.store(true, Ordering::SeqCst);
        // Async-signal-safe write + _exit, same shape as the SIGTERM
        // handler. Avoids contending with the iced event loop.
        let msg = b"allow\n";
        unsafe {
            libc::write(1, msg.as_ptr().cast::<libc::c_void>(), msg.len());
            libc::_exit(0);
        }
    });
}

struct AuthRequestPayload {
    app_name: String,
    scope: String,
    exe_path: String,
}

fn read_env() -> AuthRequestPayload {
    AuthRequestPayload {
        app_name: std::env::var("STT_AUTH_APP_NAME")
            .unwrap_or_else(|_| "<unknown app>".to_string()),
        scope: std::env::var("STT_AUTH_SCOPE").unwrap_or_else(|_| "client".to_string()),
        exe_path: std::env::var("STT_AUTH_EXE_PATH")
            .unwrap_or_else(|_| "<unknown path>".to_string()),
    }
}

fn main() -> cosmic::iced::Result {
    if std::env::var("RUST_LOG").is_ok() {
        env_logger::init();
    } else {
        env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .init();
    }

    install_termination_handlers();
    maybe_spawn_auto_approve_timer();

    let payload = read_env();

    // `no_main_window(true)` because we use a layer-shell surface
    // instead of the standard auto-created xdg_toplevel main window.
    // The surface is created from `init()` via `get_layer_surface(...)`.
    let settings = cosmic::app::Settings::default()
        .no_main_window(true)
        .exit_on_close(true);

    let result = cosmic::app::run::<ConsentApp>(settings, payload);

    // If we reach this without DECIDED being set, the user dismissed
    // the dialog without picking a button. Emit "dismissed".
    if !DECIDED.load(Ordering::SeqCst) {
        let _ = writeln!(std::io::stdout(), "{DISMISSED}");
        let _ = std::io::stdout().flush();
    }

    result
}
