# Whisper Subprocess Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a standalone `backends/whisper/` subprocess backend that serves the 9 pre-refactor Whisper variants over the `/v1` contract, with CPU + CUDA support, SSE streaming previews, and fixes for two latent bugs that break the `.en` variants and short utterances.

**Architecture:** New `backends/whisper/` directory excluded from the workspace, structured as a near-clone of `backends/voxtral/`. An axum HTTP/1.1 server bound to `SUPER_STT_BACKEND_SOCKET` exposes `/v1/{ping,status,load,transcribe,cancel}`. Inference is ported from `reference/in-tree-models/local/whisper/model.rs` with the super-stt wrappers stripped, the decoder prompt built by model capability (multilingual vs `.en`), and the broken length floor in the temperature fallback removed.

**Tech Stack:** Rust 2024 edition, axum 0.8 + hyper 1, candle (`huggingface/candle` rev `bf9e950cd300493b8768d77baec1a23ad6d44d94` — same pin the voxtral backend uses), `tokenizers = "0.23"`, `byteorder = "1.5"`, `tokio = "1"`. CUDA via feature flag.

**Spec:** `docs/superpowers/specs/2026-05-29-whisper-subprocess-backend-design.md`

---

## File map

| Path | Action | Responsibility |
|---|---|---|
| `backends/whisper/Cargo.toml` | Create | Standalone crate manifest; deps mirror voxtral with `tekken-rs` → `tokenizers`; declares both `[lib]` and `[[bin]]` |
| `backends/whisper/backend.toml` | Create | Backend manifest: identity + 9 model entries |
| `backends/whisper/src/lib.rs` | Create | `pub mod inference;` — single re-export so tests can use the engine without `include!` gymnastics |
| `backends/whisper/src/main.rs` | Create | axum `/v1` server, status state machine, request → engine glue, SSE streaming |
| `backends/whisper/src/inference.rs` | Create | `WhisperEngine`: model load, mel extraction, segmented decode |
| `backends/whisper/src/data/melfilters.bytes` | Copy | 80-bin mel filter coefficients (used by `audio::pcm_to_mel`) |
| `backends/whisper/tests/data/jfk.wav` | Create | ~11 s public-domain WAV used by the smoke test |
| `backends/whisper/tests/smoke.rs` | Create | CPU smoke tests for tiny + tiny.en, `#[ignore]` by default |
| `Cargo.toml` (root) | Modify | Add `"backends/whisper"` to `[workspace].exclude` |
| `justfile` | Modify | Add `build-whisper-backend` recipe + install stanza |

---

## Task 1: Scaffold the crate and exclude from workspace

**Files:**
- Create: `backends/whisper/Cargo.toml`
- Modify: `Cargo.toml` (root)

- [ ] **Step 1: Create `backends/whisper/Cargo.toml`**

Write the file with this exact content:

```toml
# SPDX-License-Identifier: GPL-3.0-only
# Standalone Whisper subprocess backend. Depends only on third-party crates —
# no super-stt code. Excluded from the workspace (own Cargo.lock).
#
# Cargo auto-discovers two targets: the `super_stt_backend_whisper` lib from
# src/lib.rs (used by `tests/smoke.rs`) and the `super-stt-backend-whisper`
# bin from src/main.rs (the subprocess entrypoint).
[package]
    name = "super-stt-backend-whisper"
    version = "0.1.0"
    edition = "2024"
    license = "GPL-3.0-only"
    publish = false

[dependencies]
    anyhow = "1"
    axum = { version = "0.8", default-features = false, features = ["json", "tokio"] }
    byteorder = "1.5"
    # Pinned to the same candle revision the voxtral backend uses, so the
    # candle_transformers Whisper API matches.
    candle-core = { git = "https://github.com/huggingface/candle.git", rev = "bf9e950cd300493b8768d77baec1a23ad6d44d94" }
    candle-nn = { git = "https://github.com/huggingface/candle.git", rev = "bf9e950cd300493b8768d77baec1a23ad6d44d94" }
    candle-transformers = { git = "https://github.com/huggingface/candle.git", rev = "bf9e950cd300493b8768d77baec1a23ad6d44d94" }
    env_logger = "0.11"
    futures = "0.3"
    hyper = { version = "1", features = ["http1", "server"] }
    hyper-util = { version = "0.1", features = ["server", "service", "tokio", "http1"] }
    log = "0.4"
    serde = { version = "1", features = ["derive"] }
    serde_json = "1"
    tokenizers = { version = "0.23", default-features = false, features = ["onig"] }
    tokio = { version = "1", features = ["macros", "rt-multi-thread", "net", "fs", "signal", "sync"] }

[dev-dependencies]
    dirs = "5"
    hound = "3.5"
    reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "blocking", "stream"] }

[features]
    default = []
    cuda = ["candle-core/cuda", "candle-nn/cuda", "candle-transformers/cuda"]
    cudnn = ["candle-core/cudnn"]
    flash-attn = ["candle-transformers/flash-attn"]
```

- [ ] **Step 2: Add the crate to root workspace `exclude`**

