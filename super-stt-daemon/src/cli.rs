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
        // Model names are served by installed backends and discovered at
        // runtime, so any string is accepted here and validated by the daemon.
        arg!(-m --model <model> "Override the saved preferred model for this session")
        .required(false)
        .action(ArgAction::Set)
    )
    .arg(
        arg!(--device <device> "Device to use for model execution")
        .default_value("cuda")
        .help("Choose device: cuda (GPU if available, fallback to CPU) or cpu (force CPU only)")
        .value_parser(["cuda", "cpu"])
    )
    .arg(
        arg!(-v --verbose ... "Enable verbose logging")
        .default_value("false")
        .action(ArgAction::SetTrue)
    )
    .arg(
        arg!(--"audio-theme" <theme> "Audio feedback theme")
        .default_value("classic")
        .help("Choose audio feedback style: classic, gentle, minimal, scifi, musical, nature, retro, silent")
        .value_parser(["classic", "gentle", "minimal", "scifi", "musical", "nature", "retro", "silent"])
    )
}
