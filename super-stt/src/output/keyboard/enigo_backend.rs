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

    pub fn type_text(&mut self, text: &str) -> Result<()> {
        let mut i = 0;
        let chars: Vec<char> = text.chars().collect();
        while i < chars.len() {
            let end = (i + self.typing_chunk).min(chars.len());
            let segment: String = chars[i..end].iter().collect();
            self.enigo
                .text(&segment)
                .map_err(|e| anyhow::anyhow!("Failed to type segment: {e}"))?;
            i = end;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn backspace_n(&mut self, n: usize) -> Result<()> {
        let mut remaining = n;
        while remaining > 0 {
            let batch_size = remaining.min(self.backspace_batch_size);
            for _ in 0..batch_size {
                let _ = self.enigo.key(Key::Backspace, Direction::Click);
            }
            remaining -= batch_size;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    }
}
