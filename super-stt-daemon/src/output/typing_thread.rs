// SPDX-License-Identifier: GPL-3.0-only

use anyhow::Result;
use enigo::{Enigo, Settings};
use log::{debug, info, warn};
use std::sync::{Arc, atomic::AtomicUsize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::output::preview::{State, Typer};

/// Messages that can be sent to the typing thread
#[derive(Debug)]
pub enum TypingMessage {
    /// Update preview text
    UpdatePreview {
        new_text: String,
        response: mpsc::UnboundedSender<()>,
    },
    /// Process final text (completed sentence)
    ProcessFinal {
        final_text: String,
        response: mpsc::UnboundedSender<()>,
    },
    /// Clear all typed text
    Clear { response: mpsc::UnboundedSender<()> },
    /// Get current character count
    GetCharCount {
        response: mpsc::UnboundedSender<usize>,
    },
    /// Shutdown the typing thread
    Shutdown,
}

/// Handle for communicating with the typing thread
#[derive(Clone)]
pub struct TypingThreadHandle {
    sender: mpsc::UnboundedSender<TypingMessage>,
}

impl TypingThreadHandle {
    /// Create a new typing thread and return a handle to communicate with it
    ///
    /// # Errors
    ///
    /// Returns an error if thread creation fails.
    pub fn spawn() -> Result<Self> {
        let (sender, receiver) = mpsc::unbounded_channel();

        std::thread::spawn(move || {
            if let Err(e) = Self::run_typing_thread(receiver) {
                warn!("Typing thread error: {e}");
            }
        });

        Ok(Self { sender })
    }

    /// Update preview text (non-blocking)
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be sent to the typing thread.
    pub async fn update_preview(&self, new_text: String) -> Result<()> {
        let (response_tx, mut response_rx) = mpsc::unbounded_channel();

        self.sender.send(TypingMessage::UpdatePreview {
            new_text,
            response: response_tx,
        })?;

        response_rx.recv().await;
        Ok(())
    }

    /// Process final text (non-blocking)
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be sent to the typing thread.
    pub async fn process_final(&self, final_text: String) -> Result<()> {
        let (response_tx, mut response_rx) = mpsc::unbounded_channel();

        self.sender.send(TypingMessage::ProcessFinal {
            final_text,
            response: response_tx,
        })?;

        response_rx.recv().await;
        Ok(())
    }

    /// Clear all typed text (non-blocking)
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be sent to the typing thread.
    pub async fn clear(&self) -> Result<()> {
        let (response_tx, mut response_rx) = mpsc::unbounded_channel();

        self.sender.send(TypingMessage::Clear {
            response: response_tx,
        })?;

        response_rx.recv().await;
        Ok(())
    }

    /// Get current character count (non-blocking)
    ///
    /// # Errors
    ///
    /// Returns an error if the message cannot be sent to the typing thread.
    pub async fn get_char_count(&self) -> Result<usize> {
        let (response_tx, mut response_rx) = mpsc::unbounded_channel();

        self.sender.send(TypingMessage::GetCharCount {
            response: response_tx,
        })?;

        Ok(response_rx.recv().await.unwrap_or(0))
    }

    /// Shutdown the typing thread
    pub fn shutdown(&self) {
        let _ = self.sender.send(TypingMessage::Shutdown);
    }

    /// Run the typing thread - this runs in a dedicated OS thread
    fn run_typing_thread(mut receiver: mpsc::UnboundedReceiver<TypingMessage>) -> Result<()> {
        info!("Starting typing thread");

        // Initialize enigo in this thread
        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|e| anyhow::anyhow!("Failed to initialize keyboard in typing thread: {e}"))?;

        let mut actually_typed = String::new();
        let typed_counter = Arc::new(AtomicUsize::new(0));
        let mut state = State::default();
        let cancellation_token = CancellationToken::new();

        // Use a blocking runtime for this thread
        let rt = tokio::runtime::Runtime::new()?;

        rt.block_on(async {
            while let Some(message) = receiver.recv().await {
                match message {
                    TypingMessage::UpdatePreview { new_text, response } => {
                        debug!(
                            "Typing thread: updating preview to '{}'",
                            new_text.chars().take(30).collect::<String>()
                        );

                        Typer::update_preview(
                            &mut enigo,
                            &new_text,
                            &mut actually_typed,
                            &typed_counter,
                            &mut state,
                            &cancellation_token,
                        );

                        let _ = response.send(());
                    }
                    TypingMessage::ProcessFinal {
                        final_text,
                        response,
                    } => {
                        debug!(
                            "Typing thread: processing final text '{}'",
                            final_text.chars().take(30).collect::<String>()
                        );

                        Typer::process_final_text(
                            &mut enigo,
                            &final_text,
                            &mut actually_typed,
                            &typed_counter,
                            &mut state,
                            &cancellation_token,
                        );

                        let _ = response.send(());
                    }
                    TypingMessage::Clear { response } => {
                        debug!("Typing thread: clearing all text");

                        Typer::clear_preview(
                            &mut enigo,
                            &mut actually_typed,
                            &typed_counter,
                            &mut state,
                            &cancellation_token,
                        );

                        let _ = response.send(());
                    }
                    TypingMessage::GetCharCount { response } => {
                        let count = typed_counter.load(std::sync::atomic::Ordering::Relaxed);
                        let _ = response.send(count);
                    }
                    TypingMessage::Shutdown => {
                        info!("Typing thread shutting down");
                        break;
                    }
                }
            }
        });

        info!("Typing thread exited");
        Ok(())
    }
}