In `/home/jorge/rust_projects/super-stt/Cargo.toml`, find the `exclude` line:

```toml
    exclude = ["backends/mistral", "backends/openai", "backends/voxtral"]
```

Replace with:

```toml
    exclude = ["backends/mistral", "backends/openai", "backends/voxtral", "backends/whisper"]
```

- [ ] **Step 3: Verify workspace metadata still resolves**

Run: `cargo metadata --no-deps --manifest-path /home/jorge/rust_projects/super-stt/Cargo.toml > /dev/null`
Expected: Exit 0, no output.

- [ ] **Step 4: Verify the new manifest parses (no source files yet — expect a parse-only check)**

Run: `cargo read-manifest --manifest-path /home/jorge/rust_projects/super-stt/backends/whisper/Cargo.toml > /dev/null`
Expected: Exit 0, no output.

- [ ] **Step 5: Commit**

```bash
git add backends/whisper/Cargo.toml Cargo.toml
git commit -m "Scaffold backends/whisper crate, exclude from workspace"
```

---

## Task 2: Author `backend.toml`

**Files:**
- Create: `backends/whisper/backend.toml`

- [ ] **Step 1: Create `backends/whisper/backend.toml`**

Write the file with this exact content (the 99-language list is one long line):

```toml
# SPDX-License-Identifier: GPL-3.0-only
# Whisper (local) subprocess backend. See docs/protocol/backend/.

[backend]
    source     = "github.com/super-stt/whisper"
    name       = "Whisper (local)"
    version    = "0.1.0"
    kind       = "subprocess"
    entrypoint = "super-stt-backend-whisper"
    contract   = "v1"

[network]
    allowed_hosts = []

[[models]]
    name                   = "whisper-tiny"
    provider               = "local_whisper"
    multilingual           = true
    primary_language       = "en"
    supported_languages    = ["en","zh","de","es","ru","ko","fr","ja","pt","tr","pl","ca","nl","ar","sv","it","id","hi","fi","vi","he","uk","el","ms","cs","ro","da","hu","ta","no","th","ur","hr","bg","lt","la","mi","ml","cy","sk","te","fa","lv","bn","sr","az","sl","kn","et","mk","br","eu","is","hy","ne","mn","bs","kk","sq","sw","gl","mr","pa","si","km","sn","yo","so","af","oc","ka","be","tg","sd","gu","am","yi","lo","uz","fo","ht","ps","tk","nn","mt","sa","lb","my","bo","tl","mg","as","tt","haw","ln","ha","ba","jw","su","yue"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 1073741824
    processing_interval_ms = 1000

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-tiny"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-tiny"

[[models]]
    name                   = "whisper-tiny.en"
    provider               = "local_whisper"
    multilingual           = false
    primary_language       = "en"
    supported_languages    = ["en"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 1073741824
    processing_interval_ms = 1000

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-tiny.en"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-tiny.en"

[[models]]
    name                   = "whisper-base"
    provider               = "local_whisper"
    multilingual           = true
    primary_language       = "en"
    supported_languages    = ["en","zh","de","es","ru","ko","fr","ja","pt","tr","pl","ca","nl","ar","sv","it","id","hi","fi","vi","he","uk","el","ms","cs","ro","da","hu","ta","no","th","ur","hr","bg","lt","la","mi","ml","cy","sk","te","fa","lv","bn","sr","az","sl","kn","et","mk","br","eu","is","hy","ne","mn","bs","kk","sq","sw","gl","mr","pa","si","km","sn","yo","so","af","oc","ka","be","tg","sd","gu","am","yi","lo","uz","fo","ht","ps","tk","nn","mt","sa","lb","my","bo","tl","mg","as","tt","haw","ln","ha","ba","jw","su","yue"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 1073741824
    processing_interval_ms = 1500

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-base"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-base"

[[models]]
    name                   = "whisper-base.en"
    provider               = "local_whisper"
    multilingual           = false
    primary_language       = "en"
    supported_languages    = ["en"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 1073741824
    processing_interval_ms = 1500

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-base.en"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-base.en"

[[models]]
    name                   = "whisper-small"
    provider               = "local_whisper"
    multilingual           = true
    primary_language       = "en"
    supported_languages    = ["en","zh","de","es","ru","ko","fr","ja","pt","tr","pl","ca","nl","ar","sv","it","id","hi","fi","vi","he","uk","el","ms","cs","ro","da","hu","ta","no","th","ur","hr","bg","lt","la","mi","ml","cy","sk","te","fa","lv","bn","sr","az","sl","kn","et","mk","br","eu","is","hy","ne","mn","bs","kk","sq","sw","gl","mr","pa","si","km","sn","yo","so","af","oc","ka","be","tg","sd","gu","am","yi","lo","uz","fo","ht","ps","tk","nn","mt","sa","lb","my","bo","tl","mg","as","tt","haw","ln","ha","ba","jw","su","yue"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 2147483648
    processing_interval_ms = 2000

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-small"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-small"

[[models]]
    name                   = "whisper-small.en"
    provider               = "local_whisper"
    multilingual           = false
    primary_language       = "en"
    supported_languages    = ["en"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 2147483648
    processing_interval_ms = 2000

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-small.en"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-small.en"

[[models]]
    name                   = "whisper-medium"
    provider               = "local_whisper"
    multilingual           = true
    primary_language       = "en"
    supported_languages    = ["en","zh","de","es","ru","ko","fr","ja","pt","tr","pl","ca","nl","ar","sv","it","id","hi","fi","vi","he","uk","el","ms","cs","ro","da","hu","ta","no","th","ur","hr","bg","lt","la","mi","ml","cy","sk","te","fa","lv","bn","sr","az","sl","kn","et","mk","br","eu","is","hy","ne","mn","bs","kk","sq","sw","gl","mr","pa","si","km","sn","yo","so","af","oc","ka","be","tg","sd","gu","am","yi","lo","uz","fo","ht","ps","tk","nn","mt","sa","lb","my","bo","tl","mg","as","tt","haw","ln","ha","ba","jw","su","yue"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 5368709120
    processing_interval_ms = 2000

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-medium"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-medium"

[[models]]
    name                   = "whisper-medium.en"
    provider               = "local_whisper"
    multilingual           = false
    primary_language       = "en"
    supported_languages    = ["en"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 5368709120
    processing_interval_ms = 2000

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-medium.en"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-medium.en"

[[models]]
    name                   = "whisper-large"
    provider               = "local_whisper"
    multilingual           = true
    primary_language       = "en"
    supported_languages    = ["en","zh","de","es","ru","ko","fr","ja","pt","tr","pl","ca","nl","ar","sv","it","id","hi","fi","vi","he","uk","el","ms","cs","ro","da","hu","ta","no","th","ur","hr","bg","lt","la","mi","ml","cy","sk","te","fa","lv","bn","sr","az","sl","kn","et","mk","br","eu","is","hy","ne","mn","bs","kk","sq","sw","gl","mr","pa","si","km","sn","yo","so","af","oc","ka","be","tg","sd","gu","am","yi","lo","uz","fo","ht","ps","tk","nn","mt","sa","lb","my","bo","tl","mg","as","tt","haw","ln","ha","ba","jw","su","yue"]
    supported_devices      = ["cpu", "cuda"]
    estimated_vram_bytes   = 10737418240
    processing_interval_ms = 5000

    [[models.files]]
        source   = "huggingface"
        repo     = "openai/whisper-large"
        revision = "main"
        files    = ["config.json", "tokenizer.json", "model.safetensors"]
        dest     = "models/whisper-large"
```

