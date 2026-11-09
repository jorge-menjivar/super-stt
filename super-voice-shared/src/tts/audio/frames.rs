// SPDX-License-Identifier: GPL-3.0-only
//! The `POST /v1/synthesize` response framing.
//!
//! A backend answers a synthesis request with a chunked binary body carrying
//! interleaved audio and metadata. Invariants that hold for the whole response
//! — sample rate, channel count, sample format — travel once as headers
//! ([`AudioParams`]); everything after is a sequence of length-prefixed frames:
//!
//! ```text
//! [u8 kind][u32 len little-endian][len bytes payload]
//! ```
//!
//! Chunked-transfer boundaries are deliberately not load-bearing. HTTP
//! implementations coalesce and re-split them at will, so a reader that treated
//! a transport chunk as a message would work against one backend and fail
//! against the next. The length prefix is what frames the stream.
//!
//! The decoder is incremental: [`FrameDecoder::push`] takes whatever bytes have
//! arrived and [`FrameDecoder::next_frame`] yields whole frames as they
//! complete. That is what lets the daemon start playing audio before synthesis
//! finishes.
//!
//! Backends are untrusted code, so the decoder is written as a parser of
//! hostile input: [`MAX_FRAME_LEN`] and [`MAX_TOTAL_AUDIO_BYTES`] bound what a
//! single response can make the daemon allocate, and a stream that ends without
//! a terminal frame is an error rather than a silently short utterance.

use serde::{Deserialize, Serialize};

/// Largest single frame payload, in bytes.
///
/// A length prefix read from an untrusted backend is an
/// allocate-what-I-say primitive without a ceiling. 1 MiB is far above any
/// sensible audio frame (a second of 48 kHz stereo f32 is 384 KiB) and far
/// below anything that threatens the daemon.
pub const MAX_FRAME_LEN: u32 = 1 << 20;

/// Largest total `audio` payload across one response, in bytes.
///
/// Bounds a backend that streams forever. 32 MiB is ~5.8 minutes of 24 kHz
/// mono f32, comfortably more than one utterance and far less than memory
/// pressure.
pub const MAX_TOTAL_AUDIO_BYTES: u64 = 32 << 20;

/// Header naming the response's sample rate in Hz.
pub const HEADER_SAMPLE_RATE: &str = "x-tts-sample-rate";
/// Header naming the response's channel count.
pub const HEADER_CHANNELS: &str = "x-tts-channels";
/// Header naming the response's sample format.
pub const HEADER_FORMAT: &str = "x-tts-format";
/// Content type a framed synthesis response carries.
pub const CONTENT_TYPE: &str = "application/vnd.super-tts.frames";

/// How PCM samples are encoded in an [`FrameKind::Audio`] payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SampleFormat {
    /// 32-bit float, little-endian, nominally `[-1.0, 1.0]`.
    F32Le,
    /// 16-bit signed integer, little-endian.
    ///
    /// First-class rather than a concession: `OpenAI`'s `pcm` response format is
    /// raw 24 kHz s16le, so requiring `f32le` would make every backend shaped
    /// like it widen bytes the daemon immediately narrows again.
    S16Le,
}

impl SampleFormat {
    /// Bytes per sample in this format.
    #[must_use]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::F32Le => 4,
            Self::S16Le => 2,
        }
    }

    /// Wire spelling, as it appears in the `x-tts-format` header.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::F32Le => "f32le",
            Self::S16Le => "s16le",
        }
    }

    /// Decode a payload into normalized `f32` samples.
    ///
    /// A trailing partial sample is dropped: frame boundaries are the
    /// backend's choice and need not align to a sample, so the remainder
    /// belongs to whatever the next frame carries. Callers that need
    /// sample-aligned reassembly concatenate payloads before decoding.
    #[must_use]
    pub fn decode(self, bytes: &[u8]) -> Vec<f32> {
        match self {
            Self::F32Le => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
            Self::S16Le => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
                .collect(),
        }
    }
}

impl std::str::FromStr for SampleFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "f32le" => Ok(Self::F32Le),
            "s16le" => Ok(Self::S16Le),
            other => Err(format!("unknown sample format: {other}")),
        }
    }
}

impl std::fmt::Display for SampleFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The per-response audio invariants, read from the response headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioParams {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count. `1` is mono.
    pub channels: u16,
    /// How samples are encoded in `audio` payloads.
    pub format: SampleFormat,
}

