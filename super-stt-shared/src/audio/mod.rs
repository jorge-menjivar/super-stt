// SPDX-License-Identifier: GPL-3.0-only

// Plain visualization data types — always available (no analysis dep).
pub mod types;
pub use types::*;

// The analyzer that produces the data — needs the FFT stack, so it is gated
// behind the `analysis` feature. Consumers that only render bands (the applet)
// get `FrequencyData` from `types` without pulling in `spectrum-analyzer`.
#[cfg(feature = "analysis")]
pub mod analysis;
#[cfg(feature = "analysis")]
pub use analysis::*;
