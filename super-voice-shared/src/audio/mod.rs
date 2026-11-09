// SPDX-License-Identifier: GPL-3.0-only

// The visualization data type — always available (no analysis dep).
pub use super_engine_protocol::audio::FrequencyData;

// The analyzer that produces the data — needs the FFT stack, so it is gated
// behind the `analysis` feature. Consumers that only render bands (the applet)
// get `FrequencyData` without pulling in `spectrum-analyzer`.
#[cfg(feature = "analysis")]
pub use analysis::*;
#[cfg(feature = "analysis")]
pub use super_engine_protocol::audio::analysis;
