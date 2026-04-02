// SPDX-License-Identifier: GPL-3.0-only
use cosmic::{
    Element, Renderer, Theme,
    iced::{
        core::{Rectangle, mouse},
        widget::{
            Canvas,
            canvas::{Frame, Geometry, Program},
        },
    },
};

use crate::{
    config::{
        DEFAULT_VISUALIZATION_WAVE_FREQUENCY, FREQUENCY_CONFIDENCE_THRESHOLD, FREQUENCY_SMOOTHING,
        MAX_AUDIO_FREQUENCY, MAX_VISUALIZATION_WAVE_FREQUENCY, MIN_AUDIO_FREQUENCY,
        MIN_VISUALIZATION_WAVE_FREQUENCY,
    },
    models::theme::{VisualizationColorConfig, VisualizationSide, VisualizationTheme},
    ui::components::visualizations::{
        CenteredBarsVisualization, EqualizerVisualization, PulseVisualization,
        VisualizationRenderer, WaveformVisualization,
    },
};
use super_stt_shared::{AudioAnalyzer, FrequencyData};
// Sizing handled by parent container
use crate::app::Message;

#[derive(Debug, Clone)]
pub struct VisualizationComponent {
    audio_level: f32,
    is_speech_detected: bool,
    visualization_theme: VisualizationTheme,
    visualization_side: VisualizationSide,
    audio_analyzer: AudioAnalyzer,
    audio_samples: Vec<f32>, // Store recent audio samples for analysis
    frequency_data: FrequencyData,
    visualization_colors: VisualizationColorConfig,
    smoothed_visualization_frequency: f32, // Smoothed wave frequency for stable visualization
}

impl VisualizationComponent {
    pub fn new(
        audio_level: f32,
        is_speech_detected: bool,
        visualization_theme: VisualizationTheme,
        visualization_side: VisualizationSide,
        visualization_colors: VisualizationColorConfig,
    ) -> Self {
        const SAMPLE_RATE: f32 = 44100.0;
        const BUFFER_SIZE: usize = 1024;

        Self {
            audio_level: audio_level.clamp(0.0, 1.0),
            is_speech_detected,
            visualization_theme,
            visualization_side,
            audio_analyzer: AudioAnalyzer::new(SAMPLE_RATE, BUFFER_SIZE),
            audio_samples: Vec::with_capacity(BUFFER_SIZE),
            frequency_data: FrequencyData::default(),
            visualization_colors,
            smoothed_visualization_frequency: DEFAULT_VISUALIZATION_WAVE_FREQUENCY,
        }
    }

    /// Clear the visualization data to ensure clean transition to icon
    pub fn clear(&mut self) {
        self.frequency_data = FrequencyData::default();
        self.audio_samples.clear();
        self.audio_level = 0.0;
        // Reset to default frequency
        self.smoothed_visualization_frequency = DEFAULT_VISUALIZATION_WAVE_FREQUENCY;
    }

    /// Update visualization theme without recreating the component
    pub fn update_theme(&mut self, theme: VisualizationTheme) {
        self.visualization_theme = theme;
    }

    /// Update visualization side without recreating the component
    pub fn update_side(&mut self, side: VisualizationSide) {
        self.visualization_side = side;
    }

    /// Update with pre-computed frequency bands from daemon
    pub fn update_frequency_bands(&mut self, bands: &[f32], total_energy: f32) {
        // For backward compatibility, we need to compute dominant frequency from bands
        // since the daemon might not send it yet
        let (dominant_frequency, frequency_confidence) =
            extract_dominant_frequency_from_bands(bands);

        // Create temporary frequency data for updating smoothed frequency
        self.frequency_data = FrequencyData {
            bands: bands.to_vec(),
            total_energy,
            dominant_frequency,
            frequency_confidence,
            dynamic_wave_frequency: None,
        };

        // Update smoothed wave frequency for dynamic visualization
        self.update_smoothed_wave_frequency();

        // Now update with the computed dynamic wave frequency
        self.frequency_data.dynamic_wave_frequency = Some(self.smoothed_visualization_frequency);
    }

    /// Update with new audio samples for frequency analysis
    pub fn update_audio_samples(&mut self, samples: &[f32]) {
        // Keep a rolling buffer of samples
        self.audio_samples.extend_from_slice(samples);

        // Keep only the most recent samples (buffer size)
        let buffer_size = 1024;
        if self.audio_samples.len() > buffer_size {
            let start = self.audio_samples.len() - buffer_size;
            self.audio_samples.drain(0..start);
        }

        // Perform real FFT analysis on the audio samples
        if !self.audio_samples.is_empty() {
            self.frequency_data = self.audio_analyzer.analyze(&self.audio_samples);

            // Update smoothed wave frequency for dynamic visualization
            self.update_smoothed_wave_frequency();

            // Set the dynamic wave frequency
            self.frequency_data.dynamic_wave_frequency =
                Some(self.smoothed_visualization_frequency);
        }
    }

