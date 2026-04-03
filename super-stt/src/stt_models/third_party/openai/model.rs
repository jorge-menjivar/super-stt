// SPDX-License-Identifier: GPL-3.0-only

//! `OpenAI` transcription API client.
//!
//! Sends audio to `OpenAI`'s `/v1/audio/transcriptions` endpoint and returns
//! the transcribed text. Audio is encoded as WAV in-memory before upload.

use anyhow::{Context, Result};
use log::{debug, info};

use crate::stt_models::third_party::audio::encode_wav_in_memory;

pub struct OpenAIModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl OpenAIModel {
    #[must_use]
    pub fn new(api_key: String, model_id: String) -> Self {
        info!("Creating OpenAI model client for {model_id}");
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    /// Transcribe audio data by sending it to `OpenAI`'s transcription API.
    ///
    /// Audio is encoded as 16-bit PCM WAV in memory and uploaded as multipart form data.
    ///
    /// # Errors
    ///
    /// Returns an error if WAV encoding, the HTTP request, or response parsing fails.
    pub async fn transcribe_audio(&self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
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
        let model = OpenAIModel::new("test-key".to_string(), "whisper-1".to_string());
        assert_eq!(model.model_id, "whisper-1");
    }
}
