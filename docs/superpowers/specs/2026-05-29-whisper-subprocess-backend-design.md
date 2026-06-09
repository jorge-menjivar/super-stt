# Whisper subprocess backend

**Date:** 2026-05-29
**Status:** Design — pending implementation

Port the in-tree Whisper model archived under `reference/in-tree-models/local/whisper/` into a standalone subprocess backend under `backends/whisper/`, following the pattern already established by `backends/voxtral/`.

## Goal

Ship a `kind = "subprocess"` backend that serves the 9 Whisper variants from the pre-refactor registry over the `/v1` contract, with CPU + CUDA support and SSE streaming of per-segment previews. Fix two latent bugs in the reference code that block the `.en` variants and short utterances.

## Non-goals

- New Whisper variants beyond the pre-refactor list (no `large-v2`, `large-v3`, `turbo`).
- WebSocket / realtime streaming beyond the contract's SSE `preview`/`done` shape.
- Per-token preview emission (per-segment is enough; the contract's `preview` frames are coarse-grained).
- Real cancellation of in-flight inference. `POST /v1/cancel` stays a no-op, matching `backends/voxtral`.

## Crate layout

```
backends/whisper/
├── Cargo.toml          # standalone, excluded from workspace
├── Cargo.lock          # generated
├── backend.toml        # 9 models, subprocess kind
├── src/
│   ├── main.rs         # axum /v1 server (near-copy of voxtral)
│   ├── inference.rs    # ported Whisper inference, with streaming hook
│   └── data/
│       └── melfilters.bytes      # 80-bin mel filter coefficients
└── tests/
    ├── data/
    │   └── jfk.wav                # ~11 s public-domain sample
    └── smoke.rs                   # CPU smoke test, #[ignore] by default
```

`Cargo.toml` mirrors `backends/voxtral/Cargo.toml` with these differences:

- Package `name = "super-stt-backend-whisper"`.
- Drop `tekken-rs`; add `tokenizers = { version = "0.20", default-features = false, features = ["onig"] }`.
- Keep the same pinned `candle-{core,nn,transformers}` revision the voxtral backend uses, so the whole local-backend toolchain stays consistent.
- Keep `cuda`, `cudnn`, `flash-attn` features identical to voxtral.
- `default = []` (CPU-only build is the lowest-friction default; users opt into GPU with `--features cuda`).

Root `Cargo.toml` `exclude` array gains `"backends/whisper"`.

## `backend.toml`

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
```

### Models

Nine `[[models]]` entries. Common fields:

- `provider = "local_whisper"`
- `primary_language = "en"`
- `supported_devices = ["cpu", "cuda"]`
- `[[models.files]]` block pointing at `openai/whisper-<variant>` revision `main` with `["config.json", "tokenizer.json", "model.safetensors"]` into `dest = "models/whisper-<variant>"`

Per-model varying fields:

| `name` | `multilingual` | `supported_languages` | `estimated_vram_bytes` | `processing_interval_ms` |
|---|---|---|---|---|
| `whisper-tiny` | true | full 99 | `1_073_741_824` (1 GiB) | 1000 |
| `whisper-tiny.en` | false | `["en"]` | `1_073_741_824` (1 GiB) | 1000 |
| `whisper-base` | true | full 99 | `1_073_741_824` (1 GiB) | 1500 |
| `whisper-base.en` | false | `["en"]` | `1_073_741_824` (1 GiB) | 1500 |
| `whisper-small` | true | full 99 | `2_147_483_648` (2 GiB) | 2000 |
| `whisper-small.en` | false | `["en"]` | `2_147_483_648` (2 GiB) | 2000 |
| `whisper-medium` | true | full 99 | `5_368_709_120` (5 GiB) | 2000 |
| `whisper-medium.en` | false | `["en"]` | `5_368_709_120` (5 GiB) | 2000 |
| `whisper-large` | true | full 99 | `10_737_418_240` (10 GiB) | 5000 |

The VRAM column matches OpenAI's published estimates verbatim
(<https://github.com/openai/whisper#available-models-and-languages>).

`supported_languages` for the multilingual models is Whisper's canonical 99-code list, in this order (the same order as `whisper.tokenizer.LANGUAGES`):

```
en, zh, de, es, ru, ko, fr, ja, pt, tr, pl, ca, nl, ar, sv, it, id, hi, fi, vi,
he, uk, el, ms, cs, ro, da, hu, ta, no, th, ur, hr, bg, lt, la, mi, ml, cy, sk,
te, fa, lv, bn, sr, az, sl, kn, et, mk, br, eu, is, hy, ne, mn, bs, kk, sq, sw,
gl, mr, pa, si, km, sn, yo, so, af, oc, ka, be, tg, sd, gu, am, yi, lo, uz, fo,
ht, ps, tk, nn, mt, sa, lb, my, bo, tl, mg, as, tt, haw, ln, ha, ba, jw, su, yue
```

## `src/main.rs`

A near-copy of `backends/voxtral/src/main.rs`. Concrete deltas:

1. Replace `VoxtralEngine` with `WhisperEngine` everywhere.

2. Extend `TranscribeReq`:
   ```rust
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
   ```

3. `transcribe` branches on `req.options.stream_realtime`:

   - **`false`**: existing voxtral one-shot path. Returns `200 application/json` `{ "status": "success", "transcription": "..." }`.

   - **`true`**: returns a `200 text/event-stream` response. The handler spawns a blocking task that runs `WhisperEngine::transcribe_streaming(audio, sample_rate, language, |segment_accum| tx.send(segment_accum))` on a thread, with `tx` an `mpsc::UnboundedSender<String>` whose receiver drives a `futures::stream::unfold` that emits `event: preview\ndata: {"text": "..."}\n\n` frames followed by one `event: done\ndata: {"transcription": "..."}\n\n` and closes. On error, emit `event: error\ndata: {"message": "..."}\n\n` and close.

4. Cancel stays a no-op (matches voxtral). Listed as a TODO in the source so it's discoverable.

5. The daemon-injected `x-stt-model` and `x-stt-option-*` / `x-stt-secret-*` headers are not used by this backend — local model, no secrets, no options.

## `src/inference.rs`

Port of `reference/in-tree-models/local/whisper/model.rs` with super-stt wrappers stripped (`ModelInfoData`, `Transcribe` trait, registry lookup, the `super_stt_shared` resample helper). Resampling happens in the daemon upstream; the engine assumes 16 kHz mono `f32`.

Public surface:

```rust
pub struct WhisperEngine { /* ... */ }

impl WhisperEngine {
    pub fn load(model_dir: &Path, force_cpu: bool) -> Result<Self>;
    pub fn device_label(&self) -> &'static str;     // "cpu" | "cuda" | "metal"

    /// One-shot transcription. Equivalent to `transcribe_streaming` with a
    /// callback that ignores its input.
    pub fn transcribe(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
    ) -> Result<String>;

    /// Streaming variant. Invokes `on_segment(accumulated_text)` after each
    /// 30 s segment finishes decoding. Returns the final transcription.
    pub fn transcribe_streaming<F: FnMut(&str)>(
        &mut self,
        audio: &[f32],
        sample_rate: u32,
        language: Option<&str>,
        on_segment: F,
    ) -> Result<String>;
}
```

### Bug fix 1: build the decoder prompt by capability

The reference unconditionally pushes `<|transcribe|>` (`model.rs:317`) even for `.en` models. Whisper's reference decoder builds the prompt as `[sot, language?, task?, no_timestamps]`; `.en` variants omit both the language and the task token. Pushing `<|transcribe|>` into a `.en` prompt biases the model toward an out-of-distribution prefix and produces empty / garbage output.

Detect "English-only" at load by checking whether the tokenizer has any language token (e.g. probing `<|en|>`). Store the result on the engine:

```rust
pub struct WhisperEngine {
    // ...
    is_english_only: bool,        // .en variant detection
}
```

In `decode_simple`, build the prompt accordingly:

```rust
let mut tokens = vec![self.sot_token];

if !self.is_english_only {
    // Multilingual: emit language token + task token.
    let lang_code = language.unwrap_or("en");
    let lang_tok = format!("<|{lang_code}|>");
    let lang_id = self.tokenizer
        .token_to_id(&lang_tok)
        .ok_or_else(|| anyhow::anyhow!("unsupported_language"))?;
    tokens.push(lang_id);
    tokens.push(self.transcribe_token);
}

tokens.push(self.no_timestamps_token);
```

Per the contract, language validation lives in the backend: an unknown `language` for a multilingual model surfaces as `400 unsupported_language`. The `.en` models ignore the request's `language` field entirely — the daemon will reject `language` for non-multilingual models at protocol level (the contract only permits it on multilingual models), but if the field arrives anyway, the engine silently drops it.

`main.rs` translates the `Err("unsupported_language")` from the engine into the documented `400 { "status": "error", "message": "unsupported_language" }` response; all other engine errors become `500 inference_failed`.

### Bug fix 2: drop the length floor in fallback

The reference (`model.rs:280`) requires `result.len() > 5` before accepting a temperature's output, which discards short utterances ("OK.", "Yes.") and returns `Ok(String::new())`. The fallback should accept any non-empty result on a successful decode, escalating temperature only on actual errors:

```rust
fn decode_with_fallback(&mut self, mel_segment: &Tensor, language: Option<&str>) -> Result<String> {
    let temperatures = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
    let mut last_err = None;
    for &t in &temperatures {
        match self.decode_simple(mel_segment, t, language) {
            Ok(result) if !result.trim().is_empty() => return Ok(result),
            Ok(_) => continue,           // empty result: try a higher temperature
            Err(e) => last_err = Some(e), // decode error: try a higher temperature
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("all temperatures produced empty output")))
}
```

### Streaming hook

`run_segmented` gains a callback parameter. After each segment's decoded text is pushed to `all_text`, it invokes `on_segment(&joined_so_far)`:

```rust
fn run_segmented<F: FnMut(&str)>(
    &mut self,
    mel: &Tensor,
    language: Option<&str>,
    mut on_segment: F,
) -> Result<String> {
    // ... existing loop ...
    if !segment_result.trim().is_empty() {
        all_text.push(segment_result);
        let joined = all_text.join(" ");
        on_segment(&joined);
    }
    // ...
}
```

`transcribe` calls it with `|_| {}`; `transcribe_streaming` passes the real callback.

### Mel filters

`backends/voxtral/src/data/melfilters128.bytes` is the 128-bin file (Voxtral uses 128 mels). Whisper uses 80 mels — the same file the reference reads from `reference/in-tree-models/local/data/melfilters.bytes`. Copy that file into `backends/whisper/src/data/melfilters.bytes` and `include_bytes!` it from `inference.rs`, matching the reference's `num_mel_bins == 80` branch.

The 128-bin file is not needed unless `whisper-large-v3` is added later; out of scope for this port.

## Build and install

Justfile additions:

```
# Build the standalone Whisper subprocess backend.
# Usage: just build-whisper-backend [--features cuda]
build-whisper-backend *args:
    cargo build --manifest-path backends/whisper/Cargo.toml --release {{ args }}
```

Extend `install-backends` with a stanza mirroring the voxtral one — install only when `backends/whisper/target/release/super-stt-backend-whisper` exists, copy alongside its `backend.toml`. Update the recipe header comment to mention `just build-whisper-backend [--features cuda]`.

## Testing

`backends/whisper/tests/smoke.rs`:

- `#[ignore]` by default so CI doesn't pull weights or run inference. Opt in with `cargo test --manifest-path backends/whisper/Cargo.toml -- --ignored`.
- Two test functions: `transcribes_tiny()` and `transcribes_tiny_en()`. Each loads its variant on CPU, runs `transcribe(&samples, 16000, None)`, asserts the result is non-empty and contains at least one substring from a known set (e.g. `"ask not"` for `jfk.wav`).
- Weights resolve under `~/.cache/super-stt-whisper-test/models/whisper-tiny/` etc., downloaded on first run with a small `reqwest`-driven helper in the test module (no shared crate code).
- Sample WAV: bundle `tests/data/jfk.wav` (the standard whisper.cpp/PocketSphinx sample, public domain — 11 s, ~360 KB).

This catches the two `.en` regressions explicitly: a verbatim port would fail `transcribes_tiny_en()`; the fixed port would pass it.

## Migration notes for the daemon

None. The daemon discovers the backend on disk via the existing scan, downloads its files into the backend directory before `POST /v1/load`, and routes transcription identically to voxtral. The `language` header forwarded by the daemon already exists in the contract; we just start reading it.

## Validation checklist

- `cargo build --manifest-path backends/whisper/Cargo.toml --release` succeeds on CPU and with `--features cuda`.
- `cargo clippy --all-features --workspace -- -W clippy::pedantic -D warnings -D unused_must_use` passes on the workspace (whisper backend is excluded from the workspace, so apply clippy separately to it).
- `just install-backends` deposits the binary + `backend.toml` under `<XDG_DATA_HOME>/super-stt/backends/whisper/`.
- After daemon restart, `GET /backends` lists Whisper and its 9 models.
- Selecting `whisper-tiny.en` loads successfully and `POST /v1/transcribe` returns non-empty text on a short utterance — the regression the .en fix targets.
- Selecting `whisper-tiny` with `language: "es"` produces Spanish-language output, not English.
- `stream_realtime: true` on a >30 s clip produces one or more `event: preview` frames before `event: done`.
