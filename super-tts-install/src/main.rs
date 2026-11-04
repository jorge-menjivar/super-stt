// SPDX-License-Identifier: GPL-3.0-only
//! `super-tts-install`: installs and updates Super TTS. The installer is
//! `super_engine_installer`'s; what is here is what makes it Super TTS's.

use super_engine_installer::Installer;
use super_tts_registry_types::product::{SUPER_TTS, Tts};

static INSTALLER: Installer = Installer {
    product: &SUPER_TTS,
    user_agent: Tts::USER_AGENT,
    wrapper_usage: "e.g. \"tts speak 'hello'\", or bind it to a keyboard shortcut.",
    // No shortcut of its own: this daemon's gesture is "speak what I have
    // selected", and until text acquisition exists there is no command worth
    // binding. Installing a broken shortcut is worse than installing none.
    after_install: None,
};

fn main() -> std::process::ExitCode {
    super_engine_installer::main(&INSTALLER)
}
