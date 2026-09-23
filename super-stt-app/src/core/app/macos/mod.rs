// SPDX-License-Identifier: GPL-3.0-only
//! The window frame on macOS.
//!
//! libcosmic draws its own title bar — the header bar, with minimize, maximize
//! and close on the right — because on COSMIC every window looks like that. On
//! a Mac it reads as a foreign app: no traffic lights, and the controls on the
//! wrong side. So on macOS the app keeps the native frame and lays itself out
//! the way Finder, Notes and Mail do:
//!
//! - The window has a unified title bar, transparent, with the content
//!   extending under it ([`style_window`]). The title bar is as tall as a
//!   Finder toolbar and the traffic lights sit centred in it, over the
//!   top-left corner. Clicks anywhere along the top still reach the app.
//! - libcosmic's header bar is switched off. Its nav-bar toggle is always its
//!   first item — no app-supplied item can come before it — so it would sit
//!   exactly where the traffic lights are.
//! - The sidebar toggle sits just after the traffic lights and stays there:
//!   at the top of the sidebar while it is open ([`AppModel::macos_sidebar`]),
//!   at the start of the toolbar while it is closed.
//! - The GPU and backend readouts move from the header into
//!   [`AppModel::macos_toolbar`], a row across the top of the content. Both
//!   rows move the window when dragged and zoom it on a double-click, as a
//!   title bar does.
//! - The View menu moves to the menu bar ([`menu_bar`]). The header's logo
//!   and name have no counterpart: a Mac window names its app in the menu
//!   bar and the Dock.
//!
//! libcosmic builds the window itself and passes none of iced's macOS window
//! options through, so the title-bar changes are made on the `NSWindow`
//! directly, once the window exists.

use cosmic::iced::widget::mouse_area;
use cosmic::iced::{Alignment, Length};
use cosmic::iced::{Task, window};
use cosmic::widget::{self, space::horizontal as horizontal_space};
use cosmic::{Application, Element};
use std::cell::OnceCell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSToolbar, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowButton,
    NSWindowDidExitFullScreenNotification, NSWindowOrderingMode, NSWindowStyleMask,
    NSWindowTitleVisibility, NSWindowToolbarStyle, NSWindowWillEnterFullScreenNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter};

use super::AppModel;
use crate::ui::messages::{Message, ShellMessage};

pub(crate) mod appearance;
pub(crate) mod font;
pub(crate) mod menu_bar;

/// Where the title bar's controls are, as `AppKit` laid them out. Measured
/// when the window opens: the traffic lights move between macOS releases.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TitleBar {
    /// Height of the title bar, which the toolbar rows match so their
    /// controls centre on the traffic lights.
    height: f32,
    /// Distance from the window's leading edge to the end of the zoom button.
    lights_end: f32,
    /// The window is in full screen, where the title bar and its traffic
    /// lights hide with the menu bar.
    full_screen: bool,
}

impl Default for TitleBar {
    /// A unified title bar as macOS 26 lays it out.
    fn default() -> Self {
        Self {
            height: 52.0,
            lights_end: 79.0,
            full_screen: false,
        }
    }
}

/// Gap between the zoom button and the sidebar toggle.
const LIGHTS_GAP: f32 = 8.0;

/// libcosmic's padding between the window edge and the sidebar or content,
/// for a window it believes maximized — which is how the app presents a
/// natively framed window to it (see `init`).
const EDGE_PADDING: f32 = 8.0;

/// Give the window a transparent unified title bar with the content under it,
/// and report where `AppKit` put the traffic lights.
///
/// Runs on the main thread, as `AppKit` requires: iced runs window actions on its
/// event loop, which owns the main thread on macOS.
pub(super) fn style_window(handle: raw_window_handle::WindowHandle<'_>, frosted: bool) -> TitleBar {
    let Some(window) = ns_window(&handle) else {
        log::warn!("macOS window styling skipped: no NSWindow behind the handle yet");
        return TitleBar::default();
    };
    window.setTitlebarAppearsTransparent(true);
    window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    window.setStyleMask(window.styleMask() | NSWindowStyleMask::FullSizeContentView);
    add_toolbar(&window);
    drop_toolbar_in_full_screen(&window);
    add_backdrop(&window, frosted);

    let title_bar = measure(&window).unwrap_or_default();
    log::debug!("macOS window styled: {title_bar:?}");
    title_bar
}

