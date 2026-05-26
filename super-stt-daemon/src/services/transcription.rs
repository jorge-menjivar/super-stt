// SPDX-License-Identifier: GPL-3.0-only
use anyhow::Result;
use log::{debug, error, info, warn};
use rubato::{FastFixedIn, Resampler};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;

use crate::daemon::types::SharedLoadedModel;
use crate::input::audio::AudioProcessor;

use std::collections::VecDeque;

pub struct RealTimeSession {
    pub client_id: String,
    pub buffered_pcm: Vec<f32>,
    pub resampler: FastFixedIn<f32>,
    pub input_sample_rate: u32,
    pub language: Option<String>,
    pub language_token_set: bool,
    pub tx: broadcast::Sender<String>,
    pub decoding: bool,
    pub cancellation_token: CancellationToken,
    pub last_emit: Option<std::time::Instant>,
    pub silence_start: Option<std::time::Instant>,
    pub model_min_interval: Duration,
    // Speech activity detection
    pub speech_buffer: VecDeque<bool>,
    pub recent_levels: VecDeque<f32>,
    pub baseline_level: f32,
    pub speech_threshold: f32,
}

impl RealTimeSession {
    /// Create a new real-time transcription session
    ///
    /// # Errors
    ///
    /// Returns an error if the resampler cannot be constructed.
    pub fn new(
        client_id: String,
        input_sample_rate: u32,
        language: Option<String>,
        model_min_interval: Duration,
    ) -> Result<Self> {
        let resample_ratio = 16000.0 / f64::from(input_sample_rate);
        let resampler = FastFixedIn::new(
            resample_ratio,
            10.0,
            rubato::PolynomialDegree::Septic,
            1024,
            1,
        )?;

        let (tx, _) = broadcast::channel(100);

        Ok(Self {
            client_id,
            buffered_pcm: Vec::new(),
            resampler,
            input_sample_rate,
            language,
            language_token_set: false,
            tx,
            decoding: false,
            cancellation_token: CancellationToken::new(),
            last_emit: None,
            silence_start: None,
            model_min_interval,
            // Initialize speech activity detection
            speech_buffer: VecDeque::with_capacity(5),
            recent_levels: VecDeque::with_capacity(200),
            baseline_level: 0.005,  // Initial baseline
            speech_threshold: 0.01, // Initial speech threshold
        })
    }

    ///
    /// # Errors
    ///
    /// Currently infallible, returns `Ok(())`.
    pub fn add_audio_chunk(&mut self, audio_data: &[f32]) -> Result<()> {
        self.buffered_pcm.extend_from_slice(audio_data);

        // Calculate RMS energy for this chunk
        #[allow(clippy::cast_precision_loss)]
        let rms: f32 =
            (audio_data.iter().map(|&x| x * x).sum::<f32>() / audio_data.len() as f32).sqrt();

        // Update recent levels for adaptive thresholding
        if self.recent_levels.len() >= 200 {
            self.recent_levels.pop_front();
        }
        self.recent_levels.push_back(rms);

        // Update baseline (background noise level) from quieter samples
        if rms < self.baseline_level * 1.5 {
            self.baseline_level = (self.baseline_level * 0.95) + (rms * 0.05);
        }

        // Speech threshold is adaptive based on baseline + margin
        self.speech_threshold = (self.baseline_level * 2.0).clamp(0.008, 0.025);

        // Add speech decision to buffer
        let is_speech = rms > self.speech_threshold;
        if self.speech_buffer.len() >= 5 {
            self.speech_buffer.pop_front();
        }
        self.speech_buffer.push_back(is_speech);

        // Determine if we're currently in speech (majority of recent samples)
        let speech_votes = self.speech_buffer.iter().filter(|&&x| x).count();
        let currently_speaking = speech_votes >= 2; // At least 2 out of 5 recent samples

        if currently_speaking {
            // Reset silence timer - user is speaking
            self.silence_start = None;
        } else if self.silence_start.is_none() {
            // Start silence timer - user paused speaking
            self.silence_start = Some(std::time::Instant::now());
        }

        Ok(())
    }

