// SPDX-License-Identifier: GPL-3.0-only
//! Super STT consent dialog — a small floating libcosmic window that the
//! daemon spawns when an app asks to authenticate.
//!
//! The daemon spawns this binary with env vars carrying the request details:
//!
//! - `STT_AUTH_APP_NAME` — declared (untrusted) app name from the request.
//!   Shown for a native caller, never for a web one — see [`Caller`].
//! - `STT_AUTH_SCOPES`   — space-separated scope set (e.g. `transcribe status`)
//! - `STT_AUTH_EXE_PATH` — peer `/proc/<pid>/exe` (trusted, kernel-resolved)
//! - `STT_AUTH_FLATPAK_APP_ID` — set only when the caller is inside a flatpak
//! - `STT_AUTH_WEB_ORIGIN` — set only when the caller reached the daemon over
//!   its TCP listener, carrying the browser-reported origin. The daemon sets
//!   this *or* `STT_AUTH_EXE_PATH`, never both, and this one wins.
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

use cosmic::iced::event::{self, listen_with};
use cosmic::iced::platform_specific::shell::commands::corner_radius::corner_radius;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, Layer,
};
use cosmic::iced::runtime::platform_specific::wayland::CornerRadius;
use cosmic::iced::runtime::platform_specific::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::window::Id as SurfaceId;
use cosmic::iced::{Limits, Size, Subscription};
use cosmic::prelude::*;
use cosmic::surface::{action as surface_action, surface_task};
use cosmic::widget::{button, dialog, icon, text};
use std::io::Write;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

const APP_ID: &str = "ai.menjivar.super-stt-consent";

const QUESTION_ICON: &[u8] = include_bytes!("../resources/icons/phosphor/question.svg");

/// Floor for the autosized surface, in logical pixels. Only applies for the
/// frames before `autosize` measures the dialog.
const MIN_SURFACE_DIM: f32 = 1.0;

/// No compositor-side corner clip. The state the surface starts in, and the
/// only safe one until it has a size — see [`ConsentApp::sync_corner_radius`].
const SQUARE_CORNERS: CornerRadius = CornerRadius {
    top_left: 0,
    top_right: 0,
    bottom_left: 0,
    bottom_right: 0,
};

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

/// How long the dialog stays up before dismissing itself.
///
/// Deliberately longer than the daemon's own one-minute wait, so this
/// never truncates a decision the daemon would still have accepted —
/// it is a backstop for a dialog that outlived whoever asked for it,
/// not a second deadline. The daemon reaps the helper itself, but only
/// while the request that spawned it is alive; nothing can kill a
/// dialog whose daemon has since exited. Left to wait forever such a
/// dialog holds an exclusive-keyboard layer surface and ~40 MB.
const DISMISS_AFTER: std::time::Duration = std::time::Duration::from_secs(90);

#[derive(Clone, Debug)]
enum Message {
    Allow,
    Deny,
    /// The compositor configured our surface to `Size`. Carries the id so a
    /// stray event for another surface can't drive our corner radius.
    Resized(SurfaceId, Size),
}

