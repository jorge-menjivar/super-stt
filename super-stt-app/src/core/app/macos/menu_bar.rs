// SPDX-License-Identifier: GPL-3.0-only
//! The menu bar at the top of the screen.
//!
//! A COSMIC app keeps its menus in its header bar; a Mac app keeps them in the
//! menu bar. So on macOS the View menu moves there, beside an application
//! menu with the usual Hide and Quit, in place of the bare menu winit installs.

use std::cell::OnceCell;

use cosmic::iced::futures::channel::mpsc;
use cosmic::iced::futures::{SinkExt, Stream, StreamExt};
use cosmic::widget::menu::action::MenuAction as _;
use muda::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::state::MenuAction;
use crate::ui::messages::Message;

/// Menu ID of View → About.
const ABOUT: &str = "about";

thread_local! {
    /// The installed menu bar. `AppKit` owns the menus; this keeps muda's side
    /// of them alive for as long as they are on screen.
    static MENU_BAR: OnceCell<Menu> = const { OnceCell::new() };
}

/// Install the app's menu bar, once.
///
/// Must run on the main thread, and after launch: winit installs its default
/// menu bar as the app finishes launching, replacing any set before then.
pub(crate) fn install() {
    MENU_BAR.with(|installed| {
        if installed.get().is_some() {
            return;
        }
        match build() {
            Ok(menu) => {
                menu.init_for_nsapp();
                let _ = installed.set(menu);
            }
            Err(e) => log::warn!("macOS menu bar not installed: {e}"),
        }
    });
}

fn build() -> muda::Result<Menu> {
    // macOS titles the application menu with the app's name itself; the
    // title given here is not shown.
    let application = Submenu::with_items(
        "Super STT",
        true,
        &[
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(Some("Hide Super STT")),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(Some("Quit Super STT")),
        ],
    )?;
    let view = Submenu::with_items(
        "View",
        true,
        &[&MenuItem::with_id(ABOUT, "About", true, None)],
    )?;
    Menu::with_items(&[&application, &view])
}

/// The menu bar's clicks, as the messages the header's menu sends on Linux.
pub(crate) fn events() -> impl Stream<Item = Message> {
    cosmic::iced::stream::channel(4, async |mut output| {
        // muda reports clicks on a global, blocking channel; a thread relays
        // them into this stream.
        let (relay, mut clicks) = mpsc::unbounded::<MenuId>();
        let spawned = std::thread::Builder::new()
            .name("menu-bar".into())
            .spawn(move || {
                while let Ok(event) = MenuEvent::receiver().recv() {
                    if relay.unbounded_send(event.id).is_err() {
                        break;
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("macOS menu bar will not respond: {e}");
            return;
        }
        while let Some(id) = clicks.next().await {
            if let Some(action) = action(&id) {
                let _ = output.send(action.message()).await;
            }
        }
    })
}

fn action(id: &MenuId) -> Option<MenuAction> {
    (id == ABOUT).then_some(MenuAction::About)
}
