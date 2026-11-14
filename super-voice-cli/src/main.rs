// SPDX-License-Identifier: GPL-3.0-only
//! `voice`: Super Voice's command-line client. Talks to the daemon over the
//! HTTP protocol on its Unix socket.
//!
//! Each feature's commands are under its own subcommand — `voice record` for
//! speech to text, `voice speak` for text to speech — and what the daemon has
//! once is at the top: `ping`, `status`, `logout`.
//!
//! Auth flow is delegated to `super_voice_shared::daemon::session`, the
//! shared client-side session helper. Each client (CLI, settings app,
//! applet) keeps its token in a file of its own, named by its `AppId`; the
//! CLI's is `$XDG_STATE_HOME/super-voice/clients/super-voice-cli.json`. One
//! token serves both features.
//!
//! `session::with_token` handles cache-hit, consent-popup-on-miss, and
//! `invalid_session` retry transparently — handlers below just write
//! the network call and propagate the daemon's error string.
//!
//! For tests / CI: set `SUPER_VOICE_AUTO_APPROVE=1` in the daemon's
//! environment so the consent popup is bypassed and `/auth/request`
//! auto-approves.

use anyhow::{Context, Result, anyhow};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use std::io::Read;
use std::path::PathBuf;
use super_voice_shared::daemon::http_client::{self, TranscribeOptions};
use super_voice_shared::daemon::session::{self, AppId};
use super_voice_shared::validation::get_http_socket_path;

#[cfg(target_os = "macos")]
mod hotkey;
#[cfg(target_os = "macos")]
mod service;

const APP_ID: AppId = super_voice_shared::daemon::session::app_id("super-voice-cli");
const APP_NAME: &str = "Super Voice CLI";
/// Both features' commands run on one token, so it asks for both.
const SCOPES: &[&str] = &["transcribe", "speak", "status"];

