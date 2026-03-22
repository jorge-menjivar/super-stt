// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use log::debug;
use std::process::Command;

pub struct YdotoolBackend;

impl YdotoolBackend {
    pub fn new() -> Self {
        Self
    }

    /// Check if ydotool is available by running a harmless command.
    /// This verifies both the binary and that ydotoold is accepting connections.
    pub fn is_available() -> bool {
        // Type an empty string — succeeds only if ydotoold is running.
        Command::new("ydotool")
            .args(["type", "--", ""])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    #[allow(clippy::unused_self)]
    pub fn type_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        debug!("ydotool type ({} chars)", text.len());
        let output = Command::new("ydotool")
            .arg("type")
            .arg("--")
            .arg(text)
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run ydotool type: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("ydotool type failed: {stderr}"));
        }

        Ok(())
    }

    #[allow(clippy::unused_self)]
    pub fn backspace_n(&self, n: usize) -> Result<()> {
        if n == 0 {
            return Ok(());
        }

        // evdev keycode 14 = Backspace, 1 = press, 0 = release
        let mut args = vec!["key".to_string()];
        for _ in 0..n {
            args.push("14:1".to_string());
            args.push("14:0".to_string());
        }

        debug!("ydotool backspace ({n} keys)");
        let output = Command::new("ydotool")
            .args(&args)
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run ydotool key: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("ydotool key failed: {stderr}"));
        }

        Ok(())
    }
}
