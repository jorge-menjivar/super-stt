// SPDX-License-Identifier: GPL-3.0-only
//! Input validation module for Super STT protocol messages and data
//!
//! This module provides comprehensive validation for all user inputs to prevent
//! `DoS` attacks and ensure data integrity.

use anyhow::Result;
use serde_json::Value;

/// Maximum allowed sizes for various data types to prevent `DoS` attacks
pub mod limits {
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
}

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
    /// Validate the component and return a detailed error if invalid
    ///
    /// # Errors
    /// Returns a [`ValidationError`] variant describing the specific
    /// validation failure when the input is not acceptable.
    fn validate(&self) -> Result<(), ValidationError>;
}

/// Validate string fields with length and character restrictions
///
/// # Errors
/// Returns [`ValidationError::StringTooLong`] when `value` exceeds `max_length`,
/// or [`ValidationError::InvalidCharacters`] when control characters are found.
pub fn validate_string(
    value: &str,
    field_name: &str,
    max_length: usize,
) -> Result<(), ValidationError> {
    if value.len() > max_length {
        return Err(ValidationError::StringTooLong {
            len: value.len(),
            max: max_length,
        });
    }

    // Check for control characters that could cause issues
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        return Err(ValidationError::InvalidCharacters {
            field: field_name.to_string(),
        });
    }

    Ok(())
}

/// Validate optional string fields
///
/// # Errors
/// Propagates errors from [`validate_string`] when `value` is `Some` and
/// validation fails.
pub fn validate_optional_string(
    value: &Option<String>,
    field_name: &str,
    max_length: usize,
) -> Result<(), ValidationError> {
    if let Some(s) = value {
        validate_string(s, field_name, max_length)?;
    }
    Ok(())
}

/// Validate required string fields (non-empty)
///
/// # Errors
/// Returns [`ValidationError::EmptyField`] when `value` is `None` or empty,
/// or propagates errors from [`validate_string`].
pub fn validate_required_string(
    value: &Option<String>,
    field_name: &str,
    max_length: usize,
) -> Result<(), ValidationError> {
    match value {
        Some(s) if s.is_empty() => Err(ValidationError::EmptyField {
            field: field_name.to_string(),
        }),
        Some(s) => validate_string(s, field_name, max_length),
        None => Err(ValidationError::EmptyField {
            field: field_name.to_string(),
        }),
    }
}

/// Validate audio data size
///
/// # Errors
/// Returns [`ValidationError::AudioTooLarge`] when sample count exceeds
/// [`limits::MAX_AUDIO_SAMPLES`]. Also flags suspicious constant-value buffers.
pub fn validate_audio_data(audio_data: &[f32]) -> Result<(), ValidationError> {
    if audio_data.len() > limits::MAX_AUDIO_SAMPLES {
        return Err(ValidationError::AudioTooLarge {
            samples: audio_data.len(),
            max: limits::MAX_AUDIO_SAMPLES,
        });
    }

    // Additional check for suspicious patterns that could indicate an attack
    if audio_data.len() > 1_000_000 {
        // Check if all values are the same (possible padding attack)
        if audio_data
            .windows(2)
            .all(|w| (w[0] - w[1]).abs() < f32::EPSILON)
        {
            return Err(ValidationError::AudioTooLarge {
                samples: audio_data.len(),
                max: limits::MAX_AUDIO_SAMPLES,
            });
        }
    }

    Ok(())
}

/// Validate sample rate
///
/// # Errors
/// Returns [`ValidationError::InvalidSampleRate`] if `sample_rate` falls
/// outside [`limits::MIN_SAMPLE_RATE`]..=[`limits::MAX_SAMPLE_RATE`].
pub fn validate_sample_rate(sample_rate: u32) -> Result<(), ValidationError> {
    if !(limits::MIN_SAMPLE_RATE..=limits::MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err(ValidationError::InvalidSampleRate {
            rate: sample_rate,
            min: limits::MIN_SAMPLE_RATE,
            max: limits::MAX_SAMPLE_RATE,
        });
    }
    Ok(())
}

