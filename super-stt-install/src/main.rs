// SPDX-License-Identifier: GPL-3.0-only
//! `super-stt-install`: installs and updates Super STT. The installer is
//! `super_engine_installer`'s; what is here is what makes it Super STT's,
//! the `Super+Space` shortcut among it.

mod shortcut;

use super_engine_installer::Installer;
use super_stt_registry_types::product::{SUPER_STT, Stt};

static INSTALLER: Installer = Installer {
    product: &SUPER_STT,
    user_agent: Stt::USER_AGENT,
    wrapper_usage: "Used by keyboard shortcuts (e.g. Super+Space → \"stt record --write\").",
    after_install: Some(shortcut::after_install),
};

fn main() -> std::process::ExitCode {
    super_engine_installer::main(&INSTALLER)
}
