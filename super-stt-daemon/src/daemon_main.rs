// SPDX-License-Identifier: GPL-3.0-only

//! Main daemon entry point and coordination
//!
//! This module serves as the entry point for the daemon and coordinates
//! between the modular daemon components.

use crate::cli;
use crate::config::DaemonConfig;
use crate::daemon::types::{DeviceOverride, SuperSTTDaemon};
use anyhow::{Context, Result};
use log::{error, info, warn};
use std::path::PathBuf;
use super_stt_shared::theme::AudioTheme;

/// Initialize the `env_logger`. Respects `RUST_LOG`; otherwise sets the
/// level from the `--verbose` flag.
fn init_logging(verbose: bool) {
    if std::env::var("RUST_LOG").is_ok() {
        env_logger::init();
    } else {
        let log_level = if verbose {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        };
        env_logger::Builder::from_default_env()
            .filter_level(log_level)
            .init();
    }
}

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

/// Spawn the new HTTP listener side-by-side with the legacy length-prefix
/// listener. Failure to bind is non-fatal — the legacy path keeps working
/// so existing clients don't break during the rollout.
///
/// `SUPER_STT_HTTP_SOCKET` overrides the default path; tests use this to
/// bind a unique socket without overriding `$XDG_RUNTIME_DIR` (which would
/// break the Wayland connection for spawned consent helpers).
async fn spawn_http_listener(daemon: &SuperSTTDaemon) {
    let http_socket_path = std::env::var("SUPER_STT_HTTP_SOCKET").ok().map_or_else(
        super_stt_shared::validation::get_http_socket_path,
        PathBuf::from,
    );
    if let Err(e) = crate::daemon::http_server::start_http_server(
        std::sync::Arc::new(daemon.clone()),
        http_socket_path.clone(),
        daemon.shutdown_tx.clone(),
    )
    .await
    {
        warn!(
            "HTTP listener failed to start at {}: {e}. Legacy listener will continue.",
            http_socket_path.display()
        );
    }
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

    // Check if record subcommand was used
    if let Some(record_matches) = matches.subcommand_matches("record") {
        return handle_record_command(record_matches).await;
    }

    // Check if ping subcommand was used
    if matches.subcommand_matches("ping").is_some() {
        return handle_ping_command(&matches).await;
    }

    // Check if status subcommand was used
    if matches.subcommand_matches("status").is_some() {
        return handle_status_command(&matches).await;
    }

    // Standard daemon mode
    // Load saved configuration first
    let config = DaemonConfig::load();

    // CLI flag overrides the saved preferred model for this session;
    // when it's not passed, fall back to whatever the config has.
    let model: String = matches.get_one::<String>("model").map_or_else(
        || config.transcription.preferred_model.clone(),
        Clone::clone,
    );

    let device = matches.get_one::<String>("device").unwrap();
    let force_cpu = device == "cpu";
    let verbose = matches.get_flag("verbose");
    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .unwrap_or(&cli::DEFAULT_SOCKET_PATH);

    let audio_theme =
        if matches.value_source("audio-theme") == Some(clap::parser::ValueSource::CommandLine) {
            let audio_theme_str = matches.get_one::<String>("audio-theme").unwrap();
            audio_theme_str.parse::<AudioTheme>().unwrap_or_default()
        } else {
            config.audio.theme
        };

    init_logging(verbose);

    info!("Starting Super STT Daemon");
    info!("Socket path: {}", socket_path.display());
    info!("Model: {model}");
    info!("Device: {device}");
    info!("Audio theme: {audio_theme}");

    let (model_override, device_override, audio_theme_override) =
        resolve_cli_overrides(&matches, model, audio_theme, force_cpu);

    let daemon = SuperSTTDaemon::new(
        socket_path.clone(),
        model_override,
        device_override,
        audio_theme_override,
    )
    .await?;

    info!("Daemon initialized successfully");

    // Set up Ctrl+C handler
    let shutdown_tx = daemon.shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        info!("Received Ctrl+C, initiating shutdown...");
        let _ = shutdown_tx.send(());
    });

    spawn_http_listener(&daemon).await;

    // Start the daemon and wait for it to complete
    daemon.start().await?;

    info!("Daemon stopped gracefully");

    // Give a brief moment for any remaining cleanup, then force exit if needed
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    info!("Exiting daemon process");
    std::process::exit(0);
}

