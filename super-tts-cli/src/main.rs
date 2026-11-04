// SPDX-License-Identifier: GPL-3.0-only
//! Super TTS CLI — talks to the daemon over the HTTP protocol on a Unix socket.
//!
//! Auth flow is delegated to `super_tts_shared::daemon::session`, the
//! shared client-side session helper. All client tokens (CLI, settings
//! app, applet) live under the same keyring service
//! (`super-tts-session`) keyed by per-app `AppId`. The CLI's entry is
//! `super-tts-cli@super-tts-session`.
//!
//! `session::with_token` handles cache-hit, consent-popup-on-miss, and
//! `invalid_session` retry transparently — handlers below just write
//! the network call and propagate the daemon's error string.
//!
//! For tests / CI: set `SUPER_TTS_AUTO_APPROVE=1` in the daemon's
//! environment so the consent popup is bypassed and `/auth/request`
//! auto-approves.

use anyhow::{Context, Result, anyhow};
use clap::{Arg, Command, value_parser};
use std::io::Read;
use std::path::PathBuf;
use super_tts_shared::daemon::http_client;
use super_tts_shared::daemon::session::{self, AppId};
use super_tts_shared::validation::get_http_socket_path;

const APP_ID: AppId = super_tts_shared::daemon::session::app_id("super-tts-cli");
const APP_NAME: &str = "Super TTS CLI";
const SCOPES: &[&str] = &["speak", "status"];

#[tokio::main]
async fn main() -> Result<()> {
    // The CLI previously had no logging at all; initialize it like the other
    // binaries (RUST_LOG wins, else Info) (Tier 2 #6).
    super_tts_shared::logging::init();

    // Honor SUPER_TTS_KEYRING_MOCK before any session-token access so
    // automated shells / CI (and our own integration tests) don't block on
    // the system secret service. No-op when the env var is unset.
    session::install_mock_keyring_if_requested();

    let matches = Command::new("super-tts-cli")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Super TTS command-line client (HTTP protocol)")
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
            Command::new("speak")
                .about("Speak text with the active model")
                .arg(
                    Arg::new("text")
                        .help("The text to speak; omit to read it from stdin")
                        .num_args(0..)
                        .trailing_var_arg(true),
                ), // No --voice/--language/--speed/--instructions: how the
                   // machine speaks is the user's configuration, not a flag on the
                   // thing that makes it talk. Set a voice with the settings app
                   // or `POST /pipeline/{stage}/model/{model}/voice`.
        )
        .subcommand(Command::new("stop").about("Stop the utterance now playing"))
        .subcommand(
            Command::new("logout")
                .about("Forget the cached session token (forces re-consent next call)"),
        )
        .get_matches();

    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .cloned()
        .unwrap_or_else(get_http_socket_path);

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
        Some(("speak", sub)) => {
            let text = read_text(sub)?;
            run_with_token(socket_path.clone(), |t| {
                cmd_speak(socket_path.clone(), t, text.clone())
            })
            .await
            .context("speak failed")
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

/// Thin wrapper over `session::with_token` that adapts its
/// `Result<T, String>` return into `anyhow::Result<T>` and supplies
/// the CLI's app identity. All commands route through here.
async fn run_with_token<F, Fut, T>(socket_path: PathBuf, op: F) -> Result<T>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = super_tts_shared::daemon::http_client::HttpResult<T>>,
{
    session::with_token(socket_path, APP_ID, APP_NAME, SCOPES, op)
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
) -> super_tts_shared::daemon::http_client::HttpResult<()> {
    let msg = http_client::ping(socket_path, &token).await?;
    println!("{msg}");
    Ok(())
}

async fn cmd_status(
    socket_path: PathBuf,
    token: String,
) -> super_tts_shared::daemon::http_client::HttpResult<()> {
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
    println!(
        "State:  {}",
        if resp.busy.unwrap_or(false) {
            "speaking"
        } else {
            "idle"
        }
    );
    Ok(())
}

/// The text to speak: the trailing arguments, or all of stdin when none were
/// given. Reading stdin is what makes `... | super-tts-cli speak` work, which is
/// how a shell pipeline reaches the daemon without a client of its own.
fn read_text(sub: &clap::ArgMatches) -> Result<String> {
    let words: Vec<&str> = sub
        .get_many::<String>("text")
        .map(|v| v.map(String::as_str).collect())
        .unwrap_or_default();
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
) -> super_tts_shared::daemon::http_client::HttpResult<()> {
    // Not a toggle: a second `speak` deliberately preempts the first, which is
    // what "say this instead" means. Use `stop` to fall silent.
    let resp = http_client::speak(socket_path, &token, &text).await?;
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

async fn cmd_stop(
    socket_path: PathBuf,
    token: String,
) -> super_tts_shared::daemon::http_client::HttpResult<()> {
    let resp = http_client::speak_stop(socket_path, &token).await?;
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
    use super_tts_shared::daemon::scopes::is_known_scope;

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
}