/// Give the window an empty toolbar, which is what makes its title bar
/// unified: taller, with the traffic lights centred in it. The toolbar draws
/// nothing, and clicks over it still go to the content view beneath.
fn add_toolbar(window: &NSWindow) {
    let toolbar = NSToolbar::new(objc2::MainThreadMarker::from(window));
    window.setToolbar(Some(&toolbar));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
}

/// Take the toolbar away for as long as the window is in full screen.
///
/// There `AppKit` lifts the toolbar out of the window and draws it, opaque, as
/// a band under the menu bar and over the top of the content — through the
/// transition as well as after it. So the toolbar goes as the window starts
/// into full screen and comes back once it is all the way out, which is what
/// Electron does for the same title bar. Without it the title bar hides with
/// the menu bar, as full screen expects.
fn drop_toolbar_in_full_screen(window: &NSWindow) {
    let center = NSNotificationCenter::defaultCenter();
    let entering = RcBlock::new(|notification: NonNull<NSNotification>| {
        if let Some(window) = notifying_window(notification) {
            window.setToolbar(None);
        }
    });
    let exited = RcBlock::new(|notification: NonNull<NSNotification>| {
        if let Some(window) = notifying_window(notification) {
            add_toolbar(&window);
        }
    });
    let window: &AnyObject = window.as_ref();
    // SAFETY: the names are AppKit's own constants. The observed object is the
    // window, which is what posts these. With no queue given, the blocks run
    // on the posting thread — the main thread, where AppKit posts window
    // notifications and where the blocks' AppKit calls must run. The center
    // keeps both registrations for the window's lifetime, which is the app's.
    unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWindowWillEnterFullScreenNotification),
            Some(window),
            None,
            &entering,
        );
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWindowDidExitFullScreenNotification),
            Some(window),
            None,
            &exited,
        );
    }
}

/// The window a window notification is about.
fn notifying_window(notification: NonNull<NSNotification>) -> Option<Retained<NSWindow>> {
    // SAFETY: the notification center passes a notification that is live for
    // the call.
    let notification = unsafe { notification.as_ref() };
    notification.object()?.downcast::<NSWindow>().ok()
}

thread_local! {
    /// The blurred backdrop behind the main window's content; see
    /// [`add_backdrop`].
    static BACKDROP: OnceCell<Retained<NSVisualEffectView>> = const { OnceCell::new() };
}

/// Put a blurred backdrop behind the window's content, for frosted glass:
/// the view Mac apps' own translucency is made of. It blurs whatever is
/// behind the window and keeps to the window's rounded corners; winit's blur,
/// which libcosmic uses on Wayland, blurs the window's whole rectangle, and
/// shows it as a square box around the window. The backdrop shows only
/// through the theme's translucency.
fn add_backdrop(window: &NSWindow, frosted: bool) {
    let Some(content) = window.contentView() else {
        return;
    };
    let mtm = objc2::MainThreadMarker::from(window);
    let backdrop = NSVisualEffectView::initWithFrame(mtm.alloc(), content.bounds());
    // The least tinted of the materials that follow light and dark: the
    // closest to COSMIC's frosted glass, which is a plain blur under the
    // theme's own translucency.
    backdrop.setMaterial(NSVisualEffectMaterial::HUDWindow);
    backdrop.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    // Duller behind an inactive window, as Mac windows' translucency is.
    backdrop.setState(NSVisualEffectState::FollowsWindowActiveState);
    backdrop.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    backdrop.setHidden(!frosted);
    content.addSubview_positioned_relativeTo(&backdrop, NSWindowOrderingMode::Below, None);
    // iced draws into a Metal layer that wgpu adds to the content view's
    // layer, beside the layers of its subviews, where a subview's layer
    // lands on top of it. Core Animation stacks sibling layers by their z
    // position before their order, so this one goes to the back.
    backdrop.setWantsLayer(true);
    if let Some(layer) = backdrop.layer() {
        layer.setZPosition(-1.0);
    }
    BACKDROP.with(|slot| {
        let _ = slot.set(backdrop);
    });
}