struct ConsentApp {
    core: cosmic::Core,
    surface_id: SurfaceId,
    app_name: String,
    scopes: Vec<String>,
    caller: Caller,
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
        mut core: cosmic::Core,
        flags: Self::Flags,
    ) -> (Self, cosmic::Task<cosmic::Action<Self::Message>>) {
        let surface_id = SurfaceId::unique();

        // We draw a layer-shell overlay, not an application window, so the
        // dialog belongs to the system interface: its translucency follows the
        // theme's "frosted system interface" toggle rather than the one for
        // regular windows.
        core.set_app_type(cosmic::core::AppType::System);

        // Spawn a layer-shell overlay surface. Tiling compositors do
        // not tile layer-shell surfaces, so this is the protocol-level
        // way to guarantee the dialog floats. KeyboardInteractivity is
        // Exclusive so the dialog grabs all keyboard input until the
        // user makes a choice (matches cosmic-osd's polkit dialog).
        //
        // Routed through `cosmic::surface` rather than the raw
        // `get_layer_surface` command so libcosmic tracks the surface and owns
        // its frosted-glass blur, re-applying it when the theme changes. A
        // surface spawned directly is invisible to that bookkeeping, which
        // leaves it translucent with nothing blurred behind it whenever the
        // theme has frosted glass on.
        let task = surface_task(surface_action::app_layer_shell::<Self>(
            |_| surface_action::LiveSettings {
                // Inherit the theme's blur decision.
                blur: None,
                // Corners start square and are rounded once the surface has a
                // size. libcosmic would otherwise round a tracked layer
                // surface to the theme's radius the moment it's created —
                // before the compositor has configured any size for it — and
                // cosmic-comp answers a radius wider than the surface it's
                // clipping with `cosmic_corner_radius_layer_v1: error 1:
                // corner radius too large`, killing the client. Raising the
                // surface's `size_limits` floor does not help; the request
                // goes out ahead of the first configure either way.
                corners: Some(SQUARE_CORNERS),
                padding: None,
            },
            move |_: &mut Self| SctkLayerSurfaceSettings {
                id: surface_id,
                keyboard_interactivity: KeyboardInteractivity::Exclusive,
                anchor: Anchor::empty(),
                namespace: "stt-consent".into(),
                layer: Layer::Overlay,
                size: None,
                size_limits: Limits::NONE
                    .min_width(MIN_SURFACE_DIM)
                    .min_height(MIN_SURFACE_DIM),
                ..Default::default()
            },
            // No dedicated view: libcosmic falls back to `view_window`.
            None,
        ));

        (
            Self {
                core,
                surface_id,
                app_name: flags.app_name,
                scopes: flags.scopes,
                caller: flags.caller,
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
        // What the user is being asked to approve, named the way each kind of
        // caller is best named.
        //
        // A native caller is named by `app_name`, with its verified path on the
        // line below — the name is self-reported, and the path is what the user
        // is really approving.
        //
        // A web caller is named by its origin, and `app_name` is not shown at
        // all. The origin is the only thing about a web caller that anything
        // vouches for, and it is also what the user recognises: browsers say
        // "example.com wants to use your microphone" for the same reason. Since
        // a page could otherwise put a string of its own choosing in front of
        // the user, leaving `app_name` out is also what stops a site from
        // labelling itself something it is not.
        let body_text = match &self.caller {
            Caller::Native { .. } => {
                let name = if self.app_name.is_empty() {
                    "An application"
                } else {
                    &self.app_name
                };
                format!("{name} wants access to Super STT.")
            }
            Caller::Web { origin } => format!("{origin} wants access to Super STT."),
        };

        let permission_lines = super_stt_shared::consent::permissions_for_scopes(&self.scopes);

        let mut bullet_column =
            cosmic::widget::column::with_capacity(permission_lines.len()).spacing(6);
        for line in permission_lines {
            bullet_column = bullet_column.push(bullet_row(line));
        }

        let mut control = cosmic::widget::column::with_capacity(3).spacing(12);

        // The verified identity, on its own line under the sentence. Only a
        // native caller gets one: for a web caller the sentence already *is*
        // the verified identity, and repeating the origin underneath would
        // read as a second, corroborating fact when there is only one.
        if let Caller::Native {
            exe_path,
            flatpak_app_id,
        } = &self.caller
        {
            control = control.push(text::body(match flatpak_app_id {
                // Name the app the user installed. Its executable path lives
                // inside its sandbox, so it identifies nothing here.
                Some(id) => format!("Flatpak app:  {id}"),
                None => format!("Executable:  {exe_path}"),
            }));
        }

        control = control.push(
            cosmic::widget::column::with_capacity(2)
                .push(text::heading("This will allow it to:"))
                .push(bullet_column)
                .spacing(6),
        );

        // A native caller is named by the kernel and cannot lie about it. A web
        // caller is named by its browser, and the user should be told that
        // before they grant anything — it is the difference between a checked
        // identity and a reported one.
        if matches!(self.caller, Caller::Web { .. }) {
            control = control.push(text::caption(
                "Your browser reports which website this is. That is a weaker check \
                 than Super STT does for installed programs.",
            ));
        }

        let dialog_widget = dialog::dialog()
            // `Container::Dialog` only honours the theme's translucency when
            // the dialog is *not* an overlay — an overlay sits on a dimmed
            // backdrop inside an app window, where a see-through panel would
            // read as a rendering bug. Ours is a standalone surface with the
            // desktop behind it, so it takes the frosted treatment instead.
            .is_overlay(false)
            .title("Allow access to Super STT?")
            .body(body_text)
            .icon(
                icon::from_svg_bytes(QUESTION_ICON)
                    .symbolic(true)
                    .icon()
                    .size(64),
            )
            .control(control)
            .primary_action(button::suggested("Allow").on_press(Message::Allow))
            .secondary_action(button::destructive("Deny").on_press(Message::Deny));

        // `autosize` measures the dialog widget and resizes the
        // layer-shell surface to match. Without it the surface stays
        // 0×0 and the dialog is invisible.
        cosmic::widget::autosize::autosize(dialog_widget, AUTOSIZE_ID.clone())
            .min_width(MIN_SURFACE_DIM)
            .min_height(MIN_SURFACE_DIM)
            .into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        // `Opened` covers the compositor's first configure, `Resized` any
        // later one — a font-scale change, say. Both feed the corner radius.
        listen_with(|ev, _status, id| match ev {
            event::Event::Window(
                cosmic::iced::window::Event::Opened { size, .. }
                | cosmic::iced::window::Event::Resized(size),
            ) => Some(Message::Resized(id, size)),
            _ => None,
        })
    }

    fn update(&mut self, message: Self::Message) -> cosmic::Task<cosmic::Action<Self::Message>> {
        let decision = match message {
            Message::Allow => ALLOW,
            Message::Deny => DENY,
            Message::Resized(id, size) => return self.sync_corner_radius(id, size),
        };
        // Write the decision to stdout and exit; the compositor tears the
        // surface down when the process goes away. Returning a destroy task
        // instead would never run — `emit_and_exit` diverges before the
        // runtime gets a chance to execute it.
        emit_and_exit(decision);
    }
}

impl ConsentApp {
    /// Match the compositor's corner clip to the rounded border the dialog
    /// widget draws, now that the surface has a size to measure against.
    ///
    /// Without this the surface stays a square the dialog is painted into, and
    /// its background shows through outside the rounded border at each corner.
    /// The radius is clamped to half the shorter side: cosmic-comp rejects
    /// anything wider than the surface it's clipping, and rejection means a
    /// protocol error that kills the process, not a clamp on its end. Same
    /// shape as the cosmic-osd indicator, which rounds on `Msg::Size`.
    fn sync_corner_radius(
        &self,
        id: SurfaceId,
        size: Size,
    ) -> cosmic::Task<cosmic::Action<Message>> {
        if id != self.surface_id {
            return cosmic::Task::none();
        }
        // Degenerate sizes are the pre-configure state; stay square. The
        // finite check also rejects NaN, which would otherwise slip past the
        // comparison and clamp to nothing.
        let limit = size.width.min(size.height) / 2.0;
        if !limit.is_finite() || limit < 1.0 {
            return cosmic::Task::none();
        }
        // `radius_m` is what `Container::Dialog` rounds its own border to, so
        // the clip lands exactly on the edge the user sees.
        let radii = cosmic::theme::active().cosmic().corner_radii.radius_m;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let clamp = |r: f32| r.min(limit).max(0.0).round() as u32;
        corner_radius(
            self.surface_id,
            Some(CornerRadius {
                top_left: clamp(radii[0]),
                top_right: clamp(radii[1]),
                bottom_right: clamp(radii[2]),
                bottom_left: clamp(radii[3]),
            }),
        )
        .discard()
    }
}

/// Render one bullet line. Uses a Row so wrapped text hangs under
/// itself instead of slipping behind the bullet character.
fn bullet_row(line: &str) -> Element<'_, Message> {
    cosmic::widget::row::with_capacity(2)
        .push(text::body("•"))
        .push(text::body(line))
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Start)
        .into()
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

