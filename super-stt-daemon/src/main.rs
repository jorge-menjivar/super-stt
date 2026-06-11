// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    super_stt_daemon::install_crypto_provider();
    super_stt_daemon::keyring::install_mock_if_requested();
    super_stt_daemon::run().await?;
    Ok(())
}