    /// Get a resampled processing window for preview decoding
    ///
    /// # Errors
    ///
    /// Returns an error if resampling fails.
    pub fn get_processing_window(&mut self) -> Result<Option<Vec<f32>>> {
        // Adaptive processing - no fixed intervals, just process when previous is done

        // Minimal latency for first preview
        let min_first_seconds = 2usize; // start guessing after ~2s of audio
        let min_first = min_first_seconds * self.input_sample_rate as usize;
        if self.buffered_pcm.len() < min_first {
            return Ok(None);
        }

        // Use larger window to maintain context - don't truncate during active speech
        let window_seconds = 15usize; // 15s max to preserve speech context
        let window_input_len =
            (window_seconds * self.input_sample_rate as usize).min(self.buffered_pcm.len());
        let start = self.buffered_pcm.len() - window_input_len;
        let slice = &self.buffered_pcm[start..];

        // Resample this tail slice
        let mut resampled_pcm = Vec::new();
        let full_chunks = slice.len() / 1024;
        for chunk in 0..full_chunks {
            let seg = &slice[chunk * 1024..(chunk + 1) * 1024];
            let pcm = self.resampler.process(&[seg], None)?;
            resampled_pcm.extend_from_slice(&pcm[0]);
        }
        // Do not process the remainder (<1024). Keep it for the next cycle to avoid rubato errors.

        // Keep a larger buffer during speech to maintain context (30s)
        let keep_size = (30 * self.input_sample_rate as usize).min(self.buffered_pcm.len());
        if self.buffered_pcm.len() > keep_size {
            let buffer_len = self.buffered_pcm.len();
            self.buffered_pcm.copy_within(buffer_len - keep_size.., 0);
            self.buffered_pcm.truncate(keep_size);
        }

        // Update emit time for throttling
        self.last_emit = Some(std::time::Instant::now());
        Ok(Some(resampled_pcm))
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// Send a transcription string to session subscribers
    ///
    /// # Errors
    ///
    /// Currently infallible, returns `Ok(())`.
    pub fn send_transcription(&self, text: String) -> Result<()> {
        let _ = self.tx.send(text);
        Ok(())
    }
}

pub struct RealTimeTranscriptionManager {
    sessions: Arc<RwLock<HashMap<String, RealTimeSession>>>,
    model: SharedLoadedModel,
    audio_processor: Arc<AudioProcessor>,
}

impl RealTimeTranscriptionManager {
    /// Construct a new real-time transcription manager
    pub fn new(model: SharedLoadedModel, audio_processor: Arc<AudioProcessor>) -> Self {
        let manager = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            model,
            audio_processor,
        };

        // Start processing task
        let sessions_clone = Arc::clone(&manager.sessions);
        let model_clone = Arc::clone(&manager.model);
        let processor_clone = Arc::clone(&manager.audio_processor);

        tokio::spawn(async move {
            let mut processing_interval = interval(Duration::from_millis(200)); // Check every 200ms for more responsiveness
            loop {
                processing_interval.tick().await;
                if let Err(e) =
                    Self::process_all_sessions(&sessions_clone, &model_clone, &processor_clone)
                        .await
                {
                    error!("Error processing real-time sessions: {e}");
                }
            }
        });

        manager
    }

    /// Start a real-time session and return a subscription receiver
    ///
    /// # Errors
    ///
    /// Returns an error if session creation or resampler setup fails.
    pub async fn start_session(
        &self,
        client_id: String,
        sample_rate: Option<u32>,
        language: Option<String>,
    ) -> Result<broadcast::Receiver<String>> {
        let sample_rate = sample_rate.unwrap_or(16000);

        // Get processing interval from the loaded model's definition
        let min_interval = {
            let guard = self.model.read().await;
            guard.as_ref().map_or(Duration::from_secs(1), |loaded| {
                loaded.definition.processing_interval
            })
        };

        let session = RealTimeSession::new(client_id.clone(), sample_rate, language, min_interval)?;
        let receiver = session.subscribe();

        let mut sessions = self.sessions.write().await;
        sessions.insert(client_id.clone(), session);

        info!("Started real-time transcription session for client: {client_id}");

        Ok(receiver)
    }

    /// Ingest an audio chunk for a given client
    ///
    /// # Errors
    ///
    /// Returns an error if processing fails.
    pub async fn process_audio_chunk(
        &self,
        client_id: &str,
        audio_data: Vec<f32>,
        sample_rate: u32,
    ) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(client_id) {
            if session.input_sample_rate != sample_rate {
                warn!(
                    "Sample rate mismatch for client {}: expected {}, got {}",
                    client_id, session.input_sample_rate, sample_rate
                );
            }

            // Add the audio chunk to the session buffer
            session.add_audio_chunk(&audio_data)?;
        }

