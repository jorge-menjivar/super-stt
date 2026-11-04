// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    super_tts_daemon::install_crypto_provider();
    super_tts_daemon::run().await?;
    Ok(())
}