/// Dismiss the dialog if nobody has decided within [`DISMISS_AFTER`].
///
/// Unlike the auto-approve timer below this is compiled into release
/// builds: it only ever denies, so it cannot be used to wave a consent
/// prompt through. Fail-closed is also the right answer when it does
/// fire — reaching the deadline means no human answered.
fn spawn_dismiss_timer() {
    std::thread::spawn(|| {
        std::thread::sleep(DISMISS_AFTER);
        // Claim the decision. If someone else got there first, their
        // verdict is already on stdout and must not be followed by
        // ours — the daemon reads one line and we would be racing to
        // define it.
        if DECIDED
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            unsafe { libc::_exit(0) }
        }
        // Same async-signal-safe write + `_exit` as the SIGTERM handler,
        // rather than trying to unwind the iced event loop from a thread
        // that does not own it.
        let msg = b"dismissed\n";
        unsafe {
            libc::write(1, msg.as_ptr().cast::<libc::c_void>(), msg.len());
            libc::_exit(0);
        }
    });
}

/// If `STT_AUTH_AUTO_APPROVE_AFTER_MS` is set to a parseable u64,
/// spawn a background thread that sleeps for that many milliseconds
/// and then writes "allow\n" to stdout + `_exit(0)`. Lets you (or a
/// test runner) actually see the dialog render for that long before
/// it auto-approves itself.
///
/// This is intended for integration tests / CI smoke runs, NOT for
/// production. The auto-approval bypasses any user input, so the whole
/// path is compiled out of release builds — a shipped consent gate must
/// never be able to self-approve from an env var (Tier 1 #30). `cargo test`
/// builds with `debug_assertions` on, so the smoke tests keep working.
#[cfg(debug_assertions)]
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

