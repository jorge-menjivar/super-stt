// SPDX-License-Identifier: GPL-3.0-only
//! Self-contained Voxtral inference on candle + tekken. No super-stt deps.
//!
//! Ported from the daemon's `stt_models/local/voxtral/model.rs` with the
//! super-stt wrappers (ModelInfoData, registry, the Transcribe trait, the
//! resample helper) removed: the architecture lives in
//! `candle_transformers::models::voxtral`, and the daemon resamples audio to
//! 16 kHz before sending, so this engine just loads files and decodes.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Error, Result};
use byteorder::{LittleEndian, ReadBytesExt};
use candle_core::{DType, Device, Tensor, utils};
use candle_nn::VarBuilder;
use candle_transformers::models::voxtral::{
    VoxtralCache, VoxtralConfig, VoxtralEncoderConfig, VoxtralForConditionalGeneration,
    VoxtralGenerationConfig, VoxtralLlamaConfig, audio,
};
use log::{info, warn};
use tekken::Tekkenizer;

const SAMPLE_RATE: u32 = 16000;
const MEL_FILTERS: &[u8] = include_bytes!("data/melfilters128.bytes");

/// A loaded Voxtral model ready to transcribe 16 kHz mono audio.
pub struct VoxtralEngine {
    model: VoxtralForConditionalGeneration,
    tokenizer: Tekkenizer,
    device: Device,
    audio_token_id: usize,
    cache: VoxtralCache,
    mel_filters: Vec<f32>,
}

impl VoxtralEngine {
    /// Load the model from a directory containing `config.json`, `tekken.json`,
    /// and the `*.safetensors` shards.
    pub fn load(model_dir: &Path, force_cpu: bool) -> Result<Self> {
        let files = resolve_files(model_dir)?;

        let device = if !force_cpu && utils::cuda_is_available() {
            info!("Voxtral: using CUDA device");
            Device::new_cuda(0).context("Failed to create CUDA device")?
        } else {
            info!("Voxtral: using CPU");
            Device::Cpu
        };

        let config = load_model_config(&files.config)?;
        let vb = load_model_weights(&files.weights, &device)?;
        let model = VoxtralForConditionalGeneration::new(&config, vb)?;
        let tokenizer = Tekkenizer::from_file(&files.tokenizer).map_err(Error::msg)?;
        let cache = VoxtralCache::new(true, DType::F16, &config.text_config, &device)?;

        let mut mel_filters = vec![0f32; MEL_FILTERS.len() / 4];
        Cursor::new(MEL_FILTERS).read_f32_into::<LittleEndian>(&mut mel_filters)?;

        let audio_token_id = config.audio_token_id;
        info!("Voxtral model loaded on {device:?}");
        Ok(Self {
            model,
            tokenizer,
            device,
            audio_token_id,
            cache,
            mel_filters,
        })
    }

    /// Device label for `GET /v1/status`.
    pub fn device_label(&self) -> &'static str {
        match &self.device {
            Device::Cpu => "cpu",
            Device::Cuda(_) => "cuda",
            Device::Metal(_) => "metal",
        }
    }

    /// Transcribe 16 kHz mono f32 audio. (The daemon resamples upstream.)
    pub fn transcribe(&mut self, audio_data: &[f32], sample_rate: u32) -> Result<String> {
        if sample_rate != SAMPLE_RATE {
            warn!(
                "Voxtral expects {SAMPLE_RATE}Hz; got {sample_rate}Hz (daemon should resample)"
            );
        }

        // VoxtralProcessor pads audio to a multiple of 480000 samples (30s).
        let chunk_size = 480_000;
        let padded_audio = if audio_data.len().is_multiple_of(chunk_size) {
            audio_data.to_vec()
        } else {
            let target = ((audio_data.len() / chunk_size) + 1) * chunk_size;
            let mut p = audio_data.to_vec();
            p.resize(target, 0.0);
            p
        };

        let audio_features = audio::extract_features(&padded_audio, &self.mel_filters, &self.device)?;
        let (result, _tokens) = transcribe_with_voxtral(
            &self.model,
            &self.tokenizer,
            &audio_features,
            self.audio_token_id,
            &self.device,
            &self.cache.clone(),
        )?;
        Ok(result)
    }
}

