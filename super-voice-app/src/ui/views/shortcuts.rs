// SPDX-License-Identifier: GPL-3.0-only
//! The Shortcuts page: each of Super Voice's keyboard shortcuts, where it
//! stands on this desktop, and a button to add or remove it. The daemon owns
//! the shortcuts; the page shows what it answers. The installer adds none: it
//! says to come here.

use cosmic::Element;
use cosmic::iced::Alignment;
use cosmic::iced::widget::{Space, row};
use cosmic::widget::{button, settings, text};
use super_voice_shared::models::shortcuts::{Shortcut, ShortcutId, ShortcutMethod, ShortcutState};

use super::common::{error_banner, page_layout};
use crate::state::shortcuts::ShortcutsState;
use crate::ui::messages::{Message, ShortcutsMessage};

/// The shortcut's name on the page, and what it does.
fn name(id: ShortcutId) -> (String, String) {
    match id {
        ShortcutId::Dictate => (
            crate::fl!("shortcut-dictate"),
            crate::fl!("shortcut-dictate-description"),
        ),
        ShortcutId::ReadAloud => (
            crate::fl!("shortcut-read-aloud"),
            crate::fl!("shortcut-read-aloud-description"),
        ),
    }
}

/// One shortcut's row: what it does and where it stands, with its keys and a
/// Remove button once it is set up, or the button that adds it.
fn row_for(shortcut: &Shortcut, method: ShortcutMethod) -> Element<'static, Message> {
    let (title, does) = name(shortcut.id);
    let item = settings::item::builder(title);
    let keys = shortcut.keys.clone();
    let command = shortcut.command.clone();
    match shortcut.state {
        ShortcutState::Installed => {
            let where_ = match method {
                ShortcutMethod::Cosmic => crate::fl!("shortcut-set-up"),
                ShortcutMethod::Portal | ShortcutMethod::Manual => {
                    crate::fl!("shortcut-set-up-desktop")
                }
            };
            item.description(format!("{does} {where_}"))
                .control(
                    row![
                        text::body(keys),
                        button::standard(crate::fl!("remove"))
                            .on_press(Message::Shortcuts(ShortcutsMessage::Remove(shortcut.id))),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .into()
        }
        ShortcutState::Missing => {
            let state = crate::fl!("shortcut-missing", keys = keys);
            item.description(format!("{does} {state}"))
                .control(
                    button::standard(crate::fl!("add-shortcut"))
                        .on_press(Message::Shortcuts(ShortcutsMessage::Add(shortcut.id))),
                )
                .into()
        }
        ShortcutState::Taken => {
            let state = crate::fl!("shortcut-taken", keys = keys, command = command);
            item.description(format!("{does} {state}"))
                .control(Space::new())
                .into()
        }
        ShortcutState::Manual => {
            let state = crate::fl!("shortcut-elsewhere", command = command);
            item.description(format!("{does} {state}"))
                .control(Space::new())
                .into()
        }
    }
}

pub fn page<'a>(state: &ShortcutsState, action_error: Option<&'a str>) -> Element<'a, Message> {
    let mut blocks = Vec::new();
    if let Some(message) = action_error {
        blocks.push(error_banner(message));
    }
    if let Some(list) = &state.list {
        let mut section = settings::section().title(crate::fl!("keyboard-shortcuts"));
        for shortcut in &list.shortcuts {
            section = section.add(row_for(shortcut, list.method));
        }
        blocks.push(section.into());
    }
    page_layout(crate::fl!("shortcuts"), settings::view_column(blocks))
}