        Ok(())
    }

    async fn process_all_sessions(
        sessions: &Arc<RwLock<HashMap<String, RealTimeSession>>>,
        model: &SharedLoadedModel,
        audio_processor: &Arc<AudioProcessor>,
    ) -> Result<()> {
        // Collect clients that have audio ready for processing
        let mut ready_clients: Vec<(String, Vec<f32>, CancellationToken)> = Vec::new();

        {
            let mut sessions_write = sessions.write().await;
            for (client_id, session) in sessions_write.iter_mut() {
                if session.decoding {
                    continue; // skip if a decode is already running
                }
                if let Some(resampled_audio) = session.get_processing_window()? {
                    session.decoding = true; // mark in-progress
                    ready_clients.push((
                        client_id.clone(),
                        resampled_audio,
                        session.cancellation_token.clone(),
                    ));
                }
            }
        }

        // Process each ready client in parallel
        for (client_id, resampled_audio, cancellation_token) in ready_clients {
            let model_clone = Arc::clone(model);
            let proc_clone = Arc::clone(audio_processor);
            let sessions_clone = Arc::clone(sessions);

            tokio::spawn(async move {
                tokio::select! {
                    result = Self::transcribe_audio_chunk(
                        &client_id,
                        resampled_audio,
                        &model_clone,
                        &proc_clone,
                        &sessions_clone,
                    ) => {
                        if let Err(e) = result {
                            error!("Error transcribing audio for client {client_id}: {e}");
                        }
                    }
                    () = cancellation_token.cancelled() => {
                        debug!("Transcription cancelled for client: {client_id}");
                    }
                }

                // Always clear decoding flag regardless of cancellation or completion
                let mut sessions_w = sessions_clone.write().await;
                if let Some(s) = sessions_w.get_mut(&client_id) {
                    s.decoding = false;
                }
            });
        }

        Ok(())
    }

    async fn transcribe_audio_chunk(
        client_id: &str,
        audio_data: Vec<f32>,
        model: &SharedLoadedModel,
        audio_processor: &Arc<AudioProcessor>,
        sessions: &Arc<RwLock<HashMap<String, RealTimeSession>>>,
    ) -> Result<()> {
        // Prepare and submit audio to model
        let resampled_len = audio_data.len();
        let processed = audio_processor.process_audio(&audio_data, 16000)?;

        // Check if online model to choose async vs blocking path
        let is_online = {
            let guard = model.read().await;
            guard
                .as_ref()
                .is_some_and(|loaded| loaded.instance.is_online())
        };

        let transcription_result = if is_online {
            let mut model_guard = model.write().await;
            if let Some(loaded) = model_guard.as_mut() {
                Ok(loaded.instance.transcribe_audio(&processed, 16000).await)
            } else {
                Ok(Err(anyhow::anyhow!("Model not loaded")))
            }
        } else {
            tokio::task::spawn_blocking({
                let model_clone = Arc::clone(model);
                let audio = processed;
                move || {
                    let handle = tokio::runtime::Handle::current();
                    let mut model_guard = model_clone.blocking_write();
                    if let Some(loaded) = model_guard.as_mut() {
                        handle.block_on(loaded.instance.transcribe_audio(&audio, 16000))
                    } else {
                        Err(anyhow::anyhow!("Model not loaded"))
                    }
                }
            })
            .await
        };

        match transcription_result {
            Ok(Ok(transcription)) => {
                if transcription.trim().is_empty() {
                    info!(
                        "Real-time preview produced empty transcription for {client_id} (resampled_len={resampled_len})"
                    );
                } else {
                    info!(
                        "Real-time preview transcription ({} chars): '{}'",
                        transcription.chars().count(),
                        transcription.chars().take(60).collect::<String>()
                    );
                    // Send to session subscribers
                    let sessions_read = sessions.read().await;
                    if let Some(session) = sessions_read.get(client_id) {
                        let _ = session.send_transcription(transcription.clone());
                    }

                    debug!("Real-time transcription for {}: {}", client_id, "<omitted>");
                }
            }
            Ok(Err(e)) => {
                warn!("Transcription error for client {client_id}: {e}");
            }
            Err(e) => {
                error!("Task error for client {client_id}: {e}");
            }
        }

        Ok(())
    }

    pub async fn get_active_sessions(&self) -> Vec<String> {
        let sessions = self.sessions.read().await;
        sessions.keys().cloned().collect()
    }
}
