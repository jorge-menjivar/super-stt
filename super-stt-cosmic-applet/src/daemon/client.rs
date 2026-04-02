// SPDX-License-Identifier: GPL-3.0-only
use std::path::PathBuf;
use std::sync::OnceLock;

// Generate a unique client ID for this applet instance
static CLIENT_ID: OnceLock<String> = OnceLock::new();

fn get_client_id() -> &'static str {
    CLIENT_ID
        .get_or_init(|| super_stt_shared::validation::generate_secure_client_id("super-stt-applet"))
}

/// Ping daemon to check if it's running and responsive
pub async fn ping_daemon(socket_path: PathBuf) -> Result<String, String> {
    super_stt_shared::daemon::client::ping_daemon(socket_path, get_client_id()).await
}

/// Get current daemon configuration
pub async fn fetch_daemon_config(socket_path: PathBuf) -> Result<serde_json::Value, String> {
    super_stt_shared::daemon::client::fetch_daemon_config(socket_path, get_client_id()).await
}

/// Ping daemon and get extended connection status information
pub async fn ping_daemon_with_status(
    socket_path: PathBuf,
) -> Result<super_stt_shared::daemon::client::PingResponse, String> {
    super_stt_shared::daemon::client::ping_daemon_with_status(socket_path, get_client_id()).await
}
