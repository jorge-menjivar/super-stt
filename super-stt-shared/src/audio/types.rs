// SPDX-License-Identifier: GPL-3.0-only
//! Plain audio-visualization data types.
//!
//! [`FrequencyData`] is the wire/rendering shape for frequency-band data. It
//! is deliberately free of any analysis dependency (no `spectrum-analyzer`),
//! so consumers that only *render* bands — e.g. the applet, which receives
//! pre-computed bands from the daemon over SSE — can use it without pulling
//! in the FFT stack. The analyzer that *produces* it lives in
//! [`super::analysis`], gated behind the `analysis` feature.

/// Audio frequency data for wave visualization.
#[derive(Debug, Clone)]
pub struct FrequencyData {
    pub bands: Vec<f32>, // Frequency band amplitudes (0.0 to 1.0)
    pub total_energy: f32,
    pub dominant_frequency: f32, // Dominant frequency in Hz for dynamic wave visualization
    pub frequency_confidence: f32, // Confidence of dominant frequency detection (0.0 to 1.0)
    pub dynamic_wave_frequency: Option<f32>, // Optional dynamic wave frequency for visualization
}

impl Default for FrequencyData {
    fn default() -> Self {
        Self {
            bands: vec![0.0; 64], // Default 64 frequency bands for richer visualization
            total_energy: 0.0,
            dominant_frequency: 440.0, // Default to A4 (440Hz) when no audio
            frequency_confidence: 0.0,
            dynamic_wave_frequency: None, // Let the applet handle wave frequency mapping
        }
    }
}
