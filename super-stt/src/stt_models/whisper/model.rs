// SPDX-License-Identifier: GPL-3.0-only
use anyhow::{Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use candle_core::utils::cuda_is_available;
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::{VarBuilder, ops::softmax};
use candle_transformers::models::whisper::{self as m, Config, audio};
use log::{debug, info, warn};
use std::io::Cursor;
use super_stt_shared::audio_utils::ResampleQuality;
use super_stt_shared::stt_model::STTModel;
use tokenizers::Tokenizer;

use super_stt_shared::utils::audio::resample;

const SAMPLE_RATE: u32 = 16000;

pub enum Model {
    Normal(m::model::Whisper),
}

impl Model {
    /// # Errors
    ///
    /// Returns an error if the encoder forward operation fails.
    pub fn encoder_forward(&mut self, x: &Tensor, flush: bool) -> candle_core::Result<Tensor> {
        match self {
            Self::Normal(m) => m.encoder.forward(x, flush),
        }
    }

    /// # Errors
    ///
    /// Returns an error if the decoder forward operation fails.
    pub fn decoder_forward(
        &mut self,
        x: &Tensor,
        xa: &Tensor,
        flush: bool,
    ) -> candle_core::Result<Tensor> {
        match self {
            Self::Normal(m) => m.decoder.forward(x, xa, flush),
        }
    }

    /// # Errors
    ///
    /// Returns an error if the decoder final linear operation fails.
    pub fn decoder_final_linear(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        match self {
            Self::Normal(m) => m.decoder.final_linear(x),
        }
    }
}

pub struct WhisperModel {
    model: Model,
    tokenizer: Tokenizer,
    device: Device,
    config: Config,
    mel_filters: Vec<f32>,
    sot_token: u32,
    transcribe_token: u32,
    eot_token: u32,
    no_timestamps_token: u32,
}

impl WhisperModel {
    /// # Errors
    ///
    /// Returns an error if the model cannot be loaded.
    ///
    /// # Panics
    ///
    /// Panics if file paths from the model cache cannot be converted to valid UTF-8
    /// or if a required path component is unexpectedly missing when extracting
    /// `config.json`, `tokenizer.json`, or `model.safetensors`.
    pub fn new(stt_model: &STTModel, force_cpu: bool) -> Result<Self> {
        info!("Loading Whisper {stt_model:?} model...");

        // Determine device
        let device = if !force_cpu && cuda_is_available() {
            info!("Using CUDA device");
            Device::new_cuda(0).context("Failed to create CUDA device")?
        } else {
            if force_cpu {
                info!("Using CPU (forced by user)");
            } else {
                info!("Using CPU (CUDA not available)");
            }
            Device::Cpu
        };

        // Get file paths from the unified download system
        let file_paths = crate::stt_models::download::get_model_file_paths(stt_model)?;

        // Extract the specific files we need
        let config_path = file_paths
            .iter()
            .find(|p| p.file_name().unwrap().to_str().unwrap() == "config.json")
            .ok_or_else(|| anyhow::anyhow!("config.json not found"))?;
        let tokenizer_path = file_paths
            .iter()
            .find(|p| p.file_name().unwrap().to_str().unwrap() == "tokenizer.json")
            .ok_or_else(|| anyhow::anyhow!("tokenizer.json not found"))?;
        let weights_path = file_paths
            .iter()
            .find(|p| p.file_name().unwrap().to_str().unwrap() == "model.safetensors")
            .ok_or_else(|| anyhow::anyhow!("model.safetensors not found"))?;

        info!("Model files downloaded successfully");

        // Load config
        let config_str =
            std::fs::read_to_string(config_path).context("Failed to read config file")?;
        let config: Config = serde_json::from_str(&config_str).context("Failed to parse config")?;

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?;

        // Load mel filters (built-in optimized version)
        let mel_bytes = match config.num_mel_bins {
            80 => include_bytes!("../data/melfilters.bytes").as_slice(),
            128 => include_bytes!("../data/melfilters128.bytes").as_slice(),
            nmel => return Err(anyhow::anyhow!("unexpected num_mel_bins {nmel}")),
        };
        let mut mel_filters = vec![0f32; mel_bytes.len() / 4];
        let mut cursor = Cursor::new(mel_bytes);
        cursor.read_f32_into::<LittleEndian>(&mut mel_filters)?;

        info!("Loading model weights...");
        // Load model weights using optimized memory mapping
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], m::DTYPE, &device)
                .context("Failed to load model weights")?
        };

        let model = Model::Normal(
            m::model::Whisper::load(&vb, config.clone())
                .context("Failed to create Whisper model")?,
        );

        // Get special tokens
        let sot_token = tokenizer
            .token_to_id(m::SOT_TOKEN)
            .ok_or_else(|| anyhow::anyhow!("Failed to get sot token"))?;
        let transcribe_token = tokenizer
            .token_to_id(m::TRANSCRIBE_TOKEN)
            .ok_or_else(|| anyhow::anyhow!("Failed to get transcribe token"))?;
        let eot_token = tokenizer
            .token_to_id(m::EOT_TOKEN)
            .ok_or_else(|| anyhow::anyhow!("Failed to get eot token"))?;
        let no_timestamps_token = tokenizer
            .token_to_id(m::NO_TIMESTAMPS_TOKEN)
            .ok_or_else(|| anyhow::anyhow!("Failed to get no_timestamps token"))?;

        info!("Whisper model loaded successfully");
        info!("Model device: {device:?}");

        Ok(Self {
            model,
            tokenizer,
            device,
            config,
            mel_filters,
            sot_token,
            transcribe_token,
            eot_token,
            no_timestamps_token,
        })
    }

    /// # Errors
    ///
    /// Returns an error if the audio data cannot be converted to a mel spectrogram.
    pub fn transcribe_audio(&mut self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        debug!("Transcribing audio with sample rate {sample_rate}Hz");

        // Resample to 16kHz if needed
        let audio = if sample_rate == SAMPLE_RATE {
            audio_data.to_vec()
        } else {
            warn!("Audio sample rate is {sample_rate}Hz, resampling to {SAMPLE_RATE}Hz");
            resample(audio_data, sample_rate, SAMPLE_RATE, ResampleQuality::Fast)?
        };

        // Use optimized Candle audio processing
        let mel = audio::pcm_to_mel(&self.config, &audio, &self.mel_filters);
        let mel_len = mel.len();
        let mel = Tensor::from_vec(
            mel,
            (
                1,
                self.config.num_mel_bins,
                mel_len / self.config.num_mel_bins,
            ),
            &self.device,
        )
        .context("Failed to create mel tensor")?;

        let result = self.run_segmented(&mel)?;

        Ok(result)
    }

    fn run_segmented(&mut self, mel: &Tensor) -> Result<String> {
        let (_, _, content_frames) = mel.dims3()?;
        let mut seek = 0;
        let mut all_text = Vec::new();

        let n_frames = 3000;

        while seek < content_frames {
            // Calculate segment size
            let segment_size = usize::min(content_frames - seek, n_frames);

            // Extract mel segment using narrow
            let mel_segment = mel.narrow(2, seek, segment_size)?;

            // Decode this segment with fallback temperatures
            let segment_result = self.decode_with_fallback(&mel_segment)?;

            if !segment_result.trim().is_empty() {
                all_text.push(segment_result);
            }

            seek += segment_size;
        }

        // Join all segment results
        let final_text = all_text.join(" ").trim().to_string();
        Ok(final_text)
    }

    fn decode_with_fallback(&mut self, mel_segment: &Tensor) -> Result<String> {
        let temperatures = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];

        for (i, &temperature) in temperatures.iter().enumerate() {
            match self.decode_simple(mel_segment, temperature) {
                Ok(result) => {
                    // Simple quality check - if we get reasonable text, use it
                    if !result.trim().is_empty() && result.len() > 5 {
                        return Ok(result);
                    }
                }
                Err(e) => {
                    if i == temperatures.len() - 1 {
                        return Err(e);
                    }
                }
            }
        }

        Ok(String::new())
    }

    fn decode_simple(&mut self, mel: &Tensor, temperature: f64) -> Result<String> {
        let audio_features = self.model.encoder_forward(mel, true)?;

        let suppress_tokens: Vec<f32> = (0..u32::try_from(self.config.vocab_size).unwrap())
            .map(|i| {
                if self.config.suppress_tokens.contains(&i) {
                    f32::NEG_INFINITY
                } else {
                    0f32
                }
            })
            .collect();
        let suppress_tokens_tensor = Tensor::new(suppress_tokens.as_slice(), &self.device)?;

        let sample_len = self.config.max_target_positions / 2;
        let mut tokens = vec![self.sot_token];

        // Add language token if available (optimize - check once)
        if let Some(en_token) = self.tokenizer.token_to_id("<|en|>") {
            tokens.push(en_token);
        }

        tokens.push(self.transcribe_token);
        tokens.push(self.no_timestamps_token);

        for i in 0..sample_len {
            let tokens_t = Tensor::new(tokens.as_slice(), mel.device())?;
            let tokens_t = tokens_t.unsqueeze(0)?;
            let ys = self
                .model
                .decoder_forward(&tokens_t, &audio_features, i == 0)?;

            // Skip no-speech probability calculation for performance
            let (_, seq_len, _) = ys.dims3()?;
            let logits = self
                .model
                .decoder_final_linear(&ys.i((..1, seq_len - 1..))?)?
                .i(0)?
                .i(0)?;

            // Apply suppress tokens
            let logits = logits.broadcast_add(&suppress_tokens_tensor)?;

            // Optimized token selection - greedy only for speed
            let next_token = if temperature > 0f64 {
                // Simplified sampling for performance
                let prs = softmax(&(&logits / temperature)?, 0)?;
                let logits_v: Vec<f32> = prs.to_vec1()?;
                logits_v
                    .iter()
                    .enumerate()
                    .max_by(|(_, u), (_, v)| u.total_cmp(v))
                    .map(|(i, _)| u32::try_from(i).unwrap())
                    .unwrap()
            } else {
                // Greedy decoding (fastest)
                let logits_v: Vec<f32> = logits.to_vec1()?;
                logits_v
                    .iter()
                    .enumerate()
                    .max_by(|(_, u), (_, v)| u.total_cmp(v))
                    .map(|(i, _)| u32::try_from(i).unwrap())
                    .unwrap()
            };

            tokens.push(next_token);

            if next_token == self.eot_token || tokens.len() > self.config.max_target_positions {
                break;
            }
        }

        // Decode tokens to text
        let text = self
            .tokenizer
            .decode(&tokens, true)
            .map_err(|e| anyhow::anyhow!("Tokenizer decode error: {e}"))?;

        let text = text.trim_start();
        Ok(text.to_string())
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
}