impl AudioParams {
    /// Read the params from a response's headers.
    ///
    /// The headers are authoritative over anything a manifest declared: only
    /// the backend knows what it actually produced for this request.
    ///
    /// # Errors
    /// Returns [`FrameError::BadParams`] when a header is missing, unparseable,
    /// or names an implausible rate or channel count.
    pub fn from_headers<'a>(lookup: impl Fn(&str) -> Option<&'a str>) -> Result<Self, FrameError> {
        let get = |name: &str| {
            lookup(name).ok_or_else(|| FrameError::BadParams(format!("missing {name}")))
        };
        let sample_rate: u32 = get(HEADER_SAMPLE_RATE)?
            .trim()
            .parse()
            .map_err(|_| FrameError::BadParams(format!("{HEADER_SAMPLE_RATE} is not a number")))?;
        let channels: u16 = get(HEADER_CHANNELS)?
            .trim()
            .parse()
            .map_err(|_| FrameError::BadParams(format!("{HEADER_CHANNELS} is not a number")))?;
        let format: SampleFormat = get(HEADER_FORMAT)?
            .trim()
            .parse()
            .map_err(|e: String| FrameError::BadParams(e))?;

        // Same range the manifest validates `output_sample_rate` against, so a
        // backend cannot get past the manifest check and then declare
        // something no device can serve.
        if !(8000..=192_000).contains(&sample_rate) {
            return Err(FrameError::BadParams(format!(
                "sample rate {sample_rate} outside 8000..=192000"
            )));
        }
        if !(1..=2).contains(&channels) {
            return Err(FrameError::BadParams(format!(
                "channel count {channels} outside 1..=2"
            )));
        }
        Ok(Self {
            sample_rate,
            channels,
            format,
        })
    }
}

/// Frame discriminant, as it appears in the leading byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// Raw PCM in the response's declared format.
    Audio = 0x01,
    /// A JSON [`Mark`] aligning output audio to input text.
    Mark = 0x02,
    /// Terminal success. The stream closes after it.
    Done = 0x03,
    /// Terminal failure. The stream closes after it.
    Error = 0x04,
}

impl FrameKind {
    /// Parse the discriminant byte.
    const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Audio),
            0x02 => Some(Self::Mark),
            0x03 => Some(Self::Done),
            0x04 => Some(Self::Error),
            _ => None,
        }
    }

    /// Whether this kind ends the stream.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Error)
    }
}

/// One decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// What the payload is.
    pub kind: FrameKind,
    /// The raw payload; JSON text for every kind but [`FrameKind::Audio`].
    pub payload: Vec<u8>,
}

/// Alignment between a span of output audio and a span of the request's text.
///
/// Both halves are optional so the shape covers what real providers emit:
/// `ElevenLabs` returns character alignment, `Cartesia` word timings, and a local
/// model may have neither. Character offsets index the `text` of the request
/// that produced this response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    /// Start of the audio span, in milliseconds from the response's first sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<u64>,
    /// End of the audio span, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ms: Option<u64>,
    /// Start of the text span, as a character offset into the request `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_char: Option<u32>,
    /// End of the text span, exclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_char: Option<u32>,
}

/// Why a synthesis response could not be read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FrameError {
    /// The response headers did not describe usable audio.
    #[error("synthesis response headers: {0}")]
    BadParams(String),
    /// A frame declared a length above [`MAX_FRAME_LEN`].
    #[error("frame length {len} exceeds the {MAX_FRAME_LEN}-byte cap")]
    FrameTooLarge {
        /// The declared length.
        len: u32,
    },
    /// Cumulative `audio` payload passed [`MAX_TOTAL_AUDIO_BYTES`].
    #[error("response audio exceeds the {MAX_TOTAL_AUDIO_BYTES}-byte cap")]
    TotalAudioTooLarge,
    /// The leading byte was not a known frame kind.
    #[error("unknown frame kind {kind:#04x}")]
    UnknownKind {
        /// The unrecognized discriminant.
        kind: u8,
    },
    /// The stream ended mid-frame.
    #[error("stream ended mid-frame ({have} of {want} bytes)")]
    Truncated {
        /// Bytes available.
        have: usize,
        /// Bytes the frame declared.
        want: usize,
    },
    /// The stream ended without a `done` or `error` frame.
    ///
    /// Distinguished from [`Self::Truncated`] on purpose: a backend that dies
    /// cleanly between frames would otherwise look like a short but successful
    /// utterance, and the user would hear speech cut off with no error.
    #[error("stream ended without a terminal frame")]
    MissingTerminal,
    /// Bytes arrived after a terminal frame.
    #[error("bytes followed the terminal frame")]
    TrailingData,
}