struct ModelFiles {
    config: PathBuf,
    tokenizer: PathBuf,
    weights: Vec<PathBuf>,
}

fn resolve_files(dir: &Path) -> Result<ModelFiles> {
    let config = dir.join("config.json");
    anyhow::ensure!(config.exists(), "config.json not found in {}", dir.display());
    let tokenizer = dir.join("tekken.json");
    anyhow::ensure!(
        tokenizer.exists(),
        "tekken.json not found in {}",
        dir.display()
    );
    let mut weights = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let p = entry?.path();
        if p.extension().and_then(|s| s.to_str()) == Some("safetensors") {
            weights.push(p);
        }
    }
    anyhow::ensure!(!weights.is_empty(), "no .safetensors in {}", dir.display());
    weights.sort();
    Ok(ModelFiles {
        config,
        tokenizer,
        weights,
    })
}

/// Clean up Voxtral output formatting artifacts. (Ported verbatim.)
fn post_process_transcription(text: &str) -> Result<String> {
    let mut cleaned = text.trim().to_string();
    if cleaned.starts_with("\"'") || cleaned.starts_with("'\"") {
        cleaned = cleaned
            .trim_start_matches("\"'")
            .trim_start_matches("'\"")
            .trim()
            .to_string();
    }
    if cleaned.starts_with('\'') {
        cleaned = cleaned[1..].trim().to_string();
    }
    cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned = cleaned.replace(" \"' ", " ").replace(" '\" ", " ");
    if cleaned == "." || cleaned.trim().is_empty() {
        anyhow::bail!("mel feature generation produced invalid output");
    }
    cleaned = cleaned
        .trim_end_matches('\'')
        .trim_end_matches('"')
        .to_string();
    Ok(cleaned)
}

fn transcribe_with_voxtral(
    model: &VoxtralForConditionalGeneration,
    tokenizer: &Tekkenizer,
    audio_features: &Tensor,
    audio_token_id: usize,
    device: &Device,
    cache: &VoxtralCache,
) -> Result<(String, Vec<u32>)> {
    let audio_dims = audio_features.dims();
    anyhow::ensure!(
        audio_dims.len() == 3,
        "audio features must be 3D (batch, mels, time), got {audio_dims:?}"
    );
    anyhow::ensure!(
        audio_dims[1] == 128,
        "audio features must have 128 mel bins, got {}",
        audio_dims[1]
    );

    // <s>[INST][BEGIN_AUDIO][AUDIO]*N[/INST]lang:en[TRANSCRIBE]
    let mut input_tokens = vec![1u32, 3u32, 25u32];
    let batch_size = audio_features.dim(0)?;
    let tokens_per_chunk = 375;
    let num_audio_tokens = batch_size * tokens_per_chunk;
    for _ in 0..num_audio_tokens {
        #[allow(clippy::cast_possible_truncation)]
        input_tokens.push(audio_token_id as u32);
    }
    input_tokens.extend_from_slice(&[4u32, 9909u32, 1058u32, 1262u32, 34u32]);

    let input_len = input_tokens.len();
    let input_ids = Tensor::new(input_tokens, device)?.unsqueeze(0)?;

    let config = VoxtralGenerationConfig {
        max_new_tokens: 1000,
        temperature: 0.0,
        top_p: None,
        device: device.clone(),
        cache: Some(cache.clone()),
    };

    let generated_tokens = model
        .generate(&input_ids, Some(audio_features), config)
        .map_err(|e| anyhow::anyhow!("Failed to generate tokens: {e}"))?;

    let new_tokens = if generated_tokens.len() > input_len {
        &generated_tokens[input_len..]
    } else {
        &generated_tokens
    };

    let decoded_text = tokenizer
        .decode(new_tokens, tekken::SpecialTokenPolicy::Ignore)
        .map_err(|e| anyhow::anyhow!("Failed to decode tokens: {e}"))?;

    let transcription = post_process_transcription(&decoded_text)?;
    Ok((transcription, new_tokens.to_vec()))
}

