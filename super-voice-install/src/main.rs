// SPDX-License-Identifier: GPL-3.0-only
//! `super-voice-install`: installs and updates Super Voice. The installer is
//! `super_engine_installer`'s; what is here is what makes it Super Voice's:
//! its two apps, what Super STT and Super TTS installs left to retire, and
//! the `Super+Space` shortcut.

mod shortcut;

use super_engine_installer::{Installer, Retired};
use super_voice_registry_types::product::{SUPER_VOICE, Voice};

static INSTALLER: Installer = Installer {
    product: &SUPER_VOICE,
    user_agent: Voice::USER_AGENT,
    wrapper_usage: "Used by keyboard shortcuts (e.g. Super+Space → \"voice record --write\").",
    after_install: Some(shortcut::after_install),
    apps: &["super-voice-app"],
    retired: RETIRED,
};

/// What Super STT and Super TTS installed that Super Voice replaces: their
/// daemons, CLIs, consent helpers, wrappers, installers and units, Super
/// STT's oldest layout (one `super-stt` binary), their applets, and the two
/// settings apps, which Super Voice's replaced.
const RETIRED: Retired = Retired {
    binaries: &[
        "super-stt",
        "super-stt-daemon",
        "super-stt-cli",
        "super-stt-consent",
        "super-stt-install",
        "super-stt-app",
        "stt",
        "super-tts-app",
        "super-tts-daemon",
        "super-tts-cli",
        "super-tts-consent",
        "super-tts-install",
        "tts",
    ],
    units: &["super-stt.service", "super-tts.service"],
    apps: &["super-stt-app", "super-tts-app"],
    applets: &["super-stt-cosmic-applet", "super-tts-cosmic-applet"],
};

fn main() -> std::process::ExitCode {
    super_engine_installer::main(&INSTALLER)
}

#[cfg(test)]
mod tests {
    use super::{INSTALLER, RETIRED};

    /// The apps it installs are the workspace's apps, every one of them.
    #[test]
    fn installs_the_workspaces_apps() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut ours: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().join("Cargo.toml").is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with("-app"))
            .collect();
        ours.sort();
        let mut apps = INSTALLER.apps.to_vec();
        apps.sort_unstable();
        assert_eq!(apps, ours);
    }

    /// Nothing it installs is on the retired list: an install would remove
    /// its own files right after placing them.
    #[test]
    fn retires_nothing_it_installs() {
        let product = INSTALLER.product;
        let mut installs = vec![
            product.daemon_binary(),
            format!("{}-cli", product.slug),
            product.consent_helper(),
            product.installer_binary(),
            product.short_name.to_string(),
            // The shared applet: super-engine's SHARED_APPLET.
            "super-cosmic-applet".to_string(),
        ];
        installs.extend(INSTALLER.apps.iter().map(ToString::to_string));
        for retired in RETIRED
            .binaries
            .iter()
            .chain(RETIRED.apps)
            .chain(RETIRED.applets)
        {
            assert!(
                !installs.iter().any(|name| name == retired),
                "{retired} is installed and retired at once"
            );
        }
        assert!(
            !RETIRED
                .units
                .contains(&format!("{}.service", product.slug).as_str())
        );
    }
}
