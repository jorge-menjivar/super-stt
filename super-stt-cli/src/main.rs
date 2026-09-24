// SPDX-License-Identifier: GPL-3.0-only
//! Super STT CLI — talks to the daemon over the HTTP protocol on a Unix socket.
//!
//! Auth flow is delegated to `super_stt_shared::daemon::session`, the
//! shared client-side session helper. All client tokens (CLI, settings
//! app, applet) live under the same keyring service
//! (`super-stt-session`) keyed by per-app `AppId`. The CLI's entry is
//! `super-stt-cli@super-stt-session`.
//!
//! `session::with_token` handles cache-hit, consent-popup-on-miss, and
//! `invalid_session` retry transparently — handlers below just write
//! the network call and propagate the daemon's error string.
//!
//! For tests / CI: set `SUPER_STT_AUTO_APPROVE=1` in the daemon's
//! environment so the consent popup is bypassed and `/auth/request`
//! auto-approves.

use anyhow::{Context, Result, anyhow};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use std::path::PathBuf;
use super_stt_shared::daemon::http_client::{self, TranscribeOptions};
use super_stt_shared::daemon::session::{self, AppId};
use super_stt_shared::validation::get_http_socket_path;

#[cfg(target_os = "macos")]
mod hotkey;
#[cfg(target_os = "macos")]
mod service;

const APP_ID: AppId = super_stt_shared::daemon::session::app_id("super-stt-cli");
const APP_NAME: &str = "Super STT CLI";
const SCOPES: &[&str] = &["transcribe", "status"];

fn main() -> Result<()> {
    // The CLI previously had no logging at all; initialize it like the other
    // binaries (RUST_LOG wins, else Info) (Tier 2 #6).
    super_stt_shared::logging::init();

    // Honor SUPER_STT_KEYRING_MOCK before any session-token access so
    // automated shells / CI (and our own integration tests) don't block on
    // the system secret service. No-op when the env var is unset.
    session::install_mock_keyring_if_requested();

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
        Some(("stop", _)) => {
            run_with_token(socket_path.clone(), |t| cmd_stop(socket_path.clone(), t))
                .await
                .context("stop failed")
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
        Command::new("super-stt-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Super STT command-line client (HTTP protocol)")
        .arg(
            Arg::new("socket")
                .long("socket")
                .help("Path to the daemon HTTP Unix socket")
                .value_parser(value_parser!(PathBuf))
                .global(true),
        )
        .subcommand(Command::new("ping").about("Check that the daemon is reachable"))
        .subcommand(Command::new("status").about("Print daemon state (model + device)"))
        .subcommand(
            Command::new("record")
                .about("Start a daemon-mic recording")
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
                            super_stt_shared::models::recording_stop_mode::RecordingStopMode::WIRE_VARIANTS
                                .iter()
                                .copied(),
                        )),
                ),
        )
        .subcommand(Command::new("stop").about("Stop an in-flight daemon-mic recording"))
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

/// Nothing to add on Linux: the desktop environment binds the shortcut there
/// and runs `stt record --write` itself, and systemd runs the daemon.
#[cfg(not(target_os = "macos"))]
fn platform_subcommands(cli: Command) -> Command {
    cli
}

/// Thin wrapper over `session::with_token` that adapts its
/// `Result<T, String>` return into `anyhow::Result<T>` and supplies
/// the CLI's app identity. All commands route through here.
async fn run_with_token<F, Fut, T>(socket_path: PathBuf, op: F) -> Result<T>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = super_stt_shared::daemon::http_client::HttpResult<T>>,
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
    Fut: std::future::Future<Output = super_stt_shared::daemon::http_client::HttpResult<T>>,
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
) -> super_stt_shared::daemon::http_client::HttpResult<()> {
    let msg = http_client::ping(socket_path, &token).await?;
    println!("{msg}");
    Ok(())
}

async fn cmd_status(
    socket_path: PathBuf,
    token: String,
) -> super_stt_shared::daemon::http_client::HttpResult<()> {
    let resp = http_client::status(socket_path, &token).await?;
    if resp.status != "success" {
        return Err(http_client::HttpError::Other(
            resp.message.unwrap_or_else(|| "unknown error".to_string()),
        ));
    }
    println!(
        "Model:  {}",
        resp.current_model.as_deref().unwrap_or("(none loaded)")
    );
    println!("Device: {}", resp.device.as_deref().unwrap_or("unknown"));
    Ok(())
}

async fn cmd_record(
    socket_path: PathBuf,
    token: String,
    write_mode: bool,
    wait: bool,
    stop_mode: Option<String>,
) -> super_stt_shared::daemon::http_client::HttpResult<()> {
    // Toggle behavior: probe `/v1/status` first. If a daemon-mic
    // capture is already in progress, this invocation acts as a stop
    // signal; otherwise we start a fresh recording. The daemon's
    // `/v1/transcribe` endpoint refuses with `409 recording_in_progress`
    // when something is already running — the protocol places the
    // start-or-stop decision on the client (see
    // `docs/protocol/endpoints/v1/transcribe.md`).
    let status = http_client::status(socket_path.clone(), &token).await?;
    if status.busy.unwrap_or(false) {
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
) -> super_stt_shared::daemon::http_client::HttpResult<()> {
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

fn cmd_logout() -> Result<()> {
    session::forget(APP_ID).map_err(|e| anyhow!(e))?;
    println!("Cached session token removed. Next invocation will trigger re-consent.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::SCOPES;
    use super_stt_shared::daemon::scopes::is_known_scope;

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

    /// The hotkey `LaunchAgent`'s arguments must parse. launchd restarts the
    /// listener whenever it exits, so arguments clap rejects would become a
    /// restart every few seconds with nothing listening — and the only symptom
    /// would be a shortcut that silently does nothing. Mirrors the daemon's
    /// `every_shipped_execstart_parses`.
    #[cfg(target_os = "macos")]
    #[test]
    fn hotkey_launch_agent_arguments_parse() {
        let plist = include_str!("../launchd/ai.menjivar.super-stt.hotkey.plist");
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
