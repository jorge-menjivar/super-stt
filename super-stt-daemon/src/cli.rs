// SPDX-License-Identifier: GPL-3.0-only
use clap::ArgAction;
use clap::{Command, arg, command};

#[must_use]
pub fn build() -> Command {
    command!()
    .about("🎙️ Super STT Daemon - Advanced Speech-to-text for Linux")
    .long_about(
        "A high-performance speech-to-text daemon that loads a STT model once and keeps it in memory, serving transcription requests over the HTTP protocol at $XDG_RUNTIME_DIR/stt/super-stt-http.sock. Use `super-stt-cli` (or the `stt` wrapper) to drive recordings."
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
