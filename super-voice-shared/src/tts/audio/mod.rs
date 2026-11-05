// SPDX-License-Identifier: GPL-3.0-only

// The visualization data type — always available (no analysis dep).
pub use super_engine_protocol::audio::FrequencyData;

// The `POST /v1/synthesize` response framing. Pure parsing over `serde` — no
// audio-stack dependency — so backend hosts, tests, and fixtures share one
// definition of the wire format.
pub mod frames;

// The analyzer that produces the data — needs the FFT stack, so it is gated
// behind the `analysis` feature. Consumers that only render bands (the applet)
// get `FrequencyData` without pulling in `spectrum-analyzer`.
#[cfg(feature = "analysis")]
pub use analysis::*;
#[cfg(feature = "analysis")]
pub use super_engine_protocol::audio::analysis;
