// SPDX-License-Identifier: GPL-3.0-only

use crate::audio::beeper;
use crate::audio::device::{
    AudioDeviceCache, AudioHealthStatus, get_or_initialize_audio_device,
    verify_audio_device_readiness,
};
use crate::audio::processing::{
    process_audio_data_f32_with_streaming, process_audio_data_i16_with_streaming,
};
use crate::audio::state::RecordingState;
use crate::audio::streamer::UdpAudioStreamer;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use log::info;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use super_stt_shared::AudioAnalyzer;
use super_stt_shared::audio_utils::ResampleQuality;
use super_stt_shared::models::audio::AudioLevel;
use super_stt_shared::theme::AudioTheme;
use super_stt_shared::utils::audio::resample;
use tokio::sync::broadcast;
use tokio::time;

// Audio processing loop interval
const AUDIO_LOOP_INTERVAL: Duration = Duration::from_millis(100);

pub struct DaemonAudioRecorder {
    pub sample_rate: u32,
    audio_buffer: Arc<Mutex<VecDeque<f32>>>,
    recording_state: Arc<Mutex<RecordingState>>,
    pub audio_level_tx: broadcast::Sender<AudioLevel>,
    audio_theme: AudioTheme,
    // Audio device initialization state
    audio_device_cache: Arc<Mutex<Option<AudioDeviceCache>>>,
}

impl DaemonAudioRecorder {
    /// Create a new recorder with default theme
    ///
    /// # Errors
    ///
    /// Returns an error if warm-up steps fail in a fatal way.
    pub fn new() -> Result<Self> {
        Self::new_with_theme(AudioTheme::default())
    }

    /// Create a new recorder with a specific theme
    ///
    /// # Errors
    ///
    /// Returns an error if initialization of audio resources fails.
    pub fn new_with_theme(theme: AudioTheme) -> Result<Self> {
        let (audio_level_tx, _) = broadcast::channel(1000);

        let recorder = Self {
            sample_rate: 16000,
            audio_buffer: Arc::new(Mutex::new(VecDeque::new())),
            recording_state: Arc::new(Mutex::new(RecordingState::new())),
            audio_level_tx,
            audio_theme: theme,
            audio_device_cache: Arc::new(Mutex::new(None)),
        };

        // Pre-warm audio system to prevent cold start issues
        if let Err(e) = recorder.warm_up_audio_system() {
            log::warn!("Failed to warm up audio system: {e}. Audio may have initial delay.");
        }

        Ok(recorder)
    }

    /// Change the audio theme
    pub fn set_theme(&mut self, theme: AudioTheme) {
        self.audio_theme = theme;
    }

    /// Get current audio theme
    #[must_use]
    pub fn theme(&self) -> AudioTheme {
        self.audio_theme
    }

    /// Comprehensive audio system health check
    /// This verifies both input and output audio systems are functional
    /// Perform a health check on the audio system
    ///
    /// # Errors
    ///
    /// Returns an error if device initialization or readiness checks fail.
    pub fn perform_audio_health_check(&self) -> Result<AudioHealthStatus> {
        crate::audio::device::perform_audio_health_check(&self.audio_device_cache)
    }

    /// Subscribe to audio level updates
    #[must_use]
    pub fn subscribe_audio_levels(&self) -> broadcast::Receiver<AudioLevel> {
        self.audio_level_tx.subscribe()
    }

    /// Warm up the audio system to prevent cold start issues
    fn warm_up_audio_system(&self) -> Result<()> {
        if self.audio_theme == AudioTheme::Silent {
            log::info!("Skipping audio system warm-up for Silent theme");
            return Ok(());
        }
        log::info!("Warming up audio system for reliable beep playback...");
        let device_cache = get_or_initialize_audio_device(&self.audio_device_cache)?;
        verify_audio_device_readiness(&self.audio_device_cache, &device_cache)?;
        log::info!("Audio system warm-up completed successfully");
        Ok(())
    }

