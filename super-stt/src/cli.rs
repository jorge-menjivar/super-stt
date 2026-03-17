// SPDX-License-Identifier: GPL-3.0-only
use std::path::PathBuf;

use clap::ValueHint;
use clap::{ArgAction, Command, arg, command, value_parser};
use std::sync::LazyLock;
use super_stt_shared::stt_model::STTModel;

// Use LazyLock to avoid leaking string in an uncontrolled way
pub static DEFAULT_SOCKET_PATH: LazyLock<PathBuf> =
    LazyLock::new(super_stt_shared::validation::get_secure_socket_path);
pub static DEFAULT_SOCKET_PATH_STR: LazyLock<&'static str> = LazyLock::new(|| {
    Box::leak(
        DEFAULT_SOCKET_PATH
            .to_str()
            .unwrap()
            .to_string()
            .into_boxed_str(),
    )
});
pub static DEFAULT_TIMEOUT: LazyLock<u64> = LazyLock::new(|| 10);
pub static DEFAULT_TIMEOUT_STR: LazyLock<&'static str> =
    LazyLock::new(|| Box::leak(DEFAULT_TIMEOUT.to_string().into_boxed_str()));
pub static DEFAULT_MODEL: LazyLock<STTModel> = LazyLock::new(|| STTModel::WhisperTiny);
pub static DEFAULT_MODEL_STR: LazyLock<&'static str> =
    LazyLock::new(|| Box::leak(DEFAULT_MODEL.to_string().into_boxed_str()));

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
        "A high-performance speech-to-text daemon that loads a STT model once and keeps it in memory, serving transcription requests via Unix domain socket."
    )
    .subcommand_required(false)
    .arg_required_else_help(false)
    .subcommand(
        Command::new("record")
            .about("🎤 Record audio and transcribe (manual trigger)")
            .long_about("Start recording from microphone, automatically detect speech and silence, then transcribe the audio.")
            .arg(
                arg!(-w --write "Type the transcription directly into the active window")
                .action(ArgAction::SetTrue)
            )
            .arg(
                arg!(--"stop-mode" <mode> "Recording stop mode: silence, silence-and-manual, manual")
                    .value_parser(["silence", "silence-and-manual", "manual"])
                    .required(false)
            )
            .arg(
                arg!(-s --socket <socket> "The daemon socket path")
                .default_value(*DEFAULT_SOCKET_PATH_STR)
                .value_parser(value_parser!(PathBuf))
                .value_hint(ValueHint::AnyPath)
            )
    )
    .subcommand(
        Command::new("ping")
            .about("🏓 Check if daemon is running")
            .long_about("Test connectivity to the daemon service.")
    )
    .subcommand(
        Command::new("status")
            .about("📊 Get daemon status")
            .long_about("Get detailed status information from the daemon including model and device information.")
    )
    .arg(
        arg!(-m --model <model> "The model to use for transcription")
        .default_value(*DEFAULT_MODEL_STR)
        .required(false)
        .action(ArgAction::Set)
        .value_parser(value_parser!(STTModel))
    )
    .arg(
        arg!(-s --socket <socket> "The socket to connect to")
        .default_value(*DEFAULT_SOCKET_PATH_STR)
        .required(false)
        .value_parser(value_parser!(PathBuf))
        .value_hint(ValueHint::AnyPath)
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
        arg!(--"udp-port" <port> "UDP port for audio streaming")
        .default_value("8765")
        .value_parser(value_parser!(u16))
    )
    .arg(
        arg!(--"audio-theme" <theme> "Audio feedback theme")
        .default_value("classic")
        .help("Choose audio feedback style: classic, gentle, minimal, scifi, musical, nature, retro, silent")
        .value_parser(["classic", "gentle", "minimal", "scifi", "musical", "nature", "retro", "silent"])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_stop_mode_flag_parses() {
        let matches = build()
            .try_get_matches_from(["super-stt", "record", "--stop-mode", "manual"])
            .expect("command should parse");

        let record_matches = matches
            .subcommand_matches("record")
            .expect("record subcommand should be present");

        assert_eq!(
            record_matches.get_one::<String>("stop-mode").map(String::as_str),
            Some("manual")
        );
    }

    #[test]
    fn record_without_stop_mode_has_none() {
        let matches = build()
            .try_get_matches_from(["super-stt", "record"])
            .expect("command should parse");

        let record_matches = matches
            .subcommand_matches("record")
            .expect("record subcommand should be present");

        assert!(record_matches.get_one::<String>("stop-mode").is_none());
    }
}
