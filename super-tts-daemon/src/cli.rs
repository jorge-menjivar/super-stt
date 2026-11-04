// SPDX-License-Identifier: GPL-3.0-only
use clap::ArgAction;
use clap::{Command, arg, command};

#[must_use]
pub fn build() -> Command {
    command!()
    .about("🔊 Super TTS Daemon - Advanced text-to-speech for Linux")
    .long_about(
        "A high-performance text-to-speech daemon that loads a voice model once and keeps it in memory, serving synthesis requests over the HTTP protocol at $XDG_RUNTIME_DIR/tts/super-tts-http.sock. Use `super-tts-cli` (or the `tts` wrapper) to speak text."
    )
    .subcommand_required(false)
    .arg_required_else_help(false)
    .arg(
        arg!(-v --verbose ... "Enable verbose logging")
        .default_value("false")
        .action(ArgAction::SetTrue)
    )
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
