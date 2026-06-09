// SPDX-License-Identifier: GPL-3.0-only

//! Mistral transcription API client.
//!
//! Two transport paths:
//! - **Batch** (`/v1/audio/transcriptions`): WAV upload via multipart form.
//!   Used by `voxtral-mini-latest` and similar non-realtime models.
//! - **Realtime** (`wss://api.mistral.ai/v1/audio/transcriptions/realtime`):
//!   WebSocket session that streams base64-encoded PCM s16le 16kHz mono.
//!   Required by `voxtral-mini-transcribe-realtime-*` models — those reject
//!   the batch endpoint with `Invalid model`. The model's `is_realtime_only()`
//!   flag from the registry decides which path is used.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use base64::Engine;
use candle_core::Device;
use futures::{SinkExt, StreamExt};
use log::{debug, info, warn};
use super_stt_shared::models::provider::{OnlineProvider, Provider};
use super_stt_shared::utils::audio::{ResampleQuality, resample};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::stt_models::third_party::audio::encode_wav_in_memory;
use crate::stt_models::transcribe::{ModelInfo, ModelInfoData, ModelState, Transcribe};

/// Realtime model expects PCM s16le mono at this sample rate.
const REALTIME_SAMPLE_RATE: u32 = 16_000;
/// Per-message audio cap; Mistral's `input_audio.append` documents 262144 raw
/// bytes max per chunk. We stay comfortably below that.
const REALTIME_CHUNK_BYTES: usize = 240 * 1024;

pub struct MistralModel {
    api_key: String,
    model_id: String,
    is_realtime: bool,
    client: reqwest::Client,
    info: ModelInfoData,
}

impl MistralModel {
    /// Create a Mistral client.
    /// `name` must match a registry entry whose provider is Mistral; the name
    /// itself doubles as the API model ID.
    ///
    /// # Errors
    /// Returns an error if `name` is not a registry entry served by Mistral.
    ///
    /// # Panics
    /// Panics if a registry-resolved `ModelInfoData` is missing its definition
    /// (an internal invariant — `ModelInfoData::standard` always populates it).
    pub fn new(name: &str, api_key: String) -> Result<Self> {
        let info = ModelInfoData::standard(name, Provider::Online(OnlineProvider::Mistral))
            .ok_or_else(|| anyhow!("Unknown built-in model: {name}"))?;
        let def = info.definition.expect("standard() guarantees a definition");
        if !matches!(def.provider, Provider::Online(OnlineProvider::Mistral)) {
            return Err(anyhow!("{name} is not available via Mistral"));
        }
        let model_id = def.name.to_string();
        let is_realtime = def.is_realtime_only();
        info!("Creating Mistral model client for {model_id} (realtime={is_realtime})");
        Ok(Self {
            api_key,
            model_id,
            is_realtime,
            client: {
                crate::install_crypto_provider();
                reqwest::Client::new()
            },
            info,
        })
    }

    /// Batch path: POST WAV to `/v1/audio/transcriptions`.
    async fn transcribe_batch(&self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        debug!(
            "Sending {} samples at {sample_rate}Hz to Mistral {} batch API",
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
            bail!("Mistral API returned {status}: {body}");
        }

        let json: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse Mistral API response")?;

        let text = json["text"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow!("No 'text' field in Mistral API response"))?;

        Ok(text)
    }

