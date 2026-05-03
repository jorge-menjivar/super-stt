// SPDX-License-Identifier: GPL-3.0-only

//! `OpenAI` transcription API client.
//!
//! Sends audio to `OpenAI`'s `/v1/audio/transcriptions` endpoint and returns
//! the transcribed text. Audio is encoded as WAV in-memory before upload.

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use candle_core::Device;
use log::{debug, info};
use super_stt_shared::models::provider::{OnlineProvider, Provider};

use crate::stt_models::third_party::audio::encode_wav_in_memory;
use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};

pub struct OpenAIModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
    info: ModelInfoData,
}

impl OpenAIModel {
    /// Create an `OpenAI` client.
    /// `name` must match a registry entry whose provider is `OpenAI`; the name
    /// itself doubles as the API model ID.
    ///
    /// # Errors
    /// Returns an error if `name` is not a registry entry served by `OpenAI`.
    ///
    /// # Panics
    /// Panics if a registry-resolved `ModelInfoData` is missing its definition
    /// (an internal invariant — `ModelInfoData::standard` always populates it).
    pub fn new(name: &str, api_key: String) -> Result<Self> {
        let info = ModelInfoData::standard(name, Provider::Online(OnlineProvider::OpenAI))
            .ok_or_else(|| anyhow!("Unknown built-in model: {name}"))?;
        let def = info.definition.expect("standard() guarantees a definition");
        if !matches!(def.provider, Provider::Online(OnlineProvider::OpenAI)) {
            return Err(anyhow!("{name} is not available via OpenAI"));
        }
        let model_id = def.name.to_string();
        info!("Creating OpenAI model client for {model_id}");
        Ok(Self {
            api_key,
            model_id,
            client: {
                crate::install_crypto_provider();
                reqwest::Client::new()
            },
            info,
        })
    }
}

impl ModelInfo for OpenAIModel {
    fn info(&self) -> &ModelInfoData {
        &self.info
    }
}

impl ModelState for OpenAIModel {
    /// Online models don't run on a local device — return CPU as a placeholder.
    fn device(&self) -> &Device {
        &Device::Cpu
    }
}

#[async_trait]
impl Transcribe for OpenAIModel {
    /// Transcribe audio by sending it to `OpenAI`'s transcription API.
    /// Audio is encoded as 16-bit PCM WAV in memory and uploaded as multipart form data.
    async fn transcribe_audio(&mut self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        debug!(
            "Sending {} samples at {}Hz to OpenAI {} API",
            audio_data.len(),
            sample_rate,
            self.model_id
        );

        let wav_bytes = encode_wav_in_memory(audio_data, sample_rate)?;

        let part = reqwest::multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        let form = reqwest::multipart::Form::new()
            .text("model", self.model_id.clone())
            .part("file", part);

        let response = self
            .client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("Failed to send request to OpenAI API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI API returned {status}: {body}");
        }

        let json: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse OpenAI API response")?;

        let text = json["text"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("No 'text' field in OpenAI API response"))?;

        info!(
            "OpenAI {} transcription completed: '{}'",
            self.model_id, text
        );
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_model_constructs() {
        let model = OpenAIModel::new("whisper-1", "test-key".to_string()).unwrap();
        assert_eq!(model.model_id, "whisper-1");
    }

    #[test]
    fn openai_model_rejects_non_openai_name() {
        // whisper-tiny is local-only — should fail with "not available via OpenAI"
        let result = OpenAIModel::new("whisper-tiny", "test-key".to_string());
        assert!(result.is_err());
    }
}