- [ ] **Step 2: Validate TOML syntax**

Run: `cargo metadata --no-deps --manifest-path /home/jorge/rust_projects/super-stt/backends/whisper/Cargo.toml > /dev/null`
Expected: Exit 0. (This doesn't validate the `backend.toml` schema — that lives in the daemon — but it confirms file presence doesn't break the package.)

Optionally, also run a TOML lint:
Run: `python3 -c "import tomllib; tomllib.loads(open('/home/jorge/rust_projects/super-stt/backends/whisper/backend.toml').read()); print('ok')"`
Expected: `ok`.

- [ ] **Step 3: Commit**

```bash
git add backends/whisper/backend.toml
git commit -m "Add Whisper backend manifest with 9 models"
```

---

## Task 3: Copy mel-filter coefficients

**Files:**
- Create: `backends/whisper/src/data/melfilters.bytes`

- [ ] **Step 1: Create the data directory and copy the file**

```bash
mkdir -p /home/jorge/rust_projects/super-stt/backends/whisper/src/data
cp /home/jorge/rust_projects/super-stt/reference/in-tree-models/local/data/melfilters.bytes \
   /home/jorge/rust_projects/super-stt/backends/whisper/src/data/melfilters.bytes
```

- [ ] **Step 2: Verify the file is exactly 64320 bytes (the 80-bin coefficient table)**

Run: `stat -c '%s' /home/jorge/rust_projects/super-stt/backends/whisper/src/data/melfilters.bytes`
Expected output: `64320`

- [ ] **Step 3: Commit**

```bash
git add backends/whisper/src/data/melfilters.bytes
git commit -m "Bundle 80-bin mel filters for Whisper backend"
```

---

## Task 4: Port `WhisperEngine` into `src/inference.rs` + add `src/lib.rs` (with bug fixes)

**Files:**
- Create: `backends/whisper/src/lib.rs`
- Create: `backends/whisper/src/inference.rs`

This task ports `reference/in-tree-models/local/whisper/model.rs` and applies both spec bug fixes (capability-based prompt, drop the length floor). A tiny `lib.rs` re-exports the module so the integration test can use it directly.

- [ ] **Step 1: Create `backends/whisper/src/lib.rs`**

Write the file with this exact content:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Library face of the Whisper subprocess backend. The actual binary lives in
//! `src/main.rs`; this lib exists so integration tests can `use` the engine
//! directly without round-tripping through the `/v1` socket.

