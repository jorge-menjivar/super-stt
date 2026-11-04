// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    super_voice_daemon::install_crypto_provider();
    super_voice_daemon::run().await?;
    Ok(())
}
