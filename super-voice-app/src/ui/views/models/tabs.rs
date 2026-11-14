// SPDX-License-Identifier: GPL-3.0-only
use cosmic::Element;

use crate::core::app::AppModel;
use crate::ui::messages::{Message, ModelsPageMessage};
use crate::ui::views::tabs::tab_row;

/// Tab switcher for the Library page (Installed / Browse), drawn by
/// [`tab_row`].
///
/// Still driven by `app.models_page.models_tabs`, so clicking a tab emits the same
/// [`ModelsPageMessage::ModelsTabActivated`] the segmented-button model expects.
pub(super) fn models_tab_switcher(app: &AppModel) -> Element<'_, Message> {
    let tabs = &app.models_page.models_tabs;
    let active = tabs.active();
    tab_row(tabs.iter().collect::<Vec<_>>().into_iter().map(|entity| {
        (
            tabs.text(entity).map_or_default(ToOwned::to_owned),
            entity == active,
            Message::ModelsPage(ModelsPageMessage::ModelsTabActivated(entity)),
        )
    }))
}
