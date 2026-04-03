// SPDX-License-Identifier: GPL-3.0-only

//! Mistral transcription API client.
//!
//! Sends audio to Mistral's `/v1/audio/transcriptions` endpoint and returns
//! the transcribed text. Audio is encoded as WAV in-memory before upload.

use anyhow::{Context, Result};
use log::{debug, info};

use crate::stt_models::third_party::audio::encode_wav_in_memory;

pub struct MistralModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl MistralModel {
    #[must_use]
    pub fn new(api_key: String, model_id: String) -> Self {
        info!("Creating Mistral model client for {model_id}");
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    /// Transcribe audio data by sending it to Mistral's transcription API.
    ///
    /// Audio is encoded as 16-bit PCM WAV in memory and uploaded as multipart form data.
    ///
    /// # Errors
    ///
    /// Returns an error if WAV encoding, the HTTP request, or response parsing fails.
    pub async fn transcribe_audio(&self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        debug!(
            "Sending {} samples at {sample_rate}Hz to Mistral {} API",
            audio_data.len(),
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
            .post("https://api.mistral.ai/v1/audio/transcriptions")
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("Failed to send request to Mistral API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Mistral API returned {status}: {body}");
        }

        let json: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse Mistral API response")?;

        let text = json["text"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("No 'text' field in Mistral API response"))?;

        info!(
            "Mistral {} transcription completed: '{}'",
            self.model_id, text
        );
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistral_model_constructs() {
        let model = MistralModel::new("test-key".to_string(), "voxtral-mini-latest".to_string());
        assert_eq!(model.model_id, "voxtral-mini-latest");
    }
}
