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

use crate::app::Message;
use crate::util::usize_to_f32;
use crate::{
    config::{
        DEFAULT_VISUALIZATION_WAVE_FREQUENCY, FREQUENCY_CONFIDENCE_THRESHOLD, FREQUENCY_SMOOTHING,
        MAX_AUDIO_FREQUENCY, MAX_VISUALIZATION_WAVE_FREQUENCY, MIN_AUDIO_FREQUENCY,
        MIN_VISUALIZATION_WAVE_FREQUENCY,
    },
    models::theme::{VisualizationColorConfig, VisualizationSide, VisualizationTheme},
    ui::components::visualizations::{
        CenteredBarsVisualization, DrawContext, EqualizerVisualization, PulseVisualization,
        VisualizationRenderer, WaveformVisualization,
    },
};
use super_stt_shared::FrequencyData;

#[derive(Debug, Clone)]
pub struct VisualizationComponent {
    audio_level: f32,
    is_speech_detected: bool,
    visualization_theme: VisualizationTheme,
    visualization_side: VisualizationSide,
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
        Self {
            audio_level: audio_level.clamp(0.0, 1.0),
            is_speech_detected,
            visualization_theme,
            visualization_side,
            frequency_data: FrequencyData::default(),
            visualization_colors,
            smoothed_visualization_frequency: DEFAULT_VISUALIZATION_WAVE_FREQUENCY,
        }
    }

    /// Clear the visualization data to ensure clean transition to icon
    pub fn clear(&mut self) {
        self.frequency_data = FrequencyData::default();
        self.audio_level = 0.0;
        // Reset to default frequency
        self.smoothed_visualization_frequency = DEFAULT_VISUALIZATION_WAVE_FREQUENCY;
    }

    /// Update visualization theme without recreating the component
    pub fn update_theme(&mut self, theme: VisualizationTheme) {
        self.visualization_theme = theme;
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

    /// Update the smoothed wave frequency from the current frequency
    /// data, mapping the dominant audio frequency to a wave parameter.
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

    /// Update visualization colors without recreating the entire component
    pub fn update_colors(&mut self, new_colors: VisualizationColorConfig) {
        self.visualization_colors = new_colors;
    }
}

impl<'a> From<VisualizationComponent> for Element<'a, Message> {
    fn from(visualization: VisualizationComponent) -> Element<'a, Message> {
        // `visualization` is already owned (the caller clones out of `&self`),
        // so hand it straight to the canvas — no second clone.
        Canvas::new(visualization)
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

        let ctx = DrawContext {
            bounds,
            frequency_data: &self.frequency_data,
            side: &self.visualization_side,
            color_config: &self.visualization_colors,
            is_dark: theme.cosmic().is_dark,
            cosmic_theme: theme.cosmic(),
        };

        match self.visualization_theme {
            VisualizationTheme::Pulse => PulseVisualization::default().draw(&mut frame, &ctx),
            VisualizationTheme::BottomEqualizer => {
                EqualizerVisualization::default().draw(&mut frame, &ctx);
            }
            VisualizationTheme::CenteredEqualizer => {
                CenteredBarsVisualization::default().draw(&mut frame, &ctx);
            }
            VisualizationTheme::Waveform => WaveformVisualization::default().draw(&mut frame, &ctx),
        }

        vec![frame.into_geometry()]
    }
}