/// Encode one frame. Used by tests, fixtures, and any in-tree backend.
///
/// # Panics
/// Panics if `payload` is longer than [`MAX_FRAME_LEN`] — an encoder is our own
/// code, so an oversized frame is a bug to fix rather than input to tolerate.
#[must_use]
pub fn encode(kind: FrameKind, payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).expect("frame payload fits u32");
    assert!(len <= MAX_FRAME_LEN, "frame payload exceeds MAX_FRAME_LEN");
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(kind as u8);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Incremental reader over a framed synthesis body.
///
/// Feed bytes with [`push`](Self::push) as they arrive and drain whole frames
/// with [`next_frame`](Self::next_frame). Call [`finish`](Self::finish) once the
/// body ends to catch a stream that stopped early.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
    /// Read offset into `buf`; compacted lazily so a long stream does not
    /// memmove the whole buffer on every frame.
    pos: usize,
    audio_bytes: u64,
    saw_terminal: bool,
}

impl FrameDecoder {
    /// A decoder with an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append newly arrived bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Whether a terminal frame has been read.
    #[must_use]
    pub const fn saw_terminal(&self) -> bool {
        self.saw_terminal
    }

    /// Pull the next complete frame.
    ///
    /// Returns `Ok(None)` when more bytes are needed. A terminal frame is
    /// returned like any other; afterwards the decoder reports
    /// [`FrameError::TrailingData`] rather than continuing to parse, since a
    /// backend still talking after `done` is not a stream we understand.
    ///
    /// # Errors
    /// See [`FrameError`].
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        let avail = self.buf.len() - self.pos;
        if self.saw_terminal {
            return if avail == 0 {
                Ok(None)
            } else {
                Err(FrameError::TrailingData)
            };
        }
        if avail < 5 {
            return Ok(None);
        }
        let head = &self.buf[self.pos..self.pos + 5];
        let kind =
            FrameKind::from_byte(head[0]).ok_or(FrameError::UnknownKind { kind: head[0] })?;
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]);
        if len > MAX_FRAME_LEN {
            return Err(FrameError::FrameTooLarge { len });
        }
        let len = len as usize;
        if avail - 5 < len {
            return Ok(None);
        }

        let start = self.pos + 5;
        let payload = self.buf[start..start + len].to_vec();
        self.pos = start + len;

        if kind == FrameKind::Audio {
            self.audio_bytes += len as u64;
            if self.audio_bytes > MAX_TOTAL_AUDIO_BYTES {
                return Err(FrameError::TotalAudioTooLarge);
            }
        }
        if kind.is_terminal() {
            self.saw_terminal = true;
        }

        // Reclaim consumed bytes once they dominate the buffer, so a long
        // stream stays O(live bytes) instead of growing without bound.
        if self.pos > 64 * 1024 && self.pos * 2 >= self.buf.len() {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        Ok(Some(Frame { kind, payload }))
    }

    /// Assert the stream ended cleanly.
    ///
    /// # Errors
    /// [`FrameError::MissingTerminal`] if no `done`/`error` frame was read, or
    /// [`FrameError::Truncated`] if bytes remain that never formed a frame.
    pub fn finish(&self) -> Result<(), FrameError> {
        let rest = self.buf.len() - self.pos;
        if !self.saw_terminal {
            return if rest == 0 {
                Err(FrameError::MissingTerminal)
            } else {
                let want = if rest >= 5 {
                    let h = &self.buf[self.pos..self.pos + 5];
                    5 + u32::from_le_bytes([h[1], h[2], h[3], h[4]]) as usize
                } else {
                    5
                };
                Err(FrameError::Truncated { have: rest, want })
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_frame(samples: &[f32]) -> Vec<u8> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        encode(FrameKind::Audio, &bytes)
    }

    fn done() -> Vec<u8> {
        encode(FrameKind::Done, b"{}")
    }

    fn drain(dec: &mut FrameDecoder) -> Result<Vec<Frame>, FrameError> {
        let mut out = Vec::new();
        while let Some(f) = dec.next_frame()? {
            out.push(f);
        }
        Ok(out)
    }

    #[test]
    fn round_trips_audio_mark_and_done() {
        let mark = Mark {
            start_ms: Some(120),
            end_ms: Some(380),
            start_char: Some(6),
            end_char: Some(11),
        };
        let mut wire = audio_frame(&[0.25, -0.5]);
        wire.extend(encode(FrameKind::Mark, &serde_json::to_vec(&mark).unwrap()));
        wire.extend(done());

        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        let frames = drain(&mut dec).expect("well-formed stream decodes");
        dec.finish().expect("stream is terminated");

        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].kind, FrameKind::Audio);
        assert_eq!(
            SampleFormat::F32Le.decode(&frames[0].payload),
            vec![0.25, -0.5]
        );
        assert_eq!(frames[1].kind, FrameKind::Mark);
        assert_eq!(
            serde_json::from_slice::<Mark>(&frames[1].payload).unwrap(),
            mark
        );
        assert_eq!(frames[2].kind, FrameKind::Done);
    }

    /// The decoder must not depend on how the transport split the bytes: a
    /// byte-at-a-time feed has to produce exactly the same frames as one push.
    #[test]
    fn decodes_identically_however_the_bytes_are_split() {
        let mut wire = audio_frame(&[1.0, 2.0, 3.0]);
        wire.extend(encode(FrameKind::Mark, br#"{"start_ms":0}"#));
        wire.extend(done());

        let mut whole = FrameDecoder::new();
        whole.push(&wire);
        let expected = drain(&mut whole).unwrap();

        let mut drip = FrameDecoder::new();
        let mut got = Vec::new();
        for b in &wire {
            drip.push(&[*b]);
            while let Some(f) = drip.next_frame().unwrap() {
                got.push(f);
            }
        }
        drip.finish().unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn rejects_a_frame_longer_than_the_cap() {
        let mut wire = vec![FrameKind::Audio as u8];
        wire.extend_from_slice(&(MAX_FRAME_LEN + 1).to_le_bytes());
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(
            dec.next_frame().unwrap_err(),
            FrameError::FrameTooLarge {
                len: MAX_FRAME_LEN + 1
            }
        );
    }

    /// The cap must bite on the running total, not just per frame — otherwise a
    /// backend streams unbounded audio in legal-sized pieces.
    #[test]
    fn rejects_cumulative_audio_past_the_total_cap() {
        let chunk = vec![0_u8; MAX_FRAME_LEN as usize];
        let mut dec = FrameDecoder::new();
        let frames_to_exceed = (MAX_TOTAL_AUDIO_BYTES / u64::from(MAX_FRAME_LEN)) + 1;
        let mut err = None;
        for _ in 0..frames_to_exceed {
            dec.push(&encode(FrameKind::Audio, &chunk));
            match dec.next_frame() {
                Ok(_) => {}
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        assert_eq!(err, Some(FrameError::TotalAudioTooLarge));
    }

    #[test]
    fn rejects_an_unknown_frame_kind() {
        let mut wire = vec![0x7f_u8];
        wire.extend_from_slice(&0_u32.to_le_bytes());
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(
            dec.next_frame().unwrap_err(),
            FrameError::UnknownKind { kind: 0x7f }
        );
    }

    #[test]
    fn a_stream_ending_mid_frame_is_truncated() {
        let wire = audio_frame(&[1.0, 2.0]);
        let mut dec = FrameDecoder::new();
        dec.push(&wire[..wire.len() - 3]);
        assert_eq!(drain(&mut dec).unwrap(), vec![]);
        assert!(matches!(
            dec.finish().unwrap_err(),
            FrameError::Truncated { .. }
        ));
    }

    /// A clean stop between frames is the dangerous case: without this check a
    /// half-spoken utterance looks like a successful short one.
    #[test]
    fn a_stream_ending_between_frames_is_missing_its_terminal() {
        let mut dec = FrameDecoder::new();
        dec.push(&audio_frame(&[1.0]));
        drain(&mut dec).unwrap();
        assert_eq!(dec.finish().unwrap_err(), FrameError::MissingTerminal);
    }

    #[test]
    fn an_empty_stream_is_missing_its_terminal() {
        assert_eq!(
            FrameDecoder::new().finish().unwrap_err(),
            FrameError::MissingTerminal
        );
    }

    #[test]
    fn rejects_bytes_after_the_terminal_frame() {
        let mut wire = done();
        wire.extend(audio_frame(&[1.0]));
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        assert_eq!(dec.next_frame().unwrap().unwrap().kind, FrameKind::Done);
        assert_eq!(dec.next_frame().unwrap_err(), FrameError::TrailingData);
    }

    #[test]
    fn an_error_frame_terminates_the_stream() {
        let mut dec = FrameDecoder::new();
        dec.push(&encode(FrameKind::Error, br#"{"message":"boom"}"#));
        let f = dec.next_frame().unwrap().unwrap();
        assert_eq!(f.kind, FrameKind::Error);
        assert!(f.kind.is_terminal());
        dec.finish().expect("an error frame is a valid terminator");
    }

    #[test]
    fn zero_length_frames_are_legal() {
        let mut wire = encode(FrameKind::Audio, b"");
        wire.extend(encode(FrameKind::Done, b""));
        let mut dec = FrameDecoder::new();
        dec.push(&wire);
        let frames = drain(&mut dec).unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames[0].payload.is_empty());
        dec.finish().unwrap();
    }

    #[test]
    fn s16le_decodes_to_normalized_floats() {
        let bytes: Vec<u8> = [0_i16, 16384, -16384]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let got = SampleFormat::S16Le.decode(&bytes);
        assert_eq!(got, vec![0.0, 0.5, -0.5]);
        assert_eq!(SampleFormat::S16Le.bytes_per_sample(), 2);
        assert_eq!(SampleFormat::F32Le.bytes_per_sample(), 4);
    }

    /// Frame boundaries are the backend's choice and need not land on a sample
    /// boundary, so a trailing partial sample is dropped rather than producing
    /// a garbage value from zero-padding.
    #[test]
    fn a_trailing_partial_sample_is_dropped() {
        assert_eq!(SampleFormat::F32Le.decode(&[0, 0, 0]), Vec::<f32>::new());
        assert_eq!(SampleFormat::S16Le.decode(&[0]), Vec::<f32>::new());
    }

    #[test]
    fn sample_format_round_trips_through_its_wire_spelling() {
        for f in [SampleFormat::F32Le, SampleFormat::S16Le] {
            assert_eq!(f.as_str().parse::<SampleFormat>().unwrap(), f);
        }
        assert!("f64le".parse::<SampleFormat>().is_err());
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<&'static str> {
        let owned: Vec<_> = pairs.to_vec();
        move |name: &str| owned.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
    }

    #[test]
    fn reads_audio_params_from_headers() {
        let p = AudioParams::from_headers(headers(&[
            (HEADER_SAMPLE_RATE, "24000"),
            (HEADER_CHANNELS, "1"),
            (HEADER_FORMAT, "s16le"),
        ]))
        .expect("well-formed headers parse");
        assert_eq!(
            p,
            AudioParams {
                sample_rate: 24000,
                channels: 1,
                format: SampleFormat::S16Le,
            }
        );
    }

    #[test]
    fn rejects_missing_malformed_and_implausible_params() {
        let cases: &[(&[(&'static str, &'static str)], &str)] = &[
            (
                &[(HEADER_CHANNELS, "1"), (HEADER_FORMAT, "f32le")],
                "missing rate",
            ),
            (
                &[
                    (HEADER_SAMPLE_RATE, "many"),
                    (HEADER_CHANNELS, "1"),
                    (HEADER_FORMAT, "f32le"),
                ],
                "unparseable rate",
            ),
            (
                &[
                    (HEADER_SAMPLE_RATE, "1000"),
                    (HEADER_CHANNELS, "1"),
                    (HEADER_FORMAT, "f32le"),
                ],
                "rate below range",
            ),
            (
                &[
                    (HEADER_SAMPLE_RATE, "24000"),
                    (HEADER_CHANNELS, "9"),
                    (HEADER_FORMAT, "f32le"),
                ],
                "too many channels",
            ),
            (
                &[
                    (HEADER_SAMPLE_RATE, "24000"),
                    (HEADER_CHANNELS, "1"),
                    (HEADER_FORMAT, "mp3"),
                ],
                "unknown format",
            ),
        ];
        for (hs, label) in cases {
            assert!(
                matches!(
                    AudioParams::from_headers(headers(hs)),
                    Err(FrameError::BadParams(_))
                ),
                "{label} must be rejected"
            );
        }
    }
}