    /// Realtime path: stream PCM s16le 16kHz mono over a WebSocket session,
    /// returning the final `text` from the `transcription.done` event.
    #[allow(clippy::too_many_lines)]
    async fn transcribe_realtime(&self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        debug!(
            "Streaming {} samples at {sample_rate}Hz to Mistral {} realtime API",
            audio_data.len(),
            self.model_id
        );

        let pcm_bytes = encode_pcm_s16le_16khz(audio_data, sample_rate)?;

        let url = format!(
            "wss://api.mistral.ai/v1/audio/transcriptions/realtime?model={}",
            self.model_id
        );
        let mut request = url
            .as_str()
            .into_client_request()
            .context("Failed to build realtime WebSocket request")?;
        request.headers_mut().insert(
            header::AUTHORIZATION,
            format!("Bearer {}", self.api_key)
                .parse()
                .context("Failed to encode bearer token header")?,
        );

        let (mut ws, _resp) = tokio_tungstenite::connect_async(request)
            .await
            .context("Failed to connect to Mistral realtime WebSocket")?;

        // Wait for session.created (or surface an error event from the handshake).
        loop {
            let msg = ws
                .next()
                .await
                .ok_or_else(|| anyhow!("WebSocket closed before session.created"))?
                .context("WebSocket error during handshake")?;
            if let Some(text) = message_text(&msg) {
                let v: serde_json::Value = serde_json::from_str(&text)
                    .with_context(|| format!("Invalid JSON during handshake: {text}"))?;
                match v.get("type").and_then(serde_json::Value::as_str) {
                    Some("session.created") => break,
                    Some("error") => bail!("Mistral realtime handshake error: {text}"),
                    _ => {}
                }
            }
        }

        // Stream audio in chunks, then flush + end the input.
        for chunk in pcm_bytes.chunks(REALTIME_CHUNK_BYTES) {
            let b64 = base64::engine::general_purpose::STANDARD.encode(chunk);
            let msg = serde_json::json!({"type": "input_audio.append", "audio": b64});
            ws.send(Message::Text(msg.to_string().into()))
                .await
                .context("Failed to send audio chunk")?;
        }
        ws.send(Message::Text(
            serde_json::json!({"type": "input_audio.flush"})
                .to_string()
                .into(),
        ))
        .await
        .context("Failed to send input_audio.flush")?;
        ws.send(Message::Text(
            serde_json::json!({"type": "input_audio.end"})
                .to_string()
                .into(),
        ))
        .await
        .context("Failed to send input_audio.end")?;

        // Drain events until transcription.done. Fall back to accumulated
        // deltas if no done.text is reported.
        let mut accumulated = String::new();
        while let Some(msg) = ws.next().await {
            let msg = msg.context("WebSocket error while reading transcription")?;
            if matches!(msg, Message::Close(_)) {
                break;
            }
            let Some(text) = message_text(&msg) else {
                continue;
            };
            let v: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Mistral realtime: ignoring invalid JSON event ({e}): {text}");
                    continue;
                }
            };
            match v.get("type").and_then(serde_json::Value::as_str) {
                Some("transcription.text.delta") => {
                    if let Some(t) = v.get("text").and_then(serde_json::Value::as_str) {
                        accumulated.push_str(t);
                    }
                }
                Some("transcription.done") => {
                    let final_text = v
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .map_or_else(|| accumulated.clone(), str::to_string);
                    let _ = ws.close(None).await;
                    info!(
                        "Mistral {} realtime transcription completed: '{final_text}'",
                        self.model_id
                    );
                    return Ok(final_text);
                }
                Some("error") => {
                    bail!("Mistral realtime error event: {text}");
                }
                _ => {}
            }
        }

        bail!("Mistral realtime WebSocket closed before transcription.done");
    }
}

impl ModelInfo for MistralModel {
    fn info(&self) -> &ModelInfoData {
        &self.info
    }
}

impl ModelState for MistralModel {
    fn device(&self) -> &Device {
        &Device::Cpu
    }
}

#[async_trait]
impl Transcribe for MistralModel {
    /// Transcribe audio. Dispatches to either the batch HTTP endpoint or the
    /// realtime WebSocket endpoint based on the registry definition.
    async fn transcribe_audio(&mut self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        let text = if self.is_realtime {
            self.transcribe_realtime(audio_data, sample_rate).await?
        } else {
            self.transcribe_batch(audio_data, sample_rate).await?
        };
        info!(
            "Mistral {} transcription completed: '{text}'",
            self.model_id
        );
        Ok(text)
    }
}

fn message_text(msg: &Message) -> Option<String> {
    match msg {
        Message::Text(t) => Some(t.to_string()),
        Message::Binary(b) => std::str::from_utf8(b).ok().map(str::to_string),
        _ => None,
    }
}

fn encode_pcm_s16le_16khz(audio_data: &[f32], sample_rate: u32) -> Result<Vec<u8>> {
    let resampled: Vec<f32> = if sample_rate == REALTIME_SAMPLE_RATE {
        audio_data.to_vec()
    } else {
        resample(
            audio_data,
            sample_rate,
            REALTIME_SAMPLE_RATE,
            ResampleQuality::Fast,
        )
        .context("Failed to resample audio for Mistral realtime API")?
    };
    let mut bytes = Vec::with_capacity(resampled.len() * 2);
    for sample in resampled {
        #[allow(clippy::cast_possible_truncation)]
        let i = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        bytes.extend_from_slice(&i.to_le_bytes());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mistral_model_constructs() {
        let model = MistralModel::new("voxtral-mini-latest", "test-key".to_string()).unwrap();
        assert_eq!(model.model_id, "voxtral-mini-latest");
        assert!(!model.is_realtime);
    }

    #[test]
    fn mistral_realtime_model_flagged() {
        let model = MistralModel::new(
            "voxtral-mini-transcribe-realtime-latest",
            "test-key".to_string(),
        )
        .unwrap();
        assert!(model.is_realtime);
    }

    #[test]
    fn mistral_model_rejects_non_mistral_name() {
        // whisper-tiny is local-only — should fail with "not available via Mistral"
        let result = MistralModel::new("whisper-tiny", "test-key".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn pcm_s16le_passthrough_at_target_rate() {
        let samples = [0.0_f32, 1.0, -1.0, 0.5, -0.5];
        let bytes = encode_pcm_s16le_16khz(&samples, REALTIME_SAMPLE_RATE).unwrap();
        assert_eq!(bytes.len(), samples.len() * 2);
        // 1.0 → i16::MAX (32767), little-endian
        assert_eq!(&bytes[2..4], &i16::MAX.to_le_bytes());
        // -1.0 → -i16::MAX (-32767)
        assert_eq!(&bytes[4..6], &(-i16::MAX).to_le_bytes());
    }
}
