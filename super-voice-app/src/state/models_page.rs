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
    /// The backend whose configuration sub-view is open, if any (`source`).
    pub configure_backend: Option<String>,
    /// `source` of the installed-backend card whose overflow ("⋯") menu is
    /// open, if any. Only one is open at a time.
    pub installed_menu_open: Option<String>,
    /// Whether the Browse toolbar's overflow ("⋯") menu is open. It holds
    /// Refresh and the manual-install drawer, which used to be two buttons
    /// competing with the search field.
    pub browse_menu_open: bool,
    /// The Installed tab's "Runs on" / kind filters.
    pub installed_filters: crate::state::registry::InstalledFilters,
    /// The pipeline step whose card the Models page shows under the rows.
    pub pipeline_step: crate::state::Stage,
    /// A framework picked whose build is not installed: asked about in a
    /// dialog, then downloaded, then switched to.
    pub pending_build: Option<PendingBuild>,
}

/// A framework picked for a stage's model whose backend does not have that
/// build installed.
///
/// Picking one asks first, every time, because it is a download. Once the
/// user agrees the build is installed in place of the one there, keeping the
/// downloaded models, and the model is moved to the framework when the
/// install completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingBuild {
    pub stage: crate::state::Stage,
    /// The backend whose build this is.
    pub source: String,
    /// The framework picked, as the daemon names it (`cuda`).
    pub framework: String,
    /// The user agreed and the build is downloading. Until then the dialog
    /// is up.
    pub downloading: bool,
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
            configure_backend: None,
            installed_menu_open: None,
            browse_menu_open: false,
            installed_filters: crate::state::registry::InstalledFilters::default(),
            pipeline_step: crate::state::Stage::Transcription,
            pending_build: None,
        }
    }
}
