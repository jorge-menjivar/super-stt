// SPDX-License-Identifier: GPL-3.0-only
//! Super Voice's own post-install steps, for its keyboard shortcuts. They
//! run after super-engine-installer's shared steps, as [`after_install`], and
//! are best-effort like them: a failure is logged, and the install still
//! succeeds.
//!
//! The installer adds no shortcut. The daemon does, at `/v1/shortcuts`, from
//! the app's Shortcuts page, so the installer says where that page is. It
//! only keeps the COSMIC shortcuts already installed working.

use std::path::Path;

use super_engine_installer::{AfterInstall, Components};
use super_voice_shared::cosmic_shortcuts;

/// One shortcut step, in the order [`plan`] returns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Point the installed COSMIC shortcuts at this install's `voice`.
    MigrateShortcuts,
    /// Say that shortcuts are added from the app's Shortcuts page.
    PointToShortcutsPage,
}

/// Which shortcut steps to run: none unless the daemon, whose CLI the
/// shortcuts run, was installed. The installed shortcuts are migrated on a
/// COSMIC desktop, and the Shortcuts page is pointed to only when someone is
/// at the terminal to read it, on Linux, the one place the page is.
#[must_use]
pub fn plan(components: Components, cosmic_available: bool, interactive: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    if !components.daemon {
        return steps;
    }
    if cosmic_available {
        steps.push(Step::MigrateShortcuts);
    }
    if interactive && cfg!(target_os = "linux") {
        steps.push(Step::PointToShortcutsPage);
    }
    steps
}

/// The installer's [`Installer::after_install`](super_engine_installer::Installer::after_install).
pub fn after_install(done: &AfterInstall<'_>) {
    for step in plan(done.components, done.cosmic_available, done.interactive) {
        match step {
            Step::MigrateShortcuts => migrate_shortcut(done.prefix),
            Step::PointToShortcutsPage => eprintln!("\x1b[0;32m[INFO]\x1b[0m {SHORTCUTS_PAGE}"),
        }
    }
}

/// Where to add a keyboard shortcut, as the installer says it.
const SHORTCUTS_PAGE: &str =
    "Add keyboard shortcuts from the Shortcuts page in the Super Voice app.";

