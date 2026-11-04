// SPDX-License-Identifier: GPL-3.0-only
//! Shared `/v1/synthesize` request/response plumbing for the backend hosts.
//!
//! The wasm and subprocess hosts differ only in transport (an in-process
//! component invocation vs a Unix-socket HTTP dial). The request-body build and
//! the response parsing were byte-identical in both — and had started to drift —
//! so they live here. Feature-agnostic: compiled whenever either backend is on.

use anyhow::{Result, anyhow};
use super_tts_shared::audio::frames::{AudioParams, Frame, FrameKind};

/// A `POST /v1/synthesize` request.
///
/// The daemon has already normalized the text, resolved `auto` to a concrete
/// language, and chunked to the model's `max_input_chars`, so a backend
/// receives one prosodic unit it can synthesize as written.
#[derive(Debug, Default, Clone)]
pub struct SynthesizeRequest<'a> {
    /// The text to speak. Non-empty.
    pub text: &'a str,
    /// A `voice` id, or `None` to use the model's `default_voice`.
    pub voice: Option<&'a str>,
    /// Resolved BCP-47 tag. Never `auto` — the daemon owns detection, since it
    /// owns the text.
    pub language: Option<&'a str>,
    /// Rate multiplier. Backends that cannot vary rate ignore it; the daemon
    /// does not resample to fake it.
    pub speed: Option<f32>,
    /// Free-text delivery guidance for models that accept it.
    pub instructions: Option<&'a str>,
}

/// Serialize a `/v1/synthesize` request body. Optional fields are omitted
/// rather than sent as `null`, so a backend can distinguish "unset" without
/// special-casing.
///
/// # Errors
/// Returns an error if JSON serialization fails (not expected for this shape).
pub fn build_synthesize_body(req: &SynthesizeRequest<'_>) -> Result<Vec<u8>> {
    // Built as a map rather than a `json!` literal that is then downcast, so
    // there is no `as_object_mut().expect(..)` to justify.
    let mut map = serde_json::Map::new();
    map.insert("text".into(), req.text.into());
    if let Some(v) = req.voice {
        map.insert("voice".into(), v.into());
    }
    if let Some(l) = req.language {
        map.insert("language".into(), l.into());
    }
    if let Some(s) = req.speed {
        map.insert("speed".into(), serde_json::json!(s));
    }
    if let Some(i) = req.instructions {
        map.insert("instructions".into(), i.into());
    }
    Ok(serde_json::to_vec(&serde_json::Value::Object(map))?)
}

/// A `POST /v1/voices` request: one cloned voice's reference audio.
///
/// Sent once, when a voice is first used with a loaded model — not on every
/// synthesis. The daemon splits long text into one `/v1/synthesize` call per
/// sentence, so pushing the clip with the request would re-upload and
/// re-encode it a dozen times for one paragraph. Registering instead lets a
/// backend derive its speaker embedding (or reference codes) once and key them
/// by `voice`.
///
/// The audio is base64 in JSON rather than a raw body: it rides the same
/// content type as every other backend route, so a backend author needs no
/// second parsing path for a call that happens once per voice.
#[derive(Debug, Clone)]
pub struct RegisterVoiceRequest<'a> {
    /// The full wire id, `voice:<uuid>` — the same string a later
    /// [`SynthesizeRequest::voice`] carries, so a backend can key its cache by
    /// what it will be asked for.
    pub voice: &'a str,
    /// What the clip says, when the model declared `clone_needs_transcript`.
    /// Absent otherwise; a backend that clones from a speaker embedding never
    /// needs it.
    pub transcript: Option<&'a str>,
    /// Sample rate of `pcm`.
    pub sample_rate: u32,
    /// Channel count of `pcm`. Always 1 today.
    pub channels: u16,
    /// Sample format of `pcm`, in the `x-tts-format` vocabulary.
    pub format: &'a str,
    /// The reference audio, already downmixed, resampled, and trimmed to the
    /// model's `clone_ref_seconds`.
    pub pcm: &'a [u8],
}

/// Serialize a `/v1/voices` registration body.
///
/// # Errors
/// Returns an error if JSON serialization fails (not expected for this shape).
pub fn build_register_voice_body(req: &RegisterVoiceRequest<'_>) -> Result<Vec<u8>> {
    use base64::{Engine as _, engine::general_purpose::STANDARD as B64};

    let mut map = serde_json::Map::new();
    map.insert("voice".into(), req.voice.into());
    if let Some(t) = req.transcript {
        map.insert("transcript".into(), t.into());
    }
    map.insert("sample_rate".into(), req.sample_rate.into());
    map.insert("channels".into(), req.channels.into());
    map.insert("format".into(), req.format.into());
    map.insert("audio".into(), B64.encode(req.pcm).into());
    Ok(serde_json::to_vec(&serde_json::Value::Object(map))?)
}