/// Show or hide the backdrop, with frosted glass.
fn show_backdrop(frosted: bool) {
    BACKDROP.with(|slot| {
        if let Some(backdrop) = slot.get() {
            backdrop.setHidden(!frosted);
        }
    });
}

/// Whether the window is in full screen. Checked on every resize, which every
/// change in or out of full screen is.
pub(super) fn is_full_screen(handle: raw_window_handle::WindowHandle<'_>) -> bool {
    ns_window(&handle)
        .is_some_and(|window| window.styleMask().contains(NSWindowStyleMask::FullScreen))
}

/// The `NSWindow` behind an iced window handle, once it has one.
fn ns_window(handle: &raw_window_handle::WindowHandle<'_>) -> Option<Retained<NSWindow>> {
    let raw_window_handle::RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return None;
    };
    // SAFETY: an AppKit window handle's `ns_view` points at the window's live
    // content view for as long as the handle is borrowed, which covers this
    // function.
    let view: &NSView = unsafe { appkit.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

/// Read the title bar's height and the traffic lights' extent off the window.
fn measure(window: &NSWindow) -> Option<TitleBar> {
    let height = window.contentView()?.frame().size.height;
    let zoom = window.standardWindowButton(NSWindowButton::ZoomButton)?;
    // In window coordinates, whose origin is the bottom-left corner.
    let rect = zoom.convertRect_toView(zoom.bounds(), None);
    let from_top = height - (rect.origin.y + rect.size.height);
    #[allow(clippy::cast_possible_truncation)]
    Some(TitleBar {
        // The lights are centred vertically in the title bar.
        height: (2.0 * from_top + rect.size.height) as f32,
        lights_end: (rect.origin.x + rect.size.width) as f32,
        full_screen: false,
    })
}

impl AppModel {
    /// Set up a window that has just opened: its title bar, its blur, and —
    /// once, with the first — the menu bar and the watch on the system look.
    pub(crate) fn window_opened(&self, id: window::Id) -> Task<cosmic::Action<Message>> {
        let frosted = self.shown_look().frosted();
        let style = window::run_with_handle(id, move |handle| style_window(handle, frosted)).map(
            |title_bar| {
                cosmic::Action::App(Message::Shell(ShellMessage::TitleBarMeasured(title_bar)))
            },
        );
        // Window actions run on the main thread, after launch, as the menu bar
        // and the appearance observer need.
        let app_wide = window::run_with_handle(id, |_| {
            menu_bar::install();
            appearance::observe();
        });
        Task::batch([style, app_wide.discard()])
    }

    /// The look the window shows: the system's, but frosted only outside
    /// full screen. There is no desktop behind a full-screen window to see
    /// through to, only black, and a translucent theme over black is merely a
    /// dimmer one.
    pub(crate) fn shown_look(&self) -> appearance::Look {
        if self.title_bar.full_screen {
            self.look.unfrosted()
        } else {
            self.look
        }
    }

    /// Follow a new system look.
    pub(crate) fn follow_system_look(
        &mut self,
        look: appearance::Look,
    ) -> Task<cosmic::Action<Message>> {
        let shown = self.shown_look();
        self.look = look;
        self.reshow(shown)
    }

    /// Record whether the window is in full screen.
    pub(crate) fn full_screen_checked(
        &mut self,
        full_screen: bool,
    ) -> Task<cosmic::Action<Message>> {
        let shown = self.shown_look();
        self.title_bar.full_screen = full_screen;
        self.reshow(shown)
    }

    /// Retheme if the look shown has changed from `shown`, showing or hiding
    /// the blurred backdrop with frosted glass.
    fn reshow(&self, shown: appearance::Look) -> Task<cosmic::Action<Message>> {
        let look = self.shown_look();
        if look == shown {
            return Task::none();
        }
        log::debug!("showing the look: {look:?}");
        let blur = match self.core.main_window_id() {
            Some(id) if look.frosted() != shown.frosted() => {
                let frosted = look.frosted();
                window::run_with_handle(id, move |_| show_backdrop(frosted)).discard()
            }
            _ => Task::none(),
        };
        Task::batch([cosmic::command::set_theme(look.theme()), blur])
    }

    /// The sidebar toggle, sized for the title bar.
    fn sidebar_toggle(&self) -> Element<'_, Message> {
        let icon = if self.core.nav_bar_active() {
            "navbar-open-symbolic"
        } else {
            "navbar-closed-symbolic"
        };
        widget::button::icon(widget::icon::from_name(icon))
            .padding(8)
            .class(cosmic::theme::Button::NavToggle)
            .on_press(Message::Shell(ShellMessage::ToggleSidebar))
            .into()
    }

    /// Room for the traffic lights, then the sidebar toggle.
    fn leading_controls(&self) -> widget::Row<'_, Message, cosmic::Theme> {
        let room = if self.title_bar.full_screen {
            0.0
        } else {
            self.title_bar.lights_end + LIGHTS_GAP - EDGE_PADDING
        };
        let row = widget::row::with_capacity(2).push(horizontal_space().width(room));
        if self.nav_model_impl().is_some() {
            row.push(self.sidebar_toggle())
        } else {
            // No sidebar to toggle: the daemon is disconnected.
            row
        }
    }

    /// A strip as tall as the title bar, moving the window when dragged and
    /// zooming it on a double-click.
    fn title_bar_strip<'a>(
        &self,
        row: widget::Row<'a, Message, cosmic::Theme>,
    ) -> Element<'a, Message> {
        mouse_area(
            widget::container(row.align_y(Alignment::Center).height(Length::Fill))
                .width(Length::Fill)
                .height(self.title_bar.height),
        )
        .on_press(Message::Shell(ShellMessage::DragWindow))
        .on_double_click(Message::Shell(ShellMessage::ZoomWindow))
        .into()
    }

    /// The sidebar, below a strip holding the toggle beside the traffic
    /// lights; `None` while it is closed.
    ///
    /// The sidebar itself is libcosmic's default `nav_bar`, which an override
    /// cannot call.
    pub(super) fn macos_sidebar(&self) -> Option<Element<'_, cosmic::Action<Message>>> {
        if !self.core.nav_bar_active() {
            return None;
        }
        let nav_model = self.nav_model_impl()?;
        let mut nav = widget::nav_bar(nav_model, |id| {
            cosmic::Action::Cosmic(cosmic::app::Action::NavBar(id))
        })
        .on_context(|id| cosmic::Action::Cosmic(cosmic::app::Action::NavBarContext(id)))
        .context_menu(self.nav_context_menu())
        .into_container()
        .width(Length::Shrink)
        .height(Length::Fill);
        if !self.core.is_condensed() {
            nav = nav.max_width(280);
        }
        let strip = self
            .title_bar_strip(self.leading_controls())
            .map(cosmic::Action::App);
        Some(
            widget::column::with_capacity(2)
                .push(strip)
                .push(nav)
                .width(Length::Shrink)
                .into(),
        )
    }

    /// The row across the top of the content, holding the readouts
    /// libcosmic's header bar carries on Linux. While the sidebar is closed it
    /// starts with the toggle, just where the sidebar had it.
    pub(super) fn macos_toolbar(&self) -> Element<'_, Message> {
        let sidebar_open = self.nav_model_impl().is_some() && self.core.nav_bar_active();
        let mut row = if sidebar_open {
            widget::row::with_capacity(4)
        } else {
            self.leading_controls()
        };
        row = row.push(horizontal_space());
        for item in self.header_end_impl() {
            row = row.push(item);
        }
        self.title_bar_strip(row)
    }
}