fn main() -> Result<()> {
    // The CLI previously had no logging at all; initialize it like the other
    // binaries (RUST_LOG wins, else Info) (Tier 2 #6).
    super_voice_shared::logging::init();

    let matches = build_cli().get_matches();

    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .cloned()
        .unwrap_or_else(get_http_socket_path);

    // `hotkey` runs AppKit's event loop, which has to own the main thread, so
    // it is dispatched before any runtime exists and builds its own on worker
    // threads. Every other subcommand is one request and out.
    #[cfg(target_os = "macos")]
    if let Some(("hotkey", sub)) = matches.subcommand() {
        let binding = sub
            .get_one::<String>("key")
            .map_or(hotkey::DEFAULT_BINDING, String::as_str);
        if sub.get_flag("check") {
            return hotkey::check(binding);
        }
        return hotkey::run(socket_path, binding);
    }
    // Talks to launchd rather than the daemon, and needs no runtime.
    #[cfg(target_os = "macos")]
    if let Some(("service", sub)) = matches.subcommand() {
        return service::run(sub);
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?
        .block_on(dispatch(&matches, socket_path))
}

async fn dispatch(matches: &ArgMatches, socket_path: PathBuf) -> Result<()> {
    match matches.subcommand() {
        Some(("ping", _)) => {
            run_with_token(socket_path.clone(), |t| cmd_ping(socket_path.clone(), t))
                .await
                .context("ping failed")
        }
        Some(("status", _)) => {
            run_with_token(socket_path.clone(), |t| cmd_status(socket_path.clone(), t))
                .await
                .context("status failed")
        }
        Some(("record", sub)) if matches!(sub.subcommand(), Some(("stop", _))) => {
            run_with_token(socket_path.clone(), |t| cmd_stop(socket_path.clone(), t))
                .await
                .context("stop failed")
        }
        Some(("record", sub)) => {
            let write = sub.get_flag("write");
            let wait = sub.get_flag("wait");
            let stop_mode = sub.get_one::<String>("stop-mode").cloned();
            run_with_token(socket_path.clone(), |t| {
                cmd_record(socket_path.clone(), t, write, wait, stop_mode.clone())
            })
            .await
            .context("record failed")
        }
        Some(("speak", sub)) if matches!(sub.subcommand(), Some(("stop", _))) => {
            run_with_token(socket_path.clone(), |t| {
                cmd_speak_stop(socket_path.clone(), t)
            })
            .await
            .context("stop failed")
        }
        #[cfg(target_os = "linux")]
        Some(("speak", sub)) if sub.get_flag("read") => run_with_token(socket_path.clone(), |t| {
            cmd_speak_read(socket_path.clone(), t)
        })
        .await
        .context("speak failed"),
        Some(("speak", sub)) => {
            let text = read_text(sub)?;
            run_with_token(socket_path.clone(), |t| {
                cmd_speak(socket_path.clone(), t, text.clone())
            })
            .await
            .context("speak failed")
        }
        Some(("logout", _)) => cmd_logout(),
        _ => {
            println!("Run with --help for usage.");
            Ok(())
        }
    }
}

/// The command-line interface. A function rather than inline in `main` so the
/// tests can parse the arguments the hotkey `LaunchAgent` is installed with.
fn build_cli() -> Command {
    platform_subcommands(
        Command::new("voice")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Super Voice: speech to text and text to speech")
        .arg(
            Arg::new("socket")
                .long("socket")
                .help("Path to the daemon HTTP Unix socket")
                .value_parser(value_parser!(PathBuf))
                .global(true),
        )
        .subcommand(Command::new("ping").about("Check that the daemon is reachable"))
        .subcommand(Command::new("status").about("Print each feature's model, device and state"))
        .subcommand(
            Command::new("record")
                .about("Start a recording, or stop the one in progress")
                .args_conflicts_with_subcommands(true)
                .subcommand(Command::new("stop").about("Stop the recording in progress"))
                .arg(
                    Arg::new("write")
                        .short('w')
                        .long("write")
                        .help("Type the transcription into the focused window when done")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("wait")
                        .long("wait")
                        .help("Hold the connection open and print the final transcription")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("stop-mode")
                        .long("stop-mode")
                        .help("Override the configured stop mode")
                        // Drive the accepted values from the shared wire-enum
                        // table so they can't drift from the daemon (Tier 2 #8).
                        .value_parser(clap::builder::PossibleValuesParser::new(
                            super_voice_shared::models::recording_stop_mode::RecordingStopMode::WIRE_VARIANTS
                                .iter()
                                .copied(),
                        )),
                ),
        )
        .subcommand(read_arg(
            Command::new("speak")
                .about("Speak text with the active model")
                .args_conflicts_with_subcommands(true)
                .subcommand(Command::new("stop").about("Stop the speech now playing"))
                .arg(
                    Arg::new("text")
                        .help("The text to speak; omit to read it from stdin. To speak the word \"stop\", put `--` before it")
                        .num_args(0..)
                        .trailing_var_arg(true),
                ), // No --voice/--language/--speed/--instructions: how the
                   // machine speaks is the user's configuration, not a flag on the
                   // thing that makes it talk. Set a voice with the settings app
                   // or `POST /tts/pipeline/{stage}/model/{model}/voice`.
        ))
        .subcommand(
            Command::new("logout")
                .about("Forget the cached session token (forces re-consent next call)"),
        ),
    )
}

#[cfg(target_os = "macos")]
fn platform_subcommands(cli: Command) -> Command {
    cli.subcommand(hotkey::command())
        .subcommand(service::command())
}

/// Nothing to add on Linux: the desktop environment binds the shortcuts there
/// and runs `voice record --write` and `voice speak --read` itself, and
/// systemd runs the daemon.
#[cfg(not(target_os = "macos"))]
fn platform_subcommands(cli: Command) -> Command {
    cli
}

/// `voice speak --read`, which the desktop environment's shortcut runs: the
/// text to speech counterpart of `voice record --write`. Linux only: the
/// daemon reads the selection and the clipboard from the Wayland session.
#[cfg(target_os = "linux")]
fn read_arg(speak: Command) -> Command {
    speak.arg(
        Arg::new("read")
            .long("read")
            .help("Speak the selected or copied text, as the read method in the app says. If speech is playing, stop it instead")
            .action(ArgAction::SetTrue)
            .conflicts_with("text"),
    )
}

#[cfg(not(target_os = "linux"))]
fn read_arg(speak: Command) -> Command {
    speak
}

/// Thin wrapper over `session::with_token` that adapts its
/// `Result<T, String>` return into `anyhow::Result<T>` and supplies
/// the CLI's app identity. All commands route through here.
async fn run_with_token<F, Fut, T>(socket_path: PathBuf, op: F) -> Result<T>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = super_voice_shared::daemon::http_client::HttpResult<T>>,
{
    run_with_token_as(APP_ID, APP_NAME, socket_path, op).await
}

/// [`run_with_token`] under another app identity. See `hotkey::APP_ID`.
async fn run_with_token_as<F, Fut, T>(
    app_id: AppId,
    app_name: &str,
    socket_path: PathBuf,
    op: F,
) -> Result<T>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = super_voice_shared::daemon::http_client::HttpResult<T>>,
{
    session::with_token(socket_path, app_id, app_name, SCOPES, op)
        .await
        .map_err(|e| anyhow!(e))
}

// -----------------------------------------------------------------------------
// Per-subcommand handlers
// -----------------------------------------------------------------------------
//
// Handlers return `Result<(), String>` to match the shape `session::with_token`
// expects. On `invalid_session` from the daemon the shared helper transparently
// re-runs consent and retries the op, so handlers don't need to classify
// auth-vs-non-auth errors themselves.

async fn cmd_ping(
    socket_path: PathBuf,
    token: String,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    let msg = http_client::ping(socket_path, &token).await?;
    println!("{msg}");
    Ok(())
}

async fn cmd_status(
    socket_path: PathBuf,
    token: String,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    let status = http_client::status(socket_path, &token).await?;
    for (feature, part, at_work) in [
        ("Speech to text", &status.stt, "recording"),
        ("Text to speech", &status.tts, "speaking"),
    ] {
        println!("{feature}");
        println!(
            "  Model:  {}",
            part.current_model.as_deref().unwrap_or("(none loaded)")
        );
        println!("  Device: {}", part.device);
        println!("  State:  {}", if part.busy { at_work } else { "idle" });
    }
    Ok(())
}

async fn cmd_record(
    socket_path: PathBuf,
    token: String,
    write_mode: bool,
    wait: bool,
    stop_mode: Option<String>,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    // Toggle behavior: probe `/v1/status` first. If a daemon-mic
    // capture is already in progress, this invocation acts as a stop
    // signal; otherwise we start a fresh recording. The daemon's
    // `/v1/stt/transcribe` endpoint refuses with `409 recording_in_progress`
    // when something is already running — the protocol places the
    // start-or-stop decision on the client (see
    // `docs/protocol/endpoints/v1/transcribe.md`).
    let status = http_client::status(socket_path.clone(), &token).await?;
    if status.stt.busy {
        return cmd_stop(socket_path, token).await;
    }

    let resp = http_client::transcribe(
        socket_path,
        &token,
        TranscribeOptions {
            write_mode,
            stop_mode,
            wait,
            // The CLI prints the final result (or fire-and-forgets); it does not
            // render incremental preview, so it never asks to stream it.
            stream_realtime: false,
        },
    )
    .await?;
    if resp.status != "success" {
        return Err(http_client::HttpError::Other(
            resp.message.unwrap_or_else(|| "record failed".to_string()),
        ));
    }
    if let Some(transcription) = resp.transcription {
        if transcription.trim().is_empty() {
            println!("(no speech detected)");
        } else {
            println!("{transcription}");
        }
    } else if let Some(message) = resp.message {
        println!("{message}");
    } else {
        println!("Recording started.");
    }
    Ok(())
}

async fn cmd_stop(
    socket_path: PathBuf,
    token: String,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    let resp = http_client::transcribe_stop(socket_path, &token).await?;
    if resp.status != "success" {
        return Err(http_client::HttpError::Other(
            resp.message.unwrap_or_else(|| "stop failed".to_string()),
        ));
    }
    println!(
        "{}",
        resp.message
            .unwrap_or_else(|| "stop signal sent".to_string())
    );
    Ok(())
}

/// The text to speak: the trailing arguments, or all of stdin when none were
/// given. Reading stdin is what makes `... | voice speak` work, which is how a
/// shell pipeline reaches the daemon without a client of its own.
fn read_text(sub: &ArgMatches) -> Result<String> {
    let words: Vec<&str> = sub
        .get_many::<String>("text")
        .map_or_default(|v| v.map(String::as_str).collect());
    if !words.is_empty() {
        return Ok(words.join(" "));
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("reading text from stdin")?;
    if buf.trim().is_empty() {
        return Err(anyhow!("no text given: pass it as arguments or on stdin"));
    }
    Ok(buf)
}

async fn cmd_speak(
    socket_path: PathBuf,
    token: String,
    text: String,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    // Not a toggle: a second `speak` deliberately preempts the first, which is
    // what "say this instead" means. Use `speak stop` to fall silent.
    let resp =
        super_voice_shared::tts::daemon::http_client::speak(socket_path, &token, &text).await?;
    report_speaking(resp)
}

/// Say that an utterance started, or why it did not.
fn report_speaking(
    resp: super_voice_shared::tts::models::protocol::DaemonResponse,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    if resp.status != "success" {
        return Err(http_client::HttpError::Other(
            resp.message.unwrap_or_else(|| "speak failed".to_string()),
        ));
    }
    // The daemon answers once the utterance is queued, while the device is
    // still playing it out, so the id is all there is to report.
    if let Some(id) = resp.utterance_id {
        println!("Speaking ({id}).");
    } else {
        println!("Speaking.");
    }
    Ok(())
}

/// `voice speak --read`: speak the selected or copied text, or stop. The
/// daemon reads it, from wherever the user's read method says. Pressed while
/// anything is speaking, the shortcut that runs this stops it rather than
/// reading again, the way `record` stops a recording. Without that, a long
/// passage could only be stopped from another command.
#[cfg(target_os = "linux")]
async fn cmd_speak_read(
    socket_path: PathBuf,
    token: String,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    let status = http_client::status(socket_path.clone(), &token).await?;
    if status.tts.busy {
        return cmd_speak_stop(socket_path, token).await;
    }
    let resp =
        super_voice_shared::tts::daemon::http_client::speak_read(socket_path, &token).await?;
    report_speaking(resp)
}

async fn cmd_speak_stop(
    socket_path: PathBuf,
    token: String,
) -> super_voice_shared::daemon::http_client::HttpResult<()> {
    let resp =
        super_voice_shared::tts::daemon::http_client::speak_stop(socket_path, &token).await?;
    if resp.status != "success" {
        return Err(http_client::HttpError::Other(
            resp.message.unwrap_or_else(|| "stop failed".to_string()),
        ));
    }
    // Stopping when nothing is speaking is a success, not an error — so say
    // which of the two happened rather than claiming to have stopped something.
    match resp.utterance_id {
        Some(id) => println!("Stopped ({id})."),
        None => println!("Nothing was speaking."),
    }
    Ok(())
}

fn cmd_logout() -> Result<()> {
    session::forget(APP_ID).map_err(|e| anyhow!(e))?;
    println!("Cached session token removed. Next invocation will trigger re-consent.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SCOPES;
    use super_voice_shared::daemon::scopes::is_known_scope;

    /// The CLI's requested scope set must be a subset of the daemon's shared
    /// catalog. Without this, a wire-scope rename in `scopes::known_scopes`
    /// would leave `SCOPES` requesting a token the daemon rejects, and the
    /// break would only surface at runtime as a failed `/auth/request`. Mirrors
    /// the consent binary's `scope_conformance` guard (audit 2 Tier 3 #33).
    #[test]
    fn requested_scopes_are_known() {
        for scope in SCOPES {
            assert!(
                is_known_scope(scope),
                "CLI requests scope `{scope}` that is not in scopes::known_scopes"
            );
        }
    }

    /// `stop` belongs to a feature: `voice record stop` and `voice speak stop`.
    #[test]
    fn each_feature_has_its_own_stop() {
        for feature in ["record", "speak"] {
            let matches = super::build_cli()
                .try_get_matches_from(["voice", feature, "stop"])
                .expect("parses");
            let (name, sub) = matches.subcommand().expect("a subcommand");
            assert_eq!(name, feature);
            assert!(
                matches!(sub.subcommand(), Some(("stop", _))),
                "{feature} stop"
            );
        }
        assert!(
            super::build_cli()
                .try_get_matches_from(["voice", "stop"])
                .is_err(),
            "a bare `stop` would not say which feature to stop"
        );
    }

    /// Words after `speak` are the text, and `--` lets the text be the word
    /// "stop".
    #[test]
    fn speak_takes_its_text_from_the_rest_of_the_line() {
        let text = |args: &[&str]| {
            let matches = super::build_cli()
                .try_get_matches_from(args)
                .expect("parses");
            let (_, sub) = matches.subcommand().expect("speak");
            assert!(
                sub.subcommand().is_none(),
                "{args:?} is text, not a subcommand"
            );
            sub.get_many::<String>("text")
                .expect("text")
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(text(&["voice", "speak", "hello", "there"]), "hello there");
        assert_eq!(text(&["voice", "speak", "please", "stop"]), "please stop");
        assert_eq!(text(&["voice", "speak", "--", "stop"]), "stop");
    }

    /// The binding the installer writes parses. What is read is all the text
    /// there is, so it goes with neither text nor `stop`.
    #[cfg(target_os = "linux")]
    #[test]
    fn speak_read_takes_no_text() {
        let matches = super::build_cli()
            .try_get_matches_from(["voice", "speak", "--read"])
            .expect("the COSMIC binding parses");
        let (_, sub) = matches.subcommand().expect("speak");
        assert!(sub.get_flag("read"));
        for args in [
            &["voice", "speak", "--read", "hello"][..],
            &["voice", "speak", "--read", "stop"][..],
        ] {
            assert!(
                super::build_cli().try_get_matches_from(args).is_err(),
                "{args:?}"
            );
        }
    }

    /// The binding the installer writes parses, and a recording's flags do
    /// not go with `record stop`.
    #[test]
    fn record_takes_its_flags_or_stop() {
        let matches = super::build_cli()
            .try_get_matches_from(["voice", "record", "--write"])
            .expect("the COSMIC binding parses");
        let (_, sub) = matches.subcommand().expect("record");
        assert!(sub.get_flag("write"));
        assert!(
            super::build_cli()
                .try_get_matches_from(["voice", "record", "--write", "stop"])
                .is_err()
        );
    }

    /// The hotkey `LaunchAgent`'s arguments must parse. launchd restarts the
    /// listener whenever it exits, so arguments clap rejects would become a
    /// restart every few seconds with nothing listening — and the only symptom
    /// would be a shortcut that silently does nothing. Mirrors the daemon's
    /// `every_shipped_execstart_parses`.
    #[cfg(target_os = "macos")]
    #[test]
    fn hotkey_launch_agent_arguments_parse() {
        let plist = include_str!("../launchd/ai.menjivar.super-voice.hotkey.plist");
        let array = plist
            .split("<key>ProgramArguments</key>")
            .nth(1)
            .and_then(|rest| rest.split("</array>").next())
            .expect("the plist has a ProgramArguments array");
        let args: Vec<String> = array
            .split("<string>")
            .skip(1)
            .filter_map(|s| s.split("</string>").next())
            .map(|s| s.replace("__BINDING__", super::hotkey::DEFAULT_BINDING))
            .collect();

        assert_eq!(args.get(1).map(String::as_str), Some("hotkey"));
        let matches = super::build_cli()
            .try_get_matches_from(&args)
            .expect("the hotkey LaunchAgent's arguments must parse");
        let (_, sub) = matches.subcommand().expect("a subcommand");
        let binding = sub.get_one::<String>("key").expect("--key has a value");
        super::hotkey::check(binding).expect("the default binding must be valid");
    }
}