/// Handle the record subcommand - direct recording mode
async fn handle_record_command(matches: &clap::ArgMatches) -> Result<()> {
    let write_mode = matches.get_flag("write");
    let wait = matches.get_flag("wait");
    let config = DaemonConfig::load();
    // Resolve stop mode: CLI flag → config file → default
    let stop_mode = if let Some(mode) = matches.get_one::<String>("stop-mode") {
        mode.clone()
    } else {
        config.transcription.recording_stop_mode.to_string()
    };
    // Resolve write method: CLI flag → config file → default (auto)
    let write_method = if let Some(method) = matches.get_one::<String>("write-method") {
        method.clone()
    } else {
        config.transcription.write_method.to_string()
    };
    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .unwrap_or(&cli::DEFAULT_SOCKET_PATH);

    // Initialize logging for recording mode - respect RUST_LOG env var
    if std::env::var("RUST_LOG").is_ok() {
        env_logger::init();
    } else {
        env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .init();
    }

    info!("Super STT Direct Recording Mode");

    // Try to connect to existing daemon first
    if socket_path.exists() {
        info!("Found existing daemon, sending record request...");
        return send_record_request_to_daemon(
            socket_path,
            write_mode,
            &stop_mode,
            &write_method,
            wait,
        )
        .await;
    }

    // If no daemon is running, inform user to start it first
    error!("❌ No Super STT daemon is running.");
    error!("Please start the daemon first:");
    error!("  stt");
    error!("Then try recording again:");
    error!("  stt record");

    std::process::exit(1);
}

/// Handle the ping command - check if daemon is running
async fn handle_ping_command(matches: &clap::ArgMatches) -> Result<()> {
    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .unwrap_or(&cli::DEFAULT_SOCKET_PATH);

    // Check if socket exists and is accessible
    if socket_path.exists() {
        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(_) => {
                std::process::exit(0);
            }
            Err(_) => {
                std::process::exit(1);
            }
        }
    } else {
        std::process::exit(1);
    }
}

/// Handle the status command - get daemon status information
async fn handle_status_command(matches: &clap::ArgMatches) -> Result<()> {
    let socket_path = matches
        .get_one::<PathBuf>("socket")
        .unwrap_or(&cli::DEFAULT_SOCKET_PATH);

    // Try to connect to daemon and get status
    match send_status_request_to_daemon(socket_path).await {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            error!("❌ Error getting status: {e}");
            std::process::exit(1);
        }
    }
}