/// Validate event types list
///
/// # Errors
/// Returns [`ValidationError::TooManyEventTypes`] if the list exceeds
/// [`limits::MAX_EVENT_TYPES`], or any error returned by [`validate_string`]
/// for invalid event type strings.
pub fn validate_event_types(event_types: &[String]) -> Result<(), ValidationError> {
    if event_types.len() > limits::MAX_EVENT_TYPES {
        return Err(ValidationError::TooManyEventTypes {
            count: event_types.len(),
            max: limits::MAX_EVENT_TYPES,
        });
    }

    // Validate each event type string
    for event_type in event_types {
        validate_string(event_type, "event_type", limits::MAX_NAME_LENGTH)?;
    }

    Ok(())
}

/// Validate pagination limit
///
/// # Errors
/// Returns [`ValidationError::InvalidLimit`] if `limit` is 0 or greater
/// than [`limits::MAX_EVENTS_LIMIT`].
pub fn validate_limit(limit: u32) -> Result<(), ValidationError> {
    if limit == 0 || limit > limits::MAX_EVENTS_LIMIT {
        return Err(ValidationError::InvalidLimit {
            limit,
            max: limits::MAX_EVENTS_LIMIT,
        });
    }
    Ok(())
}

/// Validate JSON data size and complexity
///
/// # Errors
/// Returns:
/// - [`ValidationError::JsonTooLarge`] if the serialized size exceeds
///   [`limits::MAX_JSON_SIZE`].
/// - [`ValidationError::JsonTooDeep`] if the nesting depth exceeds
///   [`limits::MAX_JSON_DEPTH`].
pub fn validate_json_value(value: &Value) -> Result<(), ValidationError> {
    // Check serialized size
    let serialized = serde_json::to_vec(value).map_err(|_| ValidationError::JsonTooLarge {
        size: 0,
        max: limits::MAX_JSON_SIZE,
    })?;

    if serialized.len() > limits::MAX_JSON_SIZE {
        return Err(ValidationError::JsonTooLarge {
            size: serialized.len(),
            max: limits::MAX_JSON_SIZE,
        });
    }

    // Check nesting depth
    check_depth(value, 0, limits::MAX_JSON_DEPTH)?;

    Ok(())
}

/// Validate command strings to prevent injection
///
/// # Errors
/// Returns [`ValidationError::InvalidCharacters`] if the command contains
/// disallowed characters, or any error returned by [`validate_string`].
pub fn validate_command(command: &str) -> Result<(), ValidationError> {
    validate_string(command, "command", limits::MAX_NAME_LENGTH)?;

    // Only allow alphanumeric characters, underscores, and hyphens
    if !command
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ValidationError::InvalidCharacters {
            field: "command".to_string(),
        });
    }

    Ok(())
}

/// Generate a cryptographically secure client ID
///
/// This function generates a unique client ID that prevents prediction and impersonation attacks.
///
/// Security features:
/// - UUID v4 for cryptographic randomness
/// - High-resolution timestamp for temporal uniqueness
/// - Process ID for system-level uniqueness
/// - Multi-factor composition for collision resistance
///
/// Format: `{component}-{pid}-{timestamp}-{uuid}`
#[must_use]
pub fn generate_secure_client_id(component: &str) -> String {
    let pid = std::process::id();
    let uuid = uuid::Uuid::new_v4();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_nanos();
    format!("{component}-{pid}-{timestamp}-{uuid}")
}