/// Release builds never auto-approve — the env-var bypass is debug/test only.
#[cfg(not(debug_assertions))]
fn maybe_spawn_auto_approve_timer() {}

/// Who the daemon says is asking.
///
/// Mirrors the daemon's own `PeerIdentity`, and for the same reason: the two
/// kinds warrant different sentences. "Allow this program" and "allow this
/// website" are not the same question, and the second comes with a weaker
/// guarantee behind it — so the dialog has to know which one it is asking
/// rather than infer it from an empty field.
enum Caller {
    /// A program on this machine, named by the path the kernel resolved.
    Native {
        exe_path: String,
        /// Set only for a sandboxed caller. Its `exe_path` is resolved inside
        /// its own sandbox, so showing that path would name a file the user
        /// cannot go and look at, and one that every other flatpak could
        /// equally present.
        flatpak_app_id: Option<String>,
    },
    /// A web page, named by the origin its browser reported.
    Web { origin: String },
}

struct AuthRequestPayload {
    app_name: String,
    scopes: Vec<String>,
    caller: Caller,
}

fn read_env() -> AuthRequestPayload {
    AuthRequestPayload {
        app_name: std::env::var("STT_AUTH_APP_NAME")
            .unwrap_or_else(|_| "<unknown app>".to_string()),
        scopes: std::env::var("STT_AUTH_SCOPES")
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        caller: read_caller(),
    }
}

/// Which kind of caller the daemon is asking about.
///
/// `STT_AUTH_WEB_ORIGIN` wins when present because the daemon sets exactly one
/// of the two — see its `ask_user_for_consent`. Reading it first means a
/// stale `STT_AUTH_EXE_PATH` inherited from the environment can never turn a
/// website into an executable in the sentence the user reads.
fn read_caller() -> Caller {
    if let Ok(origin) = std::env::var("STT_AUTH_WEB_ORIGIN")
        && !origin.is_empty()
    {
        return Caller::Web { origin };
    }
    Caller::Native {
        exe_path: std::env::var("STT_AUTH_EXE_PATH")
            .unwrap_or_else(|_| "<unknown path>".to_string()),
        flatpak_app_id: std::env::var("STT_AUTH_FLATPAK_APP_ID")
            .ok()
            .filter(|id| !id.is_empty()),
    }
}

fn main() -> cosmic::iced::Result {
    super_stt_shared::logging::init();

    install_termination_handlers();
    spawn_dismiss_timer();
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
