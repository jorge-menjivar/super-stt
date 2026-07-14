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
use crate::daemon::events::EventBus;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use log::info;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
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
    volume: f32,
    // Audio device initialization state
    audio_device_cache: Arc<Mutex<Option<AudioDeviceCache>>>,
}

/// Return type of [`DaemonAudioRecorder::build_audio_stream`].
type AudioStreamBundle = (
    Stream,
    tokio::task::JoinHandle<()>,
    cpal::StreamConfig,
    tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
);

/// All parameters needed to build a `cpal` input stream.
struct StreamSetup<'a> {
    device: &'a Device,
    config: &'a StreamConfig,
    sample_format: SampleFormat,
    buffer: Arc<Mutex<VecDeque<f32>>>,
    state: Arc<Mutex<RecordingState>>,
    level_tx: broadcast::Sender<AudioLevel>,
    samples_tx: tokio::sync::mpsc::UnboundedSender<Vec<f32>>,
}

impl DaemonAudioRecorder {
    /// Create a new recorder with default theme
    ///
    /// # Errors
    ///
    /// Returns an error if warm-up steps fail in a fatal way.
    pub fn new() -> Result<Self> {
        Self::new_with_theme(AudioTheme::default(), 1.0)
    }

    /// Create a new recorder with a specific theme and volume
    ///
    /// # Errors
    ///
    /// Returns an error if initialization of audio resources fails.
    pub fn new_with_theme(theme: AudioTheme, volume: f32) -> Result<Self> {
        let (audio_level_tx, _) = broadcast::channel(1000);

        let recorder = Self {
            sample_rate: 16000,
            audio_buffer: Arc::new(Mutex::new(VecDeque::new())),
            recording_state: Arc::new(Mutex::new(RecordingState::new())),
            audio_level_tx,
            audio_theme: theme,
            volume,
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
    pub async fn record_until_silence_with_streaming(
        &mut self,
        // Internal pub/sub bus that fans frequency-band frames out to
        // widget HTTP/SSE subscribers.
        events: Arc<EventBus>,
        // Optional channel to forward live mono PCM samples and device sample rate
        preview_tx: Option<tokio::sync::mpsc::UnboundedSender<(Vec<f32>, u32)>>,
        // When true, disables silence detection (recording stops only via stop signal)
        silence_detection_disabled: bool,
        // Optional external stop signal (shortcut stop or early stop)
        mut stop_rx: Option<tokio::sync::broadcast::Receiver<()>>,
    ) -> Result<Vec<f32>> {
        info!("🎤 Starting audio recording with streaming...");

        // Play start sound and wait for it to complete
        self.play_start_sound_and_wait().await;

        self.init_recording_state(silence_detection_disabled);

        let (stream, analysis_task, stream_config, samples_tx) =
            self.build_audio_stream(&events, preview_tx)?;

        let timeout_occurred = self
            .run_silence_loop(silence_detection_disabled, &mut stop_rx)
            .await;

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

        let final_audio = self.drain_and_resample(stream_config.sample_rate)?;

        log::info!("🎤 Recording completed: {} samples", final_audio.len());

        // Play end sound
        self.play_end_sound();

        Ok(final_audio)
    }

    /// Clear the audio buffer and reset recording state before a new recording.
    fn init_recording_state(&self, silence_detection_disabled: bool) {
        let mut buffer = self.audio_buffer.lock();
        buffer.clear();

        let mut state = self.recording_state.lock();
        *state = RecordingState::new();
        state.recording_start = Some(Instant::now());
        state.silence_detection_disabled = silence_detection_disabled;
    }

    /// Set up the audio device, spawn the frequency-analysis task, and build the
    /// cpal input stream.  Returns `(stream, analysis_task, stream_config,
    /// samples_tx)` so the caller can drive the lifetime of each piece.
    fn build_audio_stream(
        &self,
        events: &Arc<EventBus>,
        preview_tx: Option<tokio::sync::mpsc::UnboundedSender<(Vec<f32>, u32)>>,
    ) -> Result<AudioStreamBundle> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;

        let config = Self::get_optimal_config(&device)?;
        let sample_format = config.sample_format();
        let stream_config = config.config();

        // Create channel for sending audio samples from callback to async task for frequency analysis
        let (samples_tx, mut samples_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();

        // Start frequency analysis and broadcasting task. Publishes
        // each set of frequency bands to the widget HTTP/SSE bus
        // unconditionally — publishes with no subscribers are a cheap
        // `send` error that's dropped silently, and computing bands at
        // the cpal callback rate is cheap relative to the STT model.
        let events_clone = Arc::clone(events);
        let device_sample_rate_u32 = stream_config.sample_rate;
        // sample_rate is u32 (cpal::SampleRate = u32); sample rates ≤ 384_000 round-trip
        // exactly in f32.
        let device_sample_rate = crate::num_cast::u32_to_f32(device_sample_rate_u32);
        let analysis_task = tokio::spawn(async move {
            let frequency_analyzer = AudioAnalyzer::new(device_sample_rate_u32, 1024);

            while let Some(samples) = samples_rx.recv().await {
                let freq_data = frequency_analyzer.analyze(&samples);
                events_clone.publish_frequency_bands(
                    &freq_data.bands,
                    device_sample_rate,
                    freq_data.total_energy,
                );

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

        let stream = Self::create_audio_stream_with_streaming(StreamSetup {
            device: &device,
            config: &stream_config,
            sample_format,
            buffer: buffer_clone,
            state: state_clone,
            level_tx,
            samples_tx: samples_tx.clone(),
        })?;

        Ok((stream, analysis_task, stream_config, samples_tx))
    }

    /// Poll the recording state until silence / stop signal / timeout.
    /// Returns `true` if a timeout occurred (no speech within 60 s).
    async fn run_silence_loop(
        &self,
        silence_detection_disabled: bool,
        stop_rx: &mut Option<tokio::sync::broadcast::Receiver<()>>,
    ) -> bool {
        let start_time = Instant::now();

        loop {
            // When a stop signal is present, race between the periodic check and the stop signal
            if let Some(rx) = stop_rx {
                tokio::select! {
                    () = time::sleep(AUDIO_LOOP_INTERVAL) => {}
                    _ = rx.recv() => {
                        info!("🛑 Stop signal received, ending recording");
                        break;
                    }
                }
            } else {
                time::sleep(AUDIO_LOOP_INTERVAL).await;
            }

            let should_stop = self.recording_state.lock().should_stop();

            if should_stop {
                break;
            }

            // Intelligent timeout logic - only timeout if no speech has been detected
            // (not applicable when silence detection is disabled)
            if !silence_detection_disabled {
                let elapsed = start_time.elapsed();
                // Check if speech has been detected and recording started.
                let has_detected_speech = self.recording_state.lock().recording;

                // If speech has been detected, rely on silence detection instead of timeout
                // Only timeout if no speech has been detected at all
                if !has_detected_speech && elapsed >= Duration::from_mins(1) {
                    log::warn!("⚠️ Recording timeout: No speech detected within 60 seconds");
                    return true;
                }
            }
        }

        false
    }

    /// Extract buffered audio and resample to the target rate when needed.
    fn drain_and_resample(&self, device_sample_rate: u32) -> Result<Vec<f32>> {
        let audio_data: Vec<f32> = self.audio_buffer.lock().iter().copied().collect();

        if audio_data.is_empty() {
            return Err(anyhow::anyhow!("No audio recorded"));
        }

        if device_sample_rate == self.sample_rate {
            Ok(audio_data)
        } else {
            resample(
                &audio_data,
                device_sample_rate,
                self.sample_rate,
                ResampleQuality::Fast,
            )
        }
    }

    fn get_optimal_config(device: &Device) -> Result<cpal::SupportedStreamConfig> {
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
                let max_rate = config.max_sample_rate();
                let min_rate = config.min_sample_rate();
                // Look for configs that support common sample rates
                min_rate <= 48000 && max_rate >= 16000
            })
            .copied()
            .or_else(|| supported_configs.into_iter().next())
            .context("No supported input config")?;

        // Use a reasonable sample rate instead of max
        let target_rate = if optimal_config.max_sample_rate() >= 48000 {
            48000
        } else if optimal_config.max_sample_rate() >= 44100 {
            44100
        } else if optimal_config.max_sample_rate() >= 16000 {
            16000
        } else {
            optimal_config.max_sample_rate()
        };

        Ok(optimal_config.with_sample_rate(target_rate))
    }

    /// Play start recording sound using current theme and wait for it to
    /// complete. `play_beep_sequence` spin-waits for the sound's full duration,
    /// so run it off the async runtime (Tier 3 #4).
    async fn play_start_sound_and_wait(&self) {
        if self.audio_theme == AudioTheme::Silent {
            return;
        }
        let (frequencies, duration, fade_in, fade_out) = self.audio_theme.start_sound();
        if let Err(e) =
            beeper::play_beep_sequence_async(frequencies, duration, fade_in, fade_out, self.volume)
                .await
        {
            log::warn!("Failed to play start sound (audio permissions may be missing): {e}");
        }
    }

    /// Play end recording sound using current theme
    fn play_end_sound(&self) {
        if self.audio_theme == AudioTheme::Silent {
            return;
        }
        let (frequencies, duration, fade_in, fade_out) = self.audio_theme.end_sound();
        let volume = self.volume;
        std::thread::spawn(move || {
            if let Err(e) =
                beeper::play_beep_sequence(&frequencies, duration, fade_in, fade_out, volume)
            {
                log::warn!("Failed to play end sound (audio permissions may be missing): {e}");
            }
        });
    }

    fn create_audio_stream_with_streaming(setup: StreamSetup<'_>) -> Result<Stream> {
        let channels = setup.config.channels as usize;

        match setup.sample_format {
            SampleFormat::F32 => {
                let StreamSetup {
                    device,
                    config,
                    buffer,
                    state,
                    level_tx,
                    samples_tx,
                    ..
                } = setup;
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
                let StreamSetup {
                    device,
                    config,
                    buffer,
                    state,
                    level_tx,
                    samples_tx,
                    ..
                } = setup;
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
                "Unsupported sample format: {:?}",
                setup.sample_format
            )),
        }
    }

    /// Detect the default input device's chosen sample rate using the same logic
    /// as the recording stream setup, so callers can preconfigure dependencies
    /// (e.g., real-time preview) with the correct rate.
    ///
    /// # Errors
    ///
    /// Returns an error if no input device/config is available.
    pub fn detect_default_input_sample_rate(&self) -> Result<u32> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;
        let config = Self::get_optimal_config(&device)?;
        Ok(config.config().sample_rate)
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
        let audio_data: Vec<f32> = self.audio_buffer.lock().iter().copied().collect();
        Ok(audio_data)
    }

    /// Check if the recorder is still actively recording
    /// This checks the internal recording state
    #[must_use]
    pub fn is_still_recording(&self) -> bool {
        !self.recording_state.lock().should_stop()
    }

    /// Get a reference to the internal audio buffer for direct access during recording
    /// This allows preview functionality to access the buffer without blocking the recording thread
    #[must_use]
    pub fn get_audio_buffer_ref(&self) -> Arc<Mutex<VecDeque<f32>>> {
        Arc::clone(&self.audio_buffer)
    }
}
