// SPDX-License-Identifier: GPL-3.0-only
use clap::ArgAction;
use clap::builder::PossibleValuesParser;
use clap::{Command, arg, command};
use std::sync::LazyLock;
use super_stt_shared::models::registry;

/// Set of valid built-in model names for `--model`. Custom models can also be
/// passed by name from a configured `custom_models_dir`.
pub static MODEL_NAMES: LazyLock<Vec<&'static str>> =
    LazyLock::new(|| registry::ALL.iter().map(|d| d.name.as_ref()).collect());

// Build variant (e.g., "cuda13-cudnn-sm89", "cuda12-sm75", "cpu")
pub const BUILD_VARIANT: &str = env!("BUILD_VARIANT");

// Version string including build variant
pub static VERSION_STRING: LazyLock<String> =
    LazyLock::new(|| format!("{} ({})", env!("CARGO_PKG_VERSION"), BUILD_VARIANT));

#[must_use]
pub fn build() -> Command {
    command!()
    .version(VERSION_STRING.as_str())
    .about("🎙️ Super STT Daemon - Advanced Speech-to-text for Linux")
    .long_about(
        "A high-performance speech-to-text daemon that loads a STT model once and keeps it in memory, serving transcription requests over the HTTP protocol at $XDG_RUNTIME_DIR/stt/super-stt-http.sock. Use `super-stt-cli` (or the `stt` wrapper) to drive recordings."
    )
    .subcommand_required(false)
    .arg_required_else_help(false)
    .arg(
        arg!(-m --model <model> "Override the saved preferred model for this session")
        .required(false)
        .action(ArgAction::Set)
        .value_parser(PossibleValuesParser::new(MODEL_NAMES.iter().copied()))
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
