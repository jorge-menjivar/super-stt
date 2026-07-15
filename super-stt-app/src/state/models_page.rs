// SPDX-License-Identifier: GPL-3.0-only

//! Models-page UI selection/menu state, extracted from `AppModel` following the
//! `RegistryState` template (App Tier 3 #15).

use cosmic::widget::segmented_button;

use crate::state::ModelsTab;

/// Ephemeral Models-page UI state: the Installed/Browse tab bar plus the
/// active-backend card's selection, staging, and menu flags.
pub struct ModelsPageState {
    /// Installed / Download tab bar (the active tab carries a [`ModelsTab`]).
    pub models_tabs: segmented_button::SingleSelectModel,
    /// Source of the currently-selected (active) backend, shown in the card
    /// above the tabs. `None` when the daemon is idle.
    pub active_backend: Option<String>,
    /// Model the user has picked in the active-backend card's dropdown but
    /// hasn't yet committed via the Load button.
    pub staged_model: Option<String>,
    /// Device the user has picked for the staged model (`"none"` for an online
    /// model that needs no device choice).
    pub staged_device: Option<String>,
    /// The backend whose configuration sub-view is open, if any (`source`).
    pub configure_backend: Option<String>,
    /// `source` of the installed-backend card whose overflow ("⋯") menu is
    /// open, if any. Only one is open at a time.
    pub installed_menu_open: Option<String>,
}

impl Default for ModelsPageState {
    fn default() -> Self {
        let mut models_tabs = segmented_button::SingleSelectModel::default();
        models_tabs
            .insert()
            .text("Installed")
            .data(ModelsTab::Installed)
            .activate();
        models_tabs
            .insert()
            .text("Browse")
            .data(ModelsTab::Download);
        Self {
            models_tabs,
            active_backend: None,
            staged_model: None,
            staged_device: None,
            configure_backend: None,
            installed_menu_open: None,
        }
    }
}
