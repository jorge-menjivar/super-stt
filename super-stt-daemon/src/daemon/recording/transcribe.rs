// SPDX-License-Identifier: GPL-3.0-only

use crate::daemon::types::SuperSTTDaemon;
use crate::output::preview::Typer;
use anyhow::{Context, Result};
use log::{debug, error, info, warn};
use std::sync::Arc;

impl SuperSTTDaemon {
    /// Transcribe a chunk of audio data for preview
    pub(super) async fn transcribe_audio_chunk(&self, audio_data: &[f32]) -> Result<String> {
        debug!(
            "Processing {} samples for preview transcription",
            audio_data.len()
        );

        // Basic validation of audio data
        if audio_data.is_empty() {
            debug!("Audio data is empty, skipping transcription");
            return Ok(String::new());
        }

        // Check audio length - need at least 1 second of audio for decent transcription
        if audio_data.len() < 16000 {
            debug!(
                "Audio data too short ({} samples), skipping transcription",
                audio_data.len()
            );
            return Ok(String::new());
        }

        // Process audio
        let processed_audio = self
            .audio_processor
            .process_audio(audio_data, 16000)
            .context("Failed to process audio chunk")?;

        debug!(
            "Audio processing complete, processed {} samples",
            processed_audio.len()
        );

        let model_clone = Arc::clone(&self.model);

        // Check if online to choose async vs blocking path
        let is_online = {
            let guard = model_clone.read().await;
            guard
                .as_ref()
                .is_some_and(|loaded| loaded.instance.is_online())
        };

        let language = self.resolve_active_language().await;

        if is_online {
            let mut model_guard = model_clone.write().await;
            if let Some(loaded) = model_guard.as_mut() {
                match loaded
                    .instance
                    .transcribe_audio(&processed_audio, 16000, language.as_deref())
                    .await
                {
                    Ok(text) => Ok(text),
                    Err(e) => {
                        warn!("Online preview transcription failed, continuing: {e}");
                        Ok(String::new())
                    }
                }
            } else {
                warn!("Model not loaded for preview transcription");
                Ok(String::new())
            }
        } else {
            let result = tokio::task::spawn_blocking(move || {
                let handle = tokio::runtime::Handle::current();
                let mut model_guard = model_clone.blocking_write();
                if let Some(loaded) = model_guard.as_mut() {
                    match handle.block_on(loaded.instance.transcribe_audio(
                        &processed_audio,
                        16000,
                        language.as_deref(),
                    )) {
                        Ok(text) => text,
                        Err(e) => {
                            warn!("Preview transcription failed, continuing: {e}");
                            String::new()
                        }
                    }
                } else {
                    warn!("Model not loaded for preview transcription");
                    String::new()
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("Preview transcription task failed: {e}"))?;

            Ok(result)
        }
    }

    /// Transcribe audio with spinner if needed
    pub(super) async fn transcribe_with_spinner(
        &self,
        _typer: &mut Typer,
        audio_data: &[f32],
        _write_mode: bool,
    ) -> Result<String> {
        // If we'll type the result, show a simple spinner by typing characters and backspacing
        // This indicates work while transcription runs.
        let mut spinner_handle: Option<tokio::task::JoinHandle<()>> = None;
        let spinner_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Track how many temporary spinner characters are visible (0-3)
        let _visible_temp_chars = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Disable loader for now since it interferes with keyboard
        // TODO: Implement proper loader that doesn't conflict with final typing

        // Process audio
        let processed_audio = self
            .audio_processor
            .process_audio(audio_data, 16000)
            .context("Failed to process audio")?;

        // Transcribe the audio
        let model_clone = Arc::clone(&self.model);
        let is_online = {
            let guard = model_clone.read().await;
            guard
                .as_ref()
                .is_some_and(|loaded| loaded.instance.is_online())
        };

        let language = self.resolve_active_language().await;

        let transcription_result = if is_online {
            let start_time = std::time::Instant::now();
            let mut model_guard = model_clone.write().await;
            if let Some(loaded) = model_guard.as_mut() {
                match loaded
                    .instance
                    .transcribe_audio(&processed_audio, 16000, language.as_deref())
                    .await
                {
                    Ok(text) => {
                        let duration = start_time.elapsed();
                        info!("Online transcription completed in {duration:?}: '{text}'");
                        Ok(text)
                    }
                    Err(e) => {
                        // Surface the backend's error to the user instead of
                        // silently producing an empty "no speech" result.
                        warn!("Online transcription failed: {e}");
                        Err(e)
                    }
                }
            } else {
                error!("Model not loaded");
                Err(anyhow::anyhow!("Model not loaded"))
            }
        } else {
            tokio::task::spawn_blocking(move || {
                let handle = tokio::runtime::Handle::current();
                let start_time = std::time::Instant::now();
                let mut model_guard = model_clone.blocking_write();
                if let Some(loaded) = model_guard.as_mut() {
                    match handle.block_on(loaded.instance.transcribe_audio(
                        &processed_audio,
                        16000,
                        language.as_deref(),
                    )) {
                        Ok(text) => {
                            let duration = start_time.elapsed();
                            info!("Transcription completed in {duration:?}: '{text}'");
                            Ok(text)
                        }
                        Err(e) => {
                            warn!("Transcription failed: {e}");
                            Err(e)
                        }
                    }
                } else {
                    error!("Model not loaded");
                    Err(anyhow::anyhow!("Model not loaded"))
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("Transcription task failed: {e}"))?
        };

        // Stop spinner if it was started
        if let Some(handle) = spinner_handle.take() {
            spinner_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            // Wait for the spinner task to exit and clean up
            if let Err(e) = handle.await {
                warn!("Spinner task panicked: {e}");
            }
        }

        transcription_result
    }
}
