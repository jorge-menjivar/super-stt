// SPDX-License-Identifier: GPL-3.0-only
//! Maximum allowed sizes for various data types to prevent `DoS` attacks

/// Maximum audio data length (samples) - ~30 minutes at 16kHz
pub const MAX_AUDIO_SAMPLES: usize = 16_000 * 60 * 30;

/// Maximum string length for text fields like `client_id`, commands, etc.
pub const MAX_STRING_LENGTH: usize = 1024;

/// Maximum length for theme names and device names
pub const MAX_NAME_LENGTH: usize = 256;

/// Maximum number of event types in a subscription
pub const MAX_EVENT_TYPES: usize = 100;

/// Maximum sample rate (Hz)
pub const MAX_SAMPLE_RATE: u32 = 96_000;

/// Minimum sample rate (Hz)
pub const MIN_SAMPLE_RATE: u32 = 8_000;

/// Maximum number of events to retrieve at once
pub const MAX_EVENTS_LIMIT: u32 = 1_000;

/// Maximum JSON value depth to prevent stack overflow
pub const MAX_JSON_DEPTH: usize = 10;

/// Maximum size of JSON data fields (bytes)
pub const MAX_JSON_SIZE: usize = 1024 * 1024; // 1MB