    /// Record until silence with UDP streaming of audio samples
    ///
    /// # Errors
    ///
    /// Returns an error if device setup, recording, or resampling fails.
    ///
    /// # Panics
    ///
    /// Panics if internal mutexes for buffers or state are poisoned.
    #[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
    pub async fn record_until_silence_with_streaming(
        &mut self,
        udp_streamer: Arc<UdpAudioStreamer>,
        // Optional channel to forward live mono PCM samples and device sample rate
        preview_tx: Option<tokio::sync::mpsc::UnboundedSender<(Vec<f32>, u32)>>,
        // When true, disables silence detection (recording stops only via stop signal)
        silence_detection_disabled: bool,
        // Optional external stop signal (shortcut stop or early stop)
        mut stop_rx: Option<tokio::sync::broadcast::Receiver<()>>,
    ) -> Result<Vec<f32>> {
        info!("🎤 Starting audio recording with streaming...");

        // Play start sound and wait for it to complete
        self.play_start_sound_and_wait();

        // Clear previous recording
        {
            let mut buffer = match self.audio_buffer.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    log::warn!("Audio buffer lock was poisoned, attempting recovery");
                    poisoned.into_inner()
                }
            };
            buffer.clear();

            let mut state = match self.recording_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    log::warn!("Recording state lock was poisoned, attempting recovery");
                    poisoned.into_inner()
                }
            };
            *state = RecordingState::new();
            state.recording_start = Some(Instant::now());
            state.silence_detection_disabled = silence_detection_disabled;
        }

        // Set up audio stream
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;

        let config = self.get_optimal_config(&device)?;
        let sample_format = config.sample_format();
        let stream_config = config.config();

        // Create channel for sending audio samples from callback to async task for frequency analysis
        let (samples_tx, mut samples_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();

        // Start frequency analysis and broadcasting task (only when clients are listening)
        let udp_streamer_clone = Arc::clone(&udp_streamer);
        let device_sample_rate_u32 = stream_config.sample_rate.0;
        let device_sample_rate = device_sample_rate_u32 as f32;
        let analysis_task = tokio::spawn(async move {
            let frequency_analyzer = AudioAnalyzer::new(device_sample_rate, 1024);

            while let Some(samples) = samples_rx.recv().await {
                // Only compute frequency bands if there are clients listening
                if udp_streamer_clone.has_clients().await {
                    let freq_data = frequency_analyzer.analyze(&samples);
                    let frequency_bands = freq_data.bands;
                    let total_energy = freq_data.total_energy;

                    if let Err(e) = udp_streamer_clone
                        .broadcast_frequency_bands(
                            &frequency_bands,
                            device_sample_rate,
                            total_energy,
                            0, // daemon client ID
                        )
                        .await
                    {
                        log::warn!("Failed to broadcast frequency bands: {e}");
                    }
                }

                // Forward to real-time preview if requested
                if let Some(ref tx) = preview_tx {
                    // Ignore if receiver is dropped
                    let _ = tx.send((samples.clone(), device_sample_rate_u32));
                }
            }
        });

        // Create audio stream with UDP streaming
        let buffer_clone = self.audio_buffer.clone();
        let state_clone = self.recording_state.clone();
        let level_tx = self.audio_level_tx.clone();

        let stream = self.create_audio_stream_with_streaming(
            &device,
            &stream_config,
            sample_format,
            buffer_clone,
            state_clone,
            level_tx,
            samples_tx.clone(),
        )?;

        // Wait for recording to complete with intelligent timeout
        let start_time = Instant::now();
        let mut timeout_occurred = false;

        loop {
            // When a stop signal is present, race between the periodic check and the stop signal
            if let Some(ref mut stop_rx) = stop_rx {
                tokio::select! {
                    () = time::sleep(AUDIO_LOOP_INTERVAL) => {}
                    _ = stop_rx.recv() => {
                        info!("🛑 Stop signal received, ending recording");
                        break;
                    }
                }
            } else {
                time::sleep(AUDIO_LOOP_INTERVAL).await;
            }

            let should_stop = {
                let state = match self.recording_state.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => {
                        log::warn!(
                            "Recording state lock was poisoned during recording, attempting recovery"
                        );
                        poisoned.into_inner()
                    }
                };
                state.should_stop()
            };

            if should_stop {
                break;
            }

            // Intelligent timeout logic - only timeout if no speech has been detected
            // (not applicable when silence detection is disabled)
            if !silence_detection_disabled {
                let elapsed = start_time.elapsed();
                let has_detected_speech = {
                    let state = match self.recording_state.lock() {
                        Ok(guard) => guard,
                        Err(poisoned) => {
                            log::warn!(
                                "Recording state lock was poisoned while checking speech detection, attempting recovery"
                            );
                            poisoned.into_inner()
                        }
                    };
                    state.recording // Check if speech has been detected and recording started
                };

                // If speech has been detected, rely on silence detection instead of timeout
                // Only timeout if no speech has been detected at all
                if !has_detected_speech && elapsed >= Duration::from_secs(60) {
                    log::warn!("⚠️ Recording timeout: No speech detected within 60 seconds");
                    timeout_occurred = true;
                    break;
                }
            }
        }

        drop(stream);

        // Close the samples channel to stop the analysis task
        drop(samples_tx);

        // Wait for analysis task to finish
        let _ = analysis_task.await;

        // Check if timeout occurred
        if timeout_occurred {
            return Err(anyhow::anyhow!(
                "Timeout: No speech detected within 60 seconds"
            ));
        }

        // Extract recorded audio
        let audio_data: Vec<f32> = {
            let buffer = match self.audio_buffer.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    log::warn!(
                        "Audio buffer lock was poisoned during extraction, attempting recovery"
                    );
                    poisoned.into_inner()
                }
            };
            buffer.iter().copied().collect()
        };

        if audio_data.is_empty() {
            return Err(anyhow::anyhow!("No audio recorded"));
        }

        // Resample if needed
        let device_sample_rate = stream_config.sample_rate.0;
        let final_audio = if device_sample_rate == self.sample_rate {
            audio_data
        } else {
            resample(
                &audio_data,
                device_sample_rate,
                self.sample_rate,
                ResampleQuality::Fast,
            )?
        };

        log::info!("🎤 Recording completed: {} samples", final_audio.len());

        // Play end sound
        self.play_end_sound();

        Ok(final_audio)
    }

    #[allow(clippy::unused_self)]
    fn get_optimal_config(&self, device: &Device) -> Result<cpal::SupportedStreamConfig> {
        let mut supported_configs: Vec<_> = device.supported_input_configs()?.collect();

        // Sort by preference: F32, I16, I32, others
        supported_configs.sort_by_key(|config| match config.sample_format() {
            SampleFormat::F32 => 0,
            SampleFormat::I16 => 1,
            SampleFormat::I32 => 2,
            SampleFormat::F64 => 3,
            _ => 4,
        });

        // Find a config with reasonable sample rate (prefer 16kHz-48kHz range)
        let optimal_config = supported_configs
            .iter()
            .find(|config| {
                let max_rate = config.max_sample_rate().0;
                let min_rate = config.min_sample_rate().0;
                // Look for configs that support common sample rates
                min_rate <= 48000 && max_rate >= 16000
            })
            .copied()
            .or_else(|| supported_configs.into_iter().next())
            .context("No supported input config")?;

        // Use a reasonable sample rate instead of max
        let target_rate = if optimal_config.max_sample_rate().0 >= 48000 {
            cpal::SampleRate(48000)
        } else if optimal_config.max_sample_rate().0 >= 44100 {
            cpal::SampleRate(44100)
        } else if optimal_config.max_sample_rate().0 >= 16000 {
            cpal::SampleRate(16000)
        } else {
            optimal_config.max_sample_rate()
        };

        optimal_config.with_sample_rate(target_rate).pipe(Ok)
    }

    /// Play start recording sound using current theme and wait for it to complete
    fn play_start_sound_and_wait(&self) {
        if self.audio_theme == AudioTheme::Silent {
            return;
        }
        let (frequencies, duration, fade_in, fade_out) = self.audio_theme.start_sound();
        if let Err(e) = beeper::play_beep_sequence(&frequencies, duration, fade_in, fade_out) {
            log::warn!("Failed to play start sound (audio permissions may be missing): {e}");
        }
    }

    /// Play end recording sound using current theme
    fn play_end_sound(&self) {
        if self.audio_theme == AudioTheme::Silent {
            return;
        }
        let (frequencies, duration, fade_in, fade_out) = self.audio_theme.end_sound();
        std::thread::spawn(move || {
            if let Err(e) = beeper::play_beep_sequence(&frequencies, duration, fade_in, fade_out) {
                log::warn!("Failed to play end sound (audio permissions may be missing): {e}");
            }
        });
    }

    #[allow(clippy::too_many_arguments, clippy::unused_self)]
    fn create_audio_stream_with_streaming(
        &self,
        device: &Device,
        config: &StreamConfig,
        sample_format: SampleFormat,
        buffer: Arc<Mutex<VecDeque<f32>>>,
        state: Arc<Mutex<RecordingState>>,
        level_tx: broadcast::Sender<AudioLevel>,
        samples_tx: tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
    ) -> Result<Stream> {
        let channels = config.channels as usize;

        match sample_format {
            SampleFormat::F32 => {
                let stream = device.build_input_stream(
                    config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        process_audio_data_f32_with_streaming(
                            data,
                            channels,
                            &buffer,
                            &state,
                            &level_tx,
                            &samples_tx,
                        );
                    },
                    |err| log::error!("Stream error: {err}"),
                    None,
                )?;
                Ok(stream)
            }
            SampleFormat::I16 => {
                let stream = device.build_input_stream(
                    config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        process_audio_data_i16_with_streaming(
                            data,
                            channels,
                            &buffer,
                            &state,
                            &level_tx,
                            &samples_tx,
                        );
                    },
                    |err| log::error!("Stream error: {err}"),
                    None,
                )?;
                Ok(stream)
            }
            _ => Err(anyhow::anyhow!(
                "Unsupported sample format: {sample_format:?}"
            )),
        }
    }
    /// Detect the default input device's chosen sample rate using the same logic
    /// as the recording stream setup, so callers can preconfigure dependencies
    /// (e.g., real-time preview) with the correct rate.
    /// Detect the default input device's sample rate using the optimal config.
    ///
    /// # Errors
    ///
    /// Returns an error if no input device/config is available.
    pub fn detect_default_input_sample_rate(&self) -> Result<u32> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;
        let config = self.get_optimal_config(&device)?;
        Ok(config.config().sample_rate.0)
    }

    /// Prepare recorder for threaded operation - initializes any threaded state
    pub fn prepare_for_threaded_recording(&mut self) {
        // Initialize any threaded state here if needed in the future
        // For now, the recorder is already set up for async operation
    }

    /// Get all recorded audio data - this should be called after recording is complete
    ///
    /// # Errors
    ///
    /// Returns an error if the audio buffer cannot be accessed
    pub fn get_full_audio_data(&self) -> Result<Vec<f32>> {
        let buffer = match self.audio_buffer.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                log::warn!(
                    "Audio buffer lock was poisoned during get_full_audio_data, attempting recovery"
                );
                poisoned.into_inner()
            }
        };

        let audio_data: Vec<f32> = buffer.iter().copied().collect();
        Ok(audio_data)
    }

    /// Check if the recorder is still actively recording
    /// This checks the internal recording state
    #[must_use]
    pub fn is_still_recording(&self) -> bool {
        match self.recording_state.lock() {
            Ok(state) => !state.should_stop(),
            Err(poisoned) => {
                log::warn!(
                    "Recording state lock was poisoned during is_still_recording check, attempting recovery"
                );
                let state = poisoned.into_inner();
                !state.should_stop()
            }
        }
    }

    /// Get a reference to the internal audio buffer for direct access during recording
    /// This allows preview functionality to access the buffer without blocking the recording thread
    #[must_use]
    pub fn get_audio_buffer_ref(&self) -> Arc<Mutex<VecDeque<f32>>> {
        Arc::clone(&self.audio_buffer)
    }
}
trait PipeExt<T> {
    fn pipe<F, U>(self, f: F) -> U
    where
        F: FnOnce(T) -> U;
}

impl<T> PipeExt<T> for T {
    fn pipe<F, U>(self, f: F) -> U
    where
        F: FnOnce(T) -> U,
    {
        f(self)
    }
}