/// Point every installed shortcut at this install's `<prefix>/bin/voice`,
/// keeping its arguments. Returns `None` when there is nothing to change.
///
/// 1. One that runs Super STT's `stt`, from wherever its wrapper was
///    installed, `~/.local/bin` or the prefix. `stt` is gone, so a shortcut
///    left on it would do nothing. `record` is `voice record` too, `stt stop`
///    is `voice record stop`, and its "Super STT" description is renamed.
/// 2. One that runs `voice` from another place, such as an earlier install's
///    prefix. A bare `voice`, found on `PATH`, is left as it is.
#[must_use]
pub fn migrate_shortcut_content(content: &str, prefix: &str) -> Option<String> {
    const SPAWN: &str = "Spawn(\"";
    let voice = format!("{prefix}/bin/voice");
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    let mut changed = false;
    while let Some(at) = rest.find(SPAWN) {
        let start = at + SPAWN.len();
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest.find('"').unwrap_or(rest.len());
        let command = &rest[..end];
        let program_end = command.find(' ').unwrap_or(command.len());
        let program = &command[..program_end];
        if program == "stt" || program.ends_with("/stt") {
            let args = &command[program_end..];
            let args = args
                .strip_prefix(" stop")
                .map_or_else(|| args.to_string(), |tail| format!(" record stop{tail}"));
            out.push_str(&voice);
            out.push_str(&args);
            changed = true;
        } else if program.ends_with("/voice") && program != voice {
            out.push_str(&voice);
            out.push_str(&command[program_end..]);
            changed = true;
        } else {
            out.push_str(command);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    changed.then(|| {
        out.replace(
            "description: Some(\"Super STT\")",
            "description: Some(\"Super Voice\")",
        )
    })
}

/// Point the shortcuts in the COSMIC custom-shortcuts file that run `stt`,
/// or `voice` from another place, at this install. See
/// [`migrate_shortcut_content`].
fn migrate_shortcut(prefix: &Path) {
    let Some(shortcuts_file) = cosmic_shortcuts::file() else {
        return;
    };
    if let Ok(content) = std::fs::read_to_string(&shortcuts_file)
        && let Some(migrated) = migrate_shortcut_content(&content, &prefix.to_string_lossy())
        && let Err(e) = std::fs::write(&shortcuts_file, migrated)
    {
        log::warn!("failed to migrate COSMIC shortcut: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Super STT's binding moves to `voice`, from either place its wrapper
    /// was installed, with its arguments kept.
    #[test]
    fn migrate_moves_an_stt_shortcut_to_voice() {
        for old in [
            "/home/u/.local/bin/stt record --write",
            "/usr/local/bin/stt record --write",
        ] {
            let c =
                format!(r#"{{ (key: "space", description: Some("Super STT")): Spawn("{old}"), }}"#);
            let out = migrate_shortcut_content(&c, "/usr/local").unwrap();
            assert!(
                out.contains(r#"Spawn("/usr/local/bin/voice record --write")"#),
                "{out}"
            );
            assert!(out.contains(r#"description: Some("Super Voice")"#), "{out}");
            assert!(!out.contains("/stt "), "{out}");
        }
    }

    /// `stt stop` was speech to text's; it is `voice record stop` now.
    #[test]
    fn migrate_moves_stt_stop_under_record() {
        let c = r#"{ (key: "s"): Spawn("/usr/local/bin/stt stop"), }"#;
        let out = migrate_shortcut_content(c, "/usr/local").unwrap();
        assert!(
            out.contains(r#"Spawn("/usr/local/bin/voice record stop")"#),
            "{out}"
        );
    }

    /// Nothing runs `stt`: nothing to migrate, and other shortcuts are left
    /// alone, including one whose program merely ends in `stt`-like text.
    #[test]
    fn migrate_leaves_other_shortcuts_alone() {
        assert!(migrate_shortcut_content("{}", "/usr/local").is_none());
        let c = r#"{ (key: "t"): Spawn("/usr/bin/gstt record"), (key: "u"): Spawn("term"), }"#;
        assert!(migrate_shortcut_content(c, "/usr/local").is_none());
    }

    /// A shortcut on another install's `voice` moves to this one, arguments
    /// and all. One on this install's, or on a bare `voice`, is left alone.
    #[test]
    fn migrate_moves_voice_from_another_place() {
        let c = r#"{ (key: "space", description: Some("Super Voice")): Spawn("/home/u/.local/bin/voice record --write --wait"), }"#;
        let out = migrate_shortcut_content(c, "/usr/local").unwrap();
        assert!(
            out.contains(r#"Spawn("/usr/local/bin/voice record --write --wait")"#),
            "{out}"
        );
        for kept in [
            r#"{ (key: "space"): Spawn("/usr/local/bin/voice record --write"), }"#,
            r#"{ (key: "space"): Spawn("voice record --write"), }"#,
            r#"{ (key: "v"): Spawn("/usr/bin/nvoice --x"), }"#,
        ] {
            assert!(
                migrate_shortcut_content(kept, "/usr/local").is_none(),
                "{kept}"
            );
        }
    }

    fn daemon_only() -> Components {
        Components {
            daemon: true,
            app: false,
            applet: false,
        }
    }

    #[test]
    fn plan_shortcut_steps_require_the_daemon() {
        let daemon = daemon_only();
        let linux = cfg!(target_os = "linux");

        // No cosmic-panel on PATH: nothing to migrate, and the page is still
        // pointed to on Linux when someone is there to read it.
        let steps = plan(daemon, false, true);
        assert!(!steps.contains(&Step::MigrateShortcuts));
        assert_eq!(steps.contains(&Step::PointToShortcutsPage), linux);

        // Cosmic available, non-interactive (an update from the app): migrate
        // only, with nobody to read about the page.
        assert_eq!(plan(daemon, true, false), [Step::MigrateShortcuts]);

        // Cosmic available AND interactive: migrate first, then the page.
        let steps = plan(daemon, true, true);
        if linux {
            assert_eq!(steps, [Step::MigrateShortcuts, Step::PointToShortcutsPage]);
        } else {
            assert_eq!(steps, [Step::MigrateShortcuts]);
        }

        // App-only (no daemon component at all): no shortcut steps even with
        // cosmic available and an interactive session.
        let app_only = Components {
            daemon: false,
            app: true,
            applet: false,
        };
        assert!(plan(app_only, true, true).is_empty());
    }
}
