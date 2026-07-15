// SPDX-License-Identifier: GPL-3.0-only
//! Input validation for Super STT protocol messages and data.
use anyhow::Result;

mod inputs;
pub mod limits;
mod paths;

pub use inputs::{
    validate_audio_data, validate_command, validate_event_types, validate_json_value,
    validate_limit, validate_optional_string, validate_required_string, validate_sample_rate,
    validate_string,
};
pub use paths::{get_http_socket_path, secure_runtime_path};

/// Validation errors for better error reporting
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("String too long: {len} > {max} characters")]
    StringTooLong { len: usize, max: usize },

    #[error("Audio data too large: {samples} > {max} samples")]
    AudioTooLarge { samples: usize, max: usize },

    #[error("Invalid sample rate: {rate} (must be {min}-{max} Hz)")]
    InvalidSampleRate { rate: u32, min: u32, max: u32 },

    #[error("Too many event types: {count} > {max}")]
    TooManyEventTypes { count: usize, max: usize },

    #[error("Invalid limit: {limit} (must be 1-{max})")]
    InvalidLimit { limit: u32, max: u32 },

    #[error("JSON data too large: {size} > {max} bytes")]
    JsonTooLarge { size: usize, max: usize },

    #[error("JSON nesting too deep: {depth} > {max}")]
    JsonTooDeep { depth: usize, max: usize },

    #[error("Empty required field: {field}")]
    EmptyField { field: String },

    #[error("Invalid character in field '{field}': contains control characters")]
    InvalidCharacters { field: String },
}

// Note: ValidationError implements std::error::Error via thiserror,
// so anyhow's blanket impl provides the From conversion automatically

/// Trait for validating protocol message components
pub trait Validate {
    /// # Errors
    /// Returns a [`ValidationError`] describing the specific failure.
    fn validate(&self) -> Result<(), ValidationError>;
}
