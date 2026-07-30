// SPDX-License-Identifier: GPL-3.0-only
//! Shared `/v1/transcribe` request/response plumbing for the backend hosts.
//!
//! The wasm and subprocess hosts differ only in transport (an in-process
//! component invocation vs a Unix-socket HTTP dial). The request-body build and
//! the response parsing were byte-identical in both — and had started to drift —
//! so they live here. Feature-agnostic: compiled whenever either backend is on.

use anyhow::{Result, anyhow, bail};

/// Serialize a `/v1/transcribe` request body: `audio` samples embedded as-is at
/// `sample_rate` (the caller resamples first if its transport needs a fixed
/// rate), plus an optional `language` override.
///
/// # Errors
/// Returns an error if JSON serialization fails (not expected for this shape).
pub(crate) fn build_transcribe_body(
    audio: &[f32],
    sample_rate: u32,
    language: Option<&str>,
) -> Result<Vec<u8>> {
    let mut body = serde_json::json!({
        "audio_data": audio,
        "sample_rate": sample_rate,
    });
    if let Some(lang) = language {
        body["language"] = serde_json::Value::String(lang.to_string());
    }
    Ok(serde_json::to_vec(&body)?)
}

/// Parse a `/v1/transcribe` response: the transcript on `200`, else the
/// backend's own `detail`/`message` surfaced as the error (it's shown to the
/// user, so prefer it over the raw HTTP body).
///
/// # Errors
/// Returns an error if the body isn't JSON, a `200` is missing `transcription`,
/// or a non-`200` carries a backend error message.
pub(crate) fn parse_transcribe_response(status: u16, resp: &[u8]) -> Result<String> {
    let json: serde_json::Value = serde_json::from_slice(resp)
        .map_err(|e| anyhow!("parsing backend transcribe response: {e}"))?;
    if status == 200 {
        json["transcription"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow!("backend response missing transcription"))
    } else {
        let msg = json
            .get("detail")
            .and_then(|v| v.as_str())
            .or_else(|| json.get("message").and_then(|v| v.as_str()))
            .unwrap_or("transcription failed");
        bail!("{msg}");
    }
}

#[cfg(test)]
mod tests {
    use super::{build_transcribe_body, parse_transcribe_response};

    #[test]
    fn body_embeds_audio_rate_and_optional_language() {
        let with_lang = build_transcribe_body(&[0.1, -0.2], 16000, Some("en")).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&with_lang).unwrap();
        assert_eq!(v["sample_rate"], 16000);
        assert_eq!(v["audio_data"].as_array().unwrap().len(), 2);
        assert_eq!(v["language"], "en");

        let no_lang = build_transcribe_body(&[], 8000, None).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&no_lang).unwrap();
        assert!(v.get("language").is_none());
    }

    #[test]
    fn parse_returns_transcription_on_200() {
        let text = parse_transcribe_response(200, br#"{"transcription":"hello"}"#).unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn parse_surfaces_detail_then_message_on_error() {
        let e = parse_transcribe_response(500, br#"{"detail":"oom","message":"m"}"#).unwrap_err();
        assert_eq!(e.to_string(), "oom");
        let e = parse_transcribe_response(500, br#"{"message":"just message"}"#).unwrap_err();
        assert_eq!(e.to_string(), "just message");
        let e = parse_transcribe_response(500, br"{}").unwrap_err();
        assert_eq!(e.to_string(), "transcription failed");
    }

    #[test]
    fn parse_errors_on_200_without_transcription() {
        assert!(parse_transcribe_response(200, br#"{"other":1}"#).is_err());
    }
}