/// Get a secure socket path with comprehensive validation
///
/// This function validates the `XDG_RUNTIME_DIR` environment variable and constructs
/// a secure socket path that prevents path injection attacks.
///
/// Security features:
/// - Path length validation
/// - Path traversal prevention
/// - Directory whitelist enforcement
/// - Canonical path verification
/// - Secure fallback behavior
/// - Security event logging
#[must_use]
pub fn get_secure_socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));

    // Validate runtime directory path
    if runtime_dir.is_empty() || runtime_dir.len() > 256 {
        log::warn!("Invalid XDG_RUNTIME_DIR length, using fallback");
        return std::path::PathBuf::from("/tmp/stt/super-stt.sock");
    }

    // Check for path traversal attempts and null bytes
    if runtime_dir.contains("..") || runtime_dir.contains('\0') {
        log::warn!("Potential path traversal in XDG_RUNTIME_DIR, using fallback");
        return std::path::PathBuf::from("/tmp/stt/super-stt.sock");
    }

    // Ensure the path starts with expected prefixes
    if !runtime_dir.starts_with("/run/user/") && !runtime_dir.starts_with("/tmp/") {
        log::warn!("XDG_RUNTIME_DIR outside allowed directories: {runtime_dir}, using fallback");
        return std::path::PathBuf::from("/tmp/stt/super-stt.sock");
    }

    let path = std::path::PathBuf::from(runtime_dir)
        .join("stt")
        .join("super-stt.sock");

    // Final validation - ensure resolved path is still safe
    if let Ok(canonical) = path.canonicalize() {
        if !canonical.starts_with("/run/user/") && !canonical.starts_with("/tmp/") {
            log::warn!("Canonical socket path outside allowed directories, using fallback");
            return std::path::PathBuf::from("/tmp/stt/super-stt.sock");
        }
        canonical
    } else {
        // Path doesn't exist yet, that's okay - return the validated non-canonical path
        path
    }
}

/// Get the path of the *new* HTTP-protocol Unix socket (`super-stt-http.sock`).
///
/// This sits next to the legacy length-prefix socket from `get_secure_socket_path`,
/// in the same `$XDG_RUNTIME_DIR/stt/` directory. The two listeners run
/// side-by-side; clients pick whichever one matches their protocol.
#[must_use]
pub fn get_http_socket_path() -> std::path::PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| format!("/run/user/{}", unsafe { libc::getuid() }));

    if runtime_dir.is_empty() || runtime_dir.len() > 256 {
        log::warn!("Invalid XDG_RUNTIME_DIR length, using fallback");
        return std::path::PathBuf::from("/tmp/stt/super-stt-http.sock");
    }

    if runtime_dir.contains("..") || runtime_dir.contains('\0') {
        log::warn!("Potential path traversal in XDG_RUNTIME_DIR, using fallback");
        return std::path::PathBuf::from("/tmp/stt/super-stt-http.sock");
    }

    if !runtime_dir.starts_with("/run/user/") && !runtime_dir.starts_with("/tmp/") {
        log::warn!("XDG_RUNTIME_DIR outside allowed directories: {runtime_dir}, using fallback");
        return std::path::PathBuf::from("/tmp/stt/super-stt-http.sock");
    }

    let path = std::path::PathBuf::from(runtime_dir)
        .join("stt")
        .join("super-stt-http.sock");

    if let Ok(canonical) = path.canonicalize() {
        if !canonical.starts_with("/run/user/") && !canonical.starts_with("/tmp/") {
            log::warn!("Canonical http socket path outside allowed directories, using fallback");
            return std::path::PathBuf::from("/tmp/stt/super-stt-http.sock");
        }
        canonical
    } else {
        path
    }
}

