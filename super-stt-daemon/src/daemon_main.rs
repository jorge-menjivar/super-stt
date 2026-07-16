// SPDX-License-Identifier: GPL-3.0-only

//! Main daemon entry point and coordination.
//!
//! Boots the daemon, spawns the HTTP listener on
//! `$XDG_RUNTIME_DIR/stt/super-stt-http.sock`, and waits for a shutdown
//! signal. Client commands (`record`, `status`, etc.) live in
//! `super-stt-cli`, which talks to the daemon over HTTP.

use crate::cli;
use crate::config::DaemonConfig;
use crate::daemon::types::{DeviceOverride, SuperSTTDaemon};
use anyhow::Result;
use log::{error, info, warn};
use super_stt_shared::theme::AudioTheme;

/// Resolve the per-session overrides set on the command line. An override
/// is `Some` only when the user passed the flag explicitly — when not
/// set, the daemon falls back to whatever is in the config file.
fn resolve_cli_overrides(
    matches: &clap::ArgMatches,
    model: String,
    audio_theme: AudioTheme,
    force_cpu: bool,
) -> (Option<String>, Option<DeviceOverride>, Option<AudioTheme>) {
    let was_explicit =
        |name: &str| matches.value_source(name) == Some(clap::parser::ValueSource::CommandLine);
    let model_override = was_explicit("model").then_some(model);
    let device_override = was_explicit("device").then_some(if force_cpu {
        DeviceOverride::Cpu
    } else {
        DeviceOverride::Cuda
    });
    let audio_theme_override = was_explicit("audio-theme").then_some(audio_theme);
    (model_override, device_override, audio_theme_override)
}

/// Spawn the HTTP listener that serves every external request the daemon
/// answers. Failure to bind is fatal — there's no other transport.
///
/// Returns the listener task's [`JoinHandle`] so `run` can race it
/// against the shutdown signal: if the listener task exits before the
/// shutdown signal fires, the daemon treats itself as unreachable and
/// exits with a non-zero status.
///
/// The socket path is resolved by `get_http_socket_path`, which honors the
/// `SUPER_STT_HTTP_SOCKET` override (tests use this to bind a unique socket
/// without overriding `$XDG_RUNTIME_DIR`, which would break the Wayland
/// connection for spawned consent helpers). Clients resolve through the same
/// helper, so daemon and clients always agree on the path.
async fn spawn_http_listener(daemon: &SuperSTTDaemon) -> Result<tokio::task::JoinHandle<()>> {
    let http_socket_path = super_stt_shared::validation::get_http_socket_path();
    crate::daemon::http::start_http_server(
        std::sync::Arc::new(daemon.clone()),
        http_socket_path,
        daemon.shutdown_tx.clone(),
    )
    .await
}

/// Main entry point for the daemon
///
/// # Errors
///
/// Returns an error if the daemon fails to start.
///
/// # Panics
///
/// Panics if the daemon fails to initialize.
pub async fn run() -> Result<()> {
    let matches = cli::build().get_matches();
    let verbose = matches.get_flag("verbose");

    // Init logging BEFORE loading config so a "config invalid, reset to
    // defaults" warning emitted during the load is actually captured, instead
    // of being dropped before any logger exists (Tier 2 #6).
    super_stt_shared::logging::init_with(if verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });

    // Load saved configuration
    let config = DaemonConfig::load();

    // CLI flag overrides the saved preferred model for this session;
    // when it's not passed, fall back to whatever the config has.
    let model: String = matches.get_one::<String>("model").map_or_else(
        || config.transcription.preferred_model.clone(),
        Clone::clone,
    );

    let device = matches.get_one::<String>("device").unwrap();
    let force_cpu = device == "cpu";

    let audio_theme =
        if matches.value_source("audio-theme") == Some(clap::parser::ValueSource::CommandLine) {
            let audio_theme_str = matches.get_one::<String>("audio-theme").unwrap();
            audio_theme_str.parse::<AudioTheme>().unwrap_or_default()
        } else {
            config.audio.theme
        };

    info!("Starting Super STT Daemon");
    info!("Model: {model}");
    info!("Device: {device}");
    info!("Audio theme: {audio_theme}");

    let (model_override, device_override, audio_theme_override) =
        resolve_cli_overrides(&matches, model, audio_theme, force_cpu);

    let daemon = SuperSTTDaemon::new(model_override, device_override, audio_theme_override).await?;

    info!("Daemon initialized successfully");

    // Defensive: stop any `super-stt-backend-*` --user units left behind by
    // a previous daemon that exited ungracefully (SIGKILL / panic / a
    // skipped `Drop`). The transient unit's name embeds the spawning
    // daemon's PID, so a restarted daemon can't reach it via the regular
    // unload path — sweeping at startup is the only deterministic way to
    // recover from that. Only meaningful with the subprocess transport.
    #[cfg(feature = "subprocess-backends")]
    crate::stt_models::subprocess::cleanup_orphan_units().await;

    // Set up Ctrl+C handler
    let shutdown_tx = daemon.shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        info!("Received Ctrl+C, initiating shutdown...");
        let _ = shutdown_tx.send(());
    });

    // Subscribe BEFORE spawning the listener so a Ctrl+C arriving in
    // the gap between spawn and supervise can't slip past. broadcast
    // sends are dropped for receivers that don't yet exist; subscribing
    // first guarantees the supervisor sees every shutdown signal.
    let mut shutdown_rx = daemon.shutdown_tx.subscribe();

    let listener_handle = match spawn_http_listener(&daemon).await {
        Ok(h) => h,
        Err(e) => {
            warn!("HTTP listener failed to start: {e}");
            return Err(e);
        }
    };

    // Wait for either the shutdown signal or the listener task ending
    // early. The listener's spawned task is the daemon's only way to
    // accept new connections — if it panics or exits unexpectedly,
    // the daemon would otherwise stay parked here looking healthy
    // while being unreachable. Racing the JoinHandle catches both
    // panics (surfaced as `JoinError`) and any future code path that
    // returns from the loop without a shutdown signal.
    tokio::select! {
        biased;
        _ = shutdown_rx.recv() => {
            info!("Shutdown signal received");
        }
        join_result = listener_handle => {
            // The listener task ended before the shutdown signal —
            // signal shutdown defensively (in case anything else is
            // listening) and return an error so the process exits
            // non-zero. A `JoinError` indicates a panic; `Ok(())` here
            // means the spawned task body returned without ever
            // observing the shutdown_rx arm, which is itself a bug.
            error!("HTTP listener task exited unexpectedly: {join_result:?}");
            let _ = daemon.shutdown_tx.send(());
            return Err(anyhow::anyhow!(
                "HTTP listener task exited before shutdown signal"
            ));
        }
    }

    info!("Daemon stopped gracefully");

    // `std::process::exit` below skips every `Drop` destructor — without
    // this explicit unload the `systemd-run --user` subprocess backend
    // (e.g. Voxtral) would be orphaned. Call the daemon's shutdown unload
    // path so `Transcribe::shutdown()` runs in an async context and stops
    // the unit cleanly.
    daemon.shutdown_unload().await;

    // Drain any queued session-store writes before `process::exit` skips the
    // fire-and-forget persist task — otherwise a token minted or revoked in the
    // final moments is lost (audit 2 Tier 1 #5). Bounded so a locked keyring
    // (D-Bus unlock prompt) can't hang shutdown.
    crate::daemon::http::flush_persisted_sessions(tokio::time::Duration::from_secs(5)).await;

    // Give a brief moment for any remaining cleanup, then force exit if needed
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    info!("Exiting daemon process");
    std::process::exit(0);
}
