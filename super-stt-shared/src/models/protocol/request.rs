// SPDX-License-Identifier: GPL-3.0-only
use crate::validation::{self, Validate, ValidationError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaemonRequest {
    pub command: String,
    #[serde(default)]
    pub audio_data: Option<Vec<f32>>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
    #[serde(default)]
    pub client_id: Option<String>,

    // Notification system fields
    #[serde(default)]
    pub event_types: Option<Vec<String>>,
    #[serde(default)]
    pub client_info: Option<HashMap<String, Value>>,
    #[serde(default)]
    pub since_timestamp: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl Validate for DaemonRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        // Validate command string
        validation::validate_command(&self.command)?;

        // Validate audio data if present
        if let Some(ref audio_data) = self.audio_data {
            validation::validate_audio_data(audio_data)?;
        }

        // Validate sample rate if present
        if let Some(sample_rate) = self.sample_rate {
            validation::validate_sample_rate(sample_rate)?;
        }

        // Validate string fields
        validation::validate_optional_string(
            &self.client_id,
            "client_id",
            validation::limits::MAX_STRING_LENGTH,
        )?;
        validation::validate_optional_string(
            &self.since_timestamp,
            "since_timestamp",
            validation::limits::MAX_STRING_LENGTH,
        )?;
        validation::validate_optional_string(
            &self.event_type,
            "event_type",
            validation::limits::MAX_NAME_LENGTH,
        )?;
        validation::validate_optional_string(
            &self.language,
            "language",
            validation::limits::MAX_NAME_LENGTH,
        )?;

        // Validate event types if present
        if let Some(ref event_types) = self.event_types {
            validation::validate_event_types(event_types)?;
        }

        // Validate limit if present
        if let Some(limit) = self.limit {
            validation::validate_limit(limit)?;
        }

        // Validate JSON data if present
        if let Some(ref data) = self.data {
            validation::validate_json_value(data)?;
        }

        // Validate client_info if present
        if let Some(ref client_info) = self.client_info {
            for value in client_info.values() {
                validation::validate_json_value(value)?;
            }
        }

        Ok(())
    }
}
