// SPDX-License-Identifier: GPL-3.0-only
//! Self-contained Whisper inference on candle + tokenizers. No super-stt deps.
//!
//! Ported from `reference/in-tree-models/local/whisper/model.rs` with the
//! super-stt wrappers stripped. Two correctness fixes vs. the reference:
//!
//! 1. The decoder prompt is built by model capability — `.en` variants emit
//!    `[sot, no_timestamps]` (no language or task token), matching OpenAI's
//!    reference decoder. The original code unconditionally pushed
//!    `<|transcribe|>`, which produced empty/garbage output for `.en` models.
//! 2. The temperature-fallback no longer rejects results shorter than 6
//!    characters; any non-empty decode is accepted.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use candle_core::utils::cuda_is_available;
use candle_core::{Device, IndexOp, Tensor};
use candle_nn::{VarBuilder, ops::softmax};
use candle_transformers::models::whisper::{self as m, Config, audio};
use log::{debug, info, warn};
use tokenizers::Tokenizer;

const SAMPLE_RATE: u32 = 16000;
const MEL_FILTERS_80: &[u8] = include_bytes!("data/melfilters.bytes");

pub struct WhisperEngine {
    model: m::model::Whisper,
    tokenizer: Tokenizer,
    device: Device,
    config: Config,
    mel_filters: Vec<f32>,
    sot_token: u32,
    transcribe_token: Option<u32>,
    eot_token: u32,
    no_timestamps_token: u32,
    is_english_only: bool,
}

