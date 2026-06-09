// SPDX-License-Identifier: GPL-3.0-only

//! Deepgram transcription API client.
//!
//! Sends audio to Deepgram's `/v1/listen` endpoint as raw WAV binary
//! and returns the transcribed text. Deepgram uses `Token` auth (not Bearer)
//! and returns transcription at `results.channels[0].alternatives[0].transcript`.

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use candle_core::Device;
use log::{debug, info};
use super_stt_shared::models::provider::{OnlineProvider, Provider};

use crate::stt_models::third_party::audio::encode_wav_in_memory;
use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};

pub struct DeepgramModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
    info: ModelInfoData,
}

impl DeepgramModel {
    /// Create a Deepgram client.
    /// `name` must match a registry entry whose provider is Deepgram; the name
    /// itself doubles as the API model ID.
    ///
    /// # Errors
    /// Returns an error if `name` is not a registry entry served by Deepgram.
    ///
    /// # Panics
    /// Panics if a registry-resolved `ModelInfoData` is missing its definition
    /// (an internal invariant — `ModelInfoData::standard` always populates it).
    pub fn new(name: &str, api_key: String) -> Result<Self> {
        let info = ModelInfoData::standard(name, Provider::Online(OnlineProvider::Deepgram))
            .ok_or_else(|| anyhow!("Unknown built-in model: {name}"))?;
        let def = info.definition.expect("standard() guarantees a definition");
        if !matches!(def.provider, Provider::Online(OnlineProvider::Deepgram)) {
            return Err(anyhow!("{name} is not available via Deepgram"));
        }
        let model_id = def.name.to_string();
        info!("Creating Deepgram model client for {model_id}");
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

impl ModelInfo for DeepgramModel {
    fn info(&self) -> &ModelInfoData {
        &self.info
    }
}

impl ModelState for DeepgramModel {
    fn device(&self) -> &Device {
        &Device::Cpu
    }
}

#[async_trait]
impl Transcribe for DeepgramModel {
    /// Transcribe audio by sending it to Deepgram's listen API.
    /// Audio is encoded as WAV and sent as a raw binary body (not multipart).
    /// The model is specified as a query parameter.
    async fn transcribe_audio(&mut self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        debug!(
            "Sending {} samples at {sample_rate}Hz to Deepgram {} API",
            audio_data.len(),
            self.model_id
        );

        let wav_bytes = encode_wav_in_memory(audio_data, sample_rate)?;

        let url = format!(
            "https://api.deepgram.com/v1/listen?model={}&smart_format=true",
            self.model_id
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Content-Type", "audio/wav")
            .body(wav_bytes)
            .send()
            .await
            .context("Failed to send request to Deepgram API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Deepgram API returned {status}: {body}");
        }

        let json: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse Deepgram API response")?;

        // Deepgram response: results.channels[0].alternatives[0].transcript
        let text = json["results"]["channels"]
            .get(0)
            .and_then(|ch| ch["alternatives"].get(0))
            .and_then(|alt| alt["transcript"].as_str())
            .map(String::from)
            .ok_or_else(|| {
                anyhow::anyhow!("No transcript found in Deepgram API response: {json}")
            })?;

        info!(
            "Deepgram {} transcription completed: '{}'",
            self.model_id, text
        );
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepgram_model_constructs() {
        let model = DeepgramModel::new("nova-3", "test-key".to_string()).unwrap();
        assert_eq!(model.model_id, "nova-3");
    }

    #[test]
    fn deepgram_model_rejects_non_deepgram_name() {
        let result = DeepgramModel::new("whisper-tiny", "test-key".to_string());
        assert!(result.is_err());
    }
}