/// Turn a non-2xx voice-registration response into an error, preferring the
/// backend's own message.
#[must_use]
pub fn voice_error(status: u16, body: &[u8]) -> anyhow::Error {
    let msg = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("detail")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "registering the voice failed".to_string());
    anyhow!("backend returned {status}: {msg}")
}

/// Receives a synthesis response as it decodes.
///
/// A sink rather than a returned stream because the wasm transport reads the
/// body while its `Store` is still borrowed: handing frames out through a
/// callback keeps that borrow local to the call, and both transports then
/// share one pump.
pub trait SynthesisSink {
    /// The response's audio invariants, delivered once before any frame.
    ///
    /// # Errors
    /// Returning an error aborts the pump.
    fn on_params(&mut self, params: AudioParams) -> Result<()>;

    /// One decoded frame, in arrival order.
    ///
    /// # Errors
    /// Returning an error aborts the pump — this is how a cancelled utterance
    /// stops a backend that is still streaming.
    fn on_frame(&mut self, frame: Frame) -> Result<()>;
}

/// Collect a whole synthesis response in memory. Test and fixture helper; the
/// playback path uses a streaming sink instead.
#[derive(Debug, Default)]
pub struct CollectingSink {
    /// Params from the response headers, once seen.
    pub params: Option<AudioParams>,
    /// Every frame, in arrival order.
    pub frames: Vec<Frame>,
}

impl SynthesisSink for CollectingSink {
    fn on_params(&mut self, params: AudioParams) -> Result<()> {
        self.params = Some(params);
        Ok(())
    }

    fn on_frame(&mut self, frame: Frame) -> Result<()> {
        self.frames.push(frame);
        Ok(())
    }
}

impl CollectingSink {
    /// Concatenate every audio payload and decode it as one buffer.
    ///
    /// Decoding the concatenation rather than each frame matters: a backend may
    /// split a frame mid-sample, and per-frame decoding would drop the straddling
    /// bytes.
    #[must_use]
    pub fn samples(&self) -> Vec<f32> {
        let Some(params) = self.params else {
            return Vec::new();
        };
        let mut pcm = Vec::new();
        for f in self.frames.iter().filter(|f| f.kind == FrameKind::Audio) {
            pcm.extend_from_slice(&f.payload);
        }
        params.format.decode(&pcm)
    }
}

/// Read a framed `/v1/synthesize` response body, feeding `sink` as frames
/// complete.
///
/// Generic over the body so the wasm and subprocess transports share it —
/// wasmtime's and hyper's bodies differ in type but both implement
/// `http_body::Body<Data = Bytes>`.
///
/// # Errors
/// Returns an error when the headers do not describe usable audio, the frame
/// stream is malformed or exceeds its caps, the body errors mid-read, or the
/// backend sent an `error` frame.
#[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
pub async fn pump_synthesis<B>(
    headers: &hyper::http::HeaderMap,
    body: B,
    sink: &mut (dyn SynthesisSink + Send),
) -> Result<()>
where
    B: http_body::Body<Data = bytes::Bytes>,
    B::Error: std::fmt::Display,
{
    use anyhow::bail;
    use http_body_util::BodyExt;
    use super_tts_shared::audio::frames::FrameDecoder;

    let params = AudioParams::from_headers(|name| headers.get(name).and_then(|v| v.to_str().ok()))?;
    sink.on_params(params)?;

    let mut decoder = FrameDecoder::new();
    let mut body = std::pin::pin!(body);
    while let Some(next) = body.frame().await {
        let chunk = next.map_err(|e| anyhow!("reading synthesis body: {e}"))?;
        let Some(data) = chunk.data_ref() else {
            continue; // trailers carry nothing for this contract
        };
        decoder.push(data);
        while let Some(frame) = decoder.next_frame()? {
            if frame.kind == FrameKind::Error {
                bail!("{}", error_frame_message(&frame.payload));
            }
            sink.on_frame(frame)?;
        }
        // Stop reading as soon as the backend has said it is done, rather than
        // waiting for the body to close: a backend that never closes would
        // otherwise hold the utterance open forever.
        if decoder.saw_terminal() {
            break;
        }
    }
    decoder.finish()?;
    Ok(())
}

/// Pull the human-readable message out of an `error` frame payload, falling
/// back to a fixed string when it is not the documented shape.
#[cfg(any(feature = "wasm-backends", feature = "subprocess-backends"))]
fn error_frame_message(payload: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "synthesis failed".to_string())
}

/// Turn a non-`200` synthesize response into an error, preferring the
/// backend's own `detail`/`message` (it reaches the user) over the raw body.
#[must_use]
pub fn synthesize_error(status: u16, body: &[u8]) -> anyhow::Error {
    let msg = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("detail")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "synthesis failed".to_string());
    anyhow!("backend returned {status}: {msg}")
}