impl WhisperEngine {
    /// Load a Whisper model from a directory containing `config.json`,
    /// `tokenizer.json`, and `model.safetensors`. `model_name` is the wire name
    /// from the load request (e.g. `whisper-tiny.en`); a `.en` suffix marks
    /// the variant as English-only. We can't read this from the HF tokenizer:
    /// `openai/whisper-*.en` keeps the `<|en|>` token in `tokenizer.json` even
    /// though the model was trained never to emit it.
    ///
    /// # Errors
    ///
    /// Returns an error if the model directory is missing expected files, the
    /// requested device cannot be initialized, the config/tokenizer can't be
    /// parsed, or weights fail to load.
    pub fn load(model_dir: &Path, model_name: &str, force_cpu: bool) -> Result<Self> {
        let files = resolve_files(model_dir)?;
        // `.en` is a stable suffix on every OpenAI English-only variant
        // (whisper-tiny.en, base.en, …); the case-sensitive match is deliberate.
        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let is_english_only = model_name.ends_with(".en");

        let device = if !force_cpu && cuda_is_available() {
            info!("Whisper: using CUDA device");
            Device::new_cuda(0).context("Failed to create CUDA device")?
        } else {
            if force_cpu {
                info!("Whisper: using CPU (forced)");
            } else {
                info!("Whisper: using CPU (CUDA not available)");
            }
            Device::Cpu
        };

        let config_str = std::fs::read_to_string(&files.config)
            .with_context(|| format!("read {}", files.config.display()))?;
        let config: Config = serde_json::from_str(&config_str).context("parse config.json")?;

        let tokenizer = Tokenizer::from_file(&files.tokenizer)
            .map_err(|e| anyhow::anyhow!("load tokenizer: {e}"))?;

        // Only 80-bin filters are bundled; 128 is large-v3 territory and out of
        // scope for this backend.
        let mel_bytes = match config.num_mel_bins {
            80 => MEL_FILTERS_80,
            n => anyhow::bail!("unsupported num_mel_bins {n}; this backend bundles 80 only"),
        };
        let mut mel_filters = vec![0f32; mel_bytes.len() / 4];
        Cursor::new(mel_bytes).read_f32_into::<LittleEndian>(&mut mel_filters)?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[&files.weights], m::DTYPE, &device)
                .context("load model weights")?
        };
        let model = m::model::Whisper::load(&vb, config.clone()).context("build Whisper model")?;

        let sot_token = tokenizer
            .token_to_id(m::SOT_TOKEN)
            .ok_or_else(|| anyhow::anyhow!("missing sot token"))?;
        let eot_token = tokenizer
            .token_to_id(m::EOT_TOKEN)
            .ok_or_else(|| anyhow::anyhow!("missing eot token"))?;
        let no_timestamps_token = tokenizer
            .token_to_id(m::NO_TIMESTAMPS_TOKEN)
            .ok_or_else(|| anyhow::anyhow!("missing no_timestamps token"))?;

        let transcribe_token = if is_english_only {
            None
        } else {
            Some(
                tokenizer
                    .token_to_id(m::TRANSCRIBE_TOKEN)
                    .ok_or_else(|| anyhow::anyhow!("missing transcribe token"))?,
            )
        };

        info!("Whisper model loaded on {device:?} (english_only={is_english_only})");
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
            is_english_only,
        })
    }

    pub fn device_label(&self) -> &'static str {
        match &self.device {
            Device::Cpu => "cpu",
            Device::Cuda(_) => "cuda",
            Device::Metal(_) => "metal",
        }
    }

    pub fn is_english_only(&self) -> bool {
        self.is_english_only
    }

    /// One-shot transcription. Runs segmented decoding and returns the joined
    /// transcription.
    ///
    /// # Errors
    ///
    /// Forwards any inference or decode error from [`Self::transcribe_streaming`].
    pub fn transcribe(
        &mut self,
        audio_data: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String> {
        self.transcribe_streaming(audio_data, sample_rate, language, |_| {})
    }

    /// Streaming variant — `on_segment` is called with the accumulated
    /// transcription after each 30 s segment finishes decoding. Returns the
    /// final transcription.
    ///
    /// # Errors
    ///
    /// Returns an error if mel extraction or decoding fails, or — for
    /// multilingual models — if `language` is not a known whisper code.
    pub fn transcribe_streaming<F: FnMut(&str)>(
        &mut self,
        audio_data: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        mut on_segment: F,
    ) -> Result<String> {
        debug!("transcribe: {} samples @ {sample_rate} Hz", audio_data.len());

        if sample_rate != SAMPLE_RATE {
            warn!("Whisper expects {SAMPLE_RATE} Hz; got {sample_rate} Hz (daemon should resample)");
        }

        let mel = audio::pcm_to_mel(&self.config, audio_data, &self.mel_filters);
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
        .context("build mel tensor")?;

        self.run_segmented(&mel, language, &mut on_segment)
    }

    fn run_segmented<F: FnMut(&str)>(
        &mut self,
        mel: &Tensor,
        language: Option<&str>,
        on_segment: &mut F,
    ) -> Result<String> {
        let (_, _, content_frames) = mel.dims3()?;
        let mut seek = 0;
        let mut segments: Vec<String> = Vec::new();
        let n_frames = 3000;

        while seek < content_frames {
            let segment_size = usize::min(content_frames - seek, n_frames);
            let mel_segment = mel.narrow(2, seek, segment_size)?;

            let segment_text = self.decode_with_fallback(&mel_segment, language)?;
            if !segment_text.trim().is_empty() {
                segments.push(segment_text);
                let joined = segments.join(" ").trim().to_string();
                on_segment(&joined);
            }
            seek += segment_size;
        }

        Ok(segments.join(" ").trim().to_string())
    }

    fn decode_with_fallback(
        &mut self,
        mel_segment: &Tensor,
        language: Option<&str>,
    ) -> Result<String> {
        let temperatures = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
        let mut last_err: Option<anyhow::Error> = None;

        for &t in &temperatures {
            match self.decode_simple(mel_segment, t, language) {
                Ok(result) if !result.trim().is_empty() => return Ok(result),
                Ok(_) => {}
                Err(e) => last_err = Some(e),
            }
        }

        if let Some(err) = last_err {
            Err(err)
        } else {
            Ok(String::new())
        }
    }

    fn decode_simple(
        &mut self,
        mel: &Tensor,
        temperature: f64,
        language: Option<&str>,
    ) -> Result<String> {
        let audio_features = self.model.encoder.forward(mel, true)?;

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

        if !self.is_english_only {
            let lang_code = language.unwrap_or("en");
            let lang_tok = format!("<|{lang_code}|>");
            let lang_id = self
                .tokenizer
                .token_to_id(&lang_tok)
                .ok_or_else(|| anyhow::anyhow!("unsupported_language"))?;
            tokens.push(lang_id);
            tokens.push(
                self.transcribe_token
                    .expect("multilingual model has transcribe_token"),
            );
        }
        tokens.push(self.no_timestamps_token);

        for i in 0..sample_len {
            let tokens_t = Tensor::new(tokens.as_slice(), mel.device())?.unsqueeze(0)?;
            let ys = self
                .model
                .decoder
                .forward(&tokens_t, &audio_features, i == 0)?;

            let (_, seq_len, _) = ys.dims3()?;
            let logits = self
                .model
                .decoder
                .final_linear(&ys.i((..1, seq_len - 1..))?)?
                .i(0)?
                .i(0)?;
            let logits = logits.broadcast_add(&suppress_tokens_tensor)?;

            let next_token = if temperature > 0f64 {
                let prs = softmax(&(&logits / temperature)?, 0)?;
                let v: Vec<f32> = prs.to_vec1()?;
                v.iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.total_cmp(b))
                    .map(|(i, _)| u32::try_from(i).unwrap())
                    .unwrap()
            } else {
                let v: Vec<f32> = logits.to_vec1()?;
                v.iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.total_cmp(b))
                    .map(|(i, _)| u32::try_from(i).unwrap())
                    .unwrap()
            };

            tokens.push(next_token);
            if next_token == self.eot_token || tokens.len() > self.config.max_target_positions {
                break;
            }
        }

        let text = self
            .tokenizer
            .decode(&tokens, true)
            .map_err(|e| anyhow::anyhow!("tokenizer decode: {e}"))?;
        Ok(text.trim_start().to_string())
    }
}

struct ModelFiles {
    config: PathBuf,
    tokenizer: PathBuf,
    weights: PathBuf,
}

fn resolve_files(dir: &Path) -> Result<ModelFiles> {
    let config = dir.join("config.json");
    anyhow::ensure!(
        config.exists(),
        "config.json not found in {}",
        dir.display()
    );
    let tokenizer = dir.join("tokenizer.json");
    anyhow::ensure!(
        tokenizer.exists(),
        "tokenizer.json not found in {}",
        dir.display()
    );
    let weights = dir.join("model.safetensors");
    anyhow::ensure!(
        weights.exists(),
        "model.safetensors not found in {}",
        dir.display()
    );
    Ok(ModelFiles {
        config,
        tokenizer,
        weights,
    })
}