    /// Update with just audio level (legacy method - only used when no samples available)
    pub fn update_audio_level(&mut self, audio_level: f32, is_speech_detected: bool) {
        self.audio_level = audio_level.clamp(0.0, 1.0);
        self.is_speech_detected = is_speech_detected;

        // Always generate simulated frequency data from audio level
        // This ensures centered equalizer works even without real audio samples
        self.frequency_data = simulate_frequency_data(audio_level);

        // Update smoothed wave frequency for dynamic visualization
        self.update_smoothed_wave_frequency();

        // Set the dynamic wave frequency
        self.frequency_data.dynamic_wave_frequency = Some(self.smoothed_visualization_frequency);
    }

    /// Update the smoothed wave frequency based on current frequency data
    /// This implements the frequency-to-wave parameter mapping system
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    fn update_smoothed_wave_frequency(&mut self) {
        // Early exit if frequency confidence is too low (performance optimization)
        let target_wave_frequency =
            if self.frequency_data.frequency_confidence >= FREQUENCY_CONFIDENCE_THRESHOLD {
                // High confidence - use dynamic mapping
                map_audio_frequency_to_wave_frequency(self.frequency_data.dominant_frequency)
            } else {
                // Low confidence - fall back to default (skip expensive calculations)
                DEFAULT_VISUALIZATION_WAVE_FREQUENCY
            };

        // Apply smoothing to prevent jarring transitions (only if there's a meaningful change)
        let frequency_diff = (target_wave_frequency - self.smoothed_visualization_frequency).abs();
        if frequency_diff > 0.1 {
            // Only update if change is significant
            self.smoothed_visualization_frequency = self.smoothed_visualization_frequency
                * FREQUENCY_SMOOTHING
                + target_wave_frequency * (1.0 - FREQUENCY_SMOOTHING);
        }
    }

    /// Get the current smoothed wave frequency for visualization
    pub fn get_wave_frequency(&self) -> f32 {
        self.smoothed_visualization_frequency
    }

    /// Update visualization colors without recreating the entire component
    pub fn update_colors(&mut self, new_colors: VisualizationColorConfig) {
        self.visualization_colors = new_colors;
    }
}

impl<'a> From<VisualizationComponent> for Element<'a, Message> {
    fn from(visualization: VisualizationComponent) -> Element<'a, Message> {
        // Use applet width as cache key to force redraw when size changes
        Canvas::new(visualization.clone())
            .width(cosmic::iced::Length::Fill)
            .height(cosmic::iced::Length::Fill)
            .into()
    }
}