#![allow(clippy::doc_markdown)]

pub mod inference;
```

- [ ] **Step 2: Create `backends/whisper/src/inference.rs`**

Write the file with this exact content:

```rust
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
    transcribe_token: Option<u32>, // None for `.en` models
    eot_token: u32,
    no_timestamps_token: u32,
    is_english_only: bool,
}

impl WhisperEngine {
    /// Load a Whisper model from a directory containing `config.json`,
    /// `tokenizer.json`, and `model.safetensors`.
    pub fn load(model_dir: &Path, force_cpu: bool) -> Result<Self> {
        let files = resolve_files(model_dir)?;

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

        // `.en` tokenizers omit language tokens entirely; use that as the
        // capability discriminator.
        let is_english_only = tokenizer.token_to_id("<|en|>").is_none();
        let transcribe_token = if is_english_only {
            None
        } else {
            Some(
                tokenizer
                    .token_to_id(m::TRANSCRIBE_TOKEN)
                    .ok_or_else(|| anyhow::anyhow!("missing transcribe token"))?,
            )
        };

        info!(
            "Whisper model loaded on {device:?} (english_only={is_english_only})"
        );
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

    /// One-shot transcription. Resamples-checks audio, runs segmented decoding,
    /// returns the joined transcription.
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
    pub fn transcribe_streaming<F: FnMut(&str)>(
        &mut self,
        audio_data: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        mut on_segment: F,
    ) -> Result<String> {
        debug!("transcribe: {} samples @ {sample_rate} Hz", audio_data.len());

        if sample_rate != SAMPLE_RATE {
            warn!(
                "Whisper expects {SAMPLE_RATE} Hz; got {sample_rate} Hz (daemon should resample)"
            );
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
        let n_frames = 3000; // 30 s at 100 frames/s

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
                Ok(_) => continue, // empty decode: bump temperature
                Err(e) => last_err = Some(e),
            }
        }

        // Every temperature produced an empty decode (or each errored). If we
        // saw at least one clean empty decode, return empty rather than
        // surfacing the error — Whisper produces no text for pure silence.
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

        // Build the prompt by capability — see module docstring fix #1.
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
            let ys = self.model.decoder.forward(&tokens_t, &audio_features, i == 0)?;

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
```

- [ ] **Step 3: Verify lib + inference compile together**

Run:
```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/backends/whisper/Cargo.toml --lib
```
Expected: the lib crate compiles. (First run pulls candle from git — several minutes.) Auto-discovery only picks up `[[bin]]` when `src/main.rs` exists, so this run builds only the lib + its deps. Any compile errors inside `inference.rs` must be fixed inline before continuing.

- [ ] **Step 4: Commit**

```bash
git add backends/whisper/src/lib.rs backends/whisper/src/inference.rs
git commit -m "Add WhisperEngine: ported inference, .en prompt fix, drop length floor"
```

---

## Task 5: Author `main.rs` with /v1 routes + SSE streaming

**Files:**
- Create: `backends/whisper/src/main.rs`

- [ ] **Step 1: Create `backends/whisper/src/main.rs`**

Write the file with this exact content:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! Whisper subprocess backend: serves the Super STT `/v1` contract over a
//! pathname Unix socket (`SUPER_STT_BACKEND_SOCKET`), loading the model from
//! `SUPER_STT_BACKEND_DIR/models/<name>`. Self-contained — no super-stt deps.

#![allow(clippy::doc_markdown)]

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use futures::stream::{Stream, StreamExt};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::sync::mpsc;

use super_stt_backend_whisper::inference::WhisperEngine;

#[derive(Clone, Copy)]
enum LoadState {
    Starting,
    Loading,
    Ready,
    Error,
}

impl LoadState {
    fn as_str(self) -> &'static str {
        match self {
            LoadState::Starting => "starting",
            LoadState::Loading => "loading",
            LoadState::Ready => "ready",
            LoadState::Error => "error",
        }
    }
}

struct Status {
    state: LoadState,
    model: Option<String>,
    device: Option<String>,
    reason: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: LoadState::Starting,
            model: None,
            device: None,
            reason: None,
        }
    }
}

struct AppState {
    backend_dir: PathBuf,
    status: Mutex<Status>,
    engine: Mutex<Option<WhisperEngine>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let socket = std::env::var("SUPER_STT_BACKEND_SOCKET")
        .context("SUPER_STT_BACKEND_SOCKET must be set")?;
    let backend_dir =
        std::env::var("SUPER_STT_BACKEND_DIR").context("SUPER_STT_BACKEND_DIR must be set")?;

    let state = Arc::new(AppState {
        backend_dir: PathBuf::from(backend_dir),
        status: Mutex::new(Status::default()),
        engine: Mutex::new(None),
    });

    let app = Router::new()
        .route("/v1/ping", get(ping))
        .route("/v1/status", get(get_status))
        .route("/v1/load", post(load))
        .route("/v1/transcribe", post(transcribe))
        .route("/v1/cancel", post(cancel))
        .layer(DefaultBodyLimit::disable())
        .with_state(state);

    if let Some(parent) = std::path::Path::new(&socket).parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).with_context(|| format!("bind {socket}"))?;
    log::info!("whisper backend serving /v1 on {socket}");

    loop {
        let (stream, _) = listener.accept().await?;
        let app = app.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = TowerToHyperService::new(app);
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await
            {
                log::debug!("connection ended: {e}");
            }
        });
    }
}

async fn ping() -> Json<Value> {
    Json(json!({ "status": "success", "message": "pong" }))
}

async fn get_status(State(s): State<Arc<AppState>>) -> Json<Value> {
    let st = s.status.lock().unwrap();
    let mut out = json!({ "status": "success", "state": st.state.as_str() });
    if let Some(m) = &st.model {
        out["model"] = json!({ "name": m });
    }
    if let Some(d) = &st.device {
        out["device"] = json!(d);
    }
    if let Some(r) = &st.reason {
        out["reason"] = json!(r);
    }
    Json(out)
}

#[derive(Deserialize)]
struct LoadReq {
    name: String,
    #[serde(default)]
    device: Option<String>,
}

async fn load(State(s): State<Arc<AppState>>, Json(req): Json<LoadReq>) -> impl IntoResponse {
    {
        let mut st = s.status.lock().unwrap();
        st.state = LoadState::Loading;
        st.model = Some(req.name.clone());
        st.device = None;
        st.reason = None;
    }
    let dir = s.backend_dir.join("models").join(&req.name);
    let force_cpu = req.device.as_deref() == Some("cpu");
    let s2 = Arc::clone(&s);
    tokio::spawn(async move {
        let res = tokio::task::spawn_blocking(move || WhisperEngine::load(&dir, force_cpu)).await;
        match res {
            Ok(Ok(engine)) => {
                let label = engine.device_label().to_string();
                *s2.engine.lock().unwrap() = Some(engine);
                let mut st = s2.status.lock().unwrap();
                st.device = Some(label);
                st.state = LoadState::Ready;
                log::info!("model loaded; ready");
            }
            Ok(Err(e)) => {
                let mut st = s2.status.lock().unwrap();
                st.state = LoadState::Error;
                st.reason = Some(format!("{e:#}"));
                log::error!("model load failed: {e:#}");
            }
            Err(e) => {
                let mut st = s2.status.lock().unwrap();
                st.state = LoadState::Error;
                st.reason = Some(format!("load task panicked: {e}"));
            }
        }
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "success", "message": "Loading started" })),
    )
}

#[derive(Deserialize, Default)]
struct TranscribeOptions {
    #[serde(default)]
    stream_realtime: bool,
}

#[derive(Deserialize)]
struct TranscribeReq {
    audio_data: Vec<f32>,
    #[serde(default)]
    sample_rate: Option<u32>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    options: TranscribeOptions,
}

async fn transcribe(
    State(s): State<Arc<AppState>>,
    _headers: HeaderMap,
    Json(req): Json<TranscribeReq>,
) -> Response {
    if !matches!(s.status.lock().unwrap().state, LoadState::Ready) {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "status": "error", "message": "not_ready" })),
        )
            .into_response();
    }
    let sample_rate = req.sample_rate.unwrap_or(16000);
    let audio = req.audio_data;
    let language = req.language;

    if req.options.stream_realtime {
        transcribe_streaming(s, audio, sample_rate, language).await
    } else {
        transcribe_oneshot(s, audio, sample_rate, language).await
    }
}

async fn transcribe_oneshot(
    s: Arc<AppState>,
    audio: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
) -> Response {
    let s2 = Arc::clone(&s);
    let result = tokio::task::spawn_blocking(move || {
        let mut guard = s2.engine.lock().unwrap();
        let engine = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("engine not loaded"))?;
        engine.transcribe(&audio, sample_rate, language.as_deref())
    })
    .await;
    match result {
        Ok(Ok(text)) => (
            StatusCode::OK,
            Json(json!({ "status": "success", "transcription": text })),
        )
            .into_response(),
        Ok(Err(e)) => {
            let msg = format!("{e:#}");
            let code = if msg.contains("unsupported_language") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            let body = if msg.contains("unsupported_language") {
                json!({ "status": "error", "message": "unsupported_language" })
            } else {
                json!({ "status": "error", "message": "inference_failed", "detail": msg })
            };
            (code, Json(body)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": "inference_panicked", "detail": format!("{e}") })),
        )
            .into_response(),
    }
}

async fn transcribe_streaming(
    s: Arc<AppState>,
    audio: Vec<f32>,
    sample_rate: u32,
    language: Option<String>,
) -> Response {
    // 32 is a generous buffer — Whisper produces one preview per 30-s segment.
    let (tx, mut rx) = mpsc::unbounded_channel::<SseFrame>();
    let s2 = Arc::clone(&s);
    let preview_tx = tx.clone();

    tokio::task::spawn_blocking(move || {
        let mut guard = s2.engine.lock().unwrap();
        let Some(engine) = guard.as_mut() else {
            let _ = tx.send(SseFrame::Error("engine not loaded".to_string()));
            return;
        };
        let result = engine.transcribe_streaming(&audio, sample_rate, language.as_deref(), |t| {
            let _ = preview_tx.send(SseFrame::Preview(t.to_string()));
        });
        match result {
            Ok(text) => {
                let _ = tx.send(SseFrame::Done(text));
            }
            Err(e) => {
                let _ = tx.send(SseFrame::Error(format!("{e:#}")));
            }
        }
    });

    let stream = async_stream::stream! {
        while let Some(frame) = rx.recv().await {
            yield Ok::<_, Infallible>(frame.encode());
            if frame.is_terminal() {
                break;
            }
        }
    };

    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    resp
}

enum SseFrame {
    Preview(String),
    Done(String),
    Error(String),
}

impl SseFrame {
    fn encode(&self) -> bytes::Bytes {
        let s = match self {
            Self::Preview(t) => format!(
                "event: preview\ndata: {}\n\n",
                serde_json::to_string(&json!({ "text": t })).unwrap()
            ),
            Self::Done(t) => format!(
                "event: done\ndata: {}\n\n",
                serde_json::to_string(&json!({ "transcription": t })).unwrap()
            ),
            Self::Error(m) => format!(
                "event: error\ndata: {}\n\n",
                serde_json::to_string(&json!({ "message": m })).unwrap()
            ),
        };
        bytes::Bytes::from(s)
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Done(_) | Self::Error(_))
    }
}

async fn cancel() -> Json<Value> {
    // TODO: actually cancel an in-flight transcription. Voxtral backend has the
    // same placeholder; doing this requires plumbing a cancel token through
    // candle's decoder loop, which is non-trivial.
    Json(json!({ "status": "success", "message": "Cancelled" }))
}
```

- [ ] **Step 2: Add missing deps the file uses (`async-stream`, `bytes`)**

Run:
```bash
cd /home/jorge/rust_projects/super-stt/backends/whisper && \
  cargo add async-stream@0.3 bytes@1
```
Expected: `cargo add` records `async-stream = "0.3"` and `bytes = "1"` in `Cargo.toml`.

- [ ] **Step 3: Build the crate**

Run:
```bash
cargo build --manifest-path /home/jorge/rust_projects/super-stt/backends/whisper/Cargo.toml
```
Expected: Compiles to completion (will take several minutes the first time — pulls candle from git). Any warnings about unused imports are acceptable; errors are not.

If errors appear, address them inline before continuing.

- [ ] **Step 4: Smoke-launch the binary (no socket bind)**

The binary requires the env vars before it does anything useful. Confirm the missing-env error path:

Run:
```bash
/home/jorge/rust_projects/super-stt/backends/whisper/target/debug/super-stt-backend-whisper 2>&1 | head -5
```
Expected output starts with: `Error: SUPER_STT_BACKEND_SOCKET must be set`.

- [ ] **Step 5: Commit**

```bash
git add backends/whisper/src/main.rs backends/whisper/Cargo.toml
git commit -m "Serve Whisper /v1 contract with SSE preview streaming"
```

---

## Task 6: Justfile recipe and install-backends stanza

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Add `build-whisper-backend` recipe**

Find the existing `build-voxtral-backend` recipe in `/home/jorge/rust_projects/super-stt/justfile`:

```
# Build the standalone Voxtral subprocess backend.
# Usage: just build-voxtral-backend [--features cuda]
build-voxtral-backend *args:
    cargo build --manifest-path backends/voxtral/Cargo.toml --release {{ args }}
```

Insert below it:

```
# Build the standalone Whisper subprocess backend.
# Usage: just build-whisper-backend [--features cuda]
build-whisper-backend *args:
    cargo build --manifest-path backends/whisper/Cargo.toml --release {{ args }}
```

- [ ] **Step 2: Extend `install-backends` with a whisper stanza**

Find the existing voxtral stanza in `install-backends`:

```
    # Voxtral (subprocess). Installed only if the binary has been built.
    vox_bin="backends/voxtral/target/release/super-stt-backend-voxtral"
    if [ -f "$vox_bin" ]; then
        vox_dir="$backends_dir/voxtral"
        mkdir -p "$vox_dir"
        cp backends/voxtral/backend.toml "$vox_dir/backend.toml"
        cp "$vox_bin" "$vox_dir/super-stt-backend-voxtral"
        echo "Installed Voxtral backend -> $vox_dir"
    else
        echo "Voxtral backend not built; run 'just build-voxtral-backend [--features cuda]' to enable it." >&2
    fi
```

Insert below it (still inside the recipe, before `echo "Done. ..."`):

```
    # Whisper (subprocess). Installed only if the binary has been built.
    whisper_bin="backends/whisper/target/release/super-stt-backend-whisper"
    if [ -f "$whisper_bin" ]; then
        whisper_dir="$backends_dir/whisper"
        mkdir -p "$whisper_dir"
        cp backends/whisper/backend.toml "$whisper_dir/backend.toml"
        cp "$whisper_bin" "$whisper_dir/super-stt-backend-whisper"
        echo "Installed Whisper backend -> $whisper_dir"
    else
        echo "Whisper backend not built; run 'just build-whisper-backend [--features cuda]' to enable it." >&2
    fi
```

- [ ] **Step 3: Update the `install-backends` header comment**

Find this line above the recipe:

```
# (run `just build-voxtral-backend [--features cuda]` first for GPU support).
```

Replace with:

```
# (run `just build-{voxtral,whisper}-backend [--features cuda]` first for GPU support).
```

- [ ] **Step 4: Verify the recipe runs**

Run: `cd /home/jorge/rust_projects/super-stt && just --evaluate build-whisper-backend 2>&1 | head -3`
Expected: No "Unknown recipe" error. (We don't actually rebuild here; just confirm the recipe is wired.)

Better: actually invoke it to confirm the manifest path is correct.
Run: `cd /home/jorge/rust_projects/super-stt && just build-whisper-backend`
Expected: builds the release binary. (May take 5+ minutes the first time.)

- [ ] **Step 5: Verify install-backends deposits the binary**

Run:
```bash
cd /home/jorge/rust_projects/super-stt && just install-backends 2>&1 | grep -i whisper
```
Expected output includes: `Installed Whisper backend -> ...`

Verify on disk:
Run:
```bash
ls "${XDG_DATA_HOME:-$HOME/.local/share}/super-stt/backends/whisper/"
```
Expected: `backend.toml  super-stt-backend-whisper`

- [ ] **Step 6: Commit**

```bash
git add justfile
git commit -m "Wire Whisper backend into build/install recipes"
```

---

## Task 7: CPU smoke test with bundled WAV

**Files:**
- Create: `backends/whisper/tests/data/jfk.wav`
- Create: `backends/whisper/tests/smoke.rs`

The smoke test is `#[ignore]` by default — it pulls ~150 MB of weights and runs CPU inference. Opt in with `cargo test -- --ignored`. The point of the test is to prove the `.en` fix works: a verbatim port of the reference would fail `transcribes_tiny_en()`.

- [ ] **Step 1: Bundle the `jfk.wav` sample**

The whisper.cpp project distributes this file under MIT — content is in the public domain (audio from JFK's 1961 inauguration). Download it:

```bash
mkdir -p /home/jorge/rust_projects/super-stt/backends/whisper/tests/data
curl -fsSL \
  "https://github.com/ggerganov/whisper.cpp/raw/master/samples/jfk.wav" \
  -o /home/jorge/rust_projects/super-stt/backends/whisper/tests/data/jfk.wav
```

Verify size (~360 KB):
```bash
stat -c '%s' /home/jorge/rust_projects/super-stt/backends/whisper/tests/data/jfk.wav
```
Expected: a number between 300000 and 400000.

- [ ] **Step 2: Create `backends/whisper/tests/smoke.rs`**

Write the file with this exact content:

```rust
// SPDX-License-Identifier: GPL-3.0-only
//! CPU smoke tests. Ignored by default — opt in with
//! `cargo test --manifest-path backends/whisper/Cargo.toml -- --ignored`.
//!
//! Each test downloads its model into a per-user cache directory on first run
//! and reuses the cached files thereafter, so repeat runs are fast.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use hound::WavReader;
use reqwest::blocking::Client;
use super_stt_backend_whisper::inference::WhisperEngine;

fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .expect("cache dir")
        .join("super-stt-whisper-test")
}

fn ensure_model(name: &str, repo: &str) -> PathBuf {
    let dir = cache_dir().join(name);
    fs::create_dir_all(&dir).unwrap();
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap();
    for file in ["config.json", "tokenizer.json", "model.safetensors"] {
        let dest = dir.join(file);
        if dest.exists() && fs::metadata(&dest).unwrap().len() > 0 {
            continue;
        }
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        eprintln!("smoke: downloading {url}");
        let mut resp = client.get(&url).send().unwrap().error_for_status().unwrap();
        let tmp = dir.join(format!(".{file}.tmp"));
        let mut out = fs::File::create(&tmp).unwrap();
        resp.copy_to(&mut out).unwrap();
        out.flush().unwrap();
        fs::rename(&tmp, &dest).unwrap();
    }
    dir
}

fn load_wav(path: &Path) -> Vec<f32> {
    let mut reader = WavReader::open(path).expect("open wav");
    assert_eq!(reader.spec().sample_rate, 16000, "expected 16 kHz wav");
    assert_eq!(reader.spec().channels, 1, "expected mono wav");
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect()
}

#[test]
#[ignore = "pulls ~75 MB of weights and runs CPU inference"]
fn transcribes_tiny() {
    let dir = ensure_model("whisper-tiny", "openai/whisper-tiny");
    let audio = load_wav(Path::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/jfk.wav"),
    ));
    let mut engine = WhisperEngine::load(&dir, /* force_cpu */ true).expect("load");
    let text = engine.transcribe(&audio, 16000, None).expect("transcribe");
    eprintln!("tiny: {text:?}");
    let lower = text.to_lowercase();
    assert!(!text.trim().is_empty(), "tiny produced empty output");
    assert!(
        lower.contains("ask not") || lower.contains("country"),
        "tiny output should reference the JFK quote; got {text:?}"
    );
}

#[test]
#[ignore = "pulls ~75 MB of weights and runs CPU inference"]
fn transcribes_tiny_en() {
    let dir = ensure_model("whisper-tiny.en", "openai/whisper-tiny.en");
    let audio = load_wav(Path::new(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/jfk.wav"),
    ));
    let mut engine = WhisperEngine::load(&dir, /* force_cpu */ true).expect("load");
    assert!(engine.is_english_only(), ".en model should be english_only");
    let text = engine.transcribe(&audio, 16000, None).expect("transcribe");
    eprintln!("tiny.en: {text:?}");
    let lower = text.to_lowercase();
    assert!(!text.trim().is_empty(), "tiny.en produced empty output");
    assert!(
        lower.contains("ask not") || lower.contains("country"),
        "tiny.en output should reference the JFK quote; got {text:?}"
    );
}
```

- [ ] **Step 3: Verify the test compiles (without running the ignored tests)**

Run:
```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/backends/whisper/Cargo.toml --no-run
```
Expected: Compiles. Any unused-import warnings inside the `include!`'d code are tolerable as long as the build succeeds.

- [ ] **Step 4: Run the ignored tests to verify both pass**

Run:
```bash
cargo test --manifest-path /home/jorge/rust_projects/super-stt/backends/whisper/Cargo.toml --release -- --ignored --nocapture
```
Expected:
- First run: prints "smoke: downloading https://huggingface.co/..." lines while pulling weights.
- Both `transcribes_tiny` and `transcribes_tiny_en` pass.
- The captured output for each test shows a transcription containing "ask not" or "country".

If `transcribes_tiny_en` fails with empty output, the prompt-by-capability fix didn't take — re-check `decode_simple` in `src/inference.rs`. If both fail with the same error, it's likely a model-loading issue (paths, candle revision).

- [ ] **Step 5: Run clippy on the new crate**

Run:
```bash
cd /home/jorge/rust_projects/super-stt/backends/whisper && \
  cargo clippy --all-features -- -W clippy::pedantic -D warnings -D unused_must_use
```
Expected: passes (or only pedantic-tier warnings that aren't promoted to errors by `-D warnings`). Fix any errors inline.

- [ ] **Step 6: Commit**

```bash
git add backends/whisper/tests/data/jfk.wav backends/whisper/tests/smoke.rs backends/whisper/Cargo.toml
git commit -m "Add CPU smoke test for Whisper tiny + tiny.en"
```

---

## Task 8: End-to-end check against the running daemon

This task is not a code task — it's the human-in-the-loop verification that the backend integrates cleanly with the daemon. Run it once after Tasks 1-7 are complete.

- [ ] **Step 1: Restart the daemon to pick up the new backend**

```bash
systemctl --user restart super-stt
```

- [ ] **Step 2: Confirm discovery**

```bash
# Adjust the URL / auth header per docs/protocol/transport.md
curl -s --unix-socket "$XDG_RUNTIME_DIR/stt/daemon.sock" http://localhost/v1/backends | jq '.backends[] | select(.source | contains("whisper"))'
```
Expected: JSON object describing the Whisper backend with 9 models.

- [ ] **Step 3: Select `whisper-tiny.en` and transcribe**

Use the app's Models page (Installed tab → Whisper card → pick `whisper-tiny.en` → Select), wait for "ready", record a short utterance, and confirm non-empty transcription. The same with `whisper-tiny` and a non-English utterance (with language set in the daemon's options) is the multilingual path.

- [ ] **Step 4: Verify streaming**

The app's existing realtime path already passes `stream_realtime: true`. Watch the in-app preview update while transcribing a >30 s clip.

- [ ] **Step 5: No commit — this is a verification task.**

If any step fails, file the gap in the spec's "Validation checklist" section and loop back to the relevant code task.

---

## Self-review notes

- **Spec coverage:** All 9 models listed in the spec table ↔ Task 2. Bug fix #1 (capability-based prompt) ↔ Task 4, step 1. Bug fix #2 (drop length floor) ↔ Task 4, step 1. SSE streaming ↔ Task 5. CPU smoke test ↔ Task 7. Justfile integration ↔ Task 6. Validation checklist items ↔ Task 8.
- **Cargo `tokenizers`**: doc says "0.20" in the spec; latest is 0.23. Plan uses `tokenizers = "0.23"` (current). Update the spec opportunistically if you touch it.
- **Lib + bin split** is added so the integration test can `use super_stt_backend_whisper::inference::WhisperEngine` directly. Voxtral is bin-only; this departure from the pattern is justified by testability. Both targets are auto-discovered — no explicit `[lib]`/`[[bin]]` blocks.
- **Cancel** is a no-op everywhere. This matches the voxtral backend; the spec calls it out as out-of-scope.
