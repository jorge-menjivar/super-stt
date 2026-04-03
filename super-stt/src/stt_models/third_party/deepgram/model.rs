// SPDX-License-Identifier: GPL-3.0-only

//! Deepgram transcription API client.
//!
//! Sends audio to Deepgram's `/v1/listen` endpoint as raw WAV binary
//! and returns the transcribed text. Deepgram uses `Token` auth (not Bearer)
//! and returns transcription at `results.channels[0].alternatives[0].transcript`.

use anyhow::{Context, Result};
use log::{debug, info};

use crate::stt_models::third_party::audio::encode_wav_in_memory;

pub struct DeepgramModel {
    api_key: String,
    model_id: String,
    client: reqwest::Client,
}

impl DeepgramModel {
    #[must_use]
    pub fn new(api_key: String, model_id: String) -> Self {
        info!("Creating Deepgram model client for {model_id}");
        Self {
            api_key,
            model_id,
            client: reqwest::Client::new(),
        }
    }

    /// Transcribe audio data by sending it to Deepgram's listen API.
    ///
    /// Audio is encoded as WAV and sent as a raw binary body (not multipart).
    /// The model is specified as a query parameter.
    ///
    /// # Errors
    ///
    /// Returns an error if WAV encoding, the HTTP request, or response parsing fails.
    pub async fn transcribe_audio(&self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
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
        let model = DeepgramModel::new("test-key".to_string(), "nova-3".to_string());
        assert_eq!(model.model_id, "nova-3");
    }
}