// Helper to check JSON nesting depth without defining items after statements
fn check_depth(
    value: &Value,
    current_depth: usize,
    max_depth: usize,
) -> Result<(), ValidationError> {
    if current_depth > max_depth {
        return Err(ValidationError::JsonTooDeep {
            depth: current_depth,
            max: max_depth,
        });
    }

    match value {
        Value::Object(obj) => {
            for v in obj.values() {
                check_depth(v, current_depth + 1, max_depth)?;
            }
        }
        Value::Array(arr) => {
            for v in arr {
                check_depth(v, current_depth + 1, max_depth)?;
            }
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validate_string() {
        // Valid string
        assert!(validate_string("hello", "test", 10).is_ok());

        // Too long
        assert!(validate_string("hello world!", "test", 5).is_err());

        // Control characters
        assert!(validate_string("hello\x00world", "test", 20).is_err());

        // Allowed whitespace
        assert!(validate_string("hello\nworld\ttest", "test", 20).is_ok());
    }

    #[test]
    fn test_validate_audio_data() {
        // Valid audio
        let audio = vec![0.5f32; 1000];
        assert!(validate_audio_data(&audio).is_ok());

        // Too large
        let large_audio = vec![0.5f32; limits::MAX_AUDIO_SAMPLES + 1];
        assert!(validate_audio_data(&large_audio).is_err());

        // Suspicious pattern (all same values)
        let suspicious_audio = vec![0.5f32; 2_000_000];
        assert!(validate_audio_data(&suspicious_audio).is_err());
    }

    #[test]
    fn test_validate_sample_rate() {
        // Valid rates
        assert!(validate_sample_rate(16000).is_ok());
        assert!(validate_sample_rate(44100).is_ok());

        // Invalid rates
        assert!(validate_sample_rate(0).is_err());
        assert!(validate_sample_rate(7999).is_err());
        assert!(validate_sample_rate(96001).is_err());
    }

    #[test]
    fn test_validate_json_value() {
        // Valid JSON
        let json = json!({"key": "value", "number": 42});
        assert!(validate_json_value(&json).is_ok());

        // Too nested - create a deeply nested JSON structure
        let mut nested = json!({"level_0": {}});
        // Build a nested JSON that exceeds the depth limit
        for i in 1..15 {
            nested = json!({format!("level_{}", i): nested});
        }
        assert!(validate_json_value(&nested).is_err());
    }

    #[test]
    fn test_validate_command() {
        // Valid commands
        assert!(validate_command("transcribe").is_ok());
        assert!(validate_command("get_events").is_ok());
        assert!(validate_command("set-model").is_ok());

        // Invalid commands
        assert!(validate_command("rm -rf /").is_err());
        assert!(validate_command("cmd; rm -rf /").is_err());
        assert!(validate_command("cmd|ls").is_err());
    }

    #[test]
    fn test_generate_secure_client_id() {
        // Test that client IDs are unique
        let id1 = generate_secure_client_id("test-app");
        let id2 = generate_secure_client_id("test-app");
        assert_ne!(id1, id2, "Client IDs must be unique");

        // Test that client IDs contain the component name
        let app_id = generate_secure_client_id("super-stt-app");
        assert!(
            app_id.starts_with("super-stt-app-"),
            "Client ID should start with component name"
        );

        let applet_id = generate_secure_client_id("super-stt-applet");
        assert!(
            applet_id.starts_with("super-stt-applet-"),
            "Client ID should start with component name"
        );

        // Test that client IDs have expected format (component-pid-timestamp-uuid)
        let parts: Vec<&str> = app_id.split('-').collect();
        assert!(
            parts.len() >= 6,
            "Client ID should have at least 6 parts separated by hyphens"
        );

        // Test that the UUID part is valid (36 characters with hyphens)
        let uuid_part = parts[parts.len() - 5..].join("-");
        assert_eq!(uuid_part.len(), 36, "UUID part should be 36 characters");
    }

    #[test]
    fn test_get_secure_socket_path() {
        // Test with valid XDG_RUNTIME_DIR
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1000");
        }
        let path = get_secure_socket_path();
        assert!(path.to_string_lossy().contains("super-stt.sock"));

        // Test with potentially malicious XDG_RUNTIME_DIR (path traversal)
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "../../../etc");
        }
        let fallback_path = get_secure_socket_path();
        assert_eq!(
            fallback_path,
            std::path::PathBuf::from("/tmp/stt/super-stt.sock")
        );

        // Test with invalid directory prefix
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/etc/passwd");
        }
        let fallback_path2 = get_secure_socket_path();
        assert_eq!(
            fallback_path2,
            std::path::PathBuf::from("/tmp/stt/super-stt.sock")
        );

        // Test with extremely long path
        let long_path = "a".repeat(300);
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", &long_path);
        }
        let fallback_path3 = get_secure_socket_path();
        assert_eq!(
            fallback_path3,
            std::path::PathBuf::from("/tmp/stt/super-stt.sock")
        );

        // Clean up environment
        unsafe {
            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }
}