/// Synthesize speech-like frequency data from an overall audio level.
/// Not real analysis — it approximates a typical speech spectrum (energy
/// peaking in mid frequencies, tapering at the extremes) so the
/// equalizer stays lively when only a level is available.
fn simulate_frequency_data(audio_level: f32) -> FrequencyData {
    let mut bands = Vec::with_capacity(64);

    for i in 0..64 {
        let normalized_freq = usize_to_f32(i) / 63.0; // 0.0 to 1.0

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

        // Pseudo-random variation (0.8..1.1) for a more realistic look.
        let variation = ((usize_to_f32(i) * 1.618) % 1.0) * 0.3 + 0.8;
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
        // Linear frequency mapping (50Hz - 800Hz).
        let t = usize_to_f32(max_band_idx) / usize_to_f32(linear_bands);
        50.0 + t * (800.0 - 50.0)
    } else {
        // Logarithmic frequency mapping (800Hz - 16kHz).
        let log_bands = num_bands - linear_bands;
        let log_idx = max_band_idx - linear_bands;
        let t = usize_to_f32(log_idx) / usize_to_f32(log_bands);

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

#[cfg(test)]
mod frequency_math_tests {
    //! The band math between the daemon's frequency payload and the
    //! renderers. `draw` needs a live renderer, but everything that decides
    //! *what* is drawn — the simulated spectrum, the pitch-to-wave mapping,
    //! and the dominant-frequency estimate — is plain arithmetic.
    use super::*;

    fn mean(bands: &[f32], range: std::ops::Range<usize>) -> f32 {
        let len = range.len();
        bands[range].iter().sum::<f32>() / usize_to_f32(len)
    }

    #[test]
    fn silence_simulates_a_silent_spectrum() {
        let data = simulate_frequency_data(0.0);

        assert_eq!(data.bands.len(), 64);
        assert!(data.bands.iter().all(|band| *band == 0.0));
        assert_eq!(data.total_energy, 0.0);
    }

    #[test]
    fn the_simulated_spectrum_is_speech_shaped() {
        // The point of simulating at all is that a bare audio level draws a
        // flat block. Energy has to peak in the vowel/consonant bands and
        // taper at both extremes or the equalizer looks like a level meter.
        let data = simulate_frequency_data(1.0);

        let low = mean(&data.bands, 0..6);
        let mid = mean(&data.bands, 19..51);
        let high = mean(&data.bands, 58..64);

        assert!(mid > low * 2.0, "mid {mid} should tower over low {low}");
        assert!(mid > high * 2.0, "mid {mid} should tower over high {high}");
        assert!(data.bands.iter().all(|band| *band >= 0.0));
    }

    #[test]
    fn simulated_data_asks_for_the_default_wave_frequency() {
        // Nothing was measured, so the confidence has to stay under the
        // threshold and leave the smoother on its default.
        let data = simulate_frequency_data(0.8);

        assert!(data.frequency_confidence < FREQUENCY_CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn wave_frequency_mapping_covers_the_configured_range() {
        let slowest = map_audio_frequency_to_wave_frequency(MIN_AUDIO_FREQUENCY);
        let fastest = map_audio_frequency_to_wave_frequency(MAX_AUDIO_FREQUENCY);

        assert!((slowest - MIN_VISUALIZATION_WAVE_FREQUENCY).abs() < 1e-3);
        assert!((fastest - MAX_VISUALIZATION_WAVE_FREQUENCY).abs() < 1e-3);
    }

    #[test]
    fn wave_frequency_mapping_clamps_audio_outside_the_speech_range() {
        // Room rumble and sibilance land outside the mapped band; they pin to
        // the ends instead of driving the wave off its range.
        assert_eq!(
            map_audio_frequency_to_wave_frequency(0.0),
            map_audio_frequency_to_wave_frequency(MIN_AUDIO_FREQUENCY),
        );
        assert_eq!(
            map_audio_frequency_to_wave_frequency(48_000.0),
            map_audio_frequency_to_wave_frequency(MAX_AUDIO_FREQUENCY),
        );
    }

    #[test]
    fn wave_frequency_rises_with_pitch_and_favours_the_low_end() {
        let mut previous = f32::MIN;
        for hz in [80.0, 200.0, 400.0, 800.0, 1600.0] {
            let wave = map_audio_frequency_to_wave_frequency(hz);
            assert!(wave > previous, "{hz} Hz did not raise the wave frequency");
            previous = wave;
        }

        // The square-root shaping is what gives a low voice visible movement:
        // the middle of the audio range maps above the middle of the wave
        // range, not onto it.
        let audio_middle = (MIN_AUDIO_FREQUENCY + MAX_AUDIO_FREQUENCY) / 2.0;
        let wave_middle =
            (MIN_VISUALIZATION_WAVE_FREQUENCY + MAX_VISUALIZATION_WAVE_FREQUENCY) / 2.0;
        assert!(map_audio_frequency_to_wave_frequency(audio_middle) > wave_middle);
    }

    #[test]
    fn too_few_bands_falls_back_to_a4_with_no_confidence() {
        // Below 32 bands the index-to-Hz mapping is meaningless, so the
        // estimate has to be declared worthless rather than guessed.
        assert_eq!(
            extract_dominant_frequency_from_bands(&[1.0; 31]),
            (440.0, 0.0)
        );
    }

    #[test]
    fn a_low_peak_maps_into_the_linear_range() {
        let mut bands = vec![0.0; 64];
        bands[2] = 1.0;

        let (hz, confidence) = extract_dominant_frequency_from_bands(&bands);

        assert!((50.0..=800.0).contains(&hz), "got {hz} Hz");
        assert!(confidence > 0.0);
    }

    #[test]
    fn a_high_peak_maps_into_the_logarithmic_range() {
        let mut bands = vec![0.0; 64];
        bands[60] = 1.0;

        let (hz, _) = extract_dominant_frequency_from_bands(&bands);

        assert!((800.0..=16_000.0).contains(&hz), "got {hz} Hz");
    }

    #[test]
    fn a_clear_peak_beats_a_flat_spectrum_on_confidence() {
        let mut peaked = vec![0.01; 64];
        peaked[10] = 1.0;

        let (_, flat_confidence) = extract_dominant_frequency_from_bands(&[0.5; 64]);
        let (_, peak_confidence) = extract_dominant_frequency_from_bands(&peaked);

        assert!(
            peak_confidence > flat_confidence,
            "a spectrum with no peak ({flat_confidence}) must not outrank one with a peak ({peak_confidence})",
        );
        assert!(peak_confidence >= FREQUENCY_CONFIDENCE_THRESHOLD);
        assert!(flat_confidence < FREQUENCY_CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn silence_has_no_dominant_frequency() {
        let (_, confidence) = extract_dominant_frequency_from_bands(&[0.0; 64]);

        assert_eq!(confidence, 0.0);
    }
}

#[cfg(test)]
mod visualization_component_tests {
    //! The component's own state: what a daemon `frequency_bands` event does
    //! to the smoothed wave frequency the renderers read, and what a mic stop
    //! leaves behind.
    use super::*;
    use crate::models::theme::{VisualizationColorConfig, VisualizationSide, VisualizationTheme};

    fn component() -> VisualizationComponent {
        VisualizationComponent::new(
            0.0,
            false,
            VisualizationTheme::Waveform,
            VisualizationSide::Full,
            VisualizationColorConfig::default(),
        )
    }

    /// Bands with a single strong low peak — confident enough to move the
    /// smoother off its default.
    fn peaked_bands() -> Vec<f32> {
        let mut bands = vec![0.01; 64];
        bands[3] = 1.0;
        bands
    }

    #[test]
    fn a_spectrum_with_no_peak_keeps_the_default_wave_frequency() {
        let mut component = component();

        component.update_frequency_bands(&[0.05; 64], 0.1);

        assert!(
            (component.smoothed_visualization_frequency - DEFAULT_VISUALIZATION_WAVE_FREQUENCY)
                .abs()
                < f32::EPSILON,
            "a low-confidence estimate must not steer the wave",
        );
    }

    #[test]
    fn a_confident_spectrum_eases_the_wave_frequency_toward_its_target() {
        let mut component = component();
        let before = component.smoothed_visualization_frequency;

        component.update_frequency_bands(&peaked_bands(), 1.0);

        let after = component.smoothed_visualization_frequency;
        let target =
            map_audio_frequency_to_wave_frequency(component.frequency_data.dominant_frequency);
        assert!(
            (after - target).abs() < (before - target).abs(),
            "smoothing moved away from the target: {before} -> {after}, target {target}",
        );
        assert!(
            (after - target).abs() > f32::EPSILON,
            "smoothing jumped straight to the target instead of easing into it",
        );
    }

    #[test]
    fn the_smoothed_frequency_is_what_the_renderers_read() {
        let mut component = component();

        component.update_frequency_bands(&peaked_bands(), 1.0);

        assert_eq!(
            component.frequency_data.dynamic_wave_frequency,
            Some(component.smoothed_visualization_frequency),
        );
    }

    #[test]
    fn the_bands_and_energy_reach_the_renderers_unchanged() {
        let mut component = component();
        let bands = peaked_bands();

        component.update_frequency_bands(&bands, 0.42);

        assert_eq!(component.frequency_data.bands, bands);
        assert!((component.frequency_data.total_energy - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn clearing_retires_the_last_take() {
        // Run on every mic stop: whatever is left over must not be drawn into
        // the next recording, and the wave has to start from the default
        // again rather than from the last speaker's pitch.
        let mut component = component();
        component.update_frequency_bands(&peaked_bands(), 1.0);

        component.clear();

        assert!(
            component
                .frequency_data
                .bands
                .iter()
                .all(|band| *band == 0.0)
        );
        assert_eq!(component.audio_level, 0.0);
        assert!(
            (component.smoothed_visualization_frequency - DEFAULT_VISUALIZATION_WAVE_FREQUENCY)
                .abs()
                < f32::EPSILON,
        );
    }
}
