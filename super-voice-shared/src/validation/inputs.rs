// SPDX-License-Identifier: GPL-3.0-only
//! Validators for protocol input fields.

use super::ValidationError;
use super::limits;
use serde_json::Value;

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

    // Additional check for suspicious patterns that could indicate an attack.
    // This is a content problem, not a size overflow (the buffer is within the
    // size cap), so report it as such rather than AudioTooLarge.
    if audio_data.len() > 1_000_000 {
        // Check if all values are the same (possible padding attack)
        if audio_data
            .windows(2)
            .all(|w| (w[0] - w[1]).abs() < f32::EPSILON)
        {
            return Err(ValidationError::SuspiciousAudioContent {
                samples: audio_data.len(),
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

        // Too large → size error.
        let large_audio = vec![0.5f32; limits::MAX_AUDIO_SAMPLES + 1];
        assert!(matches!(
            validate_audio_data(&large_audio),
            Err(ValidationError::AudioTooLarge { .. })
        ));

        // Uniform padding within the size cap → content error, not AudioTooLarge.
        let suspicious_audio = vec![0.5f32; 2_000_000];
        assert!(matches!(
            validate_audio_data(&suspicious_audio),
            Err(ValidationError::SuspiciousAudioContent { .. })
        ));
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
}
