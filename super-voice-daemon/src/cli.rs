// SPDX-License-Identifier: GPL-3.0-only
use clap::ArgAction;
use clap::{Command, arg, command};

#[must_use]
pub fn build() -> Command {
    command!()
    .about("Super Voice daemon: speech to text and text to speech")
    .long_about(
        "The Super Voice daemon: loads speech-to-text and text-to-speech models once and keeps them in memory, serving requests over the HTTP protocol at $XDG_RUNTIME_DIR/voice/super-voice-http.sock. Use the `voice` command to record and speak."
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