fn load_model_weights<'a>(model_files: &'a [PathBuf], device: &Device) -> Result<VarBuilder<'a>> {
    let vb = unsafe { VarBuilder::from_mmaped_safetensors(model_files, DType::F16, device)? };
    Ok(vb)
}

fn load_model_config(config_file: &Path) -> Result<VoxtralConfig> {
    let config_str = std::fs::read_to_string(config_file)?;
    let json: serde_json::Value =
        serde_json::from_str(&config_str).context("Failed to parse config.json")?;

    let audio_token_id = json
        .get("audio_token_id")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(24);

    Ok(VoxtralConfig {
        audio_config: parse_audio_config(&json)?,
        text_config: parse_text_config(&json)?,
        audio_token_id,
        projector_hidden_act: json
            .get("projector_hidden_act")
            .and_then(|v| v.as_str())
            .unwrap_or("gelu")
            .to_string(),
    })
}

fn parse_audio_config(json: &serde_json::Value) -> Result<VoxtralEncoderConfig> {
    let a = json
        .get("audio_config")
        .ok_or_else(|| anyhow::anyhow!("Missing audio_config"))?;
    let u = |k: &str, d: usize| {
        a.get(k)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(d)
    };
    let f = |k: &str, d: f64| a.get(k).and_then(serde_json::Value::as_f64).unwrap_or(d);
    Ok(VoxtralEncoderConfig {
        vocab_size: u("vocab_size", 51866),
        hidden_size: u("hidden_size", 1280),
        num_hidden_layers: u("num_hidden_layers", 32),
        num_attention_heads: u("num_attention_heads", 20),
        num_key_value_heads: u("num_key_value_heads", 20),
        intermediate_size: u("intermediate_size", 5120),
        dropout: f("dropout", 0.0),
        attention_dropout: f("attention_dropout", 0.0),
        activation_dropout: f("activation_dropout", 0.0),
        activation_function: a
            .get("activation_function")
            .and_then(|v| v.as_str())
            .unwrap_or("gelu")
            .to_string(),
        max_source_positions: u("max_source_positions", 1500),
        layerdrop: f("layerdrop", 0.0),
        initializer_range: f("initializer_range", 0.02),
        scale_embedding: a
            .get("scale_embedding")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        num_mel_bins: u("num_mel_bins", 128),
        head_dim: u("head_dim", 64),
    })
}

#[cfg(feature = "flash-attn")]
const fn use_flash_attn() -> bool {
    true
}
#[cfg(not(feature = "flash-attn"))]
const fn use_flash_attn() -> bool {
    false
}

fn parse_text_config(json: &serde_json::Value) -> Result<VoxtralLlamaConfig> {
    let t = json
        .get("text_config")
        .ok_or_else(|| anyhow::anyhow!("Missing text_config"))?;
    let u = |k: &str, d: usize| {
        t.get(k)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(d)
    };
    #[allow(clippy::cast_possible_truncation)]
    let rope_theta = t
        .get("rope_theta")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(100_000_000.0) as f32;
    Ok(VoxtralLlamaConfig {
        vocab_size: u("vocab_size", 131_072),
        hidden_size: u("hidden_size", 3072),
        intermediate_size: u("intermediate_size", 8192),
        num_hidden_layers: u("num_hidden_layers", 30),
        num_attention_heads: u("num_attention_heads", 32),
        num_key_value_heads: u("num_key_value_heads", 8),
        head_dim: t
            .get("head_dim")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| usize::try_from(v).ok()),
        rms_norm_eps: t
            .get("rms_norm_eps")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1e-5),
        rope_theta,
        max_position_embeddings: u("max_position_embeddings", 131_072),
        use_flash_attn: use_flash_attn(),
        tie_word_embeddings: t
            .get("attention_bias")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}
