// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

pub struct EnigoBackend {
    typing_chunk: usize,
    backspace_batch_size: usize,
    enigo: Enigo,
}

impl EnigoBackend {
    pub fn new() -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("Failed to initialize enigo: {e}"))?;

        Ok(Self {
            typing_chunk: 64,
            backspace_batch_size: 20,
            enigo,
        })
    }

    /// Type `text` one character at a time through the virtual keyboard.
    ///
    /// Deliberately not `enigo.text()`. When the focused application speaks
    /// the input-method protocol, that commits the text in one piece, and a
    /// terminal hands such a commit to the shell as a paste. The shell may then
    /// rewrite it: fish escapes a `'` pasted inside an open quoted string, so
    /// `OpenAI's … I'm` landed as `OpenAI's … I\'m`. It also puts the text on
    /// a different channel from the backspaces that later erase it. Key events
    /// keep everything on one channel and are never a paste.
    ///
    /// Synchronous: enigo's handle holds raw xkbcommon pointers (`!Send`) and
    /// the work blocks (per-chunk sleeps). The [`Simulator`](super::Simulator)
    /// enum runs this under `block_in_place` so the handle never crosses an
    /// `.await` point — no thread migration mid-type (audit Tier 3 #35).
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(self.typing_chunk) {
            for &c in chunk {
                self.enigo
                    .key(Key::Unicode(c), Direction::Click)
                    .map_err(|e| anyhow::anyhow!("Failed to type {c:?}: {e}"))?;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        Ok(())
    }

    /// Press Backspace `n` times.
    ///
    /// # Errors
    /// The first click the backend refuses. Nothing can read the screen back
    /// to check that a keystroke landed, so a refusal is the one signal there
    /// is; swallowing it left a surviving character unexplained in the log.
    pub fn backspace_n(&mut self, n: usize) -> Result<()> {
        let mut sent = 0;
        while sent < n {
            let batch_size = (n - sent).min(self.backspace_batch_size);
            for _ in 0..batch_size {
                self.enigo
                    .key(Key::Backspace, Direction::Click)
                    .map_err(|e| anyhow::anyhow!("Backspace {} of {n} failed: {e}", sent + 1))?;
                sent += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    }
}
