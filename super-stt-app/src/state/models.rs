// SPDX-License-Identifier: GPL-3.0-only

//! Data models and types for the Super STT application.

// Re-export AudioTheme from shared crate
pub use super_stt_shared::models::theme::AudioTheme;

/// Daemon connection status
#[derive(Debug, Clone, Default, PartialEq)]
pub enum DaemonStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
    /// User denied the settings-scope consent prompt (either just now
    /// or via the daemon's sticky deny cache). All settings-scope
    /// operations will fail until the user explicitly retries
    /// authorization — usually after `systemctl --user restart
    /// super-stt`. The auto-retry loop is suppressed in this state to
    /// avoid spamming the daemon's deny cache.
    Blocked(String),
}

/// Recording status
#[derive(Debug, Clone, Default, PartialEq)]
pub enum RecordingStatus {
    #[default]
    Idle,
    Recording,
}

/// The page to display in the application
#[derive(Debug, Clone)]
pub enum Page {
    Connection,
    Customization,
    Recording,
    InputSimulation,
    /// The active transcription backend: its model picker, load/unload, and a
    /// side sheet for switching which installed backend is active.
    Models,
    /// Manage installed backends and browse installable ones (the old Models
    /// page's Installed / Browse tabs). No activation here — that lives on the
    /// Models page.
    Library,
}

/// The context page to display in the context drawer
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ContextPage {
    #[default]
    About,
    /// Right-side sheet for adding a backend from a Git repo URL or a local
    /// directory. Scoped to the Models page — it closes on navigation away or
    /// daemon disconnect (see `AppModel::context_drawer`).
    AddBackend,
    /// Right-side sheet for editing the active/selected backend's secrets and
    /// options. Reuses the drawer instead of a full-page takeover so the
    /// backend list stays visible behind it. The backend is identified by
    /// `AppModel::configure_backend`; also Models-scoped.
    ConfigureBackend,
    /// Right-side search sheet for picking a transcription language. Scope
    /// (global vs per-model) is carried by `AppModel::language_picker_target`.
    LanguagePicker,
    /// Right-side sheet for choosing which installed backend to activate (the
    /// Models page's "Load a backend" / "Switch backend" flow). Scoped to the
    /// Models page; picking a backend activates it and closes the sheet.
    LoadBackend,
}

/// Which tab of the Models page is active.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub enum ModelsTab {
    /// Backends already installed and discovered by the daemon.
    #[default]
    Installed,
    /// Official backends the user can install.
    Download,
}

/// Menu actions
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuAction {
    About,
}