impl Program<Message, Theme, Renderer> for VisualizationComponent {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());

        // Always clear the frame background to prevent artifacts
        frame.fill_rectangle(
            cosmic::iced::Point::ORIGIN,
            bounds.size(),
            cosmic::iced::Color::TRANSPARENT,
        );

        let is_dark = theme.cosmic().is_dark;
        let cosmic_theme = theme.cosmic();

        // Use the appropriate visualization renderer based on theme
        match self.visualization_theme {
            VisualizationTheme::Pulse => {
                PulseVisualization::default().draw(
                    &mut frame,
                    bounds,
                    &self.frequency_data,
                    &self.visualization_side,
                    &self.visualization_colors,
                    is_dark,
                    cosmic_theme,
                );
            }
            VisualizationTheme::BottomEqualizer => {
                EqualizerVisualization::default().draw(
                    &mut frame,
                    bounds,
                    &self.frequency_data,
                    &self.visualization_side,
                    &self.visualization_colors,
                    is_dark,
                    cosmic_theme,
                );
            }
            VisualizationTheme::CenteredEqualizer => {
                CenteredBarsVisualization::default().draw(
                    &mut frame,
                    bounds,
                    &self.frequency_data,
                    &self.visualization_side,
                    &self.visualization_colors,
                    is_dark,
                    cosmic_theme,
                );
            }
            VisualizationTheme::Waveform => {
                WaveformVisualization::default().draw(
                    &mut frame,
                    bounds,
                    &self.frequency_data,
                    &self.visualization_side,
                    &self.visualization_colors,
                    is_dark,
                    cosmic_theme,
                );
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Create frequency data based on speech characteristics
/// This is not real frequency analysis, but provides a more realistic representation
/// of typical speech frequency distribution than flat scaling
#[allow(clippy::cast_precision_loss)]
fn simulate_frequency_data(audio_level: f32) -> FrequencyData {
    // Generate 64 bands with speech-like frequency distribution (matching new algorithm)
    // Speech energy peaks in mid frequencies and tapers at extremes
    let mut bands = Vec::with_capacity(64);

    for i in 0..64 {
        // Create a speech-like frequency response curve
        let normalized_freq = i as f32 / 63.0; // 0.0 to 1.0

        // Speech frequency response: low at extremes, peak in middle-high
        let speech_response = if normalized_freq < 0.1 {
            // Very low frequencies (sub-bass): minimal energy
            0.2 * normalized_freq / 0.1
        } else if normalized_freq < 0.3 {
            // Low frequencies (bass): growing energy
            0.2 + 0.3 * (normalized_freq - 0.1) / 0.2
        } else if normalized_freq < 0.6 {
            // Mid frequencies (vowels): peak energy
            0.5 + 0.5 * (normalized_freq - 0.3) / 0.3
        } else if normalized_freq < 0.8 {
            // High-mid frequencies (consonants): high energy
            1.0 - 0.2 * (normalized_freq - 0.6) / 0.2
        } else {
            // High frequencies: tapering off
            0.8 * (1.0 - normalized_freq) / 0.2
        };

        // Add some variation to make it more realistic
        let variation = ((i as f32 * 1.618) % 1.0) * 0.3 + 0.8; // 0.8 to 1.1
        bands.push(audio_level * speech_response * variation);
    }

    FrequencyData {
        bands,
        total_energy: audio_level,
        dominant_frequency: 440.0,    // Default A4 for simulated data
        frequency_confidence: 0.0,    // Low confidence for simulated data
        dynamic_wave_frequency: None, // Will be set by the update method
    }
}

/// Map audio frequency (Hz) to wave visualization frequency
/// This creates an intuitive relationship where higher pitch audio = faster waves
fn map_audio_frequency_to_wave_frequency(audio_freq: f32) -> f32 {
    // Clamp audio frequency to our mapping range
    let clamped_freq = audio_freq.clamp(MIN_AUDIO_FREQUENCY, MAX_AUDIO_FREQUENCY);

    // Normalize to 0.0-1.0 range
    let normalized =
        (clamped_freq - MIN_AUDIO_FREQUENCY) / (MAX_AUDIO_FREQUENCY - MIN_AUDIO_FREQUENCY);

    // Apply non-linear mapping for more intuitive feel
    // Use square root to emphasize lower frequencies more
    let shaped = normalized.sqrt();

    // Map to wave frequency range
    MIN_VISUALIZATION_WAVE_FREQUENCY
        + shaped * (MAX_VISUALIZATION_WAVE_FREQUENCY - MIN_VISUALIZATION_WAVE_FREQUENCY)
}

fn extract_dominant_frequency_from_bands(bands: &[f32]) -> (f32, f32) {
    if bands.len() < 32 {
        return (440.0, 0.0);
    }

    // Find the band with maximum energy (optimized single pass)
    let mut max_energy = 0.0f32;
    let mut max_band_idx = 0;
    let mut total_energy = 0.0f32;

    for (i, &energy) in bands.iter().enumerate() {
        let energy_squared = energy * energy;
        total_energy += energy_squared;
        if energy > max_energy {
            max_energy = energy;
            max_band_idx = i;
        }
    }

    // Convert band index to approximate frequency
    // Our bands are: first ~20 linear from 50-800Hz, then ~44 logarithmic from 800Hz-16kHz
    let num_bands = bands.len();
    let linear_bands = (num_bands * 5) / 16; // ~20 bands for 64-band system

    let estimated_freq = if max_band_idx < linear_bands {
        // Linear frequency mapping (50Hz - 800Hz)
        #[allow(clippy::cast_precision_loss)]
        let t = max_band_idx as f32 / linear_bands as f32;
        50.0 + t * (800.0 - 50.0)
    } else {
        // Logarithmic frequency mapping (800Hz - 16kHz)
        let log_bands = num_bands - linear_bands;
        let log_idx = max_band_idx - linear_bands;
        #[allow(clippy::cast_precision_loss)]
        let t = log_idx as f32 / log_bands as f32;

        let log_min = 800.0f32.ln();
        let log_max = 16000.0f32.ln();
        (log_min + t * (log_max - log_min)).exp()
    };

    // Calculate confidence based on energy distribution
    let confidence = if total_energy > 0.0 && max_energy > 0.0 {
        let peak_ratio = (max_energy * max_energy) / total_energy;

        // Apply speech-specific weighting
        let freq_weight = if (200.0..=2000.0).contains(&estimated_freq) {
            1.2 // Boost confidence for typical speech fundamentals
        } else if (80.0..=4000.0).contains(&estimated_freq) {
            1.0 // Normal confidence for extended speech range
        } else {
            0.7 // Lower confidence for frequencies outside typical speech
        };

        (peak_ratio * freq_weight * 3.0).min(1.0)
    } else {
        0.0
    };

    (estimated_freq, confidence)
}