/// Send a record request to an existing daemon and wait for acknowledgment
async fn send_record_request_to_daemon(
    socket_path: &PathBuf,
    write_mode: bool,
    stop_mode: &str,
    write_method: &str,
    wait: bool,
) -> Result<()> {
    use super_stt_shared::models::protocol::{DaemonRequest, DaemonResponse};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(socket_path)
        .await
        .context("Failed to connect to daemon")?;

    // Send record request
    let request = DaemonRequest {
        command: "record".to_string(),
        audio_data: None,
        sample_rate: None,
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        client_id: Some("record_client".to_string()),
        data: Some(serde_json::json!({
            "write_mode": write_mode,
            "stop_mode": stop_mode,
            "write_method": write_method,
            "wait": wait,
        })),
        language: None,
        enabled: None,
    };

    let request_data = serde_json::to_vec(&request)?;
    let request_size = request_data.len() as u64;

    // Send size then data
    stream.write_all(&request_size.to_be_bytes()).await?;
    stream.write_all(&request_data).await?;

    // Read responses from the daemon.
    // When wait=true, the daemon may stream preview_text responses before the final one.
    loop {
        let mut size_bytes = [0u8; 8];
        stream
            .read_exact(&mut size_bytes)
            .await
            .context("Failed to read response from daemon")?;
        let response_size = u64::from_be_bytes(size_bytes);
        let response_len = usize::try_from(response_size)
            .context("Response size does not fit into memory on this platform")?;
        let mut response_data = vec![0u8; response_len];
        stream
            .read_exact(&mut response_data)
            .await
            .context("Failed to read response data from daemon")?;
        let response: DaemonResponse = serde_json::from_slice(&response_data)?;

        // Preview text — overwrite the current line
        if let Some(preview) = &response.preview_text {
            use std::io::Write;
            print!("\r\x1b[K{preview}");
            std::io::stdout().flush().ok();
            continue;
        }

        // Final response — clear the preview line first if we were streaming
        if wait {
            use std::io::Write;
            print!("\r\x1b[K");
            std::io::stdout().flush().ok();
        }

        if response.status == "success" {
            let msg = response.message.as_deref().unwrap_or("");
            if msg == DaemonResponse::RECORDING_STOP_SIGNAL_MSG {
                info!("🛑 Recording stopped successfully");
            } else if wait {
                if let Some(transcription) = &response.transcription {
                    if transcription.is_empty() {
                        info!("🎤 No speech detected");
                    } else {
                        info!("🎤 Transcription: {transcription}");
                    }
                } else {
                    info!("🎤 {msg}");
                }
            } else {
                info!("🎤 Recording started (stop mode: {stop_mode})");
                if write_mode {
                    info!("📝 Will type transcription when complete");
                }
            }
        } else {
            warn!(
                "Daemon rejected record request: {}",
                response.message.unwrap_or_default()
            );
        }

        break;
    }

    Ok(())
}

/// Send a status request to an existing daemon and display the response
async fn send_status_request_to_daemon(socket_path: &PathBuf) -> Result<()> {
    use super_stt_shared::models::protocol::{DaemonRequest, DaemonResponse};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let mut stream = UnixStream::connect(socket_path)
        .await
        .context("Failed to connect to daemon")?;

    // Send status request
    let request = DaemonRequest {
        command: "status".to_string(),
        audio_data: None,
        sample_rate: None,
        event_types: None,
        client_info: None,
        since_timestamp: None,
        limit: None,
        event_type: None,
        client_id: Some("status_client".to_string()),
        data: None,
        language: None,
        enabled: None,
    };

    let request_data = serde_json::to_vec(&request)?;
    let request_size = request_data.len() as u64;

    // Send size then data
    stream.write_all(&request_size.to_be_bytes()).await?;
    stream.write_all(&request_data).await?;

    // Read response size
    let mut size_bytes = [0u8; 8];
    stream.read_exact(&mut size_bytes).await?;
    let response_size = u64::from_be_bytes(size_bytes);

    // Read response data
    let response_len: usize = usize::try_from(response_size)
        .context("Response size does not fit into memory on this platform")?;
    let mut response_data = vec![0u8; response_len];
    stream.read_exact(&mut response_data).await?;

    // Parse response
    let response: DaemonResponse = serde_json::from_slice(&response_data)?;

    // Display status information
    match response.status.as_str() {
        "success" => {
            info!("Daemon Status:");
            info!(
                "  Model: {}",
                response
                    .current_model
                    .unwrap_or_else(|| "unknown".to_string())
            );
            info!(
                "  Device: {}",
                response.device.unwrap_or("unknown".to_string())
            );
        }
        "error" => {
            let message = response.message.unwrap_or("Unknown error".to_string());
            error!("❌ Error from daemon: {message}");
            return Err(anyhow::anyhow!("Daemon error: {message}"));
        }
        _ => {
            error!("❌ Unexpected response from daemon: {}", response.status);
            return Err(anyhow::anyhow!(
                "Unexpected response status: {}",
                response.status
            ));
        }
    }

    Ok(())
}
